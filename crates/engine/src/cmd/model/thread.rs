//! Thread: a real screw thread on a cylindrical face. A boss gets an external thread (a bolt), a hole an
//! internal one (a nut). Within the thread's extent the face is rebuilt at the standard's diameters, whatever its
//! own diameter was, and the tooth (or the groove) is swept along a helix.
//!
//! Every thread of the same pitch and hand on the same axis line shares one helix (its phase comes from the axis
//! line, not from where the face starts), so a bolt and the nut made for it line up however long each is.

use glam::DVec3;
use serde_json::{Value, json};
use shapecraft_doc::{ItemId, Pick};
use shapecraft_kernel::{Axis, BoolOp, PlanarCurve, PlaneFrame, Shape, SubShape};

use super::set_body_shape;
use crate::thread::{Profile, Standard, profile};
use crate::{EngineError, Params, Result, Session};

/// A cylindrical face as the Thread tool sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThreadFace {
    /// Its axis line, the same for every face on that line: through the point nearest the world origin, pointing
    /// along the larger coordinate.
    pub axis: Axis,
    /// Square to the axis, fixed by the axis alone: where a thread's turns are counted from.
    pub x: DVec3,
    pub radius: f64,
    /// Where the face starts and ends along the axis.
    pub lo: f64,
    pub hi: f64,
    /// A hole (the face looks at its axis): the thread goes in, not on.
    pub internal: bool,
}

impl ThreadFace {
    pub fn at(&self, r: f64, t: f64) -> DVec3 {
        self.axis.origin + self.x * r + self.axis.dir * t
    }
}

/// The axis line in its one way of being written.
fn canonical(axis: Axis) -> Option<(Axis, DVec3)> {
    let mut d = axis.dir.try_normalize()?;
    let a = d.abs();
    let big = if a.x >= a.y && a.x >= a.z {
        d.x
    } else if a.y >= a.z {
        d.y
    } else {
        d.z
    };
    if big < 0.0 {
        d = -d;
    }
    let o = axis.origin - d * axis.origin.dot(d);
    // The world axis furthest from the line, made square to it.
    let e = if a.x <= a.y && a.x <= a.z {
        DVec3::X
    } else if a.y <= a.z {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let x = (e - d * e.dot(d)).try_normalize()?;
    Some((Axis { origin: o, dir: d }, x))
}

/// A face of a body read as a cylinder; `None` for anything else (flat faces, cones, spheres, free-form faces).
pub fn thread_face(s: &Session, body: ItemId, face: u32) -> Option<ThreadFace> {
    let radius = s.face_radius(body, face)?;
    let b = s.doc.body(body)?;
    let raw = s.kernel.axis_of(&b.shape, SubShape::Face(face)).ok().flatten()?;
    let (axis, x) = canonical(raw)?;
    let m = &b.mesh;
    let (mut lo, mut hi, mut out) = (f64::MAX, f64::MIN, 0.0);
    for (t, f) in m.triangle_faces.iter().enumerate() {
        if *f != face {
            continue;
        }
        for k in 0..3 {
            let i = *m.indices.get(t * 3 + k)? as usize;
            let p = DVec3::from(m.positions.get(i)?.map(f64::from));
            let n = DVec3::from(m.normals.get(i)?.map(f64::from));
            let along = (p - axis.origin).dot(axis.dir);
            lo = lo.min(along);
            hi = hi.max(along);
            out += (p - axis.origin - axis.dir * along).dot(n);
        }
    }
    (hi - lo > 1e-6).then_some(ThreadFace { axis, x, radius, lo, hi, internal: out < 0.0 })
}

/// What a thread is asked to be.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSpec {
    pub standard: Standard,
    /// The size's name (as the list shows it) and its nominal major diameter.
    pub size: String,
    pub major: f64,
    pub pitch: f64,
    pub left: bool,
    /// Custom Length: how long from the start end; `None` runs the whole face.
    pub length: Option<f64>,
    /// Start at the other end of the face.
    pub flip: bool,
    /// Print clearance, mm all round (a bolt shrinks by it, a nut grows).
    pub clearance: f64,
}

