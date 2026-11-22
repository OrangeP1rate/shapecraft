//! IGES 5.3 B-rep solids (Manifold Solid B-Rep Objects, type 186), written from and read into the kernel-neutral
//! [`Brep`]: shells (514), faces (510), loops (508), edge lists (504), vertex lists (502); planes, cylinders, cones,
//! spheres and tori (190-198), surfaces of revolution (120), tabulated cylinders (122) and NURBS surfaces (128); lines
//! (110), circular arcs (100) placed by transformation matrices (124) and NURBS curves (126). This is what Open
//! CASCADE writes in its B-rep mode, so files go both ways between the two kernels. Files of loose trimmed surfaces
//! (144) are refused with a reason.

use std::collections::HashMap;
use std::fmt::Write as _;

use shapecraft_kernel::brep::{BodyKind, Brep, Curve, Edge, Face, Fin, Nurbs, NurbsSurface, Surface};
use shapecraft_kernel::{DVec3, KernelError, Result};

fn bad(message: impl Into<String>) -> KernelError {
    KernelError::InvalidInput(message.into())
}

/// A real as IGES wants it: always a decimal point, an upper-case exponent.
fn real(x: f64) -> Result<String> {
    if !x.is_finite() {
        return Err(bad("a number in the body isn't finite"));
    }
    if x == 0.0 {
        return Ok("0.".into());
    }
    let s = format!("{x:?}");
    let (mantissa, exp) = match s.split_once('e') {
        Some((m, e)) => (m.to_string(), Some(e.to_string())),
        None => (s, None),
    };
    let mantissa = if mantissa.contains('.') { mantissa } else { format!("{mantissa}.") };
    Ok(match exp {
        Some(e) => format!("{mantissa}E{e}"),
        None => mantissa,
    })
}

fn hollerith(s: &str) -> String {
    format!("{}H{s}", s.len())
}

// ---------------------------------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------------------------------

struct Entity {
    kind: u32,
    form: u32,
    params: Vec<String>,
    matrix: usize,
    top: bool,
}

#[derive(Default)]
struct Writer {
    entities: Vec<Entity>,
}

impl Writer {
    /// Adds an entity; its pointer (the sequence number of its first directory line).
    fn add(&mut self, kind: u32, form: u32, params: Vec<String>) -> usize {
        self.entities.push(Entity { kind, form, params, matrix: 0, top: false });
        2 * self.entities.len() - 1
    }

    fn point(&mut self, p: DVec3) -> Result<usize> {
        Ok(self.add(116, 0, vec![real(p.x)?, real(p.y)?, real(p.z)?]))
    }

    fn direction(&mut self, d: DVec3) -> Result<usize> {
        let d = d.normalize_or_zero();
        if d == DVec3::ZERO {
            return Err(bad("a direction of no length"));
        }
        Ok(self.add(123, 0, vec![real(d.x)?, real(d.y)?, real(d.z)?]))
    }

    fn reals(xs: impl IntoIterator<Item = f64>) -> Result<Vec<String>> {
        xs.into_iter().map(real).collect()
    }

    fn nurbs_curve(&mut self, n: &Nurbs<DVec3>) -> Result<usize> {
        if n.periodic {
            return Err(bad("a periodic spline curve can't be written to IGES yet"));
        }
        let knots = shapecraft_kernel::nurbs::expand(&n.knots, &n.mults);
        let k = n.poles.len();
        if k < 2 || knots.len() != k + n.degree as usize + 1 {
            return Err(bad("a spline curve with knots that don't match its poles"));
        }
        let mut p = vec![(k - 1).to_string(), n.degree.to_string()];
        let closed = n.poles.first().zip(n.poles.last()).is_some_and(|(a, b)| a.distance(*b) < 1e-9);
        p.extend(["0".into(), u32::from(closed).to_string(), u32::from(n.weights.is_none()).to_string(), "0".into()]);
        p.extend(Self::reals(knots.iter().copied())?);
        match &n.weights {
            Some(w) if w.len() == k => p.extend(Self::reals(w.iter().copied())?),
            Some(_) => return Err(bad("a spline curve's weights don't match its poles")),
            None => p.extend(std::iter::repeat_n("1.".to_string(), k)),
        }
        for q in &n.poles {
            p.extend(Self::reals([q.x, q.y, q.z])?);
        }
        let (Some(a), Some(b)) = (knots.first(), knots.last()) else { return Err(bad("a spline curve with no knots")) };
        p.extend(Self::reals([*a, *b, 0.0, 0.0, 0.0])?);
        Ok(self.add(126, 0, p))
    }

