//! Where the interface must not put controls, in the interface's own (rotating) points: UIKit's safe area of the
//! root view, widened by the regions the system reserves for hardware on iOS 27.1 and newer (the iPhone Duo's
//! camera and status column, which the safe area alone doesn't report).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{ClassType, msg_send};
use objc2_core_foundation::CGRect;
use objc2_foundation::{MainThreadMarker, NSArray, NSObjectProtocol};
use objc2_ui_kit::{UIApplication, UISceneActivationState, UIView, UIWindowScene};

/// Insets in system points (top, right, bottom, left), plus the view's size, or None before there is a window.
pub struct Measured {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub size: (f32, f32),
    /// The reserved regions found, as (x, y, w, h), for the log.
    pub regions: Vec<(f32, f32, f32, f32)>,
    /// A small reserved region in a top corner (the iPhone Duo's clock, Wi-Fi and camera when held sideways), as
    /// (x, y, w, h). The interface keeps its controls out of it instead of giving up the whole top row: the status bar
    /// row UIKit's safe area reports is free everywhere else, as in the reference on the Duo.
    pub corner: Option<(f32, f32, f32, f32)>,
    /// How many of the regions found were inactive (for the log).
    pub inactive: usize,
}

/// The key window's root view controller, to present system screens (the Files picker) over.
pub fn root_controller(mtm: MainThreadMarker) -> Option<Retained<AnyObject>> {
    let app = UIApplication::sharedApplication(mtm);
    for scene in app.connectedScenes() {
        if !scene.isKindOfClass(UIWindowScene::class()) {
            continue;
        }
        // SAFETY: the class was checked just above.
        let scene = unsafe { Retained::cast_unchecked::<UIWindowScene>(scene) };
        if let Some(vc) = scene.keyWindow().and_then(|w| w.rootViewController()) {
            // SAFETY: every Objective-C object is an AnyObject.
            return Some(unsafe { Retained::cast_unchecked::<AnyObject>(vc) });
        }
    }
    None
}

/// Asks iOS to apply the app's orientation lock now (it turns the screen to a side the app allows).
pub fn update_orientations() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(vc) = root_controller(mtm) else { return };
    let sel = objc2::sel!(setNeedsUpdateOfSupportedInterfaceOrientations);
    // SAFETY: -[NSObject respondsToSelector:], then -[UIViewController setNeedsUpdateOfSupportedInterfaceOrientations]
    // (iOS 16), on the main thread.
    unsafe {
        let yes: bool = msg_send![&*vc, respondsToSelector: sel];
        if yes {
            let _: () = msg_send![&*vc, setNeedsUpdateOfSupportedInterfaceOrientations];
        }
    }
}

fn root_view(mtm: MainThreadMarker) -> Option<Retained<UIView>> {
    let app = UIApplication::sharedApplication(mtm);
    for scene in app.connectedScenes() {
        if !scene.isKindOfClass(UIWindowScene::class()) {
            continue;
        }
        if !matches!(scene.activationState(), UISceneActivationState::ForegroundActive | UISceneActivationState::ForegroundInactive) {
            continue;
        }
        // SAFETY: the class was checked just above.
        let scene = unsafe { Retained::cast_unchecked::<UIWindowScene>(scene) };
        let window = scene.keyWindow()?;
        return window.rootViewController().and_then(|vc| vc.viewIfLoaded());
    }
    None
}

