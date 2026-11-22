//! Exact geometry Parasolid leaves implicit: points on intersection curves and rolling-ball blends.
//!
//! An INTERSECTION is stored as a few chart points (chordal error up to a tenth of a millimeter), and a
//! BLENDED_EDGE only as its two supports, the spine and the radius. Both are rebuilt here from closest points on
//! the analytic surfaces, so edges and blend faces meet within a micron.

use shapecraft_kernel::brep::{NurbsSurface, Surface};
use shapecraft_kernel::{DVec3, nurbs};

/// Closest point on a surface and the unit natural normal there; None for surfaces without a closed form.
pub fn project(surfaces: &[Surface], s: usize, p: DVec3) -> Option<(DVec3, DVec3)> {
    let unit = |v: DVec3| (v.length() > 1e-12).then(|| v / v.length());
    let radial = |o: DVec3, a: DVec3| {
        let d = p - o;
        let h = d.dot(a);
        (h, unit(d - h * a))
    };
    match surfaces.get(s)? {
        Surface::Plane { origin, normal, .. } => {
            let n = unit(*normal)?;
            Some((p - (p - *origin).dot(n) * n, n))
        }
        Surface::Cylinder { origin, axis, radius, .. } => {
            let a = unit(*axis)?;
            let (h, e) = radial(*origin, a);
            let e = e?;
            Some((*origin + h * a + *radius * e, e))
        }
        Surface::Cone { origin, axis, radius, half_angle, .. } => {
            // In the half plane through the axis and p, the cone is the line (t cos, R + t sin).
            let a = unit(*axis)?;
            let (h, e) = radial(*origin, a);
            let e = e?;
            let rho = (p - *origin - h * a).length();
            let (s, c) = half_angle.sin_cos();
            let t = h * c + (rho - radius) * s;
            Some((*origin + t * c * a + (radius + t * s) * e, c * e - s * a))
        }
        Surface::Sphere { center, radius, .. } => {
            let e = unit(p - *center)?;
            Some((*center + *radius * e, e))
        }
        Surface::Torus { center, axis, major, minor, .. } => {
            let a = unit(*axis)?;
            let (_, e) = radial(*center, a);
            let tube = *center + *major * e?;
            let n = unit(p - tube)?;
            Some((tube + *minor * n, n))
        }
        Surface::Offset { basis, distance } => {
            let (f, n) = project(surfaces, *basis, p)?;
            Some((f + *distance * n, n))
        }
        Surface::BSpline(s) => project_nurbs(s, p),
        _ => None,
    }
}

/// A spline surface's point and its u and v derivatives. Past its edges the surface carries on straight, as the
/// modeller that wrote the file extends it (a blend's spine stored beyond a support's edge is a straight line there):
/// from the nearest point of the edge along the first derivatives, with the twist so both directions stay consistent.
fn nurbs_d1(s: &NurbsSurface, fu: &[f64], fv: &[f64], u: f64, v: f64) -> Option<(DVec3, DVec3, DVec3)> {
    let (pu, pv) = (s.u_degree as usize, s.v_degree as usize);
    let (u0, u1, v0, v1) = (*fu.get(pu)?, *fu.get(s.nu)?, *fv.get(pv)?, *fv.get(s.nv)?);
    // Round a direction where the surface closes up, it carries on from its other end.
    let (cu, cv) = closes(s);
    let wrap = |t: f64, a: f64, b: f64, closed: bool| if closed && b > a { a + (t - a).rem_euclid(b - a) } else { t };
    let (u, v) = (wrap(u, u0, u1, cu), wrap(v, v0, v1, cv));
    let (uc, vc) = (u.clamp(u0, u1), v.clamp(v0, v1));
    let (du, dv) = (u - uc, v - vc);
    let (p, su, sv) = nurbs_inside(s, fu, fv, uc, vc)?;
    if du == 0.0 && dv == 0.0 {
        return Some((p, su, sv));
    }
    // The twist, from the v derivative a little way along u (inside the range).
    let h = (u1 - u0) * 1e-6;
    let (ua, ub) = if uc + h <= u1 { (uc, uc + h) } else { (uc - h, uc) };
    let suv = (nurbs_inside(s, fu, fv, ub, vc)?.2 - nurbs_inside(s, fu, fv, ua, vc)?.2) / (ub - ua);
    Some((p + du * su + dv * sv + du * dv * suv, su + dv * suv, sv + du * suv))
}

