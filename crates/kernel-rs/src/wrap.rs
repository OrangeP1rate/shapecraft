//! Wrap & Emboss on the Rust kernel: flat sketch regions laid onto a cylindrical or conical face with their lengths
//! kept (a cylinder unrolls to a strip, a cone to a sector), raised or engraved by a depth.
//!
//! Each outline edge becomes a wall: a ruled surface between curves fitted through the mapped points on the face and
//! `depth` off it, at shared parameters; a cap (or a pocket's floor) closes it on the same axis, so it is a cylinder
//! or cone again.
//! - Profiles that lie inside one face are built straight into the body's neutral B-rep (holes in the face, walls,
//!   cap) and read back through STEP text: no intersections to compute, so cones work as well as cylinders.
//! - Others become solids of their own, fused on or cut out (the skin starts a little inside the face, outside it
//!   for an engraving, so the boolean meets a real overlap rather than two surfaces lying on each other).

use std::f64::consts::{PI, TAU};

use arris::topo::entity::BodyKind as ABodyKind;
use shapecraft_kernel::brep::{BodyKind, Brep, Curve, Edge, Face, Fin, Nurbs, NurbsSurface, Surface};
use shapecraft_kernel::spline::{self, Fit};
use shapecraft_kernel::{BoolOp, DVec3, Kernel, PlaneFrame, Result, Shape};

use crate::{ArShape, RustKernel, bad, exact};

/// A round face measured from its axis: a cylinder (`loc` on the axis) or a cone (`loc` its apex, growing along `z`).
#[derive(Clone, Copy, Debug)]
struct Round {
    cone: bool,
    loc: DVec3,
    z: DVec3,
    radius: f64,
    angle: f64,
}

impl Round {
    fn of(s: &Surface) -> Option<Round> {
        match *s {
            Surface::Cylinder { origin, axis, radius, .. } => {
                Some(Round { cone: false, loc: origin, z: axis.try_normalize()?, radius, angle: 0.0 })
            }
            Surface::Cone { origin, axis, radius, half_angle, .. } => {
                let a = axis.try_normalize()?;
                if half_angle.abs() < 1e-9 {
                    return Some(Round { cone: false, loc: origin, z: a, radius, angle: 0.0 });
                }
                let apex = origin - a * (radius / half_angle.tan());
                Some(Round { cone: true, loc: apex, z: a * half_angle.signum(), radius: 0.0, angle: half_angle.abs() })
            }
            _ => None,
        }
    }

    /// How far `p` is from the surface (infinite), and its height along the axis.
    fn gap(&self, p: DVec3) -> (f64, f64) {
        let q = p - self.loc;
        let a = q.dot(self.z);
        let r = (q - self.z * a).length();
        if self.cone {
            let (s, c) = self.angle.sin_cos();
            ((r * c - a * s).abs(), a)
        } else {
            ((r - self.radius).abs(), a)
        }
    }
}

/// Where a profile's points go: (u round the axis, v along it or, on a cone, slant distance from the apex).
struct Wrapper {
    round: Round,
    d0: DVec3,
    t: DVec3,
    center: DVec3,
    along: DVec3,
    across: DVec3,
    rot: (f64, f64),
    u0: f64,
    /// Where the skins' seams go (u): opposite the middle of the profiles, in the widest gap they leave.
    seam: f64,
    v0: f64,
}

impl Wrapper {
    /// The profile's own 2D (along, across), turned by the rotation.
    fn st(&self, p: DVec3) -> (f64, f64) {
        let d = p - self.center;
        let (s0, t0) = (d.dot(self.along), d.dot(self.across));
        let (c, sn) = self.rot;
        (s0 * c - t0 * sn, s0 * sn + t0 * c)
    }

    fn uv(&self, (s, t): (f64, f64)) -> (f64, f64) {
        if !self.round.cone {
            return (self.u0 + s / self.round.radius, self.v0 + t);
        }
        // A cone unrolls into a sector: the centre sits at slant distance v0 from the apex, angle 0.
        let (x, y) = (self.v0 + t, s);
        (self.u0 + y.atan2(x) / self.round.angle.sin(), (x * x + y * y).sqrt())
    }

