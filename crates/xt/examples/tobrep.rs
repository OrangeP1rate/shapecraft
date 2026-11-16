//! `tobrep FILE`: each body through OCCT and back out (`to_brep`): the faces whose loops use an edge twice (a seam),
//! with their fins, and faces whose loops changed in fin count.
use shapecraft_kernel::Kernel;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let k = shapecraft_kernel_occt::OcctKernel::new();
    let f = shapecraft_xt::parse(&std::fs::read(&a[1]).unwrap_or_default()).expect("parse");
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let s = k.from_brep(&b.brep).expect("build");
        for out in k.to_brep(&s).expect("out") {
            for (i, face) in out.faces.iter().enumerate() {
                let mut uses = std::collections::HashMap::new();
                for fin in face.loops.iter().flatten() {
                    *uses.entry(fin.edge).or_insert(0) += 1;
                }
                if !uses.values().any(|&n| n > 1) {
                    continue;
                }
                let kind = format!("{:?}", out.surfaces[face.surface]).chars().take(30).collect::<String>();
                let loops: Vec<String> = face
                    .loops
                    .iter()
                    .map(|l| {
                        l.iter()
                            .map(|f| {
                                let e = &out.edges[f.edge];
                                let len = match (e.start, e.end) {
                                    (Some(s), Some(t)) => format!("{:.3}", out.vertices[s].distance(out.vertices[t])),
                                    _ => "ring".into(),
                                };
                                format!(
                                    "e{}{}({}{})",
                                    f.edge,
                                    if f.forward { "+" } else { "-" },
                                    len,
                                    if e.curve.is_none() { ",nocurve" } else { "" }
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect();
                println!("face {i} {kind}: {loops:?}");
            }
        }
    }
}
