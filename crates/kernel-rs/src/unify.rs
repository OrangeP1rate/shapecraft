//! Flat faces of a body that lie on one plane and meet along edges, merged into one face: what OCCT's
//! `ShapeUpgrade_UnifySameDomain` does after a boolean, for planes. Unions of prisms (a face tilted by a wedge, regions
//! extruded together, a push/pull) otherwise leave a seam across a flat side, and a body with extra faces numbers
//! them differently from the OCCT kernel.
//!
//! Each merged face takes the surface of one of its pieces; the other pieces' edges get exact curves in that
//! surface's (u, v) (a plane maps lines, circles and ellipses onto themselves). The result must pass Arris's
//! checker, or the body is left as it was.

use std::collections::HashMap;

use arris::geom::{Curve, Curve2, Surface};
use arris::math::{Frame, Frame2, Handedness, Point2, UnitVec2, Vec3};
use arris::topo::builder::{Assembly, Builder, EdgeKey, FaceSpec, UseSpec, effective_uses};
use arris::topo::{Body, EdgeId, FaceId, Model, Orientation};

/// The plane a face lies on, as its outward normal and offset.
fn plane_of(m: &Model, f: arris::topo::Face) -> Option<(Frame, Vec3, f64)> {
    let face = m.face(f.id).ok()?;
    let Ok(Surface::Plane { frame }) = m.surface(face.surface()) else { return None };
    let n = frame.z().into_inner() * if f.orientation == Orientation::Reversed { -1.0 } else { 1.0 };
    Some((*frame, n, n.dot(&frame.origin().coords)))
}

/// The edge's curve in the plane's (u, v), on the same parameter.
fn on_plane(plane: &Frame, curve: &Curve) -> Option<Curve2> {
    let flat = |p: arris::math::Point3| {
        let q = plane.to_local(p);
        Point2::new(q.x, q.y)
    };
    let dir = |v: Vec3| arris::math::Vec2::new(v.dot(&plane.x().into_inner()), v.dot(&plane.y().into_inner()));
    let frame2 = |f: &Frame| {
        let (x, y) = (dir(f.x().into_inner()), dir(f.y().into_inner()));
        let hand = if x.perp(&y) > 0.0 { Handedness::Right } else { Handedness::Left };
        Frame2::new(flat(f.origin()), x, hand).ok()
    };
    Some(match curve {
        Curve::Line { origin, direction } => {
            Curve2::Line { origin: flat(*origin), direction: UnitVec2::try_new(dir(direction.into_inner()), 1e-12)? }
        }
        Curve::Circle { frame, radius } => Curve2::Circle { frame: frame2(frame)?, radius: *radius },
        Curve::Ellipse { frame, major_radius, minor_radius } => {
            Curve2::Ellipse { frame: frame2(frame)?, major_radius: *major_radius, minor_radius: *minor_radius }
        }
        Curve::Nurbs(_) => return None,
    })
}

/// The faces of `b` that touch on one plane, in groups of two or more.
fn groups(m: &Model, faces: &[arris::topo::Face]) -> Vec<Vec<arris::topo::Face>> {
    let planes: Vec<Option<(Frame, Vec3, f64)>> = faces.iter().map(|f| plane_of(m, *f)).collect();
    let mut owners: HashMap<EdgeId, Vec<usize>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        for c in m.face(f.id).map(|x| x.loops().iter().flat_map(|l| l.coedges().to_vec()).collect::<Vec<_>>()).unwrap_or_default() {
            owners.entry(c.edge()).or_default().push(i);
        }
    }
    let mut parent: Vec<usize> = (0..faces.len()).collect();
    fn root(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        p[x] = r;
        r
    }
    for users in owners.values() {
        let [a, b] = users[..] else { continue };
        let (Some((_, na, da)), Some((_, nb, db))) = (&planes[a], &planes[b]) else { continue };
        if a != b && na.dot(nb) > 1.0 - 1e-9 && (da - db).abs() < 1e-7 * (1.0 + da.abs()) {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            parent[ra] = rb;
        }
    }
    let mut by_root: HashMap<usize, Vec<arris::topo::Face>> = HashMap::new();
    for (i, f) in faces.iter().enumerate() {
        let r = root(&mut parent, i);
        by_root.entry(r).or_default().push(*f);
    }
    let mut out: Vec<Vec<arris::topo::Face>> = by_root.into_values().filter(|g| g.len() > 1).collect();
    out.sort_by_key(|g| g[0].id);
    out
}

