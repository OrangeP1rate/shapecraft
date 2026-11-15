//! ShapeCraft for iPhone and iPad: the same app as the desktop, in a UIKit window drawn with Metal. Projects and
//! preferences live in the app's Documents folder. Built, signed and installed by `cargo xtask ios-run`.

#[cfg(target_os = "ios")]
fn main() {
    ios::main();
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!("shapecraft-ios runs on iOS only: cargo xtask ios-run");
}

#[cfg(target_os = "ios")]
mod files;
#[cfg(target_os = "ios")]
mod insets;

#[cfg(target_os = "ios")]
mod ios {
    use std::path::PathBuf;
    use std::sync::Arc;

    use shapecraft_kernel_occt::OcctKernel;
    use shapecraft_ui::settings::Settings;
    use shapecraft_ui::tablet::{NativeTouch, Pointer, PointerPhase, PointerTool};
    use shapecraft_ui::{Options, ShapeCraftApp, crash_log, graphics};
    use winit::platform::ios::pointers::{self, Phase, Record, Tool};

    pub fn main() {
        crash_log::install();
        // The app's sandbox: HOME is its container; Documents shows in the Files app (UIFileSharingEnabled).
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let docs = home.join("Documents");
        let docs_for_app = docs.clone();
        let _ = std::fs::create_dir_all(&docs);
        shapecraft_ui::dialogs::set_android_folder(docs.join("Exports"));
        shapecraft_ui::tablet::set_touch_screen(true);
        shapecraft_ui::set_url_opener(crate::files::open_url);
        // The iPhone Duo: upright on the front screen, on its side inside.
        winit::platform::ios::set_fold_orientations(true);
        Options::set_documents(docs.clone());
        let _ = std::fs::remove_dir_all(docs.join(".outbox"));
        let settings = home.join("Library/Preferences/ShapeCraft-settings.json");
        let opts = Options {
            projects_root: Options::default_root(),
            control_port: None,
            open: None,
            new_project: false,
            native_menu: false,
            settings_file: Some(settings),
            hosted: false,
        };
        let saved = opts.settings_file.as_deref().map(Settings::load).unwrap_or_default();
        let pass = graphics::single_pass(&saved.graphics);
        let native =
            eframe::NativeOptions { wgpu_options: graphics::configuration(&saved.graphics, &saved.gpu, &pass), ..Default::default() };
        let result = eframe::run_native(
            "ShapeCraft",
            native,
            Box::new(move |cc| {
                let docs = docs_for_app;
                cc.egui_ctx.options_mut(|o| o.input_options.max_click_duration = 10.0);
                let inner = ShapeCraftApp::new(Arc::new(OcctKernel::new()), opts, &cc.egui_ctx);
                let log = docs.join("geometry.log");
                let _ = std::fs::remove_file(&log);
                Ok(Box::new(Phone {
                    inner,
                    touch: NativeTouch::default(),
                    log,
                    frame: 0,
                    native_safe: None,
                    scale: 1.0,
                    regions: String::new(),
                    corner: None,
                    side_bar: None,
                    settle: None,
                    last_size: (0.0, 0.0),
                    last_lock: None,
                }))
            }),
        );
        if let Err(e) = result {
            graphics::report_failure(&e.to_string());
        }
    }

    /// The desktop app with what a touch screen needs around it: fingers through the tablet's touch model and the
    /// interface scaled to fit.
    struct Phone {
        inner: ShapeCraftApp,
        touch: NativeTouch,
        /// Documents/geometry.log: the window's and egui's sizes now and then, for bringing up new screens.
        log: PathBuf,
        frame: u64,
        /// The safe-area insets as UIKit gives them (system points); egui is handed them in its own points.
        native_safe: Option<egui::SafeAreaInsets>,
        /// Keep drawing until then after the start or a turn: UIKit reports the reserved corner and the vertical bar
        /// a moment after the layout changes, and an idle app wouldn't look again until touched. (A delayed repaint
        /// doesn't wake the app on iOS, so it draws straight on for this short while.)
        settle: Option<std::time::Instant>,
        /// The view's size last frame (a change is a fold, unfold or turn: settle again).
        last_size: (f32, f32),
        /// The orientation lock last frame.
        last_lock: Option<bool>,
        /// What the last measure found, for the log.
        regions: String,
        /// The reserved status corner (system points), when there is one.
        corner: Option<(f32, f32, f32, f32)>,
        /// The vertical bar's column (system points), when iOS shows it.
        side_bar: Option<(f32, f32, f32, f32)>,
        /// The interface's scale on this screen (MIN_SIZE fits the safe area; up to TOUCH_ZOOM where there's room).
        scale: f32,
    }

