//! Arris bodies as ShapeCraft's kernel-neutral [`brep::Brep`] (what the .shapr and Parasolid writers take).

use std::collections::HashMap;

use arris::geom::{Curve, Surface};
use arris::math::Frame;
use arris::topo::{Body, Model, Orientation};
use shapecraft_kernel::brep::{self, Brep, Fin, Nurbs, NurbsSurface};
use shapecraft_kernel::{DVec3, KernelError, Result};

fn d(p: arris::math::Point3) -> DVec3 {
    DVec3::new(p.x, p.y, p.z)
}

fn axes(f: &Frame) -> (DVec3, DVec3, DVec3) {
    let v = |u: arris::math::UnitVec3| DVec3::new(u.x, u.y, u.z);
    (d(f.origin()), v(f.z()), v(f.x()))
}

/// A full knot vector as distinct knots and their multiplicities.
fn runs(knots: &[f64]) -> (Vec<f64>, Vec<u32>) {
    let (mut k, mut m): (Vec<f64>, Vec<u32>) = (Vec::new(), Vec::new());
    for &x in knots {
        match k.last() {
            Some(&last) if (x - last).abs() <= 1e-14 * x.abs().max(1.0) => *m.last_mut().unwrap_or(&mut 0) += 1,
            _ => {
                k.push(x);
                m.push(1);
            }
        }
    }
    (k, m)
}

fn rational(w: &[f64]) -> Option<Vec<f64>> {
    w.iter().any(|x| (x - 1.0).abs() > 1e-15).then(|| w.to_vec())
}

fn surface(s: &Surface) -> brep::Surface {
    match s {
        Surface::Plane { frame } => {
            let (origin, normal, x) = axes(frame);
            brep::Surface::Plane { origin, normal, x }
        }
        Surface::Cylinder { frame, radius } => {
            let (origin, axis, x) = axes(frame);
            brep::Surface::Cylinder { origin, axis, x, radius: *radius }
        }
        Surface::Cone { frame, radius, half_angle } => {
            let (origin, axis, x) = axes(frame);
            brep::Surface::Cone { origin, axis, x, radius: *radius, half_angle: *half_angle }
        }
        Surface::Sphere { frame, radius } => {
            let (center, axis, x) = axes(frame);
            brep::Surface::Sphere { center, axis, x, radius: *radius }
        }
        Surface::Torus { frame, major_radius, minor_radius } => {
            let (center, axis, x) = axes(frame);
            brep::Surface::Torus { center, axis, x, major: *major_radius, minor: *minor_radius }
        }
        Surface::Nurbs(n) => {
            let [p, q] = n.degree();
            let [ku, kv] = n.knots();
            let [nu, nv] = n.counts();
            let (u_knots, u_mults) = runs(ku);
            let (v_knots, v_mults) = runs(kv);
            brep::Surface::BSpline(NurbsSurface {
                u_degree: p as u32,
                v_degree: q as u32,
                nu,
                nv,
                poles: n.control_points().iter().map(|p| d(*p)).collect(),
                weights: rational(n.weights()),
                u_knots,
                u_mults,
                v_knots,
                v_mults,
                u_periodic: false,
                v_periodic: false,
            })
        }
        // No kind of its own in the neutral form (an exact B-spline isn't written yet): `body` refuses these faces.
        Surface::EllipticCylinder { .. } => brep::Surface::Plane { origin: DVec3::ZERO, normal: DVec3::Z, x: DVec3::X },
    }
}

