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
    let scaled = K.scale(&s, DVec3::ZERO, DVec3::new(2.0, 1.0, 3.0)).unwrap();
    assert!(close(K.mass_props(&scaled).unwrap().volume, 18.0), "flat faces scale unevenly and stay exact");
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
    let e = K.fillet_edges_variable(&b, &[0], 0.1, 0.2).unwrap_err().to_string();
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

#[test]
fn an_uneven_scale_makes_a_cylinder_elliptic_and_keeps_it_exact() {
    let c = K.extrude_circle(DVec3::new(1.0, 0.0, 0.0), 2.0, DVec3::Z * 5.0).unwrap();
    let s = K.scale(&c, DVec3::ZERO, DVec3::new(2.0, 1.0, 3.0)).unwrap();
    let v = K.mass_props(&s).unwrap().volume;
    assert!(close(v, std::f64::consts::PI * 4.0 * 5.0 * 6.0), "{v}");
    let bb = K.bounds(&s).unwrap();
    assert!(bb.min.distance(DVec3::new(-2.0, -2.0, 0.0)) < 1e-6 && bb.max.distance(DVec3::new(6.0, 2.0, 15.0)) < 1e-6, "{bb:?}");
    let sphere = K.revolve_circle(
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::Y,
        1.0,
        shapecraft_kernel::Axis::new(DVec3::ZERO, DVec3::Z),
        std::f64::consts::PI,
    );
    if let Ok(sphere) = sphere {
        assert!(K.scale(&sphere, DVec3::ZERO, DVec3::new(1.0, 2.0, 1.0)).is_err(), "an ellipsoid has no exact form here");
        assert!(close(
            K.mass_props(&K.scale(&sphere, DVec3::ZERO, DVec3::splat(2.0)).unwrap()).unwrap().volume,
            32.0 / 3.0 * std::f64::consts::PI
        ));
    }
}

#[test]
fn a_loft_lines_its_sections_up_whatever_corner_they_start_at() {
    use shapecraft_kernel::{PlanarCurve, PlaneFrame};
    let sq = |z: f64, h: f64, rot: usize| {
        let mut p = vec![DVec3::new(-h, -h, z), DVec3::new(h, -h, z), DVec3::new(h, h, z), DVec3::new(-h, h, z)];
        p.rotate_left(rot);
        let curves: Vec<PlanarCurve> = (0..4).map(|i| PlanarCurve::Line { a: p[i], b: p[(i + 1) % 4] }).collect();
        K.planar_regions(PlaneFrame { origin: DVec3::new(0.0, 0.0, z), x: DVec3::X, y: DVec3::Y }, &curves).unwrap().remove(0)
    };
    for rot in 0..4 {
        let l = K.loft(&[sq(0.0, 5.0, 0), sq(10.0, 2.5, rot)]).unwrap();
        let frustum = 10.0 / 3.0 * (100.0 + 25.0 + 50.0);
        assert!(close(K.mass_props(&l).unwrap().volume, frustum), "started at corner {rot}");
    }
}

#[test]
fn two_boxes_side_by_side_fuse_into_one_box_of_six_faces() {
    let a = K.make_box(DVec3::ZERO, DVec3::new(10.0, 10.0, 5.0)).unwrap();
    let b = K.make_box(DVec3::new(10.0, 0.0, 0.0), DVec3::new(10.0, 10.0, 5.0)).unwrap();
    let both = K.boolean(shapecraft_kernel::BoolOp::Union, &a, &b).unwrap();
    assert!(close(K.mass_props(&both).unwrap().volume, 1000.0));
    assert_eq!(K.face_count(&both).unwrap(), 6, "no seam where they met");
}

#[test]
fn regions_split_by_a_line_merge_back_into_one_circle() {
    use shapecraft_kernel::{PlanarCurve, PlaneFrame};
    let frame = PlaneFrame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y };
    let curves = [
        PlanarCurve::Line { a: DVec3::new(-12.0, 3.0, 0.0), b: DVec3::new(12.0, 3.0, 0.0) },
        PlanarCurve::Circle { center: DVec3::ZERO, radius: 5.0 },
    ];
    let regions = K.planar_regions(frame, &curves).unwrap();
    assert_eq!(regions.len(), 2);
    let merged = K.merge_faces(&regions).unwrap();
    assert_eq!(merged.len(), 1);
    let solid = K.prism(&merged[0], DVec3::Z * 2.0).unwrap();
    assert_eq!(K.face_count(&solid).unwrap(), 3, "a plain cylinder");
    assert!(close(K.mass_props(&solid).unwrap().volume, std::f64::consts::PI * 50.0));
}

