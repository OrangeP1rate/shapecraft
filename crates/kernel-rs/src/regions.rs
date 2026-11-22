//! Sketch regions: the closed areas a set of planar curves divides its plane into, each with its holes.
//!
//! The curves are cut where they cross or touch, the pieces become the edges of a planar graph, dangling pieces are
//! dropped, and the faces of the graph are traced with the interior on the left (counter-clockwise = bounded, a
//! negative area = the outside of one connected group). A group lying inside a bounded face of another group is a
//! hole of the smallest such face. Lines, circles and arcs stay exact; ellipses and splines are followed as fine
//! polylines.

use std::f64::consts::{PI, TAU};

use glam::DVec2;
use shapecraft_kernel::{PlanarCurve, PlaneFrame};

/// One piece of a sketch curve in the plane's (u, v).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line {
        a: DVec2,
        b: DVec2,
    },
    /// Starts at angle `a0` and turns by `sweep` (signed: positive is counter-clockwise).
    Arc {
        c: DVec2,
        r: f64,
        a0: f64,
        sweep: f64,
    },
}

impl Seg {
    /// The arc from `from` through `via` to `to` (a line when the three are in a row).
    pub fn through(from: DVec2, via: DVec2, to: DVec2) -> Seg {
        match circle_through(from, via, to) {
            Some((c, r)) => {
                let ang = |p: DVec2| (p - c).to_angle();
                let ccw = (ang(to) - ang(from)).rem_euclid(TAU);
                let sweep = if (ang(via) - ang(from)).rem_euclid(TAU) < ccw { ccw } else { ccw - TAU };
                Seg::Arc { c, r, a0: ang(from), sweep }
            }
            None => Seg::Line { a: from, b: to },
        }
    }

    pub fn at(&self, s: f64) -> DVec2 {
        match *self {
            Seg::Line { a, b } => a.lerp(b, s),
            Seg::Arc { c, r, a0, sweep } => c + r * DVec2::from_angle(a0 + s * sweep),
        }
    }

    pub fn start(&self) -> DVec2 {
        self.at(0.0)
    }

    pub fn end(&self) -> DVec2 {
        self.at(1.0)
    }

    fn sub(&self, s0: f64, s1: f64) -> Seg {
        match *self {
            Seg::Line { .. } => Seg::Line { a: self.at(s0), b: self.at(s1) },
            Seg::Arc { c, r, a0, sweep } => Seg::Arc { c, r, a0: a0 + s0 * sweep, sweep: (s1 - s0) * sweep },
        }
    }

    pub fn reversed(&self) -> Seg {
        match *self {
            Seg::Line { a, b } => Seg::Line { a: b, b: a },
            Seg::Arc { c, r, a0, sweep } => Seg::Arc { c, r, a0: a0 + sweep, sweep: -sweep },
        }
    }

    /// The direction it leaves its start in, and how hard it turns left (1/r, negative for right).
    fn leaving(&self) -> (DVec2, f64) {
        match *self {
            Seg::Line { a, b } => ((b - a).normalize_or_zero(), 0.0),
            Seg::Arc { r, a0, sweep, .. } => {
                let s = sweep.signum();
                (s * DVec2::from_angle(a0).perp(), s / r)
            }
        }
    }

    fn length(&self) -> f64 {
        match *self {
            Seg::Line { a, b } => a.distance(b),
            Seg::Arc { r, sweep, .. } => r * sweep.abs(),
        }
    }

    /// Its share of the signed area of a loop it's part of (shoelace term plus the arc's bulge).
    pub fn area_term(&self) -> f64 {
        let (p, q) = (self.start(), self.end());
        let chord = p.perp_dot(q) / 2.0;
        match *self {
            Seg::Line { .. } => chord,
            Seg::Arc { r, sweep, .. } => chord + r * r / 2.0 * (sweep - sweep.sin()),
        }
    }

