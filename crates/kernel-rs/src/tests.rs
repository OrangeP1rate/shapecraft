use std::sync::Arc;

use serde_json::json;
use shapecraft_kernel::{Axis, DVec3, Kernel, SubShape};

use crate::{PolyKernel, RustKernel};

const K: RustKernel = RustKernel;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

#[test]
fn a_box_measures_like_a_box() {
    let b = K.make_box(DVec3::new(1.0, 2.0, 3.0), DVec3::new(10.0, 20.0, 5.0)).unwrap();
    let mp = K.mass_props(&b).unwrap();
    assert!(close(mp.volume, 1000.0), "{}", mp.volume);
    assert!(close(mp.area, 2.0 * (200.0 + 50.0 + 100.0)), "{}", mp.area);
    assert!(mp.centroid.distance(DVec3::new(6.0, 12.0, 5.5)) < 1e-9, "{}", mp.centroid);
    assert_eq!((K.face_count(&b).unwrap(), K.edge_count(&b).unwrap()), (6, 12));
    let bb = K.bounds(&b).unwrap();
    assert_eq!((bb.min, bb.max), (DVec3::new(1.0, 2.0, 3.0), DVec3::new(11.0, 22.0, 8.0)));
    // Every face's normal points out of the box.
    for f in 0..6 {
        let fp = K.face_plane(&b, f).unwrap().unwrap();
        assert!((fp.origin - mp.centroid).dot(fp.normal) > 0.0, "face {f} faces in");
        assert_eq!(K.face_edges(&b, f).unwrap().len(), 4);
    }
    let mesh = K.tessellate(&b, 0.1).unwrap();
    assert!(mesh.triangle_count() >= 12);
    assert_eq!(mesh.edges.len(), 12);
    assert!(mesh.edge_faces.iter().all(|f| f[0] != u32::MAX && f[1] != u32::MAX), "each edge between two faces");
}

#[test]
fn an_l_shape_extrudes_and_moves() {
    // An L of area 3 (concave corner), extruded 2 either way round: volume 6.
    let l = [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0], [1.0, 2.0], [0.0, 2.0]].map(|[x, y]| DVec3::new(x, y, 0.0));
    for (pts, dir) in [(l.to_vec(), DVec3::Z * 2.0), (l.iter().rev().copied().collect(), DVec3::Z * 2.0), (l.to_vec(), DVec3::Z * -2.0)] {
        let s = K.extrude_polygon(&pts, dir).unwrap();
        assert!(close(K.mass_props(&s).unwrap().volume, 6.0));
        assert_eq!(K.face_count(&s).unwrap(), 8);
    }
    let s = K.extrude_polygon(&l, DVec3::Z).unwrap();
    let mirrored = K.mirror(&s, DVec3::ZERO, DVec3::X).unwrap();
    assert!(close(K.mass_props(&mirrored).unwrap().volume, 3.0), "a mirror keeps the volume positive");
    let turned = K.rotate(&s, Axis::new(DVec3::ZERO, DVec3::Z), std::f64::consts::FRAC_PI_2).unwrap();
    assert!(K.bounds(&turned).unwrap().max.x.abs() < 1e-12);
    assert!(K.scale(&s, DVec3::ZERO, DVec3::new(2.0, 1.0, 3.0)).is_err(), "scaling is not in it yet, and says so");
    let (len, mid) = K.measure(&s, SubShape::Edge(0)).unwrap();
    assert!(len > 0.0 && mid.is_finite());
}

#[test]
fn closed_meshes_are_solids() {
    // A tetrahedron, outward triangles.
    let p = [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z];
    let tet = K.shape_from_mesh(&p, &[0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2]).unwrap();
    assert!(close(K.mass_props(&tet).unwrap().volume, 1.0 / 6.0));
    assert_eq!(K.solids(&tet).unwrap().len(), 1);
    let _ = PolyKernel;
    let open = K.shape_from_mesh(&p, &[0, 2, 1]).unwrap();
    assert!(K.solids(&open).unwrap().is_empty());
}

