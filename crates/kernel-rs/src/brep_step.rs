//! ShapeCraft's kernel-neutral [`Brep`] as STEP AP214 text, which Arris's STEP reader then builds: it fits the
//! pcurves, checks the topology and heals small gaps, so a body read from Parasolid, .shapr, NX, Solid Edge or JT
//! takes the same road into Arris as one read from a STEP file.
//!
//! One body a file. Each face is written as an `ADVANCED_FACE` whose number [`Written::faces`] keeps, so the caller
//! can follow Arris's provenance from a face back to the neutral face it came from.

use std::collections::HashMap;
use std::fmt::Write as _;

use glam::DVec3;
use shapecraft_kernel::brep::{BodyKind, Brep, Curve, Nurbs, NurbsSurface, Surface};
use shapecraft_kernel::{KernelError, Result};

/// The STEP text and, per neutral face, the `#id` of its `ADVANCED_FACE`.
pub struct Written {
    pub text: String,
    pub faces: Vec<u64>,
}

fn bad(message: impl Into<String>) -> KernelError {
    KernelError::Operation { op: "from_brep", message: message.into() }
}

/// A real as STEP writes it: always with a point, never NaN.
fn real(x: f64) -> Result<String> {
    if !x.is_finite() {
        return Err(bad("a number in the body isn't finite"));
    }
    let s = format!("{x:?}");
    Ok(if s.contains(['.', 'e', 'E']) { s } else { format!("{s}.") })
}

fn reals(xs: impl IntoIterator<Item = f64>) -> Result<String> {
    Ok(format!("({})", xs.into_iter().map(real).collect::<Result<Vec<_>>>()?.join(",")))
}

fn refs(ids: impl IntoIterator<Item = u64>) -> String {
    format!("({})", ids.into_iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(","))
}

/// `x` made square to `z` (any square direction when it lies along it).
fn square_to(z: DVec3, x: DVec3) -> DVec3 {
    let z = z.normalize_or_zero();
    let x = x - z * x.dot(z);
    if x.length() > 1e-12 { x.normalize() } else { z.any_orthonormal_vector() }
}

/// Distinct knots and multiplicities as STEP lists, after checking they fit the pole count.
fn knot_lists(knots: &[f64], mults: &[u32], poles: usize, degree: u32) -> Result<(String, String)> {
    let total: u64 = mults.iter().map(|m| u64::from(*m)).sum();
    if knots.len() != mults.len() || total != poles as u64 + u64::from(degree) + 1 {
        return Err(bad("a periodic or malformed spline (its knots don't fit its poles)"));
    }
    Ok((format!("({})", mults.iter().map(u32::to_string).collect::<Vec<_>>().join(",")), reals(knots.iter().copied())?))
}

