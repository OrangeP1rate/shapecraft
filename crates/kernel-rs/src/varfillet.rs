//! Variable-radius fillets on the Rust kernel, built into the neutral B-rep and read back through STEP text (Arris
//! has no radius laws).
//!
//! As Open CASCADE makes them: a rolling ball whose radius eases from `r1` at the edge's start to `r2` at its end
//! (r1 + (r2 − r1)(3u² − 2u³), level at both ends), the round being the envelope of the balls. Done for straight
//! edges between two flat faces, each end square to the edge on a flat face:
//! - where the round meets each side face the ball's foot runs as the easing does: a cubic, exact as a Bézier;
//! - the radius is level at the ends, so there the ball's contact circle lies in the end face: an exact arc;
//! - across the round, each contact circle is an exact rational arc; along the edge, those arcs are interpolated.

use shapecraft_kernel::brep::{Curve, Edge, Face, Fin, Nurbs, NurbsSurface, Surface};
use shapecraft_kernel::spline;
use shapecraft_kernel::{DVec3, Result, Shape};

use crate::surgery::{open, plane_out, rebuild};
use crate::{ArShape, bad};

const ONLY: &str = "the Rust kernel's variable fillets run along straight edges between two flat faces, each end square \
                    to the edge on a flat face";

/// Arcs sampled along the edge for the round's surface (the volume settles to 1e-8 by 9). 31 spans put its knots at
/// 31sts of the edge, clear of the halves, thirds and quarters a cut is likely to land on: Arris can't yet intersect a
/// plane through a knot.
const ALONG: usize = 32;

/// One edge's round: the radius at the edge's start and end, the corner's geometry.
struct Round {
    p0: DVec3,
    t: DVec3,
    len: f64,
    wa: DVec3,
    wb: DVec3,
    bis: DVec3,
    /// cot(φ/2) and 1/sin(φ/2), φ the angle inside the corner.
    cot: f64,
    csc: f64,
    r1: f64,
    r2: f64,
}

impl Round {
    fn radius(&self, s: f64) -> (f64, f64) {
        let u = (s / self.len).clamp(0.0, 1.0);
        let d = self.r2 - self.r1;
        (self.r1 + d * u * u * (3.0 - 2.0 * u), d * 6.0 * u * (1.0 - u) / self.len)
    }

    fn feet(&self, s: f64) -> (DVec3, DVec3) {
        let (r, _) = self.radius(s);
        let p = self.p0 + self.t * s;
        (p + self.wa * (r * self.cot), p + self.wb * (r * self.cot))
    }

    /// The contact circle at `s` as a rational arc from foot a to foot b: its poles and middle weight.
    fn arc(&self, s: f64) -> Result<([DVec3; 3], f64)> {
        let (r, dr) = self.radius(s);
        let c = self.p0 + self.t * s + self.bis * (r * self.csc);
        let dc = self.t + self.bis * (dr * self.csc);
        let along = dc.length();
        let m = c - dc / along * (r * dr / along);
        let (fa, fb) = self.feet(s);
        let mid = ((fa - m) + (fb - m)).try_normalize().ok_or_else(|| bad(ONLY))?;
        let rho = (fa - m).length();
        let half = (fa - m).dot(mid) / rho;
        if half < 1e-6 {
            return Err(bad(ONLY));
        }
        Ok(([fa, m + mid * (rho / half), fb], half))
    }
}

/// The faces at either side of neutral edge `e` (face, walked forwards), and at its ends the two other edges there
/// with the face they bound together.
struct Around {
    sides: [(usize, bool); 2],
}

fn sides_of(b: &shapecraft_kernel::brep::Brep, e: usize) -> Option<Around> {
    let sides: Vec<(usize, bool)> = (0..b.faces.len())
        .flat_map(|fi| b.faces[fi].loops.iter().flatten().filter(|x| x.edge == e).map(move |x| (fi, x.forward)))
        .collect();
    match sides.as_slice() {
        [a, c] if a.0 != c.0 => Some(Around { sides: [*a, *c] }),
        _ => None,
    }
}

