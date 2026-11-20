//! Plugins in the app: Preferences ▸ Plugins (installed, Browse, Add with its permissions, each plugin's settings),
//! the sheet a plugin opens (Generative Render), and the control channel's `plugins.*` methods.
//!
//! The plugins themselves run in `shapecraft-plugin-host` (sandboxed WebAssembly). This draws what they describe:
//! every control is ours, so a plugin looks and behaves like the rest of the app.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use egui::{Align2, Color32, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, TextureHandle, Ui, pos2, vec2};
use serde_json::{Map, Value, json};
use shapecraft_plugin_host::index::{self, Index};
use shapecraft_plugin_host::panel::{Choice, Tone};
use shapecraft_plugin_host::store::KeyFile;
use shapecraft_plugin_host::{Capture, Control, Installed, Library, Loaded, Manifest, Package, Panel, Ticket, results};

use crate::report::{Style, button, error_bar, link, spinner, switch, text_box};
use crate::theme::{self, Tokens, radius};

/// The official plugin the Generative Render button offers when it isn't added yet.
pub const GENERATIVE: &str = "generative-render";

thread_local! {
    static STATE: RefCell<Plugins> = RefCell::new(Plugins::new());
}

fn with<R>(f: impl FnOnce(&mut Plugins) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// What a panel shows and what the person set in it.
#[derive(Default)]
struct PanelState {
    panel: Option<Panel>,
    values: Map<String, Value>,
    /// A newer description is on its way.
    refresh: Option<Ticket>,
    /// The values changed since the last description was asked for.
    dirty: bool,
    run: Option<(String, Ticket)>,
    message: Option<(Tone, String)>,
    /// Secrets being typed (not yet saved).
    drafts: HashMap<String, String>,
}

/// The sheet a plugin's panel opens in (style "sheet"), or a plain card.
struct Sheet {
    plugin: String,
    panel: String,
    capture: Option<Capture>,
    /// The view as it was when the sheet opened.
    snapshot: Option<TextureHandle>,
    /// The newest result since then (shown in place of the snapshot).
    shown: Option<results::Entry>,
}

/// Adding a plugin: what it is and what it asks for, waiting for the person's OK.
struct Confirm {
    manifest: Manifest,
    source: Source,
}

/// The card that follows "Add Plugin": what's happening, then what to do next.
enum AddCard {
    Working { name: String, icon: String, size: u64, since: f64 },
    Done { id: String, name: String, icon: String },
    Failed { name: String, icon: String, error: String, retry: Option<Box<index::Entry>> },
}

/// What a plugin card asks the rest of the app for.
pub(crate) enum Request {
    /// Preferences ▸ Plugins, with the plugin just added open.
    OpenPlugins,
}

enum Source {
    Listed(Box<index::Entry>),
    File(PathBuf),
}

pub struct Plugins {
    lib: Option<Library>,
    installed: Vec<Installed>,
    loaded: HashMap<String, Loaded>,
    start_errors: HashMap<String, String>,
    panels: HashMap<(String, String), PanelState>,
    results: HashMap<String, Vec<results::Entry>>,
    textures: HashMap<PathBuf, TextureHandle>,
    browse: Option<Receiver<Result<Index, String>>>,
    index: Option<Result<Index, String>>,
    confirm: Option<Confirm>,
    adding: Option<(String, Receiver<Result<Package, String>>)>,
    add_card: Option<AddCard>,
    /// A finished add, held until the card has shown "working" long enough to read.
    add_done: Option<Result<Installed, String>>,
    requests: Vec<Request>,
    /// The listing being added, for Try Again.
    retry: Option<Box<index::Entry>>,
    notice: Option<(Tone, String)>,
    sheet: Option<Sheet>,
    /// The Generative Render button pressed without the plugin: offer to add it.
    offer: bool,
    /// Preferences ▸ Plugins: Browse shown instead of Installed, and the plugin whose settings are open.
    browsing: bool,
    selected: Option<String>,
    file_pick: crate::dialogs::Pick,
    save_pick: Option<(PathBuf, crate::dialogs::Pick)>,
    urls: Vec<String>,
}

fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

impl Plugins {
    fn new() -> Self {
        let lib = Library::open();
        let installed = lib.as_ref().map(Library::list).unwrap_or_default();
        Self {
            lib,
            installed,
            loaded: HashMap::new(),
            start_errors: HashMap::new(),
            panels: HashMap::new(),
            results: HashMap::new(),
            textures: HashMap::new(),
            browse: None,
            index: None,
            confirm: None,
            adding: None,
            add_card: None,
            add_done: None,
            requests: Vec::new(),
            retry: None,
            notice: None,
            sheet: None,
            offer: false,
            browsing: false,
            selected: None,
            file_pick: Default::default(),
            save_pick: None,
            urls: Vec::new(),
        }
    }

    fn reload_list(&mut self) {
        self.installed = self.lib.as_ref().map(Library::list).unwrap_or_default();
        let ids: Vec<String> = self.installed.iter().filter(|i| i.enabled).map(|i| i.manifest.id.clone()).collect();
        self.loaded.retain(|id, _| ids.contains(id));
    }

    fn installed(&self, id: &str) -> Option<&Installed> {
        self.installed.iter().find(|i| i.manifest.id == id)
    }

    /// The running plugin, started on first use.
    fn ensure(&mut self, id: &str) -> Option<&Loaded> {
        if !self.loaded.contains_key(id) {
            let inst = self.installed(id)?.clone();
            if !inst.enabled {
                return None;
            }
            let lib = self.lib.as_ref()?;
            match lib.start(&inst) {
                Ok(l) => {
                    self.start_errors.remove(id);
                    self.loaded.insert(id.to_string(), l);
                }
                Err(e) => {
                    log::warn!("plugin {id} didn't start: {e}");
                    self.start_errors.insert(id.to_string(), e.to_string());
                    return None;
                }
            }
        }
        self.loaded.get(id)
    }

    fn state(&mut self, plugin: &str, panel: &str) -> &mut PanelState {
        let dir = self.installed(plugin).map(|i| i.dir.clone());
        self.panels.entry((plugin.to_string(), panel.to_string())).or_insert_with(|| {
            let values = dir
                .and_then(|d| KeyFile::settings(&d).get(&format!("panel.{panel}")))
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            PanelState { values, dirty: true, ..Default::default() }
        })
    }

    fn results_of(&mut self, plugin: &str) -> &mut Vec<results::Entry> {
        let dir = self.installed(plugin).map(|i| i.dir.clone());
        self.results.entry(plugin.to_string()).or_insert_with(|| dir.map(|d| results::list(&d)).unwrap_or_default())
    }

    /// Everything that happens without drawing: answers coming back, new results, downloads.
    fn poll(&mut self, ctx: &egui::Context) {
        let mut busy = false;
        // Panels: refresh when the values changed; take new descriptions and finished actions.
        let keys: Vec<(String, String)> = self.panels.keys().cloned().collect();
        for (plugin, panel) in keys {
            if self.ensure(&plugin).is_none() {
                continue;
            }
            let Some(loaded) = self.loaded.get(&plugin) else { continue };
            let new_results = loaded.activity.take_results();
            let st_key = (plugin.clone(), panel.clone());
            let refresh_ticket = self
                .panels
                .get(&st_key)
                .filter(|st| st.dirty && st.refresh.is_none())
                .map(|st| loaded.panel(&panel, &Value::Object(st.values.clone())));
            if !new_results.is_empty() {
                let list = self.results_of(&plugin);
                for r in new_results.iter().rev() {
                    list.insert(0, r.clone());
                }
                if let Some(s) = self.sheet.as_mut().filter(|s| s.plugin == plugin)
                    && let Some(newest) = new_results.last()
                {
                    s.shown = Some(newest.clone());
                }
            }
            let Some(st) = self.panels.get_mut(&st_key) else { continue };
            if let Some(t) = refresh_ticket {
                st.refresh = Some(t);
                st.dirty = false;
            }
            if let Some(answer) = st.refresh.as_ref().and_then(Ticket::poll) {
                st.refresh = None;
                match answer.map_err(|e| e.to_string()).and_then(|t| Panel::parse(&t)) {
                    Ok(p) => {
                        for c in &p.controls {
                            if let (Some(id), Some(v)) = (c.id(), c.default_value()) {
                                st.values.entry(id.to_string()).or_insert(v);
                            }
                        }
                        st.panel = Some(p);
                    }
                    Err(e) => st.message = Some((Tone::Error, e)),
                }
            }
            if let Some((action, t)) = &st.run
                && let Some(answer) = t.poll()
            {
                let action = action.clone();
                st.run = None;
                st.dirty = true;
                st.message = Some(match answer {
                    Ok(text) => {
                        let msg = serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                            .unwrap_or_default();
                        (Tone::Good, msg)
                    }
                    Err(shapecraft_plugin_host::Error::Cancelled) => (Tone::Dim, "Cancelled.".into()),
                    Err(e) if action == "secret-saved" && e.to_string().starts_with("no action") => {
                        (Tone::Good, "Saved on this computer.".into())
                    }
                    Err(e) => (Tone::Error, e.to_string()),
                });
                // Newly saved sign-ins or keys change the other panels too.
                for ((p, _), other) in self.panels.iter_mut() {
                    if *p == plugin {
                        other.dirty = true;
                    }
                }
                break;
            }
            busy |= st.run.is_some() || st.refresh.is_some();
        }
        if let Some(rx) = &self.browse {
            busy = true;
            if let Ok(r) = rx.try_recv() {
                self.index = Some(r);
                self.browse = None;
            }
        }
        if let Some((id, rx)) = &self.adding {
            busy = true;
            if let Ok(r) = rx.try_recv() {
                let id = id.clone();
                self.adding = None;
                let _ = id;
                self.add_done = Some(r.and_then(|pkg| {
                    self.lib.as_ref().ok_or("no plugins folder".to_string())?.install(&pkg, false).map_err(|e| e.to_string())
                }));
            }
        }
        // The card shows "working" for at least a moment, so a quick add still reads as something that happened.
        let now = ctx.input(|i| i.time);
        if let Some(AddCard::Working { since, .. }) = &self.add_card {
            busy = true;
            if *since >= 0.0
                && now - *since >= 1.2
                && let Some(done) = self.add_done.take()
            {
                self.finish_add(done);
            }
        } else if let Some(done) = self.add_done.take() {
            self.finish_add(done);
        }
        if let Some(Some(path)) = self.file_pick.taken() {
            self.confirm_file(path);
        }
        if let Some((src, pick)) = &mut self.save_pick
            && let Some(answer) = pick.taken()
        {
            if let Some(dest) = answer {
                let dest = if dest.extension().is_none() { dest.with_extension("png") } else { dest };
                if let Err(e) = std::fs::copy(&*src, &dest) {
                    self.notice = Some((Tone::Error, format!("The picture couldn't be saved: {e}")));
                }
            }
            self.save_pick = None;
        }
        self.urls.extend(shapecraft_plugin_host::runtime::take_urls());
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    fn finish_add(&mut self, done: Result<Installed, String>) {
        let (name, icon, retry) = match self.add_card.take() {
            Some(AddCard::Working { name, icon, .. }) => (name, icon, self.retry.take()),
            _ => (String::new(), String::new(), None),
        };
        match done {
            Ok(i) => {
                let (id, n, ic) = (i.manifest.id.clone(), i.manifest.name.clone(), i.manifest.icon.clone());
                self.added(i);
                self.notice = None;
                self.add_card = Some(AddCard::Done { id, name: n, icon: ic });
            }
            Err(e) => self.add_card = Some(AddCard::Failed { name, icon, error: e, retry }),
        }
    }

    fn added(&mut self, i: Installed) {
        let name = i.manifest.name.clone();
        let id = i.manifest.id.clone();
        self.loaded.remove(&id);
        self.panels.retain(|(p, _), _| *p != id);
        self.reload_list();
        self.notice = Some((Tone::Good, format!("{name} is added.")));
        self.selected = Some(id);
        self.browsing = false;
    }

    fn fetch_index(&mut self) {
        let (tx, rx) = mpsc::channel();
        let v = app_version().to_string();
        std::thread::spawn(move || {
            let _ = tx.send(index::fetch(&v));
        });
        self.browse = Some(rx);
        self.index = None;
    }

    fn start_add(&mut self, entry: &index::Entry) {
        self.confirm = Some(Confirm { manifest: entry.manifest.clone(), source: Source::Listed(Box::new(entry.clone())) });
    }

    fn confirm_file(&mut self, path: PathBuf) {
        match shapecraft_plugin_host::library::read_package(&path) {
            Ok(pkg) => self.confirm = Some(Confirm { manifest: pkg.manifest, source: Source::File(path) }),
            Err(e) => self.notice = Some((Tone::Error, format!("That file isn't a plugin this ShapeCraft can use: {e}"))),
        }
    }

    fn accept(&mut self, now: f64) {
        let Some(c) = self.confirm.take() else { return };
        let (name, icon) = (c.manifest.name.clone(), c.manifest.icon.clone());
        match c.source {
            Source::Listed(entry) => {
                let size = entry.size;
                self.retry = Some(entry.clone());
                let (tx, rx) = mpsc::channel();
                let v = app_version().to_string();
                std::thread::spawn(move || {
                    let _ = tx.send(index::download(&v, entry.as_ref()));
                });
                self.adding = Some((name.clone(), rx));
                self.add_card = Some(AddCard::Working { name, icon, size, since: now });
            }
            Source::File(path) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                self.retry = None;
                let r = shapecraft_plugin_host::library::read_package(&path)
                    .and_then(|pkg| {
                        self.lib.as_ref().ok_or(shapecraft_plugin_host::Error::Io("no plugins folder".into()))?.install(&pkg, true)
                    })
                    .map_err(|e| e.to_string());
                self.add_card = Some(AddCard::Working { name, icon, size, since: now });
                self.add_done = Some(r);
            }
        }
    }

    fn set_value(&mut self, plugin: &str, panel: &str, id: &str, v: Value) {
        let dir = self.installed(plugin).map(|i| i.dir.clone());
        let st = self.state(plugin, panel);
        if st.values.get(id) == Some(&v) {
            return;
        }
        st.values.insert(id.to_string(), v);
        st.dirty = true;
        let text = Value::Object(st.values.clone()).to_string();
        if let Some(d) = dir {
            let _ = KeyFile::settings(&d).set(&format!("panel.{panel}"), Some(&text));
        }
    }

    fn press(&mut self, plugin: &str, panel: &str, action: &str, capture: Option<Capture>) {
        let values = Value::Object(self.state(plugin, panel).values.clone());
        let Some(loaded) = self.ensure(plugin) else { return };
        let t = loaded.run(action, &values, capture);
        let st = self.state(plugin, panel);
        st.message = None;
        st.run = Some((action.to_string(), t));
    }

    fn texture(&mut self, ctx: &egui::Context, path: &PathBuf) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(path) {
            return Some(t.clone());
        }
        let pic = shapecraft_plugin_host::images::decode(&std::fs::read(path).ok()?).ok()?;
        let img = egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &pic.pixels);
        let t = ctx.load_texture(path.to_string_lossy(), img, egui::TextureOptions::LINEAR);
        if self.textures.len() > 64 {
            self.textures.clear();
        }
        self.textures.insert(path.clone(), t.clone());
        Some(t)
    }
}