    /// The edge's curve, running from `from` to `to` (its own direction made to agree).
    fn edge_curve(&mut self, c: &Curve, reversed: bool, from: DVec3, to: DVec3) -> Result<usize> {
        match c {
            Curve::Line { .. } => Ok(self.add(110, 0, Self::reals([from.x, from.y, from.z, to.x, to.y, to.z])?)),
            Curve::Circle { center, normal, radius, .. } => {
                let n = normal.normalize() * if reversed { -1.0 } else { 1.0 };
                let x = (from - *center).normalize_or_zero();
                if x == DVec3::ZERO || n == DVec3::ZERO {
                    return Err(bad("a circle's edge doesn't start on it"));
                }
                let x = (x - n * x.dot(n)).normalize();
                let y = n.cross(x);
                let e = to - *center;
                let mut a = e.dot(y).atan2(e.dot(x));
                if a <= 1e-12 {
                    a += std::f64::consts::TAU;
                }
                let m = self.add(124, 0, Self::reals([x.x, y.x, n.x, center.x, x.y, y.y, n.y, center.y, x.z, y.z, n.z, center.z])?);
                let arc = self.add(100, 0, Self::reals([0.0, 0.0, 0.0, *radius, 0.0, radius * a.cos(), radius * a.sin()])?);
                if let Some(last) = self.entities.last_mut() {
                    last.matrix = m;
                }
                Ok(arc)
            }
            Curve::Ellipse { center, normal, x, major, minor } => {
                let n = normal.normalize();
                let xa = x.normalize();
                let ya = n.cross(xa);
                let w = std::f64::consts::FRAC_1_SQRT_2;
                let dirs =
                    [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (-1.0, 1.0), (-1.0, 0.0), (-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (1.0, 0.0)];
                let mut b = Nurbs {
                    degree: 2,
                    poles: dirs.iter().map(|(a, b)| *center + xa * (a * major) + ya * (b * minor)).collect(),
                    weights: Some((0..9).map(|i| if i % 2 == 1 { w } else { 1.0 }).collect()),
                    knots: vec![0.0, 0.25, 0.5, 0.75, 1.0],
                    mults: vec![3, 2, 2, 2, 3],
                    periodic: false,
                };
                if reversed {
                    b.poles.reverse();
                }
                self.nurbs_curve(&b)
            }
            Curve::BSpline(n) => {
                let mut n = n.clone();
                if reversed {
                    n.poles.reverse();
                    if let Some(w) = n.weights.as_mut() {
                        w.reverse();
                    }
                    let (lo, hi) = (n.knots.first().copied().unwrap_or(0.0), n.knots.last().copied().unwrap_or(1.0));
                    n.knots = n.knots.iter().rev().map(|k| lo + hi - k).collect();
                    n.mults.reverse();
                }
                self.nurbs_curve(&n)
            }
            Curve::Through(points) => {
                let mut pts = points.clone();
                if reversed {
                    pts.reverse();
                }
                if pts.len() < 2 {
                    return Err(bad("a curve through fewer than two points"));
                }
                let mut knots = vec![0.0];
                for w in pts.windows(2) {
                    knots.push(knots.last().copied().unwrap_or(0.0) + w[0].distance(w[1]).max(1e-12));
                }
                let mut mults = vec![1; knots.len()];
                mults[0] = 2;
                if let Some(m) = mults.last_mut() {
                    *m = 2;
                }
                self.nurbs_curve(&Nurbs { degree: 1, poles: pts, weights: None, knots, mults, periodic: false })
            }
            Curve::OnSurface { .. } => Err(bad("an edge with only a curve on a surface can't be written to IGES")),
        }
    }

    fn surface(&mut self, b: &Brep, s: &Surface) -> Result<usize> {
        Ok(match s {
            Surface::Plane { origin, normal, x } => {
                let (p, n, r) = (self.point(*origin)?, self.direction(*normal)?, self.direction(*x)?);
                self.add(190, 1, vec![p.to_string(), n.to_string(), r.to_string()])
            }
            Surface::Cylinder { origin, axis, x, radius } => {
                let (p, a, r) = (self.point(*origin)?, self.direction(*axis)?, self.direction(*x)?);
                self.add(192, 1, vec![p.to_string(), a.to_string(), real(*radius)?, r.to_string()])
            }
            Surface::Cone { origin, axis, x, radius, half_angle } => {
                // IGES's semi-angle is positive: a cone that narrows along its axis is the same cone about the axis
                // turned round.
                let (axis, angle) = if *half_angle < 0.0 { (-*axis, -*half_angle) } else { (*axis, *half_angle) };
                let (p, a, r) = (self.point(*origin)?, self.direction(axis)?, self.direction(*x)?);
                self.add(194, 1, vec![p.to_string(), a.to_string(), real(*radius)?, real(angle.to_degrees())?, r.to_string()])
            }
            Surface::Sphere { center, axis, x, radius } => {
                let (p, a, r) = (self.point(*center)?, self.direction(*axis)?, self.direction(*x)?);
                self.add(196, 1, vec![p.to_string(), real(*radius)?, a.to_string(), r.to_string()])
            }
            Surface::Torus { center, axis, x, major, minor } if *major > 0.0 => {
                let (p, a, r) = (self.point(*center)?, self.direction(*axis)?, self.direction(*x)?);
                self.add(198, 1, vec![p.to_string(), a.to_string(), real(*major)?, real(*minor)?, r.to_string()])
            }
            Surface::BSpline(n) => {
                if n.u_periodic || n.v_periodic {
                    return Err(bad("a periodic spline surface can't be written to IGES yet"));
                }
                let ku = shapecraft_kernel::nurbs::expand(&n.u_knots, &n.u_mults);
                let kv = shapecraft_kernel::nurbs::expand(&n.v_knots, &n.v_mults);
                if n.nu < 2 || n.nv < 2 || n.poles.len() != n.nu * n.nv {
                    return Err(bad("a spline surface with the wrong number of poles"));
                }
                if ku.len() != n.nu + n.u_degree as usize + 1 || kv.len() != n.nv + n.v_degree as usize + 1 {
                    return Err(bad("a spline surface with knots that don't match its poles"));
                }
                let mut p = vec![(n.nu - 1).to_string(), (n.nv - 1).to_string(), n.u_degree.to_string(), n.v_degree.to_string()];
                p.extend(["0", "0", if n.weights.is_none() { "1" } else { "0" }, "0", "0"].map(String::from));
                p.extend(Self::reals(ku.iter().copied())?);
                p.extend(Self::reals(kv.iter().copied())?);
                // IGES runs u fastest; the neutral form, v.
                for j in 0..n.nv {
                    for i in 0..n.nu {
                        p.push(real(n.weights.as_ref().and_then(|w| w.get(i * n.nv + j)).copied().unwrap_or(1.0))?);
                    }
                }
                for j in 0..n.nv {
                    for i in 0..n.nu {
                        let q = n.poles[i * n.nv + j];
                        p.extend(Self::reals([q.x, q.y, q.z])?);
                    }
                }
                let (Some(u0), Some(u1), Some(v0), Some(v1)) = (ku.first(), ku.last(), kv.first(), kv.last()) else {
                    return Err(bad("a spline surface with no knots"));
                };
                p.extend(Self::reals([*u0, *u1, *v0, *v1])?);
                self.add(128, 0, p)
            }
            Surface::Revolution { curve, origin, axis } => {
                let c = b.curves.get(*curve).ok_or_else(|| bad("a surface of revolution with no curve"))?;
                let line =
                    self.add(110, 0, Self::reals([origin.x, origin.y, origin.z, origin.x + axis.x, origin.y + axis.y, origin.z + axis.z])?);
                let generatrix = self.free_curve(c)?;
                self.add(120, 0, vec![line.to_string(), generatrix.to_string(), "0.".into(), real(std::f64::consts::TAU)?])
            }
            Surface::Extrusion { curve, dir } => {
                let c = b.curves.get(*curve).ok_or_else(|| bad("an extruded surface with no curve"))?;
                let directrix = self.free_curve(c)?;
                let start = start_of(c).ok_or_else(|| bad("an extruded surface's curve has no start"))?;
                let end = start + *dir;
                let mut p = vec![directrix.to_string()];
                p.extend(Self::reals([end.x, end.y, end.z])?);
                self.add(122, 0, p)
            }
            Surface::Torus { .. } => return Err(bad("a lemon-shaped torus can't be written to IGES")),
            Surface::Offset { .. } => return Err(bad("an offset surface can't be written to IGES yet")),
        })
    }

    /// A curve on its own (not an edge's): its whole natural span.
    fn free_curve(&mut self, c: &Curve) -> Result<usize> {
        match c {
            Curve::Circle { center, x, radius, .. } => {
                // A full turn: from its start round to its start.
                let p = *center + x.normalize() * *radius;
                self.edge_curve(c, false, p, p)
            }
            Curve::Line { origin, dir } => self.edge_curve(c, false, *origin, *origin + *dir),
            other => {
                let p = start_of(other).unwrap_or_default();
                self.edge_curve(other, false, p, p)
            }
        }
    }
}

fn start_of(c: &Curve) -> Option<DVec3> {
    match c {
        Curve::Line { origin, .. } => Some(*origin),
        Curve::Circle { center, x, radius, .. } => Some(*center + x.normalize() * *radius),
        Curve::Ellipse { center, x, major, .. } => Some(*center + x.normalize() * *major),
        Curve::BSpline(n) => n.poles.first().copied(),
        Curve::Through(p) => p.first().copied(),
        Curve::OnSurface { .. } => None,
    }
}

/// A rough size of each loop of a face (the diagonal of the box round its edges), to tell the outer loop of a flat
/// face from its holes.
fn loop_size(b: &Brep, l: &[Fin]) -> f64 {
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    let mut add = |p: DVec3| {
        lo = lo.min(p);
        hi = hi.max(p);
    };
    for fin in l {
        let Some(e) = b.edges.get(fin.edge) else { continue };
        for v in [e.start, e.end].into_iter().flatten() {
            if let Some(p) = b.vertices.get(v) {
                add(*p);
            }
        }
        match e.curve.and_then(|c| b.curves.get(c)) {
            Some(Curve::Circle { center, radius, .. }) => {
                add(*center - DVec3::splat(*radius));
                add(*center + DVec3::splat(*radius));
            }
            Some(Curve::Ellipse { center, major, .. }) => {
                add(*center - DVec3::splat(*major));
                add(*center + DVec3::splat(*major));
            }
            Some(Curve::BSpline(n)) => n.poles.iter().for_each(|p| add(*p)),
            Some(Curve::Through(ps)) => ps.iter().for_each(|p| add(*p)),
            _ => {}
        }
    }
    if lo.x <= hi.x { (hi - lo).length() } else { 0.0 }
}

/// The bodies as one IGES file (millimeters).
pub fn write(bodies: &[Brep]) -> Result<String> {
    let mut w = Writer::default();
    let mut tops = Vec::new();
    for b in bodies {
        if b.kind == BodyKind::Wire {
            return Err(bad("a wire body has no faces to write"));
        }
        // Vertices, one list a body.
        let mut vp = Vec::with_capacity(1 + 3 * b.vertices.len());
        vp.push(b.vertices.len().to_string());
        for v in &b.vertices {
            vp.extend(Writer::reals([v.x, v.y, v.z])?);
        }
        let vertex_list = w.add(502, 1, vp);
        // Edges, one list a body; each curve runs from its start vertex to its end.
        let mut ep = vec![b.edges.len().to_string()];
        let mut written = Vec::with_capacity(b.edges.len());
        for e in &b.edges {
            let (Some(s), Some(t)) = (e.start, e.end) else { return Err(bad("an edge without vertices can't be written to IGES")) };
            let (from, to) = (b.vertices.get(s).copied(), b.vertices.get(t).copied());
            let (Some(from), Some(to)) = (from, to) else { return Err(bad("an edge's vertex is missing")) };
            let c = e.curve.and_then(|c| b.curves.get(c)).ok_or_else(|| bad("an edge without a curve can't be written to IGES"))?;
            written.push(w.edge_curve(c, e.reversed, from, to)?);
        }
        for (k, e) in b.edges.iter().enumerate() {
            let (s, t) = (e.start.unwrap_or(0), e.end.unwrap_or(0));
            ep.extend([written[k].to_string(), vertex_list.to_string(), (s + 1).to_string(), vertex_list.to_string(), (t + 1).to_string()]);
        }
        let edge_list = w.add(504, 1, ep);
        let mut surfaces = HashMap::new();
        let mut faces = Vec::with_capacity(b.faces.len());
        for f in &b.faces {
            let surface = match surfaces.get(&f.surface) {
                Some(s) => *s,
                None => {
                    let s = w.surface(b, b.surfaces.get(f.surface).ok_or_else(|| bad("a face with no surface"))?)?;
                    surfaces.insert(f.surface, s);
                    s
                }
            };
            // Loops about the surface's normal, the outer one of a flat face first.
            let mut loops: Vec<Vec<Fin>> = f
                .loops
                .iter()
                .map(|l| if f.flip { l.iter().rev().map(|x| Fin { forward: !x.forward, ..*x }).collect() } else { l.clone() })
                .collect();
            if matches!(b.surfaces.get(f.surface), Some(Surface::Plane { .. })) && loops.len() > 1 {
                let outer = (0..loops.len()).max_by(|i, j| loop_size(b, &loops[*i]).total_cmp(&loop_size(b, &loops[*j]))).unwrap_or(0);
                loops.swap(0, outer);
            }
            let mut lps = Vec::with_capacity(loops.len());
            for l in &loops {
                let mut p = vec![l.len().to_string()];
                for fin in l {
                    p.extend([
                        "0".into(),
                        edge_list.to_string(),
                        (fin.edge + 1).to_string(),
                        u32::from(fin.forward).to_string(),
                        "0".into(),
                    ]);
                }
                lps.push(w.add(508, 1, p));
            }
            let mut p = vec![surface.to_string(), lps.len().to_string(), "1".into()];
            p.extend(lps.iter().map(|x| x.to_string()));
            faces.push((w.add(510, 1, p), !f.flip));
        }
        let shells: Vec<Vec<usize>> = if b.shells.is_empty() { vec![(0..b.faces.len()).collect()] } else { b.shells.clone() };
        let mut shell_ids = Vec::with_capacity(shells.len());
        for sh in &shells {
            let mut p = vec![sh.len().to_string()];
            for i in sh {
                let (face, agrees) = faces.get(*i).copied().ok_or_else(|| bad(format!("no face {i}")))?;
                p.extend([face.to_string(), u32::from(agrees).to_string()]);
            }
            shell_ids.push(w.add(514, if b.kind == BodyKind::Solid { 1 } else { 2 }, p));
        }
        if b.kind == BodyKind::Solid {
            let mut p = vec![shell_ids[0].to_string(), "1".into(), (shell_ids.len() - 1).to_string()];
            for v in &shell_ids[1..] {
                p.extend([v.to_string(), "0".into()]);
            }
            tops.push(w.add(186, 0, p));
        } else {
            tops.extend(shell_ids);
        }
    }
    for t in tops {
        if let Some(e) = w.entities.get_mut((t - 1) / 2) {
            e.top = true;
        }
    }
    file(&w)
}

fn file(w: &Writer) -> Result<String> {
    let mut out = String::new();
    let line = |out: &mut String, text: &str, section: char, n: usize| {
        let _ = writeln!(out, "{text:<72}{section}{n:>7}");
    };
    line(&mut out, "ShapeCraft IGES", 'S', 1);
    let max = w
        .entities
        .iter()
        .filter(|e| e.kind == 502 || e.kind == 116)
        .flat_map(|e| e.params.iter().filter_map(|p| p.parse::<f64>().ok()))
        .fold(1.0f64, |a, b| a.max(b.abs()));
    let global = [
        "1H,".to_string(),
        "1H;".to_string(),
        hollerith("ShapeCraft"),
        hollerith("model.igs"),
        hollerith("ShapeCraft"),
        hollerith("ShapeCraft"),
        "32".into(),
        "38".into(),
        "6".into(),
        "308".into(),
        "15".into(),
        hollerith("ShapeCraft"),
        "1.".into(),
        "2".into(),
        "2HMM".into(),
        "1".into(),
        "1.".into(),
        hollerith("20260101.000000"),
        "1.E-07".into(),
        real(max)?,
        String::new(),
        String::new(),
        "11".into(),
        "0".into(),
        hollerith("20260101.000000"),
    ];
    let g = format!("{};", global.join(","));
    let g_lines = wrap(&g, 72);
    for (i, l) in g_lines.iter().enumerate() {
        line(&mut out, l, 'G', i + 1);
    }
    // Parameter lines first, to know where each entity's start.
    let mut p_text = String::new();
    let mut p_at = Vec::with_capacity(w.entities.len());
    let mut pn = 0usize;
    for (k, e) in w.entities.iter().enumerate() {
        let body = format!("{},{};", e.kind, e.params.join(","));
        let chunks = wrap(&body, 64);
        p_at.push((pn + 1, chunks.len()));
        for c in chunks {
            pn += 1;
            let _ = writeln!(p_text, "{c:<64}{:>8}P{pn:>7}", 2 * k + 1);
        }
    }
    for (k, e) in w.entities.iter().enumerate() {
        let (pstart, pcount) = p_at[k];
        let status = if e.top { "00000000" } else { "00010001" };
        let d1 = format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}", e.kind, pstart, 0, 0, 0, 0, e.matrix, 0, status);
        let d2 = format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}", e.kind, 0, 0, pcount, e.form, "", "", "", 0);
        line(&mut out, &d1, 'D', 2 * k + 1);
        line(&mut out, &d2, 'D', 2 * k + 2);
    }
    out.push_str(&p_text);
    let t = format!("S{:>7}G{:>7}D{:>7}P{:>7}", 1, g_lines.len(), 2 * w.entities.len(), pn);
    line(&mut out, &t, 'T', 1);
    Ok(out)
}

