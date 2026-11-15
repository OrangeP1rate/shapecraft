//! ShapeCraft patch: the UIKit scene life cycle, which apps built with the iOS 27 SDK must use (they fail to launch
//! otherwise). The app's Info.plist names `WinitSceneDelegate` in its `UIApplicationSceneManifest`; when UIKit
//! connects the scene, every window made so far moves into it, and windows made later are put there at once.

use std::cell::RefCell;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, AnyProtocol};
use objc2::{declare_class, msg_send, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject};
use objc2_ui_kit::UIResponder;

use super::window::WinitUIWindow;

thread_local! {
    static SCENE: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static WINDOWS: RefCell<Vec<Weak<WinitUIWindow>>> = const { RefCell::new(Vec::new()) };
}

declare_class!(
    pub(crate) struct WinitSceneDelegate;

    unsafe impl ClassType for WinitSceneDelegate {
        #[inherits(NSObject)]
        type Super = UIResponder;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitSceneDelegate";
    }

    impl DeclaredClass for WinitSceneDelegate {}

    unsafe impl WinitSceneDelegate {
        #[method(scene:willConnectToSession:options:)]
        fn will_connect(&self, scene: &AnyObject, _session: &AnyObject, _options: &AnyObject) {
            let scene: Retained<AnyObject> = unsafe { Retained::retain(scene as *const _ as *mut AnyObject) }.unwrap();
            SCENE.with(|s| *s.borrow_mut() = Some(scene.clone()));
            let windows: Vec<Retained<WinitUIWindow>> =
                WINDOWS.with(|w| w.borrow().iter().filter_map(Weak::load).collect());
            for window in windows {
                place(&window, &scene);
                window.makeKeyAndVisible();
            }
        }
    }
);

/// Registers the delegate class (and its protocol, which UIKit checks) before `UIApplicationMain` looks it up.
pub(crate) fn register() {
    let class = WinitSceneDelegate::class();
    if let Some(protocol) = AnyProtocol::get("UIWindowSceneDelegate") {
        unsafe {
            objc2::ffi::class_addProtocol(
                class as *const _ as *mut objc2::ffi::objc_class,
                protocol as *const _ as *const objc2::ffi::objc_protocol,
            );
        }
    }
}

/// A new window: into the scene if it is connected already, else when it connects.
pub(crate) fn adopt(_mtm: MainThreadMarker, window: &Retained<WinitUIWindow>) {
    WINDOWS.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|weak| weak.load().is_some());
        w.push(Weak::from_retained(window));
    });
    if let Some(scene) = SCENE.with(|s| s.borrow().clone()) {
        place(window, &scene);
    }
}

fn place(window: &WinitUIWindow, scene: &AnyObject) {
    let _: () = unsafe { msg_send![window, setWindowScene: scene] };
    // Fill the scene: its coordinate space is the screen's, the fold and status areas included.
    let bounds: objc2_foundation::CGRect = unsafe {
        let coords: Retained<AnyObject> = objc2::msg_send_id![scene, coordinateSpace];
        msg_send![&coords, bounds]
    };
    let _: () = unsafe { msg_send![window, setFrame: bounds] };
}
