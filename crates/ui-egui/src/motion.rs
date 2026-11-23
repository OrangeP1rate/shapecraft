//! How things come and go.
//!
//! **Popups** (every menu, popover, card and sheet: the app's areas above the view) open the way Casa opens a photo,
//! which is Picasa's opening as people remember it: the popup grows out of a single point, fast, decelerating into
//! place, and closing pulls it back into that point, accelerating away so the eye lets go of it at once. A popover
//! grows out of the spot that was clicked to open it; a sheet over a dimmed window grows out of its own middle while
//! the dim fades in under it. Nobody calls anything: [`finish`] runs once at the end of every frame, finds the areas
//! that appeared or went away, and closing replays the popup's last frame, so no caller keeps drawing a closed popup.
//!
//! **The side columns** slide like a cartoon ([`column_begin`] / [`column_end`]): in from the edge on a spring that
//! overshoots a few points and settles back, out after a short lean the wrong way, then whipped off the side.
//!
//! Off with the system's Reduce Motion, and in the snapshot tool unless a script asks for it ([`set_enabled`]).
//! Only paint moves: clicks and layout are always where things come to rest.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::emath::TSTransform;
use egui::epaint::{ClippedShape, Color32, Shape};
use egui::layers::ShapeIdx;
use egui::{Context, Id, LayerId, Order, Pos2, Rect, Ui, vec2};

/// A popup's opening (Casa: 0.26 s), and how long the dim takes to come up (the first third of it).
const OPEN_S: f64 = 0.26;
const VEIL_IN_S: f64 = OPEN_S * 0.35;
/// A popup's closing (Casa: 0.17 s).
const CLOSE_S: f64 = 0.17;
/// Arriving decelerates, but not so hard the growth is spent in the first two frames (a curve that front-loaded reads
/// as a plain fade).
const ARRIVE: [f32; 4] = [0.25, 0.6, 0.3, 1.0];
/// Leaving accelerates away.
const LEAVE: [f32; 4] = [0.55, 0.0, 0.9, 0.45];
/// The fade on leaving is held back at first, so the popup is still visibly on its way into the point.
const LIFT: [f32; 4] = [0.6, 0.0, 1.0, 1.0];
/// A press or release this recent (seconds) is what opened a popup that appears now: it grows out of that spot.
const OPENED_BY_S: f64 = 0.6;
/// At most this many closing popups in flight (a burst of them is a swap, not a show).
const MAX_GHOSTS: usize = 8;

/// The columns: a spring for coming in (rad/s, damping ratio: about 3 % overshoot, a few points, settled in 0.4 s).
const SPRING_W: f32 = 21.0;
const SPRING_Z: f32 = 0.74;
/// Going out: the lean the wrong way (points, seconds), then the throw off the edge (seconds).
const LEAN_PT: f32 = 7.0;
const LEAN_S: f32 = 0.085;
const THROW_S: f32 = 0.2;
/// Past the window's edge, so a column's shadow goes too.
const OFF_EDGE_PT: f32 = 24.0;

static OFF: AtomicBool = AtomicBool::new(false);

/// Turns motion on or off for the whole app (the snapshot tool turns it off, so a shot is never mid-flight).
pub fn set_enabled(on: bool) {
    OFF.store(!on, Ordering::Relaxed);
}

fn enabled() -> bool {
    !OFF.load(Ordering::Relaxed) && !reduce_motion()
}

/// The system's Reduce Motion (macOS, Windows' "Animation effects"), read once in the background at the start.
fn reduce_motion() -> bool {
    static REDUCE: OnceLock<std::sync::Arc<AtomicBool>> = OnceLock::new();
    REDUCE
        .get_or_init(|| {
            let flag = std::sync::Arc::new(AtomicBool::new(false));
            let set = flag.clone();
            let _ = std::thread::Builder::new()
                .name("reduce-motion".into())
                .spawn(move || set.store(system_reduce_motion(), Ordering::Relaxed));
            flag
        })
        .load(Ordering::Relaxed)
}

