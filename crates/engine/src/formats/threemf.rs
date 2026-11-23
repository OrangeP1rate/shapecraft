//! 3MF packages: every placed mesh as its own body, with its name and color. Reads the core spec, the Production
//! extension's split model files, Materials color groups, and the part names and filament colors Bambu Studio and
//! OrcaSlicer keep in their own settings files. Streamed, as plates with hundreds of MB of triangles are common.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Cursor, Read};

use glam::{DAffine3, DMat3, DVec3};

use super::MAX_IMPORT_TRIANGLES;

/// Most bytes one model file may unpack to, and the settings files.
const MAX_MODEL: u64 = 2 << 30;
const MAX_SETTINGS: u64 = 64 << 20;
/// Most bytes one tag may have (a vertex has about 60).
const MAX_TAG: u64 = 1 << 20;
const MAX_BODIES: usize = 20_000;
const MAX_DEPTH: usize = 32;

/// One placed mesh, in millimetres.
pub struct Part {
    /// What the file calls it, if anything.
    pub name: Option<String>,
    pub color: Option<[f32; 3]>,
    pub points: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
}

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;
/// A color list's id within one model file, and the colors in it.
type Colors = HashMap<u32, Vec<[f32; 3]>>;
/// An object's model file and its id there.
type Key = (String, u32);
/// By placed object, and part of it (or none: the object itself).
type Settings = HashMap<(u32, Option<u32>), Setting>;

#[derive(Default)]
struct Model {
    unit: f64,
    objects: HashMap<u32, Object>,
    /// In file order, for a file that places nothing.
    order: Vec<u32>,
    build: Vec<Placed>,
    colors: Colors,
}

#[derive(Default)]
struct Object {
    name: Option<String>,
    /// Support or a helper (a slicer's negative or modifier part): not something to bring in.
    helper: bool,
    color: Option<(u32, u32)>,
    points: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
    /// The triangles' own colors, counted (used when the object names none).
    painted: HashMap<(u32, u32), usize>,
    components: Vec<Placed>,
}

struct Placed {
    path: Option<String>,
    object: u32,
    transform: DAffine3,
}

/// What a slicer's settings say about one object or part.
#[derive(Default)]
struct Setting {
    name: Option<String>,
    extruder: Option<usize>,
    helper: bool,
}

/// Reads a 3MF package's placed meshes, in the order they are placed.
pub fn read_3mf(bytes: &[u8]) -> Result<Vec<Part>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "not a valid 3MF file".to_string())?;
    let root = root_model(&mut zip).ok_or("the 3MF file has no model in it")?;
    let mut models: HashMap<String, Model> = HashMap::new();
    let mut wanted = vec![root.clone()];
    let mut triangles = 0;
    while let Some(path) = wanted.pop() {
        if models.contains_key(&path) {
            continue;
        }
        let Some(name) = entry_name(&zip, &path) else {
            return Err(format!("the 3MF file is missing its part {path}"));
        };
        let file = zip.by_name(&name).map_err(|e| format!("the 3MF file can't be unpacked ({e})"))?;
        let m = read_model(BufReader::with_capacity(1 << 16, file.take(MAX_MODEL)), &mut triangles)?;
        for p in m.build.iter().chain(m.objects.values().flat_map(|o| &o.components)) {
            if let Some(p) = &p.path {
                wanted.push(p.clone());
            }
        }
        models.insert(path, m);
    }
    let (settings, filaments) = slicer_settings(&mut zip);
    let Some(main) = models.get(&root) else { return Err("the 3MF file has no model in it".into()) };
    let unit = main.unit;
    // A file that places nothing: every object that isn't a piece of another.
    let mut loose = Vec::new();
    if main.build.is_empty() {
        let used: HashSet<u32> = main.objects.values().flat_map(|o| &o.components).filter(|c| c.path.is_none()).map(|c| c.object).collect();
        loose.extend(main.order.iter().filter(|id| !used.contains(id)).map(|&object| Placed {
            path: None,
            object,
            transform: DAffine3::IDENTITY,
        }));
    }
    let mut walk = Walk { models: &models, settings: &settings, filaments: &filaments, unit, out: Vec::new(), triangles: 0, visits: 0 };
    for item in main.build.iter().chain(&loose) {
        let key = (item.path.clone().unwrap_or_else(|| root.clone()), item.object);
        let setting = (key.0 == root).then(|| settings.get(&(item.object, None))).flatten();
        walk.place(&key, item.transform, None, None, setting.and_then(|s| s.extruder), item.object, 0)?;
    }
    if walk.out.is_empty() {
        return Err("the 3MF file has no meshes in it".into());
    }
    Ok(walk.out)
}