/// Each edge's start and end vertex, those the body leaves out filled in from its loops: where one fin ends the next
/// begins. Both stay `None` only for a closed edge with no vertex (a whole circle).
fn edge_ends(brep: &Brep) -> Vec<(Option<usize>, Option<usize>)> {
    let mut ends: Vec<(Option<usize>, Option<usize>)> = brep.edges.iter().map(|e| (e.start, e.end)).collect();
    // A fin's (head, tail) slots in `ends`: an edge walked backwards starts at its end.
    let slots = |edge: usize, forward: bool| if forward { ((edge, 0), (edge, 1)) } else { ((edge, 1), (edge, 0)) };
    let get = |ends: &[(Option<usize>, Option<usize>)], (e, k): (usize, usize)| ends.get(e).and_then(|v| if k == 0 { v.0 } else { v.1 });
    let set = |ends: &mut [(Option<usize>, Option<usize>)], (e, k): (usize, usize), v| {
        if let Some(x) = ends.get_mut(e) {
            if k == 0 { x.0 = Some(v) } else { x.1 = Some(v) }
        }
    };
    // Each pass fills a gap from a known neighbour; a few passes settle any body.
    for _ in 0..4 {
        let mut changed = false;
        for l in brep.faces.iter().flat_map(|f| &f.loops) {
            for (i, fin) in l.iter().enumerate() {
                let next = &l[(i + 1) % l.len()];
                let (_, tail) = slots(fin.edge, fin.forward);
                let (head, _) = slots(next.edge, next.forward);
                match (get(&ends, tail), get(&ends, head)) {
                    (Some(v), None) => {
                        set(&mut ends, head, v);
                        changed = true;
                    }
                    (None, Some(v)) => {
                        set(&mut ends, tail, v);
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
    ends
}

/// Which faces of a solid to walk the other way round so that every edge is walked once each way: a body whose
/// faces disagree (seen in Shapr3D files: a symmetric extrude's mirrored half walked backwards) is made to agree,
/// keeping the side whose loops enclose a positive volume. Nothing turns for a body that already agrees, or one no
/// turning can fix.
fn turned_faces(brep: &Brep, ends: &[(Option<usize>, Option<usize>)]) -> Vec<bool> {
    let n = brep.faces.len();
    let keep = vec![false; n];
    if brep.kind != BodyKind::Solid {
        return keep;
    }
    let mut uses: HashMap<usize, Vec<(usize, bool)>> = HashMap::new();
    for (f, face) in brep.faces.iter().enumerate() {
        for fin in face.loops.iter().flatten() {
            uses.entry(fin.edge).or_default().push((f, fin.forward));
        }
    }
    if !uses.values().any(|u| matches!(u.as_slice(), [(fa, a), (fb, b)] if fa != fb && a == b)) {
        return keep;
    }
    // Faces joined by an edge walked the same way must differ in turning; walked opposite ways, match.
    let mut links: Vec<Vec<(usize, bool)>> = vec![Vec::new(); n];
    for u in uses.values() {
        if let [(fa, a), (fb, b)] = u.as_slice()
            && fa != fb
        {
            links[*fa].push((*fb, a == b));
            links[*fb].push((*fa, a == b));
        }
    }
    let mut turn: Vec<Option<bool>> = vec![None; n];
    for seed in 0..n {
        if turn[seed].is_some() {
            continue;
        }
        turn[seed] = Some(false);
        let mut todo = vec![seed];
        while let Some(f) = todo.pop() {
            let t = turn[f].unwrap_or(false);
            for &(g, differ) in &links[f] {
                match turn[g] {
                    None => {
                        turn[g] = Some(t != differ);
                        todo.push(g);
                    }
                    Some(u) if u != (t != differ) => return keep,
                    Some(_) => {}
                }
            }
        }
    }
    let turn: Vec<bool> = turn.into_iter().map(|t| t.unwrap_or(false)).collect();
    // The volume the loops enclose (their vertices fanned to the origin), turned faces walked backwards.
    let mut volume = 0.0;
    for (f, face) in brep.faces.iter().enumerate() {
        for l in &face.loops {
            let pts: Vec<DVec3> = l
                .iter()
                .filter_map(|fin| {
                    let (a, b) = ends.get(fin.edge).copied().unwrap_or_default();
                    if fin.forward { a } else { b }
                })
                .filter_map(|v| brep.vertices.get(v).copied())
                .collect();
            let sense = if turn[f] { -1.0 } else { 1.0 };
            for i in 1..pts.len().saturating_sub(1) {
                volume += sense * pts[0].dot(pts[i].cross(pts[i + 1])) / 6.0;
            }
        }
    }
    if volume < 0.0 { turn.into_iter().map(|t| !t).collect() } else { turn }
}

struct Writer<'a> {
    brep: &'a Brep,
    ends: Vec<(Option<usize>, Option<usize>)>,
    lines: Vec<String>,
    surfaces: HashMap<usize, u64>,
    curves: HashMap<usize, u64>,
    vertices: HashMap<usize, u64>,
}

impl Writer<'_> {
    fn push(&mut self, entity: String) -> u64 {
        self.lines.push(entity);
        self.lines.len() as u64
    }

    fn point(&mut self, p: DVec3) -> Result<u64> {
        let text = format!("CARTESIAN_POINT('',{})", reals(p.to_array())?);
        Ok(self.push(text))
    }

    fn direction(&mut self, d: DVec3) -> Result<u64> {
        let d = d.normalize_or_zero();
        if d == DVec3::ZERO {
            return Err(bad("a zero direction"));
        }
        let text = format!("DIRECTION('',{})", reals(d.to_array())?);
        Ok(self.push(text))
    }

    fn placement(&mut self, origin: DVec3, z: DVec3, x: DVec3) -> Result<u64> {
        let (o, zd, xd) = (self.point(origin)?, self.direction(z)?, self.direction(square_to(z, x))?);
        Ok(self.push(format!("AXIS2_PLACEMENT_3D('',#{o},#{zd},#{xd})")))
    }

    fn axis1(&mut self, origin: DVec3, dir: DVec3) -> Result<u64> {
        let (o, d) = (self.point(origin)?, self.direction(dir)?);
        Ok(self.push(format!("AXIS1_PLACEMENT('',#{o},#{d})")))
    }

    fn vector(&mut self, dir: DVec3) -> Result<u64> {
        let d = self.direction(dir)?;
        Ok(self.push(format!("VECTOR('',#{d},1.)")))
    }

    fn nurbs_curve(&mut self, n: &Nurbs<DVec3>) -> Result<String> {
        let (mults, knots) = knot_lists(&n.knots, &n.mults, n.poles.len(), n.degree)?;
        let poles = n.poles.iter().map(|p| self.point(*p)).collect::<Result<Vec<_>>>()?;
        let (deg, list) = (n.degree, refs(poles));
        Ok(match &n.weights {
            Some(w) if w.len() == n.poles.len() => format!(
                "( BOUNDED_CURVE() B_SPLINE_CURVE({deg},{list},.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS({mults},{knots},.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE({}) REPRESENTATION_ITEM('') )",
                reals(w.iter().copied())?
            ),
            Some(_) => return Err(bad("a spline's weights don't match its poles")),
            None => format!("B_SPLINE_CURVE_WITH_KNOTS('',{deg},{list},.UNSPECIFIED.,.F.,.F.,{mults},{knots},.UNSPECIFIED.)"),
        })
    }

    fn nurbs_surface(&mut self, n: &NurbsSurface) -> Result<String> {
        if n.nu * n.nv != n.poles.len() || n.nu < 2 || n.nv < 2 {
            return Err(bad("a spline surface's poles don't match its size"));
        }
        let (um, uk) = knot_lists(&n.u_knots, &n.u_mults, n.nu, n.u_degree)?;
        let (vm, vk) = knot_lists(&n.v_knots, &n.v_mults, n.nv, n.v_degree)?;
        let mut rows = Vec::with_capacity(n.nu);
        for row in n.poles.chunks(n.nv) {
            let ids = row.iter().map(|p| self.point(*p)).collect::<Result<Vec<_>>>()?;
            rows.push(refs(ids));
        }
        let (p, q, net) = (n.u_degree, n.v_degree, format!("({})", rows.join(",")));
        Ok(match &n.weights {
            Some(w) if w.len() == n.poles.len() => {
                let weights = w.chunks(n.nv).map(|r| reals(r.iter().copied())).collect::<Result<Vec<_>>>()?.join(",");
                format!(
                    "( BOUNDED_SURFACE() B_SPLINE_SURFACE({p},{q},{net},.UNSPECIFIED.,.F.,.F.,.F.) B_SPLINE_SURFACE_WITH_KNOTS({um},{vm},{uk},{vk},.UNSPECIFIED.) GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_SURFACE(({weights})) REPRESENTATION_ITEM('') SURFACE() )"
                )
            }
            Some(_) => return Err(bad("a spline surface's weights don't match its poles")),
            None => format!("B_SPLINE_SURFACE_WITH_KNOTS('',{p},{q},{net},.UNSPECIFIED.,.F.,.F.,.F.,{um},{vm},{uk},{vk},.UNSPECIFIED.)"),
        })
    }

    fn curve(&mut self, i: usize) -> Result<u64> {
        if let Some(id) = self.curves.get(&i) {
            return Ok(*id);
        }
        let c = self.brep.curves.get(i).ok_or_else(|| bad(format!("no curve {i}")))?;
        let text = match c {
            Curve::Line { origin, dir } => {
                let (p, v) = (self.point(*origin)?, self.vector(*dir)?);
                format!("LINE('',#{p},#{v})")
            }
            Curve::Circle { center, normal, x, radius } => {
                let ax = self.placement(*center, *normal, *x)?;
                format!("CIRCLE('',#{ax},{})", real(*radius)?)
            }
            Curve::Ellipse { center, normal, x, major, minor } => {
                // STEP wants the first semi-axis the longer: turn the frame a quarter when it isn't.
                let (x, a, b) = if major >= minor { (*x, *major, *minor) } else { (normal.cross(*x), *minor, *major) };
                let ax = self.placement(*center, *normal, x)?;
                format!("ELLIPSE('',#{ax},{},{})", real(a)?, real(b)?)
            }
            Curve::BSpline(n) => self.nurbs_curve(n)?,
            Curve::Through(points) => {
                let fit = shapecraft_kernel::spline::fit(points).ok_or_else(|| bad("an intersection curve that can't be fitted"))?;
                let (knots, mults) = fit.knot_runs();
                let n = Nurbs {
                    degree: fit.degree as u32,
                    poles: fit.poles,
                    weights: None,
                    knots,
                    mults: mults.iter().map(|m| *m as u32).collect(),
                    periodic: false,
                };
                self.nurbs_curve(&n)?
            }
            Curve::OnSurface { .. } => return Err(bad("an edge has only a curve on a surface")),
        };
        let id = self.push(text);
        self.curves.insert(i, id);
        Ok(id)
    }

    fn surface(&mut self, i: usize) -> Result<u64> {
        if let Some(id) = self.surfaces.get(&i) {
            return Ok(*id);
        }
        let s = self.brep.surfaces.get(i).ok_or_else(|| bad(format!("no surface {i}")))?;
        let text = match s {
            Surface::Plane { origin, normal, x } => format!("PLANE('',#{})", self.placement(*origin, *normal, *x)?),
            Surface::Cylinder { origin, axis, x, radius } => {
                format!("CYLINDRICAL_SURFACE('',#{},{})", self.placement(*origin, *axis, *x)?, real(*radius)?)
            }
            Surface::Cone { origin, axis, x, radius, half_angle } => {
                // STEP's semi-angle is positive: a cone narrowing along its axis is the same surface (and the same
                // outward normal) about the reversed axis.
                let (axis, angle) = if *half_angle < 0.0 { (-*axis, -half_angle) } else { (*axis, *half_angle) };
                format!("CONICAL_SURFACE('',#{},{},{})", self.placement(*origin, axis, *x)?, real(*radius)?, real(angle)?)
            }
            Surface::Sphere { center, axis, x, radius } => {
                format!("SPHERICAL_SURFACE('',#{},{})", self.placement(*center, *axis, *x)?, real(*radius)?)
            }
            Surface::Torus { center, axis, x, major, minor } if *major < 0.0 => {
                // The lemon part of a spindle torus (Parasolid's signed major radius): its generating circle spun
                // about the axis keeps the same (u, v), as the OCCT kernel builds it.
                let y = axis.cross(*x);
                let ax = self.placement(*center + square_to(*axis, *x) * *major, -y, *x)?;
                let circle = self.push(format!("CIRCLE('',#{ax},{})", real(*minor)?));
                format!("SURFACE_OF_REVOLUTION('',#{circle},#{})", self.axis1(*center, *axis)?)
            }
            Surface::Torus { center, axis, x, major, minor } => {
                format!("TOROIDAL_SURFACE('',#{},{},{})", self.placement(*center, *axis, *x)?, real(*major)?, real(*minor)?)
            }
            Surface::BSpline(n) => self.nurbs_surface(n)?,
            Surface::Offset { basis, distance } => {
                if *basis == i {
                    return Err(bad("an offset surface of itself"));
                }
                format!("OFFSET_SURFACE('',#{},{},.F.)", self.surface(*basis)?, real(*distance)?)
            }
            Surface::Revolution { curve, origin, axis } => {
                format!("SURFACE_OF_REVOLUTION('',#{},#{})", self.curve(*curve)?, self.axis1(*origin, *axis)?)
            }
            Surface::Extrusion { curve, dir } => {
                format!("SURFACE_OF_LINEAR_EXTRUSION('',#{},#{})", self.curve(*curve)?, self.vector(*dir)?)
            }
        };
        let id = self.push(text);
        self.surfaces.insert(i, id);
        Ok(id)
    }

    fn vertex(&mut self, i: usize) -> Result<u64> {
        if let Some(id) = self.vertices.get(&i) {
            return Ok(*id);
        }
        let p = *self.brep.vertices.get(i).ok_or_else(|| bad(format!("no vertex {i}")))?;
        let id = self.vertex_at(p)?;
        self.vertices.insert(i, id);
        Ok(id)
    }

    fn vertex_at(&mut self, p: DVec3) -> Result<u64> {
        let pt = self.point(p)?;
        Ok(self.push(format!("VERTEX_POINT('',#{pt})")))
    }

    /// The `EDGE_CURVE` of edge `i`, or `None` for a point-like edge (no curve, one vertex: a cone's apex).
    fn edge(&mut self, i: usize, written: &mut HashMap<usize, Option<u64>>) -> Result<Option<u64>> {
        if let Some(id) = written.get(&i) {
            return Ok(*id);
        }
        let e = self.brep.edges.get(i).ok_or_else(|| bad(format!("no edge {i}")))?;
        let (first, last) = self.ends.get(i).copied().unwrap_or_default();
        let id = match e.curve {
            None if first == last => None,
            None => return Err(bad("an edge without a curve")),
            Some(c) => {
                let geometry = self.curve(c)?;
                let (start, end) = match (first, last) {
                    (Some(s), Some(t)) => (self.vertex(s)?, self.vertex(t)?),
                    // A closed edge with no vertex (a whole circle): one where its curve starts.
                    _ => {
                        let at = start_of(&self.brep.curves[c]).ok_or_else(|| bad("a closed edge with no start"))?;
                        let v = self.vertex_at(at)?;
                        (v, v)
                    }
                };
                let sense = if e.reversed { ".F." } else { ".T." };
                Some(self.push(format!("EDGE_CURVE('',#{start},#{end},#{geometry},{sense})")))
            }
        };
        written.insert(i, id);
        Ok(id)
    }
}

/// Where a closed curve starts (its parameter 0).
fn start_of(c: &Curve) -> Option<DVec3> {
    match c {
        Curve::Circle { center, normal, x, radius } => Some(*center + square_to(*normal, *x) * *radius),
        Curve::Ellipse { center, normal, x, major, .. } => Some(*center + square_to(*normal, *x) * *major),
        Curve::BSpline(n) => n.poles.first().copied(),
        Curve::Through(p) => p.first().copied(),
        Curve::Line { origin, .. } => Some(*origin),
        Curve::OnSurface { .. } => None,
    }
}

/// The body as a STEP file of one part.
pub fn write(brep: &Brep) -> Result<Written> {
    if brep.kind == BodyKind::Wire {
        return Err(bad("a wire body has no faces"));
    }
    let mut w = Writer {
        brep,
        ends: edge_ends(brep),
        lines: Vec::new(),
        surfaces: HashMap::new(),
        curves: HashMap::new(),
        vertices: HashMap::new(),
    };
    let mut edges = HashMap::new();
    let mut faces = Vec::with_capacity(brep.faces.len());
    let turned = turned_faces(brep, &w.ends);
    for (fi, f) in brep.faces.iter().enumerate() {
        let surface = w.surface(f.surface)?;
        let mut bounds = Vec::new();
        for l in &f.loops {
            let mut oriented = Vec::new();
            let mut point = None;
            let walk: Vec<_> = if turned[fi] { l.iter().rev().collect() } else { l.iter().collect() };
            for fin in walk {
                match w.edge(fin.edge, &mut edges)? {
                    Some(e) => {
                        let sense = if fin.forward != turned[fi] { ".T." } else { ".F." };
                        oriented.push(w.push(format!("ORIENTED_EDGE('',*,*,#{e},{sense})")));
                    }
                    None => point = w.ends.get(fin.edge).and_then(|e| e.0),
                }
            }
            let bound = if !oriented.is_empty() {
                w.push(format!("EDGE_LOOP('',{})", refs(oriented)))
            } else if let Some(v) = point {
                let v = w.vertex(v)?;
                w.push(format!("VERTEX_LOOP('',#{v})"))
            } else {
                continue;
            };
            bounds.push(w.push(format!("FACE_BOUND('',#{bound},.T.)")));
        }
        let sense = if f.flip { ".F." } else { ".T." };
        faces.push(w.push(format!("ADVANCED_FACE('',{},#{surface},{sense})", refs(bounds))));
    }
    let shells: Vec<&Vec<usize>> = if brep.shells.is_empty() { Vec::new() } else { brep.shells.iter().collect() };
    let all: Vec<usize> = (0..brep.faces.len()).collect();
    let shells = if shells.is_empty() { vec![&all] } else { shells };
    let face_ids = |s: &Vec<usize>| -> Result<Vec<u64>> {
        s.iter().map(|i| faces.get(*i).copied().ok_or_else(|| bad(format!("no face {i}")))).collect()
    };
    let item = if brep.kind == BodyKind::Sheet {
        let mut open = Vec::new();
        for s in &shells {
            let ids = face_ids(s)?;
            open.push(w.push(format!("OPEN_SHELL('',{})", refs(ids))));
        }
        w.push(format!("SHELL_BASED_SURFACE_MODEL('',{})", refs(open)))
    } else {
        let mut closed = Vec::new();
        for s in &shells {
            let ids = face_ids(s)?;
            closed.push(w.push(format!("CLOSED_SHELL('',{})", refs(ids))));
        }
        match closed.as_slice() {
            [outer] => w.push(format!("MANIFOLD_SOLID_BREP('',#{outer})")),
            [outer, voids @ ..] => {
                let voids: Vec<u64> = voids.iter().map(|v| w.push(format!("ORIENTED_CLOSED_SHELL('',*,#{v},.F.)"))).collect();
                w.push(format!("BREP_WITH_VOIDS('',#{outer},{})", refs(voids)))
            }
            [] => return Err(bad("a body with no faces")),
        }
    };
    let text = file(&mut w, item, brep.kind == BodyKind::Sheet)?;
    Ok(Written { text, faces })
}

/// The product, units and representation around the body's `item`, and the file's header.
fn file(w: &mut Writer<'_>, item: u64, sheet: bool) -> Result<String> {
    let app = w.push("APPLICATION_CONTEXT('core data for automotive mechanical design processes')".into());
    w.push(format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"));
    let pc = w.push(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let product = w.push(format!("PRODUCT('body','body','',(#{pc}))"));
    let formation = w.push(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let dc = w.push(format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
    let definition = w.push(format!("PRODUCT_DEFINITION('design','',#{formation},#{dc})"));
    let shape = w.push(format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));
    let length = w.push("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )".into());
    let angle = w.push("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )".into());
    let solid_angle = w.push("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )".into());
    let accuracy =
        w.push(format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#{length},'distance_accuracy_value','confusion accuracy')"));
    let context = w.push(format!(
        "( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{accuracy})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{length},#{angle},#{solid_angle})) REPRESENTATION_CONTEXT('Context #1','3D Context with UNIT and UNCERTAINTY') )"
    ));
    let world = w.placement(DVec3::ZERO, DVec3::Z, DVec3::X)?;
    let kind = if sheet { "MANIFOLD_SURFACE_SHAPE_REPRESENTATION" } else { "ADVANCED_BREP_SHAPE_REPRESENTATION" };
    let rep = w.push(format!("{kind}('',(#{world},#{item}),#{context})"));
    w.push(format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{rep})"));
    let mut out = String::with_capacity(w.lines.iter().map(|l| l.len() + 12).sum::<usize>() + 256);
    out.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ShapeCraft body'),'2;1');\n");
    out.push_str("FILE_NAME('','',(''),(''),'ShapeCraft','ShapeCraft','');\n");
    out.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n");
    for (i, l) in w.lines.iter().enumerate() {
        let _ = writeln!(out, "#{} = {l};", i + 1);
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(out)
}
