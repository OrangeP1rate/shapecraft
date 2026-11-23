//! Sketching: curves and the regions they make, sketch edits, offsets, mirrors and constraints.

use serde_json::json;
use shapecraft_doc::Pick;

use super::{session, volume};
use crate::Session;

#[test]
fn pentagon_extrudes_to_the_right_volume() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("polygon", &json!({"center": [0, 0], "radius": 10, "sides": 5})).unwrap();
    s.execute("extrude", &json!({"sketch": 1, "region": 0, "distance": 4})).unwrap();
    let area = 2.5 * 100.0 * 72f64.to_radians().sin();
    assert!((volume(&mut s) - area * 4.0).abs() < 1e-6);
}

#[test]
fn a_circle_inside_a_rectangle_gives_a_ring_and_a_disc() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [20, 10]})).unwrap();
    s.execute("circle", &json!({"center": [10, 5], "radius": 3})).unwrap();
    let sk = s.active_sketch().unwrap();
    assert_eq!(sk.regions().len(), 2);
    assert!(sk.regions()[0].area > sk.regions()[1].area, "the ring comes first");
    // Extruding the ring gives a plate with a hole; the disc separately gives a pin.
    s.execute("extrude", &json!({"sketch": 1, "region": 0, "distance": 2})).unwrap();
    let disc = std::f64::consts::PI * 9.0;
    assert!((volume(&mut s) - (200.0 - disc) * 2.0).abs() < 1e-6);
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [30, 0], "b": [40, 0]})).unwrap();
    s.execute("arc", &json!({"a": [40, 0], "mid": [35, 5], "b": [30, 0]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().regions().len(), 1, "a line and an arc close a half disc");
}

#[test]
fn ellipse_extrudes_exactly() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("ellipse", &json!({"center": [0, 0], "rx": 6, "ry": 3, "angle": 30})).unwrap();
    s.execute("extrude", &json!({"sketch": 1, "region": 0, "distance": 2})).unwrap();
    assert!((volume(&mut s) - std::f64::consts::PI * 18.0 * 2.0).abs() < 1e-6);
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    assert!(s.execute("spline", &json!({"points": [[0, 0]]})).is_err());
    s.execute("spline", &json!({"points": [[10, 0], [12, 3], [15, 0]]})).unwrap();
}

#[test]
fn control_spline_closes_a_region() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    // Three poles make a parabola: the area under it is 2/3 of the control triangle's.
    s.execute("spline", &json!({"points": [[0, 0], [5, 8], [10, 0]], "control": true})).unwrap();
    s.execute("line", &json!({"a": [10, 0], "b": [0, 0]})).unwrap();
    s.execute("extrude", &json!({"sketch": 1, "region": 0, "distance": 1})).unwrap();
    assert!((volume(&mut s) - 80.0 / 3.0).abs() < 1e-6);
    let pts = shapecraft_doc::bspline_points(&[glam::DVec2::ZERO, glam::DVec2::new(5.0, 8.0), glam::DVec2::new(10.0, 0.0)], 8);
    assert!(pts[0].distance(glam::DVec2::ZERO) < 1e-12 && pts[pts.len() - 1].distance(glam::DVec2::new(10.0, 0.0)) < 1e-12);
    assert!((pts[pts.len() / 2].y - 4.0).abs() < 1e-9, "the display curve matches the kernel's");
}

#[test]
fn trim_and_offset_sketch_curves() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [10, 10]})).unwrap();
    s.execute("line", &json!({"a": [5, -2], "b": [5, 12]})).unwrap();
    // Two regions; trimming the line's ends leaves it splitting the square.
    assert_eq!(s.active_sketch().unwrap().regions().len(), 2);
    // The rectangle is four lines (0–3); the line is 4.
    s.execute("trim", &json!({"entity": 4, "at": [5, 11]})).unwrap();
    s.execute("trim", &json!({"entity": 4, "at": [5, -1]})).unwrap();
    let sk = s.active_sketch().unwrap();
    assert_eq!(sk.entities[4], shapecraft_doc::SketchEntity::Line { a: glam::DVec2::new(5.0, 0.0), b: glam::DVec2::new(5.0, 10.0) });
    assert_eq!(sk.regions().len(), 2);
    let sketch = sk.id;
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": sketch.0, "entity": 0}]})).unwrap();
    s.execute("offset_edge", &json!({"distance": 2})).unwrap();
    let sk = s.active_sketch().unwrap();
    assert_eq!(sk.entities.len(), 6);
    assert!(matches!(s.selection[..], [Pick::SketchEntity { entity: 5, .. }]));
    assert!(s.execute("offset_edge", &json!({"distance": 0})).is_err());
}

/// #90: half of a rectangle's side comes off and the rest of the rectangle stays where it is.
#[test]
fn trimming_half_a_side_leaves_the_neighbours_alone() {
    use glam::DVec2;
    use shapecraft_doc::{Constraint, SketchEntity as E};
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [40, 20]})).unwrap();
    s.execute("line", &json!({"a": [0, 10], "b": [40, 10]})).unwrap();
    s.execute("circle", &json!({"center": [0, 10], "radius": 10})).unwrap();
    let before = s.active_sketch().unwrap().entities.clone();
    s.execute("trim", &json!({"entity": 1, "at": [40, 15]})).unwrap();
    let sk = s.active_sketch().unwrap();
    assert_eq!(sk.entities[1], E::Line { a: DVec2::new(40.0, 0.0), b: DVec2::new(40.0, 10.0) });
    for i in [0, 2, 3, 4, 5] {
        assert_eq!(sk.entities[i], before[i], "curve {i} moved");
    }
    // The corner that is left still holds, and so do the sides' directions.
    let holds = |want: &Constraint| sk.constraints.contains(want);
    let point = |entity, index| shapecraft_doc::Element::Point { entity, index };
    let side = |entity| shapecraft_doc::Element::Segment { entity, index: 0 };
    assert!(holds(&Constraint::Coincident { a: point(0, 1), b: point(1, 0) }));
    assert!(!holds(&Constraint::Coincident { a: point(1, 1), b: point(2, 0) }));
    assert!(holds(&Constraint::Vertical { a: side(1) }) && holds(&Constraint::Horizontal { a: side(2) }));
    // Undo brings the joined corner back.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.active_sketch().unwrap().constraints.len(), 8 + 2);
}

