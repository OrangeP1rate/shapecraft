//! .shapr projects from before files kept a parametric history (files up to about 2024, with no
//! `HistoryTreeNodes`). Their model is a list of solids, each a whole Parasolid transmit file in `Shapes`, an Items
//! tree in `Folders` (JSON elements: 0 folder, 1 solid, 2 sketch, 4 construction plane or axis), sketches in the
//! same tables later files use, and construction planes and axes as plain rows in meters.
//!
//! They come in as a project with no steps: what the Items list shows, in its folders. Folder children are given
//! out as items (type 6) the way later files list them, each with a name id of its own (the files have no
//! `HistoryNames`), so the folder tree is walked the same way.

use std::collections::BTreeMap;

use crate::{Body, Construction, Folder, MM, Project, Result, blob, has_table, sketches, text};

/// Items-list element types in these files.
const FOLDER: i64 = 0;
const SOLID: i64 = 1;
const SKETCH: i64 = 2;
const CONSTRUCTION: i64 = 4;
/// What later files call an item in a folder, which the import's folder walk places.
const ITEM: i64 = 6;

/// Whether the workspace is one of these older projects.
/// Projects saved in 2024 can have a short history but still keep their solids in `Shapes` (no revision tables),
/// so they are read the same way.
pub(crate) fn is_legacy(c: &rusqlite::Connection) -> bool {
    has_table(c, "Shapes") && !has_table(c, "BodyRevisionBlocks") && !has_table(c, "BodyRevisionPartitions")
}

/// A folder as the file keeps it: id, name, hidden, and its children (id, element type).
type RawFolder = (String, String, bool, Vec<(String, i64)>);

