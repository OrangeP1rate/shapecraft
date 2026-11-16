//! Solids written as transmit files and read back keep their volume (text and binary).
use shapecraft_kernel::{Axis, BoolOp, DVec3, Kernel};
use shapecraft_kernel_occt::OcctKernel;
use shapecraft_xt::Format;

fn shapes(k: &OcctKernel) -> Vec<(&'static str, shapecraft_kernel::Shape)> {
    let rect = vec![DVec3::ZERO, DVec3::new(20.0, 0.0, 0.0), DVec3::new(20.0, 10.0, 0.0), DVec3::new(0.0, 10.0, 0.0)];
    let boxy = k.extrude_polygon(&rect, DVec3::new(0.0, 0.0, 8.0)).unwrap();
    let hole = k.extrude_circle(DVec3::new(10.0, 5.0, -1.0), 3.0, DVec3::new(0.0, 0.0, 10.0)).unwrap();
    vec![
        ("box", boxy.clone()),
        ("cylinder", k.extrude_circle(DVec3::ZERO, 5.0, DVec3::new(0.0, 0.0, 12.0)).unwrap()),
        ("rounded box", k.fillet_all_edges(&boxy, 2.0).unwrap()),
        ("box with hole", k.boolean(BoolOp::Subtract, &boxy, &hole).unwrap()),
        (
            "cone",
            k.revolve_polygon(
                &[DVec3::ZERO, DVec3::new(6.0, 0.0, 0.0), DVec3::new(0.0, 0.0, 9.0)],
                Axis::new(DVec3::ZERO, DVec3::Z),
                std::f64::consts::TAU,
            )
            .unwrap(),
        ),
    ]
}

#[test]
fn written_solids_read_back() {
    let k = OcctKernel::new();
    for (name, s) in shapes(&k) {
        let v0 = k.mass_props(&s).unwrap().volume;
        let brep = k.to_brep(&s).unwrap().remove(0);
        for format in [Format::Binary, Format::Text] {
            let bytes = shapecraft_xt::write_body(&brep, format).unwrap_or_else(|e| panic!("{name}: {e}"));
            let file = shapecraft_xt::parse(&bytes).unwrap_or_else(|e| panic!("{name} {format:?}: {e}"));
            let bodies = shapecraft_xt::bodies(&file).unwrap_or_else(|e| panic!("{name} {format:?}: {e}"));
            assert_eq!(bodies.len(), 1);
            let back = k.from_brep(&bodies[0].brep).unwrap_or_else(|e| panic!("{name} {format:?}: {e}"));
            let v1 = k.mass_props(&back).unwrap().volume;
            assert!((v0 - v1).abs() < 1e-4 * v0, "{name} {format:?}: volume {v0} -> {v1}");
        }
    }
}

#[test]
fn periodic_faces_are_written_without_seams() {
    let k = OcctKernel::new();
    for (name, s) in shapes(&k) {
        let brep = k.to_brep(&s).unwrap().remove(0);
        let bytes = shapecraft_xt::write_body(&brep, Format::Binary).unwrap();
        let back = &shapecraft_xt::bodies(&shapecraft_xt::parse(&bytes).unwrap()).unwrap()[0].brep;
        // Edges as the modeller makes these solids: rings for full circles, no seam lines.
        let want = match name {
            "box" => 12,
            "cylinder" => 2,
            "rounded box" => brep.edges.len(),
            "box with hole" => 14,
            _ => brep.edges.len(),
        };
        assert_eq!(back.edges.len(), want, "{name}");
        if name == "cylinder" {
            assert!(back.vertices.is_empty() && back.faces.iter().any(|f| f.loops.len() == 2), "{name}: rings on a two-loop face");
        }
    }
}

/// Two boxes apart, as one shape (a compound of two solids).
fn two_lumps(k: &OcctKernel) -> shapecraft_kernel::Shape {
    let sq =
        |x: f64| vec![DVec3::new(x, 0.0, 0.0), DVec3::new(x + 10.0, 0.0, 0.0), DVec3::new(x + 10.0, 10.0, 0.0), DVec3::new(x, 10.0, 0.0)];
    let a = k.extrude_polygon(&sq(0.0), DVec3::new(0.0, 0.0, 5.0)).unwrap();
    let b = k.extrude_polygon(&sq(30.0), DVec3::new(0.0, 0.0, 7.0)).unwrap();
    k.boolean(BoolOp::Union, &a, &b).unwrap()
}

