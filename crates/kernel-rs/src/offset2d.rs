//! 2D offsets of chains of lines and arcs with sharp corners: each piece moved sideways, neighbours cut back or run
//! on to where they meet (OCCT's `GeomAbs_Intersection` join). Draft extrudes, Offset Edge and a face's offset
//! outline all come through here.

use std::f64::consts::TAU;

use glam::DVec2;

use crate::regions::Seg;

/// How close two ends must be to count as one point.
const TOL: f64 = 1e-7;

/// Why an offset can't be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The pieces aren't one connected chain.
    NotAChain,
    /// A piece shrinks to nothing or turns inside out (an arc offset past its centre, a side shorter than the
    /// corners eat).
    Collapses,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Refusal::NotAChain => "the curves aren't one connected chain",
            Refusal::Collapses => "that offset would collapse part of the outline",
        }
    }
}

/// The pieces in walking order, each turned to run on from the one before (the first given keeps its direction),
/// and whether the chain closes.
pub fn chain(pieces: &[Seg]) -> Result<(Vec<Seg>, bool), Refusal> {
    let Some(first) = pieces.first() else { return Err(Refusal::NotAChain) };
    if let [Seg::Arc { sweep, .. }] = pieces
        && sweep.abs() >= TAU - 1e-12
    {
        return Ok((vec![*first], true));
    }
    let mut left: Vec<Seg> = pieces.to_vec();
    // Start at a loose end when there is one, so an open chain is walked from one end to the other.
    let touches = |p: DVec2, skip: usize, all: &[Seg]| {
        all.iter().enumerate().any(|(j, q)| j != skip && (q.start().distance(p) < TOL || q.end().distance(p) < TOL))
    };
    let loose = (0..left.len()).find_map(|i| {
        if !touches(left[i].start(), i, &left) {
            Some((i, false))
        } else if !touches(left[i].end(), i, &left) {
            Some((i, true))
        } else {
            None
        }
    });
    let (i, flip) = loose.unwrap_or((0, false));
    let s = left.remove(i);
    let mut out = vec![if flip { s.reversed() } else { s }];
    while !left.is_empty() {
        let at = out.last().ok_or(Refusal::NotAChain)?.end();
        let next = left.iter().position(|q| q.start().distance(at) < TOL).map(|j| (j, false));
        let next = next.or_else(|| left.iter().position(|q| q.end().distance(at) < TOL).map(|j| (j, true)));
        let (j, flip) = next.ok_or(Refusal::NotAChain)?;
        let q = left.remove(j);
        out.push(if flip { q.reversed() } else { q });
    }
    let closed = out.len() > 1 && out[out.len() - 1].end().distance(out[0].start()) < TOL;
    // The first piece given sets the direction, as a wire takes its first edge's.
    if out.iter().any(|q| *q == first.reversed()) && !out.contains(first) {
        out = out.iter().rev().map(Seg::reversed).collect();
    }
    Ok((out, closed))
}

/// Signed area a closed chain encloses (positive counter-clockwise).
pub fn area(chain: &[Seg]) -> f64 {
    chain.iter().map(Seg::area_term).sum()
}

/// One piece moved `d` to its left.
fn shifted(s: &Seg, d: f64) -> Result<Seg, Refusal> {
    Ok(match *s {
        Seg::Line { a, b } => {
            let n = (b - a).normalize_or_zero().perp() * d;
            Seg::Line { a: a + n, b: b + n }
        }
        Seg::Arc { c, r, a0, sweep } => {
            // Left of a counter-clockwise arc is its centre.
            let r = if sweep > 0.0 { r - d } else { r + d };
            if r <= TOL {
                return Err(Refusal::Collapses);
            }
            Seg::Arc { c, r, a0, sweep }
        }
    })
}