/// A trim that splits or removes a curve keeps what still holds on the others and on what is left.
#[test]
fn trims_carry_constraints_and_construction_over() {
    use shapecraft_doc::{Constraint, Element};
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [1, 1], "opposite": [41, 21]})).unwrap();
    s.execute("line", &json!({"a": [11, -5], "b": [11, 30]})).unwrap();
    s.execute("line", &json!({"a": [31, -5], "b": [31, 30], "construction": true})).unwrap();
    s.execute("circle", &json!({"center": [60, 10], "radius": 5})).unwrap();
    // The top side's middle goes: two pieces, both level, each still joined to its own corner.
    let before = s.active_sketch().unwrap().clone();
    s.execute("trim", &json!({"entity": 2, "at": [21, 21]})).unwrap();
    let sk = s.active_sketch().unwrap().clone();
    assert_eq!(sk.entities.len(), before.entities.len() + 1);
    assert_eq!(sk.entities[4..], before.entities[3..]);
    let side = |entity| Element::Segment { entity, index: 0 };
    let point = |entity, index| Element::Point { entity, index };
    for c in [
        Constraint::Horizontal { a: side(2) },
        Constraint::Horizontal { a: side(3) },
        Constraint::Vertical { a: side(4) },
        Constraint::Coincident { a: point(1, 1), b: point(2, 0) },
        Constraint::Coincident { a: point(3, 1), b: point(4, 0) },
    ] {
        assert!(sk.constraints.contains(&c), "{c:?}");
    }
    assert_eq!(sk.construction.iter().copied().collect::<Vec<_>>(), vec![6], "the construction line kept its mark");
    // A construction line cut in two is two construction lines.
    s.execute("trim", &json!({"entity": 6, "at": [31, 10]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().construction.iter().copied().collect::<Vec<_>>(), vec![6, 7]);
    // A circle with nothing across it goes whole; nothing else moves.
    let before = s.active_sketch().unwrap().entities.clone();
    let last = before.len() - 1;
    s.execute("trim", &json!({"entity": last, "at": [65, 10]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities[..], before[..last]);
}

/// A dimension on what was cut doesn't hold any more: it goes instead of pulling the line back out.
#[test]
fn trimming_a_dimensioned_line_drops_the_dimension() {
    use glam::DVec2;
    use shapecraft_doc::{Constraint, SketchEntity as E};
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [3, 3], "b": [43, 3]})).unwrap();
    s.execute("line", &json!({"a": [23, -10], "b": [23, 10]})).unwrap();
    let id = s.active_sketch().unwrap().id.0;
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": id, "entity": 0}]})).unwrap();
    s.execute("constrain", &json!({"kind": "distance", "value": 40})).unwrap();
    assert!(s.active_sketch().unwrap().constraints.iter().any(|c| matches!(c, Constraint::Distance { .. })));
    s.execute("trim", &json!({"entity": 0, "at": [33, 3]})).unwrap();
    let sk = s.active_sketch().unwrap();
    assert_eq!(sk.entities[0], E::Line { a: DVec2::new(3.0, 3.0), b: DVec2::new(23.0, 3.0) });
    assert_eq!(sk.entities[1], E::Line { a: DVec2::new(23.0, -10.0), b: DVec2::new(23.0, 10.0) });
    assert!(!sk.constraints.iter().any(|c| matches!(c, Constraint::Distance { .. })));
}

#[test]
fn a_slot_is_held_together() {
    use glam::DVec2;
    use shapecraft_doc::{Constraint, SketchEntity as E, constraints};
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    let v = s.execute("slot", &json!({"a": [3, 5], "b": [13, 5], "radius": 2})).unwrap().value;
    assert_eq!(v["entities"].as_array().unwrap().len(), 5);
    let sk = s.active_sketch().unwrap().clone();
    assert_eq!(sk.construction.iter().copied().collect::<Vec<_>>(), vec![4]);
    for c in &sk.constraints {
        assert!(constraints::residuals(&sk.entities, c).unwrap().iter().all(|r| r.abs() < 1e-9), "{c:?}");
    }
    let areas = |s: &Session| s.active_sketch().unwrap().regions().len();
    assert_eq!(areas(&s), 1, "the construction line doesn't split it");
    // The radius and the length are driven like any other size, and the slot follows.
    s.execute("constrain", &json!({"kind": "radius", "value": 3, "entities": [1]})).unwrap();
    s.execute("constrain", &json!({"kind": "distance", "value": 20, "entities": [4]})).unwrap();
    let sk = s.active_sketch().unwrap().clone();
    let E::Line { a, b } = sk.entities[4] else { panic!() };
    assert!((a.distance(b) - 20.0).abs() < 1e-5 && (a.y - b.y).abs() < 1e-5, "{a} {b}");
    for i in [1, 3] {
        assert!((constraints::curve(&sk.entities[i]).unwrap().1 - 3.0).abs() < 1e-5, "arc {i}");
    }
    let E::Line { a: t, .. } = sk.entities[0] else { panic!() };
    assert!((t.y - a.y - 3.0).abs() < 1e-5);
    assert!(sk.constraints.iter().any(|c| matches!(c, Constraint::Horizontal { .. })));
    // From its middle, turned; and the nonsense ones are refused.
    s.execute("slot", &json!({"middle": [50, 50], "b": [53, 54], "radius": 1})).unwrap();
    let E::Line { a, .. } = s.active_sketch().unwrap().entities[9] else { panic!() };
    assert!(a.distance(DVec2::new(47.0, 46.0)) < 1e-9, "{a}");
    assert!(s.execute("slot", &json!({"a": [0, 0], "b": [0, 0], "radius": 1})).is_err());
    assert!(s.execute("slot", &json!({"a": [0, 0], "b": [5, 0], "radius": 0})).is_err());
    s.execute("extrude", &json!({"sketch": 1, "region": 0, "distance": 4})).unwrap();
}

