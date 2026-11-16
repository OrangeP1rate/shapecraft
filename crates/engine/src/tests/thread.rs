//! Thread: bolts, nuts, and a bolt that fits its nut.

use serde_json::json;
use shapecraft_doc::ItemId;

use super::session;
use crate::Session;

/// A Ø`d` cylinder `h` tall standing on z = `z`, centred on (`x`, 0); its body id.
fn cylinder(s: &mut Session, x: f64, z: f64, d: f64, h: f64) -> ItemId {
    let k = s.execute("sketch", &json!({"origin": [0, 0, z], "normal": [0, 0, 1], "x_dir": [1, 0, 0]})).unwrap();
    let sk = k.value["sketch"].as_u64().unwrap();
    s.execute("circle", &json!({"center": [x, 0], "radius": d / 2.0})).unwrap();
    let r = s.execute("extrude", &json!({"sketch": sk, "region": 0, "distance": h, "result": "new"})).unwrap();
    ItemId(r.value["body"].as_u64().unwrap())
}

/// A 24 × 24 plate `t` thick on z = `z`, centred on (`x`, 0), with a Ø`d` hole through it; its body id.
fn plate(s: &mut Session, x: f64, z: f64, t: f64, d: f64) -> ItemId {
    let k = s.execute("sketch", &json!({"origin": [0, 0, z], "normal": [0, 0, 1], "x_dir": [1, 0, 0]})).unwrap();
    let sk = k.value["sketch"].as_u64().unwrap();
    s.execute("rectangle", &json!({"center": [x, 0], "size": [24, 24]})).unwrap();
    s.execute("circle", &json!({"center": [x, 0], "radius": d / 2.0})).unwrap();
    let ring = s.doc.sketch(ItemId(sk)).unwrap().regions().iter().max_by(|a, b| a.area.total_cmp(&b.area)).unwrap().index;
    let r = s.execute("extrude", &json!({"sketch": sk, "region": ring, "distance": t, "result": "new"})).unwrap();
    ItemId(r.value["body"].as_u64().unwrap())
}

/// The body's round faces (as the Thread tool reads them).
fn round_faces(s: &Session, body: ItemId) -> Vec<(u32, crate::cmd::ThreadFace)> {
    let n = s.kernel.face_count(&s.doc.body(body).unwrap().shape).unwrap();
    (0..n).filter_map(|f| crate::cmd::thread_face(s, body, f).map(|t| (f, t))).collect()
}

fn radial_reach(s: &Session, body: ItemId, cx: f64) -> (f64, f64) {
    let m = &s.doc.body(body).unwrap().mesh;
    let (mut lo, mut hi) = (f64::MAX, 0.0_f64);
    for p in &m.positions {
        let r = (f64::from(p[0]) - cx).hypot(f64::from(p[1]));
        lo = lo.min(r);
        hi = hi.max(r);
    }
    (lo, hi)
}

#[test]
fn a_bolt_gets_m10_and_history_can_change_it() {
    let mut s = session();
    let bolt = cylinder(&mut s, 0.0, 0.0, 10.0, 20.0);
    let faces = round_faces(&s, bolt);
    assert_eq!(faces.len(), 1);
    let (face, f) = faces[0];
    assert!(!f.internal && (f.radius - 5.0).abs() < 1e-6 && (f.hi - f.lo - 20.0).abs() < 1e-6, "{f:?}");
    let before = s.kernel.mass_props(&s.doc.body(bolt).unwrap().shape).unwrap().volume;
    let r = s.execute("thread", &json!({"body": bolt.0, "faces": [face]})).unwrap();
    assert_eq!(r.value["size"], "M10");
    assert_eq!(r.value["pitch"], 1.5);
    let after = s.kernel.mass_props(&s.doc.body(bolt).unwrap().shape).unwrap().volume;
    // Between the core (Ø8.14) and the full Ø10 rod.
    let core = std::f64::consts::PI * 4.07_f64.powi(2) * 20.0;
    assert!(after < before && after > core, "{core} < {after} < {before}");
    let (_, out) = radial_reach(&s, bolt, 0.0);
    assert!((out - 4.925).abs() < 0.02, "crests at {out}");
    let step = s.doc.history.iter().position(|h| h.kind == "thread").unwrap();
    assert_eq!(s.doc.history[step].label, "Thread 01");
    s.execute("edit_step", &json!({"step": step, "params": {"size": "M8"}})).unwrap();
    let (_, out) = radial_reach(&s, bolt, 0.0);
    assert!(out < 4.0, "M8 crests at {out}");
}

/// How far each of a body's points at radius `r` (± `band`) is from the shared helix's tooth line (right hand,
/// pitch `p`: z = θ/2π · p on whole pitches), worst first.
fn off_helix(s: &Session, body: ItemId, r: f64, band: f64, p: f64) -> (usize, f64) {
    let m = &s.doc.body(body).unwrap().mesh;
    let (mut n, mut worst) = (0, 0.0_f64);
    for q in &m.positions {
        let (x, y, z) = (f64::from(q[0]), f64::from(q[1]), f64::from(q[2]));
        if (x.hypot(y) - r).abs() > band {
            continue;
        }
        let th = y.atan2(x).rem_euclid(std::f64::consts::TAU);
        let phase = (z - th / std::f64::consts::TAU * p).rem_euclid(p);
        let off = phase.min(p - phase);
        n += 1;
        worst = worst.max(off);
    }
    (n, worst)
}

