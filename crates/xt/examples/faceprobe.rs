//! `faceprobe FILE`: every face of every body through OCCT: its surface kind as read, and its area there (the
//! OCCT face it became, through `from_brep_faces`: OCCT's own face order differs from the file's).
use shapecraft_kernel::{Kernel, SubShape};
fn main() {
    let Some(p) = std::env::args().nth(1) else { return };
    let k = shapecraft_kernel_occt::OcctKernel::new();
    let f = shapecraft_xt::parse(&std::fs::read(p).unwrap_or_default()).expect("parse");
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let (s, index) = k.from_brep_faces(&b.brep).expect("build");
        for (i, face) in b.brep.faces.iter().enumerate() {
            let kind = format!("{:?}", b.brep.surfaces[face.surface]);
            let kind: String = kind.chars().take(90).collect();
            let m = index[i].and_then(|j| k.measure(&s, SubShape::Face(j)).ok());
            let loops: Vec<usize> = face.loops.iter().map(|l| l.len()).collect();
            println!("{i:3} {:?} loops {:?} flip {} {}", m.map(|(a, c)| (format!("{a:.3}"), c)), loops, face.flip, kind);
        }
        // OCCT faces no file face became (what the repair split off or made new).
        for j in 0..k.face_count(&s).unwrap_or(0) {
            if !index.contains(&Some(j)) {
                println!("  occt {j} (no file face) {:?}", k.measure(&s, SubShape::Face(j)).map(|(a, c)| (format!("{a:.3}"), c)));
            }
        }
        println!("  mass {:?}\n  mesh {:?}", k.mass_props(&s), k.mass_props_mesh(&s));
    }
}
