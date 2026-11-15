#![allow(clippy::let_unit_value)]

mod app_state;
mod event_loop;
mod monitor;
mod notification_center;
mod scene; // ShapeCraft patch
mod view;
mod view_controller;
mod window;

use std::fmt;

use crate::event::DeviceId as RootDeviceId;

pub(crate) use self::event_loop::{
    ActiveEventLoop, EventLoop, EventLoopProxy, OwnedDisplayHandle,
    PlatformSpecificEventLoopAttributes,
};
pub(crate) use self::monitor::{MonitorHandle, VideoModeHandle};
pub(crate) use self::window::{PlatformSpecificWindowAttributes, Window, WindowId};
pub(crate) use crate::cursor::{
    NoCustomCursor as PlatformCustomCursor, NoCustomCursor as PlatformCustomCursorSource,
};
pub(crate) use crate::icon::NoIcon as PlatformIcon;
pub(crate) use crate::platform_impl::Fullscreen;

/// There is no way to detect which device that performed a certain event in
/// UIKit (i.e. you can't differentiate between different external keyboards,
/// or whether it was the main touchscreen, assistive technologies, or some
/// other pointer device that caused a touch event).
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId;

impl DeviceId {
    pub const fn dummy() -> Self {
        DeviceId
    }
}

pub(crate) const DEVICE_ID: RootDeviceId = RootDeviceId(DeviceId);

/// ShapeCraft patch: the app's choice of iOS 27.1's vertical bar (platform::ios::set_vertical_bar).
pub(crate) static VERTICAL_BAR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// ShapeCraft patch: the app's orientation lock (platform::ios::set_orientation_lock): 0 none (the window's valid
/// orientations), 1 portrait, 2 landscape.
pub(crate) static ORIENTATION_LOCK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// ShapeCraft patch: the device has a hinge (iOS 27.1's UIHingeInteraction reported one): a foldable phone.
pub(crate) static FOLDABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// ShapeCraft patch: the hinge's last UIHingeStatus (0 unknown, 1 closed, 2 partly open, 3 fully open).
pub(crate) static HINGE_STATUS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// ShapeCraft patch: the app asked for one orientation per fold state (platform::ios::set_fold_orientations).
pub(crate) static FOLD_ORIENTATIONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEventExtra {}

#[derive(Debug)]
pub enum OsError {}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "os error")
    }
}