    /// The point at (u, v) moved `h` along the face's outward normal.
    fn at(&self, (u, v): (f64, f64), h: f64) -> DVec3 {
        let r = &self.round;
        let radial = self.d0 * u.cos() + self.t * u.sin();
        if !r.cone {
            return r.loc + radial * (r.radius + h) + r.z * v;
        }
        let (s, c) = r.angle.sin_cos();
        r.loc + radial * (v * s) + r.z * (v * c) + (radial * c - r.z * s) * h
    }

    /// The skin `h` off the face as a surface, its seam opposite the profiles.
    fn skin(&self, h: f64) -> Surface {
        let r = &self.round;
        let x = self.d0 * self.seam.cos() + self.t * self.seam.sin();
        if !r.cone {
            return Surface::Cylinder { origin: r.loc, axis: r.z, x, radius: r.radius + h };
        }
        // The offset of a cone is the same cone with its apex moved along the axis.
        let (s, c) = r.angle.sin_cos();
        let apex = r.loc - r.z * (h / s);
        let height = (self.v0 * c).max(1e-3);
        Surface::Cone { origin: apex + r.z * height, axis: r.z, x, radius: height * r.angle.tan(), half_angle: r.angle }
    }
}

const NEEDS_ROUND: &str = "Wrap & Emboss needs a cylindrical or conical face";

/// A round face of the shape: its number, which body it's on, its index in that body's neutral B-rep, whether
/// it faces the axis, its round and points along its edges.
struct Candidate {
    index: u32,
    body: usize,
    face: usize,
    inward: bool,
    round: Round,
    rim: Vec<DVec3>,
}

/// The round faces of the shape's bodies, and each body's neutral B-rep.
fn round_faces(s: &ArShape) -> Result<(Vec<Candidate>, Vec<Brep>)> {
    let index: std::collections::HashMap<arris::topo::FaceId, u32> =
        crate::faces_of(s).iter().enumerate().map(|(i, (_, f))| (f.id, i as u32)).collect();
    let mut out = Vec::new();
    let mut breps = Vec::new();
    for (at, b) in s.bodies.iter().enumerate() {
        let solid = s.model.body(b.id).is_ok_and(|x| x.kind() == ABodyKind::Solid);
        let (brep, (_, faces)) = crate::brep_io::body_mapped(&s.model, *b, solid)?;
        for (fid, k) in faces {
            let (Some(i), Some(face)) = (index.get(&fid), brep.faces.get(k)) else { continue };
            let Some(round) = brep.surfaces.get(face.surface).and_then(Round::of) else { continue };
            let mut rim = Vec::new();
            if let Ok(f) = s.model.face(fid) {
                for c in f.loops().iter().flat_map(|l| l.coedges()) {
                    // Straight edges come back as their ends: fill them in, so a seam through a profile is seen.
                    let pts = crate::edge_points(&s.model, c.edge_use(), Some(48));
                    if let [a, b] = pts[..] {
                        rim.extend((0..=48).map(|k| a.lerp(b, k as f64 / 48.0)));
                    } else {
                        rim.extend(pts);
                    }
                }
            }
            out.push(Candidate { index: *i, body: at, face: k, inward: face.flip, round, rim });
        }
        breps.push(brep);
    }
    out.sort_by_key(|x| x.index);
    Ok((out, breps))
}

/// One closed loop of a profile in (s, t): its pieces' points in walking order, each piece ending where the next
/// starts.
type Loop2 = Vec<Vec<(f64, f64)>>;

fn area(l: &Loop2) -> f64 {
    let pts: Vec<(f64, f64)> = l.iter().flat_map(|p| p[..p.len() - 1].iter().copied()).collect();
    (0..pts.len()).map(|i| pts[i].0 * pts[(i + 1) % pts.len()].1 - pts[(i + 1) % pts.len()].0 * pts[i].1).sum::<f64>() / 2.0
}

fn reversed(l: Loop2) -> Loop2 {
    l.into_iter()
        .rev()
        .map(|mut p| {
            p.reverse();
            p
        })
        .collect()
}