/// Splits parameter text into lines of at most `width`, breaking after delimiters so no number is cut in two.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut token = String::new();
    let flush = |cur: &mut String, token: &mut String, lines: &mut Vec<String>| {
        if cur.len() + token.len() > width && !cur.is_empty() {
            lines.push(std::mem::take(cur));
        }
        while token.len() > width {
            let rest = token.split_off(width);
            lines.push(std::mem::replace(token, rest));
        }
        cur.push_str(token);
        token.clear();
    };
    for ch in text.chars() {
        token.push(ch);
        if ch == ',' || ch == ';' {
            flush(&mut cur, &mut token, &mut lines);
        }
    }
    flush(&mut cur, &mut token, &mut lines);
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

// ---------------------------------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Value {
    Num(f64),
    Text,
    Empty,
}

struct Dir {
    kind: u32,
    form: u32,
    params: Vec<Value>,
    matrix: usize,
}

struct Reader {
    dirs: HashMap<usize, Dir>,
    scale: f64,
}

/// Splits parameter text into values (numbers, Hollerith strings, empty fields).
fn tokens(text: &str, delim: char, end: char) -> Vec<Value> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut field = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == delim || c == end {
            let f = field.trim();
            out.push(if f.is_empty() {
                Value::Empty
            } else {
                f.replace(['D', 'd'], "E").parse::<f64>().map(Value::Num).unwrap_or(Value::Text)
            });
            field.clear();
            if c == end {
                break;
            }
            i += 1;
            continue;
        }
        if (c == 'H' || c == 'h') && !field.trim().is_empty() && field.trim().chars().all(|d| d.is_ascii_digit()) {
            let n: usize = field.trim().parse().unwrap_or(0);
            let s: String = chars.iter().skip(i + 1).take(n).collect();
            i += 1 + n;
            let _ = s;
            out.push(Value::Text);
            field.clear();
            // Past the string to its delimiter.
            while i < chars.len() && chars[i] != delim && chars[i] != end {
                i += 1;
            }
            if i < chars.len() && chars[i] == end {
                break;
            }
            i += 1;
            continue;
        }
        field.push(c);
        i += 1;
    }
    out
}

