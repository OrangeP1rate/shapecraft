//! Taking faces out of a body and closing it over them, for the case that needs no surface extended: the faces'
//! whole rim is made of holes in the faces kept (a hole, pocket, slot or boss on a face). Each of those holes is
//! closed and the faces inside it go. Anything else (a rim that cuts across a face) is refused.

use std::collections::{HashMap, HashSet};

use arris::topo::builder::{Assembly, Builder, EdgeKey, FaceSpec, UseSpec, effective_uses};
use arris::topo::entity::Loop;
use arris::topo::{Body, EdgeId, FaceId, Model};

/// The loop's signed area in its face's (u, v): positive for the outer loop, negative for a hole.
fn uv_area(m: &Model, l: &Loop) -> Option<f64> {
    let mut pts = Vec::new();
    for c in l.coedges() {
        let (_, range) = m.edge(c.edge()).ok()?.curve()?;
        let p = m.curve2(c.pcurve()).ok()?;
        let (a, b) =
            if c.orientation() == arris::topo::Orientation::Reversed { (range.hi(), range.lo()) } else { (range.lo(), range.hi()) };
        pts.extend((0..16).map(|i| p.point(a + (b - a) * i as f64 / 16.0)));
    }
    let n = pts.len();
    (n >= 3).then(|| (0..n).map(|i| pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y).sum::<f64>() / 2.0)
}

/// The body without `gone`, closed over them; `None` when their rim isn't whole holes of the faces kept.
pub fn faces(m: &mut Model, b: Body, gone: &HashSet<FaceId>) -> Option<Body> {
    let faces = m.faces(b).ok()?;
    let owners = {
        let mut o: HashMap<EdgeId, Vec<FaceId>> = HashMap::new();
        for f in &faces {
            for c in m.face(f.id).ok()?.loops().iter().flat_map(|l| l.coedges()) {
                o.entry(c.edge()).or_default().push(f.id);
            }
        }
        o
    };
    let across_gone = |e: EdgeId, me: FaceId| owners.get(&e).is_some_and(|fs| fs.iter().any(|f| *f != me && gone.contains(f)));
    // Kept faces with the loops they lose: holes whose every edge borders a face going.
    let mut dropped: HashMap<FaceId, Vec<usize>> = HashMap::new();
    let mut rim: HashSet<EdgeId> = HashSet::new();
    for f in faces.iter().filter(|f| !gone.contains(&f.id)) {
        let face = m.face(f.id).ok()?;
        for (k, l) in face.loops().iter().enumerate() {
            let touching = l.coedges().iter().filter(|c| across_gone(c.edge(), f.id)).count();
            if touching == 0 {
                continue;
            }
            // Part of a loop, or the outer loop: closing it needs surfaces extended.
            if touching < l.coedges().len() || uv_area(m, l)? >= 0.0 {
                return None;
            }
            dropped.entry(f.id).or_default().push(k);
            rim.extend(l.coedges().iter().map(|c| c.edge()));
        }
    }
    // Every edge between a face going and one staying must be on a closed hole.
    for f in faces.iter().filter(|f| gone.contains(&f.id)) {
        for c in m.face(f.id).ok()?.loops().iter().flat_map(|l| l.coedges()) {
            if owners.get(&c.edge()).is_some_and(|fs| fs.iter().any(|g| !gone.contains(g))) && !rim.contains(&c.edge()) {
                return None;
            }
        }
    }
    let mut shells = Vec::new();
    for shell in m.shells(b).ok()? {
        let mut specs = Vec::new();
        for f in m.shell(shell.id).ok()?.faces() {
            if gone.contains(&f.id) {
                continue;
            }
            let Some(lose) = dropped.get(&f.id) else {
                specs.push(FaceSpec::Keep(*f));
                continue;
            };
            let face = m.face(f.id).ok()?;
            let loops = face
                .loops()
                .iter()
                .enumerate()
                .filter(|(k, _)| !lose.contains(k))
                .map(|(_, l)| {
                    effective_uses(f.orientation, l.coedges().iter().map(|c| (c.edge(), c.orientation(), c.pcurve())))
                        .into_iter()
                        .map(|(edge, orientation, pcurve)| UseSpec { edge: EdgeKey::Kept(edge), orientation, pcurve })
                        .collect()
                })
                .collect();
            specs.push(FaceSpec::New { surface: face.surface(), orientation: f.orientation, loops, tolerance: face.tolerance() });
        }
        if !specs.is_empty() {
            shells.push(specs);
        }
    }
    let (builder, _) = Builder::assemble(m, m.precision().default_tolerance, Assembly { shells, ..Assembly::default() }).ok()?;
    let body = builder.finish(m, arris::topo::entity::BodyKind::Solid).ok()?.body;
    arris::check::check(m, body, arris::check::Level::Fast).is_ok().then_some(body)
}