    /// The parameter of the point of this piece nearest `p`, and how far `p` is from it.
    fn project(&self, p: DVec2) -> (f64, f64) {
        match *self {
            Seg::Line { a, b } => {
                let d = b - a;
                let s = ((p - a).dot(d) / d.length_squared().max(1e-300)).clamp(0.0, 1.0);
                (s, p.distance(a + s * d))
            }
            Seg::Arc { .. } => {
                let s = self.param_of(p).unwrap_or_else(|| if p.distance(self.start()) < p.distance(self.end()) { 0.0 } else { 1.0 });
                (s, p.distance(self.at(s)))
            }
        }
    }

    /// For an arc: the parameter (0..=1) of the point on it at the angle of `p`, if that angle is within its sweep.
    fn param_of(&self, p: DVec2) -> Option<f64> {
        let Seg::Arc { c, a0, sweep, .. } = *self else { return None };
        let ang = (p - c).to_angle();
        let d = if sweep >= 0.0 { (ang - a0).rem_euclid(TAU) } else { (a0 - ang).rem_euclid(TAU) };
        let s = d / sweep.abs();
        if s <= 1.0 + 1e-12 {
            Some(s.min(1.0))
        } else if (TAU - d) / sweep.abs() < 1e-12 {
            Some(0.0)
        } else {
            None
        }
    }

    fn sample(&self, out: &mut Vec<DVec2>) {
        let n = match *self {
            Seg::Line { .. } => 1,
            Seg::Arc { sweep, .. } => ((sweep.abs() / TAU * 64.0).ceil() as usize).max(2),
        };
        out.extend((0..n).map(|i| self.at(i as f64 / n as f64)));
    }
}

/// A closed loop of pieces, end to start, in order.
pub type Cycle = Vec<Seg>;

/// One region: its outer loop (counter-clockwise) and its holes (clockwise), in the plane's (u, v).
#[derive(Clone, Debug)]
pub struct Region {
    pub outer: Cycle,
    pub holes: Vec<Cycle>,
    pub area: f64,
}

pub fn to_local(frame: &PlaneFrame, p: glam::DVec3) -> DVec2 {
    let d = p - frame.origin;
    DVec2::new(d.dot(frame.x), d.dot(frame.y))
}

fn polyline(points: &[DVec2], closed: bool, out: &mut Vec<Seg>) {
    for w in points.windows(2) {
        out.push(Seg::Line { a: w[0], b: w[1] });
    }
    if closed && points.len() > 2 {
        out.push(Seg::Line { a: points[points.len() - 1], b: points[0] });
    }
}

/// A clamped B-spline of degree min(3, n-1) over the poles, sampled.
fn bspline(poles: &[DVec2], n: usize) -> Vec<DVec2> {
    let k = poles.len();
    if k < 2 {
        return poles.to_vec();
    }
    let p = (k - 1).min(3);
    let spans = k - p;
    let mut knots = vec![0.0; p + 1];
    knots.extend((1..spans).map(|i| i as f64 / spans as f64));
    knots.extend(std::iter::repeat_n(1.0, p + 1));
    (0..=n)
        .map(|i| {
            let u = (i as f64 / n as f64).min(1.0 - 1e-12);
            let span = (p..k).rev().find(|&j| knots[j] <= u).unwrap_or(p);
            let mut d: Vec<DVec2> = (0..=p).map(|j| poles[span - p + j]).collect();
            for r in 1..=p {
                for j in (r..=p).rev() {
                    let ki = span - p + j;
                    let den = knots[ki + p + 1 - r] - knots[ki];
                    let a = if den.abs() < 1e-300 { 0.0 } else { (u - knots[ki]) / den };
                    d[j] = d[j - 1].lerp(d[j], a);
                }
            }
            d[p]
        })
        .collect()
}