#[test]
fn a_rounded_edge_is_a_convex_blend_and_a_hole_wall_is_none() {
    let b = K.make_box(DVec3::ZERO, DVec3::new(10.0, 10.0, 10.0)).unwrap();
    let holed = K
        .boolean(shapecraft_kernel::BoolOp::Subtract, &b, &K.extrude_circle(DVec3::new(5.0, 5.0, -1.0), 2.0, DVec3::Z * 12.0).unwrap())
        .unwrap();
    let blends = |s: &shapecraft_kernel::Shape| -> Vec<(f64, bool)> {
        (0..K.face_count(s).unwrap()).filter_map(|f| K.blend_of(s, f).unwrap()).collect()
    };
    assert!(blends(&holed).is_empty(), "a hole's wall meets its neighbours square");
    let edge = (0..K.edge_count(&holed).unwrap())
        .find(|&e| matches!(K.edge_curve(&holed, e).unwrap(), shapecraft_kernel::EdgeCurve::Line { a, b } if a.z.max(b.z) > 9.9 && (a - b).length() > 9.0))
        .unwrap();
    let rounded = K.fillet_edges(&holed, &[edge], 1.0).unwrap();
    let found = blends(&rounded);
    assert_eq!(found.len(), 1);
    assert!(close(found[0].0, 1.0) && found[0].1, "{found:?}");
}

#[test]
fn a_chordal_fillet_on_a_square_edge_has_radius_chord_over_root_two() {
    let b = K.make_box(DVec3::ZERO, DVec3::splat(10.0)).unwrap();
    let edge = (0..K.edge_count(&b).unwrap())
        .find(|&e| matches!(K.edge_curve(&b, e).unwrap(), shapecraft_kernel::EdgeCurve::Line { a, b } if a.z > 9.9 && b.z > 9.9))
        .unwrap();
    let chordal = K.fillet_edges_ex(&b, &[edge], 2.0, true, 0.0).unwrap();
    let radius = K.fillet_edges(&b, &[edge], 2.0 / 2f64.sqrt()).unwrap();
    assert!(close(K.mass_props(&chordal).unwrap().volume, K.mass_props(&radius).unwrap().volume));
    assert!(K.fillet_edges_ex(&b, &[edge], 2.0, false, 1.0).is_err(), "setback corners say they aren't here yet");
}

#[test]
fn what_the_table_calls_missing_is_refused() {
    use shapecraft_kernel::{Feature, Support};
    let b = K.make_box(DVec3::ZERO, DVec3::splat(10.0)).unwrap();
    let edge = 0;
    let refused = |f: Feature| -> bool {
        match f {
            Feature::VariableFillet => K.fillet_edges_variable(&b, &[edge], 1.0, 2.0).is_err(),
            Feature::SetbackCorners => K.fillet_edges_ex(&b, &[edge], 1.0, false, 0.5).is_err(),
            Feature::TwoDistanceChamfer => K.chamfer_edges_two(&b, &[edge], 1.0, 2.0, None).is_err(),
            Feature::MoveEdge => K.scale_edge(&b, edge, DVec3::ZERO, DVec3::splat(2.0)).is_err(),
            Feature::Fill => K.fill(&b, &[edge]).is_err(),
            Feature::WrapEmboss => K
                .wrap_emboss(&b, None, &[], shapecraft_kernel::PlaneFrame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y }, 1.0, 0.0, None)
                .is_err(),
            Feature::ProjectAsEdge => K.imprint(&b, 0, &[]).is_err(),
            Feature::Iges => K.write_iges(&[b.clone()], &std::env::temp_dir().join("never.igs")).is_err(),
            other => panic!("{other:?} is missing: add how it refuses here"),
        }
    };
    for f in Feature::ALL {
        if K.support(f) == Support::Missing {
            assert!(refused(f), "{f:?} is listed missing but works: list it as Full or Limited");
        }
    }
}

/// The edge of `s` whose middle is nearest `p`.
fn edge_near(s: &shapecraft_kernel::Shape, p: DVec3) -> u32 {
    (0..K.edge_count(s).unwrap())
        .min_by(|a, b| {
            let d = |e: u32| K.measure(s, SubShape::Edge(e)).unwrap().1.distance(p);
            d(*a).total_cmp(&d(*b))
        })
        .unwrap()
}

