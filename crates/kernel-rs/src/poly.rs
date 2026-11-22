//! Mesh bodies (STL, OBJ, 3MF imports): polyhedra the exact kernel holds as plain triangles. Also the first,
//! flat-faced Rust kernel; kept for meshes, which have no exact surfaces to hand Arris.

use std::path::Path;

use shapecraft_kernel::{
    Aabb, Axis, BoolOp, DMat3, DVec3, Distance, EdgeCurve, FacePlane, Kernel, KernelError, MassProps, Mesh, PlanarCurve, PlaneFrame,
    Result, Shape, ShapeData, SubShape, ViewFrame, ViewLine,
};

/// Polyhedra only.
#[derive(Debug, Default, Clone, Copy)]
pub struct PolyKernel;

/// A polyhedron: corner points and faces, each one loop of corner indices counter-clockwise seen from outside
/// (a sheet's faces: counter-clockwise seen from where its normal points).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Poly {
    pub points: Vec<DVec3>,
    pub faces: Vec<Vec<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Solid,
    /// Faces that don't close up: a sketch region, one face of a body, an open mesh.
    Sheet,
}

/// What this kernel's [`Shape`]s hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RsShape {
    pub parts: Vec<(Kind, Poly)>,
}

impl ShapeData for RsShape {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn not_yet(op: &'static str) -> KernelError {
    KernelError::Operation { op, message: "not in the Rust kernel yet (it has flat-faced solids so far)".into() }
}

fn bad(message: impl Into<String>) -> KernelError {
    KernelError::InvalidInput(message.into())
}

fn data(shape: &Shape) -> Result<&RsShape> {
    shape.downcast::<RsShape>()
}

fn wrap(parts: Vec<(Kind, Poly)>) -> Shape {
    Shape::new(RsShape { parts })
}

/// Newell's normal of a loop (its length is twice the loop's area).
fn newell(points: &[DVec3]) -> DVec3 {
    let mut n = DVec3::ZERO;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n
}

impl Poly {
    fn corners(&self, face: &[u32]) -> Vec<DVec3> {
        face.iter().map(|&i| self.points[i as usize]).collect()
    }

    /// Triangles of each face (ear clipping in the face's plane), as corner indices.
    fn triangles(&self, face: &[u32]) -> Vec<[u32; 3]> {
        let pts = self.corners(face);
        let n = newell(&pts);
        if n.length() < 1e-300 || pts.len() < 3 {
            return Vec::new();
        }
        let n = n.normalize();
        let u = n.any_orthonormal_vector();
        let v = n.cross(u);
        let flat: Vec<(f64, f64)> = pts.iter().map(|p| (p.dot(u), p.dot(v))).collect();
        ear_clip(&flat).into_iter().map(|[a, b, c]| [face[a], face[b], face[c]]).collect()
    }

    /// Area-weighted centroid and area of a face.
    fn face_area(&self, face: &[u32]) -> (f64, DVec3) {
        let (mut area, mut moment) = (0.0, DVec3::ZERO);
        for [a, b, c] in self.triangles(face) {
            let (a, b, c) = (self.points[a as usize], self.points[b as usize], self.points[c as usize]);
            let t = 0.5 * (b - a).cross(c - a).length();
            area += t;
            moment += t * (a + b + c) / 3.0;
        }
        (area, if area > 0.0 { moment / area } else { self.corners(face).iter().sum::<DVec3>() / face.len().max(1) as f64 })
    }

    /// Each edge once (lower corner index first), in the order the faces first reach it.
    fn edges(&self) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in &self.faces {
            for k in 0..f.len() {
                let (a, b) = (f[k], f[(k + 1) % f.len()]);
                let e = (a.min(b), a.max(b));
                if seen.insert(e) {
                    out.push(e);
                }
            }
        }
        out
    }

    fn map(&self, f: impl Fn(DVec3) -> DVec3, flip: bool) -> Poly {
        let faces = if flip { self.faces.iter().map(|l| l.iter().rev().copied().collect()).collect() } else { self.faces.clone() };
        Poly { points: self.points.iter().map(|&p| f(p)).collect(), faces }
    }
}