/// A profile face's loops in (s, t), the outline first and counter-clockwise, the holes clockwise.
fn loops_2d(w: &Wrapper, loops: &[Vec<crate::surgery::Piece>]) -> Result<Vec<Loop2>> {
    let mut out: Vec<Loop2> = Vec::new();
    for l in loops {
        let mut pieces: Loop2 = Vec::new();
        for p in l {
            let pts: Vec<(f64, f64)> = p.pts.iter().map(|q| w.st(*q)).collect();
            if pts.len() < 2 {
                continue;
            }
            // A closed edge (a circle) goes in two halves, so every wall has two ends.
            if p.from.distance(p.to) < 1e-7 && l.len() == 1 {
                let mid = pts.len() / 2;
                pieces.push(pts[..=mid].to_vec());
                pieces.push(pts[mid..].to_vec());
            } else {
                pieces.push(pts);
            }
        }
        // Each piece ends exactly where the next begins.
        for i in 0..pieces.len() {
            let next = pieces[(i + 1) % pieces.len()][0];
            if let Some(last) = pieces[i].last_mut() {
                *last = next;
            }
        }
        if !pieces.is_empty() {
            out.push(pieces);
        }
    }
    let Some(outer) = (0..out.len()).max_by(|a, b| area(&out[*a]).abs().total_cmp(&area(&out[*b]).abs())) else {
        return Err(bad("a profile with no outline"));
    };
    out.swap(0, outer);
    Ok(out.into_iter().enumerate().map(|(k, l)| if (area(&l) > 0.0) == (k == 0) { l } else { reversed(l) }).collect())
}

/// Points sampled along each profile edge; the walls are fitted through as few of them as keep the curves on the
/// skins (Arris's intersections and integrals get slow, and can fail, on walls fitted through more than they need).
const DENSE: usize = 128;

/// The curves of one wall under and over the face, fitted at shared parameters through the fewest of the piece's
/// points (every 32nd, 16th, …) that keep every point within a hair of the fit.
fn fit_pair(w: &Wrapper, piece: &[(f64, f64)], lo: f64, hi: f64) -> Result<(Fit, Fit)> {
    let uvs: Vec<(f64, f64)> = piece.iter().map(|q| w.uv(*q)).collect();
    let under: Vec<DVec3> = uvs.iter().map(|x| w.at(*x, lo)).collect();
    let over: Vec<DVec3> = uvs.iter().map(|x| w.at(*x, hi)).collect();
    let size = under.iter().chain(&over).fold(0.0_f64, |a, p| a.max(p.abs().max_element()));
    // A hundredth of a micrometre per millimetre of size: well inside what the STEP reader joins, and each halving of
    // the points makes the walls quicker for Arris to integrate (volumes, areas).
    let tol = 1e-5 * (1.0 + size);
    let last = under.len() - 1;
    for stride in [32, 16, 8, 4, 2, 1] {
        if stride > last {
            continue;
        }
        let mut pick: Vec<usize> = (0..last).step_by(stride).collect();
        pick.push(last);
        let fit = |pts: &[DVec3], params: Option<Vec<f64>>| {
            let chosen: Vec<DVec3> = pick.iter().map(|i| pts[*i]).collect();
            match params {
                None => spline::fit(&chosen),
                Some(p) => spline::fit_with(&chosen, p),
            }
        };
        let Some(a) = fit(&under, None) else { continue };
        let Some(b) = fit(&over, Some(a.params.clone())) else { continue };
        // Every dense point against the fit, at its parameter between the chosen ones it lies between.
        let off = |f: &Fit, pts: &[DVec3]| {
            (0..pick.len() - 1)
                .flat_map(|k| {
                    let (i, j) = (pick[k], pick[k + 1]);
                    let (pa, pb) = (f.params[k], f.params[k + 1]);
                    (i..=j).map(move |x| (x, pa + (pb - pa) * (x - i) as f64 / (j - i) as f64))
                })
                .map(|(x, u)| nearest(f, u, pts[x]))
                .fold(0.0_f64, f64::max)
        };
        if stride == 1 || off(&a, &under).max(off(&b, &over)) < tol {
            return Ok((a, b));
        }
    }
    Err(bad("couldn't wrap that profile"))
}

