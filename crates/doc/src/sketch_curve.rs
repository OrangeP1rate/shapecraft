//! Ellipses and splines as curves with a parameter: where they cross other curves, and a piece of one as a curve of
//! its own (a control-point spline, whatever it was cut from).

use std::f64::consts::TAU;

use glam::{DVec2, DVec3};
use shapecraft_kernel::{nurbs, spline};

use crate::sketch::{SketchEntity, bspline_at};

/// An ellipse or a spline, its parameter running from 0 to 1.
pub(crate) enum Curve {
    Ellipse { center: DVec2, rx: f64, ry: f64, angle: f64 },
    Fit(Box<spline::Fit>),
    Control(Vec<DVec2>),
}

/// A curve is followed in this many steps to find where it crosses another.
const STEPS: usize = 256;

impl Curve {
    pub fn of(e: &SketchEntity) -> Option<Curve> {
        Some(match e {
            SketchEntity::Ellipse { center, rx, ry, angle } => Curve::Ellipse { center: *center, rx: *rx, ry: *ry, angle: *angle },
            SketchEntity::Spline { points } => {
                let flat: Vec<DVec3> = points.iter().map(|p| p.extend(0.0)).collect();
                Curve::Fit(Box::new(spline::fit(&flat)?))
            }
            SketchEntity::ControlSpline { poles } if poles.len() >= 2 => Curve::Control(poles.clone()),
            _ => return None,
        })
    }

    pub fn closed(&self) -> bool {
        matches!(self, Curve::Ellipse { .. })
    }

    /// The point at `t`; a closed curve goes round again past 1.
    pub fn at(&self, t: f64) -> DVec2 {
        match self {
            Curve::Ellipse { center, rx, ry, angle } => {
                let (s, c) = (t * TAU).sin_cos();
                *center + DVec2::from_angle(*angle).rotate(DVec2::new(rx * c, ry * s))
            }
            Curve::Fit(f) => f.point(t).truncate(),
            Curve::Control(poles) => bspline_at(poles, t),
        }
    }

    fn steps(&self) -> impl Iterator<Item = (f64, DVec2)> + '_ {
        (0..=STEPS).map(|k| k as f64 / STEPS as f64).map(|t| (t, self.at(t)))
    }

    /// The parameter nearest to `p`.
    pub fn nearest(&self, p: DVec2) -> f64 {
        self.steps().min_by(|a, b| a.1.distance_squared(p).total_cmp(&b.1.distance_squared(p))).map_or(0.0, |s| s.0)
    }

    /// Where `f` (a signed distance to something) changes sign along the curve, or touches zero at a step.
    pub fn roots(&self, f: impl Fn(DVec2) -> f64) -> Vec<f64> {
        let steps: Vec<(f64, f64)> = self.steps().map(|(t, p)| (t, f(p))).collect();
        let mut out = Vec::new();
        for w in steps.windows(2) {
            let ((mut t0, f0), (mut t1, f1)) = (w[0], w[1]);
            if f0 == 0.0 {
                out.push(t0);
            } else if f0 * f1 < 0.0 {
                for _ in 0..60 {
                    let mid = (t0 + t1) / 2.0;
                    if f(self.at(mid)) * f0 > 0.0 { t0 = mid } else { t1 = mid }
                }
                out.push((t0 + t1) / 2.0);
            }
        }
        if let Some(&(t, 0.0)) = steps.last() {
            out.push(t);
        }
        out
    }

    /// Where this curve crosses `other`, as parameters on both.
    pub fn meets(&self, other: &Curve) -> Vec<(f64, f64)> {
        let (a, b): (Vec<_>, Vec<_>) = (self.steps().collect(), other.steps().collect());
        let mut out: Vec<(f64, f64)> = Vec::new();
        for p in a.windows(2) {
            let (lo, hi) = (p[0].1.min(p[1].1), p[0].1.max(p[1].1));
            for q in b.windows(2) {
                let (ql, qh) = (q[0].1.min(q[1].1), q[0].1.max(q[1].1));
                if hi.x < ql.x || qh.x < lo.x || hi.y < ql.y || qh.y < lo.y {
                    continue;
                }
                let (r, s) = (p[1].1 - p[0].1, q[1].1 - q[0].1);
                let den = r.perp_dot(s);
                if den.abs() < 1e-18 {
                    continue;
                }
                let d = q[0].1 - p[0].1;
                let (u, v) = (d.perp_dot(s) / den, d.perp_dot(r) / den);
                if !(-1e-6..=1.0 + 1e-6).contains(&u) || !(-1e-6..=1.0 + 1e-6).contains(&v) {
                    continue;
                }
                let (mut t, mut w) = (p[0].0 + (p[1].0 - p[0].0) * u, q[0].0 + (q[1].0 - q[0].0) * v);
                // The chords' crossing is close; a few steps of Newton put it on the curves.
                for _ in 0..12 {
                    let h = 1e-6;
                    let gap = other.at(w) - self.at(t);
                    let (dt, dw) = ((self.at(t + h) - self.at(t - h)) / (2.0 * h), (other.at(w + h) - other.at(w - h)) / (2.0 * h));
                    let det = dt.perp_dot(dw);
                    if det.abs() < 1e-18 {
                        break;
                    }
                    let (et, ew) = (gap.perp_dot(dw) / det, gap.perp_dot(dt) / det);
                    (t, w) = (t + et.clamp(-0.02, 0.02), w + ew.clamp(-0.02, 0.02));
                    if et.abs() < 1e-14 && ew.abs() < 1e-14 {
                        break;
                    }
                }
                if t.is_finite() && w.is_finite() && !out.iter().any(|o| (o.0 - t).abs() < 1e-9) {
                    out.push((t, w));
                }
            }
        }
        out
    }

    /// The part from `t0` to `t1` as a control-point spline, its ends exactly at `a` and `b`.
    pub fn piece(&self, t0: f64, t1: f64, a: DVec2, b: DVec2) -> SketchEntity {
        SketchEntity::ControlSpline { poles: control_poles(|u| self.at(t0 + (t1 - t0) * u), a, b) }
    }
}

