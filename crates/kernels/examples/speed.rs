//! The same modelling work timed on every kernel this build has, side by side.
//!
//! `cargo run --release -p shapecraft-kernels --example speed [REPEATS]`
//!
//! Each case runs once to warm up, then REPEATS times (default 5); the table shows the median in milliseconds and how
//! many times slower (>1) or faster (<1) the Rust kernel is than OCCT. A case a kernel refuses shows its error.

use std::time::Instant;

use shapecraft_kernel::{Axis, BoolOp, DVec3, Kernel, PlanarCurve, PlaneFrame, Result, Shape, ViewFrame};

/// A case: its name, what it works on (made before timing), and the work timed.
type Case = (&'static str, fn(&dyn Kernel) -> Result<Vec<Shape>>, fn(&dyn Kernel, &[Shape]) -> Result<()>);

fn block(k: &dyn Kernel) -> Result<Shape> {
    k.make_box(DVec3::ZERO, DVec3::new(40.0, 30.0, 20.0))
}

/// The block with a hole through it and its top edges rounded: a typical part.
fn part(k: &dyn Kernel) -> Result<Shape> {
    let holed = k.boolean(BoolOp::Subtract, &block(k)?, &k.extrude_circle(DVec3::new(20.0, 15.0, -1.0), 6.0, DVec3::Z * 22.0)?)?;
    let top: Vec<u32> = (0..k.edge_count(&holed)?)
        .filter(|&e| matches!(k.edge_curve(&holed, e), Ok(shapecraft_kernel::EdgeCurve::Line { a, b }) if a.z > 19.9 && b.z > 19.9))
        .collect();
    k.fillet_edges(&holed, &top, 3.0)
}

fn frame() -> PlaneFrame {
    PlaneFrame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y }
}

fn square(k: &dyn Kernel, z: f64, h: f64) -> Result<Shape> {
    let p = [(-h, -h), (h, -h), (h, h), (-h, h)].map(|(x, y)| DVec3::new(x, y, z));
    let curves: Vec<PlanarCurve> = (0..4).map(|i| PlanarCurve::Line { a: p[i], b: p[(i + 1) % 4] }).collect();
    let f = PlaneFrame { origin: DVec3::new(0.0, 0.0, z), ..frame() };
    k.planar_regions(f, &curves)?.into_iter().next().ok_or_else(|| shapecraft_kernel::KernelError::InvalidInput("no region".into()))
}

/// What a case is timed on, made once before the clock starts.
fn none(_: &dyn Kernel) -> Result<Vec<Shape>> {
    Ok(Vec::new())
}

fn the_part(k: &dyn Kernel) -> Result<Vec<Shape>> {
    Ok(vec![part(k)?])
}

fn the_block(k: &dyn Kernel) -> Result<Vec<Shape>> {
    Ok(vec![block(k)?])
}

/// A plate and 40 separate pins standing through it.
fn plate_and_pins(k: &dyn Kernel) -> Result<Vec<Shape>> {
    let mut out = vec![k.make_box(DVec3::new(-8.0, -8.0, 0.0), DVec3::new(100.0, 64.0, 5.0))?];
    for i in 0..40 {
        out.push(k.extrude_circle(DVec3::new((i % 8) as f64 * 12.0, (i / 8) as f64 * 12.0, -1.0), 4.0, DVec3::Z * 7.0)?);
    }
    Ok(out)
}

fn top_face(k: &dyn Kernel, s: &Shape) -> u32 {
    (0..k.face_count(s).unwrap_or(0))
        .find(|&f| k.face_plane(s, f).ok().flatten().is_some_and(|p| p.normal.z > 0.9 && p.origin.z > 19.0))
        .unwrap_or(0)
}