/// The distance from `p` to the fit near parameter `u` (a few Newton steps along the curve).
fn nearest(f: &Fit, mut u: f64, p: DVec3) -> f64 {
    for _ in 0..4 {
        let d = f.derivative(u);
        let len2 = d.length_squared();
        if len2 < 1e-30 {
            break;
        }
        u = (u + (p - f.point(u)).dot(d) / len2).clamp(0.0, 1.0);
    }
    f.point(u).distance(p)
}

/// Profiles picked together where one fills another's hole (a ring and the disc inside it) become one: the hole
/// and the outline that fills it go, and the filling profile's own holes join the other's.
fn fill_holes(profiles: &mut Vec<Vec<Loop2>>) {
    let points = |l: &Loop2| -> Vec<(f64, f64)> { l.iter().flat_map(|p| p[..p.len() - 1].iter().copied()).collect() };
    let same = |a: &Loop2, b: &Loop2| {
        let (pa, pb) = (points(a), points(b));
        let size = pa.iter().fold(1.0_f64, |m, p| m.max(p.0.abs()).max(p.1.abs()));
        pa.len() == pb.len() && pa.iter().all(|p| pb.iter().any(|q| (p.0 - q.0).hypot(p.1 - q.1) < 1e-7 * size))
    };
    'again: loop {
        for i in 0..profiles.len() {
            for k in 1..profiles[i].len() {
                for j in 0..profiles.len() {
                    if j != i && same(&profiles[i][k], &profiles[j][0]) {
                        let filler = profiles.remove(j);
                        let i = if j < i { i - 1 } else { i };
                        profiles[i].remove(k);
                        profiles[i].extend(filler.into_iter().skip(1));
                        continue 'again;
                    }
                }
            }
        }
        break;
    }
}

/// One profile's faces between `lo` and `hi` off the face, its surfaces, curves and edges added to `b`.
struct Parts {
    /// At `hi`, normal off the face; loops as the profile's.
    top: Face,
    /// At `lo`, normal into the face.
    bottom: Face,
    /// Normals out of the profile.
    sides: Vec<Face>,
}

fn walls(b: &mut Brep, w: &Wrapper, loops: &[Loop2], lo: f64, hi: f64) -> Result<Parts> {
    b.surfaces.push(w.skin(lo));
    b.surfaces.push(w.skin(hi));
    let (s_lo, s_hi) = (b.surfaces.len() - 2, b.surfaces.len() - 1);
    let mut top_loops = Vec::new();
    let mut bottom_loops = Vec::new();
    let mut sides = Vec::new();
    for l in loops {
        let first_vertex = b.vertices.len();
        let n = l.len();
        // Each loop corner, under and over, and the straight edge between them.
        let mut verticals = Vec::with_capacity(n);
        for piece in l {
            let uv = w.uv(piece[0]);
            let (p, q) = (w.at(uv, lo), w.at(uv, hi));
            b.vertices.push(p);
            b.vertices.push(q);
            b.curves.push(Curve::Line { origin: p, dir: q - p });
            let v = b.vertices.len();
            b.edges.push(Edge { curve: Some(b.curves.len() - 1), reversed: false, start: Some(v - 2), end: Some(v - 1), tolerance: None });
            verticals.push(b.edges.len() - 1);
        }
        let (mut top, mut bottom) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for (k, piece) in l.iter().enumerate() {
            let (a, z) = (first_vertex + 2 * k, first_vertex + 2 * ((k + 1) % n));
            let (c_lo, c_hi, wall) = wall_geometry(w, piece, lo, hi)?;
            b.curves.push(c_lo.0);
            b.edges.push(Edge { curve: Some(b.curves.len() - 1), reversed: c_lo.1, start: Some(a), end: Some(z), tolerance: None });
            let e_lo = b.edges.len() - 1;
            b.curves.push(c_hi.0);
            b.edges.push(Edge { curve: Some(b.curves.len() - 1), reversed: c_hi.1, start: Some(a + 1), end: Some(z + 1), tolerance: None });
            let e_hi = b.edges.len() - 1;
            b.surfaces.push(wall);
            let (va, vz) = (verticals[k], verticals[(k + 1) % n]);
            sides.push(Face {
                surface: b.surfaces.len() - 1,
                flip: false,
                loops: vec![vec![
                    Fin { edge: e_lo, forward: true, pcurve: None },
                    Fin { edge: vz, forward: true, pcurve: None },
                    Fin { edge: e_hi, forward: false, pcurve: None },
                    Fin { edge: va, forward: false, pcurve: None },
                ]],
                color: None,
            });
            top.push(Fin { edge: e_hi, forward: true, pcurve: None });
            bottom.push(Fin { edge: e_lo, forward: false, pcurve: None });
        }
        bottom.reverse();
        top_loops.push(top);
        bottom_loops.push(bottom);
    }
    Ok(Parts {
        top: Face { surface: s_hi, flip: false, loops: top_loops, color: None },
        bottom: Face { surface: s_lo, flip: true, loops: bottom_loops, color: None },
        sides,
    })
}