// ——— What the rest of the app calls ———

/// Each frame: answers, overlays (the sheet, Add confirmations), and web pages plugins asked to open.
pub(crate) fn frame(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    with(|p| {
        p.poll(&ctx);
        if p.sheet.is_some() {
            sheet(p, ui);
        }
        if p.offer {
            offer_card(p, ui);
        }
        if p.confirm.is_some() {
            confirm_card(p, ui);
        }
        if p.add_card.is_some() {
            add_card(p, ui);
        }
        for url in std::mem::take(&mut p.urls) {
            crate::landing::open_link(&url);
        }
    });
}

/// The plugin that answers Visualization's Generative Render button, when one is added and on.
pub(crate) fn visualization_plugin() -> Option<(String, String)> {
    with(|p| {
        p.installed.iter().filter(|i| i.enabled).find_map(|i| {
            let c = i.manifest.commands.iter().find(|c| c.place == "visualization" && !c.panel.is_empty())?;
            Some((i.manifest.id.clone(), c.panel.clone()))
        })
    })
}

/// Opens a plugin's panel as a sheet over the window, with the view captured for its buttons.
pub(crate) fn open_sheet(plugin: &str, panel: &str, capture: Option<Capture>) {
    with(|p| {
        if p.ensure(plugin).is_none() {
            let why = p.start_errors.get(plugin).cloned().unwrap_or_else(|| "it's turned off".into());
            p.notice = Some((Tone::Error, format!("The plugin couldn't start: {why}")));
            p.offer = false;
            return;
        }
        p.state(plugin, panel).dirty = true;
        p.offer = false;
        p.sheet = Some(Sheet { plugin: plugin.into(), panel: panel.into(), capture, snapshot: None, shown: None });
    });
}

/// The Generative Render button without the plugin: a card offering to add it.
pub(crate) fn offer_generative() {
    with(|p| {
        p.offer = true;
        if p.index.is_none() && p.browse.is_none() {
            p.fetch_index();
        }
    });
}

/// Whether a plugin sheet or card has the keyboard (the workspace leaves keys alone then).
pub(crate) fn takes_keys() -> bool {
    with(|p| p.sheet.is_some() || p.offer || p.confirm.is_some() || p.add_card.is_some())
}

/// What the plugin cards asked the rest of the app for since the last frame.
pub(crate) fn take_requests() -> Vec<Request> {
    with(|p| std::mem::take(&mut p.requests))
}

// ——— Preferences ▸ Plugins ———

pub(crate) fn prefs_tab(ui: &mut Ui, r: Rect) {
    with(|p| prefs(p, ui, r));
}

fn prefs(p: &mut Plugins, ui: &mut Ui, r: Rect) {
    let painter = ui.painter().clone();
    let x0 = r.left() + 24.0;
    let x1 = r.right() - 24.0;
    let mut y = r.top() + 92.0;
    painter.text(
        pos2(r.center().x, y),
        Align2::CENTER_CENTER,
        "Add features you want. Plugins run sealed off and can only do what they ask for when you add them.",
        theme::body(11.0),
        Tokens::text_dim(),
    );
    y += 26.0;
    // Installed | Browse
    let seg = Rect::from_center_size(pos2(r.center().x, y), vec2(200.0, 24.0));
    let picked = segmented_at(ui, seg, "plugins-seg", &["Installed", "Browse"], usize::from(p.browsing));
    if let Some(k) = picked {
        p.browsing = k == 1;
        if p.browsing && p.index.is_none() && p.browse.is_none() {
            p.fetch_index();
        }
    }
    y += 24.0;
    if let Some((tone, text)) = p.notice.clone() {
        let g = painter.layout(text, theme::body(11.5), tone_color(tone), x1 - x0);
        painter.galley(pos2(x0, y - 6.0), g.clone(), tone_color(tone));
        y += g.size().y + 6.0;
    }
    let body = Rect::from_min_max(pos2(r.left() + 1.0, y), pos2(r.right() - 1.0, r.bottom() - 60.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body.shrink2(vec2(23.0, 0.0))));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt("plugins-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if p.browsing {
            browse_list(p, ui);
        } else {
            installed_list(p, ui);
        }
    });
    // Footer: adding from a file is for people writing plugins.
    let fy = r.bottom() - 26.0;
    if !crate::tablet::on_tablet() {
        let rect = link(ui, pos2(r.left() + 16.0, fy), "Add from File…", "plugins-file");
        if ui.interact(rect, ui.scope_id().with("plugins-file-btn"), Sense::click()).clicked() && !p.file_pick.is_open() {
            p.file_pick.start(|| crate::dialogs::FileDialog::new().add_filter("ShapeCraft plugin", &["scplugin"]).pick_file());
        }
    }
}