/// Whether the surface closes up in u and in v: its poles wrap round (a periodic surface written with its full knot
/// vector repeats its first `degree` pole rows at the end).
fn closes(s: &NurbsSurface) -> (bool, bool) {
    let (pu, pv) = (s.u_degree as usize, s.v_degree as usize);
    let at = |i: usize, j: usize| s.poles.get(i * s.nv + j).copied();
    let same = |a: Option<DVec3>, b: Option<DVec3>| a.zip(b).is_some_and(|(a, b)| a.distance(b) < 1e-9 * (1.0 + a.length()));
    let u = pu >= 1 && s.nu > 2 * pu && (0..pu).all(|i| (0..s.nv).all(|j| same(at(i, j), at(s.nu - pu + i, j))));
    let v = pv >= 1 && s.nv > 2 * pv && (0..pv).all(|j| (0..s.nu).all(|i| same(at(i, j), at(i, s.nv - pv + j))));
    (u, v)
}

/// [`nurbs_d1`] within the surface's own range.
fn nurbs_inside(s: &NurbsSurface, fu: &[f64], fv: &[f64], u: f64, v: f64) -> Option<(DVec3, DVec3, DVec3)> {
    let (pu, pv) = (s.u_degree as usize, s.v_degree as usize);
    let (su, sv) = (nurbs::span(fu, pu, u).unwrap_or(pu), nurbs::span(fv, pv, v).unwrap_or(pv));
    let (nu, du) = nurbs::basis_ders(fu, pu, su, u);
    let (nv, dv) = nurbs::basis_ders(fv, pv, sv, v);
    // Keep accumulation local to the active poles; extrapolated basis functions can
    // be very large, so multiplying them by a global origin loses precision.
    let anchor = *s.poles.get((su - pu) * s.nv + (sv - pv))?;
    let (mut a, mut au, mut av) = (DVec3::ZERO, DVec3::ZERO, DVec3::ZERO);
    let (mut w, mut wu, mut wv) = (0.0, 0.0, 0.0);
    for i in 0..=pu {
        for j in 0..=pv {
            let k = (su - pu + i) * s.nv + (sv - pv + j);
            let pole = *s.poles.get(k)? - anchor;
            let wt = s.weights.as_ref().map_or(Some(1.0), |ws| ws.get(k).copied())?;
            let (b, bu, bv) = (nu[i] * nv[j] * wt, du[i] * nv[j] * wt, nu[i] * dv[j] * wt);
            a += b * pole;
            au += bu * pole;
            av += bv * pole;
            w += b;
            wu += bu;
            wv += bv;
        }
    }
    if w.abs() < 1e-300 {
        return None;
    }
    let local = a / w;
    Some((anchor + local, (au - wu * local) / w, (av - wv * local) / w))
}

