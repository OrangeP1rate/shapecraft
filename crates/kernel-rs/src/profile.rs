//! Planar profiles: the sketch regions, flat faces, polygons and circles that extrudes, revolves, lofts and sweeps
//! start from, as Arris [`Profile`]s, and their loops as pieces ([`Seg`]) for the 2D offset.

use arris::geom::{Curve, Profile, ProfileLoop, ProfileSegment, Surface};
use arris::math::{Frame, Point2, Point3};
use arris::topo::{Model, Orientation};
use glam::DVec2;
use shapecraft_kernel::{DVec3, PlaneFrame, Result, Shape};

use crate::offset2d;
use crate::regions::{self, Seg};
use crate::{AFace, bad, curve_of, exact, faces_of, p3, sign, v3};

pub(crate) fn plane_frame(frame: &PlaneFrame) -> Result<Frame> {
    let z = frame.x.cross(frame.y);
    Frame::from_orthonormal(p3(frame.origin), v3(frame.x.normalize()), v3(frame.y.normalize()), v3(z.normalize()))
        .or_else(|_| Frame::new(p3(frame.origin), v3(z), v3(frame.x)))
        .map_err(|e| bad(format!("sketch plane: {e}")))
}

fn p2(v: DVec2) -> Point2 {
    Point2::new(v.x, v.y)
}

pub(crate) fn cycle_loop(c: &regions::Cycle) -> ProfileLoop {
    if let [regions::Seg::Arc { c, r, sweep, .. }] = c.as_slice()
        && sweep.abs() > std::f64::consts::PI
    {
        return ProfileLoop::Circle { center: p2(*c), radius: *r };
    }
    let start = p2(c[0].at(0.0));
    let segments = c
        .iter()
        .map(|s| match s {
            regions::Seg::Line { b, .. } => ProfileSegment::LineTo(p2(*b)),
            regions::Seg::Arc { .. } => ProfileSegment::ArcTo { to: p2(s.at(1.0)), via: p2(s.at(0.5)) },
        })
        .collect();
    ProfileLoop::Path { start, segments }
}

/// A planar face of a body as a profile (its plane facing out of the body).
pub(crate) fn profile_of(m: &Model, f: AFace) -> Result<Profile> {
    let face = m.face(f.id).map_err(|e| bad(e.to_string()))?;
    let Surface::Plane { frame } = m.surface(face.surface()).map_err(|e| bad(e.to_string()))? else {
        return Err(bad("pick a flat face"));
    };
    let n = frame.z().into_inner() * sign(f.orientation);
    let plane = Frame::new(frame.origin(), n, frame.x().into_inner()).map_err(|e| bad(e.to_string()))?;
    let mut loops: Vec<(f64, ProfileLoop)> = Vec::new();
    for l in face.loops() {
        let cs = l.coedges();
        let local = |p: Point3| {
            let q = plane.to_local(p);
            Point2::new(q.x, q.y)
        };
        if let [c] = cs
            && let Some((Curve::Circle { frame: cf, radius }, _, _)) = curve_of(m, c.edge_use())
        {
            let ctr = local(cf.origin());
            loops.push((std::f64::consts::PI * radius * radius, ProfileLoop::Circle { center: ctr, radius }));
            continue;
        }
        let mut start = None;
        let mut segs = Vec::new();
        let mut poly: Vec<Point2> = Vec::new();
        for c in cs {
            let Some((curve, lo, hi)) = curve_of(m, c.edge_use()) else { continue };
            let (a, b) = if c.orientation() == Orientation::Reversed { (hi, lo) } else { (lo, hi) };
            start.get_or_insert(local(curve.point(a)));
            match curve {
                Curve::Line { .. } => segs.push(ProfileSegment::LineTo(local(curve.point(b)))),
                Curve::Circle { .. } => {
                    segs.push(ProfileSegment::ArcTo { to: local(curve.point(b)), via: local(curve.point((a + b) / 2.0)) })
                }
                _ => {
                    for i in 1..=24 {
                        segs.push(ProfileSegment::LineTo(local(curve.point(a + (b - a) * i as f64 / 24.0))));
                    }
                }
            }
            for i in 0..8 {
                poly.push(local(curve.point(a + (b - a) * i as f64 / 8.0)));
            }
        }
        let Some(start) = start else { continue };
        let area = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(p, q)| p.x * q.y - q.x * p.y).sum::<f64>().abs() / 2.0;
        loops.push((area, ProfileLoop::Path { start, segments: segs }));
    }
    loops.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut it = loops.into_iter().map(|l| l.1);
    let outer = it.next().ok_or_else(|| bad("the face has no boundary"))?;
    Ok(Profile { plane, outer, holes: it.collect() })
}

