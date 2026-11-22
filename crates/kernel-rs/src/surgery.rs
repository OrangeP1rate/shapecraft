//! Edits made on the kernel-neutral B-rep and read back through STEP text (`RustKernel::from_brep`), for what Arris
//! has no operation of its own for:
//! - Fill: a flat face across an opening, sewn on (a solid when that closes the body).
//! - Move Edge on a rim: a closed edge round a flat face moved, turned or scaled; the face goes with it and the face
//!   beside it is ruled again from its far edge to the new rim (a cone or cylinder where it stays round about the same
//!   axis, a ruled NURBS otherwise).
//! - Move Edge on a straight edge of flat faces: its ends move, the faces round them are fitted again (flat where
//!   they stay flat, a bilinear patch for a four-sided face that twists).

use std::collections::HashMap;

use arris::topo::entity::BodyKind as ABodyKind;
use arris::topo::{EdgeId, Model};
use shapecraft_kernel::brep::{BodyKind, Brep, Curve, Face, Fin, Nurbs, NurbsSurface, Surface};
use shapecraft_kernel::{DMat3, DVec3, Kernel, Result, Shape};

use crate::{ArShape, RustKernel, bad, edges_of, pack, wrap};

/// The picked edges' body as a neutral B-rep, which body of the shape it is, and the picks as its edge indices.
struct Opened {
    body: usize,
    brep: Brep,
    edges: Vec<usize>,
    arris: HashMap<usize, EdgeId>,
    faces: HashMap<arris::topo::FaceId, usize>,
}

fn open(s: &ArShape, picks: &[u32]) -> Result<Opened> {
    let all = edges_of(s);
    let mut body = None;
    let mut ids = Vec::with_capacity(picks.len());
    for &i in picks {
        let (b, e) = all.get(i as usize).copied().ok_or_else(|| bad(format!("no edge {i}")))?;
        if body.is_some_and(|x| x != b) {
            return Err(bad("the edges must all be on one body"));
        }
        body = Some(b);
        ids.push(e.id);
    }
    let b = body.ok_or_else(|| bad("no edges picked"))?;
    let at = s.bodies.iter().position(|x| *x == b).ok_or_else(|| bad("the edge's body isn't in the shape"))?;
    let solid = s.model.body(b.id).is_ok_and(|x| x.kind() == ABodyKind::Solid);
    let (brep, (map, faces)) = crate::brep_io::body_mapped(&s.model, b, solid)?;
    let edges = ids.iter().map(|e| map.get(e).copied().ok_or_else(|| bad("that edge has no curve"))).collect::<Result<Vec<_>>>()?;
    Ok(Opened { body: at, brep, edges, arris: map.into_iter().map(|(e, i)| (i, e)).collect(), faces })
}

/// The shape with body `at` replaced by `brep` built again.
fn rebuild(s: &ArShape, at: usize, brep: &Brep) -> Result<Shape> {
    let made = RustKernel.from_brep(brep)?;
    let made = crate::exact(&made)?;
    let mut m = Model::default();
    let mut bodies = Vec::with_capacity(s.bodies.len());
    for (k, b) in s.bodies.iter().enumerate() {
        if k == at {
            bodies.extend(crate::bring(&mut m, made)?);
        } else {
            bodies.push(m.import(&s.model, *b).map_err(|e| bad(e.to_string()))?.0);
        }
    }
    Ok(wrap(pack(&m, &bodies)?))
}

/// Points along neutral edge `e` from its start to its end (Arris evaluates its curve).
fn along(m: &Model, o: &Opened, e: usize, n: usize) -> Vec<DVec3> {
    let Some(id) = o.arris.get(&e) else { return Vec::new() };
    let Ok(edge) = m.edge(*id) else { return Vec::new() };
    let Some((cid, range)) = edge.curve() else { return Vec::new() };
    let Ok(c) = m.curve(cid) else { return Vec::new() };
    let mut pts: Vec<DVec3> = (0..=n)
        .map(|k| {
            let p = c.point(range.lo() + (range.hi() - range.lo()) * k as f64 / n as f64);
            DVec3::new(p.x, p.y, p.z)
        })
        .collect();
    if o.brep.edges.get(e).is_some_and(|x| x.reversed) {
        pts.reverse();
    }
    pts
}

/// Newell's normal of a closed polygon (length = twice its area).
fn newell(points: &[DVec3]) -> DVec3 {
    let mut n = DVec3::ZERO;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n
}

/// How many fins use each edge, and the direction of the last one.
fn uses(b: &Brep) -> HashMap<usize, (usize, bool, usize)> {
    let mut out: HashMap<usize, (usize, bool, usize)> = HashMap::new();
    for (fi, f) in b.faces.iter().enumerate() {
        for fin in f.loops.iter().flatten() {
            let u = out.entry(fin.edge).or_insert((0, fin.forward, fi));
            u.0 += 1;
            u.1 = fin.forward;
            u.2 = fi;
        }
    }
    out
}

