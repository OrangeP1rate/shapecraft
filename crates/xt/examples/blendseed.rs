//! `blendseed FILE S0 OFF0 S1 OFF1 X Y Z [DX DY DZ N]`: where a seed lands on two offset supports (`on_both`), and
//! the foot points on each; with a direction, N seeds stepped along it.
use shapecraft_kernel::DVec3;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = shapecraft_xt::parse(&std::fs::read(&a[1]).unwrap_or_default()).expect("parse");
    let num = |i: usize| a.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let (s0, o0, s1, o1) = (num(2) as usize, num(3), num(4) as usize, num(5));
    let p = DVec3::new(num(6), num(7), num(8));
    let (d, n) = (DVec3::new(num(9), num(10), num(11)), num(12).max(1.0) as usize);
    for b in shapecraft_xt::bodies(&f).expect("bodies").into_iter().take(1) {
        let s = &b.brep.surfaces;
        println!("s0 {}", format!("{:?}", s[s0]).chars().take(400).collect::<String>());
        for k in 0..n {
            let q = p + d * k as f64;
            let both = shapecraft_xt::blend::on_both(
                s,
                shapecraft_xt::blend::Support::Surface(s0, o0),
                shapecraft_xt::blend::Support::Surface(s1, o1),
                q,
            );
            let f0 = shapecraft_xt::blend::project(s, s0, q);
            println!(
                "seed {q:.4?} on_both {both:.5?} moved {:?} | foot0 {:.4?} dist {:?}",
                both.map(|b| b.distance(q)),
                f0.map(|f| f.0),
                f0.map(|f| f.0.distance(q))
            );
        }
    }
}