/// Where the lines or circles two pieces lie on cross.
fn crossings(p: &Seg, q: &Seg) -> Vec<DVec2> {
    match (*p, *q) {
        (Seg::Line { a, b }, Seg::Line { a: c, b: d }) => {
            let (r, s) = (b - a, d - c);
            let den = r.perp_dot(s);
            if den.abs() < 1e-12 * r.length() * s.length() {
                return Vec::new();
            }
            vec![a + r * ((c - a).perp_dot(s) / den)]
        }
        (Seg::Line { a, b }, Seg::Arc { c, r, .. }) | (Seg::Arc { c, r, .. }, Seg::Line { a, b }) => {
            let d = (b - a).normalize_or_zero();
            let foot = a + d * (c - a).dot(d);
            let h2 = r * r - foot.distance_squared(c);
            if h2 < -TOL * r {
                return Vec::new();
            }
            let h = h2.max(0.0).sqrt();
            vec![foot - d * h, foot + d * h]
        }
        (Seg::Arc { c: c0, r: r0, .. }, Seg::Arc { c: c1, r: r1, .. }) => {
            let dist = c0.distance(c1);
            if dist < 1e-12 || dist > r0 + r1 + TOL || dist < (r0 - r1).abs() - TOL {
                return Vec::new();
            }
            let along = (dist * dist + r0 * r0 - r1 * r1) / (2.0 * dist);
            let h = (r0 * r0 - along * along).max(0.0).sqrt();
            let u = (c1 - c0) / dist;
            let m = c0 + u * along;
            vec![m - u.perp() * h, m + u.perp() * h]
        }
    }
}

/// The angle to turn from `from` to `to` in the arc's direction, nearest `like` (its old sweep).
fn turn(from: f64, to: f64, like: f64) -> f64 {
    let base = (to - from).rem_euclid(TAU);
    [base - TAU, base, base + TAU].into_iter().min_by(|x, y| (x - like).abs().total_cmp(&(y - like).abs())).unwrap_or(base)
}

/// The piece with its end moved to `p` (on its line or circle).
fn with_end(s: &Seg, p: DVec2) -> Seg {
    match *s {
        Seg::Line { a, .. } => Seg::Line { a, b: p },
        Seg::Arc { c, r, a0, sweep } => Seg::Arc { c, r, a0, sweep: turn(a0, (p - c).to_angle(), sweep) },
    }
}

/// The piece with its start moved to `p`.
fn with_start(s: &Seg, p: DVec2) -> Seg {
    match *s {
        Seg::Line { b, .. } => Seg::Line { a: p, b },
        Seg::Arc { c, r, a0, sweep } => {
            let to = a0 + sweep;
            let a1 = (p - c).to_angle();
            Seg::Arc { c, r, a0: a1, sweep: turn(a1, to, sweep) }
        }
    }
}

/// Whether a trimmed piece still runs the way it did (didn't shrink past nothing).
fn same_way(before: &Seg, after: &Seg) -> bool {
    match (*before, *after) {
        (Seg::Line { a, b }, Seg::Line { a: c, b: d }) => (d - c).dot(b - a) > TOL * (b - a).length(),
        (Seg::Arc { sweep: s0, .. }, Seg::Arc { sweep: s1, .. }) => s0 * s1 > 0.0 && s1.abs() > 1e-9,
        _ => false,
    }
}

/// A chain (in walking order) moved `d` to its left, neighbours joined at sharp corners; `closed` joins the last
/// piece to the first too.
pub fn offset(chain: &[Seg], d: f64, closed: bool) -> Result<Vec<Seg>, Refusal> {
    let moved: Vec<Seg> = chain.iter().map(|s| shifted(s, d)).collect::<Result<_, _>>()?;
    let n = moved.len();
    let mut out = moved.clone();
    // Each joint between piece i and the next: the crossing of their lines nearest the old corner.
    let joints = if closed { n } else { n.saturating_sub(1) };
    for i in 0..joints {
        let j = (i + 1) % n;
        if n == 1 {
            break;
        }
        let corner = chain[i].end();
        let (a, b) = (moved[i].end(), moved[j].start());
        let at = if a.distance(b) < TOL {
            a
        } else {
            crossings(&moved[i], &moved[j])
                .into_iter()
                .min_by(|x, y| x.distance(corner).total_cmp(&y.distance(corner)))
                .ok_or(Refusal::Collapses)?
        };
        out[i] = with_end(&out[i], at);
        out[j] = with_start(&out[j], at);
    }
    if out.iter().zip(&moved).any(|(after, before)| !same_way(before, after)) {
        return Err(Refusal::Collapses);
    }
    Ok(out)
}