/// A curve and whether the edge on it runs against it.
type EdgeCurve = (Curve, bool);

/// One wall's curve under the face, over it (each with whether the edge runs against it) and its surface, whose
/// normal faces out of the profile. Exact where the outline runs round the axis (circles, a flat wall square to the
/// axis on a cylinder) or along it (straight lines, a flat wall through the axis); else fitted curves and a ruled
/// surface between them.
fn wall_geometry(w: &Wrapper, piece: &[(f64, f64)], lo: f64, hi: f64) -> Result<(EdgeCurve, EdgeCurve, Surface)> {
    let uvs: Vec<(f64, f64)> = piece.iter().map(|q| w.uv(*q)).collect();
    let (Some(&first), Some(&last)) = (uvs.first(), uvs.last()) else { return Err(bad("couldn't wrap that profile")) };
    let r = &w.round;
    let size = 1.0 + r.radius + w.v0.abs();
    let round_axis = uvs.iter().all(|p| (p.1 - first.1).abs() < 1e-10 * size);
    let along_axis = uvs.iter().all(|p| (p.0 - first.0).abs() < 1e-12);
    // A flat wall through the chord and the rise at the middle (u along the curve, v from under to over).
    let flat = |chord: DVec3, at: (f64, f64)| -> Option<Surface> {
        let rise = w.at(at, hi) - w.at(at, lo);
        let normal = chord.cross(rise).try_normalize()?;
        Some(Surface::Plane { origin: w.at(at, lo), normal, x: rise.normalize() })
    };
    if along_axis {
        let line = |h: f64| {
            let (p, q) = (w.at(first, h), w.at(last, h));
            (Curve::Line { origin: p, dir: q - p }, false)
        };
        let chord = w.at(last, lo) - w.at(first, lo);
        if let Some(plane) = flat(chord, first) {
            return Ok((line(lo), line(hi), plane));
        }
    }
    if round_axis && !r.cone {
        let circle =
            |h: f64| (Curve::Circle { center: r.loc + r.z * first.1, normal: r.z, x: w.d0, radius: r.radius + h }, last.0 < first.0);
        let mid = uvs[uvs.len() / 2];
        let chord = w.at(last, lo) - w.at(first, lo);
        let tangent = if chord.length() > 1e-9 * size { chord } else { w.at(uvs[uvs.len() / 2 + 1], lo) - w.at(mid, lo) };
        if let Some(Surface::Plane { origin, normal, x }) = flat(tangent, mid) {
            let normal = r.z * normal.dot(r.z).signum();
            let x = (x - normal * x.dot(normal)).try_normalize().unwrap_or(w.d0);
            return Ok((circle(lo), circle(hi), Surface::Plane { origin, normal, x }));
        }
    }
    let (fit_lo, fit_hi) = fit_pair(w, piece, lo, hi)?;
    let (knots, mults) = fit_lo.knot_runs();
    let mults: Vec<u32> = mults.iter().map(|m| *m as u32).collect();
    let nurbs = |poles: Vec<DVec3>| Nurbs {
        degree: fit_lo.degree as u32,
        poles,
        weights: None,
        knots: knots.clone(),
        mults: mults.clone(),
        periodic: false,
    };
    let mut poles = Vec::with_capacity(2 * fit_lo.poles.len());
    for (p, q) in fit_lo.poles.iter().zip(&fit_hi.poles) {
        poles.push(*p);
        poles.push(*q);
    }
    let wall = Surface::BSpline(NurbsSurface {
        u_degree: fit_lo.degree as u32,
        v_degree: 1,
        nu: fit_lo.poles.len(),
        nv: 2,
        poles,
        weights: None,
        u_knots: knots.clone(),
        u_mults: mults.clone(),
        v_knots: vec![0.0, 1.0],
        v_mults: vec![2, 2],
        u_periodic: false,
        v_periodic: false,
    });
    Ok(((Curve::BSpline(nurbs(fit_lo.poles.clone())), false), (Curve::BSpline(nurbs(fit_hi.poles.clone())), false), wall))
}