/// Closest point on a spline surface (best of a grid of samples, then Gauss–Newton steps) and the unit natural
/// normal (Su × Sv) there.
fn project_nurbs(s: &NurbsSurface, p: DVec3) -> Option<(DVec3, DVec3)> {
    let (fu, fv) = (nurbs::expand(&s.u_knots, &s.u_mults), nurbs::expand(&s.v_knots, &s.v_mults));
    let (pu, pv) = (s.u_degree as usize, s.v_degree as usize);
    if s.nu <= pu || s.nv <= pv || fu.len() != s.nu + pu + 1 || fv.len() != s.nv + pv + 1 || s.poles.len() != s.nu * s.nv {
        return None;
    }
    let (u0, u1, v0, v1) = (fu[pu], fu[s.nu], fv[pv], fv[s.nv]);
    if !(u1 > u0 && v1 > v0) {
        return None;
    }
    let (gu, gv) = ((s.nu - pu) * 3 + 2, (s.nv - pv) * 3 + 2);
    let (gu, gv) = (gu.min(16), gv.min(16));
    let mut best = (f64::INFINITY, u0, v0);
    for i in 0..=gu {
        for j in 0..=gv {
            let (u, v) = (u0 + (u1 - u0) * i as f64 / gu as f64, v0 + (v1 - v0) * j as f64 / gv as f64);
            let d = nurbs_d1(s, &fu, &fv, u, v)?.0.distance_squared(p);
            if d < best.0 {
                best = (d, u, v);
            }
        }
    }
    // Include each knot-span midpoint: a uniform grid can miss a short span entirely.
    for uw in s.u_knots.windows(2).take(16) {
        for vw in s.v_knots.windows(2).take(16) {
            for u in [uw[0], (uw[0] + uw[1]) * 0.5, uw[1]] {
                for v in [vw[0], (vw[0] + vw[1]) * 0.5, vw[1]] {
                    let d = nurbs_d1(s, &fu, &fv, u, v)?.0.distance_squared(p);
                    if d < best.0 {
                        best = (d, u, v);
                    }
                }
            }
        }
    }
    let (_, mut u, mut v) = best;
    for _ in 0..50 {
        let (q, su, sv) = nurbs_d1(s, &fu, &fv, u, v)?;
        let d = p - q;
        let (r1, r2) = (su.dot(d), sv.dot(d));
        let (ga, gb, gc) = (su.dot(su), su.dot(sv), sv.dot(sv));
        // Differentiate the first derivatives within the current knot spans. The distance
        // Hessian includes curvature, which matters when the point is far off the surface.
        let difference = |full: &[f64], degree: usize, count: usize, t: f64| {
            let i = nurbs::span(full, degree, t).unwrap_or(degree);
            let h = (full[i + 1] - full[i]).abs() * 1e-5;
            let h = h.max((full[count] - full[degree]).abs() * 1e-10);
            if t - h < full[i] {
                (t, t + h)
            } else if t + h > full[i + 1] {
                (t - h, t)
            } else {
                (t - h, t + h)
            }
        };
        let (ul, ur) = difference(&fu, pu, s.nu, u);
        let (vl, vr) = difference(&fv, pv, s.nv, v);
        let (_, sul, svul) = nurbs_d1(s, &fu, &fv, ul, v)?;
        let (_, sur, svur) = nurbs_d1(s, &fu, &fv, ur, v)?;
        let (_, _, svvl) = nurbs_d1(s, &fu, &fv, u, vl)?;
        let (_, _, svvr) = nurbs_d1(s, &fu, &fv, u, vr)?;
        let (mut a, mut b, mut c) =
            (ga - d.dot((sur - sul) / (ur - ul)), gb - d.dot((svur - svul) / (ur - ul)), gc - d.dot((svvr - svvl) / (vr - vl)));
        if a <= 0.0 || c <= 0.0 || a * c - b * b <= 1e-12 * ga * gc {
            (a, b, c) = (ga, gb, gc);
        }
        let det = a * c - b * b;
        if det.abs() < 1e-300 {
            break;
        }
        // Past the edges the surface carries on straight: blends and their boundaries reach there.
        let (eu, ev) = ((u1 - u0) * 10.0, (v1 - v0) * 10.0);
        let (du, dv) = ((r1 * c - r2 * b) / det, (a * r2 - b * r1) / det);
        let mut scale = 1.0;
        let mut next = None;
        // Near a repeated knot a full Newton step can jump onto the polynomial extension of another span.
        // Only accept a step that improves the actual distance, rather than trusting the tangent plane alone.
        // At tiny steps the distance comparison loses precision; let the iteration settle.
        for _ in 0..14 {
            let nu = (u + scale * du).clamp(u0 - eu, u1 + eu);
            let nv = (v + scale * dv).clamp(v0 - ev, v1 + ev);
            if let Some((at, tu, tv)) = nurbs_d1(s, &fu, &fv, nu, nv)
                && (at.distance_squared(p) <= q.distance_squared(p)
                    || ((du * su + dv * sv).length() < 1e-6
                        && (tu.dot(p - at).abs() / tu.length() + tv.dot(p - at).abs() / tv.length())
                            < (r1.abs() / su.length() + r2.abs() / sv.length())))
            {
                next = Some((nu, nv));
                break;
            }
            scale *= 0.5;
        }
        let Some((nu, nv)) = next else { break };
        let done = (nu - u).abs() < 1e-14 * (u1 - u0) && (nv - v).abs() < 1e-14 * (v1 - v0);
        (u, v) = (nu, nv);
        if done {
            break;
        }
    }
    let (q, su, sv) = nurbs_d1(s, &fu, &fv, u, v)?;
    let n = su.cross(sv);
    (n.length() > 1e-300).then(|| (q, n / n.length()))
}

/// The point nearest `p` on the curve where two supports, each moved out by its offset, meet.
pub fn on_both(surfaces: &[Surface], a: Support, b: Support, mut p: DVec3) -> Option<DVec3> {
    for _ in 0..60 {
        let (fa, na) = a.touch(surfaces, p)?;
        let (fb, nb) = b.touch(surfaces, p)?;
        let (fa, fb) = (fa + a.offset() * na, fb + b.offset() * nb);
        // The point closest to p on the line where both tangent planes meet.
        let c = na.dot(nb);
        let det = 1.0 - c * c;
        if det < 1e-12 {
            return None;
        }
        let (ra, rb) = ((fa - p).dot(na), (fb - p).dot(nb));
        let step = (ra - c * rb) / det * na + (rb - c * ra) / det * nb;
        // Check the residuals at the point actually used to calculate these projections.
        // Reprojecting after a negligible step can magnify rounding on extrapolated spans.
        if step.length() < 1e-9 && (fa - p).length() < 1e-7 && (fb - p).length() < 1e-7 {
            return Some(p);
        }
        p += step;
    }
    None
}