/// A closed loop grown outwards by `d` (shrunk when negative) as the boundary of the region it bounds: an outer loop
/// runs counter-clockwise and a hole clockwise when `hole` is false and true. Which side is out follows from the
/// loop's own turn, so either walking direction works.
pub fn grow_loop(chain: &[Seg], d: f64, hole: bool) -> Result<Vec<Seg>, Refusal> {
    let ccw = area(chain) > 0.0;
    // Out of the region is right of a counter-clockwise outer loop and left of a counter-clockwise hole.
    let left = if ccw != hole { -d } else { d };
    offset(chain, left, true)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn square(h: f64) -> Vec<Seg> {
        let p = [DVec2::new(0.0, 0.0), DVec2::new(h, 0.0), DVec2::new(h, h), DVec2::new(0.0, h)];
        (0..4).map(|i| Seg::Line { a: p[i], b: p[(i + 1) % 4] }).collect()
    }

    #[test]
    fn a_square_grows_and_shrinks_with_sharp_corners() {
        let grown = grow_loop(&square(10.0), 2.0, false).unwrap();
        assert!((area(&grown) - 196.0).abs() < 1e-9, "{}", area(&grown));
        assert!(grown[0].start().distance(DVec2::new(-2.0, -2.0)) < 1e-9);
        let reversed: Vec<Seg> = square(10.0).iter().rev().map(Seg::reversed).collect();
        assert!((area(&grow_loop(&reversed, -1.0, false).unwrap()) + 64.0).abs() < 1e-9, "either walk shrinks");
        assert_eq!(grow_loop(&square(10.0), -5.0, false), Err(Refusal::Collapses));
    }

    #[test]
    fn an_open_corner_offsets_to_its_left() {
        let l = [
            Seg::Line { a: DVec2::new(0.0, 0.0), b: DVec2::new(10.0, 0.0) },
            Seg::Line { a: DVec2::new(0.0, 10.0), b: DVec2::new(0.0, 0.0) },
        ];
        let (c, closed) = chain(&l).unwrap();
        assert!(!closed);
        let o = offset(&c, 1.0, false).unwrap();
        assert!(o.iter().any(|s| s.start().distance(DVec2::ONE) < 1e-9 || s.end().distance(DVec2::ONE) < 1e-9), "{o:?}");
    }

    #[test]
    fn a_slot_keeps_its_round_ends() {
        // Two lines and two half circles: a 10 × 4 slot, grown by 1 → 12 × 6 with radius 3 ends.
        let (a, b, c, d) = (DVec2::new(0.0, -2.0), DVec2::new(10.0, -2.0), DVec2::new(10.0, 2.0), DVec2::new(0.0, 2.0));
        let slot = vec![
            Seg::Line { a, b },
            Seg::through(b, DVec2::new(12.0, 0.0), c),
            Seg::Line { a: c, b: d },
            Seg::through(d, DVec2::new(-2.0, 0.0), a),
        ];
        let g = grow_loop(&slot, 1.0, false).unwrap();
        let want = 10.0 * 6.0 + std::f64::consts::PI * 9.0;
        assert!((area(&g) - want).abs() < 1e-9, "{} vs {want}", area(&g));
        let hole = grow_loop(&slot.iter().rev().map(Seg::reversed).collect::<Vec<_>>(), 1.0, true).unwrap();
        assert!((area(&hole).abs() - (10.0 * 2.0 + std::f64::consts::PI)).abs() < 1e-9, "a hole shrinks as the region grows");
    }
}