#[test]
fn extend_holds_the_new_end_on_what_it_reached() {
    use glam::DVec2;
    use shapecraft_doc::{Constraint, Element, SketchEntity as E};
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [1, 1], "b": [6, 1]})).unwrap();
    s.execute("line", &json!({"a": [10, -5], "b": [10, 5]})).unwrap();
    s.execute("circle", &json!({"center": [20, 1], "radius": 2})).unwrap();
    s.execute("constrain", &json!({"kind": "horizontal_vertical", "entities": [0]})).unwrap();
    s.execute("constrain", &json!({"kind": "distance", "value": 5, "entities": [0]})).unwrap();
    s.execute("extend", &json!({"entity": 0, "at": [5, 1]})).unwrap();
    let end = Element::Point { entity: 0, index: 1 };
    let sk = s.active_sketch().unwrap().clone();
    assert_eq!(sk.entities[0], E::Line { a: DVec2::new(1.0, 1.0), b: DVec2::new(10.0, 1.0) });
    assert!(sk.constraints.contains(&Constraint::Coincident { a: end, b: Element::Segment { entity: 1, index: 0 } }));
    assert!(sk.constraints.iter().any(|c| matches!(c, Constraint::Horizontal { .. })), "it's still level");
    assert!(!sk.constraints.iter().any(|c| matches!(c, Constraint::Distance { .. })), "its length isn't 5 any more");
    // On to the circle: the old hold is let go.
    s.execute("extend", &json!({"entity": 0, "at": [9, 1]})).unwrap();
    let sk = s.active_sketch().unwrap().clone();
    assert!(sk.constraints.contains(&Constraint::Coincident { a: end, b: Element::Curve { entity: 2 } }));
    assert!(!sk.constraints.contains(&Constraint::Coincident { a: end, b: Element::Segment { entity: 1, index: 0 } }));
    let E::Line { b, .. } = sk.entities[0] else { panic!() };
    assert!(b.distance(DVec2::new(18.0, 1.0)) < 1e-6, "{b}");
    assert!(s.execute("extend", &json!({"entity": 2, "at": [22, 1]})).is_err());
}

#[test]
fn move_mirror_and_pattern_sketch_curves() {
    use glam::DVec2;
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xz"})).unwrap();
    let sk = s.active_sketch.unwrap();
    s.execute("circle", &json!({"center": [5, 0], "radius": 1})).unwrap();
    let sel =
        |s: &mut Session, e: u32| s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": sk.0, "entity": e}]})).unwrap();
    sel(&mut s, 0);
    // World x is the xz sketch's u axis; world z its v axis.
    s.execute("translate", &json!({"by": [1, 0, 2]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities[0], SketchEntity::Circle { center: DVec2::new(6.0, 2.0), radius: 1.0 });
    s.execute("mirror", &json!({"origin": [0, 0, 0], "normal": [1, 0, 0]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities[1], SketchEntity::Circle { center: DVec2::new(-6.0, 2.0), radius: 1.0 });
    sel(&mut s, 0);
    s.execute("pattern", &json!({"direction": [0, 0, 1], "spacing": 3, "count": 3})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities.len(), 4);
    sel(&mut s, 0);
    s.execute("pattern_circular", &json!({"origin": [0, 0, 0], "axis": [0, 1, 0], "count": 4})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities.len(), 7);
    // Turned about an axis across the sketch, the picked curve leaves for a sketch of its own on the turned plane.
    s.execute("rotate", &json!({"origin": [0, 0, 0], "axis": [1, 0, 0], "angle": 90})).unwrap();
    assert_eq!(s.doc.sketch(sk).unwrap().entities.len(), 6);
    let turned = s.active_sketch().unwrap();
    assert_ne!(turned.id, sk);
    assert_eq!(turned.entities.len(), 1);
    assert!(turned.plane.normal().cross(glam::DVec3::Z).length() < 1e-9, "the xz plane turned about x faces z");
}

#[test]
fn curves_moved_off_their_plane_take_a_plane_along() {
    use glam::DVec3;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    let sk = s.active_sketch.unwrap();
    s.execute("rectangle", &json!({"corner": [20, -10], "opposite": [40, 10]})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap();
    let n = s.active_sketch().unwrap().entities.len() as u32;
    let circle = n - 1;
    let pick = |e: u32| json!({"kind": "sketch_entity", "sketch": sk.0, "entity": e});
    // In-plane moves of curves stay in the sketch's own step (no Movement step of their own).
    s.execute("select", &json!({"picks": [pick(circle)]})).unwrap();
    s.execute("translate", &json!({"by": [1, 0, 0]})).unwrap();
    assert_eq!(s.doc.history.len(), 1, "{:?}", s.doc.history.iter().map(|h| &h.label).collect::<Vec<_>>());
    // Only the circle lifted: it goes to a new sketch 8 mm up, which is now the open one; the rectangle stays.
    s.execute("select", &json!({"picks": [pick(circle)]})).unwrap();
    let out = s.execute("translate", &json!({"by": [0, 0, 8]})).unwrap();
    let new = s.active_sketch().unwrap();
    assert_ne!(new.id, sk);
    assert_eq!(out.value["name"], "Sketch 02");
    assert_eq!(new.entities.len(), 1);
    assert!((new.plane.origin - DVec3::new(0.0, 0.0, 8.0)).length() < 1e-9);
    assert_eq!(s.doc.sketch(sk).unwrap().entities.len(), n as usize - 1);
    assert!(s.selection.iter().all(|p| matches!(p, Pick::SketchEntity { sketch, .. } if *sketch == new.id)));
    let labels: Vec<String> = s.doc.history.iter().map(|h| h.label.clone()).collect();
    assert_eq!(labels, ["Sketch 01", "Sketch 02"]);
    // Undo puts the circle back; redo (a replay) makes the same new sketch again.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.doc.sketches.len(), 1);
    assert_eq!(s.doc.sketch(sk).unwrap().entities.len(), n as usize);
    s.execute("redo", &json!({})).unwrap();
    assert_eq!(s.doc.sketches.len(), 2);
    // A whole sketch picked (Items) lifts as one: its plane moves, no new sketch.
    let _ = s.execute("exit_sketching", &json!({}));
    s.execute("select", &json!({"picks": [{"kind": "sketch", "sketch": sk.0}]})).unwrap();
    s.execute("translate", &json!({"by": [0, 0, 3]})).unwrap();
    assert_eq!(s.doc.sketches.len(), 2);
    assert!((s.doc.sketch(sk).unwrap().plane.origin.z - 3.0).abs() < 1e-9);
    // And a region picked outside its sketch moves its curves in the plane.
    let before = s.doc.sketch(sk).unwrap().entities.clone();
    s.execute("select", &json!({"picks": [{"kind": "region", "sketch": sk.0, "region": 0}]})).unwrap();
    s.execute("translate", &json!({"by": [0, 5, 0]})).unwrap();
    assert_ne!(s.doc.sketch(sk).unwrap().entities, before);
    // Copy: lifted copies go to a new sketch and the originals stay; in the plane, the copies join the sketch.
    let count = s.doc.sketch(sk).unwrap().entities.len();
    s.execute("select", &json!({"picks": [{"kind": "sketch", "sketch": sk.0}]})).unwrap();
    s.execute("translate", &json!({"by": [0, 0, 6], "copy": true})).unwrap();
    assert_eq!(s.doc.sketches.len(), 3);
    assert_eq!(s.doc.sketch(sk).unwrap().entities.len(), count);
    assert!((s.doc.sketches[2].plane.origin.z - 9.0).abs() < 1e-9 && s.doc.sketches[2].entities.len() == count);
    s.execute("select", &json!({"picks": [{"kind": "sketch", "sketch": sk.0}]})).unwrap();
    s.execute("translate", &json!({"by": [50, 0, 0], "copy": true})).unwrap();
    assert_eq!(s.doc.sketch(sk).unwrap().entities.len(), count * 2);
}

#[test]
fn text_makes_regions() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    if !shapecraft_doc::text::available() {
        return;
    }
    s.execute("text", &json!({"at": [0, 0], "text": "Hi", "size": 20})).unwrap();
    // H is one region, i two (stem and dot).
    assert_eq!(s.active_sketch().unwrap().regions().len(), 3);
    assert!(s.execute("text", &json!({"at": [0, 0], "text": "   "})).is_err());
}

#[test]
fn chain_and_region_offsets() {
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [0, 10], "b": [0, 0]})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [10, 0]})).unwrap();
    let sk = s.active_sketch.unwrap().0;
    s.execute(
        "select",
        &json!({"picks": [{"kind": "sketch_entity", "sketch": sk, "entity": 0}, {"kind": "sketch_entity", "sketch": sk, "entity": 1}]}),
    )
    .unwrap();
    s.execute("offset_edge", &json!({"distance": 1})).unwrap();
    let ents = &s.active_sketch().unwrap().entities;
    assert_eq!(ents.len(), 4);
    // Joined at a sharp corner, not two separate parallels.
    let corner = glam::DVec2::new(1.0, 1.0);
    assert!(
        ents[2..].iter().all(|e| matches!(e, SketchEntity::Line { a, b } if a.distance(corner) < 1e-9 || b.distance(corner) < 1e-9)),
        "{ents:?}"
    );
    // A region's whole outline.
    s.execute("rectangle", &json!({"corner": [20, 0], "opposite": [30, 10]})).unwrap();
    let region = s.active_sketch().unwrap().regions()[0].index;
    s.execute("offset_edge", &json!({"region": region, "distance": 2})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities.len(), 12, "four new sides");
}