fn installed_list(p: &mut Plugins, ui: &mut Ui) {
    if p.installed.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No plugins yet").font(theme::body(14.0)).color(Tokens::text_strong()));
            ui.label(egui::RichText::new("Open Browse to see what you can add.").font(theme::body(11.5)).color(Tokens::text_dim()));
        });
        return;
    }
    let list = p.installed.clone();
    for inst in &list {
        ui.push_id(("installed", &inst.manifest.id), |ui| installed_row(p, ui, inst));
    }
}

fn installed_row(p: &mut Plugins, ui: &mut Ui, inst: &Installed) {
    {
        let id = inst.manifest.id.clone();
        let open = p.selected.as_deref() == Some(id.as_str());
        let w = ui.available_width();
        let desc = p.start_errors.get(&id).cloned().unwrap_or_else(|| inst.manifest.description.clone());
        let g = ui.painter().layout(desc, theme::body(10.5), Tokens::text_dim(), w - 150.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, (g.size().y + 40.0).max(58.0)), Sense::click());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, radius(8), if open { Tokens::row_selected() } else { Tokens::chip() });
        painter.rect_stroke(rect, radius(8), Stroke::new(1.0, Tokens::chip_border()), StrokeKind::Inside);
        plugin_icon(&painter, Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(30.0, 30.0)), &inst.manifest.icon);
        let tx = rect.left() + 52.0;
        let mut title = format!("{}  {}", inst.manifest.name, inst.manifest.version);
        if inst.sideloaded {
            title.push_str("  · added from a file");
        }
        painter.text(pos2(tx, rect.top() + 19.0), Align2::LEFT_CENTER, title, theme::strong(13.0), Tokens::text_strong());
        painter.galley(pos2(tx, rect.top() + 30.0), g, Tokens::text_dim());
        let mut on = inst.enabled;
        switch(ui, rect.right() - 12.0, rect.center().y, ("plugin-on", hash(&id)), &mut on);
        if on != inst.enabled {
            if let Some(lib) = &p.lib {
                let _ = lib.set_enabled(&id, on);
            }
            p.reload_list();
        }
        if resp.clicked() {
            p.selected = if open { None } else { Some(id.clone()) };
        }
        if open {
            ui.push_id(("plugin-settings", &id), |ui| settings_of(p, ui, inst));
        }
    }
}

fn settings_of(p: &mut Plugins, ui: &mut Ui, inst: &Installed) {
    let id = inst.manifest.id.clone();
    ui.add_space(2.0);
    if inst.enabled {
        p.state(&id, "settings");
        column(p, ui, &id, "settings");
    } else {
        note(ui, "Turned off. Turn it on to change its settings.", Tone::Dim);
    }
    ui.add_space(4.0);
    note(ui, &format!("It may: {}.", inst.manifest.permission_lines().join("; ").to_lowercase()), Tone::Dim);
    let row = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover()).0;
    let (remove, _) = button(ui, row.right(), row.center().y, "Remove", Style::Plain, true);
    if !inst.manifest.repository.is_empty() {
        let r = link(ui, pos2(row.left(), row.center().y), "Source and help", "plugin-repo");
        if ui.interact(r, ui.scope_id().with("plugin-repo-btn"), Sense::click()).clicked() {
            p.urls.push(inst.manifest.repository.clone());
        }
    }
    if remove {
        if let Some(lib) = &p.lib {
            if let Some(l) = p.loaded.remove(&id) {
                l.cancel();
            }
            match lib.remove(&id) {
                Ok(()) => p.notice = Some((Tone::Dim, format!("{} is removed, with its settings, keys and pictures.", inst.manifest.name))),
                Err(e) => p.notice = Some((Tone::Error, e.to_string())),
            }
        }
        p.panels.retain(|(pid, _), _| *pid != id);
        p.results.remove(&id);
        p.selected = None;
        p.reload_list();
    }
    ui.add_space(8.0);
}

fn browse_list(p: &mut Plugins, ui: &mut Ui) {
    let now = ui.ctx().input(|i| i.time);
    match &p.index {
        None => {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 80.0), Sense::hover());
            spinner(ui.painter(), r.center() - vec2(0.0, 10.0), now);
            ui.painter().text(
                r.center() + vec2(0.0, 20.0),
                Align2::CENTER_CENTER,
                "Getting the list…",
                theme::body(12.0),
                Tokens::text_dim(),
            );
        }
        Some(Err(e)) => {
            let e = e.clone();
            note(ui, &format!("The plugin list couldn't be loaded: {e}"), Tone::Error);
            let row = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover()).0;
            if button(ui, row.right(), row.center().y, "Try Again", Style::Plain, true).0 {
                p.fetch_index();
            }
        }
        Some(Ok(idx)) => {
            let entries = idx.plugins.clone();
            for tier in ["official", "community"] {
                let these: Vec<&index::Entry> = entries.iter().filter(|e| e.tier == tier).collect();
                if these.is_empty() {
                    continue;
                }
                ui.label(
                    egui::RichText::new(if tier == "official" { "From ShapeCraft" } else { "From the Community (reviewed)" })
                        .font(theme::strong(11.5))
                        .color(Tokens::text_dim()),
                );
                for e in these {
                    ui.push_id(("browse", &e.manifest.id), |ui| browse_row(p, ui, e));
                }
            }
            if entries.is_empty() {
                note(ui, "Nothing is listed for this version yet.", Tone::Dim);
            }
            note(ui, "Anyone can make one: see docs/plugins.md in ShapeCraft's repository.", Tone::Dim);
        }
    }
}

fn browse_row(p: &mut Plugins, ui: &mut Ui, e: &index::Entry) {
    let w = ui.available_width();
    let lines = e.manifest.permission_lines();
    let g = ui.painter().layout(e.manifest.description.clone(), theme::body(10.5), Tokens::text_dim(), w - 160.0);
    let gh = g.size().y;
    let h = (46.0 + gh + 14.0 * lines.len() as f32).max(58.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, radius(8), Tokens::chip());
    painter.rect_stroke(rect, radius(8), Stroke::new(1.0, Tokens::chip_border()), StrokeKind::Inside);
    plugin_icon(&painter, Rect::from_center_size(pos2(rect.left() + 26.0, rect.top() + 26.0), vec2(30.0, 30.0)), &e.manifest.icon);
    let tx = rect.left() + 52.0;
    painter.text(
        pos2(tx, rect.top() + 18.0),
        Align2::LEFT_CENTER,
        format!("{}  {}", e.manifest.name, e.manifest.version),
        theme::strong(13.0),
        Tokens::text_strong(),
    );
    painter.galley(pos2(tx, rect.top() + 29.0), g, Tokens::text_dim());
    for (k, l) in lines.iter().enumerate() {
        painter.text(
            pos2(tx, rect.top() + 38.0 + gh + 14.0 * k as f32),
            Align2::LEFT_CENTER,
            format!("• {l}"),
            theme::body(10.0),
            Tokens::text_faint(),
        );
    }
    let have = p.installed(&e.manifest.id).map(|i| i.manifest.version.clone());
    let newer = have.as_deref().is_some_and(|v| {
        shapecraft_plugin_host::manifest::parse_version(v) < shapecraft_plugin_host::manifest::parse_version(&e.manifest.version)
    });
    let busy = p.adding.as_ref().is_some_and(|(n, _)| *n == e.manifest.name)
        || matches!(&p.add_card, Some(AddCard::Working { name, .. }) if *name == e.manifest.name);
    let (label, enabled) = match (&have, newer, busy) {
        (_, _, true) => ("Adding…", false),
        (Some(_), true, _) => ("Update", true),
        (Some(_), false, _) => ("Added", false),
        (None, ..) => ("Add", true),
    };
    if button(ui, rect.right() - 12.0, rect.top() + 22.0, label, Style::Primary, enabled && p.adding.is_none()).0 {
        p.start_add(e);
    }
}

fn plugin_icon(p: &egui::Painter, r: Rect, which: &str) {
    p.rect_filled(r, radius(8), if which == "sparkle" { Tokens::accent() } else { theme::c(0x4a4a4e) });
    let icon = match which {
        "sparkle" => crate::icons::Icon::CameraSparkle,
        "image" => crate::icons::Icon::Screenshot,
        _ => crate::icons::Icon::Insert,
    };
    crate::icons::paint(p, r.shrink(6.0), icon, Color32::WHITE);
}

fn hash(s: &str) -> usize {
    s.bytes().fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(usize::from(b)))
}

fn tone_color(t: Tone) -> Color32 {
    match t {
        Tone::Dim => Tokens::text_dim(),
        Tone::Normal => Tokens::text_strong(),
        Tone::Good => theme::fixed(0x5fc46a),
        Tone::Warning => theme::fixed(0xe8a93a),
        Tone::Error => Tokens::danger(),
    }
}

