//! Hidden-line removal for drawing views, on the bodies' display triangles.
//!
//! What OCCT's `HLRBRep` draws, drawn the same way: sharp edges (not seams, not the tangent edges where a fillet meets
//! its neighbours) and the silhouettes of curved faces, each split into the stretches in front of everything and the
//! stretches behind something (dashed). A silhouette is traced where a face's triangles turn from facing the viewer
//! to facing away. A stretch is hidden when a ray from its middle toward the viewer meets a triangle first.

use std::collections::HashMap;

use arris::topo::FaceId;
use glam::{DVec2, DVec3};
use shapecraft_kernel::{Result, ViewFrame, ViewLine};

use crate::{ArShape, C, edge_faces, edge_points, edges_of, faces_of, normal_at_edge};

/// One display triangle, the face it belongs to (shape, face), and whether that face is curved (its triangles then
/// stand off the exact surface by up to the chord).
struct Tri {
    p: [DVec3; 3],
    face: (usize, FaceId),
    curved: bool,
}

/// A line to draw, in 3D, and the faces it lies on (a ray leaving it may graze them).
struct Line {
    points: Vec<DVec3>,
    faces: Vec<(usize, FaceId)>,
}

/// The view's axes: right, up, toward the viewer.
struct View {
    origin: DVec3,
    x: DVec3,
    y: DVec3,
    toward: DVec3,
}

impl View {
    fn of(v: &ViewFrame) -> Option<View> {
        let toward = v.toward.normalize_or_zero();
        let x = (v.x - toward * v.x.dot(toward)).normalize_or_zero();
        (toward != DVec3::ZERO && x != DVec3::ZERO).then(|| View { origin: v.origin, x, y: toward.cross(x), toward })
    }

    fn flat(&self, p: DVec3) -> DVec2 {
        let d = p - self.origin;
        DVec2::new(d.dot(self.x), d.dot(self.y))
    }
}

/// A grid over the view of which triangles cover each cell, for the visibility rays.
struct Grid {
    lo: DVec2,
    cell: f64,
    n: usize,
    cells: Vec<Vec<usize>>,
}

impl Grid {
    fn new(view: &View, tris: &[Tri]) -> Grid {
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for t in tris {
            for p in t.p {
                let q = view.flat(p);
                lo = lo.min(q);
                hi = hi.max(q);
            }
        }
        let n = ((tris.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
        let cell = ((hi - lo).max_element() / n as f64).max(1e-9);
        let mut cells = vec![Vec::new(); n * n];
        let at = |v: f64| ((v / cell).floor().max(0.0) as usize).min(n - 1);
        for (i, t) in tris.iter().enumerate() {
            let q = t.p.map(|p| view.flat(p) - lo);
            let (a, b) = (q[0].min(q[1]).min(q[2]), q[0].max(q[1]).max(q[2]));
            for gy in at(a.y)..=at(b.y) {
                for gx in at(a.x)..=at(b.x) {
                    cells[gy * n + gx].push(i);
                }
            }
        }
        Grid { lo, cell, n, cells }
    }

    fn near(&self, q: DVec2) -> &[usize] {
        let d = (q - self.lo) / self.cell;
        if d.x < 0.0 || d.y < 0.0 || d.x >= self.n as f64 || d.y >= self.n as f64 {
            return &[];
        }
        &self.cells[d.y as usize * self.n + d.x as usize]
    }
}

/// How far along `dir` from `from` the ray meets the triangle, if it does (Möller–Trumbore).
fn hit(from: DVec3, dir: DVec3, t: &Tri) -> Option<f64> {
    let (e1, e2) = (t.p[1] - t.p[0], t.p[2] - t.p[0]);
    let h = dir.cross(e2);
    let a = e1.dot(h);
    if a.abs() < 1e-14 * e1.length() * e2.length() {
        return None;
    }
    let s = from - t.p[0];
    let u = s.dot(h) / a;
    let q = s.cross(e1);
    let v = dir.dot(q) / a;
    let eps = 1e-9;
    if u < -eps || v < -eps || u + v > 1.0 + eps {
        return None;
    }
    Some(e2.dot(q) / a)
}

/// The display triangles and the lines a view draws, of every shape.
fn gather(shapes: &[&ArShape], chord: f64) -> Result<(Vec<Tri>, Vec<Line>)> {
    let mut tris = Vec::new();
    let mut lines = Vec::new();
    for (k, s) in shapes.iter().enumerate() {
        let bordering = edge_faces(s);
        let faces = faces_of(s);
        for b in &s.bodies {
            let tm = arris::mesh::tessellate(&s.model, *b, chord, C)
                .map_err(|e| shapecraft_kernel::KernelError::Operation { op: "drawings", message: e.to_string() })?;
            let pos: Vec<DVec3> = tm.positions().iter().map(|p| DVec3::from_array(*p)).collect();
            // Sharp edges: not seams (one face both sides), not where two faces meet tangentially.
            for (_, e) in edges_of(s).into_iter().filter(|(eb, _)| eb == b) {
                let on: Vec<(usize, FaceId)> =
                    bordering.get(&e.id).into_iter().flatten().filter_map(|&f| faces.get(f as usize)).map(|(_, f)| (k, f.id)).collect();
                let seam = on.len() == 2 && on[0].1 == on[1].1;
                let smooth = on.len() == 2 && {
                    let n = |i: usize| faces.iter().find(|(_, f)| f.id == on[i].1).and_then(|(_, f)| normal_at_edge(&s.model, *f, e.id));
                    matches!((n(0), n(1)), (Some(a), Some(b)) if a.dot(b) > 1.0 - 1e-6)
                };
                if seam || smooth {
                    continue;
                }
                lines.push(Line { points: edge_points(&s.model, e, None), faces: on });
            }
            // Triangles, each with its face (silhouettes are traced from them).
            for fr in tm.faces() {
                let curved = !matches!(
                    s.model.face(fr.face).ok().and_then(|f| s.model.surface(f.surface()).ok()),
                    Some(arris::geom::Surface::Plane { .. })
                );
                for t in &tm.triangles()[fr.triangles.clone()] {
                    tris.push(Tri { p: t.map(|i| pos[i as usize]), face: (k, fr.face), curved });
                }
            }
        }
    }
    Ok((tris, lines))
}

/// A point rounded to a nanometre, to match the corners triangles share.
type Key = [i64; 3];

/// One side of a display triangle: the triangle and the side's two corners.
struct Side {
    tri: usize,
    a: DVec3,
    b: DVec3,
}

/// The silhouette segments of each face as seen along `toward`, chained into polylines.
fn silhouettes(tris: &[Tri], toward: DVec3) -> Vec<Line> {
    // Inner edges of each face: the two triangles on either side, matched by their corner points.
    let key = |p: DVec3| -> Key { p.to_array().map(|c| (c * 1e9).round() as i64) };
    // Each triangle side (by its two corners) and the triangles along it.
    let mut sides: HashMap<(Key, Key), Vec<Side>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t.p[k], t.p[(k + 1) % 3]);
            let (ka, kb) = (key(a), key(b));
            let id = if ka < kb { (ka, kb) } else { (kb, ka) };
            sides.entry(id).or_default().push(Side { tri: i, a, b });
        }
    }
    // Edge-on counts as facing the viewer, so a strip seen exactly edge-on still has a side the silhouette runs by.
    let facing = |t: &Tri| (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).dot(toward) >= 0.0;
    let mut segs: Vec<(DVec3, DVec3, (usize, FaceId))> = Vec::new();
    for v in sides.values() {
        if let [x, y] = &v[..]
            && tris[x.tri].face == tris[y.tri].face
            && facing(&tris[x.tri]) != facing(&tris[y.tri])
        {
            segs.push((x.a, x.b, tris[x.tri].face));
        }
    }
    // Chain segments end to end.
    let mut out = Vec::new();
    while let Some((a, b, face)) = segs.pop() {
        let mut pts = vec![a, b];
        loop {
            let end = *pts.last().unwrap_or(&b);
            let Some(k) = segs.iter().position(|s| s.2 == face && (key(s.0) == key(end) || key(s.1) == key(end))) else { break };
            let s = segs.swap_remove(k);
            pts.push(if key(s.0) == key(end) { s.1 } else { s.0 });
        }
        loop {
            let start = pts[0];
            let Some(k) = segs.iter().position(|s| s.2 == face && (key(s.0) == key(start) || key(s.1) == key(start))) else { break };
            let s = segs.swap_remove(k);
            pts.insert(0, if key(s.0) == key(start) { s.1 } else { s.0 });
        }
        out.push(Line { points: pts, faces: vec![face] });
    }
    out
}

