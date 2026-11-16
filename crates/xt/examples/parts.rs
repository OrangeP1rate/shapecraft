//! `parts FILE`: what OCCT makes of each body: its solids, with their surface kinds and volumes.
use shapecraft_kernel::Kernel;
fn main() {
    let Some(p) = std::env::args().nth(1) else { return };
    let f = shapecraft_xt::parse(&std::fs::read(p).unwrap_or_default()).expect("parse");
    let k = shapecraft_kernel_occt::OcctKernel::new();
    let kind = |s: &shapecraft_kernel::brep::Surface| format!("{s:?}").split([' ', '(']).next().unwrap_or("").to_string();
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let mut m = std::collections::BTreeMap::new();
        for face in &b.brep.faces {
            *m.entry(kind(&b.brep.surfaces[face.surface])).or_insert(0) += 1;
        }
        println!("read {m:?}");
        let s = k.from_brep(&b.brep).expect("build");
        println!("whole volume {:?}", k.mass_props(&s).map(|m| m.volume));
        for part in k.solids(&s).expect("solids") {
            let v = k.mass_props(&part).map(|m| m.volume);
            for bb in k.to_brep(&part).expect("to_brep") {
                let mut m = std::collections::BTreeMap::new();
                for face in &bb.faces {
                    *m.entry(kind(&bb.surfaces[face.surface])).or_insert(0) += 1;
                }
                println!("solid {v:?}: shells {:?} {m:?}", bb.shells.iter().map(Vec::len).collect::<Vec<_>>());
            }
        }
    }
}