impl Reader {
    fn parse(text: &str) -> Result<Reader> {
        let (mut g, mut d, mut p) = (String::new(), Vec::new(), Vec::new());
        for raw in text.lines() {
            let l: Vec<char> = raw.chars().collect();
            if l.len() < 73 {
                continue;
            }
            let body: String = l[..72].iter().collect();
            match l[72] {
                'G' => g.push_str(&body),
                'D' => d.push(body),
                'P' => p.push(body),
                'C' => return Err(bad("compressed IGES files can't be read")),
                _ => {}
            }
        }
        if d.is_empty() {
            return Err(bad("not an IGES file (no directory entries)"));
        }
        let (delim, end) = {
            let g = g.trim_start();
            let delim = if g.starts_with("1H") { g.chars().nth(2).unwrap_or(',') } else { ',' };
            let rest = if g.starts_with("1H") { g.get(4..).unwrap_or("") } else { g.strip_prefix(',').unwrap_or(g) };
            let end = if rest.starts_with("1H") { rest.chars().nth(2).unwrap_or(';') } else { ';' };
            (delim, end)
        };
        let gv = tokens(&g, delim, end);
        let num = |i: usize| match gv.get(i) {
            Some(Value::Num(x)) => Some(*x),
            _ => None,
        };
        let unit = match num(13).map(|x| x as i64) {
            Some(1) => 25.4,
            Some(4) => 304.8,
            Some(5) => 1_609_344.0,
            Some(6) => 1000.0,
            Some(7) => 1_000_000.0,
            Some(8) => 0.0254,
            Some(9) => 0.001,
            Some(10) => 10.0,
            Some(11) => 0.0000254,
            _ => 1.0,
        };
        let scale = unit * num(12).filter(|s| s.is_finite() && *s > 0.0).map_or(1.0, |s| 1.0 / s);
        let field = |s: &str, k: usize| -> i64 { s.get(8 * k..8 * k + 8).map(str::trim).and_then(|x| x.parse().ok()).unwrap_or(0) };
        let mut dirs = HashMap::new();
        for (k, pair) in d.chunks(2).enumerate() {
            let [a, b] = pair else { continue };
            let kind = field(a, 0).max(0) as u32;
            let (pstart, count) = (field(a, 1).max(1) as usize, field(b, 3).max(1) as usize);
            let mut text = String::new();
            for l in p.iter().skip(pstart - 1).take(count) {
                text.push_str(l.get(..64).unwrap_or(l));
            }
            let mut params = tokens(&text, delim, end);
            if !params.is_empty() {
                params.remove(0);
            }
            dirs.insert(2 * k + 1, Dir { kind, form: field(b, 4).max(0) as u32, params, matrix: field(a, 6).max(0) as usize });
        }
        Ok(Reader { dirs, scale })
    }