/// The spec from the parameters; what's left out comes from the face (the nearest size, its coarsest pitch).
pub fn spec(p: &Params, face: &ThreadFace) -> Result<ThreadSpec> {
    let standard = match p.opt_str("standard") {
        Some(k) => Standard::from_key(k).ok_or_else(|| EngineError::BadParams(format!("no thread standard '{k}' (iso, ansi, acme)")))?,
        None => Standard::Iso,
    };
    let size = match p.opt_str("size") {
        Some(name) => standard.size(name).ok_or_else(|| EngineError::BadParams(format!("{} has no size '{name}'", standard.label())))?,
        None => standard.nearest(face.radius * 2.0),
    };
    let pitch = if p.has("tpi") {
        let tpi = p.f64("tpi")?;
        if !(tpi.is_finite() && (0.5..=400.0).contains(&tpi)) {
            return Err(EngineError::BadParams("threads per inch must be between 0.5 and 400".into()));
        }
        crate::thread::INCH / tpi
    } else if p.has("pitch") {
        p.f64("pitch")?
    } else {
        size.default_pitch()
    };
    if !(pitch.is_finite() && (0.01..=50.0).contains(&pitch)) {
        return Err(EngineError::BadParams("the pitch must be between 0.01 and 50 mm".into()));
    }
    let left = match p.opt_str("hand").unwrap_or("right") {
        "right" => false,
        "left" => true,
        other => return Err(EngineError::BadParams(format!("hand is 'right' or 'left', not '{other}'"))),
    };
    let length = match p.opt_str("extent").unwrap_or("full") {
        "full" => None,
        "custom" => {
            let l = if p.has("length") { p.f64("length")? } else { face.hi - face.lo };
            if !(l.is_finite() && l > 1e-6 && l < 1e6) {
                return Err(EngineError::BadParams("the thread length must be positive".into()));
            }
            Some(l)
        }
        other => return Err(EngineError::BadParams(format!("extent is 'full' or 'custom', not '{other}'"))),
    };
    let clearance = p.f64_or("clearance", 0.0)?;
    if !(clearance.is_finite() && (0.0..=5.0).contains(&clearance)) {
        return Err(EngineError::BadParams("the print clearance must be between 0 and 5 mm".into()));
    }
    Ok(ThreadSpec { standard, size: size.name.clone(), major: size.major, pitch, left, length, flip: p.bool_or("flip", false), clearance })
}

impl ThreadSpec {
    /// Where the thread runs along the face's axis.
    pub fn extent(&self, face: &ThreadFace) -> (f64, f64) {
        match self.length {
            None => (face.lo, face.hi),
            Some(l) => {
                let l = l.min(face.hi - face.lo);
                if self.flip { (face.hi - l, face.hi) } else { (face.lo, face.lo + l) }
            }
        }
    }

    pub fn profile(&self, face: &ThreadFace) -> Profile {
        profile(self.standard, self.major, self.pitch, face.internal, self.clearance)
    }
}

/// A ring (or a solid round when `r0` is 0) around the face's axis, `r0..r1` out and `a..b` along.
fn ring(s: &Session, f: &ThreadFace, r0: f64, r1: f64, a: f64, b: f64) -> Result<Shape> {
    let pts = [f.at(r0, a), f.at(r1, a), f.at(r1, b), f.at(r0, b)];
    Ok(s.kernel.revolve_polygon(&pts, f.axis, std::f64::consts::TAU)?)
}

/// The ways a coil is swept, tried in turn: path points a turn (16 is under a micron off the true helix at these radii),
/// and where on the turn its seam lies (a part of a turn). The same thread every time, but the kernel's sweep and
/// booleans trip over some sizes with one and not with another (#94).
const ATTEMPTS: [(u32, f64); 6] = [(16, 0.0), (16, 0.37), (24, 0.13), (12, 0.71), (20, 0.55), (32, 0.83)];

/// How far off its worked-out volume (as a part of the coil's) a thread may be and still count as right.
const CLOSE: f64 = 0.01;

