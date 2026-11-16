//! Thread's on-canvas controls : at the thread's start end a dashed line across
//! the face, a ↔ at one end (click: type the diameter, which snaps to the nearest size) and an arrow at the other,
//! with the size in a white pill with ⌄ (click: the size list) and a ≡ that opens the card. Custom Length adds a ↕
//! at the far end, the length beside it (click: type the length).

use egui::{Color32, Pos2, Rect, Stroke, Ui, pos2, vec2};
use glam::DVec3;

use super::paint::{PILL_INK, double_arrow, pill, value_box};
use crate::theme::{self, Tokens, radius};
use crate::workspace::Workspace;
use crate::workspace::tools::Tool;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Size,
    Settings,
    Diameter,
    Length,
}

const HITS: &str = "thread-hits";

/// Where the controls go on screen: the start end's middle, the two ends of the line across, the far end's
/// middle (Custom Length).
struct Frame {
    left: Pos2,
    right: Pos2,
    start: Pos2,
    far: Option<Pos2>,
}

impl Workspace {
    fn thread_frame(&self) -> Option<Frame> {
        let Tool::Thread(t) = &self.tool else { return None };
        let f = &t.face;
        let (t0, way) = t.start();
        let r = t.standard.size(&t.size).map_or(f.radius, |s| s.major / 2.0).max(f.radius) + 0.4;
        let c = f.axis.origin + f.axis.dir * t0;
        let start = self.to_screen(c)?;
        // The way across the face that looks most level on screen.
        let y = f.axis.dir.cross(f.x);
        let ex = self.to_screen(c + f.x * r)? - start;
        let ey = self.to_screen(c + y * r)? - start;
        let w: DVec3 = (f.x * f64::from(ex.x) + y * f64::from(ey.x)).try_normalize().unwrap_or(f.x);
        let (a, b) = (self.to_screen(c - w * r)?, self.to_screen(c + w * r)?);
        let (left, right) = if a.x <= b.x { (a, b) } else { (b, a) };
        let far = if t.custom { self.to_screen(c + f.axis.dir * (way * t.length)) } else { None };
        Some(Frame { left, right, start, far })
    }