#[test]
fn moving_a_region_moves_its_curves() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [10, 10]})).unwrap();
    s.execute("circle", &json!({"center": [30, 0], "radius": 2})).unwrap();
    let sk = s.active_sketch.unwrap().0;
    let sq = s.active_sketch().unwrap().regions().iter().max_by(|a, b| a.area.total_cmp(&b.area)).unwrap().index;
    assert_eq!(s.active_sketch().unwrap().region_entities(sq), vec![0, 1, 2, 3]);
    s.execute("select", &json!({"picks": [{"kind": "region", "sketch": sk, "region": sq}]})).unwrap();
    s.execute("translate", &json!({"by": [5, 0, 0]})).unwrap();
    let e = &s.active_sketch().unwrap().entities;
    assert_eq!(e[0], shapecraft_doc::SketchEntity::Line { a: glam::DVec2::new(5.0, 0.0), b: glam::DVec2::new(15.0, 0.0) });
    assert_eq!(e[4], shapecraft_doc::SketchEntity::Circle { center: glam::DVec2::new(30.0, 0.0), radius: 2.0 }, "the circle stays");
}

#[test]
fn scale_works_on_picked_sketch_curves() {
    use glam::DVec2;
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("circle", &json!({"center": [10, 0], "radius": 2})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [4, 0]})).unwrap();
    let sk = s.active_sketch.unwrap().0;
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": sk, "entity": 0}]})).unwrap();
    // About its own middle by default: the circle stays put and grows.
    s.execute("scale", &json!({"factor": 2})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities[0], SketchEntity::Circle { center: DVec2::new(10.0, 0.0), radius: 4.0 });
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": sk, "entity": 1}]})).unwrap();
    s.execute("scale", &json!({"factor": 0.5, "center": [0, 0, 0]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities[1], SketchEntity::Line { a: DVec2::ZERO, b: DVec2::new(2.0, 0.0) });
}

#[test]
#[allow(clippy::panic)]
fn sketch_constraints_hold_through_edits() {
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [20, 0]})).unwrap();
    s.execute("line", &json!({"a": [0, 10], "b": [20, 13]})).unwrap();
    s.execute("circle", &json!({"center": [40, 40], "radius": 3})).unwrap();
    let id = s.active_sketch.unwrap();
    let sk = |s: &Session| s.doc.sketch(id).unwrap().clone();
    s.execute("constrain", &json!({"kind": "parallel", "entities": [0, 1]})).unwrap();
    let e = sk(&s).entities;
    assert_eq!(e[0], SketchEntity::Line { a: glam::DVec2::ZERO, b: glam::DVec2::new(20.0, 0.0) }, "the first pick holds still");
    assert!(shapecraft_doc::constraints::error(&e, &sk(&s).constraints) < 1e-6);
    assert!(s.execute("constrain", &json!({"kind": "parallel", "entities": [0, 1]})).is_err(), "no duplicates");

    // Tilting the first line drags the second along.
    s.execute("set_entity", &json!({"sketch": id.0, "entity": 0, "value": {"type": "line", "a": [0, 0], "b": [20, 5]}})).unwrap();
    let e = sk(&s).entities;
    assert!(shapecraft_doc::constraints::error(&e, &sk(&s).constraints) < 1e-6);
    let SketchEntity::Line { a, b } = e[1] else { panic!("still a line") };
    assert!(((b - a).normalize().y - glam::DVec2::new(20.0, 5.0).normalize().y).abs() < 1e-6);

    // Locked and perpendicular to a locked parallel line can't both hold.
    s.execute("constrain", &json!({"kind": "lock", "entities": [0]})).unwrap();
    s.execute("constrain", &json!({"kind": "lock", "entities": [1]})).unwrap();
    assert!(s.execute("constrain", &json!({"kind": "perpendicular", "entities": [0, 1]})).is_err());

    // Deleting the circle renumbers nothing the constraints use; deleting a line drops its constraints.
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": id.0, "entity": 2}]})).unwrap();
    s.execute("delete", &json!({})).unwrap();
    // (The first line, drawn from the origin, is held there too.)
    assert_eq!(sk(&s).constraints.len(), 4);
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": id.0, "entity": 0}]})).unwrap();
    s.execute("delete", &json!({})).unwrap();
    assert_eq!(sk(&s).constraints.len(), 1, "only the second line's lock is left");
    assert_eq!(sk(&s).constraints[0], shapecraft_doc::Constraint::Lock { entity: 0 });

    // Construction geometry makes no regions.
    s.execute("rectangle", &json!({"corner": [50, 0], "opposite": [60, 10]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().regions().len(), 1);
    s.execute("make_construction", &json!({"entities": [1]})).unwrap();
    assert!(s.active_sketch().unwrap().regions().is_empty());
    // Drawn in construction mode.
    s.execute("circle", &json!({"center": [80, 0], "radius": 2, "construction": true})).unwrap();
    assert!(s.active_sketch().unwrap().construction.contains(&5));
    assert!(s.active_sketch().unwrap().regions().is_empty());
}

#[test]
fn three_point_rectangles_turned_or_straight() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    // Turned: four lines in one step, closing a region of side × width.
    s.execute("rectangle", &json!({"points": [[0, 0], [10, 5], [8, 12]]})).unwrap();
    assert_eq!(s.active_sketch().unwrap().entities.len(), 4);
    s.execute("extrude", &json!({"sketch": s.active_sketch.unwrap().0, "region": 0, "distance": 2})).unwrap();
    assert!((volume(&mut s) - 160.0).abs() < 1e-6);
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert!(s.active_sketch().unwrap().entities.is_empty());
    // Straight: four lines as well; a zero width is refused.
    s.execute("rectangle", &json!({"points": [[0, 0], [10, 0], [3, -4]]})).unwrap();
    let e = &s.active_sketch().unwrap().entities;
    assert!(e.len() == 4 && e.iter().all(|e| matches!(e, shapecraft_doc::SketchEntity::Line { .. })));
    assert!(shapecraft_doc::rectangle_loop(e, 0).is_some());
    assert!(s.execute("rectangle", &json!({"points": [[0, 0], [10, 0], [3, 0]]})).is_err());
    assert!(s.execute("rectangle", &json!({"points": [[0, 0], [0, 0], [3, 1]]})).is_err());
}