struct Walk<'a> {
    models: &'a HashMap<String, Model>,
    settings: &'a Settings,
    filaments: &'a [[f32; 3]],
    unit: f64,
    out: Vec<Part>,
    triangles: usize,
    visits: usize,
}

impl Walk<'_> {
    /// `name` and `color`: what the objects around this one are called and colored. `item`: the placed object the
    /// slicer's settings list this part under.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        key: &Key,
        at: DAffine3,
        name: Option<&str>,
        color: Option<[f32; 3]>,
        extruder: Option<usize>,
        item: u32,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the 3MF file's components go in circles".into());
        }
        // Nothing but components in components, however many times over.
        self.visits += 1;
        if self.visits > 1_000_000 {
            return Err(too_big());
        }
        let Some(model) = self.models.get(&key.0) else { return Ok(()) };
        let Some(o) = model.objects.get(&key.1) else { return Ok(()) };
        // The settings list a part under its placed object; an object placed as it is has only its own entry.
        let part = self.settings.get(&(item, Some(key.1))).filter(|_| depth > 0);
        if o.helper || part.is_some_and(|p| p.helper) {
            return Ok(());
        }
        let name = o.name.as_deref().or(name);
        let pick = |(group, index): (u32, u32)| model.colors.get(&group)?.get(index as usize).copied();
        let color = o.color.and_then(pick).or(color);
        let extruder = part.and_then(|p| p.extruder).or(extruder);
        for c in &o.components {
            let key = (c.path.clone().unwrap_or_else(|| key.0.clone()), c.object);
            self.place(&key, at * c.transform, name, color, extruder, item, depth + 1)?;
        }
        if o.triangles.is_empty() {
            return Ok(());
        }
        self.triangles += o.triangles.len();
        if self.out.len() >= MAX_BODIES || self.triangles > MAX_IMPORT_TRIANGLES {
            return Err(too_big());
        }
        let painted = || o.painted.iter().max_by_key(|(k, n)| (**n, std::cmp::Reverse(**k))).and_then(|(k, _)| pick(*k));
        let filament = || self.filaments.get(extruder.unwrap_or(1).checked_sub(1)?).copied();
        let own = part.or_else(|| self.settings.get(&(item, None))).and_then(|s| s.name.as_deref());
        let points: Vec<DVec3> = o.points.iter().map(|p| at.transform_point3(*p) * self.unit).collect();
        if points.iter().any(|p| !p.is_finite() || p.abs().max_element() >= 1e9) {
            return Err("the mesh has invalid coordinates".into());
        }
        let mirrored = at.matrix3.determinant() < 0.0;
        self.out.push(Part {
            name: own.or(name).map(tidy_name).filter(|n| !n.is_empty()),
            color: color.or_else(painted).or_else(filament),
            points,
            triangles: o.triangles.iter().map(|&[a, b, c]| if mirrored { [a, c, b] } else { [a, b, c] }).collect(),
        });
        Ok(())
    }
}

fn too_big() -> String {
    format!("the 3MF file is too big (over {} million triangles or {MAX_BODIES} parts)", MAX_IMPORT_TRIANGLES / 1_000_000)
}