fn note(ui: &mut Ui, text: &str, tone: Tone) {
    if tone == Tone::Error {
        error_bar(ui, text);
        return;
    }
    let w = ui.available_width();
    let g = ui.painter().layout(text.to_string(), theme::body(11.5), tone_color(tone), w);
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, tone_color(tone));
}

/// A segmented control filling `r`; returns a newly picked segment.
fn segmented_at(ui: &Ui, r: Rect, id: &str, items: &[&str], current: usize) -> Option<usize> {
    let p = ui.painter();
    p.rect_filled(r, radius(7), theme::c(0x2c2c2c));
    p.rect_stroke(r, radius(7), Stroke::new(1.0, theme::c(0x3a3a3c)), StrokeKind::Inside);
    let w = r.width() / items.len().max(1) as f32;
    let mut picked = None;
    for (k, item) in items.iter().enumerate() {
        let s = Rect::from_min_size(pos2(r.left() + w * k as f32, r.top()), vec2(w, r.height()));
        if k == current {
            p.rect_filled(s.shrink(2.0), radius(5), theme::c(0x5c5c5e));
        }
        p.text(s.center(), Align2::CENTER_CENTER, *item, theme::body(12.0), Tokens::text_strong());
        if ui.interact(s, ui.scope_id().with((id, k)), Sense::click()).clicked() && k != current {
            picked = Some(k);
        }
    }
    picked
}

// ——— Drawing a panel's controls ———

/// What the person did in a panel this frame.
enum Act {
    Set(String, Value),
    Press(String, bool),
    SaveSecret(String, String),
    ForgetSecret(String),
    Open(String),
    Cancel,
}

/// A panel as a column (settings in Preferences, plain cards).
fn column(p: &mut Plugins, ui: &mut Ui, plugin: &str, panel_id: &str) {
    let Some(dir) = p.installed(plugin).map(|i| i.dir.clone()) else { return };
    let now = ui.ctx().input(|i| i.time);
    let (panel, values, running, message, progress) = {
        let busy_progress = p.loaded.get(plugin).map(|l| l.activity.progress());
        let st = p.state(plugin, panel_id);
        (st.panel.clone(), st.values.clone(), st.run.as_ref().map(|r| r.0.clone()), st.message.clone(), busy_progress)
    };
    let Some(panel) = panel else {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        spinner(ui.painter(), r.center(), now);
        return;
    };
    let mut acts = Vec::new();
    for (k, c) in panel.controls.iter().enumerate() {
        ui.push_id(("ctl", k), |ui| control(ui, c, &values, &dir, running.as_deref(), &mut acts, p.drafts_mut(plugin, panel_id)));
    }
    if let Some(a) = &running {
        let (_, text) = progress.unwrap_or_default();
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        spinner(ui.painter(), pos2(r.left() + 10.0, r.center().y), now);
        let label = if text.is_empty() { format!("Working ({a})…") } else { text };
        ui.painter().text(pos2(r.left() + 26.0, r.center().y), Align2::LEFT_CENTER, label, theme::body(11.5), Tokens::text_dim());
        if button(ui, r.right(), r.center().y, "Cancel", Style::Plain, true).0 {
            acts.push(Act::Cancel);
        }
    }
    if let Some((tone, text)) = message.filter(|(_, t)| !t.is_empty()) {
        note(ui, &text, tone);
    }
    apply(p, plugin, panel_id, acts, None, &dir);
}

impl Plugins {
    fn drafts_mut(&mut self, plugin: &str, panel: &str) -> &mut HashMap<String, String> {
        &mut self.state(plugin, panel).drafts
    }
}

fn apply(p: &mut Plugins, plugin: &str, panel_id: &str, acts: Vec<Act>, capture: Option<Capture>, dir: &std::path::Path) {
    for a in acts {
        match a {
            Act::Set(id, v) => p.set_value(plugin, panel_id, &id, v),
            Act::Press(action, wants_capture) => {
                let cap = if wants_capture { capture.clone() } else { None };
                p.press(plugin, panel_id, &action, cap);
            }
            Act::SaveSecret(id, value) => {
                let r = KeyFile::secrets(dir).set(&id, Some(value.trim()));
                // Plugins may answer "secret-saved" (fetch what the new key unlocks); its answer becomes the message.
                if r.is_ok()
                    && let Some(l) = p.ensure(plugin)
                {
                    let t = l.run("secret-saved", &json!({ "id": id }), None);
                    p.state(plugin, panel_id).run = Some(("secret-saved".into(), t));
                }
                let st = p.state(plugin, panel_id);
                st.drafts.remove(&id);
                st.dirty = true;
                st.message = Some(match r {
                    Ok(()) => (Tone::Good, "Saved on this computer.".into()),
                    Err(e) => (Tone::Error, e),
                });
                for ((pp, _), s) in p.panels.iter_mut() {
                    if pp == plugin {
                        s.dirty = true;
                    }
                }
            }
            Act::ForgetSecret(id) => {
                let _ = KeyFile::secrets(dir).set(&id, None);
                for ((pp, _), s) in p.panels.iter_mut() {
                    if pp == plugin {
                        s.dirty = true;
                    }
                }
            }
            Act::Open(url) => p.urls.push(url),
            Act::Cancel => {
                if let Some(l) = p.loaded.get(plugin) {
                    l.cancel();
                }
            }
        }
    }
}

fn label_of<'a>(label: &'a str, id: &'a str) -> &'a str {
    if label.is_empty() { id } else { label }
}

fn small_label(ui: &mut Ui, text: &str) {
    if !text.is_empty() {
        ui.label(egui::RichText::new(text).font(theme::strong(11.0)).color(Tokens::text_dim()));
    }
}

/// One control in a column.
fn control(
    ui: &mut Ui,
    c: &Control,
    values: &Map<String, Value>,
    dir: &std::path::Path,
    running: Option<&str>,
    acts: &mut Vec<Act>,
    drafts: &mut HashMap<String, String>,
) {
    let w = ui.available_width();
    match c {
        Control::Heading { text } => {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(text).font(theme::strong(13.0)).color(Tokens::text_strong()));
        }
        Control::Note { text, tone } => note(ui, text, *tone),
        Control::Separator {} => {
            let (r, _) = ui.allocate_exact_size(vec2(w, 9.0), Sense::hover());
            ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, Tokens::divider()));
        }
        Control::Link { label, url } => {
            let (r, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
            let lr = link(ui, pos2(r.left(), r.center().y), label, "plugin-link");
            if ui.interact(lr, ui.scope_id().with("link"), Sense::click()).clicked() && url.starts_with("https://") {
                acts.push(Act::Open(url.clone()));
            }
        }
        Control::Text { id, label, placeholder, multiline, .. } => {
            small_label(ui, label);
            let mut text = values.get(id).and_then(Value::as_str).unwrap_or("").to_string();
            let before = text.clone();
            text_box(ui, &format!("pl-text-{id}"), &mut text, if *multiline { 76.0 } else { 28.0 }, placeholder, false, !*multiline);
            if text != before {
                acts.push(Act::Set(id.clone(), Value::from(text)));
            }
        }
        Control::Secret { id, label, placeholder } => {
            small_label(ui, label_of(label, id));
            let saved = KeyFile::secrets(dir).get(id).is_some();
            let row = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover()).0;
            if saved {
                ui.painter().text(
                    pos2(row.left(), row.center().y),
                    Align2::LEFT_CENTER,
                    "✓ Saved on this computer",
                    theme::body(12.0),
                    tone_color(Tone::Good),
                );
                if button(ui, row.right(), row.center().y, "Remove", Style::Plain, true).0 {
                    acts.push(Act::ForgetSecret(id.clone()));
                }
            } else {
                let draft = drafts.entry(id.clone()).or_default();
                let field = Rect::from_min_max(row.min, pos2(row.right() - 70.0, row.max.y));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
                child.add(
                    egui::TextEdit::singleline(draft)
                        .id(ui.make_persistent_id(("secret", id)))
                        .password(true)
                        .hint_text(placeholder.as_str())
                        .font(theme::body(12.5))
                        .desired_width(field.width()),
                );
                let ok = !draft.trim().is_empty();
                if button(ui, row.right(), row.center().y, "Save", Style::Primary, ok).0 {
                    acts.push(Act::SaveSecret(id.clone(), draft.clone()));
                }
            }
        }
        Control::Toggle { id, label, .. } => {
            let mut on = values.get(id).and_then(Value::as_bool).unwrap_or(false);
            let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
            ui.painter().text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, label, theme::body(12.5), Tokens::text_strong());
            let before = on;
            switch(ui, r.right(), r.center().y, ("pl-toggle", hash(id)), &mut on);
            if on != before {
                acts.push(Act::Set(id.clone(), Value::from(on)));
            }
        }
        Control::Segmented { id, label, options, .. } => {
            small_label(ui, label);
            let cur = values.get(id).and_then(Value::as_str).unwrap_or("");
            let k = options.iter().position(|o| o == cur).unwrap_or(0);
            let (r, _) = ui.allocate_exact_size(vec2(w.min(90.0 * options.len() as f32), 24.0), Sense::hover());
            let refs: Vec<&str> = options.iter().map(String::as_str).collect();
            if let Some(n) = segmented_at(ui, r, &format!("pl-seg-{id}"), &refs, k) {
                acts.push(Act::Set(id.clone(), Value::from(options[n].clone())));
            }
        }
        Control::Menu { id, label, options, empty, .. } => {
            small_label(ui, label);
            let cur = values.get(id).and_then(Value::as_str).unwrap_or("").to_string();
            if let Some(v) = menu(ui, id, options, &cur, empty, w) {
                acts.push(Act::Set(id.clone(), Value::from(v)));
            }
        }
        Control::Button { label, action, primary, capture, enabled, .. } => {
            let row = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover()).0;
            let g = ui.painter().layout_no_wrap(label.clone(), theme::body(13.0), Color32::WHITE);
            let right = row.left() + g.size().x + 26.0;
            let ok = enabled.unwrap_or(true) && running.is_none();
            if button(ui, right, row.center().y, label, if *primary { Style::Primary } else { Style::Plain }, ok).0 {
                acts.push(Act::Press(action.clone(), *capture));
            }
        }
        Control::Results {} | Control::Stage {} => {}
    }
}