#[test]
fn rectangles_are_four_lines_resized_together() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    let v = s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [10, 4]})).unwrap().value;
    assert_eq!(v["entities"], json!([0, 1, 2, 3]));
    let id = s.active_sketch.unwrap();
    s.selection = (0..4).map(|entity| Pick::SketchEntity { sketch: id, entity }).collect();
    assert!((s.selection_length().unwrap() - 28.0).abs() < 1e-12);
    assert_eq!(s.selection_summary().as_deref(), Some("4 edges"));
    s.execute("arc", &json!({"a": [0, 10], "mid": [5, 15], "b": [10, 10]})).unwrap();
    s.selection = vec![Pick::SketchEntity { sketch: id, entity: 4 }];
    assert!((s.selection_length().unwrap() - std::f64::consts::PI * 5.0).abs() < 1e-9, "a half circle");
    s.execute("undo", &json!({})).unwrap();
    // One undo step resizes all four sides.
    let lines: Vec<serde_json::Value> = shapecraft_doc::SketchEntity::rectangle_lines(glam::DVec2::ZERO, glam::DVec2::new(12.0, 4.0))
        .iter()
        .enumerate()
        .map(|(i, e)| json!([i, e]))
        .collect();
    s.execute("set_entity", &json!({"sketch": id.0, "entities": lines})).unwrap();
    let e = &s.active_sketch().unwrap().entities;
    let f = shapecraft_doc::rectangle_frame(e, shapecraft_doc::rectangle_loop(e, 0).unwrap()).unwrap();
    assert!((f.width - 12.0).abs() < 1e-12);
    s.execute("undo", &json!({})).unwrap();
    let e = &s.active_sketch().unwrap().entities;
    let f = shapecraft_doc::rectangle_frame(e, shapecraft_doc::rectangle_loop(e, 2).unwrap()).unwrap();
    assert!((f.width - 10.0).abs() < 1e-12);
    assert_eq!(s.active_sketch().unwrap().regions().len(), 1);
}

#[test]
fn regions_split_by_a_line_extrude_as_one_solid() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [-12, 3], "b": [12, 3]})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap();
    let sk = s.active_sketch.unwrap();
    let picks: Vec<_> = (0..2).map(|r| json!({"kind": "region", "sketch": sk.0, "region": r})).collect();
    s.execute("select", &json!({"picks": picks})).unwrap();
    s.execute("extrude", &json!({"distance": 5})).unwrap();
    assert_eq!(s.doc.bodies.len(), 1);
    let b = &s.doc.bodies[0];
    // A plain cylinder: top, bottom and the round wall, with no wall or seam where the line was.
    assert_eq!(s.kernel.face_count(&b.shape).unwrap(), 3);
}

