//! `sldprt PART.sldprt… [--out DIR]`: list each part's Parasolid partitions and the model's bodies; with --out,
//! write the model partition as DIR/<part>.x_b.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).cloned();
    for path in args.iter().filter(|a| a.as_str() != "--out" && Some(*a) != out.as_ref()) {
        let bytes = std::fs::read(path).unwrap_or_default();
        println!("{path}: solidworks {}", shapecraft_xt::sldprt::is_solidworks(&bytes));
        for p in shapecraft_xt::sldprt::partitions(&bytes) {
            let summary = match shapecraft_xt::parse(&p.bytes).and_then(|f| shapecraft_xt::bodies(&f)) {
                Ok(b) => format!("{} bodies, faces {:?}", b.len(), b.iter().map(|b| b.brep.faces.len()).collect::<Vec<_>>()),
                Err(e) => format!("ERROR {e}"),
            };
            println!("  {:?} {} ({} bytes): {summary}", p.kind, p.name, p.bytes.len());
        }
        if let (Some(dir), Some(m)) = (&out, shapecraft_xt::sldprt::model(&bytes)) {
            let stem = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().into_owned();
            std::fs::write(format!("{dir}/{stem}.x_b"), &m.bytes).unwrap();
        }
    }
}