const CASES: &[Case] = &[
    ("box − cylinder", none, |k, _| {
        k.boolean(BoolOp::Subtract, &block(k)?, &k.extrude_circle(DVec3::new(20.0, 15.0, -1.0), 6.0, DVec3::Z * 22.0)?).map(drop)
    }),
    ("fillet 12 box edges", the_block, |k, s| k.fillet_edges(&s[0], &(0..12).collect::<Vec<_>>(), 2.0).map(drop)),
    ("chamfer 12 box edges", the_block, |k, s| k.chamfer_edges(&s[0], &(0..12).collect::<Vec<_>>(), 2.0).map(drop)),
    ("typical part (hole + 4 fillets)", none, |k, _| part(k).map(drop)),
    ("shell, top open", the_block, |k, s| k.shell(&s[0], &[top_face(k, &s[0]).min(5)], 2.0).map(drop)),
    ("sketch: 40 circles in a grid → regions", none, |k, _| {
        let mut curves: Vec<PlanarCurve> = (0..40)
            .map(|i| PlanarCurve::Circle { center: DVec3::new((i % 8) as f64 * 12.0, (i / 8) as f64 * 12.0, 0.0), radius: 4.0 })
            .collect();
        let r = [(-8.0, -8.0), (92.0, -8.0), (92.0, 56.0), (-8.0, 56.0)].map(|(x, y)| DVec3::new(x, y, 0.0));
        curves.extend((0..4).map(|i| PlanarCurve::Line { a: r[i], b: r[(i + 1) % 4] }));
        k.planar_regions(frame(), &curves).map(drop)
    }),
    ("40 holes: pins cut one by one", plate_and_pins, |k, s| {
        s[1..].iter().try_fold(s[0].clone(), |plate, pin| k.boolean(BoolOp::Subtract, &plate, pin)).map(drop)
    }),
    ("40 holes: pins joined, then one cut", plate_and_pins, |k, s| {
        let pins = s[2..].iter().try_fold(s[1].clone(), |all, pin| k.boolean(BoolOp::Union, &all, pin))?;
        k.boolean(BoolOp::Subtract, &s[0], &pins).map(drop)
    }),
    ("40 holes: pins as tools of one cut (a pattern)", plate_and_pins, |k, s| k.boolean_many(BoolOp::Subtract, &s[0], &s[1..]).map(drop)),
    ("extrude with 5° draft", none, |k, _| k.prism_draft(&square(k, 0.0, 10.0)?, DVec3::Z * 15.0, 5f64.to_radians()).map(drop)),
    ("revolve a square a full turn", none, |k, _| {
        let moved = k.translate(&square(k, 0.0, 3.0)?, DVec3::X * 10.0)?;
        k.revolve(&moved, Axis::new(DVec3::ZERO, DVec3::Y), std::f64::consts::TAU).map(drop)
    }),
    ("loft two squares", none, |k, _| k.loft(&[square(k, 0.0, 10.0)?, square(k, 20.0, 5.0)?]).map(drop)),
    ("thread: helical revolve, 5 turns", none, |k, _| {
        let s = k.translate(&square(k, 0.0, 0.8)?, DVec3::X * 6.0)?;
        let s = k.rotate(&s, Axis::new(DVec3::ZERO, DVec3::X), std::f64::consts::FRAC_PI_2)?;
        k.revolve_helix(&s, Axis::new(DVec3::ZERO, DVec3::Z), std::f64::consts::TAU * 5.0, 10.0).map(drop)
    }),
    ("display mesh of the part", the_part, |k, s| k.tessellate(&s[0], 0.05).map(drop)),
    // On a fresh copy each time: a kernel may remember what it measured of a shape.
    ("volume of the part (first time)", the_part, |k, s| k.mass_props(&k.translate(&s[0], DVec3::X)?).map(drop)),
    ("bounds of the part (first time)", the_part, |k, s| k.bounds(&k.translate(&s[0], DVec3::X)?).map(drop)),
    ("STEP write + read the part", the_part, |k, s| {
        let path = std::env::temp_dir().join(format!("speed-{}-{}.step", k.name(), std::process::id()));
        k.write_step(&s[..1], &path)?;
        let back = k.read_step(&path);
        let _ = std::fs::remove_file(&path);
        back.map(drop)
    }),
    ("drawing view (hidden lines)", the_part, |k, s| {
        let view = ViewFrame { origin: DVec3::ZERO, toward: DVec3::new(1.0, -1.0, 1.0), x: DVec3::new(1.0, 1.0, 0.0) };
        k.hidden_lines(&s[..1], view, 0.05).map(drop)
    }),
    ("Parasolid round trip (to_brep → from_brep)", the_part, |k, s| k.to_brep(&s[0])?.iter().try_for_each(|b| k.from_brep(b).map(drop))),
    ("push/pull the part's top face 2 mm", the_part, |k, s| k.extrude_face(&s[0], top_face(k, &s[0]), 2.0).map(drop)),
    ("offset the part's top face 2 mm", the_part, |k, s| k.offset_faces(&s[0], &[top_face(k, &s[0])], 2.0).map(drop)),
    ("scale the part ×2", the_part, |k, s| k.scale(&s[0], DVec3::ZERO, DVec3::splat(2.0)).map(drop)),
    ("move the part", the_part, |k, s| k.translate(&s[0], DVec3::X * 5.0).map(drop)),
];

fn median(mut t: Vec<f64>) -> f64 {
    t.sort_by(f64::total_cmp);
    t[t.len() / 2]
}

fn time(k: &dyn Kernel, case: &Case, repeats: usize) -> std::result::Result<f64, String> {
    let input = (case.1)(k).map_err(|e| format!("setting up: {e}"))?;
    (case.2)(k, &input).map_err(|e| e.to_string())?;
    let mut t = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let at = Instant::now();
        (case.2)(k, &input).map_err(|e| e.to_string())?;
        t.push(at.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(median(t))
}

fn main() {
    let repeats = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(5usize).clamp(1, 1000);
    let kernels: Vec<_> = shapecraft_kernels::names().iter().filter_map(|n| shapecraft_kernels::make(n).ok()).collect();
    print!("{:<44}", "case (median ms)");
    for k in &kernels {
        print!("{:>12}", k.name());
    }
    println!("{:>12}", if kernels.len() == 2 { "rust/occt" } else { "" });
    for case in CASES {
        print!("{:<44}", case.0);
        let times: Vec<_> = kernels.iter().map(|k| time(k.as_ref(), case, repeats)).collect();
        for t in &times {
            match t {
                Ok(ms) => print!("{ms:>12.2}"),
                Err(_) => print!("{:>12}", "refused"),
            }
        }
        match times.as_slice() {
            [Ok(o), Ok(r)] => println!("{:>11.2}×", r / o),
            _ => println!(),
        }
        for (k, t) in kernels.iter().zip(&times) {
            if let Err(e) = t {
                println!("    {}: {e}", k.name());
            }
        }
    }
    shapecraft_kernels::finish_process(0);
}
