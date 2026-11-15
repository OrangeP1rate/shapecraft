//! What a long command is doing, for the screen that waits for it: a line of text and how far along it is. The
//! command (on its own thread) updates it; the window reads it each frame.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Progress {
    text: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    /// A new stage: its line of text and how many steps it has (0 when that isn't known).
    pub fn stage(&self, text: impl Into<String>, total: u64) {
        if let Ok(mut t) = self.text.lock() {
            *t = text.into();
        }
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    /// One more step of the stage done.
    pub fn step(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    /// The stage's steps done so far.
    pub fn set_done(&self, done: u64) {
        self.done.store(done, Ordering::Relaxed);
    }

    /// The text, steps done and steps in all (0 when unknown).
    pub fn snapshot(&self) -> (String, u64, u64) {
        let text = self.text.lock().map(|t| t.clone()).unwrap_or_default();
        (text, self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }
}
