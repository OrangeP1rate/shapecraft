//! `loopprobe FILE FACE`: a face's loops as read: per fin the edge's curve kind, its ends, and the loop's signed
//! area in the face plane (from the fin vertices).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = shapecraft_xt::parse(&std::fs::read(&a[1]).unwrap_or_default()).expect("parse");
    let fi: usize = a[2].parse().unwrap_or(0);
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let b = b.brep;
        let face = &b.faces[fi];
        let n = match &b.surfaces[face.surface] {
            shapecraft_kernel::brep::Surface::Plane { normal, .. } => *normal,
            _ => shapecraft_kernel::DVec3::Z,
        };
        println!("surface {:?}", b.surfaces[face.surface]);
        for (li, l) in face.loops.iter().enumerate() {
            let mut pts = Vec::new();
            for fin in l {
                let e = &b.edges[fin.edge];
                let (s, t) = if fin.forward { (e.start, e.end) } else { (e.end, e.start) };
                let kind = e
                    .curve
                    .map(|c| {
                        format!("{:?}", b.curves[c])
                            .chars()
                            .take(if std::env::var_os("LONG").is_some() { 100_000 } else { 60 })
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                println!(
                    "  loop {li} edge {} fwd {} rev {} {:?}->{:?} {}",
                    fin.edge,
                    fin.forward,
                    e.reversed,
                    s.map(|v| b.vertices[v]),
                    t.map(|v| b.vertices[v]),
                    kind
                );
                if let Some(v) = s {
                    pts.push(b.vertices[v]);
                }
            }
            let mut area = shapecraft_kernel::DVec3::ZERO;
            for k in 0..pts.len() {
                area += pts[k].cross(pts[(k + 1) % pts.len()]);
            }
            println!("  loop {li}: {} fins, signed polygon area {:.3}", l.len(), area.dot(n) / 2.0 * if face.flip { -1.0 } else { 1.0 });
        }
    }
}
