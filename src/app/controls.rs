//! The auto-hiding control bar (seek bar, transport, volume, tracks), the empty and
//! error screens, and the region-selection overlay.

use egui::{Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::commands::{split_args, trim_float};
use super::hud::card;
use super::{ACCENT, FlickApp, TEXT, TEXT_DIM};
use crate::media::clock;

/// Glyphs from Segoe Fluent Icons / Segoe MDL2 Assets.
pub mod icon {
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const PREV: &str = "\u{E892}";
    pub const NEXT: &str = "\u{E893}";
    pub const VOL1: &str = "\u{E993}";
    pub const VOL2: &str = "\u{E994}";
    pub const VOL3: &str = "\u{E995}";
    pub const MUTE: &str = "\u{E74F}";
    pub const FULLSCREEN: &str = "\u{E740}";
    pub const WINDOWED: &str = "\u{E73F}";
    pub const CC: &str = "\u{E7F0}";
    pub const AUDIO: &str = "\u{E8D6}";
    pub const REPEAT: &str = "\u{E8EE}";
    pub const WARNING: &str = "\u{E7BA}";
}

const BAR_H: f32 = 104.0;

pub fn icons(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("icons".into()))
}

/// A flat icon button: no frame until hovered, accent color when `active`.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tip: &str, active: bool, size: f32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    let hover = resp.hovered() || resp.is_pointer_button_down_on();
    if hover {
        ui.painter().rect_filled(r, CornerRadius::same(8), Color32::from_white_alpha(if resp.is_pointer_button_down_on() { 30 } else { 18 }));
    }
    let color = if active { ACCENT } else { TEXT };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, glyph, icons(size), color);
    resp.on_hover_text(tip)
}

/// A seek-bar drag in progress, or just released and waiting for mpv to land.
pub struct Scrub {
    /// The time being scrubbed to (NaN until the first pointer frame).
    pub t: f64,
    /// Playback was running when the drag began: resume it on release.
    resume: bool,
    /// The last seek target sent, and when.
    sent: Option<(f64, f64)>,
    /// When `t` last changed.
    moved: f64,
    /// An exact seek to `t` has gone out.
    exact: bool,
    /// When the button came up.
    released: Option<f64>,
}