/// The sketch curves as pieces in (u, v).
pub fn pieces(frame: &PlaneFrame, curves: &[PlanarCurve]) -> Vec<Seg> {
    let l = |p: glam::DVec3| to_local(frame, p);
    let mut out = Vec::new();
    for c in curves {
        match c {
            PlanarCurve::Line { a, b } => out.push(Seg::Line { a: l(*a), b: l(*b) }),
            PlanarCurve::Circle { center, radius } => out.push(Seg::Arc { c: l(*center), r: *radius, a0: 0.0, sweep: TAU }),
            PlanarCurve::Arc { a, mid, b } => out.push(Seg::through(l(*a), l(*mid), l(*b))),
            PlanarCurve::Ellipse { center, major, rx, ry } => {
                let c = l(*center);
                let mx = DVec2::new(major.dot(frame.x), major.dot(frame.y)).normalize_or(DVec2::X);
                let pts: Vec<DVec2> = (0..128)
                    .map(|i| {
                        let t = i as f64 / 128.0 * TAU;
                        c + mx * rx * t.cos() + mx.perp() * ry * t.sin()
                    })
                    .collect();
                polyline(&pts, true, &mut out);
            }
            PlanarCurve::Spline { points } => {
                let pts: Vec<DVec2> = match shapecraft_kernel::spline::fit(points) {
                    Some(f) => f.sample(16).into_iter().map(l).collect(),
                    None => points.iter().map(|p| l(*p)).collect(),
                };
                polyline(&pts, false, &mut out);
            }
            PlanarCurve::BSpline { poles } => {
                let p: Vec<DVec2> = poles.iter().map(|p| l(*p)).collect();
                polyline(&bspline(&p, 16 * p.len().max(2)), false, &mut out);
            }
        }
    }
    out.retain(|s| s.length() > 1e-12);
    out
}

