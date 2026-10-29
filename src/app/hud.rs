//! Overlays: toasts, the details panel, command palette, help, context menu and modals.

use std::path::Path;

use egui::{Align2, Color32, CornerRadius, Frame, Key, Margin, Modifiers, Rect, RichText, Stroke};

use super::commands::{COMMANDS, split_args, trim_float};
use super::{ACCENT, FlickApp, TEXT_DIM};
use crate::media::{clock, human_size, timestamp};

pub(super) fn card() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(22, 22, 26, 236))
        .stroke(Stroke::new(1.0, Color32::from_white_alpha(16)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 9))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(110) })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Palette entries: (label, command line).
const PALETTE: &[(&str, &str)] = &[
    ("Open file…", "open-dialog"),
    ("Open file or link from clipboard", "paste"),
    ("Play / pause", "play"),
    ("Next file in folder", "next"),
    ("Previous file in folder", "prev"),
    ("Restart from the beginning", "seek 0"),
    ("Speed 0.5×", "speed 0.5"),
    ("Speed 1×", "speed 1"),
    ("Speed 1.5×", "speed 1.5"),
    ("Speed 2×", "speed 2"),
    ("Mute / unmute", "mute"),
    ("Loop this file", "loop"),
    ("Clip: mark start here", "clip start"),
    ("Clip: mark end here", "clip end"),
    ("Clip: clear marks", "clip off"),
    ("Copy clip as ffmpeg -ss/-to", "copy clip"),
    ("Select a region (crop)", "select"),
    ("Copy region as ffmpeg crop filter", "copy crop"),
    ("Clear region", "select off"),
    ("Next subtitle track", "sub next"),
    ("Subtitles off", "sub off"),
    ("Next audio track", "audio next"),
    ("Rotate 90°", "rotate next"),
    ("Reset rotation", "rotate 0"),
    ("Save screenshot", "screenshot"),
    ("Copy frame to clipboard", "copy frame"),
    ("Copy timestamp", "copy time"),
    ("Copy file path", "copy path"),
    ("Show in Explorer", "reveal"),
    ("Reload from disk", "reload"),
    ("Toggle details panel", "info"),
    ("Fullscreen", "fullscreen"),
    ("Always on top", "ontop"),
    ("Auto-play next file: toggle", "autonext"),
    ("Move to Recycle Bin…", "delete"),
    ("Stop", "stop"),
    ("Keyboard shortcuts", "help"),
    ("Toggle: stay in tray when closed", "background toggle"),
    ("Hide window", "hide"),
    ("Quit Flick", "quit"),
];

const SHORTCUTS: &[(&str, &str)] = &[
    ("Space / K / click", "Play / pause"),
    ("← →", "Seek 5 s  (Shift: 1 s, Ctrl: 1 min)"),
    ("J", "Back 10 s"),
    ("Ctrl+Wheel", "Seek"),
    ("Shift+drag seek bar", "Fine scrub (10× slower)"),
    ("0 … 9", "Jump to 0% … 90%"),
    (", .", "Previous / next frame"),
    ("↑ ↓ / Wheel", "Volume"),
    ("M", "Mute"),
    ("− =", "Slower / faster   (Backspace: 1×)"),
    ("[ ]  \\", "Clip start / end marks, clear"),
    ("L", "Loop file"),
    ("C", "Select a region (for cropping)"),
    ("N P / PgDn PgUp", "Next / previous file in folder"),
    ("S / A", "Cycle subtitles / audio track"),
    ("Shift+S", "Save screenshot"),
    ("R", "Rotate 90°"),
    ("F / Enter / double-click", "Fullscreen"),
    ("I", "Details"),
    ("T", "Always on top"),
    ("Ctrl+C", "Copy timestamp   (Ctrl+Shift+C: path)"),
    ("Ctrl+V", "Open a copied file or link"),
    ("Ctrl+O / Ctrl+P", "Open file / command palette"),
    ("Ctrl+R / F5", "Reload"),
    ("Delete", "Move to Recycle Bin"),
    ("Ctrl+W", "Close to tray"),
    ("Esc", "Clear region / leave fullscreen"),
];