/// Triangles of a simple polygon given counter-clockwise or clockwise in 2D, as indices into it.
fn ear_clip(pts: &[(f64, f64)]) -> Vec<[usize; 3]> {
    let area2: f64 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    let ccw = area2 > 0.0;
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    if !ccw {
        idx.reverse();
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut out = Vec::with_capacity(pts.len().saturating_sub(2));
    let mut guard = 0;
    while idx.len() > 3 && guard < pts.len() * pts.len() {
        guard += 1;
        let n = idx.len();
        let ear = (0..n).find(|&i| {
            let (p, c, x) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            if cross(pts[p], pts[c], pts[x]) <= 1e-18 {
                return false;
            }
            // No other corner inside the triangle.
            idx.iter().all(|&q| {
                q == p || q == c || q == x || {
                    let (d1, d2, d3) = (cross(pts[p], pts[c], pts[q]), cross(pts[c], pts[x], pts[q]), cross(pts[x], pts[p], pts[q]));
                    !(d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0)
                }
            })
        });
        let i = ear.unwrap_or(0);
        let (p, c, x) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
        out.push(if ccw { [p, c, x] } else { [x, c, p] });
        idx.remove(i);
    }
    if idx.len() == 3 {
        out.push(if ccw { [idx[0], idx[1], idx[2]] } else { [idx[2], idx[1], idx[0]] });
    }
    out
}

/// A solid from a closed planar polygon pushed along `dir`.
fn prism_of(points: &[DVec3], dir: DVec3) -> Result<Poly> {
    let n = points.len();
    if n < 3 {
        return Err(bad("a polygon needs at least three corners"));
    }
    let normal = newell(points);
    if normal.length() < 1e-12 {
        return Err(bad("the polygon has no area"));
    }
    let along = normal.dot(dir);
    if along.abs() < 1e-12 * dir.length().max(1.0) * normal.length() {
        return Err(bad("the direction lies in the polygon's plane"));
    }
    // The bottom faces away from `dir`: its loop runs clockwise seen along `dir`.
    let base: Vec<DVec3> = if along > 0.0 { points.iter().rev().copied().collect() } else { points.to_vec() };
    let mut pts = base.clone();
    pts.extend(base.iter().map(|&p| p + dir));
    let n32 = n as u32;
    let mut faces = vec![(0..n32).collect::<Vec<_>>(), (0..n32).rev().map(|i| i + n32).collect()];
    for i in 0..n32 {
        let j = (i + 1) % n32;
        faces.push(vec![j, i, i + n32, j + n32]);
    }
    Ok(Poly { points: pts, faces })
}

/// Closed loops of line segments (end to end within `tol`), each as its corner points; None when a segment is left
/// dangling or a corner joins more than two.
fn line_loops(segments: &[(DVec3, DVec3)], tol: f64) -> Option<Vec<Vec<DVec3>>> {
    let mut left: Vec<(DVec3, DVec3)> = segments.to_vec();
    let mut loops = Vec::new();
    while let Some((a, b)) = left.pop() {
        let mut pts = vec![a];
        let mut end = b;
        loop {
            if end.distance(pts[0]) <= tol {
                break;
            }
            pts.push(end);
            let next = left.iter().position(|(p, q)| p.distance(end) <= tol || q.distance(end) <= tol)?;
            let (p, q) = left.swap_remove(next);
            end = if p.distance(end) <= tol { q } else { p };
        }
        if pts.len() < 3 {
            return None;
        }
        loops.push(pts);
    }
    Some(loops)
}

fn inside_2d(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1) {
            inside = !inside;
        }
    }
    inside
}

impl Kernel for PolyKernel {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn make_box(&self, min: DVec3, size: DVec3) -> Result<Shape> {
        if size.min_element() <= 0.0 {
            return Err(bad("a box needs a positive size"));
        }
        let base = [min, min + DVec3::new(size.x, 0.0, 0.0), min + DVec3::new(size.x, size.y, 0.0), min + DVec3::new(0.0, size.y, 0.0)];
        Ok(wrap(vec![(Kind::Solid, prism_of(&base, DVec3::new(0.0, 0.0, size.z))?)]))
    }