    fn dir(&self, at: usize) -> Result<&Dir> {
        self.dirs.get(&at).ok_or_else(|| bad(format!("the IGES file points at a missing entity ({at})")))
    }

    fn num(d: &Dir, i: usize) -> Result<f64> {
        match d.params.get(i) {
            Some(Value::Num(x)) if x.is_finite() => Ok(*x),
            Some(Value::Empty) => Ok(0.0),
            _ => Err(bad(format!("entity {} is missing a number", d.kind))),
        }
    }

    fn int(d: &Dir, i: usize) -> Result<usize> {
        let x = Self::num(d, i)?;
        if x < 0.0 || x > 1e9 {
            return Err(bad(format!("entity {} has a count out of range", d.kind)));
        }
        Ok(x as usize)
    }

    /// The matrix (rotation part, translation) placing entity `d`, composed through any chain.
    fn placement(&self, d: &Dir) -> Result<(glam::DMat3, DVec3)> {
        let mut m = (glam::DMat3::IDENTITY, DVec3::ZERO);
        let mut at = d.matrix;
        let mut hops = 0;
        while at != 0 {
            hops += 1;
            if hops > 16 {
                return Err(bad("transformation matrices point round in a circle"));
            }
            let t = self.dir(at)?;
            let v: Vec<f64> = (0..12).map(|i| Self::num(t, i)).collect::<Result<_>>()?;
            let r = glam::DMat3::from_cols(DVec3::new(v[0], v[4], v[8]), DVec3::new(v[1], v[5], v[9]), DVec3::new(v[2], v[6], v[10]));
            let tr = DVec3::new(v[3], v[7], v[11]) * self.scale;
            m = (r * m.0, r * m.1 + tr);
            at = t.matrix;
        }
        Ok(m)
    }