/// A pop-up menu of grouped choices; the newly picked value.
fn menu(ui: &mut Ui, id: &str, options: &[Choice], cur: &str, empty: &str, w: f32) -> Option<String> {
    // A value the list doesn't have (yet: an older pick, a list not refreshed) still shows by its name.
    let shown = options
        .iter()
        .find(|c| c.value == cur)
        .map(|c| c.shown().to_string())
        .or_else(|| (!cur.is_empty()).then(|| cur.split_once(':').map_or(cur, |(_, m)| m).to_string()));
    let text = shown.unwrap_or_else(|| if options.is_empty() { empty.to_string() } else { "Choose…".to_string() });
    let mut picked = None;
    egui::ComboBox::from_id_salt(("pl-menu", id))
        .width(w.min(380.0))
        .selected_text(egui::RichText::new(text).font(theme::body(12.5)).color(Tokens::text_strong()))
        .show_ui(ui, |ui| {
            let mut last_group = None;
            for c in options {
                if last_group != Some(&c.group) && !c.group.is_empty() {
                    ui.label(egui::RichText::new(&c.group).font(theme::strong(11.0)).color(Tokens::text_dim()));
                }
                last_group = Some(&c.group);
                let label = if c.detail.is_empty() { c.shown().to_string() } else { format!("{}    {}", c.shown(), c.detail) };
                if ui.selectable_label(c.value == cur, label).clicked() {
                    picked = Some(c.value.clone());
                }
            }
        });
    picked
}

// ——— The sheet (Generative Render) ———

fn dim_and_card(ui: &mut Ui, id: egui::Id, w: f32, h: f32, f: impl FnOnce(&mut Ui, Rect)) {
    let ctx = ui.ctx().clone();
    let full = ctx.content_rect();
    let card = Rect::from_center_size(full.center(), vec2(w.min(full.width() - 24.0), h.min(full.height() - 24.0)));
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(full.min).show(&ctx, |ui| {
        let _ = ui.allocate_rect(full, Sense::click());
        let p = ui.painter().clone();
        p.rect_filled(full, 0.0, Color32::from_black_alpha(110));
        p.rect_filled(card.translate(vec2(0.0, 8.0)), radius(14), Color32::from_black_alpha(90));
        p.rect_filled(card, radius(12), Tokens::panel());
        p.rect_stroke(card, radius(12), Stroke::new(1.0, Tokens::panel_border()), StrokeKind::Inside);
        let _ = ui.interact(card, id.with("card"), Sense::click());
        f(ui, card);
    });
    ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, id));
}

fn sheet(p: &mut Plugins, ui: &mut Ui) {
    let Some((plugin, panel_id)) = p.sheet.as_ref().map(|s| (s.plugin.clone(), s.panel.clone())) else { return };
    let Some(dir) = p.installed(&plugin).map(|i| i.dir.clone()) else {
        p.sheet = None;
        return;
    };
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let (panel, values, running, message) = {
        let st = p.state(&plugin, &panel_id);
        (st.panel.clone(), st.values.clone(), st.run.as_ref().map(|r| r.0.clone()), st.message.clone())
    };
    let progress = p.loaded.get(&plugin).map(|l| l.activity.progress()).unwrap_or_default();
    let shown_tex = p.sheet.as_ref().and_then(|s| s.shown.clone()).and_then(|e| p.texture(&ctx, &e.path));
    if let Some(s) = p.sheet.as_mut()
        && s.snapshot.is_none()
        && let Some(c) = &s.capture
    {
        let img = egui::ColorImage::from_rgba_unmultiplied([c.view.width as usize, c.view.height as usize], &c.view.pixels);
        s.snapshot = Some(ctx.load_texture("plugin-sheet-snapshot", img, egui::TextureOptions::LINEAR));
    }
    let snapshot = p.sheet.as_ref().and_then(|s| s.snapshot.clone());
    let capture = p.sheet.as_ref().and_then(|s| s.capture.clone());
    let shown_entry = p.sheet.as_ref().and_then(|s| s.shown.clone());
    let results: Vec<results::Entry> = p.results_of(&plugin).iter().take(12).cloned().collect();
    let thumbs: Vec<Option<TextureHandle>> = results.iter().map(|e| p.texture(&ctx, &e.path)).collect();
    let drafts = std::mem::take(p.drafts_mut(&plugin, &panel_id));
    let mut acts = Vec::new();
    let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) && running.is_none();
    let mut save: Option<PathBuf> = None;
    let mut show: Option<results::Entry> = None;
    let title = panel.as_ref().map(|p| p.title.clone()).unwrap_or_default();
    let touch = crate::tablet::touch_ui();
    let w = if touch { 640.0 } else { 600.0 };
    let msg_h = message
        .as_ref()
        .filter(|(t, s)| !s.is_empty() && *t != Tone::Good)
        .map_or(0.0, |(_, s)| 30.0 + 16.0 * (s.len() as f32 / 80.0).ceil());
    let h = (w - 36.0) * 0.6 + if results.is_empty() { 190.0 } else { 250.0 } + msg_h;
    if running.is_some() {
        ctx.request_repaint();
    }
    dim_and_card(ui, egui::Id::NULL.with("plugin-sheet"), w, h, |ui, card| {
        let painter = ui.painter().clone();
        let x0 = card.left() + 18.0;
        let x1 = card.right() - 18.0;
        painter.text(pos2(x0, card.top() + 24.0), Align2::LEFT_CENTER, &title, theme::strong(15.0), Tokens::text_strong());
        // The picture: the view, or the newest result, keeping its shape.
        let stage = Rect::from_min_max(pos2(x0, card.top() + 44.0), pos2(x1, card.top() + 44.0 + (x1 - x0) * 0.6));
        painter.rect_filled(stage, radius(8), theme::c(0x141414));
        let tex = shown_tex.clone().or(snapshot.clone());
        if let Some(t) = &tex {
            let size = t.size_vec2();
            let k = (stage.width() / size.x).min(stage.height() / size.y);
            let r = Rect::from_center_size(stage.center(), size * k);
            painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if running.is_some() {
            // While it works: the picture dims, a soft glow breathes around it, and a line says what's going on.
            painter.rect_filled(stage, radius(8), Color32::from_black_alpha(150));
            let pulse = (0.5 + 0.5 * (now * 2.4).sin()) as f32;
            let glow = Tokens::accent().gamma_multiply(0.35 + 0.45 * pulse);
            painter.rect_stroke(stage, radius(8), Stroke::new(3.0, glow), StrokeKind::Inside);
            spinner(&painter, stage.center() - vec2(0.0, 26.0), now);
            let text = if progress.1.is_empty() { "Generating scene…".to_string() } else { progress.1.clone() };
            painter.text(stage.center() + vec2(0.0, 2.0), Align2::CENTER_CENTER, text, theme::strong(13.5), Color32::WHITE);
        }
        // The prompt bar over the picture's bottom edge: the words, Generate, Save.
        let panel_ref = panel.as_ref();
        let prompt = panel_ref.and_then(|p| p.controls.iter().find(|c| matches!(c, Control::Text { .. })));
        let go = panel_ref.and_then(|p| p.controls.iter().find(|c| matches!(c, Control::Button { primary: true, .. })));
        let bar = Rect::from_min_max(pos2(stage.left() + 12.0, stage.bottom() - 62.0), pos2(stage.right() - 12.0, stage.bottom() - 10.0));
        painter.rect_filled(bar.translate(vec2(0.0, 3.0)), radius(12), Color32::from_black_alpha(70));
        painter.rect_filled(bar, radius(12), Tokens::panel());
        painter.rect_stroke(bar, radius(12), Stroke::new(1.0, Tokens::panel_border()), StrokeKind::Inside);
        let go_w = 96.0;
        let field = Rect::from_min_max(pos2(bar.left() + 6.0, bar.top() + 6.0), pos2(bar.right() - go_w - 46.0, bar.bottom() - 6.0));
        if let Some(Control::Text { id, placeholder, .. }) = prompt {
            let mut text = values.get(id).and_then(Value::as_str).unwrap_or("").to_string();
            let before = text.clone();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field));
            text_box(&mut child, &format!("sheet-{id}"), &mut text, field.height(), placeholder, false, false);
            if text != before {
                acts.push(Act::Set(id.clone(), Value::from(text)));
            }
        }
        if let Some(Control::Button { label, action, capture: wants, enabled, .. }) = go {
            let ok = enabled.unwrap_or(true) && running.is_none();
            let enter = ok && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
            if button(ui, bar.right() - 44.0, bar.center().y, label, Style::Primary, ok).0 || enter {
                acts.push(Act::Press(action.clone(), *wants));
            }
        }
        // Save: the shown result (dim until there is one).
        let save_r = Rect::from_center_size(pos2(bar.right() - 22.0, bar.center().y), vec2(30.0, 30.0));
        let can_save = shown_entry.is_some() && running.is_none();
        let resp = ui.interact(save_r, ui.scope_id().with("sheet-save"), if can_save { Sense::click() } else { Sense::hover() });
        if can_save && resp.hovered() {
            painter.rect_filled(save_r, radius(6), Tokens::button_hover());
        }
        save_glyph(&painter, save_r.center(), if can_save { Tokens::text_strong() } else { Tokens::text_ghost() });
        if can_save && resp.clicked() {
            save = shown_entry.as_ref().map(|e| e.path.clone());
        }
        // Under the picture: the other controls in a row (mode, keep exact, how many, model).
        let mut row_ui = ui.new_child(
            egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(x0, stage.bottom() + 12.0), pos2(x1, card.bottom() - 50.0))),
        );
        row_ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
        if let Some(panel) = panel_ref {
            row_ui.horizontal_wrapped(|ui| {
                for (k, c) in panel.controls.iter().enumerate() {
                    let skip = matches!(c, Control::Stage {} | Control::Results {} | Control::Note { .. } | Control::Menu { .. })
                        || std::ptr::eq(c, prompt.map_or(std::ptr::null(), |x| x as *const _))
                        || std::ptr::eq(c, go.map_or(std::ptr::null(), |x| x as *const _));
                    if skip {
                        continue;
                    }
                    ui.push_id(("sheet-ctl", k), |ui| inline_control(ui, c, &values, &mut acts));
                }
            });
            // Menus (the model) get a line of their own: their names are long.
            for (k, c) in panel.controls.iter().enumerate() {
                if let Control::Menu { label, .. } = c {
                    row_ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(label).font(theme::body(12.0)).color(Tokens::text_dim()));
                        ui.push_id(("sheet-menu", k), |ui| inline_control(ui, c, &values, &mut acts));
                    });
                }
            }
            for c in &panel.controls {
                if let Control::Note { text, tone } = c {
                    note(&mut row_ui, text, *tone);
                }
            }
        } else {
            let (r, _) = row_ui.allocate_exact_size(vec2(40.0, 30.0), Sense::hover());
            spinner(row_ui.painter(), r.center(), now);
        }
        if let Some((tone, text)) = message.clone().filter(|(t, s)| !s.is_empty() && *t != Tone::Good) {
            note(&mut row_ui, &text, tone);
        }
        // Earlier pictures: click one to look at it again.
        if !results.is_empty() {
            row_ui.add_space(4.0);
            row_ui.horizontal(|ui| {
                for (e, t) in results.iter().zip(&thumbs) {
                    let Some(t) = t else { continue };
                    let size = t.size_vec2();
                    let th = 46.0;
                    let r = ui.allocate_exact_size(vec2(th * size.x / size.y.max(1.0), th), Sense::click());
                    ui.painter().image(t.id(), r.0, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    let on = shown_entry.as_ref().is_some_and(|s| s.path == e.path);
                    let edge = if on { Tokens::accent() } else { Tokens::chip_border() };
                    ui.painter().rect_stroke(r.0, radius(4), Stroke::new(if on { 2.0 } else { 1.0 }, edge), StrokeKind::Inside);
                    let resp = r.1.on_hover_text(format!("{}\n{}", e.title, e.details));
                    if resp.clicked() {
                        show = Some(e.clone());
                    }
                }
            });
        }
        let close_label = if running.is_some() { "Cancel" } else { "Close" };
        if button(ui, x1, card.bottom() - 26.0, close_label, Style::Plain, true).0 {
            if running.is_some() {
                acts.push(Act::Cancel);
            } else {
                close = true;
            }
        }
        painter.text(
            pos2(x0, card.bottom() - 26.0),
            Align2::LEFT_CENTER,
            "Made by the service you picked, on your own account or computer.",
            theme::body(10.5),
            Tokens::text_faint(),
        );
    });
    *p.drafts_mut(&plugin, &panel_id) = drafts;
    apply(p, &plugin, &panel_id, acts, capture, &dir);
    if let Some(e) = show
        && let Some(s) = p.sheet.as_mut()
    {
        s.shown = Some(e);
    }
    if let Some(src) = save {
        let mut pick = crate::dialogs::Pick::default();
        pick.start(|| crate::dialogs::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("Generative Render.png").save_file());
        p.save_pick = Some((src, pick));
    }
    if close {
        p.sheet = None;
    }
}

