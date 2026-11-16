//! `rtdiff IN`: each body through OCCT and then our XT writer and reader; prints the surfaces and curves that come
//! back different (a writer check).
use shapecraft_kernel::Kernel;
fn main() {
    let Some(inp) = std::env::args().nth(1) else { return eprintln!("usage: rtdiff IN") };
    let f = shapecraft_xt::parse(&std::fs::read(&inp).unwrap_or_default()).expect("parse");
    let k = shapecraft_kernel_occt::OcctKernel::new();
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let s = k.from_brep(&b.brep).expect("build");
        for b2 in k.to_brep(&s).expect("to_brep") {
            let x = shapecraft_xt::write_body(&b2, shapecraft_xt::Format::Binary).expect("write");
            let b3 = shapecraft_xt::bodies(&shapecraft_xt::parse(&x).expect("reparse")).expect("rebodies").remove(0).brep;
            println!(
                "faces {} -> {}, surfaces {} -> {}, curves {} -> {}",
                b2.faces.len(),
                b3.faces.len(),
                b2.surfaces.len(),
                b3.surfaces.len(),
                b2.curves.len(),
                b3.curves.len()
            );
            let skind = |b: &shapecraft_kernel::brep::Brep, f: &shapecraft_kernel::brep::Face| {
                let s = format!("{:?}", b.surfaces[f.surface]);
                let k = s.split([' ', '(']).next().unwrap_or("").to_string();
                let loops: Vec<String> = f
                    .loops
                    .iter()
                    .map(|l| {
                        l.iter()
                            .map(|fin| {
                                let e = &b.edges[fin.edge];
                                match e.curve {
                                    Some(c) => format!("{:?}", b.curves[c]).chars().take(2).collect::<String>(),
                                    None => "--".into(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .collect();
                format!("{k} flip={} loops={:?}", f.flip, loops)
            };
            let s2 = k.from_brep(&b2).expect("b2");
            let s3 = k.from_brep(&b3).expect("b3");
            for (i, (f2, f3)) in b2.faces.iter().zip(&b3.faces).enumerate() {
                let a2 = k.measure(&s2, shapecraft_kernel::SubShape::Face(i as u32)).map(|m| m.0).unwrap_or(f64::NAN);
                let a3 = k.measure(&s3, shapecraft_kernel::SubShape::Face(i as u32)).map(|m| m.0).unwrap_or(f64::NAN);
                println!("F{i}: area {a2:.4} -> {a3:.4} {}\n  -> {}", skind(&b2, f2), skind(&b3, f3));
            }
            let kind = |c: &shapecraft_kernel::brep::Curve| format!("{c:?}").split([' ', '(']).next().unwrap_or("").to_string();
            for (i, (e2, e3)) in b2.edges.iter().zip(&b3.edges).enumerate() {
                let c2 = e2.curve.map(|c| kind(&b2.curves[c]));
                let c3 = e3.curve.map(|c| kind(&b3.curves[c]));
                let v = |b: &shapecraft_kernel::brep::Brep, x: Option<usize>| x.map(|i| b.vertices[i]);
                if c2 != c3 || e2.reversed != e3.reversed || v(&b2, e2.start) != v(&b3, e3.start) || v(&b2, e2.end) != v(&b3, e3.end) {
                    println!(
                        "edge {i}: {c2:?} rev {} {:?}..{:?}\n     -> {c3:?} rev {} {:?}..{:?}",
                        e2.reversed,
                        v(&b2, e2.start),
                        v(&b2, e2.end),
                        e3.reversed,
                        v(&b3, e3.start),
                        v(&b3, e3.end)
                    );
                }
            }
            for (i, (f2, f3)) in b2.faces.iter().zip(&b3.faces).enumerate() {
                let (s2, s3) = (format!("{:?}", b2.surfaces[f2.surface]), format!("{:?}", b3.surfaces[f3.surface]));
                if s2 != s3 || f2.flip != f3.flip {
                    println!("face {i}: flip {} -> {}\n  {}\n  {}", f2.flip, f3.flip, &s2[..s2.len().min(400)], &s3[..s3.len().min(400)]);
                }
            }
        }
    }
}