/// One face over the pieces of `group`: their loops' uses less the edges between them, chained into loops again.
fn merged(m: &mut Model, group: &[arris::topo::Face]) -> Option<FaceSpec> {
    let base = group[0];
    let base_face = m.face(base.id).ok()?.clone();
    let (frame, _, _) = plane_of(m, base)?;
    let mut uses: Vec<(EdgeId, Orientation, arris::topo::Curve2Id, FaceId)> = Vec::new();
    for f in group {
        let face = m.face(f.id).ok()?;
        for l in face.loops() {
            let walk = effective_uses(f.orientation, l.coedges().iter().map(|c| (c.edge(), c.orientation(), c.pcurve())));
            uses.extend(walk.into_iter().map(|(e, o, p)| (e, o, p, f.id)));
        }
    }
    let mut count: HashMap<EdgeId, usize> = HashMap::new();
    for u in &uses {
        *count.entry(u.0).or_default() += 1;
    }
    uses.retain(|u| count.get(&u.0) == Some(&1));
    // Every use on the base face's surface: the others' curves mapped into its (u, v).
    let mut specs = Vec::with_capacity(uses.len());
    for (e, o, p, f) in uses {
        let pcurve = if f == base.id {
            p
        } else {
            let (cid, _) = m.edge(e).ok()?.curve()?;
            let c2 = on_plane(&frame, m.curve(cid).ok()?)?;
            m.add_curve2(c2)
        };
        let edge = m.edge(e).ok()?;
        let (from, to) = if o == Orientation::Forward { (edge.start(), edge.end()) } else { (edge.end(), edge.start()) };
        specs.push((UseSpec { edge: EdgeKey::Kept(e), orientation: o, pcurve }, from, to));
    }
    // Chain the uses end to start into loops.
    let mut loops = Vec::new();
    while let Some(first) = specs.pop() {
        let start = first.1;
        let mut at = first.2;
        let mut lp = vec![first.0];
        while at != start {
            let k = specs.iter().position(|s| s.1 == at)?;
            let next = specs.swap_remove(k);
            at = next.2;
            lp.push(next.0);
        }
        loops.push(lp);
    }
    Some(FaceSpec::New { surface: base_face.surface(), orientation: base.orientation, loops, tolerance: base_face.tolerance() })
}

/// The body with its touching coplanar faces merged, or the body as it was when it has none (or merging fails).
pub fn planar_faces(m: &mut Model, b: Body) -> Body {
    let try_merge = |m: &mut Model| -> Option<Body> {
        let faces = m.faces(b).ok()?;
        let groups = groups(m, &faces);
        if groups.is_empty() {
            return None;
        }
        let mut replaced: HashMap<FaceId, Option<FaceSpec>> = HashMap::new();
        for g in &groups {
            let spec = merged(m, g)?;
            replaced.insert(g[0].id, Some(spec));
            for f in &g[1..] {
                replaced.insert(f.id, None);
            }
        }
        let mut shells = Vec::new();
        for shell in m.shells(b).ok()? {
            let mut specs = Vec::new();
            for f in m.shell(shell.id).ok()?.faces() {
                match replaced.get_mut(&f.id) {
                    Some(spec) => specs.extend(spec.take()),
                    None => specs.push(FaceSpec::Keep(*f)),
                }
            }
            shells.push(specs);
        }
        let assembly = Assembly { shells, ..Assembly::default() };
        let (builder, _) = Builder::assemble(m, m.precision().default_tolerance, assembly).ok()?;
        let body = builder.finish(m, arris::topo::entity::BodyKind::Solid).ok()?.body;
        arris::check::check(m, body, arris::check::Level::Fast).is_ok().then_some(body)
    };
    // A failed attempt may leave pcurves behind; they are dropped when the result is packed.
    try_merge(m).unwrap_or(b)
}