/// A loop walked the other way round.
fn back(l: &[Fin]) -> Vec<Fin> {
    l.iter().rev().map(|f| Fin { forward: !f.forward, ..*f }).collect()
}

/// The solid one profile makes between skins `lo` and `hi` off the face.
fn region_solid(w: &Wrapper, loops: &[Loop2], lo: f64, hi: f64) -> Result<Brep> {
    let mut b = Brep { kind: BodyKind::Solid, ..Default::default() };
    let p = walls(&mut b, w, loops, lo, hi)?;
    b.faces.push(p.top);
    b.faces.push(p.bottom);
    b.faces.extend(p.sides);
    b.shells.push((0..b.faces.len()).collect());
    Ok(b)
}

/// The profiles built into the face itself: each outline a hole in it (its holes left as islands of the face),
/// walls and a cap off it, or walls and a floor into it.
fn into_face(b: &mut Brep, target: usize, w: &Wrapper, profiles: &[Vec<Loop2>], depth: f64) -> Result<()> {
    let (surface, flip, color) = (b.faces[target].surface, b.faces[target].flip, b.faces[target].color);
    let mut made = Vec::new();
    for loops in profiles {
        let mut p = walls(b, w, loops, depth.min(0.0), depth.max(0.0))?;
        // The loops lying on the face, walked about its normal: the outline counter-clockwise first.
        let on_face: Vec<Vec<Fin>> = if depth > 0.0 { p.bottom.loops.iter().map(|l| back(l)).collect() } else { p.top.loops.clone() };
        for (k, l) in on_face.iter().enumerate() {
            // The outline is a hole in the face; a hole in the profile leaves an island of the face.
            let walked = back(l);
            if k == 0 {
                b.faces[target].loops.push(walked);
            } else {
                made.push(Face { surface, flip, loops: vec![walked], color });
            }
        }
        if depth > 0.0 {
            p.top.color = color;
            made.push(p.top);
            made.extend(p.sides.into_iter().map(|f| Face { color, ..f }));
        } else {
            // A pocket: the walls and floor of the cut-out piece, turned to face into it.
            made.extend(p.sides.into_iter().map(|f| Face { flip: true, loops: f.loops.iter().map(|l| back(l)).collect(), color, ..f }));
            made.push(Face { surface: p.bottom.surface, flip: false, loops: p.bottom.loops.iter().map(|l| back(l)).collect(), color });
        }
    }
    let shell = b.shells.iter().position(|sh| sh.contains(&target)).ok_or_else(|| bad("the face isn't in a shell"))?;
    for f in made {
        b.faces.push(f);
        let i = b.faces.len() - 1;
        b.shells[shell].push(i);
    }
    Ok(())
}

