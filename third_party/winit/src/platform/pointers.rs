//! ShapeCraft patch: pointers with their tool, shared by the Android and iOS backends (see PATCH.md).

use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Finger,
    Stylus,
    Mouse,
    Eraser,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
    Cancel,
    /// Moving without touching (a stylus above the glass, a mouse).
    Hover,
    /// Hovering ended (the stylus left the range).
    HoverExit,
    /// A mouse button went down or up: `button` says which.
    ButtonPress,
    ButtonRelease,
    /// A scroll wheel: `scroll` is (horizontal, vertical) in wheel steps.
    Scroll,
}

/// One pointer at one moment, in physical pixels of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Record {
    pub tool: Tool,
    pub id: u64,
    pub phase: Phase,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    /// The buttons held (Android's MotionEvent button state bits).
    pub buttons: u32,
    /// For `ButtonPress` / `ButtonRelease`: the button that changed (the same bits).
    pub button: u32,
    pub scroll: (f32, f32),
}

/// Button bits, as Android numbers them.
pub const PRIMARY: u32 = 1;
pub const SECONDARY: u32 = 2;
pub const TERTIARY: u32 = 4;
pub const STYLUS_PRIMARY: u32 = 32;
pub const STYLUS_SECONDARY: u32 = 64;

/// More than this unread means no one is reading: the oldest go.
const CAP: usize = 4096;

static RECORDS: Mutex<Vec<Record>> = Mutex::new(Vec::new());

pub(crate) fn push(r: Record) {
    if let Ok(mut v) = RECORDS.lock() {
        if v.len() >= CAP {
            v.remove(0);
        }
        v.push(r);
    }
}

/// Everything recorded since the last call, oldest first.
pub fn take() -> Vec<Record> {
    RECORDS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}