    /// Draws the controls and the size list when it's open.
    pub(super) fn draw_thread_overlay(&mut self, ui: &mut Ui) {
        let Some(fr) = self.thread_frame() else { return };
        let Tool::Thread(t) = &self.tool else { return };
        let painter = ui.painter_at(self.view_rect);
        let white = Color32::from_white_alpha(230);
        painter.add(egui::Shape::dashed_line(&[fr.left, fr.right], Stroke::new(1.3, white), 5.0, 4.0));
        let across = (fr.right - fr.left).normalized();
        let mut hits = Vec::new();
        // ↔ (the diameter) on the left end, an arrowhead on the right.
        let dia_at = fr.left - across * 14.0;
        double_arrow(&painter, dia_at, across, 11.0, Color32::WHITE, theme::c(0x5a5d66));
        hits.push((Hit::Diameter, Rect::from_center_size(dia_at, vec2(30.0, 22.0))));
        let tip = fr.right + across * 4.0;
        let side = vec2(-across.y, across.x);
        painter.add(egui::Shape::convex_polygon(
            vec![tip, tip - across * 9.0 + side * 4.0, tip - across * 9.0 - side * 4.0],
            Color32::WHITE,
            Stroke::NONE,
        ));
        // The size pill with its ⌄, and the ≡ beside it.
        let mid = fr.start;
        let galley = painter.layout_no_wrap(t.size.clone(), theme::strong(10.5), PILL_INK);
        let size_rect = Rect::from_center_size(mid - vec2(10.0, 0.0), vec2(galley.size().x + 26.0, 18.0));
        painter.rect_filled(size_rect, radius(5), Color32::WHITE);
        painter.galley(pos2(size_rect.left() + 6.0, size_rect.center().y - galley.size().y / 2.0), galley, PILL_INK);
        let chev = pos2(size_rect.right() - 8.5, size_rect.center().y);
        crate::icons::paint(&painter, Rect::from_center_size(chev, vec2(8.0, 8.0)), crate::icons::Icon::ChevronDown, PILL_INK);
        hits.push((Hit::Size, size_rect));
        let gear = Rect::from_center_size(pos2(size_rect.right() + 13.0, mid.y), vec2(18.0, 18.0));
        painter.rect_filled(gear, radius(5), Color32::WHITE);
        for (k, dy) in [-3.5_f32, 0.0, 3.5].into_iter().enumerate() {
            let y = gear.center().y + dy;
            painter.hline(gear.left() + 4.5..=gear.right() - 4.5, y, Stroke::new(1.2, PILL_INK));
            let knob = gear.left() + [6.5, 11.0, 8.0][k];
            painter.circle_filled(pos2(knob, y), 1.6, PILL_INK);
        }
        hits.push((Hit::Settings, gear));
        // Custom Length: a dashed guide up the axis to a blue ↕ and the length beside it.
        if let Some(far) = fr.far {
            let along = (far - fr.start).normalized();
            painter.add(egui::Shape::dashed_line(&[fr.start, far], Stroke::new(1.3, white), 5.0, 4.0));
            double_arrow(&painter, far, along, 11.0, Tokens::accent(), theme::c(0x5a5d66));
            hits.push((Hit::Length, Rect::from_center_size(far, vec2(22.0, 30.0))));
            let text = shapecraft_doc::format_length(t.length, self.session.doc.units);
            let beside = vec2(-along.y, along.x);
            let beside = if beside.x < 0.0 { -beside } else { beside };
            let at = fr.start + (far - fr.start) * 0.5 + beside * (20.0 + text.chars().count() as f32 * 3.3);
            if t.typing_length && self.numeric.is_some() {
                value_box(&painter, at, "", self.numeric.as_deref());
            } else {
                pill(&painter, at, &text, false);
            }
        }
        if !t.typing_length && self.numeric.is_some() {
            value_box(&painter, dia_at - across * 70.0, "", self.numeric.as_deref());
        }
        ui.ctx().data_mut(|d| d.insert_temp(ui.scope_id().with(HITS), hits));
        // The size list, under its pill.
        if t.size_menu {
            let sizes = t.standard.sizes();
            let names: Vec<String> = sizes.iter().map(|s| s.name.clone()).collect();
            let rows: Vec<(&str, Option<crate::icons::Icon>, bool)> = names.iter().map(|n| (n.as_str(), None, *n == t.size)).collect();
            let mut scroll = t.size_scroll;
            if scroll < 0.0 {
                let at = rows.iter().position(|r| r.2).unwrap_or(0);
                scroll = at.saturating_sub(7).min(rows.len().saturating_sub(14)) as f32;
            }
            let top = pos2(size_rect.left(), size_rect.bottom() + 4.0);
            let (hit, rect) = crate::workspace::popover::pull_down_scrolled(ui, "thread-sizes", top, 110.0, &rows, 14, &mut scroll);
            let outside = self.now - self.popup_opened_at > 0.15
                && ui.input(|i| i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|q| !rect.contains(q)));
            let picked = hit.and_then(|k| names.get(k).cloned());
            if let Tool::Thread(t) = &mut self.tool {
                t.size_scroll = scroll;
                if hit.is_some() || outside {
                    t.size_menu = false;
                }
            }
            if let (Some(name), Tool::Thread(t)) = (picked, &self.tool) {
                let mut t = t.clone();
                t.set_size(&name);
                self.apply_thread(t);
            }
        }
    }

    /// A click on one of Thread's controls; true when it took the click.
    pub(super) fn thread_click(&mut self, ui: &Ui, c: Pos2) -> bool {
        let Tool::Thread(t) = &mut self.tool else { return false };
        if t.size_menu {
            // The list takes its own clicks; one outside just closes it.
            return true;
        }
        let hits: Vec<(Hit, Rect)> = ui.ctx().data(|d| d.get_temp(ui.scope_id().with(HITS))).unwrap_or_default();
        let Some((hit, _)) = hits.into_iter().find(|(_, r)| r.expand(2.0).contains(c)) else { return false };
        match hit {
            Hit::Size => {
                t.size_menu = true;
                t.size_scroll = -1.0;
                self.popup_opened_at = self.now;
            }
            Hit::Settings => {
                self.options_menu = if self.options_menu.is_some() { None } else { Some(c) };
                self.popup_opened_at = self.now;
            }
            Hit::Diameter | Hit::Length => {
                t.typing_length = hit == Hit::Length;
                self.numeric = Some(String::new());
                self.numeric_fresh = true;
            }
        }
        true
    }
}