/// The edges at vertex `v` other than `skip`.
fn edges_at(b: &shapecraft_kernel::brep::Brep, v: usize, skip: usize) -> Vec<usize> {
    (0..b.edges.len()).filter(|&i| i != skip && (b.edges[i].start == Some(v) || b.edges[i].end == Some(v))).collect()
}

fn in_face(b: &shapecraft_kernel::brep::Brep, f: usize, e: usize) -> bool {
    b.faces[f].loops.iter().flatten().any(|x| x.edge == e)
}

pub fn fillet_variable(s: &ArShape, picks: &[u32], r1: f64, r2: f64) -> Result<Shape> {
    if !(r1.is_finite() && r2.is_finite() && r1 > 0.0 && r2 > 0.0) || picks.is_empty() {
        return Err(bad("a variable fillet needs edges and two positive radii"));
    }
    let mut o = open(s, picks)?;
    let edges = o.edges.clone();
    // Edges sharing an end would need a corner between two rounds.
    let ends: Vec<usize> = edges.iter().flat_map(|e| [o.brep.edges[*e].start, o.brep.edges[*e].end]).flatten().collect();
    if (1..ends.len()).any(|i| ends[..i].contains(&ends[i])) {
        return Err(bad("the Rust kernel's variable fillets don't meet at corners yet: pick edges that don't touch"));
    }
    for e in edges {
        one_edge(&mut o.brep, e, r1, r2)?;
    }
    rebuild(s, o.body, &o.brep)
}

