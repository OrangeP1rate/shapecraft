//! `faceprobe FILE`: every face of every body through OCCT: its surface kind as read, and its area there.
use shapecraft_kernel::{Kernel, SubShape};
fn main() {
    let Some(p) = std::env::args().nth(1) else { return };
    let k = shapecraft_kernel_occt::OcctKernel::new();
    let f = shapecraft_xt::parse(&std::fs::read(p).unwrap_or_default()).expect("parse");
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let s = k.from_brep(&b.brep).expect("build");
        for (i, face) in b.brep.faces.iter().enumerate() {
            let kind = format!("{:?}", b.brep.surfaces[face.surface]);
            let kind: String = kind.chars().take(90).collect();
            let m = k.measure(&s, SubShape::Face(i as u32));
            let loops: Vec<usize> = face.loops.iter().map(|l| l.len()).collect();
            println!("{i:3} {:?} loops {:?} flip {} {}", m.map(|(a, c)| (format!("{a:.3}"), c)), loops, face.flip, kind);
        }
    }
}