/// A fin's first and last vertex.
fn ends(b: &Brep, fin: &Fin) -> (Option<usize>, Option<usize>) {
    let e = &b.edges[fin.edge];
    if fin.forward { (e.start, e.end) } else { (e.end, e.start) }
}

/// Fins joined end to start into closed loops.
fn chain(b: &Brep, mut left: Vec<Fin>) -> Result<Vec<Vec<Fin>>> {
    let mut loops = Vec::new();
    while let Some(first) = left.pop() {
        let (start, mut at) = ends(b, &first);
        let mut l = vec![first];
        while at != start {
            let k = left.iter().position(|f| ends(b, f).0 == at).ok_or_else(|| bad("the edges don't go all the way round an opening"))?;
            let f = left.swap_remove(k);
            at = ends(b, &f).1;
            l.push(f);
        }
        loops.push(l);
    }
    Ok(loops)
}

fn size_of(b: &Brep) -> f64 {
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for v in &b.vertices {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    if lo.x <= hi.x { (hi - lo).length() } else { 1.0 }
}

/// Fill: one flat face across the opening the edges go round (each edge must border one face only).
pub fn fill(s: &ArShape, picks: &[u32]) -> Result<Shape> {
    let mut o = open(s, picks)?;
    let used = uses(&o.brep);
    let mut fins = Vec::with_capacity(o.edges.len());
    let mut shell_face = None;
    for &e in &o.edges {
        match used.get(&e) {
            Some(&(1, forward, face)) => {
                fins.push(Fin { edge: e, forward: !forward, pcurve: None });
                shell_face.get_or_insert(face);
            }
            _ => return Err(bad("those edges aren't round an opening (each must border one face only)")),
        }
    }
    let loops = chain(&o.brep, fins)?;
    let mut rings: Vec<(f64, Vec<Fin>, Vec<DVec3>)> = loops
        .into_iter()
        .map(|l| {
            let mut pts = Vec::new();
            for fin in &l {
                let mut p = along(&s.model, &o, fin.edge, 24);
                if !fin.forward {
                    p.reverse();
                }
                p.pop();
                pts.extend(p);
            }
            (newell(&pts).length(), l, pts)
        })
        .collect();
    rings.sort_by(|a, b| b.0.total_cmp(&a.0));
    let Some((area, _, outer)) = rings.first() else { return Err(bad("no opening")) };
    if *area < 1e-12 || outer.len() < 3 {
        return Err(bad("the opening has no area"));
    }
    let normal = newell(outer).normalize();
    let origin = outer.iter().sum::<DVec3>() / outer.len() as f64;
    let tol = 1e-6 * (1.0 + size_of(&o.brep));
    if rings.iter().flat_map(|r| &r.2).any(|p| (*p - origin).dot(normal).abs() > tol) {
        return Err(bad("the Rust kernel fills flat openings only; this one isn't flat (the Open CASCADE kernel fills any)"));
    }
    o.brep.surfaces.push(Surface::Plane { origin, normal, x: normal.any_orthonormal_vector() });
    o.brep.faces.push(Face {
        surface: o.brep.surfaces.len() - 1,
        flip: false,
        loops: rings.into_iter().map(|r| r.1).collect(),
        color: None,
    });
    let new_face = o.brep.faces.len() - 1;
    let shell = shell_face.and_then(|f| o.brep.shells.iter().position(|s| s.contains(&f))).unwrap_or(0);
    match o.brep.shells.get_mut(shell) {
        Some(sh) => sh.push(new_face),
        None => o.brep.shells.push(vec![new_face]),
    }
    let closed = uses(&o.brep).values().all(|u| u.0 == 2);
    o.brep.kind = if closed { BodyKind::Solid } else { BodyKind::Sheet };
    rebuild(s, o.body, &o.brep)
}

/// A full circle as a rational quadratic NURBS (nine poles) starting at `c + r·x`, running counter-clockwise about `n`.
fn circle_nurbs(c: DVec3, n: DVec3, x: DVec3, r: f64) -> Nurbs<DVec3> {
    let y = n.cross(x);
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let dirs = [x, x + y, y, y - x, -x, -x - y, -y, x - y, x];
    Nurbs {
        degree: 2,
        poles: dirs.iter().map(|d| c + *d * r).collect(),
        weights: Some((0..9).map(|i| if i % 2 == 1 { w } else { 1.0 }).collect()),
        knots: vec![0.0, 0.25, 0.5, 0.75, 1.0],
        mults: vec![3, 2, 2, 2, 3],
        periodic: false,
    }
}

/// Moves one edge by x' = linear·x + offset: a closed rim round a flat face, or a straight edge between flat faces.
pub fn map_edge(s: &ArShape, pick: u32, linear: DMat3, offset: DVec3) -> Result<Shape> {
    if !linear.is_finite() || !offset.is_finite() {
        return Err(bad("move edge needs finite numbers"));
    }
    if linear.determinant().is_nan() || linear.determinant() <= 1e-12 {
        return Err(bad("that would fold the edge flat or inside out"));
    }
    let o = open(s, &[pick])?;
    let e = o.edges[0];
    let edge = o.brep.edges[e].clone();
    if edge.start == edge.end { rim(s, o, linear, offset) } else { straight(s, o, linear, offset) }
}

const RIM: &str = "the Rust kernel moves a closed edge on its own when it's a circle round a flat face, with a round face beside it";

fn rim(s: &ArShape, mut o: Opened, lin: DMat3, off: DVec3) -> Result<Shape> {
    let e = o.edges[0];
    let b = &o.brep;
    let around: Vec<usize> = (0..b.faces.len()).filter(|f| b.faces[*f].loops.iter().flatten().any(|x| x.edge == e)).collect();
    let [f1, f2] = around.as_slice() else { return Err(bad(RIM)) };
    let is_cap = |f: usize| {
        matches!(b.surfaces.get(b.faces[f].surface), Some(Surface::Plane { .. }))
            && b.faces[f].loops.len() == 1
            && b.faces[f].loops[0].len() == 1
            && b.faces[f].loops[0][0].edge == e
    };
    let (cap, side) = if is_cap(*f1) {
        (*f1, *f2)
    } else if is_cap(*f2) {
        (*f2, *f1)
    } else {
        return Err(bad(RIM));
    };
    // The side's other edges: the far rim, and seams (edges it holds twice).
    let mut count: HashMap<usize, usize> = HashMap::new();
    for fin in b.faces[side].loops.iter().flatten() {
        *count.entry(fin.edge).or_default() += 1;
    }
    let far: Vec<usize> = count.iter().filter(|(x, n)| **x != e && **n == 1).map(|(x, _)| *x).collect();
    let seams: Vec<usize> = count.iter().filter(|(_, n)| **n == 2).map(|(x, _)| *x).collect();
    let [far] = far.as_slice() else { return Err(bad(RIM)) };
    let far = *far;
    let circle = |i: usize| match b.edges[i].curve.and_then(|c| b.curves.get(c)) {
        Some(Curve::Circle { center, normal, x, radius }) => Some((*center, normal.normalize(), *x, *radius)),
        _ => None,
    };
    let (Some((cr, nr, _, rr)), Some((cf, nf, _, rf))) = (circle(e), circle(far)) else { return Err(bad(RIM)) };
    let (Some(vr), Some(vf)) = (b.edges[e].start, b.edges[far].start) else { return Err(bad(RIM)) };
    let (pr, pf) = (b.vertices[vr], b.vertices[vf]);
    let xr = (pr - cr).normalize_or_zero();
    let xf = (pf - cf).normalize_or_zero();
    if xr == DVec3::ZERO || xf == DVec3::ZERO {
        return Err(bad(RIM));
    }
    let yr = nr.cross(xr);
    // Which way the side faces: away from the axis when the cap faces away from the far rim (a boss), else toward
    // it (a hole's wall).
    let Some(Surface::Plane { normal: cap_n, .. }) = b.surfaces.get(b.faces[cap].surface).cloned() else { return Err(bad(RIM)) };
    let cap_out = if b.faces[cap].flip { -cap_n } else { cap_n };
    let away = cap_out.dot(cr - cf) > 0.0;
    let map = |p: DVec3| lin * p + off;
    // The new rim.
    let (lx, ly) = (lin * xr, lin * yr);
    let c2 = map(cr);
    let n2 = lx.cross(ly).normalize_or_zero();
    let p2 = map(pr);
    let size = 1.0 + size_of(b);
    let round =
        ((lx.length() - ly.length()).abs() < 1e-9 * lx.length().max(ly.length())) && lx.dot(ly).abs() < 1e-9 * lx.length() * ly.length();
    let rim_curve = if round {
        Curve::Circle { center: c2, normal: n2, x: lx.normalize(), radius: rr * lx.length() }
    } else {
        let mut n = circle_nurbs(cr, nr, xr, rr);
        n.poles.iter_mut().for_each(|p| *p = map(*p));
        Curve::BSpline(n)
    };
    // The side, ruled from the far rim to the new one.
    let axis = c2 - cf;
    let coaxial = round && n2.cross(nf).length() < 1e-9 && axis.cross(nf).length() < 1e-9 * size && axis.length() > 1e-9 * size;
    let side_surface = if coaxial {
        let h = axis.length();
        let a = axis / h;
        let r2 = rr * lx.length();
        if (r2 - rf).abs() < 1e-9 * size {
            Surface::Cylinder { origin: cf, axis: a, x: xf, radius: rf }
        } else {
            Surface::Cone { origin: cf, axis: a, x: xf, radius: rf, half_angle: ((r2 - rf) / h).atan() }
        }
    } else {
        let lower = circle_nurbs(cf, nf, xf, rf);
        let mut upper = circle_nurbs(cr, nr, xr, rr);
        upper.poles.iter_mut().for_each(|p| *p = map(*p));
        if n2.dot(nf) < 0.0 {
            upper.poles.reverse();
        }
        let mut poles = Vec::with_capacity(18);
        for (a, b) in lower.poles.iter().zip(&upper.poles) {
            poles.push(*a);
            poles.push(*b);
        }
        Surface::BSpline(NurbsSurface {
            u_degree: 2,
            v_degree: 1,
            nu: 9,
            nv: 2,
            poles,
            weights: lower.weights.as_ref().map(|w| w.iter().flat_map(|x| [*x, *x]).collect()),
            u_knots: lower.knots.clone(),
            u_mults: lower.mults.clone(),
            v_knots: vec![0.0, 1.0],
            v_mults: vec![2, 2],
            u_periodic: false,
            v_periodic: false,
        })
    };
    let side_flip = match side_surface {
        Surface::BSpline(_) => {
            let n = nf.cross(xf).cross(p2 - pf);
            (n.dot(xf) > 0.0) != away
        }
        _ => !away,
    };
    // Write it in: the rim's vertex and curve, the cap's plane, the side's surface, the seams' lines.
    let b = &mut o.brep;
    b.vertices[vr] = p2;
    b.curves.push(rim_curve);
    b.edges[e].curve = Some(b.curves.len() - 1);
    let Some(Surface::Plane { origin, x, .. }) = b.surfaces.get(b.faces[cap].surface).cloned() else { return Err(bad(RIM)) };
    let cap_normal = lin.inverse().transpose() * cap_n;
    let x2 = lin * x;
    b.surfaces.push(Surface::Plane {
        origin: map(origin),
        normal: cap_normal.normalize(),
        x: (x2 - cap_normal.normalize() * x2.dot(cap_normal.normalize())).normalize_or_zero(),
    });
    b.faces[cap].surface = b.surfaces.len() - 1;
    b.surfaces.push(side_surface);
    b.faces[side].surface = b.surfaces.len() - 1;
    b.faces[side].flip = side_flip;
    for seam in seams {
        let (Some(a), Some(z)) = (b.edges[seam].start, b.edges[seam].end) else { return Err(bad(RIM)) };
        let (pa, pz) = (b.vertices[a], b.vertices[z]);
        b.curves.push(Curve::Line { origin: pa, dir: (pz - pa).normalize_or_zero() });
        b.edges[seam].curve = Some(b.curves.len() - 1);
        b.edges[seam].reversed = false;
    }
    for f in &mut b.faces {
        for fin in f.loops.iter_mut().flatten() {
            fin.pcurve = None;
        }
    }
    rebuild(s, o.body, &o.brep)
}

const STRAIGHT: &str = "the Rust kernel moves a straight edge when the faces round its ends are flat with straight edges";

fn straight(s: &ArShape, mut o: Opened, lin: DMat3, off: DVec3) -> Result<Shape> {
    let e = o.edges[0];
    let b = &mut o.brep;
    if !matches!(b.edges[e].curve.and_then(|c| b.curves.get(c)), Some(Curve::Line { .. })) {
        return Err(bad(STRAIGHT));
    }
    let (Some(v1), Some(v2)) = (b.edges[e].start, b.edges[e].end) else { return Err(bad(STRAIGHT)) };
    let moved = [v1, v2];
    let touches = |i: usize, b: &Brep| moved.iter().any(|v| b.edges[i].start == Some(*v) || b.edges[i].end == Some(*v));
    let faces: Vec<usize> = (0..b.faces.len()).filter(|f| b.faces[*f].loops.iter().flatten().any(|x| touches(x.edge, b))).collect();
    // Before: each face's outward normal.
    let mut outward = Vec::with_capacity(faces.len());
    for &f in &faces {
        let Some(Surface::Plane { normal, .. }) = b.surfaces.get(b.faces[f].surface) else { return Err(bad(STRAIGHT)) };
        if b.faces[f]
            .loops
            .iter()
            .flatten()
            .any(|x| !matches!(b.edges[x.edge].curve.and_then(|c| b.curves.get(c)), Some(Curve::Line { .. })))
        {
            return Err(bad(STRAIGHT));
        }
        outward.push(if b.faces[f].flip { -*normal } else { *normal });
    }
    for v in moved {
        b.vertices[v] = lin * b.vertices[v] + off;
    }
    for i in 0..b.edges.len() {
        if touches(i, b) {
            let (Some(a), Some(z)) = (b.edges[i].start, b.edges[i].end) else { return Err(bad(STRAIGHT)) };
            let (pa, pz) = (b.vertices[a], b.vertices[z]);
            if pa.distance(pz) < 1e-9 {
                return Err(bad("that would shrink an edge to nothing"));
            }
            b.curves.push(Curve::Line { origin: pa, dir: (pz - pa).normalize() });
            b.edges[i].curve = Some(b.curves.len() - 1);
            b.edges[i].reversed = false;
        }
    }
    let tol = 1e-7 * (1.0 + size_of(b));
    for (k, &f) in faces.iter().enumerate() {
        let corners: Vec<DVec3> = b.faces[f].loops[0].iter().filter_map(|fin| ends(b, fin).0.map(|v| b.vertices[v])).collect();
        let all: Vec<DVec3> = b.faces[f].loops.iter().flatten().filter_map(|fin| ends(b, fin).0.map(|v| b.vertices[v])).collect();
        let n = newell(&corners);
        if n.length() < 1e-12 {
            return Err(bad("that would fold a face flat"));
        }
        let mut n = n.normalize();
        if n.dot(outward[k]) < 0.0 {
            n = -n;
        }
        let origin = corners.iter().sum::<DVec3>() / corners.len() as f64;
        let flat = all.iter().all(|p| (*p - origin).dot(n).abs() < tol);
        let surface = if flat {
            Surface::Plane { origin, normal: n, x: n.any_orthonormal_vector() }
        } else if b.faces[f].loops.len() == 1 && corners.len() == 4 {
            // A twisted four-sided face: the bilinear patch through its corners.
            let [p0, p1, p2, p3] = [corners[0], corners[1], corners[2], corners[3]];
            Surface::BSpline(NurbsSurface {
                u_degree: 1,
                v_degree: 1,
                nu: 2,
                nv: 2,
                poles: vec![p0, p3, p1, p2],
                weights: None,
                u_knots: vec![0.0, 1.0],
                u_mults: vec![2, 2],
                v_knots: vec![0.0, 1.0],
                v_mults: vec![2, 2],
                u_periodic: false,
                v_periodic: false,
            })
        } else {
            return Err(bad("that would bend a face with more than four sides (the Open CASCADE kernel can)"));
        };
        let flip = match &surface {
            Surface::BSpline(_) => (corners[1] - corners[0]).cross(corners[3] - corners[0]).dot(outward[k]) < 0.0,
            _ => false,
        };
        b.surfaces.push(surface);
        b.faces[f].surface = b.surfaces.len() - 1;
        b.faces[f].flip = flip;
    }
    for f in &mut b.faces {
        for fin in f.loops.iter_mut().flatten() {
            fin.pcurve = None;
        }
    }
    rebuild(s, o.body, &o.brep)
}

const CHAMFER2: &str = "the Rust kernel makes two-distance chamfers on outside edges: straight ones between flat faces, and circles \
                        where a flat face meets a cylinder or cone about the same axis";

/// A face's outward normal when it's flat.
fn plane_out(b: &Brep, f: usize) -> Option<DVec3> {
    match b.surfaces.get(b.faces[f].surface)? {
        Surface::Plane { normal, .. } => Some(normal.normalize() * if b.faces[f].flip { -1.0 } else { 1.0 }),
        _ => None,
    }
}

/// A face's outward normal at `p` on a circle about `center` along `n`: a flat face square to the axis, or a
/// cylinder or cone about it.
fn round_out(b: &Brep, f: usize, center: DVec3, n: DVec3, p: DVec3) -> Option<DVec3> {
    let sign = if b.faces[f].flip { -1.0 } else { 1.0 };
    let rho = (p - center).normalize_or_zero();
    let on_axis = |origin: DVec3, axis: DVec3| axis.normalize().cross(n).length() < 1e-9 && (center - origin).cross(n).length() < 1e-7;
    match b.surfaces.get(b.faces[f].surface)? {
        Surface::Plane { normal, .. } if normal.normalize().cross(n).length() < 1e-9 => Some(normal.normalize() * sign),
        Surface::Cylinder { origin, axis, .. } if on_axis(*origin, *axis) => Some(rho * sign),
        Surface::Cone { origin, axis, half_angle, .. } if on_axis(*origin, *axis) => {
            Some((rho * half_angle.cos() - axis.normalize() * half_angle.sin()) * sign)
        }
        _ => None,
    }
}

/// The cross-section of the cut at `p`: the corner between `qa` (da into face a) and `qb` (db into face b), with a
/// margin outside the body so no face of the cut lies on a face of the body.
fn chamfer_section(p: DVec3, wa: DVec3, wb: DVec3, na: DVec3, nb: DVec3, da: f64, db: f64) -> Vec<DVec3> {
    let (qa, qb) = (p + wa * da, p + wb * db);
    let u = (qa - qb).normalize();
    let e = 0.25 * da.min(db);
    let (qa2, qb2) = (qa + u * e, qb - u * e);
    vec![qa2, qa2 + na * e, p + (na + nb) * e, qb2 + nb * e, qb2]
}

/// Whether `q` is inside the body (the first face a ray from it meets is one it leaves through).
fn inside(m: &Model, body: arris::topo::Body, q: DVec3) -> bool {
    use arris::ops::query::{Ray, RayHits, ray_fire};
    let dir = DVec3::new(0.3141, 0.5926, 0.7419).normalize();
    let ray = Ray { origin: crate::p3(q), direction: crate::v3(dir) };
    ray_fire(m, body, &ray, RayHits::First)
        .is_ok_and(|h| h.first().is_some_and(|h| !h.tangent && DVec3::new(h.normal.x, h.normal.y, h.normal.z).dot(dir) > 0.0))
}

/// A chamfer `d1` along one face and `d2` along the other: `on_face` (a face numbered as the shape's) for the edges
/// beside it, else each edge's first face. Cut as a boolean: the corner swept along a straight edge or round a circle.
pub fn chamfer_two(shape: &Shape, s: &ArShape, picks: &[u32], d1: f64, d2: f64, on_face: Option<u32>) -> Result<Shape> {
    use shapecraft_kernel::{Axis, BoolOp};
    if !(d1.is_finite() && d2.is_finite() && d1 > 0.0 && d2 > 0.0) || picks.is_empty() {
        return Err(bad("chamfer needs edges and positive distances"));
    }
    let o = open(s, picks)?;
    let body = s.bodies[o.body];
    let kernel_faces = crate::faces_of(s);
    let kernel_of: HashMap<usize, usize> =
        kernel_faces.iter().enumerate().filter_map(|(i, (_, f))| o.faces.get(&f.id).map(|n| (*n, i))).collect();
    let on = match on_face {
        Some(f) => Some(kernel_faces.get(f as usize).and_then(|(_, af)| o.faces.get(&af.id).copied()).ok_or_else(|| bad("no such face"))?),
        None => None,
    };
    let b = &o.brep;
    let mut tools = Vec::with_capacity(o.edges.len());
    for &e in &o.edges {
        let sides: Vec<(usize, bool)> = (0..b.faces.len())
            .flat_map(|fi| b.faces[fi].loops.iter().flatten().filter(|x| x.edge == e).map(move |x| (fi, x.forward)))
            .collect();
        let [first, second] = sides.as_slice() else { return Err(bad(CHAMFER2)) };
        // A seam (a round face meeting itself) has nothing to cut.
        if first.0 == second.0 {
            continue;
        }
        let a_first = match on {
            Some(f) if f == second.0 => false,
            Some(f) if f == first.0 => true,
            _ => kernel_of.get(&first.0).unwrap_or(&usize::MAX) <= kernel_of.get(&second.0).unwrap_or(&usize::MAX),
        };
        let ((fa, fwa), (fb, fwb)) = if a_first { (*first, *second) } else { (*second, *first) };
        let edge = &b.edges[e];
        let curve = edge.curve.and_then(|c| b.curves.get(c)).ok_or_else(|| bad(CHAMFER2))?;
        let along = |t: DVec3, forward: bool| if forward { t } else { -t };
        match curve {
            Curve::Line { .. } => {
                let (Some(v0), Some(v1)) = (edge.start, edge.end) else { return Err(bad(CHAMFER2)) };
                let (p0, p1) = (b.vertices[v0], b.vertices[v1]);
                let t = (p1 - p0).normalize_or_zero();
                let (Some(na), Some(nb)) = (plane_out(b, fa), plane_out(b, fb)) else { return Err(bad(CHAMFER2)) };
                let (wa, wb) = (na.cross(along(t, fwa)), nb.cross(along(t, fwb)));
                if t == DVec3::ZERO || wa.dot(nb) > -1e-9 {
                    return Err(bad(CHAMFER2));
                }
                let step = 0.1 * d1.min(d2);
                for (v, out) in [(p0, -t), (p1, t)] {
                    if inside(&s.model, body, v + out * step + (wa + wb) * step) {
                        return Err(bad("the Rust kernel's two-distance chamfers stop at outside corners; this edge runs into a wall"));
                    }
                }
                let l = 3.0 * (d1 + d2);
                let section: Vec<DVec3> = chamfer_section(p0, wa, wb, na, nb, d1, d2).into_iter().map(|q| q - t * l).collect();
                tools.push(RustKernel.extrude_polygon(&section, t * ((p1 - p0).length() + 2.0 * l))?);
            }
            Curve::Circle { center, normal, .. } => {
                let p = edge.start.map(|v| b.vertices[v]).ok_or_else(|| bad(CHAMFER2))?;
                let n = normal.normalize();
                let rho = (p - *center).normalize_or_zero();
                let tangent = n.cross(rho) * if edge.reversed { -1.0 } else { 1.0 };
                let (Some(na), Some(nb)) = (round_out(b, fa, *center, n, p), round_out(b, fb, *center, n, p)) else {
                    return Err(bad(CHAMFER2));
                };
                let (wa, wb) = (na.cross(along(tangent, fwa)), nb.cross(along(tangent, fwb)));
                if rho == DVec3::ZERO || wa.dot(nb) > -1e-9 {
                    return Err(bad(CHAMFER2));
                }
                let section = chamfer_section(p, wa, wb, na, nb, d1, d2);
                if section.iter().any(|q| (*q - *center).dot(rho) <= 1e-9) {
                    return Err(bad("the chamfer is wider than the circle"));
                }
                tools.push(RustKernel.revolve_polygon(&section, Axis::new(*center, n), std::f64::consts::TAU)?);
            }
            _ => return Err(bad(CHAMFER2)),
        }
    }
    if tools.is_empty() {
        return Err(bad("no edges to chamfer"));
    }
    let mut out = shape.clone();
    for t in &tools {
        out = RustKernel.boolean(BoolOp::Subtract, &out, t)?;
    }
    Ok(out)
}

const IMPRINT: &str =
    "the Rust kernel projects closed outlines that land inside a flat face (ones crossing its edge need the Open CASCADE kernel)";

/// One piece of a closed outline: its curve, whether the walk runs against the curve's own direction, its ends,
/// and points along it in walking order.
struct Piece {
    curve: Curve,
    against: bool,
    from: DVec3,
    to: DVec3,
    pts: Vec<DVec3>,
}

/// The closed outlines of a tool: every loop of its face(s), or the one closed edge named.
fn outlines(tool: &Shape, edge: Option<u32>) -> Result<Vec<Vec<Piece>>> {
    let t = crate::exact(tool)?;
    let m = &t.model;
    let piece = |eid: EdgeId, reversed: bool| -> Result<Piece> {
        let e = m.edge(eid).map_err(|x| bad(x.to_string()))?;
        let (cid, range) = e.curve().ok_or_else(|| bad(IMPRINT))?;
        let c = m.curve(cid).map_err(|x| bad(x.to_string()))?;
        let mut pts: Vec<DVec3> = (0..=32)
            .map(|k| {
                let p = c.point(range.lo() + (range.hi() - range.lo()) * k as f64 / 32.0);
                DVec3::new(p.x, p.y, p.z)
            })
            .collect();
        let start = m.vertex(e.start()).map(|v| v.point()).map_err(|x| bad(x.to_string()))?;
        let start = DVec3::new(start.x, start.y, start.z);
        // The edge may run against its curve; then the walk along the edge does too.
        let edge_against = pts.first().is_some_and(|p| p.distance(start) > pts.last().map_or(0.0, |q| q.distance(start)) + 1e-9);
        let against = edge_against != reversed;
        if against {
            pts.reverse();
        }
        let (Some(&from), Some(&to)) = (pts.first(), pts.last()) else { return Err(bad(IMPRINT)) };
        Ok(Piece { curve: crate::brep_io::curve(c), against, from, to, pts })
    };
    if let Some(i) = edge {
        let (_, e) = crate::edge_at(t, i)?;
        let p = piece(e.id, false)?;
        if p.from.distance(p.to) > 1e-7 {
            return Err(bad(IMPRINT));
        }
        return Ok(vec![vec![p]]);
    }
    let mut out = Vec::new();
    for (_, f) in crate::faces_of(t) {
        let face = m.face(f.id).map_err(|x| bad(x.to_string()))?;
        for l in face.loops() {
            let mut ps = Vec::new();
            for c in l.coedges() {
                ps.push(piece(c.edge(), c.orientation() == arris::topo::Orientation::Reversed)?);
            }
            if !ps.is_empty() {
                out.push(ps);
            }
        }
    }
    Ok(out)
}

/// A curve dropped onto the plane through `o` square to `n`.
fn flatten(c: &Curve, o: DVec3, n: DVec3) -> Option<Curve> {
    let p = |q: DVec3| q - n * (q - o).dot(n);
    let d = |v: DVec3| v - n * v.dot(n);
    Some(match c {
        Curve::Line { origin, dir } => {
            let dir = d(*dir);
            if dir.length() < 1e-12 {
                return None;
            }
            Curve::Line { origin: p(*origin), dir }
        }
        Curve::Circle { center, normal, x, radius } if normal.normalize().cross(n).length() < 1e-9 => {
            Curve::Circle { center: p(*center), normal: *normal, x: *x, radius: *radius }
        }
        Curve::Circle { center, normal, x, radius } => {
            let mut b = circle_nurbs(*center, normal.normalize(), x.normalize(), *radius);
            b.poles.iter_mut().for_each(|q| *q = p(*q));
            Curve::BSpline(b)
        }
        Curve::Ellipse { center, normal, x, major, minor } if normal.normalize().cross(n).length() < 1e-9 => {
            Curve::Ellipse { center: p(*center), normal: *normal, x: *x, major: *major, minor: *minor }
        }
        Curve::BSpline(b) => {
            let mut b = b.clone();
            b.poles.iter_mut().for_each(|q| *q = p(*q));
            Curve::BSpline(b)
        }
        _ => return None,
    })
}

/// A face in the plane's 2D: its number, its outline and its holes.
type Region = (usize, Vec<(f64, f64)>, Vec<Vec<(f64, f64)>>);

fn inside_2d(q: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > q.1) != (b.1 > q.1) && q.0 < (b.0 - a.0) * (q.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Project as Edge: the tools' closed outlines dropped onto flat face `face` along its normal and imprinted there;
/// each splits off the piece of face inside it.
pub fn imprint(s: &ArShape, face: u32, tools: &[(Shape, Option<u32>)]) -> Result<Shape> {
    let (_, af) = crate::face_at(s, face)?;
    let all = edges_of(s);
    // Open the face's body (through one of its edges).
    let face_edge = {
        let f = s.model.face(af.id).map_err(|x| bad(x.to_string()))?;
        let first = f.loops().iter().flat_map(|l| l.coedges()).next().map(|c| c.edge()).ok_or_else(|| bad(IMPRINT))?;
        all.iter().position(|(_, e)| e.id == first).ok_or_else(|| bad(IMPRINT))? as u32
    };
    let mut o = open(s, &[face_edge])?;
    let target = *o.faces.get(&af.id).ok_or_else(|| bad(IMPRINT))?;
    let Some(Surface::Plane { origin, normal, .. }) = o.brep.surfaces.get(o.brep.faces[target].surface).cloned() else {
        return Err(bad(IMPRINT));
    };
    let n = normal.normalize();
    let out_n = if o.brep.faces[target].flip { -n } else { n };
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    let flat = |p: DVec3| ((p - origin).dot(u), (p - origin).dot(v));
    let ring = |pts: &[DVec3]| -> Vec<(f64, f64)> { pts.iter().map(|p| flat(*p)).collect() };
    // Each face's outline and holes in 2D, for finding which one an outline lands in.
    let loop_pts = |o: &Opened, l: &[Fin]| -> Vec<DVec3> {
        let mut pts = Vec::new();
        for fin in l {
            let mut p = along(&s.model, o, fin.edge, 24);
            if !fin.forward {
                p.reverse();
            }
            p.pop();
            pts.extend(p);
        }
        pts
    };
    let mut rings: Vec<Vec<(f64, f64)>> = o.brep.faces[target].loops.iter().map(|l| ring(&loop_pts(&o, l))).collect();
    rings.sort_by(|a, b| area2(b).abs().total_cmp(&area2(a).abs()));
    let mut regions: Vec<Region> = vec![(target, rings[0].clone(), rings[1..].to_vec())];
    let original = regions[0].clone();
    let within = |r: &Region, q: (f64, f64)| inside_2d(q, &r.1) && !r.2.iter().any(|h| inside_2d(q, h));
    // The outlines, flattened, largest first.
    let mut flat_outlines = Vec::new();
    for (tool, edge) in tools {
        for pieces in outlines(tool, *edge)? {
            let mut out = Vec::with_capacity(pieces.len());
            for pc in pieces {
                let curve = flatten(&pc.curve, origin, n).ok_or_else(|| bad(IMPRINT))?;
                let drop = |q: DVec3| q - n * (q - origin).dot(n);
                out.push(Piece {
                    curve,
                    against: pc.against,
                    from: drop(pc.from),
                    to: drop(pc.to),
                    pts: pc.pts.into_iter().map(drop).collect(),
                });
            }
            let pts: Vec<DVec3> = out.iter().flat_map(|p| p.pts[..p.pts.len().saturating_sub(1)].to_vec()).collect();
            flat_outlines.push((newell(&pts).dot(out_n), out, pts));
        }
    }
    flat_outlines.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
    let tol = 1e-7 * (1.0 + size_of(&o.brep));
    let mut landed = 0;
    for (signed, pieces, pts) in flat_outlines {
        if signed.abs() < 1e-12 {
            continue;
        }
        let poly = ring(&pts);
        let hits = poly.iter().filter(|q| within(&original, **q)).count();
        if hits == 0 {
            continue;
        }
        if hits < poly.len() {
            return Err(bad(IMPRINT));
        }
        // The innermost face holding it.
        let k = (0..regions.len()).rev().find(|k| poly.iter().all(|q| within(&regions[*k], *q))).ok_or_else(|| bad(IMPRINT))?;
        // Its edges.
        let b = &mut o.brep;
        let vertex = |p: DVec3, b: &mut Brep| -> usize {
            if let Some(i) = b.vertices.iter().rposition(|q| q.distance(p) < tol) {
                return i;
            }
            b.vertices.push(p);
            b.vertices.len() - 1
        };
        let mut fins = Vec::with_capacity(pieces.len());
        for pc in pieces {
            let (a, z) = (vertex(pc.from, b), vertex(pc.to, b));
            b.curves.push(pc.curve);
            b.edges.push(shapecraft_kernel::brep::Edge {
                curve: Some(b.curves.len() - 1),
                reversed: pc.against,
                start: Some(a),
                end: Some(z),
                tolerance: None,
            });
            fins.push(Fin { edge: b.edges.len() - 1, forward: true, pcurve: None });
        }
        let back: Vec<Fin> = fins.iter().rev().map(|f| Fin { forward: false, ..*f }).collect();
        let (outer, hole) = if signed > 0.0 { (fins, back) } else { (back, fins) };
        let parent = regions[k].0;
        b.faces[parent].loops.push(hole);
        let made = Face { surface: b.faces[target].surface, flip: b.faces[target].flip, loops: vec![outer], color: b.faces[target].color };
        b.faces.push(made);
        let new_face = b.faces.len() - 1;
        if let Some(sh) = b.shells.iter_mut().find(|sh| sh.contains(&target)) {
            sh.push(new_face);
        }
        regions[k].2.push(poly.clone());
        regions.push((new_face, poly, Vec::new()));
        landed += 1;
    }
    if landed == 0 {
        return Err(bad("nothing lands on that face"));
    }
    rebuild(s, o.body, &o.brep)
}

fn area2(p: &[(f64, f64)]) -> f64 {
    (0..p.len()).map(|i| p[i].0 * p[(i + 1) % p.len()].1 - p[(i + 1) % p.len()].0 * p[i].1).sum()
}