#[test]
#[allow(clippy::panic)]
fn an_angle_dimension_turns_the_second_line_about_the_corner() {
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    // The corner observed in s15-006: 120.96° between a line to the left and one up to the right.
    s.execute("line", &json!({"a": [-30, 0], "b": [0, 0]})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [15, 25]})).unwrap();
    let id = s.active_sketch.unwrap();
    s.execute("constrain", &json!({"kind": "angle", "value": 90, "entities": [0, 1]})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    assert_eq!(sk.entities[0], SketchEntity::Line { a: glam::DVec2::new(-30.0, 0.0), b: glam::DVec2::ZERO }, "the first pick holds still");
    let SketchEntity::Line { a, b } = sk.entities[1] else { panic!("still a line") };
    assert!(a.length() < 1e-9, "it turns about the shared corner");
    assert!((b.length() - 850f64.sqrt()).abs() < 1e-6, "keeps its length: {b}");
    assert!(b.x.abs() < 1e-6 && b.y > 0.0, "upright, on the side it was: {b}");
    assert!(shapecraft_doc::constraints::error(&sk.entities, &sk.constraints) < 1e-6);
    // The angle holds when the first line is tilted.
    s.execute("set_entity", &json!({"sketch": id.0, "entity": 0, "value": {"type": "line", "a": [-30, -10], "b": [0, 0]}})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    assert!(shapecraft_doc::constraints::error(&sk.entities, &sk.constraints) < 1e-6);
    // A new value replaces it; nonsense is refused.
    s.execute("constrain", &json!({"kind": "angle", "value": 45, "entities": [0, 1]})).unwrap();
    assert_eq!(s.doc.sketch(id).unwrap().constraints.iter().filter(|c| matches!(c, shapecraft_doc::Constraint::Angle { .. })).count(), 1);
    for bad in [0.0, 180.0, -5.0, f64::NAN] {
        assert!(s.execute("constrain", &json!({"kind": "angle", "value": bad, "entities": [0, 1]})).is_err());
    }
}

#[test]
fn point_distances_between_points_and_to_a_line() {
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [10, 0]})).unwrap();
    s.execute("line", &json!({"a": [3, 8], "b": [12, 14]})).unwrap();
    let id = s.active_sketch.unwrap();
    // Two picked points (one end of each line): the second line moves to meet the distance.
    s.execute("select", &json!({"picks": [{"kind": "sketch_point", "sketch": id.0, "entity": 0, "index": 0}, {"kind": "sketch_point", "sketch": id.0, "entity": 1, "index": 0}]})).unwrap();
    s.execute("constrain", &json!({"kind": "point_distance", "value": 5})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    assert_eq!(sk.entities[0], SketchEntity::Line { a: glam::DVec2::ZERO, b: glam::DVec2::new(10.0, 0.0) }, "the first holds still");
    let SketchEntity::Line { a, .. } = sk.entities[1] else { panic!("still a line") };
    assert!((a.length() - 5.0).abs() < 1e-6, "{a}");
    // A point and a line: square to the line.
    s.execute("select", &json!({"picks": [{"kind": "sketch_point", "sketch": id.0, "entity": 1, "index": 1}, {"kind": "sketch_entity", "sketch": id.0, "entity": 0}]})).unwrap();
    s.execute("constrain", &json!({"kind": "point_distance", "value": 20})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    assert!(shapecraft_doc::constraints::error(&sk.entities, &sk.constraints) < 1e-6);
    let (SketchEntity::Line { a: p0, b: p1 }, SketchEntity::Line { a: q0, b: q1 }) = (&sk.entities[0], &sk.entities[1]) else { panic!() };
    let d = (*q1 - *p0).perp_dot((*p1 - *p0).normalize()).abs();
    assert!((d - 20.0).abs() < 1e-6 && (q0.distance(*p0) - 5.0).abs() < 1e-6, "both kept: {d}");
    assert_eq!(sk.constraints.len(), 3, "the two distances and the first line's origin");
    // A new value replaces the old one; nonsense is refused.
    s.execute("constrain", &json!({"kind": "point_distance", "value": 15, "elements": [{"kind": "point", "entity": 1, "index": 1}, {"kind": "segment", "entity": 0, "index": 0}]})).unwrap();
    assert_eq!(s.doc.sketch(id).unwrap().constraints.len(), 3);
    for bad in [
        json!({"kind": "point_distance", "value": -1, "elements": [{"kind": "point", "entity": 0, "index": 0}, {"kind": "point", "entity": 1, "index": 0}]}),
        json!({"kind": "point_distance", "value": 3, "elements": [{"kind": "point", "entity": 0, "index": 9}, {"kind": "point", "entity": 1, "index": 0}]}),
        json!({"kind": "point_distance", "value": 3, "elements": [{"kind": "segment", "entity": 0, "index": 0}, {"kind": "point", "entity": 1, "index": 0}]}),
    ] {
        assert!(s.execute("constrain", &bad).is_err());
    }
}

#[test]
fn a_sketch_mirror_stays_linked_to_its_axis() {
    use shapecraft_doc::SketchEntity;
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [0, -10], "b": [0, 10]})).unwrap();
    s.execute("line", &json!({"a": [2, 0], "b": [6, 4]})).unwrap();
    s.execute("circle", &json!({"center": [5, -5], "radius": 1})).unwrap();
    let id = s.active_sketch.unwrap();
    s.execute(
        "select",
        &json!({"picks": [{"kind": "sketch_entity", "sketch": id.0, "entity": 1}, {"kind": "sketch_entity", "sketch": id.0, "entity": 2}]}),
    )
    .unwrap();
    s.execute("mirror", &json!({"origin": [0, 0, 0], "normal": [1, 0, 0]})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    assert_eq!(sk.entities.len(), 5);
    let sym = sk.constraints.iter().filter(|c| matches!(c, shapecraft_doc::Constraint::Symmetric { .. })).count();
    assert_eq!(sym, 3, "two line ends and the circle's center");
    assert!(sk.constraints.iter().any(|c| matches!(c, shapecraft_doc::Constraint::Equal { .. })));
    // Moving the original drags its mirror image along.
    s.execute("set_entity", &json!({"sketch": id.0, "entity": 1, "value": {"type": "line", "a": [3, 0], "b": [6, 4]}})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    let SketchEntity::Line { a, .. } = sk.entities[3] else { panic!("the mirrored line") };
    assert!(a.x < -2.5, "followed: {a}");
    assert!(shapecraft_doc::constraints::error(&sk.entities, &sk.constraints) < 1e-6);
}

#[test]
fn an_offset_stays_parallel_or_concentric() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [10, 0]})).unwrap();
    s.execute("circle", &json!({"center": [20, 0], "radius": 3})).unwrap();
    let id = s.active_sketch.unwrap();
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": id.0, "entity": 0}]})).unwrap();
    s.execute("offset_edge", &json!({"distance": 2})).unwrap();
    s.execute("select", &json!({"picks": [{"kind": "sketch_entity", "sketch": id.0, "entity": 1}]})).unwrap();
    s.execute("offset_edge", &json!({"distance": 1})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    let kinds: Vec<&str> = sk.constraints.iter().map(|c| c.label()).collect();
    // The line drawn from the origin stays on it.
    assert_eq!(kinds, vec!["Coincident", "Parallel", "Distance", "Concentric"]);
    // Turning the line turns its offset too.
    s.execute("set_entity", &json!({"sketch": id.0, "entity": 0, "value": {"type": "line", "a": [0, 0], "b": [10, 5]}})).unwrap();
    let sk = s.doc.sketch(id).unwrap().clone();
    assert!(shapecraft_doc::constraints::error(&sk.entities, &sk.constraints) < 1e-6);
}