    fn pt(&self, d: &Dir, i: usize) -> Result<DVec3> {
        Ok(DVec3::new(Self::num(d, i)?, Self::num(d, i + 1)?, Self::num(d, i + 2)?) * self.scale)
    }

    fn point_of(&self, at: usize) -> Result<DVec3> {
        let d = self.dir(at)?;
        let (r, t) = self.placement(d)?;
        Ok(r * self.pt(d, 0)? + t)
    }

    fn direction_of(&self, at: usize) -> Result<DVec3> {
        let d = self.dir(at)?;
        let (r, _) = self.placement(d)?;
        let v = DVec3::new(Self::num(d, 0)?, Self::num(d, 1)?, Self::num(d, 2)?);
        Ok((r * v).normalize_or_zero())
    }

    fn nurbs_curve(&self, d: &Dir) -> Result<Nurbs<DVec3>> {
        let k = Self::int(d, 0)?;
        let m = Self::int(d, 1)?;
        let polynomial = Self::num(d, 4)? != 0.0;
        let n = k + 1;
        let nk = n + m + 1;
        let base = 6;
        let knots: Vec<f64> = (0..nk).map(|i| Self::num(d, base + i)).collect::<Result<_>>()?;
        let weights: Vec<f64> = (0..n).map(|i| Self::num(d, base + nk + i)).collect::<Result<_>>()?;
        let pole_at = base + nk + n;
        let (r, t) = self.placement(d)?;
        let poles: Vec<DVec3> = (0..n).map(|i| self.pt(d, pole_at + 3 * i).map(|p| r * p + t)).collect::<Result<_>>()?;
        let (knots, mults) = runs(&knots);
        let rational = !polynomial && weights.iter().any(|w| (w - weights[0]).abs() > 1e-12);
        Ok(Nurbs { degree: m as u32, poles, weights: rational.then_some(weights), knots, mults, periodic: false })
    }

    /// An edge's curve as the neutral form has it.
    fn curve(&self, at: usize) -> Result<Curve> {
        let d = self.dir(at)?;
        let (r, t) = self.placement(d)?;
        Ok(match d.kind {
            110 => {
                let (a, b) = (r * self.pt(d, 0)? + t, r * self.pt(d, 3)? + t);
                Curve::Line { origin: a, dir: (b - a).normalize_or_zero() }
            }
            100 => {
                let z = Self::num(d, 0)? * self.scale;
                let c = DVec3::new(Self::num(d, 1)?, Self::num(d, 2)?, 0.0) * self.scale + DVec3::Z * z;
                let s = DVec3::new(Self::num(d, 3)?, Self::num(d, 4)?, 0.0) * self.scale + DVec3::Z * z;
                let radius = (s - c).length();
                let (x, y) = (r * DVec3::X, r * DVec3::Y);
                Curve::Circle { center: r * c + t, normal: x.cross(y).normalize_or_zero(), x: (r * (s - c)).normalize_or_zero(), radius }
            }
            126 => Curve::BSpline(self.nurbs_curve(d)?),
            other => return Err(bad(format!("edges on IGES curve type {other} can't be read yet"))),
        })
    }