/// The control points of a clamped uniform cubic spline that follows `curve` (0 to 1) from `a` to `b`: as few as keep
/// it within a hundred-thousandth of its size.
fn control_poles(curve: impl Fn(f64) -> DVec2, a: DVec2, b: DVec2) -> Vec<DVec2> {
    let sample = |m: usize| -> Vec<(f64, DVec2)> { (0..=m).map(|k| k as f64 / m as f64).map(|u| (u, curve(u))).collect() };
    let check = sample(400);
    let (lo, hi) = check.iter().fold((a.min(b), a.max(b)), |(lo, hi), (_, p)| (lo.min(*p), hi.max(*p)));
    let tol = ((hi - lo).length() * 1e-5).max(1e-9);
    let mut best: Option<(f64, Vec<DVec2>)> = None;
    for n in 4..=40 {
        let Some(poles) = least_squares(&sample(8 * n), n, a, b) else { continue };
        let off = check.iter().map(|(u, p)| bspline_at(&poles, *u).distance(*p)).fold(0.0, f64::max);
        if best.as_ref().is_none_or(|(o, _)| off < *o) {
            best = Some((off, poles));
        }
        if off <= tol {
            break;
        }
    }
    best.map_or_else(|| vec![a, b], |(_, poles)| poles)
}

/// The best `n` control points for the samples, with the ends held.
fn least_squares(samples: &[(f64, DVec2)], n: usize, a: DVec2, b: DVec2) -> Option<Vec<DVec2>> {
    const P: usize = 3;
    let spans = n - P;
    let knots: Vec<f64> = (0..n + P + 1).map(|i| (i as f64 - P as f64).clamp(0.0, spans as f64) / spans as f64).collect();
    let free = n - 2;
    let mut m = vec![vec![0.0; free]; free];
    let mut rhs = vec![DVec2::ZERO; free];
    for (u, p) in samples {
        let (span, basis) = nurbs::basis(&knots, P, *u)?;
        let mut left = *p;
        let mut row = Vec::with_capacity(P + 1);
        for (k, v) in basis.iter().enumerate() {
            match span - P + k {
                0 => left -= a * *v,
                i if i == n - 1 => left -= b * *v,
                i => row.push((i - 1, *v)),
            }
        }
        for &(i, v) in &row {
            rhs[i] += left * v;
            for &(j, w) in &row {
                m[i][j] += v * w;
            }
        }
    }
    // The normal equations are symmetric and positive definite: plain elimination is enough.
    for c in 0..free {
        let pivot = *m[c].get(c)?;
        if pivot.abs() < 1e-14 {
            return None;
        }
        let (above, below) = m.split_at_mut(c + 1);
        let top = &above[c];
        for (r, line) in below.iter_mut().enumerate() {
            let k = line[c] / pivot;
            if k != 0.0 {
                line.iter_mut().zip(top).skip(c).for_each(|(x, y)| *x -= k * y);
                let t = rhs[c];
                rhs[c + 1 + r] -= t * k;
            }
        }
    }
    for c in (0..free).rev() {
        let sum: DVec2 = (c + 1..free).map(|j| rhs[j] * m[c][j]).sum();
        rhs[c] = (rhs[c] - sum) / m[c][c];
    }
    let mut poles = vec![a];
    poles.extend(rhs);
    poles.push(b);
    poles.iter().all(|p| p.is_finite()).then_some(poles)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_piece_of_a_control_spline_is_that_spline() {
        let poles: Vec<DVec2> = [(0.0, 0.0), (8.0, -11.0), (17.0, 0.0), (26.0, -13.0), (33.0, -1.0)].iter().map(|&p| p.into()).collect();
        let c = Curve::Control(poles);
        for (t0, t1) in [(0.3, 1.0), (0.0, 0.42), (0.2, 0.9)] {
            let SketchEntity::ControlSpline { poles } = c.piece(t0, t1, c.at(t0), c.at(t1)) else { panic!() };
            assert!(poles.len() <= 40, "{}", poles.len());
            for k in 0..=97 {
                let u = k as f64 / 97.0;
                let off = bspline_at(&poles, u).distance(c.at(t0 + (t1 - t0) * u));
                assert!(off < 5e-4, "{t0}..{t1} at {u}: {off}");
            }
        }
    }

    #[test]
    fn curves_cross_where_they_cross() {
        let e = Curve::Ellipse { center: DVec2::ZERO, rx: 7.0, ry: 3.0, angle: 0.0 };
        let f = Curve::Ellipse { center: DVec2::ZERO, rx: 3.0, ry: 7.0, angle: 0.0 };
        let at = e.meets(&f);
        assert_eq!(at.len(), 4);
        for (t, w) in at {
            assert!(e.at(t).distance(f.at(w)) < 1e-9);
            assert!((e.at(t).x.abs() - e.at(t).y.abs()).abs() < 1e-9);
        }
        let roots = e.roots(|p| p.y - 1.0);
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|t| (e.at(*t).y - 1.0).abs() < 1e-12));
    }
}