/// A small control in the sheet's row.
fn inline_control(ui: &mut Ui, c: &Control, values: &Map<String, Value>, acts: &mut Vec<Act>) {
    match c {
        Control::Segmented { id, options, .. } => {
            let cur = values.get(id).and_then(Value::as_str).unwrap_or("");
            let k = options.iter().position(|o| o == cur).unwrap_or(0);
            let w: f32 = options.iter().map(|o| o.len() as f32 * 7.0 + 22.0).sum();
            let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
            let refs: Vec<&str> = options.iter().map(String::as_str).collect();
            if let Some(n) = segmented_at(ui, r, &format!("sh-seg-{id}"), &refs, k) {
                acts.push(Act::Set(id.clone(), Value::from(options[n].clone())));
            }
        }
        Control::Toggle { id, label, .. } => {
            let mut on = values.get(id).and_then(Value::as_bool).unwrap_or(false);
            let g = ui.painter().layout_no_wrap(label.clone(), theme::body(12.0), Tokens::text_strong());
            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 52.0, 26.0), Sense::hover());
            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, Tokens::text_strong());
            let before = on;
            switch(ui, r.right(), r.center().y, ("sh-toggle", hash(id)), &mut on);
            if on != before {
                acts.push(Act::Set(id.clone(), Value::from(on)));
            }
        }
        Control::Menu { id, options, empty, .. } => {
            let cur = values.get(id).and_then(Value::as_str).unwrap_or("").to_string();
            let w = (ui.available_width() - 30.0).clamp(150.0, 340.0);
            if let Some(v) = menu(ui, id, options, &cur, empty, w) {
                acts.push(Act::Set(id.clone(), Value::from(v)));
            }
        }
        Control::Button { label, action, capture, .. } if ui.button(label).clicked() => {
            acts.push(Act::Press(action.clone(), *capture));
        }
        _ => {}
    }
}

/// A tray with a down arrow (save).
fn save_glyph(p: &egui::Painter, c: egui::Pos2, col: Color32) {
    let s = Stroke::new(1.6, col);
    p.line_segment([c + vec2(0.0, -7.0), c + vec2(0.0, 3.0)], s);
    p.line_segment([c + vec2(-4.0, -1.0), c + vec2(0.0, 3.0)], s);
    p.line_segment([c + vec2(4.0, -1.0), c + vec2(0.0, 3.0)], s);
    p.line_segment([c + vec2(-7.0, 2.0), c + vec2(-7.0, 7.0)], s);
    p.line_segment([c + vec2(-7.0, 7.0), c + vec2(7.0, 7.0)], s);
    p.line_segment([c + vec2(7.0, 7.0), c + vec2(7.0, 2.0)], s);
}

/// A round badge with a tick (permissions, "ready").
fn tick_badge(p: &egui::Painter, c: egui::Pos2, r: f32, fill: Color32) {
    p.circle_filled(c, r, fill);
    let s = Stroke::new((r * 0.22).max(1.4), Color32::WHITE);
    p.line_segment([c + vec2(-r * 0.42, r * 0.02), c + vec2(-r * 0.1, r * 0.34)], s);
    p.line_segment([c + vec2(-r * 0.1, r * 0.34), c + vec2(r * 0.45, -r * 0.3)], s);
}

/// The plugin's tile, bigger and with a soft sheen (the cards).
fn hero_icon(p: &egui::Painter, r: Rect, which: &str) {
    let base = if which == "sparkle" { Tokens::accent() } else { theme::c(0x4a4a4e) };
    p.rect_filled(r.translate(vec2(0.0, 3.0)), radius(14), Color32::from_black_alpha(60));
    p.rect_filled(r, radius(14), base);
    let top = Rect::from_min_max(r.min, pos2(r.max.x, r.center().y));
    p.rect_filled(top, egui::CornerRadius { nw: 14, ne: 14, sw: 0, se: 0 }, Color32::from_white_alpha(18));
    let icon = match which {
        "sparkle" => crate::icons::Icon::CameraSparkle,
        "image" => crate::icons::Icon::Screenshot,
        _ => crate::icons::Icon::Insert,
    };
    crate::icons::paint(p, r.shrink(r.width() * 0.22), icon, Color32::WHITE);
}

/// Centred text wrapped to `w`; returns its height.
fn centred(p: &egui::Painter, top: egui::Pos2, w: f32, text: &str, font: egui::FontId, color: Color32) -> f32 {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, w);
    job.halign = egui::Align::Center;
    let g = p.layout_job(job);
    let h = g.size().y;
    // A centred layout is laid out around x = 0: paint it from the middle.
    p.galley(pos2(top.x + w / 2.0, top.y), g, color);
    h
}