pub(crate) fn profile_of_shape(s: &Shape) -> Result<Profile> {
    let a = exact(s)?;
    if let Some(p) = &a.profile {
        return Ok(p.clone());
    }
    let (_, f) = faces_of(a).into_iter().next().ok_or_else(|| bad("pick a flat face"))?;
    profile_of(&a.model, f)
}

pub(crate) fn polygon_profile(points: &[DVec3]) -> Result<Profile> {
    if points.len() < 3 {
        return Err(bad("a polygon needs three corners"));
    }
    let mut n = DVec3::ZERO;
    for i in 0..points.len() {
        n += points[i].cross(points[(i + 1) % points.len()]);
    }
    let x = (points[1] - points[0]).normalize_or_zero();
    let plane = Frame::new(p3(points[0]), v3(n), v3(x)).map_err(|e| bad(format!("polygon: {e}")))?;
    let local = |p: DVec3| {
        let q = plane.to_local(p3(p));
        Point2::new(q.x, q.y)
    };
    let mut segments: Vec<ProfileSegment> = points[1..].iter().map(|p| ProfileSegment::LineTo(local(*p))).collect();
    segments.push(ProfileSegment::LineTo(local(points[0])));
    Ok(Profile { plane, outer: ProfileLoop::Path { start: local(points[0]), segments }, holes: Vec::new() })
}

pub(crate) fn circle_profile(center: DVec3, normal: DVec3, radius: f64) -> Result<Profile> {
    let plane = Frame::from_z(p3(center), v3(normal)).map_err(|e| bad(format!("circle: {e}")))?;
    Ok(Profile { plane, outer: ProfileLoop::Circle { center: Point2::origin(), radius }, holes: Vec::new() })
}

/// A loop as pieces in its own walking order; `None` for an ellipse (no piece for it yet).
pub(crate) fn loop_segs(l: &ProfileLoop) -> Option<Vec<Seg>> {
    let v = |p: &Point2| DVec2::new(p.x, p.y);
    match l {
        ProfileLoop::Circle { center, radius } => Some(vec![Seg::Arc { c: v(center), r: *radius, a0: 0.0, sweep: std::f64::consts::TAU }]),
        ProfileLoop::Ellipse { .. } => None,
        ProfileLoop::Path { start, segments } => {
            let mut at = v(start);
            let mut out = Vec::with_capacity(segments.len());
            for s in segments {
                let to = v(&s.end());
                out.push(match s {
                    ProfileSegment::LineTo(_) => Seg::Line { a: at, b: to },
                    ProfileSegment::ArcTo { via, .. } => Seg::through(at, v(via), to),
                    ProfileSegment::EllipseTo { .. } => return None,
                });
                at = to;
            }
            Some(out)
        }
    }
}

/// The profile's region grown by `d` all round (shrunk when negative), corners kept sharp: its outer loop moves out,
/// its holes in.
pub(crate) fn grown(p: &Profile, d: f64) -> Result<Profile> {
    let grow = |l: &ProfileLoop, hole: bool| -> Result<ProfileLoop> {
        let segs = loop_segs(l).ok_or_else(|| bad("an ellipse can't be offset yet"))?;
        Ok(cycle_loop(&offset2d::grow_loop(&segs, d, hole).map_err(|e| bad(e.message()))?))
    };
    Ok(Profile { plane: p.plane, outer: grow(&p.outer, false)?, holes: p.holes.iter().map(|h| grow(h, true)).collect::<Result<_>>()? })
}
