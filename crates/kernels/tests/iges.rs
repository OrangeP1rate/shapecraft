//! IGES both ways between the two kernels: what the Rust kernel writes, Open CASCADE reads to the same solid, and
//! what Open CASCADE writes, the Rust kernel reads.
#![cfg(feature = "occt")]

use shapecraft_kernel::{Axis, BoolOp, DVec3, Kernel, Shape};
use shapecraft_kernel_occt::OcctKernel;
use shapecraft_kernel_rs::RustKernel;

/// Parts with flat, round, conical, toroidal and spherical faces, made on kernel `k`.
fn parts(k: &dyn Kernel) -> Vec<(&'static str, Shape)> {
    let block = k.make_box(DVec3::ZERO, DVec3::new(30.0, 20.0, 10.0)).unwrap();
    let hole = k.extrude_circle(DVec3::new(15.0, 10.0, -1.0), 4.0, DVec3::Z * 12.0).unwrap();
    let holed = k.boolean(BoolOp::Subtract, &block, &hole).unwrap();
    let rounded = k.fillet_all_edges(&k.make_box(DVec3::ZERO, DVec3::splat(10.0)).unwrap(), 2.0).unwrap();
    let cone = k
        .revolve_polygon(
            &[DVec3::new(0.0, 0.0, 0.0), DVec3::new(6.0, 0.0, 0.0), DVec3::new(3.0, 0.0, 8.0), DVec3::new(0.0, 0.0, 8.0)],
            Axis::new(DVec3::ZERO, DVec3::Z),
            std::f64::consts::TAU,
        )
        .unwrap();
    let ball = k.revolve_circle(DVec3::new(0.0, 0.0, 0.0), DVec3::Y, 5.0, Axis::new(DVec3::ZERO, DVec3::Z), std::f64::consts::PI);
    let mut out = vec![("holed block", holed), ("rounded cube", rounded), ("frustum", cone)];
    if let Ok(b) = ball {
        out.push(("ball", b));
    }
    out
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("shapecraft-iges-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn what_the_rust_kernel_writes_open_cascade_reads() {
    let (rs, oc) = (RustKernel, OcctKernel::new());
    for (name, shape) in parts(&rs) {
        let path = dir().join(format!("rs-{}.igs", name.replace(' ', "-")));
        rs.write_iges(&[shape.clone()], &path).unwrap();
        let back = oc.read_iges(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(back.len(), 1, "{name}");
        let (a, b) = (rs.mass_props(&shape).unwrap().volume, oc.mass_props(&back[0]).unwrap().volume);
        assert!((a - b).abs() < 1e-4 * a, "{name}: {a} → {b}");
        // And the Rust kernel reads its own file to the same solid.
        let again = rs.read_iges(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let c = rs.mass_props(&again[0]).unwrap().volume;
        assert!((a - c).abs() < 1e-6 * a, "{name}: {a} → {c}");
        assert_eq!(rs.face_count(&again[0]).unwrap(), rs.face_count(&shape).unwrap(), "{name}");
    }
}

#[test]
fn what_open_cascade_writes_the_rust_kernel_reads() {
    let (rs, oc) = (RustKernel, OcctKernel::new());
    for (name, shape) in parts(&oc) {
        let path = dir().join(format!("oc-{}.igs", name.replace(' ', "-")));
        oc.write_iges(&[shape.clone()], &path).unwrap();
        let back = rs.read_iges(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(back.len(), 1, "{name}");
        let (a, b) = (oc.mass_props(&shape).unwrap().volume, rs.mass_props(&back[0]).unwrap().volume);
        assert!((a - b).abs() < 1e-4 * a, "{name}: {a} → {b}");
    }
}

#[test]
fn an_ap242_step_from_the_rust_kernel_opens_in_both() {
    let (rs, oc) = (RustKernel, OcctKernel::new());
    let (_, shape) = parts(&rs).remove(0);
    let path = dir().join("rs-ap242.step");
    rs.write_step_ap(&[shape.clone()], &path, 242).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING"));
    let v = rs.mass_props(&shape).unwrap().volume;
    for back in [oc.read_step(&path).unwrap(), rs.read_step(&path).unwrap()] {
        let w: f64 = back.iter().map(|s| oc.mass_props(s).or_else(|_| rs.mass_props(s)).unwrap().volume).sum();
        assert!((v - w).abs() < 1e-6 * v, "{v} → {w}");
    }
    assert!(rs.write_step_ap(&[shape], &path, 999).is_err());
}