/// "sword.stl" is the file a slicer's part came from: "sword".
fn tidy_name(name: &str) -> String {
    let n = name.trim();
    let n = match n.rsplit_once('.') {
        Some((stem, ext)) if ["stl", "obj", "3mf", "step", "stp", "amf"].contains(&ext.to_ascii_lowercase().as_str()) => stem,
        _ => n,
    };
    n.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// The archive's entry for a part path ("/3D/3dmodel.model"), whatever its case.
fn entry_name(zip: &Archive, path: &str) -> Option<String> {
    let want = path.trim_start_matches('/');
    zip.file_names().find(|n| n.trim_start_matches('/').eq_ignore_ascii_case(want)).map(str::to_string)
}

fn read_text(zip: &mut Archive, path: &str) -> Option<Vec<u8>> {
    let name = entry_name(zip, path)?;
    let mut out = Vec::new();
    zip.by_name(&name).ok()?.take(MAX_SETTINGS).read_to_end(&mut out).ok()?;
    Some(out)
}

/// The package's start part: the one its relationships name, or the usual place.
fn root_model(zip: &mut Archive) -> Option<String> {
    let mut found = None;
    if let Some(rels) = read_text(zip, "_rels/.rels") {
        let _ = tags(&rels[..], &mut |t| {
            if found.is_none() && t.is("Relationship") && t.get("Type").is_some_and(|k| k.ends_with("/3dmodel")) {
                found = t.get("Target");
            }
            Ok(())
        });
    }
    let usual = "3D/3dmodel.model".to_string();
    found
        .into_iter()
        .chain([usual])
        .find(|p| entry_name(zip, p).is_some())
        .or_else(|| zip.file_names().find(|n| n.to_ascii_lowercase().ends_with(".model")).map(str::to_string))
}

fn read_model(r: impl BufRead, total: &mut usize) -> Result<Model, String> {
    let mut m = Model { unit: 1.0, ..Default::default() };
    let mut object: Option<(u32, Object)> = None;
    let mut group: Option<u32> = None;
    tags(r, &mut |t| {
        match t.name {
            b"vertex" => {
                if let Some((_, o)) = &mut object {
                    let p = DVec3::new(t.num("x")?, t.num("y")?, t.num("z")?);
                    o.points.push(p);
                    if o.points.len() > MAX_IMPORT_TRIANGLES * 3 {
                        return Err(too_big());
                    }
                }
            }
            b"triangle" => {
                if let Some((_, o)) = &mut object {
                    let v = [t.id("v1")?, t.id("v2")?, t.id("v3")?];
                    if v.iter().any(|&i| i as usize >= o.points.len()) {
                        return Err("the 3MF file has a triangle index out of range".into());
                    }
                    if let Some(p1) = t.get("p1").and_then(|v| v.trim().parse::<u32>().ok())
                        && let Some(pid) = t.get("pid").and_then(|v| v.trim().parse().ok()).or(o.color.map(|c| c.0))
                    {
                        *o.painted.entry((pid, p1)).or_default() += 1;
                    }
                    o.triangles.push(v);
                    *total += 1;
                    if *total > MAX_IMPORT_TRIANGLES {
                        return Err(too_big());
                    }
                }
            }
            b"model" if !t.end => {
                m.unit = match t.get("unit").as_deref().unwrap_or("millimeter") {
                    "micron" => 0.001,
                    "millimeter" => 1.0,
                    "centimeter" => 10.0,
                    "inch" => 25.4,
                    "foot" => 304.8,
                    "meter" => 1000.0,
                    other => return Err(format!("the 3MF file is in an unknown unit ({other:?})")),
                };
            }
            b"object" => {
                if let Some((id, o)) = object.take() {
                    m.order.push(id);
                    m.objects.insert(id, o);
                }
                if !t.end {
                    let pid = t.get("pid").and_then(|v| v.trim().parse().ok());
                    let index = t.get("pindex").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    let o = Object {
                        name: t.get("name").filter(|n| !n.trim().is_empty()),
                        helper: t.get("type").is_some_and(|k| matches!(k.as_str(), "support" | "solidsupport" | "other")),
                        color: pid.map(|p| (p, index)),
                        ..Default::default()
                    };
                    object = Some((t.id("id")?, o));
                }
            }
            b"component" => {
                if let Some((_, o)) = &mut object {
                    o.components.push(t.placed()?);
                    if o.components.len() > MAX_BODIES {
                        return Err(too_big());
                    }
                }
            }
            b"item" if !t.end => {
                m.build.push(t.placed()?);
                if m.build.len() > MAX_BODIES {
                    return Err(too_big());
                }
            }
            b"basematerials" | b"colorgroup" => group = if t.end { None } else { Some(t.id("id")?) },
            b"base" | b"color" if !t.end => {
                if let Some(g) = group {
                    let c = t.get("displaycolor").or_else(|| t.get("color")).and_then(|c| parse_color(&c));
                    m.colors.entry(g).or_default().push(c.unwrap_or([0.8; 3]));
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    if let Some((id, o)) = object.take() {
        m.order.push(id);
        m.objects.insert(id, o);
    }
    Ok(m)
}

/// Bambu Studio's and OrcaSlicer's own files: each object's and part's name, kind and filament, and the filaments'
/// colors.
fn slicer_settings(zip: &mut Archive) -> (Settings, Vec<[f32; 3]>) {
    let mut out = Settings::new();
    let (mut object, mut part) = (None, None);
    if let Some(text) = read_text(zip, "Metadata/model_settings.config") {
        let _ = tags(&text[..], &mut |t| {
            let id = || t.get("id").and_then(|v| v.trim().parse::<u32>().ok());
            match t.name {
                b"object" => object = if t.end { None } else { id() },
                b"part" => {
                    part = if t.end { None } else { id() };
                    if let (Some(o), Some(p), Some(kind)) = (object, part, t.get("subtype")) {
                        out.entry((o, Some(p))).or_default().helper = kind != "normal_part";
                    }
                }
                b"metadata" => {
                    if let (Some(o), Some(key), Some(value)) = (object, t.get("key"), t.get("value")) {
                        let s = out.entry((o, part)).or_default();
                        match key.as_str() {
                            "name" if !value.trim().is_empty() => s.name = Some(value),
                            // 0 is "as the object has it".
                            "extruder" => s.extruder = value.trim().parse().ok().filter(|n| *n > 0),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        });
    }
    let filaments = read_text(zip, "Metadata/project_settings.config")
        .and_then(|t| serde_json::from_slice::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            let list = v.get("filament_colour")?.as_array()?.clone();
            Some(list.iter().map(|c| c.as_str().and_then(parse_color).unwrap_or([0.8; 3])).collect())
        })
        .unwrap_or_default();
    (out, filaments)
}

/// "#RRGGBB" or "#RRGGBBAA".
fn parse_color(text: &str) -> Option<[f32; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
        return None;
    }
    let c = |k: usize| u8::from_str_radix(hex.get(k..k + 2)?, 16).ok().map(|v| f32::from(v) / 255.0);
    Some([c(0)?, c(2)?, c(4)?])
}

/// One XML tag, its name without the namespace prefix.
struct Tag<'a> {
    name: &'a [u8],
    /// `</name>`
    end: bool,
    attributes: &'a [u8],
}

impl Tag<'_> {
    fn is(&self, name: &str) -> bool {
        self.name == name.as_bytes() && !self.end
    }

    /// An attribute's raw value; `name` matches without a namespace prefix ("path" finds `p:path`).
    fn raw(&self, name: &str) -> Option<&[u8]> {
        let mut rest = self.attributes;
        loop {
            let eq = rest.iter().position(|&b| b == b'=')?;
            let key = rest.get(..eq)?.trim_ascii();
            let after = rest.get(eq + 1..)?.trim_ascii_start();
            let quote = *after.first().filter(|q| matches!(q, b'"' | b'\''))?;
            let close = after.get(1..)?.iter().position(|&b| b == quote)?;
            let value = after.get(1..1 + close)?;
            if local(key) == name.as_bytes() {
                return Some(value);
            }
            rest = after.get(close + 2..)?;
        }
    }

    fn get(&self, name: &str) -> Option<String> {
        let text = String::from_utf8_lossy(self.raw(name)?);
        Some(if text.contains('&') { unescape(&text) } else { text.into_owned() })
    }

    fn num(&self, name: &str) -> Result<f64, String> {
        self.raw(name)
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .ok_or_else(|| "the 3MF file has a broken vertex".to_string())
    }

    fn id(&self, name: &str) -> Result<u32, String> {
        self.raw(name)
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|v| v.trim().parse().ok())
            .ok_or_else(|| format!("the 3MF file has a broken {}", String::from_utf8_lossy(self.name)))
    }

    fn placed(&self) -> Result<Placed, String> {
        let transform = match self.get("transform") {
            None => DAffine3::IDENTITY,
            Some(t) => {
                let v: Vec<f64> = t.split_whitespace().filter_map(|w| w.parse().ok()).filter(|v: &f64| v.is_finite()).collect();
                let &[a, b, c, d, e, f, g, h, i, x, y, z] = v.as_slice() else {
                    return Err("the 3MF file has a broken transform".into());
                };
                // Row vectors in the file: its rows are our columns.
                let m = DMat3::from_cols(DVec3::new(a, b, c), DVec3::new(d, e, f), DVec3::new(g, h, i));
                DAffine3::from_mat3_translation(m, DVec3::new(x, y, z))
            }
        };
        Ok(Placed { path: self.get("path"), object: self.id("objectid")?, transform })
    }
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => name.get(i + 1..).unwrap_or(name),
        None => name,
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        rest = rest.get(i..).unwrap_or_default();
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = rest.get(1..).unwrap_or_default();
            continue;
        };
        let name = rest.get(1..end).unwrap_or_default();
        let c = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(rest.get(..=end).unwrap_or_default()),
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// Calls `f` for every tag in an XML file (`<a/>` once, as a start). Text, comments, processing instructions and
/// CDATA are passed over; a document type (entities) is refused, as the 3MF spec does.
fn tags(mut r: impl BufRead, f: &mut dyn FnMut(&Tag) -> Result<(), String>) -> Result<(), String> {
    let broken = |e: std::io::Error| format!("the 3MF file can't be read ({e})");
    let mut buf = Vec::new();
    loop {
        // Up to the next tag.
        loop {
            let chunk = r.fill_buf().map_err(broken)?;
            if chunk.is_empty() {
                return Ok(());
            }
            match chunk.iter().position(|&b| b == b'<') {
                Some(i) => {
                    r.consume(i + 1);
                    break;
                }
                None => {
                    let n = chunk.len();
                    r.consume(n);
                }
            }
        }
        buf.clear();
        // A `>` inside a quoted value, a comment or CDATA doesn't end it.
        loop {
            let before = buf.len();
            let left = MAX_TAG.saturating_sub(before as u64);
            let n = (&mut r).take(left).read_until(b'>', &mut buf).map_err(broken)?;
            if buf.last() != Some(&b'>') {
                return if n == 0 || left == 0 || buf.len() as u64 >= MAX_TAG {
                    Err("the 3MF file's XML is broken".into())
                } else {
                    Err("the 3MF file ends in the middle".into())
                };
            }
            let done = if buf.starts_with(b"!--") {
                buf.len() >= 6 && buf.ends_with(b"-->")
            } else if buf.starts_with(b"![CDATA[") {
                buf.ends_with(b"]]>")
            } else if buf.starts_with(b"?") {
                buf.ends_with(b"?>")
            } else {
                let mut quote = None;
                for &b in &buf {
                    match quote {
                        None if matches!(b, b'"' | b'\'') => quote = Some(b),
                        Some(q) if q == b => quote = None,
                        _ => {}
                    }
                }
                quote.is_none()
            };
            if done {
                break;
            }
        }
        match buf.first() {
            Some(b'?') => continue,
            Some(b'!') if buf.starts_with(b"!DOCTYPE") => return Err("the 3MF file has a document type, which isn't allowed".into()),
            Some(b'!') => continue,
            _ => {}
        }
        let body = buf.strip_suffix(b">").unwrap_or(&buf);
        let (end, body) = match body.strip_prefix(b"/") {
            Some(b) => (true, b),
            None => (false, body.strip_suffix(b"/").unwrap_or(body)),
        };
        let cut = body.iter().position(u8::is_ascii_whitespace).unwrap_or(body.len());
        let (name, attributes) = body.split_at(cut);
        f(&Tag { name: local(name), end, attributes })?;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const CUBE: &str = "<mesh><vertices><vertex x='0' y='0' z='0'/><vertex x='1' y='0' z='0'/><vertex x='0' y='1' z='0'/><vertex x='0' y='0' z='1'/></vertices><triangles><triangle v1='0' v2='2' v3='1'/><triangle v1='0' v2='1' v3='3' pid='9' p1='1'/><triangle v1='0' v2='3' v3='2' pid='9' p1='1'/><triangle v1='1' v2='2' v3='3'/></triangles></mesh>";

    fn package(files: &[(&str, &str)]) -> Vec<u8> {
        let entries: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, t)| (*n, t.as_bytes().to_vec())).collect();
        super::super::zip_store(&entries).unwrap()
    }

    fn read(files: &[(&str, &str)]) -> Result<Vec<Part>, String> {
        read_3mf(&package(files))
    }

    fn lo_hi(p: &Part) -> (DVec3, DVec3) {
        p.points.iter().fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(a, b), q| (a.min(*q), b.max(*q)))
    }

    #[test]
    fn placed_objects_with_names_colors_units_and_mirrors() {
        let model = format!(
            "<?xml version=\"1.0\"?><!-- > a comment --><model unit='centimeter' xmlns:m='x'><resources><basematerials id='1'><base name='a' displaycolor='#FF0000'/></basematerials><m:colorgroup id='9'><m:color color='#0000FFFF'/><m:color color='#00FF00FF'/></m:colorgroup>\
             <object id='3' name='Red &amp; small' pid='1' pindex='0'>{CUBE}</object><object id='4'>{CUBE}</object>\
             <object id='5' type='support'>{CUBE}</object><object id='6' name='Pair'><components><component objectid='3' transform='1 0 0 0 1 0 0 0 1 5 0 0'/><component objectid='4'/><component objectid='5'/></components></object></resources>\
             <build><item objectid='6' transform='1 0 0 0 1 0 0 0 1 0 0 2'/><item objectid='4' transform='-1 0 0 0 1 0 0 0 1 0 0 0'/></build></model>"
        );
        let parts = read(&[("3D/3dmodel.model", &model)]).unwrap();
        assert_eq!(parts.len(), 3, "the support stays out");
        assert_eq!(parts[0].name.as_deref(), Some("Red & small"));
        assert_eq!(parts[0].color, Some([1.0, 0.0, 0.0]));
        assert_eq!(lo_hi(&parts[0]), (DVec3::new(50.0, 0.0, 20.0), DVec3::new(60.0, 10.0, 30.0)));
        // Named after what holds it, and coloured as most of its triangles are.
        assert_eq!((parts[1].name.as_deref(), parts[1].color), (Some("Pair"), Some([0.0, 1.0, 0.0])));
        // Mirrored: turned the right way out again.
        assert_eq!(lo_hi(&parts[2]).0.x, -10.0);
        assert_eq!(parts[2].triangles[0], [0, 1, 2]);
    }

    #[test]
    fn split_model_files_with_slicer_names_and_filaments() {
        let main = "<model xmlns:p='x'><resources><object id='2'><components><component p:path='/3D/Objects/a.model' objectid='1' transform='1 0 0 0 1 0 0 0 1 0 0 0'/><component p:path='/3D/Objects/a.model' objectid='7'/></components></object></resources><build><item objectid='2' transform='1 0 0 0 1 0 0 0 1 10 0 0'/></build></model>";
        let part = format!(
            "<model unit='millimeter'><resources><object id='1'>{CUBE}</object><object id='7'>{CUBE}</object></resources><build/></model>"
        );
        let part = part.replace(" pid='9' p1='1'", "");
        let settings = "<config><object id='2'><metadata key='name' value='All.stl'/><metadata key='extruder' value='1'/><part id='1' subtype='normal_part'><metadata key='name' value='sword.stl'/><metadata key='extruder' value='2'/></part><part id='7' subtype='negative_part'><metadata key='name' value='hole'/></part></object></config>";
        let project = "{\"filament_colour\": [\"#FFFFFF\", \"#00FF00FF\"]}";
        let files = [
            (
                "_rels/.rels",
                "<Relationships><Relationship Target='/3D/3dmodel.model' Type='http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel'/></Relationships>",
            ),
            ("3D/3dmodel.model", main),
            ("3D/Objects/a.model", &part),
            ("Metadata/model_settings.config", settings),
            ("Metadata/project_settings.config", project),
        ];
        let parts = read(&files).unwrap();
        assert_eq!(parts.len(), 1, "the negative part stays out");
        assert_eq!((parts[0].name.as_deref(), parts[0].color), (Some("sword"), Some([0.0, 1.0, 0.0])));
        assert_eq!(lo_hi(&parts[0]).0.x, 10.0);
    }

    #[test]
    fn broken_and_hostile_files_are_refused() {
        assert!(read_3mf(b"PK not a zip").is_err());
        for bad in [
            "<model unit='parsec'/>".to_string(),
            "<!DOCTYPE x [<!ENTITY a 'b'>]><model/>".into(),
            format!("<model><resources><object id='1'>{CUBE}</object>").replace("v3='1'", "v3='99'"),
            format!("<model><resources><object id='1'>{CUBE}</object></resources>").replace("x='1'", "x='nan'"),
            "<model><resources><object id='1'><components><component objectid='1'/></components></object></resources><build><item objectid='1'/></build></model>".into(),
            "<model><build><item objectid='1' p:path='/3D/none.model'/></build></model>".into(),
            "<model><resources><object id='1' name='unfinished".into(),
            "<model/>".into(),
        ] {
            assert!(read(&[("3D/3dmodel.model", &bad)]).is_err(), "{bad}");
        }
        // Components inside components a thousand times over: stopped, not expanded.
        let mut m = format!("<model><resources><object id='0'>{CUBE}</object>");
        for k in 1..=9 {
            m += &format!("<object id='{k}'><components>{}</components></object>", format!("<component objectid='{}'/>", k - 1).repeat(10));
        }
        m += "</resources><build><item objectid='9'/></build></model>";
        assert!(read(&[("3D/3dmodel.model", &m)]).is_err());
    }
}