pub(crate) fn read(c: &rusqlite::Connection, p: &mut Project) -> Result<()> {
    p.legacy = true;
    // The Items tree as the file keeps it.
    let mut raw: Vec<RawFolder> = Vec::new();
    if has_table(c, "Folders") {
        let mut q = c.prepare("SELECT FolderID, FolderName, Hidden, FolderElements FROM Folders")?;
        let rows = q.query_map([], |r| Ok((text(r.get(0)?), text(r.get(1)?), r.get::<_, Option<bool>>(2)?, text(r.get(3)?))))?;
        for (id, name, hidden, elements) in rows.flatten().take(100_000) {
            let json: serde_json::Value = serde_json::from_str(&elements).unwrap_or_default();
            let children = json["elements"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .take(100_000)
                        .map(|e| {
                            let id = match &e["id"] {
                                serde_json::Value::String(s) => s.clone(),
                                v => v.to_string(),
                            };
                            (id, e["type"].as_i64().unwrap_or(-1))
                        })
                        .collect()
                })
                .unwrap_or_default();
            raw.push((id, name, hidden.unwrap_or(false), children));
        }
    }
    // The first releases grouped solids in shape groups instead of folders: each group is a folder under the root.
    if raw.is_empty() && has_table(c, "ShapeGroups") && has_table(c, "Shapes") && crate::has_column(c, "Shapes", "ShapeGroupID") {
        let mut members: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();
        let mut q = c.prepare("SELECT ShapeID, ShapeGroupID FROM Shapes ORDER BY rowid")?;
        for (shape, group) in q.query_map([], |r| Ok((text(r.get(0)?), text(r.get(1)?))))?.flatten().take(100_000) {
            members.entry(group).or_default().push((shape, SOLID));
        }
        let mut q = c.prepare("SELECT ShapeGroupID, ShapeGroupName, Hidden FROM ShapeGroups ORDER BY OrderInList")?;
        let mut root = Vec::new();
        for (id, name, hidden) in
            q.query_map([], |r| Ok((text(r.get(0)?), text(r.get(1)?), r.get::<_, Option<i64>>(2)?)))?.flatten().take(100_000)
        {
            root.push((id.clone(), FOLDER));
            raw.push((id.clone(), name, hidden.unwrap_or(0) != 0, members.remove(&id).unwrap_or_default()));
        }
        raw.push(("root".into(), "root".into(), false, root));
    }
    // Each element's folder path (names from the top, not counting the root) and whether a folder above it is hidden.
    let mut place: BTreeMap<String, (Vec<String>, bool)> = BTreeMap::new();
    if let Some(root) = raw.iter().position(|f| f.1 == "root") {
        let mut stack: Vec<(usize, Vec<String>, bool, usize)> = vec![(root, Vec::new(), raw[root].2, 0)];
        let mut seen = std::collections::HashSet::new();
        while let Some((at, path, hidden, depth)) = stack.pop() {
            if depth > 100 || !seen.insert(at) {
                continue;
            }
            for (id, ty) in &raw[at].3 {
                if *ty == FOLDER {
                    if let Some(sub) = raw.iter().position(|f| &f.0 == id) {
                        let mut path = path.clone();
                        path.push(raw[sub].1.clone());
                        stack.push((sub, path, hidden || raw[sub].2, depth + 1));
                    }
                } else {
                    place.entry(id.clone()).or_insert_with(|| (path.clone(), hidden));
                }
            }
        }
    }
    // Every solid, sketch and construction element gets a name id, which folders list it by.
    let mut next_name = 0_i64;
    let mut name_of = |key: String, p: &mut Project| -> i64 {
        next_name += 1;
        p.item_by_meta.insert(key, next_name);
        next_name
    };
    let palette = palette(c)?;
    let mut unreadable = Vec::new();
    {
        // The first releases (2017-2018) kept no name or Hidden flag on a solid; it sat in a shape group instead.
        let sql = format!(
            "SELECT ShapeID, ShapeData, {}, {} FROM Shapes ORDER BY rowid",
            column(c, "Shapes", "Hidden", "0"),
            column(c, "Shapes", "ShapeName", "''")
        );
        // Shapr3D itself lists a solid from those releases as "Body".
        let unnamed = if crate::has_column(c, "Shapes", "ShapeName") { None } else { Some("Body".to_string()) };
        let mut q = c.prepare(&sql)?;
        let rows = q.query_map([], |r| Ok((text(r.get(0)?), blob(r.get(1)?), r.get::<_, Option<bool>>(2)?, text(r.get(3)?))))?;
        for (id, data, hidden, name) in rows.flatten().take(100_000) {
            let (folder, folder_hidden) = place.get(&id).cloned().unwrap_or_default();
            let name_id = name_of(id.clone(), p);
            let solids = match shapecraft_xt::parse(&data).and_then(|f| shapecraft_xt::bodies(&f)) {
                Ok(solids) => solids,
                Err(e) => {
                    unreadable.push(format!("{name}: {e}"));
                    continue;
                }
            };
            for mut solid in solids {
                let color = paint(&mut solid, &palette).or(solid.color);
                p.bodies.push(Body {
                    partition: None,
                    import: None,
                    folder: folder.clone(),
                    step: None,
                    name: Some(name.clone()).filter(|n| !n.is_empty()).or(solid.name).or_else(|| unnamed.clone()),
                    item_name_id: Some(name_id),
                    brep: solid.brep,
                    color,
                    hidden: hidden.unwrap_or(false) || folder_hidden,
                    mesh: solid.mesh,
                });
            }
        }
    }
    p.unreadable = unreadable;
    if has_table(c, "SketchControllers") && has_table(c, "SketchCurves") {
        let mut all = sketches(c)?;
        for sk in &mut all {
            let key = sk.key.clone();
            if let Some((_, true)) = place.get(&key) {
                sk.hidden = true;
            }
            let n = name_of(key, p);
            p.sketch_items.insert(sk.id, n);
        }
        p.sketches = all;
    }
    if has_table(c, "ConstructionGeometryPlanes") {
        // Planes saved before about 2020 have no in-plane direction (0, 0, 0 here: one is chosen from the normal).
        let sql = format!(
            "SELECT GeometryID, Name, IsHidden, CenterXPosition, CenterYPosition, CenterZPosition, NormalXValue, NormalYValue, NormalZValue, \
             Size, {}, {}, {} FROM ConstructionGeometryPlanes ORDER BY OrderInList",
            column(c, "ConstructionGeometryPlanes", "ReferenceXValue", "0"),
            column(c, "ConstructionGeometryPlanes", "ReferenceYValue", "0"),
            column(c, "ConstructionGeometryPlanes", "ReferenceZValue", "0")
        );
        let mut q = c.prepare(&sql)?;
        let rows = q.query_map([], |r| {
            let f = |i: usize| r.get::<_, Option<f64>>(i).map(|v| v.unwrap_or(0.0));
            Ok((
                text(r.get(0)?),
                text(r.get(1)?),
                r.get::<_, Option<bool>>(2)?.unwrap_or(false),
                [f(3)? * MM, f(4)? * MM, f(5)? * MM],
                [f(6)?, f(7)?, f(8)?],
                f(9)? * MM,
                [f(10)?, f(11)?, f(12)?],
            ))
        })?;
        for (id, name, hidden, origin, normal, size, x) in rows.flatten().take(100_000) {
            let folder_hidden = place.get(&id).is_some_and(|(_, h)| *h);
            let item_name_id = name_of(id, p);
            p.construction.push(Construction::Plane { name, hidden: hidden || folder_hidden, origin, normal, x, size, item_name_id });
        }
    }
    if has_table(c, "ConstructionGeometryLines") {
        let mut q = c.prepare(
            "SELECT GeometryID, Name, IsHidden, StartXPosition, StartYPosition, StartZPosition, VectorX, VectorY, VectorZ \
             FROM ConstructionGeometryLines ORDER BY OrderInList",
        )?;
        let rows = q.query_map([], |r| {
            let f = |i: usize| r.get::<_, Option<f64>>(i).map(|v| v.unwrap_or(0.0));
            Ok((
                text(r.get(0)?),
                text(r.get(1)?),
                r.get::<_, Option<bool>>(2)?.unwrap_or(false),
                [f(3)? * MM, f(4)? * MM, f(5)? * MM],
                [f(6)? * MM, f(7)? * MM, f(8)? * MM],
            ))
        })?;
        for (id, name, hidden, start, vector) in rows.flatten().take(100_000) {
            let folder_hidden = place.get(&id).is_some_and(|(_, h)| *h);
            let item_name_id = name_of(id, p);
            p.construction.push(Construction::Axis { name, hidden: hidden || folder_hidden, start, vector, item_name_id });
        }
    }
    // The tree, with solids, sketches and construction listed as items by their name ids' keys.
    p.folders = raw
        .into_iter()
        .map(|(id, name, hidden, children)| Folder {
            id,
            name,
            hidden,
            children: children
                .into_iter()
                .map(|(id, ty)| match ty {
                    SOLID | SKETCH | CONSTRUCTION => (id, ITEM),
                    _ => (id, ty),
                })
                .collect(),
        })
        .collect();
    Ok(())
}