/// Chart points made dense (at least `min` pieces overall) and moved onto the exact curve by `exact`.
///
/// The chart's own points are points of the curve and stay, in their order. A point between two of them is added
/// only when `exact` puts it where it belongs: across the chord from its seed (within the chord's sag) and no
/// further along the chord than half a step. Where a surface stops short of the curve the foot points pile up
/// instead, and those are left out, so the result never doubles back. None when fewer than two points remain.
pub fn densify(chart: &[DVec3], min: usize, exact: impl Fn(DVec3) -> Option<DVec3>) -> Option<Vec<DVec3>> {
    let pieces = chart.len().saturating_sub(1).max(1);
    let m = min.div_ceil(pieces).clamp(1, 16);
    let mut out = Vec::with_capacity(pieces * m + 1);
    for (i, w) in chart.windows(2).enumerate() {
        let length = w[0].distance(w[1]);
        let dir = (w[1] - w[0]) / length.max(1e-300);
        let (reach, slide) = (0.25 * length + 1e-6, 0.45 * length / m as f64 + 1e-9);
        // A chart point is on the curve already; the exact one is taken when it is the same point to a micron.
        let own = |p: DVec3| exact(p).filter(|q| q.distance(p) <= 1e-3).unwrap_or(p);
        out.push(own(w[0]));
        for k in 1..m {
            let seed = w[0].lerp(w[1], k as f64 / m as f64);
            if let Some(q) = exact(seed).filter(|q| q.distance(seed) <= reach && (*q - seed).dot(dir).abs() <= slide) {
                out.push(q);
            }
        }
        if i == pieces - 1 {
            out.push(own(w[1]));
        }
    }
    (out.len() >= 2).then_some(out)
}

/// The ball centers of a blend, dense and exact: the spine's chart points with points between them, each moved onto
/// the curve where the two offset supports meet. A spine can run on past where a support ends (the file keeps the
/// whole intersection, the blend face uses a part of it): a point that has to move far from the chart to reach both
/// supports isn't on the blend, and the longest stretch of points that stay by the chart is what the blend follows.
fn spine_on_supports(surfaces: &[Surface], s: [Support; 2], chart: &[DVec3]) -> Option<Vec<DVec3>> {
    let pieces = chart.len().checked_sub(1).filter(|&n| n > 0)?;
    let m = 32usize.div_ceil(pieces).clamp(1, 16);
    let mut placed: Vec<Option<DVec3>> = Vec::with_capacity(pieces * m + 1);
    for (i, w) in chart.windows(2).enumerate() {
        // A point of the curve lies across the chord from its seed, by the chord's sag (a small part of its length).
        // One that moved further, or along the chord by as much as the step to the next seed, landed on another
        // branch or where a support stops: the points pile up there.
        let length = w[0].distance(w[1]);
        let dir = (w[1] - w[0]) / length.max(1e-300);
        let (reach, slide) = (0.25 * length + 1e-6, 0.45 * length / m as f64 + 1e-9);
        let last = if i == pieces - 1 { m } else { m - 1 };
        for k in 0..=last {
            let seed = w[0].lerp(w[1], k as f64 / m as f64);
            placed.push(on_both(surfaces, s[0], s[1], seed).filter(|q| q.distance(seed) <= reach && (*q - seed).dot(dir).abs() <= slide));
        }
    }
    // The longest run of placed points that keep moving on.
    let (mut best, mut run): (Vec<DVec3>, Vec<DVec3>) = (Vec::new(), Vec::new());
    for p in placed.into_iter().chain(std::iter::once(None)) {
        match p {
            Some(p) if run.last().is_none_or(|q: &DVec3| q.distance(p) > 1e-9) => run.push(p),
            Some(_) => {}
            None => {
                if run.len() > best.len() {
                    best = std::mem::take(&mut run);
                }
                run.clear();
            }
        }
    }
    (best.len() >= 4).then_some(best)
}

/// The part of a blend's spine its faces need: from three stations before the first one any face corner lies at to
/// three after the last, with stations added in its long stretches so the surface follows the ball closely.
/// A closed spine, or corners spread over most of it, keeps everything.
fn stretch_for(surfaces: &[Surface], s: [Support; 2], spine: Vec<DVec3>, near: &[DVec3]) -> Vec<DVec3> {
    let n = spine.len();
    let closed = spine[0].distance(spine[n - 1]) < 1e-6;
    // A corner lies on the ball's section at its station: across the spine from the center there. Among the stations
    // about a ball's radius away, that is the one whose tangent the corner is least along.
    let tangent = |i: usize| (spine[(i + 1).min(n - 1)] - spine[i.saturating_sub(1)]).normalize_or_zero();
    let radius = s[0].radius().max(s[1].radius());
    let station = |p: &DVec3| {
        let along = |i: usize| (*p - spine[i]).dot(tangent(i)).abs();
        (0..n)
            .filter(|&i| spine[i].distance(*p) <= 1.5 * radius + 1e-6)
            .min_by(|&i, &j| along(i).total_cmp(&along(j)))
            .or_else(|| (0..n).min_by(|&i, &j| spine[i].distance_squared(*p).total_cmp(&spine[j].distance_squared(*p))))
    };
    let used: Vec<usize> = near.iter().filter(|p| p.is_finite()).filter_map(station).collect();
    let (Some(&lo), Some(&hi)) = (used.iter().min(), used.iter().max()) else { return spine };
    let (lo, hi) = (lo.saturating_sub(3), (hi + 3).min(n - 1));
    if closed || hi - lo < 1 || (hi - lo) * 5 >= (n - 1) * 4 {
        return spine;
    }
    let mut part = spine[lo..=hi].to_vec();
    // Even out the stations: a stretch much longer than the rest is halved (a few rounds), short ones are left alone.
    let goal = part.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>() / 32.0;
    for _ in 0..6 {
        let mut finer = Vec::with_capacity(part.len() * 2);
        for w in part.windows(2) {
            finer.push(w[0]);
            let (mid, step) = ((w[0] + w[1]) / 2.0, w[0].distance(w[1]));
            if step > 1.5 * goal
                && let Some(q) = on_both(surfaces, s[0], s[1], mid).filter(|q| q.distance(mid) <= 0.25 * step)
            {
                finer.push(q);
            }
        }
        finer.push(part[part.len() - 1]);
        if finer.len() == part.len() || finer.len() > 256 {
            break;
        }
        part = finer;
    }
    part
}