impl FlickApp {
    fn controls_wanted(&self, ctx: &egui::Context, rect: Rect) -> bool {
        let pb = &self.player.pb;
        let now = self.last_time;
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let near_bottom = pointer.is_some_and(|p| rect.contains(p) && p.y > rect.bottom() - BAR_H);
        let recently = now - self.last_activity < 2.0;
        let want = pb.pause
            || recently
            || near_bottom
            || self.scrub.is_some()
            || self.loading
            || self.error.is_some()
            || !pb.has_video()
            || egui::Popup::is_any_open(ctx);
        if want && !near_bottom && !pb.pause {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((2.05 - (now - self.last_activity)).max(0.05)));
        }
        want
    }

    pub fn draw_controls(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        if self.current.is_none() {
            return;
        }
        let want = self.controls_wanted(&ctx, rect);
        self.lift_subtitles(want, rect);
        let alpha = ctx.animate_bool_with_time(Id::unique("controls-vis"), want, 0.2);
        if alpha <= 0.0 {
            // Controls gone while playing: hide the cursor too, like any video player.
            if !self.selecting && ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p)) {
                ctx.set_cursor_icon(egui::CursorIcon::None);
            }
            return;
        }
        let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - BAR_H), rect.max);
        egui::Area::new(Id::unique("controls"))
            .fixed_pos(bar.min)
            .order(egui::Order::Middle)
            .show(&ctx, |ui| {
                ui.set_min_size(bar.size());
                ui.set_max_size(bar.size());
                ui.multiply_opacity(alpha);
                scrim(ui.painter(), bar);
                let seek = Rect::from_min_max(pos2(bar.left() + 18.0, bar.bottom() - 74.0), pos2(bar.right() - 18.0, bar.bottom() - 54.0));
                self.seek_bar(ui, seek);
                let row = Rect::from_min_max(pos2(bar.left() + 10.0, bar.bottom() - 46.0), pos2(bar.right() - 10.0, bar.bottom() - 8.0));
                self.button_row(ui, row);
            });
        // Full-screen title, top left, fading with the controls.
        if self.is_fullscreen() {
            if let Some(cur) = self.current.clone() {
                let name = self.display_name(&cur);
                egui::Area::new(Id::unique("fs-title"))
                    .fixed_pos(rect.min)
                    .order(egui::Order::Middle)
                    .interactable(false)
                    .show(&ctx, |ui| {
                        ui.multiply_opacity(alpha);
                        let top = Rect::from_min_size(rect.min, vec2(rect.width(), 80.0));
                        scrim_top(ui.painter(), top);
                        ui.painter().text(rect.min + vec2(22.0, 20.0), Align2::LEFT_TOP, name, FontId::proportional(17.0), TEXT);
                    });
            }
        }
    }

    /// Move subtitles above the control bar while it's showing, so neither hides the other.
    fn lift_subtitles(&mut self, controls: bool, rect: Rect) {
        let mut pos = 100;
        if let (true, Some(area)) = (controls, self.video_area(rect)) {
            let limit = rect.bottom() - BAR_H + 16.0;
            if area.bottom() > limit && area.height() > 0.0 {
                pos = (((limit - area.top()) / area.height()) * 100.0).clamp(50.0, 100.0) as i64;
            }
        }
        if pos != self.sub_pos {
            self.sub_pos = pos;
            let _ = self.player.set("sub-pos", &pos.to_string());
        }
    }

    fn seek_bar(&mut self, ui: &mut egui::Ui, r: Rect) {
        let pb = self.player.pb.clone();
        let resp = ui.interact(r, Id::unique("seek"), Sense::click_and_drag());
        let Some(dur) = pb.duration else {
            // Live streams and still-loading files: just a quiet track.
            ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(r.width(), 4.0)), 2.0, Color32::from_white_alpha(40));
            self.scrub = None;
            return;
        };
        let x_of = |t: f64| r.left() + (t / dur).clamp(0.0, 1.0) as f32 * r.width();
        let t_of = |x: f32| ((x - r.left()) / r.width()).clamp(0.0, 1.0) as f64 * dur;
        let dragging = self.scrub.as_ref().is_some_and(|s| s.released.is_none());
        let active = resp.hovered() || dragging;
        let thick = 4.0 + 2.0 * ui.ctx().animate_bool_with_time(Id::unique("seek-thick"), active, 0.12);
        let track = Rect::from_center_size(r.center(), vec2(r.width(), thick));
        let p = ui.painter();
        p.rect_filled(track, thick / 2.0, Color32::from_white_alpha(46));
        if let Some(c) = pb.cache_time.filter(|c| *c > 0.0) {
            let t0 = pb.time_pos.unwrap_or(0.0);
            if c > t0 {
                let cr = Rect::from_min_max(pos2(x_of(t0), track.top()), pos2(x_of(c), track.bottom()));
                p.rect_filled(cr, thick / 2.0, Color32::from_white_alpha(40));
            }
        }
        let shown = self.shown_time();
        let played = Rect::from_min_max(track.min, pos2(x_of(shown), track.bottom()));
        p.rect_filled(played, thick / 2.0, ACCENT);
        // Clip marks: the range is tinted, each mark is a tick above the track.
        let [a, b] = pb.ab;
        if let (Some(a), Some(b)) = (a, b) {
            let cr = Rect::from_min_max(pos2(x_of(a), track.top() - 3.0), pos2(x_of(b), track.bottom() + 3.0));
            p.rect_filled(cr, 2.0, ACCENT.gamma_multiply(0.28));
        }
        for t in [a, b].into_iter().flatten() {
            let x = x_of(t);
            p.rect_filled(Rect::from_min_max(pos2(x - 1.0, track.top() - 7.0), pos2(x + 1.0, track.bottom() + 7.0)), 1.0, ACCENT);
        }
        if active {
            p.circle_filled(pos2(x_of(shown), track.center().y), 7.0, TEXT);
        }
        // The tip follows the knob while dragging (they part ways in Shift fine mode),
        // and shows tenths then, since that's when the exact spot matters.
        let tip_at = if dragging { Some((x_of(shown), shown)) } else { resp.hover_pos().map(|h| (h.x, t_of(h.x))) };
        if let Some((hx, t)) = tip_at {
            let label = if dragging { format!("{}.{}", clock(t), (t.fract() * 10.0) as u32) } else { clock(t) };
            let galley = p.layout_no_wrap(label, FontId::monospace(12.0), TEXT);
            let size = galley.size() + vec2(14.0, 8.0);
            let x = hx.clamp(r.left() + size.x / 2.0, r.right() - size.x / 2.0);
            let tip = Rect::from_center_size(pos2(x, r.top() - size.y / 2.0 - 4.0), size);
            p.rect_filled(tip, 6.0, Color32::from_rgba_unmultiplied(22, 22, 26, 236));
            p.galley(tip.center() - galley.size() / 2.0, galley, TEXT);
        }
        self.scrub_input(ui, &resp, r, dur);
    }

    /// Drive a seek-bar drag. Playback pauses for the drag and resumes after. While the
    /// pointer moves, seeks go to keyframes (cheap, so the picture keeps up); once it
    /// rests, an exact seek lands on the frame under it. Shift slows the drag 10x.
    fn scrub_input(&mut self, ui: &egui::Ui, resp: &egui::Response, r: Rect, dur: f64) {
        let now = self.last_time;
        let per_px = dur / r.width().max(1.0) as f64;
        let t_of = |x: f32| ((x - r.left()) / r.width()).clamp(0.0, 1.0) as f64 * dur;
        let pointer = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on());
        if let Some(pos) = pointer {
            let (fine, dx) = ui.input(|i| (i.modifiers.shift, i.pointer.delta().x));
            let mut s = match self.scrub.take() {
                Some(s) if s.released.is_none() => s,
                // A new drag. If the last one is still settling, its resume carries over:
                // mpv may not have reported the unpause yet.
                prev => {
                    let pb = &self.player.pb;
                    let resume = prev.is_some_and(|p| p.resume) || (!pb.pause && !pb.eof);
                    if resume {
                        let _ = self.player.set("pause", "yes");
                    }
                    Scrub { t: f64::NAN, resume, sent: None, moved: now, exact: false, released: None }
                }
            };
            let t = if fine && !s.t.is_nan() { (s.t + dx as f64 * per_px / 10.0).clamp(0.0, dur) } else { t_of(pos.x) };
            if t != s.t {
                s.t = t;
                s.moved = now;
                s.exact = false;
            }
            if !s.exact {
                // Don't stack seeks while mpv is still working on the last one.
                let busy = self.player.pb.seeking && s.sent.is_some_and(|(_, at)| now - at < 0.25);
                if !busy {
                    if now - s.moved > 0.12 {
                        let _ = self.player.cmd(&["seek", &format!("{:.3}", s.t), "absolute+exact"]);
                        s.sent = Some((s.t, now));
                        s.exact = true;
                    } else if s.sent.is_none_or(|(st, _)| (st - s.t).abs() >= per_px) {
                        let _ = self.player.cmd(&["seek", &format!("{:.3}", s.t), "absolute+keyframes"]);
                        s.sent = Some((s.t, now));
                    }
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
            }
            self.scrub = Some(s);
            return;
        }
        if resp.clicked() && self.scrub.as_ref().is_none_or(|s| s.released.is_some()) {
            // A click too quick to be seen as held: just seek there.
            if let Some(pos) = resp.interact_pointer_pos() {
                let _ = self.player.cmd(&["seek", &format!("{:.3}", t_of(pos.x)), "absolute+exact"]);
            }
        }
        let pb = &self.player.pb;
        let Some(s) = &mut self.scrub else { return };
        match s.released {
            None => {
                if !(s.exact && s.sent.is_some_and(|(st, _)| st == s.t)) {
                    let _ = self.player.cmd(&["seek", &format!("{:.3}", s.t), "absolute+exact"]);
                }
                if s.resume {
                    let _ = self.player.set("pause", "no");
                }
                s.released = Some(now);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
            }
            // Keep the knob at the target until mpv's position gets there, so it doesn't
            // flick back to the old spot for a frame.
            Some(at) => {
                let landed = !pb.seeking && pb.time_pos.is_some_and(|p| (p - s.t).abs() < 0.5) && now - at > 0.05;
                if landed || now - at > 1.5 {
                    self.scrub = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
                }
            }
        }
    }

    /// The position the UI shows: the scrub target during and just after a drag.
    fn shown_time(&self) -> f64 {
        self.scrub.as_ref().map(|s| s.t).filter(|t| !t.is_nan()).or(self.player.pb.time_pos).unwrap_or(0.0)
    }

    fn button_row(&mut self, ui: &mut egui::Ui, row: Rect) {
        let pb = self.player.pb.clone();
        let mut run: Vec<String> = Vec::new();
        let fullscreen = self.is_fullscreen();
        ui.scope_builder(egui::UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Mid-scrub the pause is ours; show what playback will go back to.
            let playing = match &self.scrub {
                Some(s) if s.released.is_none() => s.resume,
                _ => !pb.pause && !pb.eof,
            };
            if icon_button(ui, if playing { icon::PAUSE } else { icon::PLAY }, if playing { "Pause (Space)" } else { "Play (Space)" }, false, 18.0).clicked() {
                run.push("play".into());
            }
            if self.files.len() > 1 {
                if icon_button(ui, icon::PREV, "Previous file (P)", false, 15.0).clicked() {
                    run.push("prev".into());
                }
                if icon_button(ui, icon::NEXT, "Next file (N)", false, 15.0).clicked() {
                    run.push("next".into());
                }
            }
            let vol_icon = if pb.mute || pb.volume <= 0.0 {
                icon::MUTE
            } else if pb.volume < 34.0 {
                icon::VOL1
            } else if pb.volume < 67.0 {
                icon::VOL2
            } else {
                icon::VOL3
            };
            if icon_button(ui, vol_icon, "Mute (M)", false, 16.0).clicked() {
                run.push("mute".into());
            }
            if let Some(v) = volume_slider(ui, if pb.mute { 0.0 } else { pb.volume }) {
                run.push(format!("volume {v:.0}"));
            }
            ui.add_space(10.0);
            let t = self.shown_time();
            let time = match pb.duration {
                Some(d) => format!("{} / {}", clock(t), clock(d)),
                None => clock(t),
            };
            ui.label(RichText::new(time).monospace().size(13.0).color(TEXT));
            if self.files.len() > 1 {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{} of {}", self.index + 1, self.files.len())).small().color(TEXT_DIM));
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, if fullscreen { icon::WINDOWED } else { icon::FULLSCREEN }, "Fullscreen (F)", false, 15.0).clicked() {
                    run.push("fullscreen".into());
                }
                let subs = self.player.tracks_of("sub");
                if !subs.is_empty() {
                    let on = subs.iter().any(|t| t.selected);
                    let r = icon_button(ui, icon::CC, "Subtitles (S)", on, 17.0);
                    track_menu(&r, &subs, true, "sub", &mut run);
                }
                let audio = self.player.tracks_of("audio");
                if audio.len() > 1 {
                    let r = icon_button(ui, icon::AUDIO, "Audio track (A)", false, 16.0);
                    track_menu(&r, &audio, false, "audio", &mut run);
                }
                if icon_button(ui, icon::REPEAT, if pb.loop_file { "Loop: on (L)" } else { "Loop: off (L)" }, pb.loop_file, 16.0).clicked() {
                    run.push("loop".into());
                }
                let speed = egui::Button::new(RichText::new(format!("{}×", trim_float(pb.speed))).monospace().color(
                    if (pb.speed - 1.0).abs() > 1e-6 { ACCENT } else { TEXT_DIM },
                ))
                .frame(false)
                .min_size(vec2(40.0, 36.0));
                let r = ui.add(speed).on_hover_text("Speed (− / =)");
                egui::Popup::menu(&r).show(|ui| {
                    for s in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
                        let sel = (pb.speed - s).abs() < 1e-6;
                        if ui.add(egui::Button::selectable(sel, format!("{}×", trim_float(s)))).clicked() {
                            run.push(format!("speed {s}"));
                        }
                    }
                });
                if let [Some(a), Some(b)] = pb.ab {
                    ui.add_space(4.0);
                    let r = ui
                        .add(egui::Button::new(RichText::new(format!("Clip {} – {}", clock(a), clock(b))).small().color(ACCENT)).frame(false))
                        .on_hover_text("Looping between the clip marks. Click to clear (\\)");
                    if r.clicked() {
                        run.push("clip off".into());
                    }
                }
            });
        });
        let ctx = ui.ctx().clone();
        for c in run {
            if let Err(e) = self.exec(&ctx, &split_args(&c), None) {
                self.toast(e);
            }
        }
    }

    /// Nothing loaded: an inviting drop target. Failed to load: what went wrong.
    pub fn draw_empty_or_error(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        if let Some(err) = self.error.clone() {
            let name = self.current.as_deref().map(|c| self.display_name(c)).unwrap_or_default();
            egui::Area::new(Id::unique("error"))
                .anchor(Align2::CENTER_CENTER, [0.0, -20.0])
                .order(egui::Order::Middle)
                .show(&ctx, |ui| {
                    card().inner_margin(egui::Margin::same(22)).show(ui, |ui| {
                        ui.set_max_width(440.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::WARNING).font(icons(18.0)).color(ACCENT));
                            ui.label(RichText::new("Can't play this file").size(16.0).strong());
                        });
                        ui.label(RichText::new(name).color(TEXT_DIM));
                        ui.add_space(4.0);
                        ui.label(RichText::new(err).small().color(TEXT_DIM));
                    });
                });
            return;
        }
        if self.current.is_some() {
            if self.loading && !self.player.pb.has_video() {
                egui::Area::new(Id::unique("spinner"))
                    .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                    .interactable(false)
                    .show(&ctx, |ui| {
                        ui.add(egui::Spinner::new().size(28.0).color(ACCENT));
                    });
            } else if !self.player.pb.has_video() && !self.loading {
                // Audio only: show the title instead of a black screen.
                let name = self.current.as_deref().map(|c| self.display_name(c)).unwrap_or_default();
                let p = ui.painter();
                p.text(rect.center() - vec2(0.0, 40.0), Align2::CENTER_CENTER, icon::AUDIO, icons(54.0), ACCENT.gamma_multiply(0.9));
                p.text(rect.center() + vec2(0.0, 20.0), Align2::CENTER_CENTER, name, FontId::proportional(17.0), TEXT);
            }
            return;
        }
        let p = ui.painter();
        let c = rect.center() - vec2(0.0, 30.0);
        p.circle_filled(c, 46.0, Color32::from_white_alpha(8));
        p.text(c + vec2(3.0, 0.0), Align2::CENTER_CENTER, icon::PLAY, icons(38.0), ACCENT);
        p.text(c + vec2(0.0, 78.0), Align2::CENTER_CENTER, "Drop a video here", FontId::proportional(18.0), TEXT);
        p.text(
            c + vec2(0.0, 106.0),
            Align2::CENTER_CENTER,
            "Click or Ctrl+O to open  ·  Ctrl+V to paste a file or link",
            FontId::proportional(13.0),
            TEXT_DIM,
        );
    }

    /// The marked region: everything outside is dimmed, with its size and position on a label.
    pub fn draw_region(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(reg) = self.region else {
            if self.selecting {
                if let Some(area) = self.video_area(rect) {
                    ui.painter().rect_stroke(area, 0.0, Stroke::new(1.0, ACCENT.gamma_multiply(0.6)), egui::StrokeKind::Inside);
                }
            }
            return;
        };
        let Some(r) = self.region_rect(reg, rect) else { return };
        let Some(area) = self.video_area(rect) else { return };
        let p = ui.painter();
        let dim = Color32::from_black_alpha(130);
        for shade in [
            Rect::from_min_max(area.min, pos2(area.right(), r.top())),
            Rect::from_min_max(pos2(area.left(), r.bottom()), area.max),
            Rect::from_min_max(pos2(area.left(), r.top()), pos2(r.left(), r.bottom())),
            Rect::from_min_max(pos2(r.right(), r.top()), pos2(area.right(), r.bottom())),
        ] {
            p.rect_filled(shade, 0.0, dim);
        }
        p.rect_stroke(r, 0.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        if self.drag_from.is_some() {
            let label = format!("{} × {}", reg.w, reg.h);
            p.text(r.left_top() + vec2(4.0, -4.0), Align2::LEFT_BOTTOM, label, FontId::monospace(12.0), ACCENT);
            return;
        }
        let ctx = ui.ctx().clone();
        let mut run = None;
        let anchor = pos2(r.left().max(rect.left() + 8.0), (r.top() - 44.0).max(rect.top() + 8.0));
        egui::Area::new(Id::unique("region-card")).fixed_pos(anchor).order(egui::Order::Middle).show(&ctx, |ui| {
            card().inner_margin(egui::Margin::symmetric(10, 5)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} × {}  at  {}, {}", reg.w, reg.h, reg.x, reg.y)).monospace());
                    if ui.small_button("Copy crop").on_hover_text(reg.ffmpeg()).clicked() {
                        run = Some("copy crop");
                    }
                    if ui.small_button("Clear").clicked() {
                        run = Some("select off");
                    }
                });
            });
        });
        if let Some(c) = run {
            if let Err(e) = self.exec(&ctx, &split_args(c), None) {
                self.toast(e);
            }
        }
    }
}

