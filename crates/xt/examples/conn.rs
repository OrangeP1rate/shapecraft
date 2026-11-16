//! `conn FILE`: each body's faces grouped by shared edges, and edges not used exactly twice (a reader check).
fn main() {
    let Some(p) = std::env::args().nth(1) else { return };
    let f = shapecraft_xt::parse(&std::fs::read(p).unwrap_or_default()).expect("parse");
    for b in shapecraft_xt::bodies(&f).expect("bodies") {
        let b = b.brep;
        let mut users: Vec<Vec<usize>> = vec![Vec::new(); b.edges.len()];
        for (fi, face) in b.faces.iter().enumerate() {
            for fin in face.loops.iter().flatten() {
                users[fin.edge].push(fi);
            }
        }
        let odd: Vec<(usize, &Vec<usize>)> = users.iter().enumerate().filter(|(_, u)| u.len() != 2).collect();
        println!("faces {} edges {}, edges not used twice: {}", b.faces.len(), b.edges.len(), odd.len());
        for (e, u) in odd.iter().take(10) {
            println!("  edge {e} {:?} used by {u:?}", b.edges[*e]);
        }
        let mut dir: Vec<Vec<bool>> = vec![Vec::new(); b.edges.len()];
        for face in &b.faces {
            for fin in face.loops.iter().flatten() {
                dir[fin.edge].push(fin.forward);
            }
        }
        let same: Vec<usize> = dir.iter().enumerate().filter(|(_, d)| d.len() == 2 && d[0] == d[1]).map(|(i, _)| i).collect();
        let kind = |s: &shapecraft_kernel::brep::Surface| format!("{s:?}").split([' ', '(']).next().unwrap_or("").to_string();
        println!("edges whose two fins run the same way: {}", same.len());
        for e in same.iter().take(12) {
            let fs: Vec<String> =
                users[*e].iter().map(|&f| format!("F{f} {} flip={}", kind(&b.surfaces[b.faces[f].surface]), b.faces[f].flip)).collect();
            println!("  edge {e}: {fs:?}");
        }
        let mut comp: Vec<usize> = (0..b.faces.len()).collect();
        fn root(c: &mut [usize], mut i: usize) -> usize {
            while c[i] != i {
                c[i] = c[c[i]];
                i = c[i];
            }
            i
        }
        for u in &users {
            for w in u.windows(2) {
                let (a, c) = (root(&mut comp, w[0]), root(&mut comp, w[1]));
                comp[a] = c;
            }
        }
        let mut sizes = std::collections::BTreeMap::new();
        for i in 0..b.faces.len() {
            *sizes.entry(root(&mut comp, i)).or_insert(0) += 1;
        }
        println!("components {:?}", sizes.values().collect::<Vec<_>>());
    }
}