/// Rows in a rounded group (as the Export dialog's switches): a badge, a bold line and a dim one under it, each
/// wrapped to the width. Draws at `top`, `w` wide; returns the height.
fn feature_rows(p: &egui::Painter, top: egui::Pos2, w: f32, rows: &[(&str, &str)], badge: Color32) -> f32 {
    let text_w = w - 46.0 - 14.0;
    let laid: Vec<(std::sync::Arc<egui::Galley>, Option<std::sync::Arc<egui::Galley>>)> = rows
        .iter()
        .map(|(title, detail)| {
            let font = if detail.is_empty() { theme::body(13.0) } else { theme::strong(13.0) };
            let t = p.layout(title.to_string(), font, Tokens::text_strong(), text_w);
            let d = (!detail.is_empty()).then(|| p.layout(detail.to_string(), theme::body(11.5), Tokens::text_dim(), text_w));
            (t, d)
        })
        .collect();
    let heights: Vec<f32> =
        laid.iter().map(|(t, d)| (t.size().y + d.as_ref().map_or(0.0, |d| d.size().y + 2.0) + 22.0).max(42.0)).collect();
    let total: f32 = heights.iter().sum();
    let rect = Rect::from_min_size(top, vec2(w, total));
    p.rect_filled(rect, radius(10), Tokens::chip());
    p.rect_stroke(rect, radius(10), Stroke::new(1.0, Tokens::chip_border()), StrokeKind::Inside);
    let mut y = top.y;
    for (k, ((t, d), h)) in laid.into_iter().zip(&heights).enumerate() {
        let r = Rect::from_min_size(pos2(rect.left(), y), vec2(w, *h));
        if k > 0 {
            p.hline((r.left() + 46.0)..=(r.right() - 14.0), r.top(), Stroke::new(1.0, Tokens::chip_border()));
        }
        tick_badge(p, pos2(r.left() + 24.0, r.center().y), 9.0, badge);
        let block = t.size().y + d.as_ref().map_or(0.0, |d| d.size().y + 2.0);
        let ty = r.center().y - block / 2.0;
        let th = t.size().y;
        p.galley(pos2(r.left() + 46.0, ty), t, Tokens::text_strong());
        if let Some(d) = d {
            p.galley(pos2(r.left() + 46.0, ty + th + 2.0), d, Tokens::text_dim());
        }
        y += h;
    }
    total
}

/// The card the Generative Render button shows before the plugin is added.
fn offer_card(p: &mut Plugins, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let entry =
        p.index.as_ref().and_then(|r| r.as_ref().ok()).and_then(|i| i.plugins.iter().find(|e| e.manifest.id == GENERATIVE).cloned());
    let failed = p.index.as_ref().and_then(|r| r.as_ref().err()).cloned();
    let mut add = false;
    let mut retry = false;
    dim_and_card(ui, egui::Id::NULL.with("plugin-offer"), 480.0, 446.0, |ui, card| {
        let painter = ui.painter().clone();
        let pad = 28.0;
        let mut y = card.top() + 32.0;
        hero_icon(&painter, Rect::from_center_size(pos2(card.center().x, y + 28.0), vec2(56.0, 56.0)), "sparkle");
        y += 56.0 + 20.0;
        painter.text(
            pos2(card.center().x, y + 11.0),
            Align2::CENTER_CENTER,
            "Generative Render",
            theme::strong(19.0),
            Tokens::text_strong(),
        );
        y += 32.0;
        y += centred(
            &painter,
            pos2(card.left() + pad, y),
            card.width() - 2.0 * pad,
            "Describe a scene and get a picture of your model in it.",
            theme::body(13.5),
            Tokens::text_dim(),
        );
        y += 20.0;
        let rows = [
            ("Your own AI", "OpenRouter, OpenAI or Google Gemini, or free on your computer"),
            ("Your model stays exact", "Only the scene around it is made up"),
            ("No limits of ours", "Make as many as your provider allows"),
        ];
        y += feature_rows(&painter, pos2(card.left() + pad, y), card.width() - 2.0 * pad, &rows, Tokens::accent()) + 16.0;
        centred(
            &painter,
            pos2(card.left() + pad, y),
            card.width() - 2.0 * pad,
            "It's a plugin, so ShapeCraft stays lean for everyone who doesn't use it.",
            theme::body(11.5),
            Tokens::text_faint(),
        );
        let by = card.bottom() - 34.0;
        painter.hline(card.x_range(), by - 26.0, Stroke::new(1.0, Tokens::divider()));
        match (&entry, &failed) {
            (Some(_), _) => {
                if button(ui, card.right() - pad + 8.0, by, "Add Plugin…", Style::Primary, p.adding.is_none()).0 {
                    add = true;
                }
            }
            (None, Some(e)) => {
                painter.text(pos2(card.left() + pad, by), Align2::LEFT_CENTER, short(e, 46), theme::body(11.5), Tokens::danger());
                if button(ui, card.right() - pad + 8.0, by, "Try Again", Style::Primary, true).0 {
                    retry = true;
                }
            }
            (None, None) => {
                spinner(&painter, pos2(card.left() + pad + 8.0, by), now);
                painter.text(
                    pos2(card.left() + pad + 24.0, by),
                    Align2::LEFT_CENTER,
                    "Looking it up…",
                    theme::body(12.0),
                    Tokens::text_dim(),
                );
            }
        }
        if button(ui, card.right() - pad - 112.0, by, "Not Now", Style::Plain, true).0 {
            close = true;
        }
    });
    if retry {
        p.fetch_index();
    }
    if add && let Some(e) = entry {
        p.start_add(&e);
        p.offer = false;
    }
    if close {
        p.offer = false;
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// "Add <plugin>?" with what it asks for.
fn confirm_card(p: &mut Plugins, ui: &mut Ui) {
    let Some(c) = &p.confirm else { return };
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let m = c.manifest.clone();
    let from_file = matches!(c.source, Source::File(_));
    let lines = m.permission_lines();
    let mut answer = None;
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        answer = Some(false);
    }
    // Hosts read better as a list under the line than as one long sentence.
    let split: Vec<(String, String)> = lines
        .iter()
        .map(|l| match l.strip_prefix("Talk to ").filter(|rest| rest.contains('.') && !rest.contains(" on ")) {
            Some(hosts) => ("Talk to these services".to_string(), hosts.to_string()),
            None => (l.clone(), String::new()),
        })
        .collect();
    let rows_h: f32 = split.iter().map(|(_, d)| if d.is_empty() { 42.0 } else { 58.0 }).sum();
    let h = 250.0 + rows_h + if from_file { 52.0 } else { 0.0 };
    dim_and_card(ui, egui::Id::NULL.with("plugin-confirm"), 480.0, h, |ui, card| {
        let painter = ui.painter().clone();
        let pad = 28.0;
        let x0 = card.left() + pad;
        hero_icon(&painter, Rect::from_min_size(pos2(x0, card.top() + 28.0), vec2(48.0, 48.0)), &m.icon);
        painter.text(
            pos2(x0 + 64.0, card.top() + 42.0),
            Align2::LEFT_CENTER,
            format!("Add {}?", m.name),
            theme::strong(17.0),
            Tokens::text_strong(),
        );
        let by = if m.authors.is_empty() { String::new() } else { format!("By {} · ", m.authors.join(", ")) };
        painter.text(
            pos2(x0 + 64.0, card.top() + 63.0),
            Align2::LEFT_CENTER,
            format!("{by}version {}", m.version),
            theme::body(12.0),
            Tokens::text_dim(),
        );
        let mut y = card.top() + 100.0;
        painter.text(pos2(x0, y), Align2::LEFT_CENTER, "It will be able to", theme::strong(12.0), Tokens::text_dim());
        y += 14.0;
        let rows: Vec<(&str, &str)> = split.iter().map(|(t, d)| (t.as_str(), d.as_str())).collect();
        y += feature_rows(&painter, pos2(x0, y), card.width() - 2.0 * pad, &rows, theme::fixed(0x3d9a50)) + 14.0;
        let g = painter.layout(
            "Nothing else: not your files, not other plugins. You can switch it off or remove it any time in Preferences ▸ Plugins.".into(),
            theme::body(11.5),
            Tokens::text_dim(),
            card.width() - 2.0 * pad,
        );
        let gh = g.size().y;
        painter.galley(pos2(x0, y), g, Tokens::text_dim());
        y += gh + 12.0;
        if from_file {
            let g = painter.layout(
                "This one comes from a file, not ShapeCraft's reviewed list. Only add it if you trust where it came from.".into(),
                theme::body(11.5),
                tone_color(Tone::Warning),
                card.width() - 2.0 * pad,
            );
            painter.galley(pos2(x0, y), g, tone_color(Tone::Warning));
        }
        let by = card.bottom() - 34.0;
        painter.hline(card.x_range(), by - 26.0, Stroke::new(1.0, Tokens::divider()));
        if button(ui, card.right() - pad + 8.0, by, "Add Plugin", Style::Primary, true).0 {
            answer = Some(true);
        }
        if button(ui, card.right() - pad - 108.0, by, "Cancel", Style::Plain, true).0 {
            answer = Some(false);
        }
    });
    match answer {
        Some(true) => p.accept(now),
        Some(false) => p.confirm = None,
        None => {}
    }
}

/// Adding a plugin, start to finish: working (with what it's doing), then ready (with what to do next) or what went
/// wrong (with Try Again).
fn add_card(p: &mut Plugins, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    // Started without a clock (the control channel): it starts now.
    if let Some(AddCard::Working { since, .. }) = &mut p.add_card
        && *since < 0.0
    {
        *since = now;
    }
    let Some(card_state) = &p.add_card else { return };
    let mut out: Option<&'static str> = None;
    let (name, icon) = match card_state {
        AddCard::Working { name, icon, .. } | AddCard::Done { name, icon, .. } | AddCard::Failed { name, icon, .. } => {
            (name.clone(), icon.clone())
        }
    };
    if matches!(card_state, AddCard::Working { .. }) {
        ctx.request_repaint();
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        out = Some("close");
    }
    let state = match card_state {
        AddCard::Working { size, since, .. } => (0u8, String::new(), *size, now - *since),
        AddCard::Done { .. } => (1u8, String::new(), 0, 0.0),
        AddCard::Failed { error, .. } => (2u8, error.clone(), 0, 0.0),
    };
    let card_h = if state.0 == 0 { 250.0 } else { 270.0 };
    dim_and_card(ui, egui::Id::NULL.with("plugin-adding"), 440.0, card_h, |ui, card| {
        let painter = ui.painter().clone();
        let pad = 28.0;
        let c = card.center().x;
        let icon_r = Rect::from_center_size(pos2(c, card.top() + 62.0), vec2(56.0, 56.0));
        hero_icon(&painter, icon_r, &icon);
        let by = card.bottom() - 34.0;
        match state.0 {
            0 => {
                // Working: a bar that fills as it goes (downloads are small; the checking takes a moment).
                let t = ((state.3 / 1.6) as f32).clamp(0.0, 0.92);
                let step =
                    if t < 0.45 { format!("Downloading {}…", size_text(state.2)) } else { "Checking and installing…".to_string() };
                painter.text(
                    pos2(c, card.top() + 118.0),
                    Align2::CENTER_CENTER,
                    format!("Adding {name}"),
                    theme::strong(17.0),
                    Tokens::text_strong(),
                );
                painter.text(pos2(c, card.top() + 144.0), Align2::CENTER_CENTER, step, theme::body(12.5), Tokens::text_dim());
                let bar = Rect::from_center_size(pos2(c, card.top() + 178.0), vec2(card.width() - 2.0 * pad - 40.0, 6.0));
                painter.rect_filled(bar, radius(3), theme::c(0x3a3a3c));
                let fill = Rect::from_min_size(bar.min, vec2(bar.width() * (0.08 + 0.92 * t), bar.height()));
                painter.rect_filled(fill, radius(3), Tokens::accent());
                painter.text(
                    pos2(c, by),
                    Align2::CENTER_CENTER,
                    "It runs sealed off, with only the permissions you just saw.",
                    theme::body(11.5),
                    Tokens::text_faint(),
                );
            }
            1 => {
                tick_badge(&painter, icon_r.right_bottom() - vec2(4.0, 4.0), 13.0, theme::fixed(0x3d9a50));
                painter.text(
                    pos2(c, card.top() + 118.0),
                    Align2::CENTER_CENTER,
                    format!("{name} is ready"),
                    theme::strong(17.0),
                    Tokens::text_strong(),
                );
                centred(
                    &painter,
                    pos2(card.left() + pad, card.top() + 136.0),
                    card.width() - 2.0 * pad,
                    "Connect the AI you'd like to use, then press Generative Render in Visualization. You'll find it any time in Preferences ▸ Plugins.",
                    theme::body(12.5),
                    Tokens::text_dim(),
                );
                painter.hline(card.x_range(), by - 26.0, Stroke::new(1.0, Tokens::divider()));
                if button(ui, card.right() - pad + 8.0, by, "Set Up Connections…", Style::Primary, true).0 {
                    out = Some("setup");
                }
                if button(ui, card.right() - pad - 172.0, by, "Later", Style::Plain, true).0 {
                    out = Some("close");
                }
            }
            _ => {
                painter.text(
                    pos2(c, card.top() + 118.0),
                    Align2::CENTER_CENTER,
                    format!("{name} couldn't be added"),
                    theme::strong(17.0),
                    Tokens::text_strong(),
                );
                centred(
                    &painter,
                    pos2(card.left() + pad, card.top() + 136.0),
                    card.width() - 2.0 * pad,
                    &short(&state.1, 180),
                    theme::body(12.0),
                    Tokens::danger(),
                );
                painter.hline(card.x_range(), by - 26.0, Stroke::new(1.0, Tokens::divider()));
                if button(ui, card.right() - pad + 8.0, by, "Try Again", Style::Primary, true).0 {
                    out = Some("retry");
                }
                if button(ui, card.right() - pad - 104.0, by, "Close", Style::Plain, true).0 {
                    out = Some("close");
                }
            }
        }
    });
    match out {
        Some("setup") => {
            if let Some(AddCard::Done { id, .. }) = &p.add_card {
                p.selected = Some(id.clone());
            }
            p.browsing = false;
            p.requests.push(Request::OpenPlugins);
            p.add_card = None;
        }
        Some("retry") => {
            let retry = match p.add_card.take() {
                Some(AddCard::Failed { retry, .. }) => retry,
                _ => None,
            };
            if let Some(e) = retry {
                p.start_add(&e);
            }
        }
        Some(_) => p.add_card = None,
        None => {}
    }
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) } else { format!("{} KB", (bytes / 1024).max(1)) }
}