#[test]
fn a_vertical_distance_type_sets_only_the_lines_height() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("line", &json!({"a": [0, 0], "b": [30, 20]})).unwrap();
    s.execute("constrain", &json!({"kind": "distance", "value": 8, "along": "vertical", "entities": [0]})).unwrap();
    let Some(shapecraft_doc::SketchEntity::Line { a, b }) = s.doc.sketches[0].entities.first().cloned() else { panic!("no line") };
    assert!(((b.y - a.y).abs() - 8.0).abs() < 1e-6, "{a} {b}");
    assert!(s.execute("constrain", &json!({"kind": "distance", "value": 8, "along": "sideways", "entities": [0]})).is_err());
}

/// A sketch says where its axes point, and `x_dir` turns them.
#[test]
fn sketch_returns_its_frame_and_takes_x_dir() {
    let mut s = super::session();
    let out = s.execute("sketch", &serde_json::json!({"origin": [0, 0, 5], "normal": [0, 0, 1]})).unwrap();
    assert_eq!(out.value["normal"], serde_json::json!([0.0, 0.0, 1.0]));
    let out = s.execute("sketch", &serde_json::json!({"origin": [0, 0, 5], "normal": [0, 0, 1], "x_dir": [1, 0, 0.3]})).unwrap();
    assert_eq!(out.value["x"], serde_json::json!([1.0, 0.0, 0.0]));
    assert_eq!(out.value["y"], serde_json::json!([0.0, 1.0, 0.0]));
    assert!(s.execute("sketch", &serde_json::json!({"origin": [0, 0, 0], "normal": [0, 0, 1], "x_dir": [0, 0, 2]})).is_err());
}

/// inspect says how regions nest: the ring between two rectangles has a hole, the middle sits inside it.
#[test]
fn inspect_reports_region_nesting() {
    let mut s = super::session();
    s.execute("sketch", &serde_json::json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &serde_json::json!({"corner": [0, 0], "opposite": [20, 20]})).unwrap();
    s.execute("rectangle", &serde_json::json!({"corner": [5, 5], "opposite": [15, 15]})).unwrap();
    s.execute("rectangle", &serde_json::json!({"corner": [40, 0], "opposite": [50, 10]})).unwrap();
    let v = s.execute("inspect", &serde_json::json!({})).unwrap().value;
    let regions = v["sketches"][0]["regions"].as_array().unwrap().clone();
    assert_eq!(regions.len(), 3);
    let ring = regions.iter().find(|r| (r["area_mm2"].as_f64().unwrap() - 300.0).abs() < 1e-6).unwrap();
    let middle =
        regions.iter().find(|r| (r["area_mm2"].as_f64().unwrap() - 100.0).abs() < 1e-6 && r["center"][0].as_f64().unwrap() < 30.0).unwrap();
    let lone = regions.iter().find(|r| r["center"][0].as_f64().unwrap() > 30.0).unwrap();
    assert_eq!(ring["holes"], 1);
    assert!(ring["inside"].is_null());
    assert_eq!(middle["holes"], 0);
    assert_eq!(middle["inside"], ring["region"]);
    assert_eq!(lone["holes"], 0);
    assert!(lone["inside"].is_null());
}

/// The counters of "o" and "d" are regions inside their letters; the letters themselves sit inside nothing.
#[test]
fn text_counters_sit_inside_their_letters() {
    let mut s = super::session();
    s.execute("sketch", &serde_json::json!({"plane": "xy"})).unwrap();
    s.execute("text", &serde_json::json!({"at": [0, 0], "text": "Nod", "size": 20})).unwrap();
    let v = s.execute("inspect", &serde_json::json!({})).unwrap().value;
    let regions = v["sketches"][0]["regions"].as_array().unwrap().clone();
    let solid: Vec<_> = regions.iter().filter(|r| r["inside"].is_null()).collect();
    let counters: Vec<_> = regions.iter().filter(|r| !r["inside"].is_null()).collect();
    assert_eq!(solid.len(), 3, "{regions:?}");
    assert_eq!(counters.len(), 2, "{regions:?}");
    assert_eq!(solid.iter().filter(|r| r["holes"] == 1).count(), 2);
}