    fn surface(&self, at: usize, b: &mut Brep) -> Result<Surface> {
        let d = self.dir(at)?;
        let ptr = |i: usize| Self::int(d, i);
        let refdir = |i: usize, axis: DVec3| -> Result<DVec3> {
            let x = match d.params.get(i) {
                Some(Value::Num(v)) if *v > 0.0 => self.direction_of(*v as usize)?,
                _ => axis.any_orthonormal_vector(),
            };
            Ok((x - axis * x.dot(axis)).normalize_or_zero())
        };
        Ok(match d.kind {
            190 => {
                let (o, n) = (self.point_of(ptr(0)?)?, self.direction_of(ptr(1)?)?);
                Surface::Plane { origin: o, normal: n, x: refdir(2, n)? }
            }
            192 => {
                let (o, a) = (self.point_of(ptr(0)?)?, self.direction_of(ptr(1)?)?);
                Surface::Cylinder { origin: o, axis: a, x: refdir(3, a)?, radius: Self::num(d, 2)? * self.scale }
            }
            194 => {
                let (o, a) = (self.point_of(ptr(0)?)?, self.direction_of(ptr(1)?)?);
                Surface::Cone {
                    origin: o,
                    axis: a,
                    x: refdir(4, a)?,
                    radius: Self::num(d, 2)? * self.scale,
                    half_angle: Self::num(d, 3)?.to_radians(),
                }
            }
            196 => {
                let c = self.point_of(ptr(0)?)?;
                let a = match d.params.get(2) {
                    Some(Value::Num(v)) if *v > 0.0 => self.direction_of(*v as usize)?,
                    _ => DVec3::Z,
                };
                Surface::Sphere { center: c, axis: a, x: refdir(3, a)?, radius: Self::num(d, 1)? * self.scale }
            }
            198 => {
                let (c, a) = (self.point_of(ptr(0)?)?, self.direction_of(ptr(1)?)?);
                Surface::Torus {
                    center: c,
                    axis: a,
                    x: refdir(4, a)?,
                    major: Self::num(d, 2)? * self.scale,
                    minor: Self::num(d, 3)? * self.scale,
                }
            }
            128 => {
                let (k1, k2, m1, m2) = (Self::int(d, 0)?, Self::int(d, 1)?, Self::int(d, 2)?, Self::int(d, 3)?);
                let polynomial = Self::num(d, 6)? != 0.0;
                let (nu, nv) = (k1 + 1, k2 + 1);
                let (nku, nkv) = (nu + m1 + 1, nv + m2 + 1);
                let mut i = 9;
                let ku: Vec<f64> = (0..nku).map(|k| Self::num(d, i + k)).collect::<Result<_>>()?;
                i += nku;
                let kv: Vec<f64> = (0..nkv).map(|k| Self::num(d, i + k)).collect::<Result<_>>()?;
                i += nkv;
                let w: Vec<f64> = (0..nu * nv).map(|k| Self::num(d, i + k)).collect::<Result<_>>()?;
                i += nu * nv;
                let (r, t) = self.placement(d)?;
                let mut poles = vec![DVec3::ZERO; nu * nv];
                let mut weights = vec![1.0; nu * nv];
                for jj in 0..nv {
                    for ii in 0..nu {
                        let k = jj * nu + ii;
                        poles[ii * nv + jj] = r * self.pt(d, i + 3 * k)? + t;
                        weights[ii * nv + jj] = w[k];
                    }
                }
                let rational = !polynomial && weights.iter().any(|x| (x - weights[0]).abs() > 1e-12);
                let (u_knots, u_mults) = runs(&ku);
                let (v_knots, v_mults) = runs(&kv);
                Surface::BSpline(NurbsSurface {
                    u_degree: m1 as u32,
                    v_degree: m2 as u32,
                    nu,
                    nv,
                    poles,
                    weights: rational.then_some(weights),
                    u_knots,
                    u_mults,
                    v_knots,
                    v_mults,
                    u_periodic: false,
                    v_periodic: false,
                })
            }
            120 => {
                let axis = self.dir(ptr(0)?)?;
                let (r, t) = self.placement(axis)?;
                let (a, z) = (r * self.pt(axis, 0)? + t, r * self.pt(axis, 3)? + t);
                let c = self.curve(ptr(1)?)?;
                b.curves.push(c);
                Surface::Revolution { curve: b.curves.len() - 1, origin: a, axis: (z - a).normalize_or_zero() }
            }
            122 => {
                let c = self.curve(ptr(0)?)?;
                let start = start_of(&c).ok_or_else(|| bad("a tabulated cylinder's curve has no start"))?;
                let end = self.pt(d, 1)?;
                b.curves.push(c);
                Surface::Extrusion { curve: b.curves.len() - 1, dir: end - start }
            }
            other => return Err(bad(format!("faces on IGES surface type {other} can't be read yet"))),
        })
    }