/// The point of the shared helix: a bolt and a nut threaded on their own, of different lengths and starting at
/// different heights, still screw together. The bolt's crests and the nut's roots lie on one helix (each within
/// half its flat), so the bolt's teeth run in the nut's grooves.
#[test]
fn a_bolt_fits_the_nut_made_for_it() {
    let mut s = session();
    let bolt = cylinder(&mut s, 0.0, 0.0, 10.0, 20.0);
    let nut = plate(&mut s, 40.0, 6.3, 8.0, 8.5);
    let bf = round_faces(&s, bolt)[0].0;
    let (nf, f) = round_faces(&s, nut)[0];
    assert!(f.internal, "{f:?}");
    s.execute("thread", &json!({"body": bolt.0, "faces": [bf], "size": "M10"})).unwrap();
    // The nut's own axis line: moved onto the bolt's afterwards, the helix must still match.
    let r = s.execute("thread", &json!({"body": nut.0, "faces": [nf], "size": "M10"})).unwrap();
    assert_eq!(r.value["internal"], true);
    assert!((r.value["minor_diameter"].as_f64().unwrap() - 8.526).abs() < 0.01);
    let bolt_pr = crate::thread::profile(crate::thread::Standard::Iso, 10.0, 1.5, false, 0.0);
    let nut_pr = crate::thread::profile(crate::thread::Standard::Iso, 10.0, 1.5, true, 0.0);
    let (n, worst) = off_helix(&s, bolt, bolt_pr.major, 0.003, 1.5);
    assert!(n > 20 && worst <= bolt_pr.width_at(bolt_pr.major, false) / 2.0 + 0.02, "bolt crests: {n} points, {worst} off");
    s.execute("translate", &json!({"items": [nut.0], "by": [-40, 0, 0]})).unwrap();
    let (n, worst) = off_helix(&s, nut, nut_pr.major, 0.003, 1.5);
    assert!(n > 20 && worst <= nut_pr.width_at(nut_pr.major, true) / 2.0 + 0.02, "nut roots: {n} points, {worst} off");
    // The nut's crests clear the bolt's roots, and the bolt's crests the nut's roots.
    assert!(nut_pr.minor > bolt_pr.minor && nut_pr.major > bolt_pr.major);
}

#[test]
fn custom_length_from_either_end_keeps_the_rest_of_the_face() {
    let mut s = session();
    let bolt = cylinder(&mut s, 0.0, 0.0, 10.0, 20.0);
    let face = round_faces(&s, bolt)[0].0;
    s.execute("thread", &json!({"body": bolt.0, "faces": [face], "extent": "custom", "length": 8, "flip": true})).unwrap();
    // The shank keeps its Ø10; the thread is in the top 8.
    let m = &s.doc.body(bolt).unwrap().mesh;
    let (mut low_r, mut high_r) = (0.0_f64, 0.0_f64);
    for p in &m.positions {
        let r = f64::from(p[0]).hypot(f64::from(p[1]));
        if p[2] < 11.5 {
            low_r = low_r.max(r)
        } else if p[2] > 12.5 {
            high_r = high_r.max(r)
        }
    }
    assert!((low_r - 5.0).abs() < 1e-3, "shank {low_r}");
    assert!((high_r - 4.925).abs() < 0.02, "thread {high_r}");
    // A left-hand ANSI one on a fresh rod builds too.
    let rod = cylinder(&mut s, 30.0, 0.0, 12.7, 15.0);
    let face = round_faces(&s, rod)[0].0;
    let r = s.execute("thread", &json!({"body": rod.0, "faces": [face], "standard": "ansi", "hand": "left", "clearance": 0.2})).unwrap();
    assert_eq!(r.value["size"], "1/2\"");
    let (_, out) = radial_reach(&s, rod, 30.0);
    assert!(out < 6.35 - 0.2, "clearance takes the crests in: {out}");
}

#[test]
fn thread_refuses_what_it_cannot_do() {
    let mut s = session();
    let bolt = cylinder(&mut s, 0.0, 0.0, 10.0, 20.0);
    let face = round_faces(&s, bolt)[0].0;
    let flat = (0..3).find(|f| *f != face).unwrap();
    for bad in [
        json!({"body": bolt.0, "faces": [flat]}),
        json!({"body": bolt.0, "faces": [face], "size": "M7.77"}),
        json!({"body": bolt.0, "faces": [face], "standard": "bspp"}),
        json!({"body": bolt.0, "faces": [face], "pitch": -1}),
        json!({"body": bolt.0, "faces": [face], "pitch": "fine"}),
        json!({"body": bolt.0, "faces": [face], "hand": "up"}),
        json!({"body": bolt.0, "faces": [face], "extent": "custom", "length": 0}),
        json!({"body": bolt.0, "faces": [face], "clearance": 9}),
        json!({"body": bolt.0, "faces": []}),
    ] {
        assert!(s.execute("thread", &bad).is_err(), "{bad}");
    }
}

/// Sizes off the face's own diameter build too (the thread replaces the face, so a bigger size adds material),
/// and a Custom Length as long as the face is the whole face.
#[test]
fn bigger_and_smaller_sizes_and_a_full_custom_length() {
    for (size, extent) in [("M12", "full"), ("M8", "full"), ("M10", "custom"), ("M12", "custom")] {
        let mut s = session();
        let bolt = cylinder(&mut s, 0.0, 0.0, 10.0, 20.0);
        let face = round_faces(&s, bolt)[0].0;
        let r = s.execute("thread", &json!({"body": bolt.0, "faces": [face], "size": size, "extent": extent, "length": 20}));
        assert!(r.is_ok(), "{size} {extent}: {r:?}");
        let (_, out) = radial_reach(&s, bolt, 0.0);
        let major = crate::thread::Standard::Iso.size(size).unwrap().major;
        assert!((out - major / 2.0).abs() < 0.2, "{size} {extent}: crests at {out}");
    }
}