/// The volume of the tooth (or groove) from `r0` to `r1` out, per mm of thread: a thread's section square to its axis
/// has the same area all along, only turned.
fn section(pr: &Profile, internal: bool, r0: f64, r1: f64) -> f64 {
    // 2πr · w(r)/p with w linear in r: Simpson's rule is exact.
    let g = |r: f64| std::f64::consts::TAU * r * pr.width_at(r, internal) / pr.pitch;
    (r1 - r0) / 6.0 * (g(r0) + 4.0 * g((r0 + r1) / 2.0) + g(r1))
}

/// The tooth (a bolt) or the groove (a nut) swept along the helix over `a..b`, cut square at both ends.
fn coil(s: &Session, f: &ThreadFace, pr: &Profile, left: bool, a: f64, b: f64, (per_turn, seam): (u32, f64)) -> Result<Shape> {
    let p = pr.pitch;
    let rin = inner(pr);
    let rout = pr.major;
    // At the counting line (x) a tooth's middle sits on a whole number of pitches: the same for every face on
    // this axis line.
    let start = ((a - p) / p).floor() * p - p + seam * p;
    let turns = ((b + p - start) / p).ceil() + 1.0;
    let turn = seam * std::f64::consts::TAU * if left { -1.0 } else { 1.0 };
    let x = glam::DQuat::from_axis_angle(f.axis.dir, turn) * f.x;
    let at = |r: f64, t: f64| f.axis.origin + x * r + f.axis.dir * t;
    let (wi, wo) = (pr.width_at(rin, f.internal) / 2.0, pr.width_at(rout, f.internal) / 2.0);
    let corners = [at(rin, start - wi), at(rout, start - wo), at(rout, start + wo), at(rin, start + wi)];
    let angle = turns * std::f64::consts::TAU * if left { -1.0 } else { 1.0 };
    let swept = match s.kernel.helix_coil(&corners, f.axis, angle, turns * p, per_turn) {
        Ok(c) => c,
        Err(_) => {
            let lines: Vec<PlanarCurve> = (0..4).map(|k| PlanarCurve::Line { a: corners[k], b: corners[(k + 1) % 4] }).collect();
            let frame = PlaneFrame { origin: f.axis.origin, x, y: f.axis.dir };
            let profile = s
                .kernel
                .planar_regions(frame, &lines)?
                .into_iter()
                .next()
                .ok_or_else(|| EngineError::NotAvailable("the thread's profile couldn't be made".into()))?;
            s.kernel.revolve_helix_with(&profile, f.axis, angle, turns * p, per_turn)?
        }
    };
    let bounds = ring(s, f, 0.0, rout + p, a, b)?;
    Ok(s.kernel.boolean(BoolOp::Intersect, &swept, &bounds)?)
}

/// Where the tooth starts: into the body a little (an overlap the union can't miss); the thread module checks it fits
/// every size.
fn inner(pr: &Profile) -> f64 {
    pr.minor - crate::thread::OVERLAP * pr.pitch
}

