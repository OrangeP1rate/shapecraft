//! `native FILE…`: the Parasolid bodies inside NX, Solid Edge and JT files.
fn main() {
    use shapecraft_xt::native::{Native, jt, kind, nx, solid_edge};
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let k = kind(&bytes);
        let files = match k {
            Some(Native::Nx) => nx(&bytes),
            Some(Native::SolidEdge) => solid_edge(&bytes).unwrap_or_default(),
            Some(Native::Jt) => match jt(&bytes) {
                Ok(j) => {
                    println!("{path}: {} meshes {} jt-breps {}", j.version, j.meshes, j.jt_breps);
                    j.parts
                }
                Err(e) => {
                    println!("{path}: {e}");
                    continue;
                }
            },
            None => Vec::new(),
        };
        println!("{path}: {k:?}, {} transmit files", files.len());
        for ps in files {
            let summary = match shapecraft_xt::parse(&ps).and_then(|f| shapecraft_xt::bodies(&f)) {
                Ok(b) => format!("{} bodies, faces {:?}", b.len(), b.iter().map(|b| b.brep.faces.len()).collect::<Vec<_>>()),
                Err(e) => format!("ERROR {e}"),
            };
            println!("  {} bytes: {summary}", ps.len());
        }
    }
}