fn volume_slider(ui: &mut egui::Ui, volume: f64) -> Option<f64> {
    let (r, resp) = ui.allocate_exact_size(vec2(84.0, 36.0), Sense::click_and_drag());
    let track = Rect::from_center_size(r.center(), vec2(r.width() - 8.0, 4.0));
    let frac = (volume / 100.0).clamp(0.0, 1.0) as f32;
    let p = ui.painter();
    p.rect_filled(track, 2.0, Color32::from_white_alpha(46));
    p.rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * frac, track.bottom())), 2.0, TEXT);
    if resp.hovered() || resp.dragged() {
        p.circle_filled(pos2(track.left() + track.width() * frac, track.center().y), 6.0, TEXT);
    }
    let resp = resp.on_hover_text(format!("Volume {volume:.0}%"));
    if resp.dragged() || resp.clicked() {
        let x = resp.interact_pointer_pos()?.x;
        return Some((((x - track.left()) / track.width()).clamp(0.0, 1.0) * 100.0).round() as f64);
    }
    None
}

fn track_menu(resp: &egui::Response, tracks: &[super::player::Track], with_off: bool, cmd: &str, run: &mut Vec<String>) {
    egui::Popup::menu(resp).show(|ui| {
        ui.set_min_width(220.0);
        if with_off {
            let none = !tracks.iter().any(|t| t.selected);
            if ui.add(egui::Button::selectable(none, "Off")).clicked() {
                run.push(format!("{cmd} off"));
            }
        }
        for t in tracks {
            if ui.add(egui::Button::selectable(t.selected, &t.label)).clicked() {
                run.push(format!("{cmd} {}", t.id));
            }
        }
    });
}

/// Bottom gradient so white controls read over bright video.
fn scrim(p: &egui::Painter, r: Rect) {
    gradient(p, r, Color32::TRANSPARENT, Color32::from_black_alpha(190));
}

fn scrim_top(p: &egui::Painter, r: Rect) {
    gradient(p, r, Color32::from_black_alpha(160), Color32::TRANSPARENT);
}

fn gradient(p: &egui::Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.left_bottom(), bottom);
    m.colored_vertex(r.right_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    p.add(egui::Shape::mesh(m));
}