    fn extrude_polygon(&self, points: &[DVec3], dir: DVec3) -> Result<Shape> {
        Ok(wrap(vec![(Kind::Solid, prism_of(points, dir)?)]))
    }

    fn extrude_circle(&self, _center: DVec3, _radius: f64, _dir: DVec3) -> Result<Shape> {
        Err(not_yet("extrude a circle"))
    }

    fn boolean(&self, _op: BoolOp, _a: &Shape, _b: &Shape) -> Result<Shape> {
        Err(not_yet("boolean"))
    }

    fn fillet_all_edges(&self, _shape: &Shape, _radius: f64) -> Result<Shape> {
        Err(not_yet("fillet"))
    }

    fn mass_props(&self, shape: &Shape) -> Result<MassProps> {
        let (mut volume, mut area, mut moment, mut area_moment) = (0.0, 0.0, DVec3::ZERO, DVec3::ZERO);
        for (kind, p) in &data(shape)?.parts {
            for f in &p.faces {
                let (a, c) = p.face_area(f);
                area += a;
                area_moment += a * c;
                if *kind == Kind::Solid {
                    // Signed tetrahedra from the origin.
                    for [i, j, k] in p.triangles(f) {
                        let (a, b, c) = (p.points[i as usize], p.points[j as usize], p.points[k as usize]);
                        let v = a.dot(b.cross(c)) / 6.0;
                        volume += v;
                        moment += v * (a + b + c) / 4.0;
                    }
                }
            }
        }
        let centroid = if volume.abs() > 1e-300 {
            moment / volume
        } else if area > 0.0 {
            area_moment / area
        } else {
            DVec3::ZERO
        };
        Ok(MassProps { volume, area, centroid })
    }

    fn bounds(&self, shape: &Shape) -> Result<Aabb> {
        let mut pts = data(shape)?.parts.iter().flat_map(|(_, p)| p.points.iter().copied());
        let first = pts.next().ok_or_else(|| bad("an empty shape has no bounds"))?;
        let (min, max) = pts.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
        Ok(Aabb { min, max })
    }

    fn tessellate(&self, shape: &Shape, _deflection: f64) -> Result<Mesh> {
        let mut mesh = Mesh::default();
        let f32s = |p: DVec3| [p.x as f32, p.y as f32, p.z as f32];
        let (mut face_base, mut edge_base) = (0u32, 0u32);
        for (_, p) in &data(shape)?.parts {
            for (fi, f) in p.faces.iter().enumerate() {
                let n = newell(&p.corners(f)).normalize_or_zero();
                for [a, b, c] in p.triangles(f) {
                    for i in [a, b, c] {
                        mesh.indices.push(mesh.positions.len() as u32);
                        mesh.positions.push(f32s(p.points[i as usize]));
                        mesh.normals.push(f32s(n));
                    }
                    mesh.triangle_faces.push(face_base + fi as u32);
                }
            }
            for (ei, (a, b)) in p.edges().into_iter().enumerate() {
                mesh.edges.push(vec![f32s(p.points[a as usize]), f32s(p.points[b as usize])]);
                mesh.edge_ids.push(edge_base + ei as u32);
                let mut faces = [u32::MAX; 2];
                let mut k = 0;
                for (fi, f) in p.faces.iter().enumerate() {
                    let has = (0..f.len()).any(|j| {
                        let (x, y) = (f[j], f[(j + 1) % f.len()]);
                        (x.min(y), x.max(y)) == (a, b)
                    });
                    if has && k < 2 {
                        faces[k] = face_base + fi as u32;
                        k += 1;
                    }
                }
                mesh.edge_faces.push(faces);
            }
            face_base += p.faces.len() as u32;
            edge_base += p.edges().len() as u32;
        }
        Ok(mesh)
    }