#[test]
fn what_it_cant_do_yet_says_so() {
    let b = K.make_box(DVec3::ZERO, DVec3::ONE).unwrap();
    let e = K.imprint(&b, 0, &[]).unwrap_err().to_string();
    assert!(e.contains("not in the Rust kernel yet"), "{e}");
    let occt_made = shapecraft_kernel::Shape::new(Foreign);
    assert!(K.mass_props(&occt_made).is_err(), "another kernel's shape is refused, not misread");
}

struct Foreign;
impl shapecraft_kernel::ShapeData for Foreign {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The whole engine on this kernel: sketch a rectangle, extrude it, look at it, move it, export it as STL.
#[test]
fn the_engine_models_on_the_rust_kernel() {
    let mut s = shapecraft_engine::Session::new(Arc::new(RustKernel), "Rust");
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [20, 10]})).unwrap();
    let sketch = s.active_sketch.unwrap();
    s.execute("extrude", &json!({"sketch": sketch.0, "region": 0, "distance": 5})).unwrap();
    assert_eq!(s.doc.bodies.len(), 1);
    let id = s.doc.bodies[0].id.0;
    let info = s.execute("inspect", &json!({"body": id, "faces": false})).unwrap().value;
    let v = info["bodies"][0]["volume_mm3"].as_f64().unwrap();
    assert!(close(v, 1000.0), "{v}");
    assert_eq!(info["bodies"][0]["face_count"], 6);
    s.execute("translate", &json!({"by": [5, 0, 0], "items": [id]})).unwrap();
    let info = s.execute("inspect", &json!({"body": id, "faces": false})).unwrap().value;
    assert!(close(info["bodies"][0]["bounds"]["min"][0].as_f64().unwrap(), 5.0), "{}", info["bodies"][0]["bounds"]);
    let stl = std::env::temp_dir().join(format!("shapecraft-rust-kernel-{}.stl", std::process::id()));
    s.execute("export_stl", &json!({"path": stl.to_string_lossy()})).unwrap();
    assert_eq!(std::fs::metadata(&stl).unwrap().len(), 84 + 12 * 50);
    let _ = std::fs::remove_file(&stl);
    // A circle is a cylinder now; a hole through the block is a boolean.
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("circle", &json!({"center": [10, 5], "radius": 2})).unwrap();
    let sk = s.active_sketch.unwrap();
    let disc = s.doc.sketch(sk).unwrap().regions().iter().find(|r| r.area < 20.0).map(|r| r.index).unwrap();
    s.execute("extrude", &json!({"sketch": sk.0, "region": disc, "distance": 5})).unwrap();
    assert_eq!(s.doc.bodies.len(), 1, "the disc joins the block");
}

#[test]
fn packing_keeps_the_face_order() {
    // STEP face colours and from_brep's face map number faces before a body is packed into its own model.
    let b = K.make_box(DVec3::ZERO, DVec3::new(1.0, 2.0, 3.0)).unwrap();
    let cut = K
        .boolean(shapecraft_kernel::BoolOp::Subtract, &b, &K.extrude_circle(DVec3::new(0.5, 1.0, -1.0), 0.3, DVec3::Z * 5.0).unwrap())
        .unwrap();
    let a = crate::exact(&cut).unwrap();
    let before: Vec<_> = a.model.faces(a.bodies[0]).unwrap().iter().map(|f| a.model.face(f.id).unwrap().surface()).collect();
    let packed = crate::pack(&a.model, &a.bodies).unwrap();
    let after: Vec<_> = packed
        .model
        .faces(packed.bodies[0])
        .unwrap()
        .iter()
        .map(|f| packed.model.surface(packed.model.face(f.id).unwrap().surface()).unwrap().clone())
        .collect();
    let before: Vec<_> = before.iter().map(|s| a.model.surface(*s).unwrap().clone()).collect();
    assert_eq!(before, after);
}