fn one_edge(b: &mut shapecraft_kernel::brep::Brep, e: usize, r1: f64, r2: f64) -> Result<()> {
    let edge = b.edges[e].clone();
    let (Some(v0), Some(v1)) = (edge.start, edge.end) else { return Err(bad(ONLY)) };
    if !matches!(edge.curve.and_then(|c| b.curves.get(c)), Some(Curve::Line { .. })) {
        return Err(bad(ONLY));
    }
    let around = sides_of(b, e).ok_or_else(|| bad(ONLY))?;
    let [(fa, fwa), (fb, fwb)] = around.sides;
    let (p0, p1) = (b.vertices[v0], b.vertices[v1]);
    let len = p0.distance(p1);
    let t = (p1 - p0) / len;
    let (Some(na), Some(nb)) = (plane_out(b, fa), plane_out(b, fb)) else { return Err(bad(ONLY)) };
    let along = |forward: bool| if forward { t } else { -t };
    // Into each face, square to the edge (a face walks its edge with the face on its left).
    let (wa, wb) = (na.cross(along(fwa)), nb.cross(along(fwb)));
    if len < 1e-9 || wa.dot(nb) > -1e-9 {
        return Err(bad("the Rust kernel's variable fillets round outside edges"));
    }
    let phi = wa.dot(wb).clamp(-1.0, 1.0).acos();
    let bis = (wa + wb).normalize();
    let round = Round { p0, t, len, wa, wb, bis, cot: 1.0 / (phi / 2.0).tan(), csc: 1.0 / (phi / 2.0).sin(), r1, r2 };

    // Each end: the two edges along the side faces (square to the edge, long enough) and the end face.
    let mut ends = Vec::with_capacity(2);
    for (v, r) in [(v0, r1), (v1, r2)] {
        let others = edges_at(b, v, e);
        let [x, y] = others[..] else { return Err(bad(ONLY)) };
        let (ea, eb) = if in_face(b, fa, x) && in_face(b, fb, y) {
            (x, y)
        } else if in_face(b, fa, y) && in_face(b, fb, x) {
            (y, x)
        } else {
            return Err(bad(ONLY));
        };
        let end_face =
            (0..b.faces.len()).find(|&f| f != fa && f != fb && in_face(b, f, ea) && in_face(b, f, eb)).ok_or_else(|| bad(ONLY))?;
        let square = plane_out(b, end_face).is_some_and(|n| n.dot(t).abs() > 1.0 - 1e-9);
        let reach = r * round.cot;
        let long_enough = |k: usize| {
            let ed = &b.edges[k];
            let other = if ed.start == Some(v) { ed.end } else { ed.start };
            matches!(ed.curve.and_then(|c| b.curves.get(c)), Some(Curve::Line { .. }))
                && other.is_some_and(|o| b.vertices[o].distance(b.vertices[v]) > reach * (1.0 + 1e-6))
        };
        if !square || !long_enough(ea) || !long_enough(eb) {
            return Err(bad(ONLY));
        }
        ends.push((v, ea, eb, end_face));
    }

    // The new corners, and the side edges cut back to them.
    let mut corners = Vec::with_capacity(2);
    for (k, &(v, ea, eb, _)) in ends.iter().enumerate() {
        let s = if k == 0 { 0.0 } else { len };
        let (qa, qb) = round.feet(s);
        b.vertices.push(qa);
        b.vertices.push(qb);
        let (ia, ib) = (b.vertices.len() - 2, b.vertices.len() - 1);
        for (k, to) in [(ea, ia), (eb, ib)] {
            let ed = &mut b.edges[k];
            if ed.start == Some(v) {
                ed.start = Some(to);
            } else {
                ed.end = Some(to);
            }
        }
        corners.push((ia, ib));
    }

    // Where the round meets the side faces: the feet, a cubic along the edge.
    let foot_curve = |w: DVec3| {
        let (a, z) = (p0 + w * (r1 * round.cot), p0 + t * len + w * (r2 * round.cot));
        Curve::BSpline(Nurbs {
            degree: 3,
            poles: vec![a, a + t * (len / 3.0), z - t * (len / 3.0), z],
            weights: None,
            knots: vec![0.0, 1.0],
            mults: vec![4, 4],
            periodic: false,
        })
    };
    let new_edge = |b: &mut shapecraft_kernel::brep::Brep, curve: Curve, from: usize, to: usize| {
        b.curves.push(curve);
        b.edges.push(Edge { curve: Some(b.curves.len() - 1), reversed: false, start: Some(from), end: Some(to), tolerance: None });
        b.edges.len() - 1
    };
    let ga = new_edge(b, foot_curve(wa), corners[0].0, corners[1].0);
    let gb = new_edge(b, foot_curve(wb), corners[0].1, corners[1].1);
    // The ends: arcs of the end radii about the ball's centre there.
    let mut arcs = Vec::with_capacity(2);
    for (k, &(ia, ib)) in corners.iter().enumerate() {
        let (s, r) = if k == 0 { (0.0, r1) } else { (len, r2) };
        let c = p0 + t * s + bis * (r * round.csc);
        let x = (b.vertices[ia] - c).normalize();
        let normal = x.cross((b.vertices[ib] - c).normalize()).normalize();
        arcs.push(new_edge(b, Curve::Circle { center: c, normal, x, radius: r }, ia, ib));
    }

    // The side faces take the feet in the edge's place; each end face takes its arc between its two side edges.
    for (f, g) in [(fa, ga), (fb, gb)] {
        for fin in b.faces[f].loops.iter_mut().flatten() {
            if fin.edge == e {
                fin.edge = g;
            }
        }
    }
    let mut end_use = Vec::with_capacity(2);
    for (k, &(_, ea, eb, face)) in ends.iter().enumerate() {
        let arc = arcs[k];
        let mut used = None;
        for l in &mut b.faces[face].loops {
            let n = l.len();
            let Some(i) = (0..n).find(|&i| {
                let (x, y) = (l[i].edge, l[(i + 1) % n].edge);
                (x == ea && y == eb) || (x == eb && y == ea)
            }) else {
                continue;
            };
            // Arriving along ea at foot a, the arc runs a → b (as made); arriving along eb, b → a.
            let forward = l[i].edge == ea;
            l.insert(i + 1, Fin { edge: arc, forward, pcurve: None });
            used = Some(forward);
            break;
        }
        end_use.push(used.ok_or_else(|| bad(ONLY))?);
    }

    // The round: every edge walked against its other face.
    let use_a = b.faces[fa].loops.iter().flatten().find(|x| x.edge == ga).map(|x| x.forward).ok_or_else(|| bad(ONLY))?;
    let use_b = b.faces[fb].loops.iter().flatten().find(|x| x.edge == gb).map(|x| x.forward).ok_or_else(|| bad(ONLY))?;
    let fin = |edge: usize, other_forward: bool| Fin { edge, forward: !other_forward, pcurve: None };
    // Walking ga against face a: from the start end (forward) or the far end.
    let loop_fins = if !use_a {
        vec![fin(ga, use_a), fin(arcs[1], end_use[1]), fin(gb, use_b), fin(arcs[0], end_use[0])]
    } else {
        vec![fin(ga, use_a), fin(arcs[0], end_use[0]), fin(gb, use_b), fin(arcs[1], end_use[1])]
    };

    b.surfaces.push(Surface::BSpline(round_surface(&round)?));
    let surface = b.surfaces.len() - 1;
    // u runs along the edge, v from face a to face b: u × v against the way out of the corner flips it.
    let (qa, qb) = round.feet(len / 2.0);
    let flip = t.cross(qb - qa).dot(-bis) < 0.0;
    let color = b.faces[fa].color;
    b.faces.push(Face { surface, flip, loops: vec![loop_fins], color });
    let made = b.faces.len() - 1;
    if let Some(sh) = b.shells.iter_mut().find(|sh| sh.contains(&fa)) {
        sh.push(made);
    }
    Ok(())
}

