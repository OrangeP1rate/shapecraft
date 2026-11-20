//! Running a plugin: wasmtime with the Pulley interpreter (no compiler in the app, no executable memory, so it also
//! runs on iOS), the host functions of `wit/shapecraft-plugin.wit` with every permission checked, and limits on
//! memory and time. A trap (a panic in the plugin, running out of memory, Cancel) is an error, never a crash.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};

use crate::images::{self, Mask, Picture};
use crate::manifest::Permissions;
use crate::results;
use crate::store::KeyFile;
use crate::{Error, Result, net, signin};

/// Generated from the WIT (kept apart: the macro's code means wasmtime's `Result`, not ours).
mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "plugin",
    });
}

use bindings::Plugin;
use bindings::shapecraft::plugin::host as wit;

/// The settings an engine runs plugins with. Building (`precompile`) must use the same, or loading refuses the file.
pub fn engine_config() -> Result<Config> {
    let mut c = Config::new();
    c.target("pulley64").map_err(|e| Error::Runtime(e.to_string()))?;
    c.wasm_component_model(true);
    c.epoch_interruption(true);
    Ok(c)
}

/// The engine every plugin shares, with a clock that ticks every 20 ms for time limits and Cancel.
pub fn engine() -> Result<Engine> {
    static ENGINE: OnceLock<std::result::Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let engine = engine_config().and_then(|c| Engine::new(&c).map_err(|e| Error::Runtime(e.to_string())));
            let engine = engine.map_err(|e| e.to_string())?;
            let weak = engine.weak();
            let _ = std::thread::Builder::new().name("plugin-clock".into()).spawn(move || {
                while let Some(e) = weak.upgrade() {
                    e.increment_epoch();
                    drop(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
            Ok(engine)
        })
        .clone()
        .map_err(Error::Runtime)
}

/// Turns a component (`.wasm`) into the file the app loads (`.cwasm`): Pulley bytecode for this API's engine.
#[cfg(feature = "compile")]
pub fn precompile(component: &[u8]) -> Result<Vec<u8>> {
    let engine = Engine::new(&engine_config()?).map_err(|e| Error::Runtime(e.to_string()))?;
    engine.precompile_component(component).map_err(|e| Error::Runtime(format!("{e:#}")))
}

/// Loads a precompiled plugin.
///
/// The one `unsafe` outside kernel-occt (AGENTS.md): wasmtime can only check that a precompiled file was made by
/// the same wasmtime with the same settings, not that it's harmless. So only files whose SHA-256 matched the
/// registry's index (built by our CI from reviewed source) or that the person added on purpose from a file, with
/// plugin authoring turned on, ever get here.
pub fn load(bytes: &[u8]) -> Result<Component> {
    let engine = engine()?;
    // SAFETY: see above; the bytes are a checked registry download or a file the person chose to trust.
    unsafe { Component::deserialize(&engine, bytes) }.map_err(|e| Error::Load(format!("{e:#}")))
}

/// Progress, Cancel and new results of a plugin's running action, shared with the UI.
#[derive(Default)]
pub struct Activity {
    pub cancel: AtomicBool,
    pub busy: AtomicBool,
    pub progress: Mutex<(f32, String)>,
    /// Results made since the UI last looked.
    pub new_results: Mutex<Vec<results::Entry>>,
}

impl Activity {
    pub fn progress(&self) -> (f32, String) {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn take_results(&self) -> Vec<results::Entry> {
        self.new_results.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

/// Web pages plugins asked to open; the UI opens them on its own thread (where the tablet page and phones can).
pub fn take_urls() -> Vec<String> {
    URLS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

static URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The view captured for an action (see `wit` `capture`).
#[derive(Clone, Debug)]
pub struct Capture {
    pub view: Picture,
    pub model: Mask,
    pub selection: Option<Mask>,
}

/// What a plugin's store holds: who it is, what it may do, and the call in progress.
pub struct State {
    pub id: String,
    pub perms: Permissions,
    pub dir: PathBuf,
    pub activity: Arc<Activity>,
    pub capture: Option<Capture>,
    /// When the current call must be done by (panels are quick; actions run until done or cancelled).
    pub deadline: Option<Instant>,
    limits: StoreLimits,
}

/// Memory a plugin may use (pictures in flight included).
const MEMORY: usize = 1 << 30;

impl State {
    pub fn new(id: &str, perms: Permissions, dir: PathBuf, activity: Arc<Activity>) -> Self {
        let limits = StoreLimitsBuilder::new().memory_size(MEMORY).instances(8).tables(64).memories(8).build();
        Self { id: id.to_string(), perms, dir, activity, capture: None, deadline: None, limits }
    }

    fn cancelled(&self) -> bool {
        self.activity.cancel.load(Ordering::Relaxed)
    }
}

/// One instance of a plugin in its own store, ready for calls.
pub struct Instance {
    store: Store<State>,
    plugin: Plugin,
}

impl Instance {
    pub fn new(component: &Component, state: State) -> Result<Self> {
        let engine = engine()?;
        let mut linker: Linker<State> = Linker::new(&engine);
        Plugin::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s).map_err(|e| Error::Runtime(format!("{e:#}")))?;
        let mut store = Store::new(&engine, state);
        store.limiter(|s| &mut s.limits);
        store.epoch_deadline_callback(|ctx| {
            let s = ctx.data();
            if s.cancelled() && s.deadline.is_none() {
                return Err(wasmtime::Error::msg("cancelled"));
            }
            if s.deadline.is_some_and(|d| Instant::now() > d) {
                return Err(wasmtime::Error::msg("the plugin took too long"));
            }
            Ok(UpdateDeadline::Continue(1))
        });
        store.set_epoch_deadline(1);
        let plugin = Plugin::instantiate(&mut store, component, &linker).map_err(|e| Error::Runtime(format!("{e:#}")))?;
        let mut me = Self { store, plugin };
        me.store.data_mut().deadline = Some(Instant::now() + Duration::from_secs(10));
        let started = me.plugin.call_start(&mut me.store);
        me.store.data_mut().deadline = None;
        started.map_err(trap)?.map_err(Error::Plugin)?;
        Ok(me)
    }

    pub fn panel(&mut self, id: &str, values: &str) -> Result<String> {
        self.store.data_mut().deadline = Some(Instant::now() + Duration::from_secs(5));
        self.store.set_epoch_deadline(1);
        let out = self.plugin.call_panel(&mut self.store, id, values);
        self.store.data_mut().deadline = None;
        out.map_err(trap)?.map_err(Error::Plugin)
    }

    pub fn run(&mut self, action: &str, values: &str, capture: Option<Capture>) -> Result<String> {
        let data = self.store.data_mut();
        data.capture = capture;
        data.deadline = None;
        data.activity.cancel.store(false, Ordering::Relaxed);
        self.store.set_epoch_deadline(1);
        let out = self.plugin.call_run(&mut self.store, action, values);
        self.store.data_mut().capture = None;
        out.map_err(trap)?.map_err(Error::Plugin)
    }
}

fn trap(e: wasmtime::Error) -> Error {
    let text = format!("{e:#}");
    if text.contains("cancelled") {
        Error::Cancelled
    } else if text.contains("took too long") {
        Error::Plugin("the plugin took too long to answer".into())
    } else {
        Error::Trap(text.lines().next().unwrap_or("").to_string())
    }
}

fn to_picture(i: wit::Image) -> Picture {
    Picture { width: i.width, height: i.height, pixels: i.pixels }
}

fn from_picture(p: Picture) -> wit::Image {
    wit::Image { width: p.width, height: p.height, pixels: p.pixels }
}

fn from_mask(m: Mask) -> wit::Mask {
    wit::Mask { width: m.width, height: m.height, values: m.values }
}

impl wit::Host for State {
    fn log(&mut self, level: wit::Level, message: String) {
        let message: String = message.chars().take(2000).collect();
        let id = &self.id;
        match level {
            wit::Level::Debug => log::debug!("[plugin {id}] {message}"),
            wit::Level::Info => log::info!("[plugin {id}] {message}"),
            wit::Level::Warn => log::warn!("[plugin {id}] {message}"),
            wit::Level::Error => log::error!("[plugin {id}] {message}"),
        }
    }

    fn fetch(&mut self, req: wit::Request) -> std::result::Result<wit::Response, String> {
        if self.cancelled() {
            return Err("cancelled".into());
        }
        let a = net::allowed(&self.perms, &req.url)?;
        let headers: Vec<(String, String)> = req.headers.into_iter().map(|h| (h.name, h.value)).collect();
        let answer = net::fetch(&req.method, &req.url, &headers, req.body, req.timeout, &a.host)?;
        Ok(wit::Response {
            status: answer.status,
            headers: answer.headers.into_iter().map(|(name, value)| wit::Header { name, value }).collect(),
            body: answer.body,
        })
    }

    fn secret(&mut self, name: String) -> Option<String> {
        KeyFile::secrets(&self.dir).get(&name)
    }

    fn set_secret(&mut self, name: String, value: Option<String>) -> std::result::Result<(), String> {
        KeyFile::secrets(&self.dir).set(&name, value.as_deref())
    }

    fn setting(&mut self, name: String) -> Option<String> {
        KeyFile::settings(&self.dir).get(&name)
    }

    fn set_setting(&mut self, name: String, value: Option<String>) -> std::result::Result<(), String> {
        if let Some(v) = &value
            && serde_json::from_str::<serde_json::Value>(v).is_err()
        {
            return Err(format!("setting \"{name}\" isn't JSON text"));
        }
        KeyFile::settings(&self.dir).set(&name, value.as_deref())
    }

    fn progress(&mut self, fraction: f32, message: String) -> bool {
        let f = if fraction.is_finite() { fraction.min(1.0) } else { -1.0 };
        if let Ok(mut p) = self.activity.progress.lock() {
            *p = (f, message.chars().take(120).collect());
        }
        !self.cancelled()
    }

    fn capture(&mut self) -> Option<wit::ViewCapture> {
        let c = self.capture.clone()?;
        Some(wit::ViewCapture { view: from_picture(c.view), model: from_mask(c.model), selection: c.selection.map(from_mask) })
    }

    fn decode_image(&mut self, bytes: Vec<u8>) -> std::result::Result<wit::Image, String> {
        images::decode(&bytes).map(from_picture)
    }

    fn encode_png(&mut self, picture: wit::Image) -> std::result::Result<Vec<u8>, String> {
        images::encode_png(&to_picture(picture))
    }

    fn encode_jpeg(&mut self, picture: wit::Image, quality: u8) -> std::result::Result<Vec<u8>, String> {
        images::encode_jpeg(&to_picture(picture), quality)
    }

    fn fit(&mut self, picture: wit::Image, width: u32, height: u32) -> std::result::Result<wit::Image, String> {
        images::fit(&to_picture(picture), width, height).map(from_picture)
    }

    fn composite(&mut self, base: wit::Image, top: wit::Image, amount: wit::Mask) -> std::result::Result<wit::Image, String> {
        let m = Mask { width: amount.width, height: amount.height, values: amount.values };
        images::composite(&to_picture(base), &to_picture(top), &m).map(from_picture)
    }

    fn base64_encode(&mut self, bytes: Vec<u8>) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn base64_decode(&mut self, text: String) -> std::result::Result<Vec<u8>, String> {
        use base64::Engine as _;
        let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(clean).map_err(|e| format!("not base64 ({e})"))
    }

    fn sha256(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).to_vec()
    }

    fn random(&mut self, len: u32) -> Vec<u8> {
        let mut v = vec![0u8; len.min(4096) as usize];
        if getrandom::fill(&mut v).is_err() {
            // No OS randomness is a broken system; the plugin gets zeros and its PKCE check will fail loudly.
            v.fill(0);
        }
        v
    }

    fn now(&mut self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    fn add_result(&mut self, picture: wit::Image, title: String, details: String) {
        match results::save(&self.dir, &to_picture(picture), &title, &details) {
            Ok(e) => {
                if let Ok(mut v) = self.activity.new_results.lock() {
                    v.push(e);
                }
            }
            Err(e) => log::warn!("[plugin {}] a result couldn't be kept: {e}", self.id),
        }
    }

    fn sign_in(&mut self, url: String) -> std::result::Result<wit::SignInResult, String> {
        if !self.perms.sign_in {
            return Err("this plugin didn't ask to sign in".into());
        }
        let a = net::allowed(&self.perms, &url)?;
        if !a.https {
            return Err("sign-in pages must be https".into());
        }
        let waiting = signin::listen()?;
        let redirect = waiting.redirect.clone();
        if let Ok(mut v) = URLS.lock() {
            v.push(signin::fill(&url, &redirect));
        }
        let query = waiting.wait(Duration::from_secs(300), &self.activity.cancel)?;
        Ok(wit::SignInResult { redirect, query })
    }

    fn open_url(&mut self, url: String) -> std::result::Result<(), String> {
        let a = net::parse(&url)?;
        if !a.https {
            return Err("only https:// pages can be opened".into());
        }
        if let Ok(mut v) = URLS.lock() {
            v.push(url);
        }
        Ok(())
    }
}