#[test]
fn fill_closes_an_exact_open_box() {
    let cube = K.make_box(DVec3::ZERO, DVec3::splat(10.0)).unwrap();
    let mut b = K.to_brep(&cube).unwrap().remove(0);
    // Take the top away: an open box (a sheet).
    let top = (0..b.faces.len()).find(|f| matches!(b.surfaces[b.faces[*f].surface], shapecraft_kernel::brep::Surface::Plane { origin, normal, .. } if normal.z.abs() > 0.9 && origin.z > 5.0)).unwrap();
    b.faces.remove(top);
    b.shells = vec![(0..b.faces.len()).collect()];
    b.kind = shapecraft_kernel::brep::BodyKind::Sheet;
    let cup = K.from_brep(&b).unwrap();
    let rim: Vec<u32> =
        (0..K.edge_count(&cup).unwrap()).filter(|e| (K.measure(&cup, SubShape::Edge(*e)).unwrap().1.z - 10.0).abs() < 1e-9).collect();
    assert_eq!(rim.len(), 4);
    let closed = K.fill(&cup, &rim).unwrap();
    assert!(close(K.mass_props(&closed).unwrap().volume, 1000.0));
    assert_eq!(K.face_count(&closed).unwrap(), 6);
    // An edge with faces on both sides isn't round an opening.
    assert!(K.fill(&cube, &[0]).is_err());
}

#[test]
fn a_rim_moves_scales_and_turns_on_its_own() {
    let pi = std::f64::consts::PI;
    let post = K.extrude_circle(DVec3::ZERO, 4.0, DVec3::Z * 10.0).unwrap();
    let top = edge_near(&post, DVec3::new(0.0, 0.0, 10.0));
    let vol = |s: &shapecraft_kernel::Shape| K.mass_props(s).unwrap().volume;
    // Lifted 5: a taller cylinder.
    let taller = K.map_edge(&post, top, shapecraft_kernel::DMat3::IDENTITY, DVec3::Z * 5.0).unwrap();
    assert!(close(vol(&taller), pi * 16.0 * 15.0), "{}", vol(&taller));
    // Doubled about its centre: a frustum, still a cone.
    let c = DVec3::new(0.0, 0.0, 10.0);
    let frustum = K.scale_edge(&post, top, c, DVec3::splat(2.0)).unwrap();
    assert!((vol(&frustum) / (pi * 10.0 / 3.0 * (16.0 + 32.0 + 64.0)) - 1.0).abs() < 1e-9, "{}", vol(&frustum));
    // Wider one way only: the side ruled to an ellipse (π·4·(4+6)/2·10 = 200π).
    let oval = K.scale_edge(&post, top, c, DVec3::new(1.5, 1.0, 1.0)).unwrap();
    assert!((vol(&oval) / (200.0 * pi) - 1.0).abs() < 1e-4, "{}", vol(&oval));
    let bb = K.bounds(&oval).unwrap();
    assert!((bb.max.x - 6.0).abs() < 1e-6 && (bb.max.y - 4.0).abs() < 1e-6, "{bb:?}");
    // Turned 15° about a line through its centre: same height on average, close to the same volume.
    let turn = shapecraft_kernel::DMat3::from_axis_angle(DVec3::X, 15f64.to_radians());
    let tilted = K.map_edge(&post, top, turn, c - turn * c).unwrap();
    assert!((vol(&tilted) / (pi * 160.0) - 1.0).abs() < 0.02, "{}", vol(&tilted));
    // A boss on a plate: its top scaled down, the plate stays.
    let plate = K.make_box(DVec3::new(-10.0, -10.0, -2.0), DVec3::new(20.0, 20.0, 2.0)).unwrap();
    let boss = K.boolean(shapecraft_kernel::BoolOp::Union, &plate, &post).unwrap();
    let top = edge_near(&boss, c);
    let tapered = K.scale_edge(&boss, top, c, DVec3::splat(0.5)).unwrap();
    let cone = pi * 10.0 / 3.0 * (16.0 + 8.0 + 4.0);
    assert!((vol(&tapered) / (800.0 + cone) - 1.0).abs() < 1e-9, "{}", vol(&tapered));
    // A straight edge isn't a rim; a fold is refused.
    assert!(K.map_edge(&post, top, shapecraft_kernel::DMat3::ZERO, DVec3::ZERO).is_err());
}