pub fn measure() -> Option<Measured> {
    let mtm = MainThreadMarker::new()?;
    let view = root_view(mtm)?;
    let insets = view.safeAreaInsets();
    let bounds = view.bounds();
    let (w, h) = (bounds.size.width as f32, bounds.size.height as f32);
    let mut m = Measured {
        top: insets.top as f32,
        // (UIKit's own safe area, before the reserved regions widen it.)
        right: insets.right as f32,
        bottom: insets.bottom as f32,
        left: insets.left as f32,
        size: (w, h),
        regions: Vec::new(),
        corner: None,
        inactive: 0,
    };
    // iOS 27.1: -[UIView reservedRegionsOfKind:] with +[UIViewReservedRegionKind occlusionRegionKind].
    let Some(kind_class) = AnyClass::get(c"UIViewReservedRegionKind") else { return Some(m) };
    // SAFETY: both selectors are in the iOS 27.1 SDK headers (UIViewReservedRegion.h, UIView.h), and the class only
    // exists where they do.
    // Inactive ones too (UIViewReservedRegionQueryOptionsIncludeInactive = 1): after the iPhone Duo once showed the app
    // boxed on its inside screen, iOS went on drawing the clock in a floating pill and reporting its block only as
    // inactive — still where the clock sits, so the controls keep out of it all the same.
    let regions: Option<Retained<NSArray<AnyObject>>> = unsafe {
        let kind: Option<Retained<AnyObject>> = msg_send![kind_class, occlusionRegionKind];
        kind.map(|kind| msg_send![&*view, reservedRegionsOfKind: &*kind, options: 1usize])
    };
    let mut found: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut idle: Vec<(f32, f32, f32, f32)> = Vec::new();
    for region in regions.iter().flat_map(|r| r.iter()) {
        // SAFETY: `frame` and `isActive` are the region's properties in the same header.
        let (frame, active): (CGRect, bool) = unsafe { (msg_send![&*region, frame], msg_send![&*region, isActive]) };
        m.inactive += usize::from(!active);
        let r = (frame.origin.x as f32, frame.origin.y as f32, frame.size.width as f32, frame.size.height as f32);
        if !active {
            idle.push(r);
        }
        found.push(r);
    }
    // Biggest first, so a camera inside the status block is already clear by the time it comes up.
    found.sort_by(|a, b| (b.2 * b.3).total_cmp(&(a.2 * a.3)));
    // A corner block (narrow and short, touching the top or bottom and one side) becomes the corner rather than a whole
    // edge's inset. At the top the status row's inset goes with it; held
    // the other way up the block may sit at the bottom (the camera end), and the controls keep out of it there.
    // Folded shut (upright, 466 wide) the Duo's status block is 200 × 82 in the top right: a corner too.
    let narrow = if w > h { w / 3.0 } else { w / 2.0 };
    if let Some(&c) = found
        .iter()
        .find(|(x, y, rw, rh)| (*y <= 1.0 || y + rh >= h - 1.0) && (*x <= 1.0 || x + rw >= w - 1.0) && *rw < narrow && *rh < h / 4.0)
    {
        m.corner = Some(c);
        if c.1 <= 1.0 {
            m.top = 0.0;
        }
    }
    for (x, y, rw, rh) in found {
        m.regions.push((x, y, rw, rh));
        // An inactive region only counts as the status corner (above): the opened Duo also reports one along the
        // bottom edge (62 × 40), which isn't covering anything.
        if idle.contains(&(x, y, rw, rh)) {
            continue;
        }
        // The corner, and anything inside it (the folded Duo's camera sits in its status block).
        if m.corner.is_some_and(|(cx, cy, cw, ch)| x >= cx - 1.0 && y >= cy - 1.0 && x + rw <= cx + cw + 1.0 && y + rh <= cy + ch + 1.0) {
            continue;
        }
        let clear = y + rh <= m.top || x >= w - m.right || y >= h - m.bottom || x + rw <= m.left;
        if clear {
            continue;
        }
        // Push one edge in past the region: of the edges it touches (or is nearest to), the one that costs the
        // least room. The Duo's status block sits in a corner, 84 pt wide and 174 tall: the side gives way, not the top.
        let reach = [y + rh, w - x, h - y, x + rw]; // inset needed from top, right, bottom, left
        let dist = [y, w - (x + rw), h - (y + rh), x];
        let near = dist.iter().copied().fold(f32::INFINITY, f32::min);
        let cost = |e: usize| if e % 2 == 0 { reach[e] * w } else { reach[e] * h };
        let edge = (0..4).filter(|e| dist[*e] <= near + 1.0).min_by(|a, b| cost(*a).total_cmp(&cost(*b))).unwrap_or(0);
        match edge {
            0 => m.top = m.top.max(reach[0]),
            1 => m.right = m.right.max(reach[1]),
            2 => m.bottom = m.bottom.max(reach[2]),
            _ => m.left = m.left.max(reach[3]),
        }
    }
    Some(m)
}