impl FlickApp {
    pub fn draw_overlays(&mut self, ctx: &egui::Context, _rect: Rect) {
        let now = self.last_time;
        if self.settings.info && self.current.is_some() {
            self.info_panel(ctx);
        }
        if let Some((msg, t)) = self.toast.clone() {
            let a = (1.0 - ((now - t - 1.4) / 0.4).clamp(0.0, 1.0)) as f32;
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(Align2::CENTER_TOP, [0.0, 16.0])
                .order(egui::Order::Tooltip)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.multiply_opacity(a);
                    card().show(ui, |ui| ui.label(msg));
                });
        }
        if self.player.pb.buffering && self.current.is_some() {
            egui::Area::new(egui::Id::unique("buffering"))
                .anchor(Align2::RIGHT_TOP, [-16.0, 16.0])
                .interactable(false)
                .show(ctx, |ui| {
                    ui.add(egui::Spinner::new().size(18.0).color(ACCENT));
                });
        }
    }

    fn info_panel(&mut self, ctx: &egui::Context) {
        let p = &self.player;
        let pb = &p.pb;
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(cur) = &self.current {
            rows.push(("File", self.display_name(cur)));
        }
        // ffmpeg names the MP4 demuxer "mov,mp4,m4a,..."; the extension reads better.
        let ext = self.current_path().and_then(|f| f.extension().map(|e| e.to_string_lossy().to_uppercase()));
        let fmt = ext.or_else(|| p.str("file-format").map(|f| f.split(',').next().unwrap_or("").to_string())).unwrap_or_default();
        let size = p.num("file-size").map(|s| human_size(s as u64)).unwrap_or_default();
        rows.push(("Container", [fmt, size].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")));
        if let Some(d) = pb.duration {
            rows.push(("Duration", format!("{}  ({})", clock(d), timestamp(d))));
        }
        if pb.has_video() {
            let mut v = Vec::new();
            if let Some(c) = p.str("video-format") {
                v.push(c);
            }
            if let Some([w, h]) = pb.size() {
                v.push(format!("{w}×{h}"));
            }
            if let Some(fps) = p.num("container-fps") {
                v.push(format!("{} fps", trim_float(fps)));
            }
            rows.push(("Video", v.join(" · ")));
            if let Some([dw, dh]) = pb.dsize().filter(|d| Some(*d) != pb.size()) {
                rows.push(("Display", format!("{dw}×{dh}")));
            }
            // With hardware decoding pixelformat is the surface type ("cuda", "d3d11"); the real one is hw-pixelformat.
            if let Some(px) = p.str("video-params/hw-pixelformat").or_else(|| p.str("video-params/pixelformat")) {
                let mut s = px;
                if let Some(g) = p.str("video-params/gamma").filter(|g| g != "auto") {
                    s.push_str(&format!(" · {g}"));
                }
                rows.push(("Pixels", s));
            }
            let hw = p.str("hwdec-current").filter(|h| h != "no");
            rows.push(("Decoder", hw.map_or("software".into(), |h| format!("hardware ({h})"))));
            if let Some(b) = p.num("video-bitrate") {
                rows.push(("Video rate", format!("{:.1} Mb/s", b / 1e6)));
            }
        }
        if let Some(c) = p.str("audio-codec-name") {
            let mut a = vec![c];
            if let Some(ch) = p.audio_param("channel-count") {
                a.push(format!("{ch:.0} ch"));
            }
            if let Some(sr) = p.audio_param("samplerate") {
                a.push(format!("{:.1} kHz", sr / 1000.0));
            }
            if let Some(b) = p.num("audio-bitrate") {
                a.push(format!("{:.0} kb/s", b / 1000.0));
            }
            rows.push(("Audio", a.join(" · ")));
        }
        if let Some(d) = p.num("frame-drop-count").filter(|d| *d > 0.0) {
            rows.push(("Dropped", format!("{d:.0} frames")));
        }
        if let Some(t) = pb.time_pos {
            rows.push(("Position", timestamp(t)));
        }
        if let Some(r) = self.region {
            rows.push(("Region", r.ffmpeg()));
        }
        egui::Area::new(egui::Id::unique("info"))
            .anchor(Align2::RIGHT_TOP, [-14.0, 14.0])
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                card().show(ui, |ui| {
                    ui.set_max_width(360.0);
                    egui::Grid::new("info-grid").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(RichText::new(k).color(TEXT_DIM));
                            ui.add(egui::Label::new(v).wrap());
                            ui.end_row();
                        }
                    });
                });
            });
        // Bitrate and position change continuously.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    pub fn context_menu(&mut self, resp: &egui::Response) {
        let mut cmd: Option<String> = None;
        let has = self.current.is_some();
        resp.context_menu(|ui| {
            ui.set_min_width(210.0);
            menu_items(ui, &[("Open…", "open-dialog"), ("Paste file or link", "paste")], &mut cmd);
            if has {
                ui.separator();
                let playing = !self.player.pb.pause;
                menu_items(ui, &[(if playing { "Pause" } else { "Play" }, "play")], &mut cmd);
                ui.menu_button("Speed", |ui| {
                    for s in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0] {
                        let sel = (self.player.pb.speed - s).abs() < 1e-6;
                        if ui.add(egui::Button::selectable(sel, format!("{}×", trim_float(s)))).clicked() {
                            cmd = Some(format!("speed {s}"));
                            ui.close();
                        }
                    }
                });
                for (label, kind, prop) in [("Audio", "audio", "audio"), ("Subtitles", "sub", "sub")] {
                    let tracks = self.player.tracks_of(kind);
                    if tracks.is_empty() {
                        continue;
                    }
                    ui.menu_button(label, |ui| {
                        if kind == "sub" && ui.add(egui::Button::selectable(!tracks.iter().any(|t| t.selected), "Off")).clicked() {
                            cmd = Some("sub off".into());
                            ui.close();
                        }
                        for t in &tracks {
                            if ui.add(egui::Button::selectable(t.selected, &t.label)).clicked() {
                                cmd = Some(format!("{prop} {}", t.id));
                                ui.close();
                            }
                        }
                    });
                }
                let loop_label = if self.player.pb.loop_file { "Stop looping" } else { "Loop" };
                menu_items(ui, &[(loop_label, "loop"), ("Rotate 90°", "rotate next")], &mut cmd);
                ui.separator();
                menu_items(
                    ui,
                    &[
                        ("Mark clip start", "clip start"),
                        ("Mark clip end", "clip end"),
                        ("Select region", "select"),
                        ("Save screenshot", "screenshot"),
                        ("Copy frame", "copy frame"),
                        ("Copy timestamp", "copy time"),
                        ("Copy path", "copy path"),
                        ("Show in Explorer", "reveal"),
                    ],
                    &mut cmd,
                );
            }
            ui.separator();
            let ontop = if self.ontop { "Always on top ✓" } else { "Always on top" };
            menu_items(ui, &[("Details", "info"), (ontop, "ontop"), ("Fullscreen", "fullscreen")], &mut cmd);
            ui.separator();
            menu_items(ui, &[("Command palette…", "palette"), ("Keyboard shortcuts", "help")], &mut cmd);
            if has {
                menu_items(ui, &[("Move to Recycle Bin…", "delete")], &mut cmd);
            }
        });
        if let Some(c) = cmd {
            let ctx = resp.ctx.clone();
            if let Err(e) = self.exec(&ctx, &split_args(&c), None) {
                self.toast(e);
            }
        }
    }

    pub fn palette_ui(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let q = self.palette.text.to_lowercase();
        let matches: Vec<(&str, &str)> = PALETTE
            .iter()
            .copied()
            .filter(|(l, c)| q.is_empty() || fuzzy(&l.to_lowercase(), &q) || c.starts_with(q.as_str()))
            .take(10)
            .collect();
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        if down {
            self.palette.selected = (self.palette.selected + 1).min(matches.len().saturating_sub(1));
        }
        if up {
            self.palette.selected = self.palette.selected.saturating_sub(1);
        }
        let mut run: Option<String> = None;
        let modal = egui::Modal::new(egui::Id::unique("palette"))
            .backdrop_color(Color32::from_black_alpha(90))
            .frame(card().inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                let te = egui::TextEdit::singleline(&mut self.palette.text)
                    .hint_text("Type a command…   e.g.  seek 1:30,  speed 1.5,  clip 10 20,  open https://…")
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(16.0));
                let r = ui.add(te);
                if self.palette.just_opened {
                    r.request_focus();
                    self.palette.just_opened = false;
                }
                if r.changed() {
                    self.palette.selected = 0;
                }
                ui.separator();
                for (i, (label, cmd)) in matches.iter().enumerate() {
                    let sel = i == self.palette.selected;
                    let resp = ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::selectable(sel, RichText::new(*label))
                            .right_text(RichText::new(*cmd).monospace().color(TEXT_DIM)),
                    );
                    if resp.clicked() {
                        run = Some(cmd.to_string());
                    }
                }
                if matches.is_empty() && !q.is_empty() {
                    ui.label(RichText::new("Enter runs this as a command (flick ctl syntax)").small().color(TEXT_DIM));
                }
            });
        if enter {
            let typed = self.palette.text.trim().to_string();
            let first_word_is_cmd = typed
                .split_whitespace()
                .next()
                .is_some_and(|w| COMMANDS.iter().any(|(u, _)| u.split(['|', ' ']).any(|t| t == w)));
            run = if first_word_is_cmd && (matches.is_empty() || typed.contains(' ')) {
                Some(typed)
            } else if crate::media::is_url(&typed) {
                Some(format!("open \"{typed}\""))
            } else {
                matches.get(self.palette.selected).map(|(_, c)| c.to_string()).or(Some(typed))
            };
        }
        if modal.should_close() {
            self.palette.open = false;
        }
        if let Some(cmd) = run.filter(|c| !c.is_empty()) {
            self.palette.open = false;
            if let Err(e) = self.exec(ctx, &split_args(&cmd), None) {
                self.toast(e);
            }
        }
    }

    pub fn modals(&mut self, ctx: &egui::Context) {
        if self.help {
            let m = egui::Modal::new(egui::Id::unique("help")).frame(card().inner_margin(Margin::same(20))).show(ctx, |ui| {
                ui.label(RichText::new("Keyboard shortcuts").size(17.0).strong());
                ui.add_space(8.0);
                egui::Grid::new("keys").num_columns(2).spacing([24.0, 5.0]).show(ui, |ui| {
                    for (k, d) in SHORTCUTS {
                        ui.label(RichText::new(*k).monospace().color(ACCENT));
                        ui.label(*d);
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Everything is scriptable:  flick ctl help").small().color(TEXT_DIM));
            });
            if m.should_close() {
                self.help = false;
            }
        }
        if let Some(path) = self.confirm_delete.clone() {
            let mut close = false;
            let mut confirm = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            let m = egui::Modal::new(egui::Id::unique("delete")).frame(card().inner_margin(Margin::same(20))).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new("Move to Recycle Bin?").size(16.0).strong());
                ui.label(RichText::new(file_name(&path)).color(TEXT_DIM));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("  Delete  ").color(Color32::from_rgb(255, 120, 120))).clicked() {
                        confirm = true;
                    }
                    if ui.button("  Cancel  ").clicked() {
                        close = true;
                    }
                });
            });
            if m.should_close() {
                close = true;
            }
            if confirm {
                // Release the file first: Windows won't recycle a file that's open.
                let next = self.files.iter().position(|p| p == &path).map(|i| {
                    self.files.remove(i);
                    i
                });
                self.stop();
                match trash::delete(&path) {
                    Ok(()) => self.toast(format!("Moved {} to the Recycle Bin", file_name(&path))),
                    Err(e) => self.toast(format!("Couldn't delete: {e}")),
                }
                if let Some(i) = next.filter(|_| !self.files.is_empty()) {
                    self.goto_index(i.min(self.files.len() - 1));
                }
                close = true;
            }
            if close {
                self.confirm_delete = None;
            }
        }
    }
}

fn menu_items(ui: &mut egui::Ui, items: &[(&str, &str)], cmd: &mut Option<String>) {
    for (label, c) in items {
        if ui.button(*label).clicked() {
            *cmd = Some(c.to_string());
            ui.close();
        }
    }
}

/// Subsequence match: "nxf" matches "next file".
fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().filter(|c| !c.is_whitespace()).all(|c| it.any(|h| h == c))
}


