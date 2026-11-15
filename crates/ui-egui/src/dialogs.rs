//! The system's file and message dialogs. Desktops get `rfd`. Android has no dialog `rfd` can open from a native
//! activity: opening, importing and exporting go through the system's picker instead, asked for by the Android app
//! (see `TabletRequest`); the folder picks left here (Work Folder, backups) go to one shared folder
//! (`set_android_folder`, the app's folder under Android/data, reachable from a computer over USB), and messages go
//! to the system log (`adb logcat -s ShapeCraft`).

#[cfg(not(target_os = "android"))]
pub use rfd::{FileDialog, MessageDialog, MessageLevel};

#[cfg(target_os = "android")]
pub use android::{FileDialog, MessageDialog, MessageLevel, set_android_folder};

#[cfg(target_os = "android")]
mod android {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    static FOLDER: OnceLock<PathBuf> = OnceLock::new();

    /// Where saves and exports go. Set once at start; later calls are ignored.
    pub fn set_android_folder(dir: PathBuf) {
        let _ = FOLDER.set(dir);
    }

    fn folder() -> Option<PathBuf> {
        let dir = FOLDER.get()?.clone();
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    /// The same builder as `crate::dialogs::FileDialog`, for the calls ShapeCraft makes.
    #[derive(Default)]
    pub struct FileDialog {
        name: Option<String>,
    }

    impl FileDialog {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn add_filter(self, _name: impl Into<String>, _extensions: &[impl ToString]) -> Self {
            self
        }
        pub fn set_title(self, _title: impl Into<String>) -> Self {
            self
        }
        pub fn set_directory(self, _dir: impl AsRef<Path>) -> Self {
            self
        }
        pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
            self.name = Some(name.into());
            self
        }
        /// No picker yet: nothing is chosen.
        pub fn pick_file(self) -> Option<PathBuf> {
            None
        }
        pub fn pick_folder(self) -> Option<PathBuf> {
            folder()
        }
        /// The shared folder, under the suggested name (only its last part, so it can't point anywhere else).
        pub fn save_file(self) -> Option<PathBuf> {
            let name = self.name.as_deref().map(Path::new).and_then(Path::file_name)?.to_owned();
            Some(folder()?.join(name))
        }
    }

    pub enum MessageLevel {
        Error,
    }

    /// Messages go to the system log, the only place a failed start can still show them.
    #[derive(Default)]
    pub struct MessageDialog {
        text: String,
    }

    impl MessageDialog {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn set_level(self, _level: MessageLevel) -> Self {
            self
        }
        pub fn set_title(mut self, title: impl Into<String>) -> Self {
            self.text = format!("{}: {}", title.into(), self.text);
            self
        }
        pub fn set_description(mut self, text: impl Into<String>) -> Self {
            self.text.push_str(&text.into());
            self
        }
        /// Always "seen", like a dialog with one OK button.
        pub fn show(self) -> bool {
            log::error!("{}", self.text);
            true
        }
    }
}

/// One system dialog open while the window keeps answering. `taken` answers `None` while it is open, then
/// `Some(None)` when it was cancelled or `Some(path)` when a path was chosen.
#[derive(Default)]
pub struct Pick {
    rx: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
}

impl std::fmt::Debug for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_open() { "Pick(open)" } else { "Pick(idle)" })
    }
}

impl Pick {
    /// Opens the dialog that `wait` drives, replacing an answer not yet taken. On Linux the wait happens on
    /// another thread: a portal dialog blocks the thread that waits for it, and that thread is the one that has to
    /// keep answering the compositor — left blocked, the desktop declares the app not responding ten seconds in.
    /// macOS wants its dialogs on the main thread (and pumps its own events there), and Win32 dialogs pump
    /// messages too, so those run where they always did and answer at once.
    pub fn start(&mut self, wait: impl FnOnce() -> Option<std::path::PathBuf> + Send + 'static) {
        self.rx = Self::open(wait).rx;
    }

    /// The dialog opened and its wait begun, on whichever thread waits for it.
    fn open(wait: impl FnOnce() -> Option<std::path::PathBuf> + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        #[cfg(target_os = "linux")]
        std::thread::spawn(move || {
            let _ = tx.send(wait());
        });
        #[cfg(not(target_os = "linux"))]
        let _ = tx.send(wait());
        Self { rx: Some(rx) }
    }

    pub fn is_open(&self) -> bool {
        self.rx.is_some()
    }

    /// The dialog's answer, read once it has closed.
    pub fn taken(&mut self) -> Option<Option<std::path::PathBuf>> {
        match self.rx.as_ref()?.try_recv() {
            Ok(answer) => {
                self.rx = None;
                Some(answer)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                Some(None)
            }
        }
    }
}