/// Whether every profile lies inside the face with room to spare and clear of its other edges, so it can be built
/// into the face. `rim` is points along the face's edges.
fn fits_inside(w: &Wrapper, rim: &[DVec3], profiles: &[Vec<Loop2>]) -> bool {
    let r = &w.round;
    // Face points in the wrapper's (u from the centre, v).
    let uv_of = |p: DVec3| {
        let q = p - r.loc;
        let a = q.dot(r.z);
        let radial = q - r.z * a;
        let mut u = radial.dot(w.t).atan2(radial.dot(w.d0)) - w.u0;
        u = (u + PI).rem_euclid(TAU) - PI;
        (u, if r.cone { q.length() } else { a })
    };
    let rim: Vec<(f64, f64)> = rim.iter().map(|p| uv_of(*p)).collect();
    if rim.is_empty() {
        return false;
    }
    let profile_uv: Vec<Vec<(f64, f64)>> = profiles
        .iter()
        .map(|loops| {
            loops[0]
                .iter()
                .flat_map(|p| {
                    p.iter().map(|q| {
                        let (u, v) = w.uv(*q);
                        (u - w.u0, v)
                    })
                })
                .collect()
        })
        .collect();
    let all: Vec<(f64, f64)> = profile_uv.iter().flatten().copied().collect();
    // Measured from the profiles' centre, the face's angles go only half-way round each side.
    if all.iter().any(|p| p.0.abs() > PI - 0.01) {
        return false;
    }
    let margin = 1e-4 * (1.0 + r.loc.abs().max_element() + r.radius);
    let (vlo, vhi) = rim.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    if all.iter().any(|p| p.1 <= vlo + margin || p.1 >= vhi - margin) {
        return false;
    }
    // A face that doesn't go all the way round: the profile must stay out of the angles it leaves open.
    let mut us: Vec<f64> = rim.iter().map(|p| p.0).collect();
    us.sort_by(f64::total_cmp);
    let gaps = us.windows(2).map(|x| (x[0], x[1])).chain(us.last().zip(us.first()).map(|(l, f)| (*l, f + TAU)));
    for (a, b) in gaps {
        if b - a > 0.3 && all.iter().any(|p| (p.0 > a && p.0 < b) || (p.0 + TAU > a && p.0 + TAU < b)) {
            return false;
        }
    }
    // No edge of the face (a hole, an earlier emboss) inside an outline, and the outlines apart from each other.
    let scale = if r.cone { w.v0 * r.angle.sin() } else { r.radius };
    let flat = |p: (f64, f64)| (p.0 * scale, p.1);
    for (k, poly) in profile_uv.iter().enumerate() {
        let poly: Vec<(f64, f64)> = poly.iter().map(|p| flat(*p)).collect();
        if rim.iter().any(|p| crate::surgery::inside_2d(flat(*p), &poly)) {
            return false;
        }
        for other in &profile_uv[k + 1..] {
            if other.iter().any(|p| crate::surgery::inside_2d(flat(*p), &poly)) {
                return false;
            }
        }
    }
    let near = |a: &[(f64, f64)], b: &[(f64, f64)]| a.iter().any(|p| b.iter().any(|q| (flat(*p).0 - flat(*q).0).hypot(p.1 - q.1) < margin));
    !(0..profile_uv.len()).any(|i| (i + 1..profile_uv.len()).any(|j| near(&profile_uv[i], &profile_uv[j])))
}