    fn face_count(&self, shape: &Shape) -> Result<u32> {
        Ok(data(shape)?.parts.iter().map(|(_, p)| p.faces.len() as u32).sum())
    }

    fn face_plane(&self, shape: &Shape, face: u32) -> Result<Option<FacePlane>> {
        let (p, f) = find_face(data(shape)?, face)?;
        let (_, origin) = p.face_area(f);
        Ok(Some(FacePlane { origin, normal: newell(&p.corners(f)).normalize_or_zero() }))
    }

    fn extrude_face(&self, _shape: &Shape, _face: u32, _distance: f64) -> Result<Shape> {
        Err(not_yet("push or pull a face"))
    }

    fn fillet_edges(&self, _shape: &Shape, _edges: &[u32], _radius: f64) -> Result<Shape> {
        Err(not_yet("fillet"))
    }

    fn fillet_edges_variable(&self, _shape: &Shape, _edges: &[u32], _r1: f64, _r2: f64) -> Result<Shape> {
        Err(not_yet("fillet"))
    }

    fn translate(&self, shape: &Shape, by: DVec3) -> Result<Shape> {
        map_shape(shape, |p| p + by, false)
    }

    fn planar_regions(&self, frame: PlaneFrame, curves: &[PlanarCurve]) -> Result<Vec<Shape>> {
        let mut segments = Vec::new();
        for c in curves {
            match c {
                PlanarCurve::Line { a, b } => segments.push((*a, *b)),
                _ => return Err(not_yet("sketch regions with curves")),
            }
        }
        let size = segments.iter().map(|(a, b)| a.length().max(b.length())).fold(1.0, f64::max);
        let loops = line_loops(&segments, 1e-9 * size).ok_or_else(|| not_yet("sketch regions where lines cross or branch"))?;
        let normal = frame.x.cross(frame.y).normalize_or_zero();
        let flat = |l: &[DVec3]| l.iter().map(|p| ((*p - frame.origin).dot(frame.x), (*p - frame.origin).dot(frame.y))).collect::<Vec<_>>();
        let flats: Vec<Vec<(f64, f64)>> = loops.iter().map(|l| flat(l)).collect();
        for (i, a) in flats.iter().enumerate() {
            if flats.iter().enumerate().any(|(j, b)| i != j && inside_2d(a[0], b)) {
                return Err(not_yet("sketch regions with holes"));
            }
        }
        Ok(loops
            .into_iter()
            .map(|mut l| {
                if newell(&l).dot(normal) < 0.0 {
                    l.reverse();
                }
                let faces = vec![(0..l.len() as u32).collect()];
                wrap(vec![(Kind::Sheet, Poly { points: l, faces })])
            })
            .collect())
    }

    fn measure(&self, shape: &Shape, sub: SubShape) -> Result<(f64, DVec3)> {
        let d = data(shape)?;
        match sub {
            SubShape::Whole => {
                let mp = self.mass_props(shape)?;
                let solid = d.parts.iter().any(|(k, _)| *k == Kind::Solid);
                Ok((if solid { mp.volume } else { mp.area }, mp.centroid))
            }
            SubShape::Face(i) => {
                let (p, f) = find_face(d, i)?;
                Ok(p.face_area(f))
            }
            SubShape::Edge(i) => {
                let (a, b) = find_edge(d, i)?;
                Ok((a.distance(b), (a + b) / 2.0))
            }
        }
    }

    fn distance(&self, _a: &Shape, _sa: SubShape, _b: &Shape, _sb: SubShape) -> Result<Distance> {
        Err(not_yet("distance"))
    }

    fn prism(&self, face: &Shape, dir: DVec3) -> Result<Shape> {
        let mut parts = Vec::new();
        for (_, p) in &data(face)?.parts {
            for f in &p.faces {
                parts.push((Kind::Solid, prism_of(&p.corners(f), dir)?));
            }
        }
        if parts.is_empty() {
            return Err(bad("nothing to extrude"));
        }
        Ok(wrap(parts))
    }

    fn prism_draft(&self, face: &Shape, dir: DVec3, angle: f64) -> Result<Shape> {
        if angle.abs() > 1e-12 {
            return Err(not_yet("draft angles"));
        }
        self.prism(face, dir)
    }