#[test]
fn a_body_of_separate_solids_stays_one_body() {
    let k = OcctKernel::new();
    let s = two_lumps(&k);
    let parts = k.to_brep(&s).unwrap();
    assert_eq!(parts.len(), 2);
    let bytes = shapecraft_xt::write_solids(&parts, Format::Binary).unwrap();
    let bodies = shapecraft_xt::bodies(&shapecraft_xt::parse(&bytes).unwrap()).unwrap();
    assert_eq!(bodies.len(), 1, "one body with two solid regions");
    let back = k.from_brep(&bodies[0].brep).unwrap();
    let (v0, v1) = (k.mass_props(&s).unwrap().volume, k.mass_props(&back).unwrap().volume);
    assert!((v0 - 1200.0).abs() < 1e-6 && (v1 - v0).abs() < 1e-6, "volume {v0} -> {v1}");
    assert_eq!(k.to_brep(&back).unwrap().len(), 2);
}

#[test]
fn an_open_mesh_is_written_as_a_sheet() {
    let k = OcctKernel::new();
    // Two triangles of an open mesh (no shell around them until the kernel groups them).
    let pts = [DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 0.0, 0.0), DVec3::new(10.0, 10.0, 0.0), DVec3::new(0.0, 10.0, 1.0)];
    let s = k.shape_from_mesh(&pts, &[0, 1, 2, 0, 2, 3]).unwrap();
    let brep = k.to_brep(&s).unwrap().remove(0);
    assert_eq!(brep.kind, shapecraft_kernel::brep::BodyKind::Sheet);
    assert_eq!(brep.faces.len(), 2, "both triangles kept");
    for format in [Format::Binary, Format::Text] {
        let bytes = shapecraft_xt::write_body(&brep, format).unwrap();
        let back = &shapecraft_xt::bodies(&shapecraft_xt::parse(&bytes).unwrap()).unwrap()[0].brep;
        assert_eq!(back.kind, shapecraft_kernel::brep::BodyKind::Sheet);
        assert_eq!((back.faces.len(), back.edges.len()), (2, 5), "{format:?}");
        let area: f64 = (0..2).map(|i| k.measure(&k.from_brep(back).unwrap(), shapecraft_kernel::SubShape::Face(i)).unwrap().0).sum();
        let want = 50.0 + 0.5 * (DVec3::new(10.0, 10.0, 0.0).cross(DVec3::new(0.0, 10.0, 1.0))).length();
        assert!((area - want).abs() < 1e-6, "{format:?}: area {area} vs {want}");
    }
}

#[test]
fn a_wire_body_is_written_with_its_edges() {
    use shapecraft_kernel::brep::{BodyKind, Brep, Curve, Edge};
    let mut b = Brep { kind: BodyKind::Wire, ..Brep::default() };
    // An L of two lines and a separate line: two pieces.
    b.vertices =
        vec![DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(10.0, 5.0, 0.0), DVec3::new(0.0, 20.0, 0.0), DVec3::new(5.0, 20.0, 0.0)];
    for (s, e) in [(0, 1), (1, 2), (3, 4)] {
        let (p, q) = (b.vertices[s], b.vertices[e]);
        b.curves.push(Curve::Line { origin: p, dir: (q - p).normalize() });
        b.edges.push(Edge { curve: Some(b.curves.len() - 1), reversed: false, start: Some(s), end: Some(e), tolerance: None });
    }
    for format in [Format::Binary, Format::Text] {
        let bytes = shapecraft_xt::write_body(&b, format).unwrap();
        let back = &shapecraft_xt::bodies(&shapecraft_xt::parse(&bytes).unwrap()).unwrap()[0].brep;
        assert_eq!(back.kind, BodyKind::Wire, "{format:?}");
        assert_eq!((back.edges.len(), back.vertices.len()), (3, 5), "{format:?}");
    }
}