fn curve(c: &Curve) -> brep::Curve {
    match c {
        Curve::Line { origin, direction } => {
            brep::Curve::Line { origin: d(*origin), dir: DVec3::new(direction.x, direction.y, direction.z) }
        }
        Curve::Circle { frame, radius } => {
            let (center, normal, x) = axes(frame);
            brep::Curve::Circle { center, normal, x, radius: *radius }
        }
        Curve::Ellipse { frame, major_radius, minor_radius } => {
            let (center, normal, x) = axes(frame);
            brep::Curve::Ellipse { center, normal, x, major: *major_radius, minor: *minor_radius }
        }
        Curve::Nurbs(n) => {
            let (knots, mults) = runs(n.knots());
            brep::Curve::BSpline(Nurbs {
                degree: n.degree() as u32,
                poles: n.control_points().iter().map(|p| d(*p)).collect(),
                weights: rational(n.weights()),
                knots,
                mults,
                periodic: false,
            })
        }
    }
}

/// One body as a [`Brep`].
pub fn body(m: &Model, b: Body, solid: bool) -> Result<Brep> {
    let err = |e: &dyn std::fmt::Display| KernelError::Operation { op: "to_brep", message: e.to_string() };
    let mut out = Brep { kind: if solid { brep::BodyKind::Solid } else { brep::BodyKind::Sheet }, ..Default::default() };
    let (mut surfaces, mut curves, mut verts, mut edges) = (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
    for shell in m.shells(b).map_err(|e| err(&e))? {
        let mut faces = Vec::new();
        let entity = m.shell(shell.id).map_err(|e| err(&e))?;
        for f in entity.faces() {
            let face = m.face(f.id).map_err(|e| err(&e))?;
            let sid = *surfaces.entry(face.surface()).or_insert_with(|| {
                out.surfaces.push(m.surface(face.surface()).map(surface).unwrap_or(brep::Surface::Plane {
                    origin: DVec3::ZERO,
                    normal: DVec3::Z,
                    x: DVec3::X,
                }));
                out.surfaces.len() - 1
            });
            if let Ok(Surface::EllipticCylinder { .. }) = m.surface(face.surface()) {
                return Err(err(&"an elliptic-cylinder face can't be written to this format yet"));
            }
            let mut loops = Vec::new();
            for l in face.loops() {
                let mut fins = Vec::new();
                for c in l.coedges() {
                    let eid = c.edge();
                    let index = match edges.get(&eid) {
                        Some(i) => *i,
                        None => {
                            let e = m.edge(eid).map_err(|x| err(&x))?;
                            let mut vertex = |v| -> Result<usize> {
                                if let Some(i) = verts.get(&v) {
                                    return Ok(*i);
                                }
                                out.vertices.push(d(m.vertex(v).map_err(|x| err(&x))?.point()));
                                verts.insert(v, out.vertices.len() - 1);
                                Ok(out.vertices.len() - 1)
                            };
                            let (s, t) = (vertex(e.start())?, vertex(e.end())?);
                            let (curve_index, reversed) = match e.curve() {
                                Some((cid, range)) => {
                                    let ci = *curves.entry(cid).or_insert_with(|| {
                                        out.curves.push(m.curve(cid).map(curve).unwrap_or(brep::Curve::Through(Vec::new())));
                                        out.curves.len() - 1
                                    });
                                    let start = m.curve(cid).map(|c| d(c.point(range.lo()))).unwrap_or_default();
                                    (Some(ci), start.distance(out.vertices[s]) > start.distance(out.vertices[t]) + 1e-9)
                                }
                                None => (None, false),
                            };
                            out.edges.push(brep::Edge {
                                curve: curve_index,
                                reversed,
                                start: Some(s),
                                end: Some(t),
                                tolerance: Some(e.tolerance()),
                            });
                            edges.insert(eid, out.edges.len() - 1);
                            out.edges.len() - 1
                        }
                    };
                    fins.push(Fin { edge: index, forward: c.orientation() == Orientation::Forward, pcurve: None });
                }
                loops.push(fins);
            }
            out.faces.push(brep::Face { surface: sid, flip: f.orientation == Orientation::Reversed, loops, color: None });
            faces.push(out.faces.len() - 1);
        }
        out.shells.push(faces);
    }
    Ok(out)
}