    fn revolve(&self, _face: &Shape, _axis: Axis, _angle: f64) -> Result<Shape> {
        Err(not_yet("revolve"))
    }

    fn shell(&self, _shape: &Shape, _open_faces: &[u32], _thickness: f64) -> Result<Shape> {
        Err(not_yet("shell"))
    }

    fn chamfer_edges(&self, _shape: &Shape, _edges: &[u32], _distance: f64) -> Result<Shape> {
        Err(not_yet("chamfer"))
    }

    fn revolve_polygon(&self, _points: &[DVec3], _axis: Axis, _angle: f64) -> Result<Shape> {
        Err(not_yet("revolve"))
    }

    fn revolve_circle(&self, _center: DVec3, _normal: DVec3, _radius: f64, _axis: Axis, _angle: f64) -> Result<Shape> {
        Err(not_yet("revolve"))
    }

    fn rotate(&self, shape: &Shape, axis: Axis, angle: f64) -> Result<Shape> {
        let dir = axis.dir.try_normalize().ok_or_else(|| bad("the axis has no direction"))?;
        let m = DMat3::from_axis_angle(dir, angle);
        map_shape(shape, |p| axis.origin + m * (p - axis.origin), false)
    }

    fn mirror(&self, shape: &Shape, origin: DVec3, normal: DVec3) -> Result<Shape> {
        let n = normal.try_normalize().ok_or_else(|| bad("the mirror plane has no normal"))?;
        map_shape(shape, |p| p - 2.0 * (p - origin).dot(n) * n, true)
    }

    fn scale(&self, shape: &Shape, center: DVec3, factors: DVec3) -> Result<Shape> {
        if factors.min_element() <= 0.0 {
            return Err(bad("scale factors must be positive"));
        }
        map_shape(shape, |p| center + (p - center) * factors, false)
    }

    fn offset_faces(&self, _shape: &Shape, _faces: &[u32], _distance: f64) -> Result<Shape> {
        Err(not_yet("offset faces"))
    }

    fn split_by_plane(&self, _shape: &Shape, _origin: DVec3, _normal: DVec3) -> Result<Vec<Shape>> {
        Err(not_yet("split"))
    }

    fn solids(&self, shape: &Shape) -> Result<Vec<Shape>> {
        Ok(data(shape)?.parts.iter().filter(|(k, _)| *k == Kind::Solid).map(|part| wrap(vec![part.clone()])).collect())
    }

    fn axis_of(&self, _shape: &Shape, _sub: SubShape) -> Result<Option<Axis>> {
        // Flat faces and straight edges have no axis of their own.
        Ok(None)
    }

    fn blend_of(&self, _shape: &Shape, _face: u32) -> Result<Option<(f64, bool)>> {
        Ok(None)
    }

    fn edge_count(&self, shape: &Shape) -> Result<u32> {
        Ok(data(shape)?.parts.iter().map(|(_, p)| p.edges().len() as u32).sum())
    }

    fn hidden_lines(&self, _shapes: &[Shape], _view: ViewFrame, _deflection: f64) -> Result<Vec<ViewLine>> {
        Err(not_yet("drawings"))
    }

    fn offset_curves(&self, _frame: PlaneFrame, _curves: &[PlanarCurve], _distance: f64) -> Result<Shape> {
        Err(not_yet("offset curves"))
    }

    fn offset_face_outline(&self, _face: &Shape, _distance: f64) -> Result<Shape> {
        Err(not_yet("offset"))
    }

    fn edge_curve(&self, shape: &Shape, edge: u32) -> Result<EdgeCurve> {
        let (a, b) = find_edge(data(shape)?, edge)?;
        Ok(EdgeCurve::Line { a, b })
    }

