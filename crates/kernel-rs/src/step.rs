//! STEP files through Arris's reader and writer, with their product structure: each part's name, colour, faces
//! coloured apart and the assemblies it sits in, both ways.

use std::path::Path;

use arris::io::step::{FaceColour, Occurrence, ProductTree, Rgb};
use arris::math::Isometry;
use arris::topo::{Body, Model};
use shapecraft_kernel::{KernelError, PartInfo, Result, Shape};

use crate::{C, bring, exact, pack, wrap};

fn step_error(e: impl std::fmt::Display) -> KernelError {
    KernelError::Operation { op: "STEP files", message: e.to_string() }
}

fn rgb(c: [f32; 3]) -> Rgb {
    Rgb(c.map(f64::from))
}

fn srgb(c: Rgb) -> [f32; 3] {
    c.0.map(|x| x as f32)
}

/// The occurrence a part path leads to under `roots`, made (an assembly of no solids) where it isn't there yet.
fn assembly<'a>(roots: &'a mut Vec<Occurrence>, path: &[String]) -> &'a mut Vec<Occurrence> {
    let Some((name, rest)) = path.split_first() else { return roots };
    let at = match roots.iter().position(|o| o.solids.is_empty() && o.name == *name) {
        Some(i) => i,
        None => {
            roots.push(occurrence(name.clone(), Vec::new(), None));
            roots.len() - 1
        }
    };
    assembly(&mut roots[at].children, rest)
}

fn occurrence(name: String, solids: Vec<usize>, colour: Option<Rgb>) -> Occurrence {
    Occurrence { product: None, name, placement: Ok(Isometry::identity()), colour, solids, children: Vec::new() }
}

/// Writes each part as a product of its own (named, coloured), inside assemblies named by its path.
pub fn write_parts(parts: &[(Shape, PartInfo)], path: &Path) -> Result<()> {
    let mut m = Model::default();
    let mut bodies: Vec<Body> = Vec::new();
    let mut tree = ProductTree::default();
    for (k, (shape, info)) in parts.iter().enumerate() {
        let s = exact(shape)?;
        let first = bodies.len();
        bodies.extend(bring(&mut m, s)?);
        let solids: Vec<usize> = (first..bodies.len()).collect();
        // Faces coloured apart: the shape's face numbering is its bodies' faces in order, as they were copied.
        let faces: Vec<(usize, arris::topo::FaceId)> =
            solids.iter().flat_map(|&i| m.faces(bodies[i]).unwrap_or_default().into_iter().map(move |f| (i, f.id))).collect();
        for (face, colour) in &info.face_colors {
            if let Some(&(solid, face)) = faces.get(*face as usize) {
                tree.faces.push(FaceColour { solid, face, colour: rgb(*colour) });
            }
        }
        let name = info.name.clone().unwrap_or_else(|| format!("Body {}", k + 1));
        let part = occurrence(name, solids, info.color.map(rgb));
        assembly(&mut tree.roots, &info.path).push(part);
    }
    let text = arris::io::step::write_products(&m, &bodies, &tree, C).map_err(step_error)?;
    std::fs::write(path, text).map_err(|e| KernelError::Io(e.to_string()))
}

/// Every solid of the file Arris can read, with its part's name, colour and assembly path, in the file's order.
pub fn read_parts(path: &Path) -> Result<Vec<(Shape, PartInfo)>> {
    let bytes = std::fs::read(path).map_err(|e| KernelError::Io(e.to_string()))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut m = Model::default();
    let read = arris::io::step::read(&mut m, &text, &Default::default(), C).map_err(step_error)?;
    // Each solid's part, from the product tree (a solid no occurrence holds keeps a blank one).
    let mut info: Vec<PartInfo> = vec![PartInfo::default(); read.solids.len()];
    fn walk(o: &Occurrence, path: &mut Vec<String>, info: &mut [PartInfo]) {
        for &i in &o.solids {
            if let Some(p) = info.get_mut(i) {
                p.name = (!o.name.is_empty()).then(|| o.name.clone());
                p.color = o.colour.map(srgb);
                p.path = path.clone();
            }
        }
        path.push(o.name.clone());
        for c in &o.children {
            walk(c, path, info);
        }
        path.pop();
    }
    for root in &read.products.roots {
        walk(root, &mut Vec::new(), &mut info);
    }
    let mut out = Vec::new();
    for (i, (solid, mut part)) in read.solids.iter().zip(info).enumerate() {
        let Ok(b) = &solid.result else { continue };
        // Packing copies the faces in order (a kernel test checks), so a face keeps its place in the numbering.
        let original: Vec<arris::topo::FaceId> = m.faces(b.body).unwrap_or_default().iter().map(|f| f.id).collect();
        let shape = pack(&m, &[b.body])?;
        for fc in read.products.faces.iter().filter(|fc| fc.solid == i) {
            if let Some(k) = original.iter().position(|f| *f == fc.face) {
                part.face_colors.push((k as u32, srgb(fc.colour)));
            }
        }
        out.push((wrap(shape), part));
    }
    if out.is_empty() {
        return Err(step_error("no solid in it could be read"));
    }
    Ok(out)
}