#[cfg(target_os = "macos")]
fn system_reduce_motion() -> bool {
    std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
}

#[cfg(windows)]
fn system_reduce_motion() -> bool {
    // Settings ▸ Accessibility ▸ Visual effects ▸ Animation effects off writes MinAnimate "0".
    windows_registry::CURRENT_USER
        .open(r"Control Panel\Desktop\WindowMetrics")
        .and_then(|k| k.get_string("MinAnimate"))
        .is_ok_and(|v| v.trim() == "0")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system_reduce_motion() -> bool {
    false
}

/// A popup on screen, as last drawn.
#[derive(Clone)]
struct Pop {
    born: f64,
    /// The point it grows out of and goes back into (the layer's own coordinates).
    origin: Pos2,
    card: Rect,
    shapes: Vec<ClippedShape>,
}

/// A popup on its way out: its last frame, shrinking into its point.
#[derive(Clone)]
struct Ghost {
    layer: LayerId,
    start: f64,
    /// How grown and how opaque it was when it closed (it may close before it finished opening).
    scale: f32,
    alpha: f32,
    veil_alpha: f32,
    origin: Pos2,
    shapes: Vec<ClippedShape>,
}

/// A side column's slide.
#[derive(Clone, Default)]
struct Column {
    shown: bool,
    /// The frame it was last asked about (none yet: the workspace just opened).
    seen: Option<u64>,
    /// Coming in: when, and from how far off its place (points, signed).
    enter: Option<(f64, f32)>,
    /// Going out: when, from where, and to where (points, signed).
    exit: Option<(f64, f32, f32)>,
    /// Points that take it fully off the window, signed toward its edge.
    away: f32,
    /// Where it is drawn this frame (points off its place).
    at: f32,
    /// Its place, as last shown.
    rect: Option<Rect>,
    shapes: Vec<ClippedShape>,
}

/// A popup drawn in line with other things on a layer (a flyout of the tool column): its frames, or its way out.
#[derive(Clone)]
struct Inline {
    born: f64,
    origin: Pos2,
    card: Rect,
    seen: u64,
    shapes: Vec<ClippedShape>,
    /// Closing: when, and how grown / opaque it was then.
    closing: Option<(f64, f32, f32)>,
}

#[derive(Clone, Default)]
struct Motion {
    pops: HashMap<LayerId, Pop>,
    ghosts: Vec<Ghost>,
    /// The last press or release: where, when.
    touch: Option<(Pos2, f64)>,
    columns: HashMap<Id, Column>,
    /// Popups drawn in line on another layer (`popup_begin` / `popup_end`).
    inline: HashMap<Id, Inline>,
    /// Values gliding to a new target: (from, to, since).
    tweens: HashMap<Id, (f32, f32, f64)>,
    /// Pages that swap in place: (which page, since).
    swaps: HashMap<Id, (u64, f64)>,
    /// The screen showing (the projects screen or a project) and its last frame, for the dissolve between them.
    screen: Option<(u64, Vec<ClippedShape>)>,
    /// The screen just left, dissolving over the new one: since, its frame, and which way (into a project or out).
    screen_ghost: Option<(f64, Vec<ClippedShape>, bool)>,
    /// Tooltip-order areas that are popups (asked for each frame by `include`).
    extra: HashSet<LayerId>,
}

fn key() -> Id {
    Id::unique("shapecraft-motion")
}

fn with<R>(ctx: &Context, f: impl FnOnce(&mut Motion) -> R) -> R {
    ctx.data_mut(|d| f(d.get_temp_mut_or_default::<Motion>(key())))
}

/// A tooltip-order area that is really a popup (a dropdown list, a submenu): it opens and closes like the rest.
pub fn include(ctx: &Context, layer: LayerId) {
    with(ctx, |m| {
        m.extra.insert(layer);
    });
}

/// One cubic-bezier timing curve (CSS / Core Animation control points), y at x.
fn bezier(c: [f32; 4], x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.0 || x >= 1.0 {
        return x;
    }
    let [x1, y1, x2, y2] = c;
    let at = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    // Newton on x(t) = x, then bisection if it wanders.
    let mut t = x;
    for _ in 0..8 {
        let err = at(x1, x2, t) - x;
        if err.abs() < 1e-5 {
            return at(y1, y2, t);
        }
        let u = 1.0 - t;
        let d = 3.0 * u * u * x1 + 6.0 * u * t * (x2 - x1) + 3.0 * t * t * (1.0 - x2);
        if d.abs() < 1e-6 {
            break;
        }
        t = (t - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    t = x;
    for _ in 0..30 {
        let v = at(x1, x2, t);
        if (v - x).abs() < 1e-5 {
            break;
        }
        if v < x {
            lo = t;
        } else {
            hi = t;
        }
        t = 0.5 * (lo + hi);
    }
    at(y1, y2, t)
}

fn ease_out(x: f32) -> f32 {
    let u = 1.0 - x.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// A full-window dim behind a sheet: it fades, it doesn't grow.
fn is_veil(shape: &Shape, screen: Rect) -> bool {
    match shape {
        Shape::Rect(r) => r.rect.width() >= screen.width() * 0.9 && r.rect.height() >= screen.height() * 0.9,
        _ => false,
    }
}

/// What's drawn of a popup apart from its dim.
fn card_of(shapes: &[ClippedShape], screen: Rect) -> Option<Rect> {
    let mut card = Rect::NOTHING;
    for s in shapes {
        if is_veil(&s.shape, screen) {
            continue;
        }
        let r = s.shape.visual_bounding_rect().intersect(s.clip_rect);
        if r.is_positive() && r.is_finite() {
            card = card.union(r);
        }
    }
    card.is_positive().then_some(card)
}

/// Scales a shape about a point.
fn grow(s: &mut ClippedShape, origin: Pos2, scale: f32) {
    if (scale - 1.0).abs() > 1e-4 {
        s.transform(TSTransform::new(origin.to_vec2() * (1.0 - scale), scale));
    }
}

/// Fades a shape.
fn fade(shape: &mut Shape, a: f32) {
    if a >= 0.999 {
        return;
    }
    match shape {
        Shape::Vec(v) => v.iter_mut().for_each(|s| fade(s, a)),
        Shape::Text(t) => t.opacity_factor *= a,
        Shape::Callback(_) | Shape::Noop => {}
        _ => egui::epaint::shape_transform::adjust_colors(shape, move |c| {
            if *c != Color32::PLACEHOLDER {
                *c = c.gamma_multiply(a);
            }
        }),
    }
}

/// How far open a popup is `t` seconds after it appeared: (scale, the card's opacity, the dim's opacity).
fn opening(t: f64, seed: f32) -> (f32, f32, f32) {
    let e = (t / OPEN_S) as f32;
    let scale = seed + (1.0 - seed) * bezier(ARRIVE, e);
    let a = ease_out((t / VEIL_IN_S) as f32);
    (scale, a, a)
}

/// A pixel, near enough, of whatever size the card lands at.
fn seed(card: Rect) -> f32 {
    (2.0 / card.width().max(card.height()).max(1.0)).max(0.004)
}

/// Runs once at the very end of each frame, after everything is drawn: popups that appeared grow out of their point,
/// popups that went away shrink back into it.
pub fn finish(ctx: &Context) {
    let now = ctx.input(|i| i.time);
    let screen = ctx.content_rect();
    let touched = ctx.input(|i| (i.pointer.any_pressed() || i.pointer.any_released()).then(|| i.pointer.interact_pos()).flatten());
    let mut m = with(ctx, std::mem::take);
    if let Some(p) = touched {
        m.touch = Some((p, now));
    }
    if !enabled() {
        m.pops.clear();
        m.ghosts.clear();
        m.extra.clear();
        ctx.data_mut(|d| d.insert_temp(key(), m));
        return;
    }
    let mut layers: Vec<LayerId> = ctx
        .memory(|mem| mem.areas().visible_layer_ids())
        .into_iter()
        .filter(|l| matches!(l.order, Order::Middle | Order::Foreground))
        .collect();
    layers.extend(m.extra.drain());
    layers.sort_by_key(|l| (l.order, l.id.value()));
    layers.dedup();
    // Where a press was, in each layer's own coordinates (a zoomed layer has its own).
    let from_global: HashMap<LayerId, TSTransform> =
        layers.iter().filter_map(|&l| Some((l, ctx.layer_transform_from_global(l)?))).collect();

    // What each popup drew this frame.
    let drawn: Vec<(LayerId, Vec<ClippedShape>)> = ctx.graphics(|g| {
        layers
            .iter()
            .filter_map(|&l| {
                let list = g.get(l)?;
                (!list.is_empty()).then(|| (l, list.all_entries().cloned().collect::<Vec<_>>()))
            })
            // An area's first frame is laid out unseen (placeholders only): it hasn't appeared yet.
            .filter(|(_, shapes)| card_of(shapes, screen).is_some() || shapes.iter().any(|s| is_veil(&s.shape, screen)))
            .collect()
    });
    let born_now: Vec<(LayerId, Rect)> =
        drawn.iter().filter(|(l, _)| !m.pops.contains_key(l)).filter_map(|(l, s)| Some((*l, card_of(s, screen)?))).collect();
    let mut settled_births: HashSet<LayerId> = HashSet::new();

    // Popups that went away this frame.
    let gone: Vec<LayerId> = m.pops.keys().filter(|l| !drawn.iter().any(|(d, _)| d == *l)).copied().collect();
    for layer in gone {
        let Some(pop) = m.pops.remove(&layer) else { continue };
        // The same thing drawn again under a new name (its id changed) carries on as it was.
        if let Some((nl, _)) = born_now.iter().find(|(_, r)| close_to(*r, pop.card)) {
            settled_births.insert(*nl);
            continue;
        }
        if m.ghosts.len() >= MAX_GHOSTS || pop.shapes.is_empty() {
            continue;
        }
        let (scale, alpha, veil_alpha) = opening(now - pop.born, seed(pop.card));
        // Its own layer, named after the popup's, so a reopening finds it.
        m.ghosts.retain(|g| g.layer != ghost_layer(layer));
        m.ghosts.push(Ghost { layer: ghost_layer(layer), start: now, scale, alpha, veil_alpha, origin: pop.origin, shapes: pop.shapes });
    }

    let recent = m.touch.filter(|(_, t)| now - t <= OPENED_BY_S).map(|(p, _)| p);
    let mut moving = false;
    for (layer, shapes) in drawn {
        let card = card_of(&shapes, screen).unwrap_or_else(|| Rect::from_center_size(screen.center(), vec2(2.0, 2.0)));
        let veiled = shapes.iter().any(|s| is_veil(&s.shape, screen));
        if !m.pops.contains_key(&layer) {
            // Reopened while still closing: it grows back from as far as it had shrunk.
            let ghost_at = m.ghosts.iter().position(|g| g.layer == ghost_layer(layer));
            let mut born = now;
            if let Some(i) = ghost_at {
                let g = m.ghosts.remove(i);
                let left = 1.0 - bezier(LEAVE, ((now - g.start) / CLOSE_S) as f32);
                born = now - OPEN_S * f64::from((g.scale * left).clamp(0.0, 1.0)) * 0.5;
            }
            if settled_births.contains(&layer) {
                born = now - OPEN_S;
            }
            let origin = match recent {
                Some(p) if !veiled => {
                    let p = from_global.get(&layer).map_or(p, |t| *t * p);
                    card.clamp(p)
                }
                _ => card.center(),
            };
            m.pops.insert(layer, Pop { born, origin, card, shapes: Vec::new() });
        }
        let Some(pop) = m.pops.get_mut(&layer) else { continue };
        pop.card = card;
        pop.shapes = shapes;
        let t = now - pop.born;
        if t >= OPEN_S {
            continue;
        }
        moving = true;
        let (scale, alpha, veil_alpha) = opening(t, seed(card));
        let origin = pop.origin;
        ctx.graphics_mut(|g| {
            if let Some(list) = g.get_mut(layer) {
                for i in 0..list.next_idx().0 {
                    list.mutate_shape(ShapeIdx(i), |s| {
                        if veiled && is_veil(&s.shape, screen) {
                            fade(&mut s.shape, veil_alpha);
                        } else {
                            grow(s, origin, scale);
                            fade(&mut s.shape, alpha);
                        }
                    });
                }
            }
        });
    }

    // Popups on their way out.
    m.ghosts.retain(|g| now - g.start < CLOSE_S);
    for g in &m.ghosts {
        moving = true;
        let c = ((now - g.start) / CLOSE_S) as f32;
        let scale = (g.scale * (1.0 - bezier(LEAVE, c))).max(0.004);
        let lift = 1.0 - bezier(LIFT, c);
        ctx.graphics_mut(|gr| {
            let list = gr.entry(g.layer);
            for s in &g.shapes {
                let mut s = s.clone();
                if is_veil(&s.shape, screen) {
                    fade(&mut s.shape, g.veil_alpha * lift);
                } else {
                    grow(&mut s, g.origin, scale);
                    fade(&mut s.shape, g.alpha * lift);
                }
                list.add(s.clip_rect, s.shape);
            }
        });
    }
    if moving || m.columns.values().any(|c| c.enter.is_some() || c.exit.is_some()) {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(key(), m));
}

fn ghost_layer(of: LayerId) -> LayerId {
    LayerId::new(of.order, of.id.with("motion-ghost"))
}

fn close_to(a: Rect, b: Rect) -> bool {
    (a.min - b.min).length() < 1.5 && (a.max - b.max).length() < 1.5
}

/// Which way a column leaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    Right,
}

/// A damped spring from `x0` points off its rest, `t` seconds in.
fn spring(x0: f32, t: f32) -> f32 {
    let wd = SPRING_W * (1.0 - SPRING_Z * SPRING_Z).sqrt();
    let decay = (-SPRING_Z * SPRING_W * t).exp();
    x0 * decay * ((wd * t).cos() + (SPRING_Z * SPRING_W / wd) * (wd * t).sin())
}

/// Going out: a lean the wrong way, a beat, then thrown off the edge. (Where it is, whether it's gone.)
fn throw(from: f32, to: f32, t: f32) -> (f32, bool) {
    let lean = -to.signum() * LEAN_PT;
    if t < LEAN_S {
        return (from + lean * ease_out(t / LEAN_S), false);
    }
    let u = (t - LEAN_S) / THROW_S;
    if u >= 1.0 {
        return (to, true);
    }
    let start = from + lean;
    (start + (to - start) * bezier(LEAVE, u), false)
}

/// Where a side column is this frame. `rect` is its place if it's shown; the answer is its inner edge as drawn now
/// (sliding in or out, overshoot included), or none once it's gone, so what's docked beside it rides along.
/// Draw the column between [`column_begin`] and [`column_end`].
pub fn column(ctx: &Context, name: &str, rect: Option<Rect>, edge: Edge) -> Option<f32> {
    let now = ctx.input(|i| i.time);
    let pass = ctx.cumulative_pass_nr();
    let on = enabled();
    let screen = ctx.content_rect();
    let id = key().with(name);
    let (edge_now, moving) = with(ctx, |m| {
        let col = m.columns.entry(id).or_default();
        if col.seen == Some(pass) {
            return (inner(col, edge), false);
        }
        // A frame skipped (another screen in between) counts as hidden.
        let was_shown = col.shown && col.seen.is_some_and(|s| s + 1 >= pass);
        col.seen = Some(pass);
        match rect {
            Some(r) => {
                col.away = match edge {
                    Edge::Left => -(r.right() - screen.left() + OFF_EDGE_PT),
                    Edge::Right => screen.right() - r.left() + OFF_EDGE_PT,
                };
                if !was_shown && on {
                    let from = if col.exit.is_some() { col.at } else { col.away };
                    col.enter = Some((now, from));
                    col.exit = None;
                }
                col.shown = true;
                col.rect = Some(r);
                col.at = 0.0;
                if let Some((t0, from)) = col.enter.filter(|_| on) {
                    let t = (now - t0) as f32;
                    let x = spring(from, t);
                    if t > 0.15 && x.abs() < 0.25 {
                        col.enter = None;
                    } else {
                        col.at = x;
                    }
                } else {
                    col.enter = None;
                }
            }
            None => {
                if was_shown && on && !col.shapes.is_empty() {
                    col.exit = Some((now, col.at, col.away));
                }
                col.shown = false;
                col.enter = None;
                col.at = 0.0;
                match col.exit.filter(|_| on) {
                    Some((t0, from, to)) => {
                        let (x, done) = throw(from, to, (now - t0) as f32);
                        col.at = x;
                        if done {
                            col.exit = None;
                            col.shapes.clear();
                        }
                    }
                    None => {
                        col.exit = None;
                        col.shapes.clear();
                    }
                }
            }
        }
        (inner(col, edge), col.enter.is_some() || col.exit.is_some())
    });
    if moving {
        ctx.request_repaint();
    }
    edge_now
}

/// A column's inner edge as drawn now, if any of it is.
fn inner(col: &Column, edge: Edge) -> Option<f32> {
    if !col.shown && col.exit.is_none() {
        return None;
    }
    let r = col.rect?;
    Some(match edge {
        Edge::Left => r.right() + col.at,
        Edge::Right => r.left() + col.at,
    })
}

/// Marks where a column's drawing starts in the layer `ui` paints to.
pub fn column_begin(ui: &Ui) -> ShapeIdx {
    let layer = ui.layer_id();
    ui.ctx().graphics_mut(|g| g.entry(layer).next_idx())
}

/// Ends a column's drawing (after [`column`] this frame): moves what was drawn to where the column is now, or, once
/// it's hidden, draws its last frame there on its way out.
pub fn column_end(ui: &Ui, name: &str, begin: ShapeIdx) {
    let ctx = ui.ctx().clone();
    let layer = ui.layer_id();
    let id = key().with(name);
    let Some(mut col) = with(&ctx, |m| m.columns.remove(&id)) else { return };
    let shift = TSTransform::from_translation(vec2(col.at, 0.0));
    if col.shown {
        let end = ctx.graphics_mut(|g| g.entry(layer).next_idx());
        col.shapes = ctx.graphics(|g| {
            g.get(layer).map(|l| l.all_entries().skip(begin.0).take(end.0.saturating_sub(begin.0)).cloned().collect()).unwrap_or_default()
        });
        if col.at != 0.0 {
            ctx.graphics_mut(|g| g.entry(layer).transform_range(begin, end, shift));
        }
    } else if col.exit.is_some() {
        ctx.graphics_mut(|g| {
            let list = g.entry(layer);
            for s in &col.shapes {
                let mut s = s.clone();
                s.transform(shift);
                list.add(s.clip_rect, s.shape);
            }
        });
    }
    with(&ctx, |m| {
        m.columns.insert(id, col);
    });
}

/// Marks where an in-line popup's drawing starts (one drawn on a shared layer, not in an area of its own).
pub fn popup_begin(ui: &Ui) -> ShapeIdx {
    column_begin(ui)
}

/// Ends an in-line popup's drawing: `shown` says whether it was drawn this frame. It grows out of the spot that opened
/// it and shrinks back into it, as the area popups do.
pub fn popup_end(ui: &Ui, name: &str, begin: ShapeIdx, shown: bool) {
    let ctx = ui.ctx().clone();
    let layer = ui.layer_id();
    let now = ctx.input(|i| i.time);
    let pass = ctx.cumulative_pass_nr();
    let screen = ctx.content_rect();
    let id = key().with(("inline", name));
    let on = enabled();
    let (mut pop, touch) = with(&ctx, |m| (m.inline.remove(&id), m.touch));
    if shown {
        let end = ctx.graphics_mut(|g| g.entry(layer).next_idx());
        let shapes: Vec<ClippedShape> = ctx.graphics(|g| {
            g.get(layer).map(|l| l.all_entries().skip(begin.0).take(end.0.saturating_sub(begin.0)).cloned().collect()).unwrap_or_default()
        });
        let Some(card) = card_of(&shapes, screen) else { return };
        let fresh = pop.as_ref().is_none_or(|p| p.closing.is_some() || p.seen + 1 < pass);
        let mut p = match pop.take() {
            Some(p) if !fresh => p,
            _ => {
                let recent = touch.filter(|(_, t)| now - t <= OPENED_BY_S + 0.05).map(|(p, _)| p);
                Inline {
                    born: if on { now } else { now - OPEN_S },
                    origin: recent.map_or(card.center(), |p| card.clamp(p)),
                    card,
                    seen: pass,
                    shapes: Vec::new(),
                    closing: None,
                }
            }
        };
        p.seen = pass;
        p.card = card;
        p.shapes = shapes;
        let t = now - p.born;
        if on && t < OPEN_S {
            let (scale, alpha, _) = opening(t, seed(card));
            let origin = p.origin;
            ctx.graphics_mut(|g| {
                let list = g.entry(layer);
                for i in begin.0..end.0 {
                    list.mutate_shape(ShapeIdx(i), |s| {
                        grow(s, origin, scale);
                        fade(&mut s.shape, alpha);
                    });
                }
            });
            ctx.request_repaint();
        }
        pop = Some(p);
    } else if let Some(mut p) = pop.take() {
        if p.closing.is_none() && on && p.seen + 1 >= pass {
            let (scale, alpha, _) = opening(now - p.born, seed(p.card));
            p.closing = Some((now, scale, alpha));
        }
        if let Some((t0, scale0, alpha0)) = p.closing.filter(|_| on)
            && now - t0 < CLOSE_S
        {
            let c = ((now - t0) / CLOSE_S) as f32;
            let scale = (scale0 * (1.0 - bezier(LEAVE, c))).max(0.004);
            let alpha = alpha0 * (1.0 - bezier(LIFT, c));
            ctx.graphics_mut(|g| {
                let list = g.entry(layer);
                for s in &p.shapes {
                    let mut s = s.clone();
                    grow(&mut s, p.origin, scale);
                    fade(&mut s.shape, alpha);
                    list.add(s.clip_rect, s.shape);
                }
            });
            ctx.request_repaint();
            pop = Some(p);
        }
    }
    if let Some(p) = pop {
        with(&ctx, |m| {
            m.inline.insert(id, p);
        });
    }
}

/// A value that glides to its new target (a highlight moving to another tab) on the arriving curve, instead of jumping.
pub fn tween(ctx: &Context, name: &str, target: f32, secs: f64) -> f32 {
    let now = ctx.input(|i| i.time);
    let on = enabled();
    let id = key().with(("tween", name));
    let (v, moving) = with(ctx, |m| {
        let e = m.tweens.entry(id).or_insert((target, target, now));
        if (e.1 - target).abs() > f32::EPSILON {
            // From wherever it is now, so a second change mid-glide doesn't jump.
            let u = ((now - e.2) / secs) as f32;
            let at = e.0 + (e.1 - e.0) * bezier(ARRIVE, u);
            *e = (at, target, now);
        }
        let u = if on { ((now - e.2) / secs) as f32 } else { 1.0 };
        (e.0 + (e.1 - e.0) * bezier(ARRIVE, u), u < 1.0)
    });
    if moving {
        ctx.request_repaint();
    }
    v
}

/// Marks where a page that swaps in place starts (a Preferences tab).
pub fn swap_begin(ui: &Ui) -> ShapeIdx {
    column_begin(ui)
}

/// Ends a swapping page: when `page` changes, the new page fades in (0.16 s, as Casa's settings panes do).
pub fn swap_end(ui: &Ui, name: &str, begin: ShapeIdx, page: u64) {
    const SWAP_S: f64 = 0.16;
    let ctx = ui.ctx().clone();
    let layer = ui.layer_id();
    let now = ctx.input(|i| i.time);
    let id = key().with(("swap", name));
    let since = with(&ctx, |m| {
        let e = m.swaps.entry(id).or_insert((page, f64::NEG_INFINITY));
        if e.0 != page {
            *e = (page, now);
        }
        e.1
    });
    let t = now - since;
    if !enabled() || t >= SWAP_S {
        return;
    }
    let a = ease_out((t / SWAP_S) as f32);
    let rise = TSTransform::from_translation(vec2(0.0, 4.0 * (1.0 - a)));
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        for i in begin.0..list.next_idx().0 {
            list.mutate_shape(ShapeIdx(i), |s| {
                s.transform(rise);
                fade(&mut s.shape, a);
            });
        }
    });
    ctx.request_repaint();
}

/// The whole window changing screens (the projects screen to a project and back): the old screen dissolves over the
/// new one, swelling a little going into a project and sinking going back out. Call once a frame, after everything is
/// drawn, with which screen is showing (`deeper` is the project side).
pub fn screen(ctx: &Context, which: u64, deeper: bool) {
    const DISSOLVE_S: f64 = 0.24;
    let now = ctx.input(|i| i.time);
    let on = enabled();
    let layer = LayerId::background();
    let shot: Vec<ClippedShape> =
        if on { ctx.graphics(|g| g.get(layer).map(|l| l.all_entries().cloned().collect()).unwrap_or_default()) } else { Vec::new() };
    let ghost = with(ctx, |m| {
        if let Some((was, last)) = m.screen.take()
            && was != which
            && on
            && !last.is_empty()
        {
            m.screen_ghost = Some((now, last, deeper));
        }
        m.screen = Some((which, shot));
        if m.screen_ghost.as_ref().is_some_and(|(t0, _, _)| now - t0 >= DISSOLVE_S) {
            m.screen_ghost = None;
        }
        m.screen_ghost.clone()
    });
    let Some((t0, shapes, deeper)) = ghost else { return };
    let u = ((now - t0) / DISSOLVE_S) as f32;
    let a = 1.0 - bezier(ARRIVE, u);
    let grow_by = if deeper { 0.035 } else { -0.03 };
    let scale = 1.0 + grow_by * bezier(ARRIVE, u);
    let c = ctx.content_rect().center();
    ctx.graphics_mut(|g| {
        let list = g.entry(LayerId::new(Order::Foreground, key().with("screen-ghost")));
        for s in &shapes {
            let mut s = s.clone();
            grow(&mut s, c, scale);
            fade(&mut s.shape, a);
            list.add(s.clip_rect, s.shape);
        }
    });
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_start_and_end_where_they_should() {
        for c in [ARRIVE, LEAVE, LIFT] {
            assert!(bezier(c, 0.0).abs() < 1e-4);
            assert!((bezier(c, 1.0) - 1.0).abs() < 1e-4);
            let mut last = 0.0;
            for i in 1..=20 {
                let y = bezier(c, i as f32 / 20.0);
                assert!(y + 1e-4 >= last, "monotonic");
                last = y;
            }
        }
        // Arriving is mostly done by the middle, leaving has barely started.
        assert!(bezier(ARRIVE, 0.5) > 0.75);
        assert!(bezier(LEAVE, 0.5) < 0.35);
    }

    #[test]
    fn a_column_springs_in_with_a_small_overshoot_and_settles() {
        let from = -300.0;
        let mut over: f32 = 0.0;
        for i in 0..=60 {
            let x = spring(from, i as f32 / 120.0);
            over = over.max(x);
        }
        assert!(over > 2.0 && over < 14.0, "overshoot {over}");
        assert!(spring(from, 0.45).abs() < 1.0);
    }

    #[test]
    fn a_column_leans_in_before_it_is_thrown_out() {
        let to = -320.0;
        let (lean, _) = throw(0.0, to, LEAN_S);
        assert!(lean > 5.0, "leans toward the middle first: {lean}");
        let (x, done) = throw(0.0, to, LEAN_S + THROW_S + 0.001);
        assert!(done && x == to);
    }
}