    fn face_edges(&self, shape: &Shape, face: u32) -> Result<Vec<u32>> {
        let d = data(shape)?;
        let (mut face_base, mut edge_base) = (0u32, 0u32);
        for (_, p) in &d.parts {
            let edges = p.edges();
            if face < face_base + p.faces.len() as u32 {
                let f = &p.faces[(face - face_base) as usize];
                return Ok((0..f.len())
                    .filter_map(|k| {
                        let (a, b) = (f[k], f[(k + 1) % f.len()]);
                        edges.iter().position(|e| *e == (a.min(b), a.max(b))).map(|i| edge_base + i as u32)
                    })
                    .collect());
            }
            face_base += p.faces.len() as u32;
            edge_base += edges.len() as u32;
        }
        Err(bad(format!("no face {face}")))
    }

    fn face(&self, shape: &Shape, face: u32) -> Result<Shape> {
        let (p, f) = find_face(data(shape)?, face)?;
        let points = p.corners(f);
        let faces = vec![(0..points.len() as u32).collect()];
        Ok(wrap(vec![(Kind::Sheet, Poly { points, faces })]))
    }

    fn imprint(&self, _shape: &Shape, _face: u32, _tools: &[(Shape, Option<u32>)]) -> Result<Shape> {
        Err(not_yet("imprint"))
    }

    fn loft(&self, _sections: &[Shape]) -> Result<Shape> {
        Err(not_yet("loft"))
    }

    fn sweep(&self, _profile: &Shape, _frame: PlaneFrame, _path: &[PlanarCurve]) -> Result<Shape> {
        Err(not_yet("sweep"))
    }

    fn write_step(&self, _shapes: &[Shape], _path: &Path) -> Result<()> {
        Err(not_yet("STEP files"))
    }

    fn write_step_ap(&self, _shapes: &[Shape], _path: &Path, _protocol: u32) -> Result<()> {
        Err(not_yet("STEP files"))
    }

    fn read_step(&self, _path: &Path) -> Result<Vec<Shape>> {
        Err(not_yet("STEP files"))
    }

    fn write_iges(&self, _shapes: &[Shape], _path: &Path) -> Result<()> {
        Err(not_yet("IGES files"))
    }

    fn read_iges(&self, _path: &Path) -> Result<Vec<Shape>> {
        Err(not_yet("IGES files"))
    }

    fn empty(&self) -> Result<Shape> {
        Ok(wrap(Vec::new()))
    }

    fn shape_from_mesh(&self, positions: &[DVec3], indices: &[u32]) -> Result<Shape> {
        if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= positions.len()) {
            return Err(bad("the mesh's triangles don't index its points"));
        }
        let faces: Vec<Vec<u32>> = indices.chunks(3).map(|t| t.to_vec()).collect();
        let poly = Poly { points: positions.to_vec(), faces };
        // Closed when every edge borders exactly two triangles.
        let mut uses = std::collections::HashMap::new();
        for f in &poly.faces {
            for k in 0..3 {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        let kind = if !uses.is_empty() && uses.values().all(|&n| n == 2) { Kind::Solid } else { Kind::Sheet };
        Ok(wrap(vec![(kind, poly)]))
    }
}

fn map_shape(shape: &Shape, f: impl Fn(DVec3) -> DVec3, flip: bool) -> Result<Shape> {
    Ok(wrap(data(shape)?.parts.iter().map(|(k, p)| (*k, p.map(&f, flip))).collect()))
}

fn find_face(d: &RsShape, face: u32) -> Result<(&Poly, &[u32])> {
    let mut base = 0u32;
    for (_, p) in &d.parts {
        if face < base + p.faces.len() as u32 {
            return Ok((p, &p.faces[(face - base) as usize]));
        }
        base += p.faces.len() as u32;
    }
    Err(bad(format!("no face {face}")))
}

fn find_edge(d: &RsShape, edge: u32) -> Result<(DVec3, DVec3)> {
    let mut base = 0u32;
    for (_, p) in &d.parts {
        let edges = p.edges();
        if edge < base + edges.len() as u32 {
            let (a, b) = edges[(edge - base) as usize];
            return Ok((p.points[a as usize], p.points[b as usize]));
        }
        base += edges.len() as u32;
    }
    Err(bad(format!("no edge {edge}")))
}
