//! The Files app's picker (UIDocumentPickerViewController) for what the app asks of a file picker: a file to open
//! (Import and the like: the picker copies it into the app, `asCopy`) and files it wrote to save somewhere (Export:
//! "Save to Files"). One picker at a time; requests wait their turn. Answers come back on the next frame.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};
use shapecraft_ui::TabletRequest;

/// What a picker came back with.
pub enum Answer {
    /// The copy of the chosen file, for the request `id`.
    Picked(u64, PathBuf),
    /// This many files went where the person chose.
    Saved(usize),
    Cancelled,
}

pub struct Ivars {
    /// The open request this picker answers; None for an export.
    open: Cell<Option<u64>>,
    /// How many files an export hands over.
    count: Cell<usize>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the delegate keeps no Drop state UIKit relies on.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ShapeCraftFilesDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    impl Delegate {
        // UIDocumentPickerDelegate (UIDocumentPickerViewController.h): UIKit asks with respondsToSelector:.
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &AnyObject, urls: &NSArray<NSURL>) {
            let answer = match self.ivars().open.get() {
                Some(id) => match urls.firstObject().and_then(|u| u.path()) {
                    Some(p) => Answer::Picked(id, PathBuf::from(p.to_string())),
                    None => Answer::Cancelled,
                },
                None => Answer::Saved(self.ivars().count.get()),
            };
            finish(answer);
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn was_cancelled(&self, _picker: &AnyObject) {
            finish(Answer::Cancelled);
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, open: Option<u64>, count: usize) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { open: Cell::new(open), count: Cell::new(count) });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// Requests not shown yet (a picker was already up).
    static QUEUE: RefCell<VecDeque<TabletRequest>> = const { RefCell::new(VecDeque::new()) };
    /// The picker's delegate while one is up (the picker only holds it weakly).
    static SHOWING: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    static ANSWERS: RefCell<Vec<Answer>> = const { RefCell::new(Vec::new()) };
}

fn finish(answer: Answer) {
    SHOWING.with(|s| s.borrow_mut().take());
    ANSWERS.with(|a| a.borrow_mut().push(answer));
}

/// Answers since the last call.
pub fn answers() -> Vec<Answer> {
    ANSWERS.with(|a| std::mem::take(&mut *a.borrow_mut()))
}

/// Whether a picker is up or waiting (the app keeps drawing now and then to see its answer).
pub fn busy() -> bool {
    SHOWING.with(|s| s.borrow().is_some()) || QUEUE.with(|q| !q.borrow().is_empty())
}

/// This frame's requests join the queue; the next one shows if no picker is up.
pub fn ask(requests: Vec<TabletRequest>) {
    QUEUE.with(|q| q.borrow_mut().extend(requests));
    if SHOWING.with(|s| s.borrow().is_some()) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    while let Some(r) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
        let shown = match r {
            TabletRequest::Pick { id, .. } => show_open(mtm, id),
            TabletRequest::Saved(dir) => {
                let mut files: Vec<PathBuf> =
                    std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default();
                files.sort();
                !files.is_empty() && show_export(mtm, &files)
            }
        };
        if shown {
            break;
        }
    }
}

fn show_open(mtm: MainThreadMarker, id: u64) -> bool {
    let (Some(cls), Some(ut)) = (AnyClass::get(c"UIDocumentPickerViewController"), AnyClass::get(c"UTType")) else { return false };
    // Any file: STEP, Parasolid and the rest have no system type of their own, and the app sorts them by extension.
    // SAFETY: +[UTType typeWithIdentifier:] (UniformTypeIdentifiers, iOS 14), returns nil for an unknown identifier.
    let data: Option<Retained<AnyObject>> = unsafe { msg_send![ut, typeWithIdentifier: &*NSString::from_str("public.data")] };
    let Some(data) = data else { return false };
    let types = NSArray::from_retained_slice(&[data]);
    // SAFETY: -[UIDocumentPickerViewController initForOpeningContentTypes:asCopy:] (iOS 14).
    let picker: Option<Retained<AnyObject>> = unsafe {
        let a: Allocated<AnyObject> = msg_send![cls, alloc];
        msg_send![a, initForOpeningContentTypes: &*types, asCopy: Bool::YES]
    };
    picker.is_some_and(|p| present(mtm, &p, Delegate::new(mtm, Some(id), 0)))
}

fn show_export(mtm: MainThreadMarker, files: &[PathBuf]) -> bool {
    let Some(cls) = AnyClass::get(c"UIDocumentPickerViewController") else { return false };
    let urls: Vec<Retained<NSURL>> = files.iter().map(|f| NSURL::fileURLWithPath(&NSString::from_str(&f.to_string_lossy()))).collect();
    let urls = NSArray::from_retained_slice(&urls);
    // SAFETY: -[UIDocumentPickerViewController initForExportingURLs:asCopy:] (iOS 14).
    let picker: Option<Retained<AnyObject>> = unsafe {
        let a: Allocated<AnyObject> = msg_send![cls, alloc];
        msg_send![a, initForExportingURLs: &*urls, asCopy: Bool::YES]
    };
    picker.is_some_and(|p| present(mtm, &p, Delegate::new(mtm, None, files.len())))
}

/// Shows the picker over whatever is in front.
fn present(mtm: MainThreadMarker, picker: &AnyObject, delegate: Retained<Delegate>) -> bool {
    let Some(mut vc) = crate::insets::root_controller(mtm) else { return false };
    loop {
        // SAFETY: -[UIViewController presentedViewController].
        let next: Option<Retained<AnyObject>> = unsafe { msg_send![&*vc, presentedViewController] };
        match next {
            Some(n) => vc = n,
            None => break,
        }
    }
    let none: Option<&block2::DynBlock<dyn Fn()>> = None;
    // SAFETY: the picker's delegate property (weak; kept alive in SHOWING) and
    // -[UIViewController presentViewController:animated:completion:].
    unsafe {
        let _: () = msg_send![picker, setDelegate: &*delegate];
        let _: () = msg_send![&*vc, presentViewController: picker, animated: Bool::YES, completion: none];
    }
    SHOWING.with(|s| *s.borrow_mut() = Some(delegate));
    true
}

/// Opens a web page in the browser (Help's links, Report a Problem or Idea…, a newer version's download page).
pub fn open_url(url: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(link) = NSURL::URLWithString(&NSString::from_str(url)) else { return };
    let app = objc2_ui_kit::UIApplication::sharedApplication(mtm);
    let options = objc2_foundation::NSDictionary::<NSString, AnyObject>::new();
    let none: Option<&block2::DynBlock<dyn Fn(Bool)>> = None;
    // SAFETY: -[UIApplication openURL:options:completionHandler:] (iOS 10), on the main thread.
    unsafe {
        let _: () = msg_send![&*app, openURL: &*link, options: &*options, completionHandler: none];
    }
}