pub fn circle_through(a: DVec2, b: DVec2, c: DVec2) -> Option<(DVec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-14 * (a.length_squared() + b.length_squared() + c.length_squared()).max(1.0) {
        return None;
    }
    let (a2, b2, c2) = (a.length_squared(), b.length_squared(), c.length_squared());
    let center = DVec2::new(
        (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d,
        (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d,
    );
    Some((center, center.distance(a)))
}

/// Where two pieces cross: parameters on each.
fn crossings(p: &Seg, q: &Seg, tol: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    match (*p, *q) {
        (Seg::Line { a, b }, Seg::Line { a: c, b: d }) => {
            let (r, s) = (b - a, d - c);
            let den = r.perp_dot(s);
            if den.abs() > 1e-12 * r.length() * s.length() {
                let t = (c - a).perp_dot(s) / den;
                let u = (c - a).perp_dot(r) / den;
                let (et, eu) = (tol / r.length(), tol / s.length());
                if (-et..=1.0 + et).contains(&t) && (-eu..=1.0 + eu).contains(&u) {
                    out.push((t.clamp(0.0, 1.0), u.clamp(0.0, 1.0)));
                }
            }
        }
        (Seg::Line { .. }, Seg::Arc { .. }) => {
            for (u, t) in crossings(q, p, tol) {
                out.push((t, u));
            }
        }
        (Seg::Arc { c, r, .. }, Seg::Line { a, b }) => {
            let d = b - a;
            let f = a - c;
            let (qa, qb, qc) = (d.dot(d), 2.0 * f.dot(d), f.dot(f) - r * r);
            let disc = qb * qb - 4.0 * qa * qc;
            let roots: Vec<f64> = if disc < 0.0 {
                // Grazing within tolerance: the nearest point.
                let t = -qb / (2.0 * qa);
                if (a + t * d).distance(c) - r < tol { vec![t] } else { vec![] }
            } else {
                let sq = disc.sqrt();
                vec![(-qb - sq) / (2.0 * qa), (-qb + sq) / (2.0 * qa)]
            };
            let et = tol / d.length();
            for t in roots {
                if (-et..=1.0 + et).contains(&t) {
                    let pt = a + t.clamp(0.0, 1.0) * d;
                    if let Some(s) = p.param_of(pt) {
                        out.push((s, t.clamp(0.0, 1.0)));
                    }
                }
            }
        }
        (Seg::Arc { c: c1, r: r1, .. }, Seg::Arc { c: c2, r: r2, .. }) => {
            let d = c1.distance(c2);
            if d < tol && (r1 - r2).abs() < tol {
                return out; // the same circle: shared ends are found by projection
            }
            if d > r1 + r2 + tol || d < (r1 - r2).abs() - tol || d < 1e-300 {
                return out;
            }
            let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
            let h = (r1 * r1 - a * a).max(0.0).sqrt();
            let e = (c2 - c1) / d;
            let m = c1 + a * e;
            let pts = if h < tol * 1e-3 { vec![m] } else { vec![m + h * e.perp(), m - h * e.perp()] };
            for pt in pts {
                if let (Some(s), Some(t)) = (p.param_of(pt), q.param_of(pt)) {
                    out.push((s, t));
                }
            }
        }
    }
    out
}

struct Graph {
    verts: Vec<DVec2>,
    /// Pieces with their end vertices.
    edges: Vec<(Seg, usize, usize)>,
}

fn vertex(verts: &mut Vec<DVec2>, p: DVec2, tol: f64) -> usize {
    if let Some(i) = verts.iter().position(|v| v.distance(p) <= tol) {
        return i;
    }
    verts.push(p);
    verts.len() - 1
}

fn build(segs: &[Seg], tol: f64) -> Graph {
    let mut cuts: Vec<Vec<f64>> = segs.iter().map(|_| vec![0.0, 1.0]).collect();
    for i in 0..segs.len() {
        for j in i + 1..segs.len() {
            for (s, t) in crossings(&segs[i], &segs[j], tol) {
                cuts[i].push(s);
                cuts[j].push(t);
            }
        }
    }
    // Ends touching another piece (T-junctions, overlaps, the same circle twice).
    for i in 0..segs.len() {
        for j in 0..segs.len() {
            if i == j {
                continue;
            }
            for e in [segs[j].start(), segs[j].end()] {
                let (s, d) = segs[i].project(e);
                if d <= tol {
                    cuts[i].push(s);
                }
            }
        }
    }
    let mut g = Graph { verts: Vec::new(), edges: Vec::new() };
    for (seg, cut) in segs.iter().zip(&mut cuts) {
        cut.sort_by(f64::total_cmp);
        let len = seg.length();
        cut.dedup_by(|a, b| (*a - *b).abs() * len <= tol);
        if let Some(last) = cut.last_mut() {
            *last = 1.0;
        }
        for w in cut.windows(2) {
            let piece = seg.sub(w[0], w[1]);
            if piece.length() <= tol {
                continue;
            }
            let (a, b) = (vertex(&mut g.verts, piece.start(), tol), vertex(&mut g.verts, piece.end(), tol));
            if a == b && !matches!(piece, Seg::Arc { sweep, .. } if sweep.abs() > PI) {
                continue;
            }
            // The same piece twice (a line drawn over another): keep one.
            let mid = piece.at(0.5);
            let dup = g.edges.iter().any(|(s, x, y)| ((*x == a && *y == b) || (*x == b && *y == a)) && s.project(mid).1 <= tol);
            if !dup {
                g.edges.push((piece, a, b));
            }
        }
    }
    // Pieces that end in mid-air bound nothing.
    loop {
        let mut degree = vec![0usize; g.verts.len()];
        for (_, a, b) in &g.edges {
            degree[*a] += 1;
            degree[*b] += 1;
        }
        let before = g.edges.len();
        g.edges.retain(|(_, a, b)| degree[*a] > 1 && degree[*b] > 1);
        if g.edges.len() == before {
            break;
        }
    }
    g
}

/// The regions the curves enclose, in (u, v). Bounded faces only, each with the groups inside it as holes.
pub fn regions(segs: &[Seg]) -> Vec<Region> {
    let span = segs.iter().flat_map(|s| [s.start(), s.end(), s.at(0.5)]).fold(0.0f64, |m, p| m.max(p.abs().max_element()));
    let tol = 1e-7 * span.max(1.0);
    let g = build(segs, tol);
    let n = g.edges.len();
    if n == 0 {
        return Vec::new();
    }
    // Half-edge h: edge h/2, forward when even.
    let half = |h: usize| -> (Seg, usize, usize) {
        let (s, a, b) = g.edges[h / 2];
        if h.is_multiple_of(2) { (s, a, b) } else { (s.reversed(), b, a) }
    };
    let mut out_at: Vec<Vec<usize>> = vec![Vec::new(); g.verts.len()];
    for h in 0..2 * n {
        out_at[half(h).1].push(h);
    }
    for list in &mut out_at {
        let key = |h: &usize| half(*h).0.leaving();
        list.sort_by(|x, y| {
            let ((dx, kx), (dy, ky)) = (key(x), key(y));
            let (ax, ay) = (dx.to_angle(), dy.to_angle());
            if (ax - ay).abs() < 1e-9 { kx.total_cmp(&ky) } else { ax.total_cmp(&ay) }
        });
    }
    let next = |h: usize| -> usize {
        let (_, _, v) = half(h);
        let twin = h ^ 1;
        let list = &out_at[v];
        let i = list.iter().position(|&x| x == twin).unwrap_or(0);
        list[(i + list.len() - 1) % list.len()]
    };
    // Trace every face.
    let mut seen = vec![false; 2 * n];
    let mut faces: Vec<(Vec<usize>, f64)> = Vec::new();
    for start in 0..2 * n {
        if seen[start] {
            continue;
        }
        let mut cyc = Vec::new();
        let mut h = start;
        while !seen[h] && cyc.len() <= 2 * n {
            seen[h] = true;
            cyc.push(h);
            h = next(h);
        }
        let area: f64 = cyc.iter().map(|&h| half(h).0.area_term()).sum();
        faces.push((cyc, area));
    }
    // Connected groups, to know which outer boundary belongs where.
    let mut group: Vec<usize> = (0..g.verts.len()).collect();
    fn find(g: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while g[r] != r {
            r = g[r];
        }
        g[x] = r;
        r
    }
    for (_, a, b) in &g.edges {
        let (ra, rb) = (find(&mut group, *a), find(&mut group, *b));
        group[ra] = rb;
    }
    let cycle = |hs: &[usize]| -> Cycle { hs.iter().map(|&h| half(h).0).collect() };
    let poly = |c: &Cycle| -> Vec<DVec2> {
        let mut pts = Vec::new();
        for s in c {
            s.sample(&mut pts);
        }
        pts
    };
    let area_eps = tol * span.max(1.0);
    let mut out: Vec<(Region, usize, Vec<DVec2>)> = Vec::new();
    for (hs, area) in &faces {
        if *area > area_eps {
            let c = cycle(hs);
            let p = poly(&c);
            let grp = find(&mut group, half(hs[0]).1);
            out.push((Region { outer: c, holes: Vec::new(), area: *area }, grp, p));
        }
    }
    for (hs, area) in &faces {
        if *area >= -area_eps {
            continue;
        }
        let grp = find(&mut group, half(hs[0]).1);
        let probe = half(hs[0]).0.at(0.5);
        let host = out
            .iter()
            .enumerate()
            .filter(|(_, (_, g2, p))| *g2 != grp && inside(probe, p))
            .min_by(|a, b| a.1.0.area.total_cmp(&b.1.0.area))
            .map(|(i, _)| i);
        if let Some(i) = host {
            let c = cycle(hs);
            out[i].0.area += area;
            out[i].0.holes.push(c);
        }
    }
    out.into_iter().map(|(r, _, _)| r).collect()
}

fn inside(p: DVec2, poly: &[DVec2]) -> bool {
    let mut wind = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            wind = !wind;
        }
    }
    wind
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    fn frame() -> PlaneFrame {
        PlaneFrame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<PlanarCurve> {
        let p = [DVec3::new(x0, y0, 0.0), DVec3::new(x1, y0, 0.0), DVec3::new(x1, y1, 0.0), DVec3::new(x0, y1, 0.0)];
        (0..4).map(|i| PlanarCurve::Line { a: p[i], b: p[(i + 1) % 4] }).collect()
    }

    fn areas(curves: &[PlanarCurve]) -> Vec<f64> {
        let mut a: Vec<f64> = regions(&pieces(&frame(), curves)).iter().map(|r| r.area).collect();
        a.sort_by(|x, y| y.total_cmp(x));
        a
    }

    fn near(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6 * y.abs().max(1.0))
    }

    #[test]
    fn a_rectangle_with_a_circle_inside_is_a_plate_with_a_hole_and_a_disc() {
        let mut c = rect(0.0, 0.0, 20.0, 20.0);
        c.push(PlanarCurve::Circle { center: DVec3::new(10.0, 10.0, 0.0), radius: 4.0 });
        let disc = PI * 16.0;
        assert!(near(&areas(&c), &[400.0 - disc, disc]), "{:?}", areas(&c));
        let r = regions(&pieces(&frame(), &c));
        assert_eq!(r.iter().map(|r| r.holes.len()).sum::<usize>(), 1);
    }

    #[test]
    fn a_circle_over_an_edge_makes_three() {
        let mut c = rect(0.0, 0.0, 20.0, 20.0);
        c.push(PlanarCurve::Circle { center: DVec3::new(20.0, 10.0, 0.0), radius: 4.0 });
        let half = PI * 8.0;
        assert!(near(&areas(&c), &[400.0 - half, half, half]), "{:?}", areas(&c));
    }

    #[test]
    fn crossing_lines_and_dangling_ends() {
        // A plus sign through a square: four quarters; a line sticking out bounds nothing.
        let mut c = rect(0.0, 0.0, 10.0, 10.0);
        c.push(PlanarCurve::Line { a: DVec3::new(5.0, -3.0, 0.0), b: DVec3::new(5.0, 13.0, 0.0) });
        c.push(PlanarCurve::Line { a: DVec3::new(0.0, 5.0, 0.0), b: DVec3::new(10.0, 5.0, 0.0) });
        assert!(near(&areas(&c), &[25.0; 4]), "{:?}", areas(&c));
    }

    #[test]
    fn a_slot_of_arcs_and_lines() {
        // Two half circles joined by lines: a stadium of area 10*4 + pi*4.
        let c = vec![
            PlanarCurve::Line { a: DVec3::new(0.0, 0.0, 0.0), b: DVec3::new(10.0, 0.0, 0.0) },
            PlanarCurve::Arc { a: DVec3::new(10.0, 0.0, 0.0), mid: DVec3::new(12.0, 2.0, 0.0), b: DVec3::new(10.0, 4.0, 0.0) },
            PlanarCurve::Line { a: DVec3::new(10.0, 4.0, 0.0), b: DVec3::new(0.0, 4.0, 0.0) },
            PlanarCurve::Arc { a: DVec3::new(0.0, 4.0, 0.0), mid: DVec3::new(-2.0, 2.0, 0.0), b: DVec3::new(0.0, 0.0, 0.0) },
        ];
        assert!(near(&areas(&c), &[40.0 + PI * 4.0]), "{:?}", areas(&c));
    }

    #[test]
    fn two_overlapping_circles_make_three() {
        let c = vec![
            PlanarCurve::Circle { center: DVec3::ZERO, radius: 5.0 },
            PlanarCurve::Circle { center: DVec3::new(6.0, 0.0, 0.0), radius: 5.0 },
        ];
        let a = areas(&c);
        assert_eq!(a.len(), 3, "{a:?}");
        assert!((a.iter().sum::<f64>() - (2.0 * PI * 25.0 - a[2])).abs() < 1e-6, "{a:?}");
    }
}