    /// One body from a shell list (the first closed, the rest voids for a solid).
    fn body(&self, shells: &[(usize, bool)], kind: BodyKind) -> Result<Brep> {
        let mut b = Brep { kind, ..Brep::default() };
        let mut vertices: HashMap<(usize, usize), usize> = HashMap::new();
        let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
        let mut surfaces: HashMap<usize, usize> = HashMap::new();
        for &(shell_at, _) in shells {
            let shell = self.dir(shell_at)?;
            if shell.kind != 514 {
                return Err(bad("a solid's shell isn't a shell"));
            }
            let n = Self::int(shell, 0)?;
            let mut face_ids = Vec::with_capacity(n);
            for k in 0..n {
                let face_at = Self::int(shell, 1 + 2 * k)?;
                let agrees = Self::num(shell, 2 + 2 * k)? != 0.0;
                let face = self.dir(face_at)?;
                if face.kind != 510 {
                    return Err(bad("a shell's face isn't a face"));
                }
                let surf_at = Self::int(face, 0)?;
                let surface = match surfaces.get(&surf_at) {
                    Some(s) => *s,
                    None => {
                        let s = self.surface(surf_at, &mut b)?;
                        b.surfaces.push(s);
                        surfaces.insert(surf_at, b.surfaces.len() - 1);
                        b.surfaces.len() - 1
                    }
                };
                let nl = Self::int(face, 1)?;
                let mut loops = Vec::with_capacity(nl);
                for l in 0..nl {
                    let lp = self.dir(Self::int(face, 3 + l)?)?;
                    if lp.kind != 508 {
                        return Err(bad("a face's loop isn't a loop"));
                    }
                    let ne = Self::int(lp, 0)?;
                    let mut fins = Vec::with_capacity(ne);
                    let mut i = 1;
                    for _ in 0..ne {
                        let kind = Self::int(lp, i)?;
                        let (list_at, idx, forward) = (Self::int(lp, i + 1)?, Self::int(lp, i + 2)?, Self::num(lp, i + 3)? != 0.0);
                        let pcurves = Self::int(lp, i + 4)?;
                        i += 5 + 2 * pcurves;
                        if kind != 0 {
                            continue; // A vertex in the loop (a degenerate edge): the neutral form leaves it out.
                        }
                        let edge = match edges.get(&(list_at, idx)) {
                            Some(e) => *e,
                            None => {
                                let list = self.dir(list_at)?;
                                let base = 1 + 5 * idx.checked_sub(1).ok_or_else(|| bad("an edge index of 0"))?;
                                let curve = self.curve(Self::int(list, base)?)?;
                                let mut vertex = |vl: usize, vi: usize, b: &mut Brep| -> Result<usize> {
                                    if let Some(v) = vertices.get(&(vl, vi)) {
                                        return Ok(*v);
                                    }
                                    let l = self.dir(vl)?;
                                    let p = self.pt(l, 1 + 3 * vi.checked_sub(1).ok_or_else(|| bad("a vertex index of 0"))?)?;
                                    let (r, t) = self.placement(l)?;
                                    b.vertices.push(r * p + t);
                                    vertices.insert((vl, vi), b.vertices.len() - 1);
                                    Ok(b.vertices.len() - 1)
                                };
                                let s = vertex(Self::int(list, base + 1)?, Self::int(list, base + 2)?, &mut b)?;
                                let e = vertex(Self::int(list, base + 3)?, Self::int(list, base + 4)?, &mut b)?;
                                // A line drawn from its end to its start runs the other way.
                                let reversed = match &curve {
                                    Curve::Line { origin, .. } => origin.distance(b.vertices[s]) > origin.distance(b.vertices[e]) + 1e-9,
                                    _ => false,
                                };
                                let curve = match curve {
                                    Curve::Line { dir, .. } if reversed => Curve::Line { origin: b.vertices[e], dir },
                                    Curve::Line { dir, .. } => Curve::Line { origin: b.vertices[s], dir },
                                    c => c,
                                };
                                b.curves.push(curve);
                                b.edges.push(Edge {
                                    curve: Some(b.curves.len() - 1),
                                    reversed,
                                    start: Some(s),
                                    end: Some(e),
                                    tolerance: None,
                                });
                                edges.insert((list_at, idx), b.edges.len() - 1);
                                b.edges.len() - 1
                            }
                        };
                        fins.push(Fin { edge, forward, pcurve: None });
                    }
                    // About the face's own normal.
                    if !agrees {
                        fins = fins.into_iter().rev().map(|x| Fin { forward: !x.forward, ..x }).collect();
                    }
                    if !fins.is_empty() {
                        loops.push(fins);
                    }
                }
                b.faces.push(Face { surface, flip: !agrees, loops, color: None });
                face_ids.push(b.faces.len() - 1);
            }
            b.shells.push(face_ids);
        }
        Ok(b)
    }
}

fn runs(full: &[f64]) -> (Vec<f64>, Vec<u32>) {
    let (mut knots, mut mults): (Vec<f64>, Vec<u32>) = (Vec::new(), Vec::new());
    for k in full {
        match knots.last() {
            Some(last) if (k - last).abs() <= 1e-12 * last.abs().max(1.0) => {
                if let Some(m) = mults.last_mut() {
                    *m += 1;
                }
            }
            _ => {
                knots.push(*k);
                mults.push(1);
            }
        }
    }
    (knots, mults)
}

/// Every B-rep solid (186) of an IGES file, in millimeters; shells (514) no solid uses come as sheets.
pub fn read(text: &str) -> Result<Vec<Brep>> {
    let r = Reader::parse(text)?;
    let mut keys: Vec<usize> = r.dirs.keys().copied().collect();
    keys.sort_unstable();
    let mut used_shells = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &k in &keys {
        let d = &r.dirs[&k];
        if d.kind != 186 {
            continue;
        }
        let mut shells = vec![(Reader::int(d, 0)?, true)];
        let voids = Reader::int(d, 2).unwrap_or(0);
        for v in 0..voids {
            shells.push((Reader::int(d, 3 + 2 * v)?, false));
        }
        used_shells.extend(shells.iter().map(|s| s.0));
        out.push(r.body(&shells, BodyKind::Solid)?);
    }
    for &k in &keys {
        let d = &r.dirs[&k];
        if d.kind == 514 && !used_shells.contains(&k) {
            out.push(r.body(&[(k, true)], if d.form == 1 { BodyKind::Solid } else { BodyKind::Sheet })?);
        }
    }
    if out.is_empty() {
        let trimmed = keys.iter().any(|k| matches!(r.dirs[k].kind, 144 | 143));
        return Err(bad(if trimmed {
            "this IGES file holds loose trimmed surfaces; the Rust kernel reads IGES solids (B-rep), the Open CASCADE kernel reads both"
        } else {
            "the IGES file has no solids in it"
        }));
    }
    Ok(out)
}