/// A support of a blend: what the ball rolls on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Support {
    /// A surface (its index) and the signed offset along its natural normal that puts it through the ball's center.
    Surface(usize, f64),
    /// The edge where two surfaces meet, and the ball's radius: a ball rolling between a face and an edge (Parasolid
    /// keeps the edge as a blend of no radius between the edge's faces). The ball's centers are a pipe round it.
    Edge(usize, usize, f64),
}

impl Support {
    /// How far the ball's center is from the support.
    pub fn radius(self) -> f64 {
        match self {
            Support::Surface(_, d) => d.abs(),
            Support::Edge(_, _, r) => r.abs(),
        }
    }

    /// The signed distance along `touch`'s direction from the support out to the ball's center.
    pub fn offset(self) -> f64 {
        match self {
            Support::Surface(_, d) => d,
            Support::Edge(_, _, r) => r.abs(),
        }
    }

    /// The point of the support nearest `p` and the unit direction the support's offset is taken along there (a
    /// surface's natural normal; from an edge, straight out towards `p`).
    pub fn touch(self, surfaces: &[Surface], p: DVec3) -> Option<(DVec3, DVec3)> {
        match self {
            Support::Surface(s, _) => project(surfaces, s, p),
            Support::Edge(a, b, _) => {
                let q = on_both(surfaces, Support::Surface(a, 0.0), Support::Surface(b, 0.0), p)?;
                let out = p - q;
                (out.length() > 1e-9).then(|| (q, out / out.length()))
            }
        }
    }
}

/// The rolling-ball blend between two supports along `spine` (ball centers in the spine's own order), as a
/// rational surface: u along the spine, v around the ball from the first support to the second.
///
/// `near` are the corners of the faces that lie on the blend. The file keeps the whole curve the ball's center could
/// run along, often far past the faces (straight on, beyond the supports' edges); with corners given, the surface is
/// made for the stretch they need and a little more, where its stations are close enough to follow it well.
pub fn rolling_ball(surfaces: &[Surface], s: [Support; 2], spine: &[DVec3], near: &[DVec3]) -> Result<NurbsSurface, &'static str> {
    let spine = spine_on_supports(surfaces, s, spine).ok_or("ball centers do not meet the offset supports")?;
    let spine = stretch_for(surfaces, s, spine, near);
    // One extra station past each end, so the face's end arcs sit inside the surface. Where a support ends there
    // (a short step is tried before giving up), the blend ends with its spine.
    let n = spine.len();
    let past = |end: DVec3, inner: DVec3| {
        [1.0, 0.5, 0.25].into_iter().find_map(|k| {
            let seed = end + (end - inner) * k;
            // It has to carry on outwards: at least half the step past the end, and not swung far aside.
            let (step, out) = (seed.distance(end), (seed - end).normalize_or_zero());
            on_both(surfaces, s[0], s[1], seed).filter(|q| (*q - end).dot(out) >= 0.5 * step && q.distance(seed) <= 0.75 * step)
        })
    };
    let (before, after) = (past(spine[0], spine[1]), past(spine[n - 1], spine[n - 2]));
    let centers: Vec<DVec3> = before.into_iter().chain(spine.iter().copied()).chain(after).collect();
    let mut rows: Vec<[[f64; 4]; 5]> = Vec::with_capacity(centers.len());
    for c in &centers {
        let (f0, _) = s[0].touch(surfaces, *c).ok_or("cannot project to the first blend support")?;
        let (f1, _) = s[1].touch(surfaces, *c).ok_or("cannot project to the second blend support")?;
        let (d0, d1) = (f0 - *c, f1 - *c);
        let r = (d0.length() + d1.length()) / 2.0;
        let mid = d0.normalize_or_zero() + d1.normalize_or_zero();
        if r < 1e-9 || mid.length() < 1e-6 {
            return Err("degenerate circular section of a blend");
        }
        // Two circular arcs keep each weight near one, even where a blend opens almost 180 degrees. A
        // single arc's near-zero middle weights can become negative when interpolated along the spine.
        let w = (d0.angle_between(d1) / 4.0).cos();
        let middle = mid.normalize();
        let m0 = *c + (d0.normalize() + middle).normalize() * (r / w);
        let m1 = *c + (middle + d1.normalize()).normalize() * (r / w);
        let h = |p: DVec3, w: f64| [p.x * w, p.y * w, p.z * w, w];
        rows.push([h(f0, 1.0), h(m0, w), h(*c + middle * r, 1.0), h(m1, w), h(f1, 1.0)]);
    }
    // Ball centers parametrize u (chord length); every pole row is interpolated in homogeneous coordinates so
    // the surface goes through each station's arc exactly.
    let params = chord_params(&centers);
    let (knots, mults, full) = averaged_knots(&params, 3).ok_or("not enough distinct blend stations")?;
    let mut cols: Vec<Vec<[f64; 4]>> = Vec::new();
    for j in 0..5 {
        let pts: Vec<[f64; 4]> = rows.iter().map(|r| r[j]).collect();
        cols.push(interpolate(&params, &full, 3, &pts).ok_or("cannot interpolate the blend stations")?);
    }
    let nu = cols[0].len();
    let mut poles = Vec::with_capacity(nu * 5);
    let mut weights = Vec::with_capacity(nu * 5);
    for i in 0..nu {
        for col in &cols {
            let [x, y, z, w] = col[i];
            if w < 1e-9 {
                return Err("nonpositive rational blend weight");
            }
            poles.push(DVec3::new(x / w, y / w, z / w));
            weights.push(w);
        }
    }
    Ok(NurbsSurface {
        u_degree: 3,
        v_degree: 2,
        nu,
        nv: 5,
        poles,
        weights: Some(weights),
        u_knots: knots,
        u_mults: mults,
        v_knots: vec![0.0, 0.5, 1.0],
        v_mults: vec![3, 2, 3],
        u_periodic: false,
        v_periodic: false,
    })
}