#[test]
fn two_distance_chamfers_on_straight_edges_and_rims() {
    let vol = |s: &shapecraft_kernel::Shape| K.mass_props(s).unwrap().volume;
    // A 10 mm cube's top front edge: 1 along the top, 3 down the front (the cube's face 0..6 order decides "first").
    let cube = K.make_box(DVec3::ZERO, DVec3::splat(10.0)).unwrap();
    let e = edge_near(&cube, DVec3::new(5.0, 0.0, 10.0));
    let top = (0..6).find(|f| K.face_plane(&cube, *f).unwrap().unwrap().normal.z > 0.9).unwrap();
    let cut = K.chamfer_edges_two(&cube, &[e], 1.0, 3.0, Some(top)).unwrap();
    assert!(close(vol(&cut), 985.0), "{}", vol(&cut));
    assert_eq!(K.face_count(&cut).unwrap(), 7);
    // The bevel runs from 1 back on the top to 3 down the front: its normal is (0, -3, 1) normalised.
    let bevel = (0..7).filter_map(|f| K.face_plane(&cut, f).unwrap()).find(|p| p.normal.y < -0.1 && p.normal.z > 0.1).unwrap();
    assert!((bevel.normal.y / bevel.normal.z - (-3.0)).abs() < 1e-9, "{:?}", bevel.normal);
    // Two edges meeting at a corner.
    let side = edge_near(&cube, DVec3::new(10.0, 5.0, 10.0));
    let both = K.chamfer_edges_two(&cube, &[e, side], 1.0, 2.0, None).unwrap();
    assert!(vol(&both) < 1000.0 - 2.0 * 9.0, "{}", vol(&both));
    // A post's top rim: 1 across the top, 0.5 down the side (a ring of triangles: 2π·(3 − 1/3)·0.25).
    let post = K.extrude_circle(DVec3::ZERO, 3.0, DVec3::Z * 6.0).unwrap();
    let rim = edge_near(&post, DVec3::new(0.0, 0.0, 6.0));
    let cap = (0..K.face_count(&post).unwrap()).find(|f| K.face_plane(&post, *f).unwrap().is_some_and(|p| p.normal.z > 0.9)).unwrap();
    let ring = K.chamfer_edges_two(&post, &[rim], 1.0, 0.5, Some(cap)).unwrap();
    let removed = std::f64::consts::TAU * (3.0 - 1.0 / 3.0) * 0.25;
    assert!((vol(&post) - vol(&ring) - removed).abs() < 1e-9, "{}", vol(&post) - vol(&ring));
    // An edge that runs into a wall (the top of a step) is refused, not cut into the wall.
    let step = K
        .boolean(
            shapecraft_kernel::BoolOp::Union,
            &K.make_box(DVec3::ZERO, DVec3::new(20.0, 10.0, 5.0)).unwrap(),
            &K.make_box(DVec3::new(10.0, 0.0, 0.0), DVec3::new(10.0, 10.0, 10.0)).unwrap(),
        )
        .unwrap();
    let low = edge_near(&step, DVec3::new(5.0, 0.0, 5.0));
    assert!(K.chamfer_edges_two(&step, &[low], 1.0, 2.0, None).is_err());
}

#[test]
fn outlines_projected_onto_a_face_split_it() {
    use shapecraft_kernel::BoolOp;
    let pi = std::f64::consts::PI;
    let block = K.make_box(DVec3::ZERO, DVec3::new(20.0, 20.0, 10.0)).unwrap();
    let top_of = |s: &shapecraft_kernel::Shape, z: f64| {
        (0..K.face_count(s).unwrap())
            .filter(|f| K.face_plane(s, *f).unwrap().is_some_and(|p| p.normal.z > 0.9 && (p.origin.z - z).abs() < 1e-9))
            .collect::<Vec<u32>>()
    };
    let top = top_of(&block, 10.0)[0];
    // A ring (a tube's end face) 20 mm above, over the middle of the top.
    let tube = K
        .boolean(
            BoolOp::Subtract,
            &K.extrude_circle(DVec3::new(10.0, 10.0, 25.0), 6.0, DVec3::Z * 5.0).unwrap(),
            &K.extrude_circle(DVec3::new(10.0, 10.0, 24.0), 3.0, DVec3::Z * 7.0).unwrap(),
        )
        .unwrap();
    let ring = K.face(&tube, top_of(&tube, 30.0)[0]).unwrap();
    let split = K.imprint(&block, top, &[(ring.clone(), None)]).unwrap();
    assert_eq!(K.face_count(&split).unwrap(), 8, "the top, the ring and the disc inside it");
    assert!(close(K.mass_props(&split).unwrap().volume, 4000.0));
    // The ring pulled up 2 mm stands on the block.
    let ring_face =
        top_of(&split, 10.0).into_iter().find(|f| (K.measure(&split, SubShape::Face(*f)).unwrap().0 - pi * 27.0).abs() < 1e-6).unwrap();
    let raised = K.extrude_face(&split, ring_face, 2.0).unwrap();
    assert!((K.mass_props(&raised).unwrap().volume - (4000.0 + 2.0 * pi * 27.0)).abs() < 1e-6);
    // One that hangs over the edge is refused; one that misses the face lands nowhere.
    let corner = K.extrude_circle(DVec3::new(0.0, 0.0, 25.0), 4.0, DVec3::Z).unwrap();
    let over = K.face(&corner, top_of(&corner, 26.0)[0]).unwrap();
    assert!(K.imprint(&block, top, &[(over, None)]).is_err());
    let far = K.extrude_circle(DVec3::new(50.0, 50.0, 25.0), 4.0, DVec3::Z).unwrap();
    let away = K.face(&far, top_of(&far, 26.0)[0]).unwrap();
    assert!(K.imprint(&block, top, &[(away, None)]).is_err());
}