/// Threads one face of `shape` (worked out from where the face is, so it doesn't matter what its number is now).
pub fn thread_shape(s: &Session, shape: &Shape, f: &ThreadFace, spec: &ThreadSpec) -> Result<Shape> {
    let pr = spec.profile(f);
    if pr.minor <= 1e-3 {
        return Err(EngineError::NotAvailable("this thread is too small to model".into()));
    }
    let (a, b) = spec.extent(f);
    if b - a < 1e-3 {
        return Err(EngineError::NotAvailable("the thread needs some length".into()));
    }
    // A little past the face, so the new walls never lie on the old one.
    let past = (0.01 * f.radius).clamp(0.005, 0.05);
    // The wall goes to the thread's base: a bolt's roots, a nut's crests.
    let base = pr.minor;
    let grows = f.internal == (f.radius > base);
    let rough = if grows {
        let (r0, r1) = if f.internal { (base, f.radius + past) } else { ((f.radius - past).max(0.0), base) };
        s.kernel.boolean(BoolOp::Union, shape, &ring(s, f, r0, r1, a, b)?)?
    } else {
        let (r0, r1) = if f.internal { ((f.radius - past).max(0.0), base) } else { (base, f.radius + past) };
        s.kernel.boolean(BoolOp::Subtract, shape, &ring(s, f, r0, r1, a, b)?)?
    };
    let before = s.kernel.mass_props(&rough)?.volume;
    // A hair inside the extent: ends flush with the body's own end faces make the union fail on some sizes.
    let (a, b) = (a + 1e-3, b - 1e-3);
    let whole = section(&pr, f.internal, inner(&pr), pr.major) * (b - a);
    // What it adds to a bolt (or takes out of a nut) whose wall is all there.
    let expected = section(&pr, f.internal, base, pr.major) * (b - a);
    let (mut best, mut last): (Option<(f64, Shape)>, _) = (None, None);
    for way in ATTEMPTS {
        let made = coil(s, f, &pr, spec.left, a, b, way).and_then(|c| {
            let v = s.kernel.mass_props(&c)?.volume;
            if (v - whole).abs() > whole * CLOSE {
                return Err(EngineError::NotAvailable("the thread couldn't be swept".into()));
            }
            let op = if f.internal { BoolOp::Subtract } else { BoolOp::Union };
            let out = s.kernel.boolean(op, &rough, &c)?;
            let change = (s.kernel.mass_props(&out)?.volume - before) * if f.internal { -1.0 } else { 1.0 };
            Ok((change, out))
        });
        match made {
            // More than the whole tooth, or the other way: the kernel lost part of the body.
            Ok((change, _)) if !(-whole * CLOSE..=whole * (1.0 + CLOSE)).contains(&change) => {}
            Ok((change, out)) => {
                let off = (change - expected).abs() / whole;
                // Two ways that come out the same are right too: a wall with holes across it, or walls close by.
                let agree = best.as_ref().is_some_and(|(o, _)| (off - o).abs() <= CLOSE);
                if best.as_ref().is_none_or(|(o, _)| off < *o) {
                    best = Some((off, out));
                }
                if off <= CLOSE || agree {
                    break;
                }
            }
            Err(e) => last = Some(e),
        }
    }
    match (best, last) {
        (Some((_, out)), _) => Ok(out),
        (None, Some(e)) => Err(e),
        (None, None) => Err(EngineError::NotAvailable("the thread couldn't be built on this face".into())),
    }
}

pub(super) fn thread(s: &mut Session, p: &Params) -> Result<Value> {
    const PICK: &str = "Select a cylindrical face to thread";
    let (body, faces) = super::faces::face_target(s, p, PICK)?;
    if faces.is_empty() {
        return Err(EngineError::NotAvailable(PICK.into()));
    }
    if faces.len() > 64 {
        return Err(EngineError::BadParams("thread at most 64 faces at once".into()));
    }
    let b = s.doc.body(body).ok_or_else(|| EngineError::BadParams(format!("no body {}", body.0)))?;
    if b.is_mesh() {
        return Err(EngineError::NotAvailable("Thread works on solid bodies, not meshes".into()));
    }
    let mut jobs = Vec::new();
    for &face in &faces {
        let f = thread_face(s, body, face).ok_or_else(|| EngineError::NotAvailable("Thread works on cylindrical faces".into()))?;
        jobs.push((f, spec(p, &f)?));
    }
    let mut shape = b.shape.clone();
    for (f, spec) in &jobs {
        shape = thread_shape(s, &shape, f, spec)?;
    }
    set_body_shape(s, body, shape)?;
    s.selection.retain(|pk| !matches!(pk, Pick::Face { body: b, .. } if *b == body));
    let first = &jobs[0];
    let pr = first.1.profile(&first.0);
    Ok(json!({
        "body": body.0,
        "standard": first.1.standard.key(),
        "size": first.1.size,
        "pitch": first.1.pitch,
        "internal": first.0.internal,
        "major_diameter": pr.major * 2.0,
        "minor_diameter": pr.minor * 2.0,
    }))
}