/// The visible and hidden stretches of every sharp edge and silhouette of the shapes, in view-plane millimetres.
pub fn hidden_lines(shapes: &[&ArShape], frame: &ViewFrame, deflection: f64) -> Result<Vec<ViewLine>> {
    let Some(view) = View::of(frame) else { return Err(crate::bad("bad view direction")) };
    let chord = deflection.clamp(1e-4, 10.0);
    let (tris, mut lines) = gather(shapes, chord)?;
    lines.extend(silhouettes(&tris, view.toward));
    let grid = Grid::new(&view, &tris);
    let size = tris.iter().flat_map(|t| t.p).fold(0.0f64, |m, p| m.max(p.abs().max_element())).max(1.0);
    // A ray from a line may graze the faces it lies on: a hit on them nearer than this doesn't hide it. Lines are on
    // the exact surfaces, whose triangles stand off them by up to the chord.
    let near = 1e-7 * size;
    let graze = (chord * size).sqrt() * 2.0;
    let step = size / 400.0;
    let hidden = |p: DVec3, line: &Line| {
        grid.near(view.flat(p)).iter().any(|&i| {
            let t = &tris[i];
            let own = t.curved && line.faces.contains(&t.face);
            hit(p, view.toward, t).is_some_and(|d| d > if own { graze } else { near })
        })
    };
    let mut out = Vec::new();
    for line in &lines {
        // Each stretch between the curve's points is tested in pieces short enough to be all in front or all
        // behind; a run keeps the curve's own points and breaks only where that changes.
        let mut run: Vec<DVec2> = line.points.first().map(|p| vec![view.flat(*p)]).unwrap_or_default();
        let mut run_hidden = None;
        for w in line.points.windows(2) {
            let pieces = ((w[0].distance(w[1]) / step).ceil() as usize).clamp(1, 64);
            for k in 0..pieces {
                let (a, b) = (w[0].lerp(w[1], k as f64 / pieces as f64), w[0].lerp(w[1], (k + 1) as f64 / pieces as f64));
                let h = hidden(a.lerp(b, 0.5), line);
                if run_hidden.is_some_and(|r| r != h) {
                    let at = view.flat(a);
                    if run.last() != Some(&at) {
                        run.push(at);
                    }
                    out.push(ViewLine { points: std::mem::replace(&mut run, vec![at]), hidden: run_hidden == Some(true) });
                }
                run_hidden = Some(h);
            }
            run.push(view.flat(w[1]));
        }
        if run.len() >= 2 {
            out.push(ViewLine { points: run, hidden: run_hidden == Some(true) });
        }
    }
    Ok(out)
}