// ——— Control channel ———

/// `plugins.*` on the control channel (and MCP through it): list, add from a file or the list, open a sheet, read a
/// panel, set values, press a button, wait for it.
pub(crate) fn control_call(method: &str, params: &Value, capture: impl FnOnce() -> Option<Capture>) -> Result<Value, String> {
    let s = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match method {
        "plugins.list" => with(|p| {
            p.reload_list();
            Ok(json!(
                p.installed
                    .iter()
                    .map(|i| json!({
                        "id": i.manifest.id, "name": i.manifest.name, "version": i.manifest.version,
                        "enabled": i.enabled, "sideloaded": i.sideloaded,
                        "permissions": i.manifest.permission_lines(),
                    }))
                    .collect::<Vec<_>>()
            ))
        }),
        "plugins.install" => with(|p| {
            let pkg = shapecraft_plugin_host::library::read_package(std::path::Path::new(&s("path"))).map_err(|e| e.to_string())?;
            let lib = p.lib.as_ref().ok_or("no plugins folder")?;
            let i = lib.install(&pkg, params.get("sideloaded").and_then(Value::as_bool).unwrap_or(true)).map_err(|e| e.to_string())?;
            let id = i.manifest.id.clone();
            p.added(i);
            Ok(json!({ "id": id }))
        }),
        "plugins.remove" => with(|p| {
            let id = s("id");
            if let Some(l) = p.loaded.remove(&id) {
                l.cancel();
            }
            p.lib.as_ref().ok_or("no plugins folder")?.remove(&id).map_err(|e| e.to_string())?;
            p.panels.retain(|(pid, _), _| *pid != id);
            p.reload_list();
            Ok(Value::Null)
        }),
        "plugins.panel" => with(|p| {
            let (id, panel) = (s("id"), if s("panel").is_empty() { "main".into() } else { s("panel") });
            if let Some(Value::Object(vals)) = params.get("values") {
                for (k, v) in vals {
                    p.set_value(&id, &panel, k, v.clone());
                }
            }
            let values = Value::Object(p.state(&id, &panel).values.clone());
            let loaded = p.ensure(&id).ok_or("the plugin isn't added or is off")?;
            let text = loaded.panel(&panel, &values).wait().map_err(|e| e.to_string())?;
            Ok(json!({ "panel": serde_json::from_str::<Value>(&text).unwrap_or(Value::Null), "values": values }))
        }),
        "plugins.secret" => with(|p| {
            let dir = p.installed(&s("id")).map(|i| i.dir.clone()).ok_or("the plugin isn't added")?;
            let v = params.get("value").and_then(Value::as_str);
            KeyFile::secrets(&dir).set(&s("name"), v)?;
            Ok(Value::Null)
        }),
        "plugins.run" => {
            let cap = if params.get("capture").and_then(Value::as_bool).unwrap_or(false) { capture() } else { None };
            with(|p| {
                let (id, panel) = (s("id"), if s("panel").is_empty() { "main".into() } else { s("panel") });
                let values = Value::Object(p.state(&id, &panel).values.clone());
                let loaded = p.ensure(&id).ok_or("the plugin isn't added or is off")?;
                let t = loaded.run(&s("action"), &values, cap);
                if params.get("wait").and_then(Value::as_bool).unwrap_or(true) {
                    let out = t.wait().map_err(|e| e.to_string())?;
                    let new = loaded.activity.take_results();
                    let paths: Vec<String> = new.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
                    if !new.is_empty() {
                        let list = p.results_of(&id);
                        for r in new.iter().rev() {
                            list.insert(0, r.clone());
                        }
                    }
                    Ok(json!({ "answer": serde_json::from_str::<Value>(&out).unwrap_or(Value::Null), "results": paths }))
                } else {
                    p.state(&id, &panel).run = Some((s("action"), t));
                    Ok(Value::Null)
                }
            })
        }
        "plugins.select" => with(|p| {
            p.selected = Some(s("id")).filter(|i| !i.is_empty());
            p.browsing = false;
            Ok(Value::Null)
        }),
        // Presses the confirmation card's Add Plugin (to look at the add cards offscreen).
        "plugins.accept" => with(|p| {
            p.accept(-1.0);
            Ok(Value::Null)
        }),
        "plugins.add" => with(|p| {
            let e =
                p.index.as_ref().and_then(|r| r.as_ref().ok()).and_then(|i| i.plugins.iter().find(|e| e.manifest.id == s("id")).cloned());
            let e = e.ok_or("not in the list (plugins.browse first)")?;
            p.start_add(&e);
            Ok(Value::Null)
        }),
        "plugins.close" => with(|p| {
            p.sheet = None;
            p.offer = false;
            p.confirm = None;
            p.add_card = None;
            Ok(Value::Null)
        }),
        "plugins.browse" => with(|p| {
            if params.get("refresh").and_then(Value::as_bool).unwrap_or(false) || p.index.is_none() && p.browse.is_none() {
                p.fetch_index();
            }
            p.browsing = true;
            Ok(Value::Null)
        }),
        "plugins.offer" => with(|p| {
            p.offer = true;
            if p.index.is_none() && p.browse.is_none() {
                p.fetch_index();
            }
            Ok(Value::Null)
        }),
        _ => Err(format!("unknown method {method}")),
    }
}
