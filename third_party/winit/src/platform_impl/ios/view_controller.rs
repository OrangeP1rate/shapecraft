use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol};
use objc2_ui_kit::{
    UIDevice, UIInterfaceOrientationMask, UIRectEdge, UIResponder, UIStatusBarStyle,
    UIUserInterfaceIdiom, UIView, UIViewController,
};

use super::app_state::{self};
use crate::platform::ios::{ScreenEdge, StatusBarStyle, ValidOrientations};
use crate::window::WindowAttributes;

pub struct ViewControllerState {
    prefers_status_bar_hidden: Cell<bool>,
    preferred_status_bar_style: Cell<UIStatusBarStyle>,
    prefers_home_indicator_auto_hidden: Cell<bool>,
    supported_orientations: Cell<UIInterfaceOrientationMask>,
    preferred_screen_edges_deferring_system_gestures: Cell<UIRectEdge>,
}

declare_class!(
    pub(crate) struct WinitViewController;

    unsafe impl ClassType for WinitViewController {
        #[inherits(UIResponder, NSObject)]
        type Super = UIViewController;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitUIViewController";
    }

    impl DeclaredClass for WinitViewController {
        type Ivars = ViewControllerState;
    }

    unsafe impl WinitViewController {
        #[method(shouldAutorotate)]
        fn should_autorotate(&self) -> bool {
            true
        }

        // ShapeCraft patch: iOS 27.1's vertical status bar (iPhone Duo) off unless the app asks for it
        // (platform::ios::set_vertical_bar), so the clock and Wi-Fi sit in a short row at the top instead of a
        // column down the side (UIVerticalBarBehaviorAutomatic = 0, Disabled = 1).
        #[method(preferredVerticalBarBehavior)]
        fn preferred_vertical_bar_behavior(&self) -> isize {
            if super::VERTICAL_BAR.load(std::sync::atomic::Ordering::Relaxed) { 0 } else { 1 }
        }

        #[method(prefersStatusBarHidden)]
        fn prefers_status_bar_hidden(&self) -> bool {
            self.ivars().prefers_status_bar_hidden.get()
        }

        #[method(preferredStatusBarStyle)]
        fn preferred_status_bar_style(&self) -> UIStatusBarStyle {
            self.ivars().preferred_status_bar_style.get()
        }

        #[method(prefersHomeIndicatorAutoHidden)]
        fn prefers_home_indicator_auto_hidden(&self) -> bool {
            self.ivars().prefers_home_indicator_auto_hidden.get()
        }

        // ShapeCraft patch: the app's lock (platform::ios::set_orientation_lock) wins over the window's choice.
        #[method(supportedInterfaceOrientations)]
        fn supported_orientations(&self) -> UIInterfaceOrientationMask {
            match super::ORIENTATION_LOCK.load(std::sync::atomic::Ordering::Relaxed) {
                1 => UIInterfaceOrientationMask::Portrait,
                2 => UIInterfaceOrientationMask::Landscape,
                _ => self.ivars().supported_orientations.get(),
            }
        }

        #[method(preferredScreenEdgesDeferringSystemGestures)]
        fn preferred_screen_edges_deferring_system_gestures(&self) -> UIRectEdge {
            self.ivars()
                .preferred_screen_edges_deferring_system_gestures
                .get()
        }
    }
);

impl WinitViewController {
    pub(crate) fn set_prefers_status_bar_hidden(&self, val: bool) {
        self.ivars().prefers_status_bar_hidden.set(val);
        self.setNeedsStatusBarAppearanceUpdate();
    }

    pub(crate) fn set_preferred_status_bar_style(&self, val: StatusBarStyle) {
        let val = match val {
            StatusBarStyle::Default => UIStatusBarStyle::Default,
            StatusBarStyle::LightContent => UIStatusBarStyle::LightContent,
            StatusBarStyle::DarkContent => UIStatusBarStyle::DarkContent,
        };
        self.ivars().preferred_status_bar_style.set(val);
        self.setNeedsStatusBarAppearanceUpdate();
    }

    pub(crate) fn set_prefers_home_indicator_auto_hidden(&self, val: bool) {
        self.ivars().prefers_home_indicator_auto_hidden.set(val);
        let os_capabilities = app_state::os_capabilities();
        if os_capabilities.home_indicator_hidden {
            self.setNeedsUpdateOfHomeIndicatorAutoHidden();
        } else {
            os_capabilities.home_indicator_hidden_err_msg("ignoring")
        }
    }

    pub(crate) fn set_preferred_screen_edges_deferring_system_gestures(&self, val: ScreenEdge) {
        let val = {
            assert_eq!(val.bits() & !ScreenEdge::ALL.bits(), 0, "invalid `ScreenEdge`");
            UIRectEdge(val.bits().into())
        };
        self.ivars().preferred_screen_edges_deferring_system_gestures.set(val);
        let os_capabilities = app_state::os_capabilities();
        if os_capabilities.defer_system_gestures {
            self.setNeedsUpdateOfScreenEdgesDeferringSystemGestures();
        } else {
            os_capabilities.defer_system_gestures_err_msg("ignoring")
        }
    }