    impl eframe::App for Phone {
        fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
            // The interface is scaled here rather than with egui's zoom: a zoom change makes egui rebuild the screen
            // from its safe area, which shifts it whenever there are insets (iOS has them), and the next fit then
            // shrinks it again. egui-winit measures with a zoom of 1, so its numbers are system points throughout.
            // egui-winit's insets come from the window, in the screen's fixed portrait frame; the root view's
            // turn with the interface, and the Duo's camera and status column are added from its reserved regions.
            raw.safe_area_insets = None;
            // The iPhone Duo opened on the projects screen: iOS's vertical bar (clock over Wi-Fi in a column down the
            // side, the page's buttons under it), as the reference keeps it there. A project gets the top row.
            // A phone that folds keeps one orientation per screen: the front screen (folded, 466 × 678) upright for
            // the projects, the big inside screen (opened, 951 × 669) on its side for modeling. Ordinary phones and
            // iPads turn as they like.
            // (The screen's own size, not the window's: locked upright, the app gets a 469 × 669 box on the inside
            // screen, which would keep it locked upright there.)
            // (winit applies it from the hinge as it moves; this notices the change to settle the layout.)
            let lock = winit::platform::ios::orientation_lock();
            if lock != self.last_lock {
                self.last_lock = lock;
                self.settle = None;
            }
            if let Some(screen) = raw.screen_rect {
                let want =
                    screen.width() > screen.height() && screen.width().min(screen.height()) >= 600.0 && self.inner.workspace().is_none();
                if want != winit::platform::ios::vertical_bar() {
                    winit::platform::ios::set_vertical_bar(want);
                    crate::insets::update_vertical_bar();
                    self.settle = None;
                }
            }
            let until = *self.settle.get_or_insert_with(|| std::time::Instant::now() + std::time::Duration::from_millis(1500));
            if std::time::Instant::now() < until {
                ctx.request_repaint();
            }
            if let Some(mut m) = crate::insets::measure() {
                if m.size != self.last_size {
                    self.last_size = m.size;
                    self.settle = None;
                    ctx.request_repaint();
                }
                // iOS sometimes stops reporting the status block's region to the app (seen on the iPhone Duo after it
                // once showed the app boxed on the inside screen: no regions, active or not, even after a relaunch)
                // and gives a plain top inset instead. Then the block goes where this phone reported it before:
                // opened, with the vertical bar, the clock block 84 × 124 at the top of an 84-wide column on the
                // right; opened without it, 144 × 82 in the top right; folded, 200 × 82 in the top right.
                if m.corner.is_none() && m.regions.is_empty() && m.top > 0.0 && winit::platform::ios::foldable() {
                    let (w, h) = m.size;
                    m.corner = Some(if w > h && winit::platform::ios::vertical_bar() {
                        (w - 84.0, 0.0, 84.0, 124.0)
                    } else if w > h {
                        (w - 144.0, 0.0, 144.0, m.top)
                    } else {
                        (w - 200.0, 0.0, 200.0, m.top)
                    });
                    m.top = 0.0;
                }
                // The bar is a column UIKit insets the right side for: the page runs on under it (the buttons there
                // are the page's own), and its status block at the top is the corner they keep out of.
                self.side_bar = None;
                if winit::platform::ios::vertical_bar() && m.right >= 40.0 && m.size.0 > m.size.1 {
                    let (w, h) = m.size;
                    let x = w - m.right;
                    self.side_bar = Some((x, 0.0, m.right, h));
                    let status = m.regions.iter().copied().filter(|r| r.0 >= x - 1.0).max_by(|a, b| (a.2 * a.3).total_cmp(&(b.2 * b.3)));
                    m.corner = Some(status.unwrap_or((x, 0.0, m.right, 120.0)));
                    m.right = 0.0;
                    m.top = 0.0;
                } else if winit::platform::ios::vertical_bar()
                    && m.size.0 > m.size.1
                    && let Some((x, _, cw, ch)) =
                        m.corner.filter(|c| c.0 + c.2 >= m.size.0 - 1.0 && c.1 <= 1.0 && c.2 <= 120.0 && c.3 > c.2)
                {
                    // What the Duo (iOS 27.2) actually does: no inset for the bar, only its clock block as an
                    // occlusion region (84 × 124 in the top right), taller than wide. The column under it is the bar.
                    let _ = ch;
                    self.side_bar = Some((x, 0.0, cw, m.size.1));
                }
                self.native_safe =
                    Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { top: m.top, right: m.right, bottom: m.bottom, left: m.left }));
                self.regions = format!(
                    "view={:?} regions={:?} inactive={} hinge={:?} lock={:?} corner={:?} bar={:?} uikit_safe={:?}",
                    m.size,
                    m.regions,
                    m.inactive,
                    winit::platform::ios::hinge_closed(),
                    winit::platform::ios::orientation_lock(),
                    m.corner,
                    self.side_bar,
                    crate::insets::uikit_safe()
                ) + " " + &crate::insets::status_probe();
                self.corner = m.corner;
            }
            let native_ppp = raw.viewport().native_pixels_per_point.unwrap_or(3.0);
            if let Some(screen) = raw.screen_rect {
                let m = self.native_safe.map(|s| s.0).unwrap_or_default();
                let safe = (screen.width() - m.left - m.right, screen.height() - m.top - m.bottom);
                // A phone sideways that's tall enough for the columns (the opened iPhone Duo, 951 × 669) keeps its own
                // points and gets the compact layout; smaller or upright screens are fitted as before.
                let (short, long) = (screen.width().min(screen.height()), screen.width().max(screen.height()));
                let sideways = screen.width() > screen.height();
                shapecraft_ui::tablet::set_small_screen(short < 700.0);
                // Upright and narrow (the Duo folded shut): the projects screen at the phone's own size, and a project
                // waits until it's opened up.
                // On a phone that folds, folded means on the front screen (its own size, whatever the window is).
                let folded = winit::platform::ios::hinge_closed().unwrap_or(!sideways && short < 600.0);
                shapecraft_ui::tablet::set_folded(folded);
                // An ordinary iPhone (tall and thin, no hinge; the Duo's screens, even its boxed one, are near 1.4 : 1)
                // keeps the same two places: upright for the projects, on its side for a project, which opens at once.
                let slab = !winit::platform::ios::foldable() && short < 600.0 && long / short > 1.8;
                shapecraft_ui::tablet::set_turns_to_model(slab);
                if slab {
                    let want = Some(self.inner.workspace().is_some());
                    if winit::platform::ios::orientation_lock() != want {
                        winit::platform::ios::set_orientation_lock(want);
                        crate::insets::update_orientations();
                    }
                }
                let z = if folded || sideways && short >= 600.0 && short < 700.0 && long >= 900.0 {
                    1.0
                } else if slab {
                    // An iPhone on its side (about 400 tall): the Duo's compact layout needs about 500.
                    (short / 500.0).min(1.0)
                } else {
                    NativeTouch::fit_zoom_for(safe.0, safe.1).unwrap_or(1.0)
                };
                self.scale = z;
                shapecraft_ui::tablet::set_side_bar(
                    self.side_bar.map(|(x, y, w, h)| egui::Rect::from_min_size(egui::pos2(x / z, y / z), egui::vec2(w / z, h / z))),
                );
                shapecraft_ui::tablet::set_reserved_corner(
                    self.corner.map(|(x, y, w, h)| egui::Rect::from_min_size(egui::pos2(x / z, y / z), egui::vec2(w / z, h / z))),
                );
                let mut m = m;
                let mut rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen.size() / z);
                if slab && sideways {
                    // An iPhone on its side: the Dynamic Island's end is a strip down one side (iOS 27 reports it as
                    // an occlusion region), and the top row is free but for the clock in its middle. The interface
                    // starts past the strip and runs to the top, as on the Duo.
                    rect = egui::Rect::from_min_max(
                        egui::pos2(m.left / z, 0.0),
                        egui::pos2((screen.width() - m.right) / z, screen.height() / z),
                    );
                    m.left = 0.0;
                    m.right = 0.0;
                    m.top = 0.0;
                }
                raw.screen_rect = Some(rect);
                raw.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
                    left: m.left / z,
                    right: m.right / z,
                    top: m.top / z,
                    bottom: m.bottom / z,
                }));
                if let Some(v) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
                    v.native_pixels_per_point = Some(native_ppp * z);
                }
            }
            let ppp = ctx.pixels_per_point();
            let list: Vec<Pointer> = pointers::take().into_iter().map(|r| pointer(r, ppp)).collect();
            self.touch.input(raw, Some(list), self.inner.workspace());
        }

        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            self.frame += 1;
            // The simulator can't be tapped from a script: SIMCTL_CHILD_SHAPECRAFT_SIM_NEW=1 opens a project at once.
            if self.frame == 30 && std::env::var_os("SHAPECRAFT_SIM_NEW").is_some() {
                self.inner.start_new_project();
            }
            if self.frame < 20 || self.frame % 30 == 0 && self.frame < 3000 {
                let zoom = self.scale;
                let line = ctx.input(|i| {
                    let v = i.viewport();
                    format!(
                        "f{} viewport={:?} content={:?} inner={:?} outer={:?} monitor={:?} native_ppp={:?} ppp={} zoom={} safe={:?} {}\n",
                        self.frame,
                        i.viewport_rect(),
                        i.content_rect(),
                        v.inner_rect,
                        v.outer_rect,
                        v.monitor_size,
                        v.native_pixels_per_point,
                        i.pixels_per_point,
                        zoom,
                        i.safe_area_insets(),
                        self.regions,
                    )
                });
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.log) {
                    let _ = f.write_all(line.as_bytes());
                }
            }
            for answer in crate::files::answers() {
                match answer {
                    crate::files::Answer::Picked(id, path) => self.inner.tablet_file_arrived(id, path),
                    crate::files::Answer::Saved(n) => {
                        self.inner.toast(if n == 1 { "Saved".to_string() } else { format!("Saved {n} files") })
                    }
                    crate::files::Answer::Cancelled => {}
                }
            }
            if crate::files::busy() {
                ctx.request_repaint();
            }
            self.inner.logic(ctx, frame);
        }

        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            self.inner.ui(ui, frame);
            // Import and Export go through the Files app's picker.
            crate::files::ask(self.inner.take_tablet_requests());
            let over_view = self.inner.workspace().is_some_and(|ws| ws.viewport_hot);
            self.touch.after_frame(ui.ctx(), over_view);
        }

        fn on_exit(&mut self) {
            self.inner.on_exit();
        }
    }

    /// One of winit's touch records as the touch model's pointer, in points: the Pencil draws and picks, fingers
    /// move the view, a trackpad or mouse pointer clicks.
    fn pointer(r: Record, ppp: f32) -> Pointer {
        let tool = match r.tool {
            Tool::Stylus | Tool::Eraser => PointerTool::Pen,
            Tool::Mouse => PointerTool::Mouse,
            Tool::Finger | Tool::Unknown => PointerTool::Finger,
        };
        let phase = match r.phase {
            Phase::Down | Phase::ButtonPress => PointerPhase::Down,
            Phase::Up | Phase::ButtonRelease => PointerPhase::Up,
            Phase::Move => PointerPhase::Move,
            Phase::Cancel => PointerPhase::Cancel,
            Phase::Hover => PointerPhase::Hover,
            Phase::HoverExit => PointerPhase::Leave,
            Phase::Scroll => PointerPhase::Scroll(egui::vec2(r.scroll.0, r.scroll.1)),
        };
        Pointer { tool, id: r.id as i64, phase, pos: egui::pos2(r.x / ppp, r.y / ppp), button: 0 }
    }
}