/// UIKit's safe area of the root view alone (top, right, bottom, left), for the log.
pub fn uikit_safe() -> Option<(f32, f32, f32, f32)> {
    let view = root_view(MainThreadMarker::new()?)?;
    let i = view.safeAreaInsets();
    Some((i.top as f32, i.right as f32, i.bottom as f32, i.left as f32))
}

/// For the log: the status bar's frame (UIStatusBarManager), the trait collection's vertical bar edge, and how many
/// occlusion regions the plain query returns.
pub fn status_probe() -> String {
    let Some(mtm) = MainThreadMarker::new() else { return String::new() };
    let Some(view) = root_view(mtm) else { return String::new() };
    let app = UIApplication::sharedApplication(mtm);
    let mut out = String::new();
    for scene in app.connectedScenes() {
        if !scene.isKindOfClass(UIWindowScene::class()) {
            continue;
        }
        // SAFETY: -[UIWindowScene statusBarManager] (iOS 13) and -[UIStatusBarManager statusBarFrame].
        let frame: Option<CGRect> = unsafe {
            let m: Option<Retained<AnyObject>> = msg_send![&*scene, statusBarManager];
            m.map(|m| msg_send![&*m, statusBarFrame])
        };
        if let Some(f) = frame {
            out += &format!("sbar=({},{},{},{}) ", f.origin.x, f.origin.y, f.size.width, f.size.height);
        }
    }
    // SAFETY: -[UITraitCollection verticalBarEdge] (iOS 27.1), checked first.
    unsafe {
        let traits: Retained<AnyObject> = msg_send![&*view, traitCollection];
        let answers: bool = msg_send![&*traits, respondsToSelector: objc2::sel!(verticalBarEdge)];
        if answers {
            let edge: isize = msg_send![&*traits, verticalBarEdge];
            out += &format!("edge={edge} ");
        }
    }
    if let Some(kind_class) = AnyClass::get(c"UIViewReservedRegionKind") {
        // SAFETY: as in `measure`.
        let n: usize = unsafe {
            let kind: Option<Retained<AnyObject>> = msg_send![kind_class, occlusionRegionKind];
            kind.map_or(0, |kind| {
                let r: Retained<NSArray<AnyObject>> = msg_send![&*view, reservedRegionsOfKind: &*kind];
                r.count()
            })
        };
        out += &format!("plain_regions={n}");
    }
    out
}

/// Tells UIKit the app's vertical-bar choice changed (winit's view controller answers
/// `preferredVerticalBarBehavior` from `winit::platform::ios::vertical_bar`). iOS 27.1 and newer only.
pub fn update_vertical_bar() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = UIApplication::sharedApplication(mtm);
    for scene in app.connectedScenes() {
        if !scene.isKindOfClass(UIWindowScene::class()) {
            continue;
        }
        // SAFETY: the class was checked just above.
        let scene = unsafe { Retained::cast_unchecked::<UIWindowScene>(scene) };
        let Some(vc) = scene.keyWindow().and_then(|w| w.rootViewController()) else { continue };
        let sel = objc2::sel!(setNeedsUpdateOfVerticalBarConfiguration);
        if vc.respondsToSelector(sel) {
            // SAFETY: the selector is in the iOS 27.1 SDK's UIViewController.h, takes nothing and returns nothing,
            // and the controller just said it answers it.
            unsafe {
                let _: () = msg_send![&*vc, setNeedsUpdateOfVerticalBarConfiguration];
            }
        }
    }
}