    pub(crate) fn set_supported_interface_orientations(
        &self,
        mtm: MainThreadMarker,
        valid_orientations: ValidOrientations,
    ) {
        let mask = match (valid_orientations, UIDevice::currentDevice(mtm).userInterfaceIdiom()) {
            (ValidOrientations::LandscapeAndPortrait, UIUserInterfaceIdiom::Phone) => {
                UIInterfaceOrientationMask::AllButUpsideDown
            },
            (ValidOrientations::LandscapeAndPortrait, _) => UIInterfaceOrientationMask::All,
            (ValidOrientations::Landscape, _) => UIInterfaceOrientationMask::Landscape,
            (ValidOrientations::Portrait, UIUserInterfaceIdiom::Phone) => {
                UIInterfaceOrientationMask::Portrait
            },
            (ValidOrientations::Portrait, _) => {
                UIInterfaceOrientationMask::Portrait
                    | UIInterfaceOrientationMask::PortraitUpsideDown
            },
        };
        self.ivars().supported_orientations.set(mask);
        #[allow(deprecated)]
        UIViewController::attemptRotationToDeviceOrientation(mtm);
        // ShapeCraft patch: the iOS 16+ way (the call above does nothing there).
        let sel = objc2::sel!(setNeedsUpdateOfSupportedInterfaceOrientations);
        if self.respondsToSelector(sel) {
            // SAFETY: UIViewController.h, iOS 16; takes nothing, returns nothing.
            unsafe {
                let _: () = objc2::msg_send![self, setNeedsUpdateOfSupportedInterfaceOrientations];
            }
        }
    }

    pub(crate) fn new(
        mtm: MainThreadMarker,
        window_attributes: &WindowAttributes,
        view: &UIView,
    ) -> Retained<Self> {
        // These are set properly below, we just to set them to something in the meantime.
        let this = mtm.alloc().set_ivars(ViewControllerState {
            prefers_status_bar_hidden: Cell::new(false),
            preferred_status_bar_style: Cell::new(UIStatusBarStyle::Default),
            prefers_home_indicator_auto_hidden: Cell::new(false),
            supported_orientations: Cell::new(UIInterfaceOrientationMask::All),
            preferred_screen_edges_deferring_system_gestures: Cell::new(UIRectEdge::empty()),
        });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };

        this.set_prefers_status_bar_hidden(
            window_attributes.platform_specific.prefers_status_bar_hidden,
        );

        this.set_preferred_status_bar_style(
            window_attributes.platform_specific.preferred_status_bar_style,
        );

        this.set_supported_interface_orientations(
            mtm,
            window_attributes.platform_specific.valid_orientations,
        );

        this.set_prefers_home_indicator_auto_hidden(
            window_attributes.platform_specific.prefers_home_indicator_hidden,
        );

        this.set_preferred_screen_edges_deferring_system_gestures(
            window_attributes.platform_specific.preferred_screen_edges_deferring_system_gestures,
        );

        this.setView(Some(view));
        watch_hinge(&this, view);

        this
    }
}

/// ShapeCraft patch: iOS 27.1's UIHingeInteraction on the root view tells whether the device folds (its update
/// carries a hinge, or nil where there is none). UIHinge.h / UIHingeInteraction.h in the iOS 27.2 SDK.
fn watch_hinge(vc: &WinitViewController, view: &UIView) {
    use std::sync::atomic::Ordering::Relaxed;

    use objc2::rc::{Allocated, Weak};
    use objc2::runtime::{AnyClass, AnyObject};
    let Some(cls) = AnyClass::get("UIHingeInteraction") else { return };
    let weak = Weak::from(vc);
    let handler = block2::RcBlock::new(move |_interaction: *mut AnyObject, update: *mut AnyObject| {
        if update.is_null() {
            return;
        }
        // SAFETY: UIHingeInteractionUpdate's `hinge` property (nullable UIHinge) and UIHinge's `status`.
        let hinge: *mut AnyObject = unsafe { objc2::msg_send![update, hinge] };
        if hinge.is_null() {
            return;
        }
        super::FOLDABLE.store(true, Relaxed);
        let status: isize = unsafe { objc2::msg_send![hinge, status] };
        super::HINGE_STATUS.store(status.clamp(0, 3) as u8, Relaxed);
        if !super::FOLD_ORIENTATIONS.load(Relaxed) || status == 0 {
            return;
        }
        // Closed: the front screen, upright. Open (partly or fully): the inside screen, on its side.
        let lock = if status == 1 { 1 } else { 2 };
        if super::ORIENTATION_LOCK.swap(lock, Relaxed) != lock {
            if let Some(vc) = weak.load() {
                // SAFETY: UIViewController.h, iOS 16; takes nothing, returns nothing.
                unsafe {
                    let _: () = objc2::msg_send![&*vc, setNeedsUpdateOfSupportedInterfaceOrientations];
                }
            }
        }
    });
    // SAFETY: -[UIHingeInteraction initWithUpdateHandler:] and -[UIView addInteraction:] (iOS 27.1 / 11).
    unsafe {
        let a: Allocated<AnyObject> = msg_send_id![cls, alloc];
        let interaction: Option<Retained<AnyObject>> = msg_send_id![a, initWithUpdateHandler: &*handler];
        if let Some(i) = interaction {
            let _: () = objc2::msg_send![view, addInteraction: &*i];
        }
    }
}