/// `name` when the table has that column, else `fallback` (an SQL literal): the schema grew column by column.
fn column(c: &rusqlite::Connection, table: &str, name: &str, fallback: &str) -> String {
    if crate::has_column(c, table, name) { name.to_string() } else { fallback.to_string() }
}

/// The default grey, which is no colour of its own.
const DEFAULT_GREY: u32 = 0xA0_A0A4;

/// Material instance → its base colour (0–1), `None` for the default grey.
fn palette(c: &rusqlite::Connection) -> Result<BTreeMap<i64, Option<[f32; 3]>>> {
    let mut out = BTreeMap::new();
    if !has_table(c, "MaterialInstances") {
        return Ok(out);
    }
    let mut q = c.prepare("SELECT InstanceID, Properties FROM MaterialInstances")?;
    for (id, props) in q.query_map([], |r| Ok((r.get::<_, i64>(0)?, text(r.get(1)?))))?.flatten().take(100_000) {
        let json: serde_json::Value = serde_json::from_str(&props).unwrap_or_default();
        // RGBA in one number.
        let Some(rgba) = json["baseColor"].as_u64() else { continue };
        out.insert(id, rgb((rgba >> 8) as u32 & 0xFF_FFFF));
    }
    Ok(out)
}

fn rgb(v: u32) -> Option<[f32; 3]> {
    (v != DEFAULT_GREY).then(|| [(v >> 16) as f32 / 255.0, ((v >> 8) & 0xFF) as f32 / 255.0, (v & 0xFF) as f32 / 255.0])
}

/// A solid's colours from the materials set on its faces: the commonest is the body's, and faces of another colour
/// keep theirs. Returns the body's colour.
fn paint(solid: &mut shapecraft_xt::Body, palette: &BTreeMap<i64, Option<[f32; 3]>>) -> Option<[f32; 3]> {
    let colours: Vec<Option<Option<[f32; 3]>>> =
        solid.face_ids.iter().map(|id| solid.face_materials.get(id).and_then(|m| palette.get(m)).copied()).collect();
    let mut counts: Vec<(Option<[f32; 3]>, usize)> = Vec::new();
    for c in colours.iter().flatten() {
        match counts.iter_mut().find(|(k, _)| k == c) {
            Some((_, n)) => *n += 1,
            None => counts.push((*c, 1)),
        }
    }
    let body = counts.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c)?;
    for (face, c) in solid.brep.faces.iter_mut().zip(&colours) {
        if let Some(c) = c
            && *c != body
        {
            // A grey face on a coloured body is painted grey, so the body's colour doesn't cover it.
            face.color = Some(c.unwrap_or([0xA0 as f32 / 255.0, 0xA0 as f32 / 255.0, 0xA4 as f32 / 255.0]));
        }
    }
    body
}
