//! `shaprdump FILE.shapr`: what we understand of a Shapr3D project.
fn main() {
    let Some(path) = std::env::args().nth(1) else { return eprintln!("usage: shaprdump FILE") };
    let f = match shapecraft_shapr::ShaprFile::read(std::path::Path::new(&path)) {
        Ok(f) => f,
        Err(e) => return eprintln!("{e}"),
    };
    println!("metadata {}", f.metadata);
    let p = match shapecraft_shapr::Project::read(&f) {
        Ok(p) => p,
        Err(e) => return eprintln!("{e}"),
    };
    for (k, v) in &p.settings {
        println!("  {k} = {v}");
    }
    for s in &p.steps {
        println!("step #{} {:?} {}", s.node, s.name, s.op);
        for (id, v) in &s.params {
            println!("    #{id} {v}");
        }
    }
    for f in &p.folders {
        println!("folder {} {:?} hidden {} {:?}", f.id, f.name, f.hidden, f.children);
    }
    println!("names {:?}", p.item_names);
    for b in &p.bodies {
        println!(
            "body {:?} colour {:?} painted {} hidden {} in {:?} partition {:?}: {} faces {} edges",
            b.name,
            b.color.map(|c| c.map(|x| (x * 255.0).round() as u8)),
            b.brep.faces.iter().filter(|f| f.color.is_some()).count(),
            b.hidden,
            b.folder,
            b.partition,
            b.brep.faces.len(),
            b.brep.edges.len()
        );
    }
    if std::env::var_os("FACE_COLOURS").is_some() {
        for b in &p.bodies {
            let mut m = std::collections::BTreeMap::new();
            for f in &b.brep.faces {
                *m.entry(f.color.or(b.color).map(|c| c.map(|x| (x * 255.0).round() as u8))).or_insert(0) += 1;
            }
            println!("faces {:?} {:?}", b.name, m);
            if b.brep.faces.len() <= 1 {
                println!("  vertices {:?}", b.brep.vertices);
            }
        }
    }
    for k in &p.sketches {
        println!("sketch {} {:?} hidden {} curves {}", k.id, k.name, k.hidden, k.curves.len());
    }
    for c in &p.construction {
        println!("construction {c:?}");
    }
    for u in &p.unreadable {
        println!("unreadable {u}");
    }
}
