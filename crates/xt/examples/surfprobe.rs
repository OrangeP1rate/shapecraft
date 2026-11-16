//! `surfprobe FILE FACE`: a B-spline face's surface at the ends of its parameter ranges (does it close up?).
use shapecraft_kernel::DVec3;
use shapecraft_kernel::brep::Surface;
fn flat(knots: &[f64], mults: &[u32]) -> Vec<f64> {
    knots.iter().zip(mults).flat_map(|(&k, &m)| std::iter::repeat_n(k, m as usize)).collect()
}
// De Boor on homogeneous points.
fn eval(p: &[(DVec3, f64)], deg: usize, t: &[f64], x: f64) -> DVec3 {
    let n = p.len();
    let mut k = deg;
    while k + 1 < n && t[k + 1] <= x {
        k += 1;
    }
    let mut d: Vec<(DVec3, f64)> = (0..=deg).map(|j| (p[j + k - deg].0 * p[j + k - deg].1, p[j + k - deg].1)).collect();
    for r in 1..=deg {
        for j in (r..=deg).rev() {
            let i = j + k - deg;
            let den = t[i + deg + 1 - r] - t[i];
            let a = if den == 0.0 { 0.0 } else { (x - t[i]) / den };
            d[j] = (d[j - 1].0 * (1.0 - a) + d[j].0 * a, d[j - 1].1 * (1.0 - a) + d[j].1 * a);
        }
    }
    d[deg].0 / d[deg].1
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = shapecraft_xt::parse(&std::fs::read(&a[1]).unwrap_or_default()).expect("parse");
    let fi: usize = a[2].parse().unwrap_or(0);
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let Surface::BSpline(s) = &b.brep.surfaces[b.brep.faces[fi].surface] else { continue };
        let tv = flat(&s.v_knots, &s.v_mults);
        // Where the face's vertices sit on the surface (ruled in u only: u extrapolated linearly).
        if s.nu == 2 && s.u_degree == 1 {
            let row = |r: usize| -> Vec<(DVec3, f64)> {
                (0..s.nv).map(|j| (s.poles[r * s.nv + j], s.weights.as_ref().map_or(1.0, |w| w[r * s.nv + j]))).collect()
            };
            let (r0, r1) = (row(0), row(1));
            let (v0, v1) = (tv[s.v_degree as usize], tv[s.nv]);
            let face = &b.brep.faces[fi];
            for l in &face.loops {
                for fin in l {
                    let e = &b.brep.edges[fin.edge];
                    for vx in [e.start, e.end].into_iter().flatten().map(|i| b.brep.vertices[i]) {
                        let mut best = (f64::MAX, 0.0, 0.0);
                        for k in 0..4000 {
                            let v = v0 + (v1 - v0) * k as f64 / 4000.0;
                            let (a, c) = (eval(&r0, s.v_degree as usize, &tv, v), eval(&r1, s.v_degree as usize, &tv, v));
                            let d = c - a;
                            let u = (vx - a).dot(d) / d.length_squared();
                            let dist = (a + d * u).distance(vx);
                            if dist < best.0 {
                                best = (dist, u, v);
                            }
                        }
                        println!("vertex {vx:?}: u {:.4} v {:.5} dist {:.4}", best.1, best.2, best.0);
                    }
                }
            }
        }
        let (v0, v1) = (tv[s.v_degree as usize], tv[s.nv]);
        println!("v range {v0} .. {v1} (knots {} .. {})", tv[0], tv[tv.len() - 1]);
        for row in 0..s.nu {
            let p: Vec<(DVec3, f64)> =
                (0..s.nv).map(|j| (s.poles[row * s.nv + j], s.weights.as_ref().map_or(1.0, |w| w[row * s.nv + j]))).collect();
            let (x0, x1) = (eval(&p, s.v_degree as usize, &tv, v0), eval(&p, s.v_degree as usize, &tv, v1 - 1e-12));
            let pts: Vec<DVec3> =
                (0..=8).map(|k| eval(&p, s.v_degree as usize, &tv, v0 + (v1 - v0) * k as f64 / 8.0 - 1e-12 * (k / 8) as f64)).collect();
            println!("row {row} samples {:?}", pts.iter().map(|q| format!("({:.1},{:.1},{:.1})", q.x, q.y, q.z)).collect::<Vec<_>>());
            println!("row {row}: start {x0:?} end {x1:?} gap {:.6}; first pole {:?} last {:?}", x0.distance(x1), p[0].0, p[s.nv - 1].0);
        }
    }
}