fn chord_params(pts: &[DVec3]) -> Vec<f64> {
    let mut t = vec![0.0];
    for w in pts.windows(2) {
        t.push(t[t.len() - 1] + w[0].distance(w[1]).max(1e-12));
    }
    let total = t[t.len() - 1];
    t.iter().map(|x| x / total).collect()
}

/// Knots by averaging (Piegl–Tiller 9.8): distinct knots, multiplicities and the full vector.
fn averaged_knots(params: &[f64], p: usize) -> Option<(Vec<f64>, Vec<u32>, Vec<f64>)> {
    let n = params.len().checked_sub(1)?;
    if n < p {
        return None;
    }
    let mut full = vec![0.0; p + 1];
    for j in 1..=n - p {
        full.push(params[j..j + p].iter().sum::<f64>() / p as f64);
    }
    full.extend(std::iter::repeat_n(1.0, p + 1));
    let (mut knots, mut mults): (Vec<f64>, Vec<u32>) = (Vec::new(), Vec::new());
    for &k in &full {
        match knots.last() {
            Some(&last) if (k - last).abs() < 1e-12 => *mults.last_mut()? += 1,
            _ => {
                knots.push(k);
                mults.push(1);
            }
        }
    }
    Some((knots, mults, full))
}

/// Global interpolation: poles of the degree-`p` spline on `full` that passes through `pts` at `params`.
// Row operations read best with explicit indices.
#[allow(clippy::needless_range_loop)]
fn interpolate(params: &[f64], full: &[f64], p: usize, pts: &[[f64; 4]]) -> Option<Vec<[f64; 4]>> {
    let n = pts.len();
    let mut a: Vec<Vec<f64>> = params.iter().map(|&t| nurbs::basis_all(full, p, t)).collect();
    if a.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut b: Vec<[f64; 4]> = pts.to_vec();
    // Gaussian elimination with partial pivoting.
    for col in 0..n {
        let piv = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
            for k in 0..4 {
                b[row][k] -= f * b[col][k];
            }
        }
    }
    let mut x = vec![[0.0; 4]; n];
    for row in (0..n).rev() {
        for k in 0..4 {
            let s: f64 = (row + 1..n).map(|j| a[row][j] * x[j][k]).sum();
            x[row][k] = (b[row][k] - s) / a[row][row];
        }
    }
    x.iter().flatten().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &NurbsSurface, u: f64, v: f64) -> DVec3 {
        let nu = nurbs::basis_all(&nurbs::expand(&s.u_knots, &s.u_mults), s.u_degree as usize, u);
        let nv = nurbs::basis_all(&nurbs::expand(&s.v_knots, &s.v_mults), s.v_degree as usize, v);
        let w = s.weights.as_ref().map_or(vec![1.0; s.poles.len()], Clone::clone);
        let (mut p, mut ws) = (DVec3::ZERO, 0.0);
        for i in 0..s.nu {
            for j in 0..s.nv {
                let k = nu[i] * nv[j] * w[i * s.nv + j];
                p += k * s.poles[i * s.nv + j];
                ws += k;
            }
        }
        p / ws
    }

    #[test]
    fn a_ball_rolled_between_two_planes_is_a_quarter_cylinder() {
        // Floor z = 0 and wall y = 0, ball of radius 2 inside the corner (x runs along the edge).
        let surfaces = vec![
            Surface::Plane { origin: DVec3::ZERO, normal: DVec3::Z, x: DVec3::X },
            Surface::Plane { origin: DVec3::ZERO, normal: DVec3::Y, x: DVec3::X },
        ];
        let spine: Vec<DVec3> = (0..6).map(|i| DVec3::new(i as f64, 2.0, 2.0)).collect();
        let s = rolling_ball(&surfaces, [Support::Surface(0, 2.0), Support::Surface(1, 2.0)], &spine, &[]).expect("blend");
        for u in [0.0, 0.3, 0.5, 0.9, 1.0] {
            for v in [0.0, 0.25, 0.5, 1.0] {
                let p = eval(&s, u, v);
                let off = (p.y - 2.0).hypot(p.z - 2.0);
                assert!((off - 2.0).abs() < 1e-9, "u {u} v {v}: {p} is {off} from the spine");
            }
        }
        assert!((eval(&s, 0.5, 0.0).z).abs() < 1e-9, "v = 0 touches the first support");
        assert!((eval(&s, 0.5, 1.0).y).abs() < 1e-9, "v = 1 touches the second support");
    }

    #[test]
    fn a_ball_rolled_between_a_floor_and_an_edge_touches_the_edge() {
        // Floor z = 0; the edge is where the planes x = 0 and z = 3 meet (a line along y). A ball of radius 2 on the
        // floor touching that edge has its center at x = √3, z = 2.
        let surfaces = vec![
            Surface::Plane { origin: DVec3::ZERO, normal: DVec3::Z, x: DVec3::X },
            Surface::Plane { origin: DVec3::ZERO, normal: DVec3::X, x: DVec3::Y },
            Surface::Plane { origin: DVec3::new(0.0, 0.0, 3.0), normal: DVec3::Z, x: DVec3::X },
        ];
        let x = 3f64.sqrt();
        let spine: Vec<DVec3> = (0..6).map(|i| DVec3::new(x + 0.05, i as f64, 2.0)).collect();
        let s = rolling_ball(&surfaces, [Support::Surface(0, 2.0), Support::Edge(1, 2, 2.0)], &spine, &[]).expect("blend");
        for u in [0.0, 0.3, 0.5, 0.9, 1.0] {
            for v in [0.0, 0.25, 0.5, 1.0] {
                let p = eval(&s, u, v);
                let off = (p.x - x).hypot(p.z - 2.0);
                assert!((off - 2.0).abs() < 1e-9, "u {u} v {v}: {p} is {off} from the ball centers");
            }
        }
        assert!(eval(&s, 0.5, 0.0).z.abs() < 1e-9, "v = 0 touches the floor");
        let e = eval(&s, 0.5, 1.0);
        assert!(e.x.abs() < 1e-9 && (e.z - 3.0).abs() < 1e-9, "v = 1 touches the edge: {e}");
    }

    #[test]
    fn projection_finds_a_short_knot_span() {
        // A narrow triangular ridge in an extrusion; uniform parameter samples miss it.
        let mut poles = Vec::new();
        for x in [0.0, 0.0, 10.0, 0.0, 0.0] {
            for y in [0.0, 2.0] {
                poles.push(DVec3::new(x, y, 0.0));
            }
        }
        let s = NurbsSurface {
            u_degree: 1,
            v_degree: 1,
            nu: 5,
            nv: 2,
            poles,
            weights: None,
            u_knots: vec![0.0, 0.172, 0.173, 0.174, 1.0],
            u_mults: vec![2, 1, 1, 1, 2],
            v_knots: vec![0.0, 1.0],
            v_mults: vec![2, 2],
            u_periodic: false,
            v_periodic: false,
        };
        let (q, _) = project_nurbs(&s, DVec3::new(9.0, 1.0, 0.2)).expect("projection");
        assert!(q.distance(DVec3::new(9.0, 1.0, 0.0)) < 1e-8, "{q:?}");
    }

    #[test]
    fn a_cone_and_a_plane_meet_on_their_exact_curve() {
        let surfaces = vec![
            Surface::Cone { origin: DVec3::ZERO, axis: DVec3::Z, x: DVec3::X, radius: 5.0, half_angle: 0.1 },
            Surface::Plane { origin: DVec3::new(0.0, 0.0, 1.0), normal: DVec3::Z, x: DVec3::X },
        ];
        let p = on_both(&surfaces, Support::Surface(0, 0.0), Support::Surface(1, 0.0), DVec3::new(3.0, 3.0, 0.7)).expect("on both");
        assert!((p.z - 1.0).abs() < 1e-9);
        assert!((p.truncate().length() - (5.0 + 0.1f64.tan())).abs() < 1e-9);
    }
    /// z = x² over x in [-2, 2], y in [0, 3]: a quadratic Bézier in x (poles at x = -2, 0, 2, heights 4, -4, 4),
    /// straight in y.
    fn parabola_sheet() -> NurbsSurface {
        let mut poles = Vec::new();
        for (x, z) in [(-2.0, 4.0), (0.0, -4.0), (2.0, 4.0)] {
            for y in [0.0, 3.0] {
                poles.push(DVec3::new(x, y, z));
            }
        }
        NurbsSurface {
            u_degree: 2,
            v_degree: 1,
            nu: 3,
            nv: 2,
            poles,
            weights: None,
            u_knots: vec![0.0, 1.0],
            u_mults: vec![3, 3],
            v_knots: vec![0.0, 1.0],
            v_mults: vec![2, 2],
            u_periodic: false,
            v_periodic: false,
        }
    }

    #[test]
    fn spline_derivatives_match_differences() {
        let s = parabola_sheet();
        let (fu, fv) = (nurbs::expand(&s.u_knots, &s.u_mults), nurbs::expand(&s.v_knots, &s.v_mults));
        let (p, su, sv) = nurbs_d1(&s, &fu, &fv, 0.3, 0.6).expect("eval");
        let h = 1e-6;
        let (pu, _, _) = nurbs_d1(&s, &fu, &fv, 0.3 + h, 0.6).expect("eval");
        let (pv, _, _) = nurbs_d1(&s, &fu, &fv, 0.3, 0.6 + h).expect("eval");
        assert!(((pu - p) / h - su).length() < 1e-4, "{su} vs {}", (pu - p) / h);
        assert!(((pv - p) / h - sv).length() < 1e-4, "{sv} vs {}", (pv - p) / h);
        // On the parabola: z = x².
        assert!((p.z - p.x * p.x).abs() < 1e-12);
    }

    #[test]
    fn far_extrapolation_preserves_a_translated_linear_bezier() {
        let origin = DVec3::new(143.123456789, -97.123456789, 63.123456789);
        let mut poles = Vec::new();
        for i in 0..5 {
            for j in 0..2 {
                poles.push(origin + DVec3::new(i as f64 / 512.0, j as f64, 0.0));
            }
        }
        let s = NurbsSurface {
            u_degree: 4,
            v_degree: 1,
            nu: 5,
            nv: 2,
            poles,
            weights: None,
            u_knots: vec![0.0, 1.0],
            u_mults: vec![5, 5],
            v_knots: vec![0.0, 1.0],
            v_mults: vec![2, 2],
            u_periodic: false,
            v_periodic: false,
        };
        let (fu, fv) = (nurbs::expand(&s.u_knots, &s.u_mults), nurbs::expand(&s.v_knots, &s.v_mults));
        let (q, su, sv) = nurbs_d1(&s, &fu, &fv, 60.3, 0.4).expect("evaluation");
        assert!(q.distance(origin + DVec3::new(4.0 * 60.3 / 512.0, 0.4, 0.0)) < 1e-8, "{q:?}");
        assert!(su.distance(DVec3::new(4.0 / 512.0, 0.0, 0.0)) < 1e-9, "{su:?}");
        assert!(sv.distance(DVec3::Y) < 1e-8, "{sv:?}");
    }

    #[test]
    fn projection_near_the_curvature_center_converges_to_the_known_foot() {
        let foot = DVec3::new(0.4, 1.2, 0.16);
        let normal = DVec3::new(-0.8, 0.0, 1.0).normalize();
        let (q, _) = project_nurbs(&parabola_sheet(), foot + 0.5 * normal).expect("projection");
        assert!(q.distance(foot) < 1e-9, "{q:?}");
    }

    #[test]
    fn closest_point_on_a_spline_surface_and_a_little_past_its_edge() {
        let surfaces = vec![Surface::BSpline(parabola_sheet())];
        // Just above the vertex (closer than its radius of curvature, 1/2): the foot is the vertex, normal vertical.
        let (f, n) = project(&surfaces, 0, DVec3::new(0.0, 1.0, 0.3)).expect("project");
        assert!(f.distance(DVec3::new(0.0, 1.0, 0.0)) < 1e-9, "{f}");
        assert!(n.cross(DVec3::Z).length() < 1e-9);
        // Off the side: the straight direction carries on, so the foot is past y = 3, still on z = x².
        let (f, _) = project(&surfaces, 0, DVec3::new(1.0, 4.0, 1.0)).expect("project");
        assert!((f.y - 4.0).abs() < 1e-9 && (f.z - f.x * f.x).abs() < 1e-9, "{f}");
    }
}