/// What carries on when a dialog closes, with the path chosen (None when it was cancelled).
type Done<A> = Box<dyn FnOnce(&mut A, Option<std::path::PathBuf>) + Send>;

/// Files and folders being asked for while the window keeps answering, each with what carries on when its dialog
/// closes. Polled once a frame.
pub struct Pending<A> {
    picks: Vec<(Pick, Done<A>)>,
}

impl<A> Default for Pending<A> {
    fn default() -> Self {
        Self { picks: Vec::new() }
    }
}

impl<A> Pending<A> {
    /// Asks for a file to open. `build` shapes the dialog; `done` runs when it closes, with the path (None when it
    /// was cancelled).
    pub fn open_file(
        &mut self,
        build: impl FnOnce(FileDialog) -> FileDialog + Send + 'static,
        done: impl FnOnce(&mut A, Option<std::path::PathBuf>) + Send + 'static,
    ) {
        if self.is_open() {
            return;
        }
        self.push(Pick::open(move || build(FileDialog::new()).pick_file()), done);
    }

    /// Asks for a folder, the same way.
    pub fn open_folder(
        &mut self,
        build: impl FnOnce(FileDialog) -> FileDialog + Send + 'static,
        done: impl FnOnce(&mut A, Option<std::path::PathBuf>) + Send + 'static,
    ) {
        if self.is_open() {
            return;
        }
        self.push(Pick::open(move || build(FileDialog::new()).pick_folder()), done);
    }

    /// Asks where to save a file, the same way.
    pub fn save_file(
        &mut self,
        build: impl FnOnce(FileDialog) -> FileDialog + Send + 'static,
        done: impl FnOnce(&mut A, Option<std::path::PathBuf>) + Send + 'static,
    ) {
        if self.is_open() {
            return;
        }
        self.push(Pick::open(move || build(FileDialog::new()).save_file()), done);
    }

    fn push(&mut self, pick: Pick, done: impl FnOnce(&mut A, Option<std::path::PathBuf>) + Send + 'static) {
        self.picks.push((pick, Box::new(done)));
    }

    /// A dialog is up: the window keeps drawing frames so its answer is taken as soon as it closes, and a second
    /// click on the same menu item doesn't stack another dialog on top.
    pub fn is_open(&self) -> bool {
        !self.picks.is_empty()
    }

    /// Takes over another list's dialogs (those opened while this one was taken out to be polled).
    pub fn picks_from(&mut self, other: Self) {
        self.picks.extend(other.picks);
    }

    /// Carries on what the user answered.
    pub fn poll(&mut self, app: &mut A) {
        for i in (0..self.picks.len()).rev() {
            let Some(answer) = self.picks[i].0.taken() else { continue };
            let (_, done) = self.picks.remove(i);
            done(app, answer);
        }
    }
}

#[cfg(test)]
mod pending_tests {
    use super::*;

    #[derive(Default)]
    struct Answers(Vec<Option<std::path::PathBuf>>);

    fn wait_for(p: &mut Pending<Answers>, a: &mut Answers, n: usize) {
        for _ in 0..500 {
            p.poll(a);
            if a.0.len() >= n {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// An answer carries on once, cancelled ones too, and nothing is left open after.
    #[test]
    fn answers_carry_on_once_and_close() {
        let mut p = Pending::<Answers>::default();
        let mut a = Answers::default();
        p.push(Pick::open(|| Some("/tmp/x.step".into())), |a: &mut Answers, path| a.0.push(path));
        p.push(Pick::open(|| None), |a: &mut Answers, path| a.0.push(path));
        assert!(p.is_open());
        wait_for(&mut p, &mut a, 2);
        assert_eq!(a.0.len(), 2);
        assert!(a.0.contains(&Some("/tmp/x.step".into())) && a.0.contains(&None));
        assert!(!p.is_open());
        p.poll(&mut a);
        assert_eq!(a.0.len(), 2, "each answer carries on only once");
    }

    /// Dialogs opened while a list was taken out to be polled are kept when it goes back.
    #[test]
    fn picks_opened_meanwhile_are_kept() {
        let mut kept = Pending::<Answers>::default();
        kept.push(Pick::open(|| None), |a: &mut Answers, path| a.0.push(path));
        let mut meanwhile = Pending::<Answers>::default();
        meanwhile.push(Pick::open(|| Some("/tmp/y.png".into())), |a: &mut Answers, path| a.0.push(path));
        kept.picks_from(meanwhile);
        let mut a = Answers::default();
        wait_for(&mut kept, &mut a, 2);
        assert_eq!(a.0.len(), 2);
    }
}