/// The round as a NURBS: u along the edge (cubic, through the arcs at even steps), v across (each arc exact).
fn round_surface(round: &Round) -> Result<NurbsSurface> {
    let mut rows: [Vec<DVec3>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut weights = Vec::with_capacity(ALONG);
    for i in 0..ALONG {
        let s = round.len * i as f64 / (ALONG - 1) as f64;
        let (poles, w) = round.arc(s)?;
        rows[0].push(poles[0]);
        rows[1].push(poles[1] * w);
        rows[2].push(poles[2]);
        weights.push(DVec3::splat(w));
    }
    let params: Vec<f64> = (0..ALONG).map(|i| i as f64 / (ALONG - 1) as f64).collect();
    let fit = |pts: &[DVec3]| spline::fit_with(pts, params.clone()).ok_or_else(|| bad("couldn't fit the round"));
    let (a, m, z, w) = (fit(&rows[0])?, fit(&rows[1])?, fit(&rows[2])?, fit(&weights)?);
    let n = a.poles.len();
    let mut poles = Vec::with_capacity(3 * n);
    let mut ws = Vec::with_capacity(3 * n);
    for i in 0..n {
        let wi = w.poles[i].x;
        if wi.is_nan() || wi <= 1e-6 {
            return Err(bad("couldn't fit the round"));
        }
        poles.extend([a.poles[i], m.poles[i] / wi, z.poles[i]]);
        ws.extend([1.0, wi, 1.0]);
    }
    let (knots, mults) = a.knot_runs();
    Ok(NurbsSurface {
        u_degree: a.degree as u32,
        v_degree: 2,
        nu: n,
        nv: 3,
        poles,
        weights: Some(ws),
        u_knots: knots,
        u_mults: mults.iter().map(|m| *m as u32).collect(),
        v_knots: vec![0.0, 1.0],
        v_mults: vec![3, 3],
        u_periodic: false,
        v_periodic: false,
    })
}