pub fn wrap_emboss(
    body: &Shape,
    face: Option<u32>,
    regions: &[Shape],
    frame: PlaneFrame,
    depth: f64,
    rotation: f64,
    center: Option<DVec3>,
) -> Result<Shape> {
    if regions.is_empty() {
        return Err(bad("pick sketch profiles to wrap"));
    }
    if !(depth.is_finite() && depth.abs() > 1e-7) {
        return Err(bad("the emboss depth must not be zero"));
    }
    if !rotation.is_finite() || center.is_some_and(|c| !c.is_finite()) {
        return Err(bad("bad rotation or centre"));
    }
    let s = exact(body)?;
    let nrm = frame.x.cross(frame.y).try_normalize().ok_or_else(|| bad("bad sketch plane"))?;
    let profiles = regions.iter().map(|r| crate::surgery::face_outlines(r, DENSE)).collect::<Result<Vec<_>>>()?;
    let all: Vec<DVec3> = profiles.iter().flatten().flatten().flatten().flat_map(|p| p.pts.iter().copied()).collect();
    if all.is_empty() {
        return Err(bad("empty profiles"));
    }
    let (mn, mx) = all.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let pc = (mn + mx) / 2.0;

    // The face: the one named, else the round face nearest the profiles' centre.
    let (rounds, mut breps) = round_faces(s)?;
    let pick = match face {
        Some(i) => {
            crate::face_at(s, i)?;
            rounds.iter().find(|r| r.index == i).ok_or_else(|| bad(NEEDS_ROUND))?
        }
        None => {
            let score = |c: &Candidate| {
                let (gap, a) = c.round.gap(pc);
                let heights: Vec<f64> = c.rim.iter().map(|p| (*p - c.round.loc).dot(c.round.z)).collect();
                let lo = heights.iter().copied().fold(f64::MAX, f64::min);
                let hi = heights.iter().copied().fold(f64::MIN, f64::max);
                let over = if heights.is_empty() { 0.0 } else { (lo - a).max(a - hi).max(0.0) };
                let near = c.rim.iter().map(|p| p.distance(pc)).fold(f64::MAX, f64::min);
                gap.hypot(over).min(near)
            };
            rounds.iter().min_by(|a, b| score(a).total_cmp(&score(b))).ok_or_else(|| bad("the body has no cylindrical or conical face"))?
        }
    };
    let round = pick.round;

    let z = round.z;
    let side = nrm - z * nrm.dot(z);
    let d0 = side.try_normalize().ok_or_else(|| bad("the sketch faces along the axis: draw it on a plane beside the face"))?;
    let t = z.cross(d0);
    let mut across = nrm.cross(t);
    if across.dot(z) < 0.0 {
        across = -across;
    }
    let dc = center.unwrap_or(pc) - round.loc;
    let mut w = Wrapper {
        round,
        d0,
        t,
        center: pc,
        along: t,
        across: across.normalize(),
        rot: (rotation.cos(), rotation.sin()),
        u0: 0.0,
        seam: 0.0,
        v0: 0.0,
    };
    if !round.cone {
        w.u0 = if center.is_some() { dc.dot(t).atan2(dc.dot(d0)) } else { dc.dot(t) / round.radius };
        w.v0 = dc.dot(z);
    } else {
        let rho = dc.dot(z) / round.angle.cos();
        if rho.is_nan() || rho <= 1e-6 {
            return Err(bad("the profile sits beyond the cone's tip"));
        }
        w.v0 = rho;
        let r = rho * round.angle.sin();
        w.u0 = if center.is_some() { dc.dot(t).atan2(dc.dot(d0)) } else { dc.dot(t) / r };
    }

    let sink = 1e-3_f64.max((0.25 * depth.abs()).min(0.05 * if round.cone { 1.0 } else { round.radius }));
    let (lo, hi) = if depth > 0.0 { (-sink, depth) } else { (depth, sink) };
    if !round.cone && round.radius + lo <= 1e-6 {
        return Err(bad("the face is too small for that engraving"));
    }

    let mut flat = Vec::new();
    for faces in &profiles {
        for loops in faces {
            let l2 = loops_2d(&w, loops)?;
            if round.cone && l2.iter().flatten().flatten().any(|q| w.uv(*q).1 + lo.min(0.0) / round.angle.tan().max(1e-9) <= 1e-6) {
                return Err(bad("the profile runs past the cone's tip"));
            }
            flat.push(l2);
        }
    }

    fill_holes(&mut flat);
    let (umin, umax) =
        flat.iter().flatten().flatten().flatten().map(|q| w.uv(*q).0).fold((f64::MAX, f64::MIN), |(a, b), u| (a.min(u), b.max(u)));
    if umax - umin >= TAU - 1e-6 {
        return Err(bad("the profiles would wrap all the way round the face and overlap"));
    }
    w.seam = (umin + umax) / 2.0 + PI;

    // Inside one outward face: built into it.
    if !pick.inward && fits_inside(&w, &pick.rim, &flat) {
        let brep = &mut breps[pick.body];
        into_face(brep, pick.face, &w, &flat, depth)?;
        return crate::surgery::rebuild(s, pick.body, brep);
    }
    let mut tools = Vec::new();
    for l2 in &flat {
        tools.push(RustKernel.from_brep(&region_solid(&w, l2, lo, hi)?)?);
    }
    let op = if depth > 0.0 { BoolOp::Union } else { BoolOp::Subtract };
    let mut out = body.clone();
    for tool in &tools {
        out = RustKernel.boolean(op, &out, tool)?;
    }
    Ok(out)
}