/// inspect narrows to one sketch or body, and can leave the face lists out.
#[test]
fn inspect_filters() {
    let mut s = super::session();
    s.execute("sketch", &serde_json::json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &serde_json::json!({"corner": [0, 0], "opposite": [20, 20]})).unwrap();
    let sk = s.active_sketch.unwrap().0;
    let body = s.execute("extrude", &serde_json::json!({"sketch": sk, "region": 0, "distance": 5})).unwrap().value["body"].clone();
    let v = s.execute("inspect", &serde_json::json!({"sketch": sk})).unwrap().value;
    assert_eq!(v["sketches"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["bodies"].as_array().map(Vec::len), Some(0));
    let v = s.execute("inspect", &serde_json::json!({"body": body, "faces": false})).unwrap().value;
    assert_eq!(v["bodies"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["bodies"][0]["faces"].as_array().map(Vec::len), Some(0));
    assert_eq!(v["bodies"][0]["face_count"], 6);
    assert!(s.execute("inspect", &serde_json::json!({"body": 999})).is_err());
}

/// A copied sketch comes back whole, in place, named like a copied body, and a second copy gets the next number (#37).
#[test]
fn duplicate_sketch_copies_a_whole_sketch() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
    s.execute("line", &json!({"a": [5, 0], "b": [9, 0]})).unwrap();
    let k = s.active_sketch.unwrap();
    s.execute("exit_sketching", &json!({})).unwrap();
    let out = s.execute("duplicate_sketch", &json!({"sketch": k.0})).unwrap().value;
    let copy = shapecraft_doc::ItemId(out["sketch"].as_u64().unwrap());
    assert_eq!(out["name"], "Sketch 01 (1)");
    let (a, b) = (s.doc.sketch(k).unwrap(), s.doc.sketch(copy).unwrap());
    assert_eq!(a.entities, b.entities);
    assert_eq!(a.plane, b.plane);
    assert_eq!(s.selection, vec![shapecraft_doc::Pick::Sketch { sketch: copy }]);
    let again = s.execute("duplicate_sketch", &json!({"sketch": k.0})).unwrap().value;
    assert_eq!(again["name"], "Sketch 01 (2)");
    // One undo takes the last copy back.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.doc.sketches.len(), 2);
}

/// #49: one letter of a text picks the whole text, every letter and none of the counters, beside a rectangle that
/// isn't part of it; extruding that pick gives one body per letter with the holes kept.
#[test]
fn select_text_takes_every_letter_but_no_counters() {
    let mut s = super::session();
    if !shapecraft_doc::text::available() {
        return;
    }
    s.execute("sketch", &serde_json::json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &serde_json::json!({"center": [30, -40], "size": [40, 10]})).unwrap();
    s.execute("text", &serde_json::json!({"at": [0, 0], "text": "OBA", "size": 20})).unwrap();
    let sk = s.active_sketch.unwrap();
    s.execute("exit_sketching", &serde_json::json!({})).unwrap();
    let all = s.doc.sketch(sk).unwrap().regions().len();
    assert_eq!(all, 1 + 3 + 4, "rectangle, three letters, four counters");
    // Any letter: the O's region is the one with the most area left of x = 15.
    let o = s
        .doc
        .sketch(sk)
        .unwrap()
        .regions()
        .iter()
        .filter(|r| r.centroid.x < 15.0 && r.centroid.y > -20.0)
        .max_by(|a, b| a.area.total_cmp(&b.area))
        .unwrap()
        .index;
    let v = s.execute("select_text", &serde_json::json!({"sketch": sk.0, "region": o})).unwrap().value;
    assert_eq!(v["regions"].as_array().unwrap().len(), 3, "{v}");
    assert_eq!(s.selection.len(), 3);
    // From a pick too.
    s.selection = vec![shapecraft_doc::Pick::Region { sketch: sk, region: o }];
    s.execute("select_text", &serde_json::json!({})).unwrap();
    assert_eq!(s.selection.len(), 3);
    // The rectangle is no text.
    let rect = s.doc.sketch(sk).unwrap().regions().iter().find(|r| r.centroid.y < -30.0).unwrap().index;
    assert!(s.execute("select_text", &serde_json::json!({"sketch": sk.0, "region": rect})).is_err());
}

/// Offset Edge on the edges of a body lying on the open sketch's plane (a board's outline): only their offset goes
/// into the sketch, the four sides joined as one rectangle 1 mm in.
#[test]
fn offset_edge_takes_body_edges_on_the_sketch_plane() {
    let mut s = session();
    s.execute("sketch", &json!({"plane": "xy"})).unwrap();
    s.execute("rectangle", &json!({"corner": [0, 0], "opposite": [10, 20]})).unwrap();
    s.execute("extrude", &json!({"sketch": 1, "region": 0, "distance": 5})).unwrap();
    let body = s.doc.bodies[0].id;
    let top = (0..6).find(|&f| s.kernel.face_plane(&s.doc.bodies[0].shape, f).unwrap().is_some_and(|p| p.origin.z > 4.9)).unwrap();
    s.execute("sketch", &json!({"body": body.0, "face": top})).unwrap();
    let sketch = s.active_sketch.unwrap();
    let m = s.doc.bodies[0].mesh.clone();
    let edges: Vec<serde_json::Value> = (0..m.edges.len())
        .filter(|&k| m.edges[k].iter().all(|p| (p[2] - 5.0).abs() < 1e-4))
        .map(|k| json!({"body": body.0, "edge": m.edge_ids[k]}))
        .collect();
    assert_eq!(edges.len(), 4);
    s.execute("offset_edge", &json!({"edges": edges, "distance": -1})).unwrap();
    let sk = s.doc.sketch(sketch).unwrap();
    assert_eq!(sk.entities.len(), 4, "only the offset, not the edges themselves");
    let mut lens: Vec<f64> = sk.entities.iter().map(|e| e.outline()[0].distance(*e.outline().last().unwrap())).collect();
    lens.sort_by(f64::total_cmp);
    assert!((lens[0] - 8.0).abs() < 1e-6 && (lens[3] - 18.0).abs() < 1e-6, "{lens:?}");
    // An edge off the plane is refused.
    let side = (0..m.edges.len()).find(|&k| m.edges[k].iter().any(|p| p[2] < 1.0)).unwrap();
    let off = json!([{"body": body.0, "edge": s.doc.bodies[0].mesh.edge_ids[side]}]);
    assert!(s.execute("offset_edge", &json!({"edges": off, "distance": 1})).is_err());
}
