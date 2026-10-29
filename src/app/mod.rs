//! The player window.

mod commands;
mod controls;
mod hud;
mod player;
mod tray;
mod video;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};

use egui::{Color32, Key, Modifiers, Pos2, Rect, Sense};
use serde::{Deserialize, Serialize};

use crate::ipc::{self, Incoming, Response};
use crate::mpv::Mpv;
use player::Player;
use video::SharedSurface;

pub const CANVAS: Color32 = Color32::from_rgb(0x13, 0x13, 0x15);
pub const ACCENT: Color32 = Color32::from_rgb(0xf5, 0xb8, 0x4a);
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xe9, 0xec);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8d, 0x8d, 0x95);

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub volume: f64,
    pub mute: bool,
    pub info: bool,
    /// At the end of a file, play the next one in the folder.
    pub autonext: bool,
    /// Closing the window hides it (tray icon stays) so the next open is instant.
    pub background: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { volume: 100.0, mute: false, info: false, autonext: false, background: true }
    }
}

/// A rectangle in video pixels, for crop commands.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Region {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Region {
    pub fn ffmpeg(&self) -> String {
        format!("crop={}:{}:{}:{}", self.w, self.h, self.x, self.y)
    }
}

struct SnapReq {
    path: PathBuf,
    reply: mpsc::Sender<Response>,
}

#[derive(Default)]
struct Palette {
    open: bool,
    text: String,
    selected: usize,
    just_opened: bool,
}

pub struct FlickApp {
    ctx: egui::Context,
    ipc_rx: Receiver<Incoming>,
    settings: Settings,
    player: Player,
    surface: Option<SharedSurface>,

    /// The folder playlist (or the explicit list that was opened).
    files: Vec<PathBuf>,
    index: usize,
    /// What's playing: a file path or a URL.
    current: Option<String>,
    list_dirty: bool,
    error: Option<String>,
    /// `loadfile` sent, waiting for mpv to report the file loaded (or failed).
    loading: bool,
    was_eof: bool,

    region: Option<Region>,
    selecting: bool,
    drag_from: Option<Pos2>,
    scrub: Option<controls::Scrub>,
    video_rect: Rect,
    /// Current mpv sub-pos (100 = bottom); raised while the controls show.
    sub_pos: i64,

    palette: Palette,
    help: bool,
    confirm_delete: Option<PathBuf>,
    toast: Option<(String, f64)>,
    last_activity: f64,
    last_time: f64,
    ontop: bool,
    snaps: Vec<SnapReq>,
    title: String,

    _watcher: Option<notify::RecommendedWatcher>,
    watch_rx: Receiver<PathBuf>,
    watch_tx: mpsc::Sender<PathBuf>,
    watched_dir: Option<PathBuf>,
    reload_at: Option<f64>,

    startup: Vec<Vec<String>>,
    quit: bool,
    hidden: bool,
    hide_in: u8,
    tray: Option<tray::Tray>,
    dialog_tx: mpsc::Sender<Vec<PathBuf>>,
    dialog_rx: Receiver<Vec<PathBuf>>,
    awake: bool,
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Flick")
}

/// Settings are read before the window exists (mpv needs the volume at init), so they
/// live in our own file rather than eframe's storage.
fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

fn load_settings() -> Settings {
    std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Start the GUI. `startup` holds commands (argv form) to run on the first frame.
/// `background`: start with the window hidden (login autostart); it appears on the first open.
pub fn run(server: ipc::Server, startup: Vec<Vec<String>>, background: bool) -> Result<(), String> {
    let settings = load_settings();
    let opts = player::options(&data_dir(), settings.volume, settings.mute);
    let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mpv = Arc::new(Mpv::new(&opts)?);

    let ctx_cell: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let ipc_rx = server.spawn(ctx_cell.clone());

    let icon = image::load_from_memory(include_bytes!("../../assets/icon-256.png"))
        .map(|i| {
            let i = i.to_rgba8();
            egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
        })
        .ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Flick")
        .with_app_id("flick")
        .with_inner_size([1120.0, 680.0])
        .with_min_inner_size([360.0, 220.0])
        .with_drag_and_drop(true)
        .with_visible(!background);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "Flick",
        options,
        Box::new(move |cc| {
            let _ = ctx_cell.set(cc.egui_ctx.clone());
            crate::win::style_titlebar(cc, [CANVAS.r(), CANVAS.g(), CANVAS.b()]);
            setup_style(&cc.egui_ctx);
            {
                let ctx = cc.egui_ctx.clone();
                mpv.set_wakeup(Box::new(move || ctx.request_repaint()));
            }
            // The GL context is current here, which mpv's renderer needs.
            let dirty = Arc::new(AtomicBool::new(true));
            let surface = {
                let (ctx, flag) = (cc.egui_ctx.clone(), dirty.clone());
                mpv.create_renderer(
                    crate::mpv::wgl_get_proc_address,
                    Box::new(move || {
                        flag.store(true, Ordering::Release);
                        ctx.request_repaint();
                    }),
                )
                .map(|r| video::Surface::new(r, dirty))
            };
            let surface = match surface {
                Ok(s) => Some(s),
                Err(e) => {
                    crate::win::message_box(&format!("Flick couldn't set up video output:\n\n{e}"));
                    None
                }
            };
            let mut app = FlickApp::new(cc.egui_ctx.clone(), ipc_rx, settings, Player::new(mpv), surface, startup, background);
            app.tray = tray::create(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let try_font = |file: &str| std::fs::read(Path::new(&windir).join("Fonts").join(file)).ok();
    if let Some(data) = try_font("SegUIVar.ttf").or_else(|| try_font("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(data)));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    if let Some(data) = try_font("CascadiaMono.ttf").or_else(|| try_font("consola.ttf")) {
        fonts.font_data.insert("mono".into(), Arc::new(egui::FontData::from_owned(data)));
        fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "mono".into());
    }
    if let Some(data) = try_font("seguisym.ttf") {
        fonts.font_data.insert("symbols".into(), Arc::new(egui::FontData::from_owned(data)));
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("symbols".into());
        }
    }
    // Player glyphs: Segoe Fluent Icons (Windows 11), else Segoe MDL2 Assets (Windows 10). Same code points.
    if let Some(data) = try_font("SegoeIcons.ttf").or_else(|| try_font("segmdl2.ttf")) {
        fonts.font_data.insert("icons".into(), Arc::new(egui::FontData::from_owned(data)));
        fonts.families.insert(egui::FontFamily::Name("icons".into()), vec!["icons".into()]);
    } else {
        fonts.families.insert(egui::FontFamily::Name("icons".into()), vec!["symbols".into()]);
    }
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = CANVAS;
    visuals.window_fill = Color32::from_rgb(0x1c, 0x1c, 0x20);
    visuals.window_stroke = egui::Stroke::new(1.0, Color32::from_white_alpha(14));
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.hyperlink_color = ACCENT;
    visuals.override_text_color = Some(TEXT);
    for w in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(6);
    }
    visuals.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(8);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(20);
    visuals.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.5);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
        s.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.5));
    });
}

impl FlickApp {
    fn new(
        ctx: egui::Context,
        ipc_rx: Receiver<Incoming>,
        settings: Settings,
        player: Player,
        surface: Option<SharedSurface>,
        startup: Vec<Vec<String>>,
        hidden: bool,
    ) -> Self {
        let (watch_tx, watch_rx) = mpsc::channel();
        let (dialog_tx, dialog_rx) = mpsc::channel();
        Self {
            ctx,
            ipc_rx,
            settings,
            player,
            surface,
            files: Vec::new(),
            index: 0,
            current: None,
            list_dirty: false,
            error: None,
            loading: false,
            was_eof: false,
            region: None,
            selecting: false,
            drag_from: None,
            scrub: None,
            video_rect: Rect::NOTHING,
            sub_pos: 100,
            palette: Palette::default(),
            help: false,
            confirm_delete: None,
            toast: None,
            last_activity: 0.0,
            last_time: 0.0,
            ontop: false,
            snaps: Vec::new(),
            title: String::new(),
            _watcher: None,
            watch_rx,
            watch_tx,
            watched_dir: None,
            reload_at: None,
            startup,
            quit: false,
            hidden,
            hide_in: if hidden { 2 } else { 0 },
            tray: None,
            dialog_tx,
            dialog_rx,
            awake: false,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), self.last_time));
        self.ctx.request_repaint();
    }

    fn save_settings(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_vec_pretty(&self.settings) {
            let _ = std::fs::write(settings_path(), s);
        }
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.current.as_deref().filter(|c| !crate::media::is_url(c)).map(PathBuf::from)
    }

    // ---------------------------------------------------------------- files

    pub fn open(&mut self, path: &Path) {
        let path = absolute(path);
        if path.is_dir() {
            let list = folder_media(&path);
            if list.is_empty() {
                self.toast(format!("No videos in {}", path.display()));
            } else {
                self.open_list(list);
            }
            return;
        }
        if self.list_dirty || !self.files.iter().any(|p| p == &path) {
            self.files = crate::media::siblings(&path);
            self.list_dirty = false;
        }
        self.index = self.files.iter().position(|p| p == &path).unwrap_or(0);
        self.play(path.display().to_string(), None);
    }

    /// Open an explicit list of files as the playlist.
    pub fn open_list(&mut self, paths: Vec<PathBuf>) {
        let paths: Vec<PathBuf> = paths.iter().map(|p| absolute(p)).collect();
        if paths.len() == 1 {
            return self.open(&paths[0]);
        }
        let Some(first) = paths.first().cloned() else { return };
        self.files = paths;
        self.index = 0;
        self.play(first.display().to_string(), None);
    }

    pub fn open_url(&mut self, url: &str) {
        self.files.clear();
        self.index = 0;
        self.play(url.trim().to_string(), None);
    }

    /// Remember where we were in the outgoing file, so reopening it resumes there.
    /// `sync`: wait for the write (on exit, before mpv is torn down).
    fn save_position(&self, sync: bool) {
        let pb = &self.player.pb;
        let (Some(t), Some(d)) = (pb.time_pos, pb.duration) else { return };
        if self.current.is_some() && t > 10.0 && d - t > 15.0 && !pb.eof {
            let args = ["write-watch-later-config"];
            let _ = if sync { self.player.cmd_sync(&args) } else { self.player.cmd(&args) };
        }
    }

    /// Load `target` (path or URL). `start`: resume at this position (reloads).
    fn play(&mut self, target: String, start: Option<(f64, bool)>) {
        self.save_position(false);
        if let Some(p) = (!crate::media::is_url(&target)).then(|| PathBuf::from(&target)) {
            self.watch_dir(&p);
        }
        let new_file = self.current.as_deref() != Some(target.as_str());
        if new_file {
            // Never flash the previous file's last frame.
            if let Some(s) = &self.surface {
                if let Ok(mut s) = s.lock() {
                    s.invalidate();
                }
            }
            self.region = None;
            self.selecting = false;
            let _ = self.player.set("ab-loop-a", "no");
            let _ = self.player.set("ab-loop-b", "no");
            let _ = self.player.set("video-rotate", "0");
        }
        let opts = match start {
            Some((t, paused)) => format!("start={t:.3},pause={}", if paused { "yes" } else { "no" }),
            None => {
                let _ = self.player.set_flag("pause", false);
                String::new()
            }
        };
        let res = if opts.is_empty() {
            self.player.cmd(&["loadfile", &target, "replace"])
        } else {
            self.player.cmd(&["loadfile", &target, "replace", "-1", &opts])
        };
        self.current = Some(target);
        self.was_eof = false;
        match res {
            Ok(()) => {
                self.error = None;
                self.loading = true;
            }
            Err(e) => self.error = Some(e),
        }
        self.ctx.request_repaint();
    }

    pub fn step(&mut self, delta: isize) -> bool {
        if self.list_dirty {
            if let Some(cur) = self.current_path() {
                self.files = crate::media::siblings(&cur);
                self.index = self.files.iter().position(|p| p == &cur).unwrap_or(0);
            }
            self.list_dirty = false;
        }
        let n = self.files.len();
        if n < 2 {
            return false;
        }
        self.index = (self.index as isize + delta).rem_euclid(n as isize) as usize;
        let p = self.files[self.index].clone();
        self.play(p.display().to_string(), None);
        true
    }

    pub fn goto_index(&mut self, i: usize) {
        if i < self.files.len() {
            self.index = i;
            let p = self.files[i].clone();
            self.play(p.display().to_string(), None);
        }
    }

    /// Reload the current file in place, keeping position and pause state
    /// (used when the file changes on disk, e.g. Claude re-encoded it).
    pub fn reload(&mut self) {
        let Some(cur) = self.current.clone() else { return };
        let pb = &self.player.pb;
        let t = pb.time_pos.unwrap_or(0.0);
        let paused = pb.pause;
        self.play(cur, Some((t, paused)));
    }

    pub fn stop(&mut self) {
        self.save_position(false);
        let _ = self.player.cmd(&["stop"]);
        self.current = None;
        self.error = None;
        self.loading = false;
        self.region = None;
        self.selecting = false;
        self._watcher = None;
        self.watched_dir = None;
    }

    fn watch_dir(&mut self, path: &Path) {
        let Some(dir) = path.parent().map(Path::to_path_buf) else { return };
        if self.watched_dir.as_ref() == Some(&dir) {
            return;
        }
        use notify::Watcher;
        let tx = self.watch_tx.clone();
        let ctx = self.ctx.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                for p in ev.paths {
                    let _ = tx.send(p);
                }
                ctx.request_repaint();
            }
        });
        if let Ok(mut w) = watcher {
            if w.watch(&dir, notify::RecursiveMode::NonRecursive).is_ok() {
                self._watcher = Some(w);
                self.watched_dir = Some(dir);
            }
        }
    }

    fn poll_watcher(&mut self, now: f64) {
        let cur = self.current_path();
        while let Ok(p) = self.watch_rx.try_recv() {
            if cur.as_deref() == Some(p.as_path()) {
                // Encoders write for a while; reload once the file has been quiet for a moment.
                self.reload_at = Some(now + 0.8);
            } else {
                self.list_dirty = true;
            }
        }
        match self.reload_at {
            Some(t) if now >= t => {
                self.reload_at = None;
                if cur.as_ref().is_some_and(|p| p.exists()) {
                    self.reload();
                    self.toast("Reloaded (file changed on disk)");
                }
            }
            Some(t) => self.ctx.request_repaint_after(std::time::Duration::from_secs_f64(t - now)),
            None => {}
        }
    }

    // ------------------------------------------------------------- per frame

    fn poll(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.last_time = now;
        let ch = self.player.poll();
        if ch.file_loaded {
            self.loading = false;
            self.scrub = None;
            self.error = None;
        }
        if let Some(e) = ch.command_errors.into_iter().last() {
            self.toast(e);
        }
        if let Some(e) = ch.error {
            self.loading = false;
            self.error = Some(e);
        }
        let eof = self.player.pb.eof;
        if eof && !self.was_eof && !self.loading {
            self.on_eof();
        }
        self.was_eof = eof;
        self.poll_watcher(now);

        if let Some((_, t)) = &self.toast {
            if now - t > 1.8 {
                self.toast = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(60));
            }
        }
        // Keep the display awake while a video is actually playing.
        let playing = self.current.is_some() && !self.player.pb.pause && !eof && self.player.pb.has_video();
        if playing != self.awake {
            self.awake = playing;
            crate::win::keep_awake(playing);
        }
        // While playing, the clock needs a repaint even if the video itself isn't redrawn (audio files).
        if self.current.is_some() && !self.player.pb.pause {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn on_eof(&mut self) {
        let _ = self.player.cmd(&["delete-watch-later-config"]);
        if self.settings.autonext && !self.player.pb.loop_file && self.index + 1 < self.files.len() {
            self.step(1);
        }
    }

    fn handle_ipc(&mut self, ctx: &egui::Context) {
        while let Ok(inc) = self.ipc_rx.try_recv() {
            let argv = inc.req.argv.clone();
            let cwd = inc.req.cwd.clone();
            if argv.first().map(String::as_str) == Some("snap") && argv.iter().any(|a| a == "--ui") {
                match commands::parse_snap(&argv[1..], cwd.as_deref()) {
                    Ok((path, _)) => {
                        if self.hidden {
                            self.show_window(ctx);
                        }
                        self.snaps.push(SnapReq { path, reply: inc.reply });
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        let _ = inc.reply.send(Response::err(e));
                    }
                }
                continue;
            }
            let raise = matches!(argv.first().map(String::as_str), Some("open" | "focus" | "open-dialog" | "url"));
            let res = self.exec(ctx, &argv, cwd.as_deref());
            if raise && res.is_ok() {
                self.show_window(ctx);
            }
            let _ = inc.reply.send(res.into());
        }
    }

    // ------------------------------------------------------------- keyboard

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.palette.open || self.help || self.confirm_delete.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut cmds: Vec<String> = Vec::new();
        ctx.input_mut(|i| {
            let none = Modifiers::NONE;
            let ctrl = Modifiers::CTRL;
            let shift = Modifiers::SHIFT;
            let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
            let mut key = |m: Modifiers, k: Key, c: &str| {
                if i.consume_key(m, k) {
                    cmds.push(c.to_string());
                }
            };
            key(ctrl, Key::P, "palette");
            key(ctrl, Key::O, "open-dialog");
            key(ctrl, Key::V, "paste");
            key(ctrl_shift, Key::C, "copy path");
            key(ctrl, Key::C, "copy time");
            key(ctrl, Key::W, "hide");
            key(ctrl, Key::R, "reload");
            key(none, Key::F5, "reload");
            key(none, Key::F1, "help");
            key(shift, Key::Questionmark, "help");
            key(none, Key::Questionmark, "help");
            key(none, Key::Space, "play");
            key(none, Key::K, "play");
            key(shift, Key::ArrowLeft, "seek -1");
            key(shift, Key::ArrowRight, "seek +1");
            key(ctrl, Key::ArrowLeft, "seek -60");
            key(ctrl, Key::ArrowRight, "seek +60");
            key(none, Key::ArrowLeft, "seek -5");
            key(none, Key::ArrowRight, "seek +5");
            key(none, Key::J, "seek -10");
            key(none, Key::ArrowUp, "volume +5");
            key(none, Key::ArrowDown, "volume -5");
            key(none, Key::M, "mute");
            key(none, Key::Comma, "frame prev");
            key(none, Key::Period, "frame next");
            key(none, Key::Minus, "speed down");
            key(none, Key::Equals, "speed up");
            key(shift, Key::Equals, "speed up");
            key(none, Key::Plus, "speed up");
            key(none, Key::Backspace, "speed 1");
            key(none, Key::OpenBracket, "clip start");
            key(none, Key::CloseBracket, "clip end");
            key(none, Key::Backslash, "clip off");
            key(none, Key::L, "loop");
            key(none, Key::N, "next");
            key(none, Key::P, "prev");
            key(none, Key::PageDown, "next");
            key(none, Key::PageUp, "prev");
            key(none, Key::Home, "seek 0");
            key(shift, Key::S, "screenshot");
            key(none, Key::S, "sub next");
            key(none, Key::A, "audio next");
            key(none, Key::C, "select");
            key(none, Key::R, "rotate next");
            key(none, Key::I, "info");
            key(none, Key::T, "ontop");
            key(none, Key::F, "fullscreen");
            key(none, Key::F11, "fullscreen");
            key(none, Key::Enter, "fullscreen");
            key(none, Key::Delete, "delete");
            for (n, k) in [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9]
                .into_iter()
                .enumerate()
            {
                key(none, k, &format!("seek {}%", n * 10));
            }
            if i.consume_key(none, Key::Escape) {
                cmds.push("escape".into());
            }
            if i.pointer.button_pressed(egui::PointerButton::Extra1) {
                cmds.push("prev".into());
            }
            if i.pointer.button_pressed(egui::PointerButton::Extra2) {
                cmds.push("next".into());
            }
        });
        for c in cmds {
            let argv = commands::split_args(&c);
            if let Err(e) = self.exec(ctx, &argv, None) {
                self.toast(e);
            }
        }
    }

    // ------------------------------------------------------------- snapshot

    fn finish_snaps(&mut self, ctx: &egui::Context) {
        if self.snaps.is_empty() {
            return;
        }
        let snaps = std::mem::take(&mut self.snaps);
        let ctx2 = ctx.clone();
        ctx.request_screenshot(move |img: Arc<egui::ColorImage>| {
            let [w, h] = img.size;
            let raw: Vec<u8> = img.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b(), 255]).collect();
            let res = image::RgbaImage::from_raw(w as u32, h as u32, raw).ok_or_else(|| "bad screenshot buffer".to_string());
            for s in snaps {
                let r = res.as_ref().map_err(Clone::clone).and_then(|buf| {
                    if let Some(dir) = s.path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    buf.save(&s.path).map_err(|e| format!("couldn't write {}: {e}", s.path.display()))?;
                    Ok(serde_json::json!({ "path": s.path, "width": w, "height": h }))
                });
                let _ = s.reply.send(r.into());
            }
            ctx2.request_repaint();
        });
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let t = match &self.current {
            Some(c) => format!("{}  —  Flick", self.display_name(c)),
            None => "Flick".into(),
        };
        if t != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }

    /// File name for paths; mpv's media title (e.g. a YouTube title) for URLs.
    pub fn display_name(&self, target: &str) -> String {
        if crate::media::is_url(target) {
            let t = &self.player.pb.title;
            if t.is_empty() || t == target { target.to_string() } else { t.clone() }
        } else {
            Path::new(target).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| target.to_string())
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if dropped.is_empty() {
            return;
        }
        // A subtitle dropped onto a playing video is added to it rather than opened.
        let (subs, media): (Vec<PathBuf>, Vec<PathBuf>) =
            dropped.into_iter().partition(|p| crate::media::is_subtitle_path(p));
        if !media.is_empty() {
            self.open_list(media);
        }
        if self.current.is_some() {
            for s in subs {
                match self.player.cmd(&["sub-add", &s.display().to_string(), "select"]) {
                    Ok(()) => self.toast(format!("Subtitles: {}", s.file_name().unwrap_or_default().to_string_lossy())),
                    Err(e) => self.toast(e),
                }
            }
        }
        self.show_window(ctx);
    }

    pub fn show_window(&mut self, ctx: &egui::Context) {
        self.hide_in = 0;
        if self.hidden {
            self.hidden = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
        if ctx.input(|i| i.viewport().minimized) == Some(true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// Hide to the tray. Playback stops (and its position is saved), so nothing plays unseen.
    pub fn hide_window(&mut self, ctx: &egui::Context) {
        self.hidden = true;
        self.hide_in = 2;
        ctx.request_repaint();
        if ctx.input(|i| i.viewport().fullscreen) == Some(true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        self.stop();
        self.files.clear();
        self.palette.open = false;
        self.help = false;
        self.confirm_delete = None;
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.tray.as_ref().map(|t| &t.rx) else { return };
        let actions: Vec<tray::TrayAction> = rx.try_iter().collect();
        for a in actions {
            match a {
                tray::TrayAction::Show => self.show_window(ctx),
                tray::TrayAction::Open => self.open_dialog(),
                tray::TrayAction::Quit => self.quit = true,
            }
        }
    }

    /// Native file picker on a worker thread so the UI keeps painting.
    pub fn open_dialog(&mut self) {
        let tx = self.dialog_tx.clone();
        let ctx = self.ctx.clone();
        let dir = self.current_path().and_then(|p| p.parent().map(Path::to_path_buf));
        std::thread::spawn(move || {
            let mut exts: Vec<&str> = crate::media::VIDEO_EXTENSIONS.to_vec();
            exts.extend_from_slice(crate::media::AUDIO_EXTENSIONS);
            let mut d = rfd::FileDialog::new()
                .set_title("Open video")
                .add_filter("Videos and audio", &exts)
                .add_filter("All files", &["*"]);
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            if let Some(paths) = d.pick_files() {
                let _ = tx.send(paths);
                ctx.request_repaint();
            }
        });
    }

    /// Where the video actually appears inside `rect` (letterboxed), in points.
    pub fn video_area(&self, rect: Rect) -> Option<Rect> {
        let [dw, dh] = self.player.pb.dsize()?;
        // dw/dh are before rotation; phone videos often carry a 90° rotate flag.
        let pb = &self.player.pb;
        let rot = (pb.rotate + pb.user_rotate).rem_euclid(360) % 180 == 90;
        let (dw, dh) = if rot { (dh as f32, dw as f32) } else { (dw as f32, dh as f32) };
        let s = (rect.width() / dw).min(rect.height() / dh);
        Some(Rect::from_center_size(rect.center(), egui::vec2(dw * s, dh * s)))
    }

    /// The frame size crop coordinates refer to: stored pixels, in display orientation
    /// (ffmpeg auto-rotates before applying filters).
    pub fn crop_space(&self) -> Option<[i64; 2]> {
        let pb = &self.player.pb;
        let [w, h] = pb.size().or(pb.dsize())?;
        let rot = (pb.rotate + pb.user_rotate).rem_euclid(360) % 180 == 90;
        Some(if rot { [h, w] } else { [w, h] })
    }

    fn handle_video_input(&mut self, ctx: &egui::Context, resp: &egui::Response, rect: Rect) {
        if self.selecting {
            if resp.drag_started_by(egui::PointerButton::Primary) {
                // Where the button went down, not where egui decided it's a drag.
                self.drag_from = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            }
            if let (Some(a), Some(b)) = (self.drag_from, resp.interact_pointer_pos()) {
                if resp.dragged() || resp.drag_stopped() {
                    self.region = self.region_from_points(a, b, rect);
                }
            }
            if resp.drag_stopped() {
                self.drag_from = None;
                self.selecting = false;
                if let Some(r) = self.region {
                    self.toast(format!("Region {}×{} at {},{}", r.w, r.h, r.x, r.y));
                }
            }
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            return;
        }
        // A click toggles play right away instead of waiting out the double-click
        // window; a double click undoes that toggle and goes fullscreen.
        if resp.double_clicked() {
            if self.current.is_some() {
                let _ = self.exec(ctx, &["play".into()], None);
            }
            let _ = self.exec(ctx, &["fullscreen".into()], None);
        } else if resp.clicked() && self.current.is_some() {
            let _ = self.exec(ctx, &["play".into()], None);
        }
        if self.current.is_none() && resp.clicked() {
            self.open_dialog();
        }
        // Wheel: volume. Ctrl/Shift+wheel: seek.
        if resp.hovered() {
            let (dy, mods) = ctx.input(|i| {
                let dy: f32 = i
                    .events
                    .iter()
                    .map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => delta.y,
                        _ => 0.0,
                    })
                    .sum();
                (dy, i.modifiers)
            });
            if dy != 0.0 && self.current.is_some() {
                let steps = if dy > 0.0 { 1.0 } else { -1.0 };
                let cmd = if mods.ctrl || mods.shift {
                    format!("seek {:+}", steps * 5.0)
                } else {
                    format!("volume {:+}", steps * 5.0)
                };
                let _ = self.exec(ctx, &commands::split_args(&cmd), None);
            }
        }
    }

    fn region_from_points(&self, a: Pos2, b: Pos2, rect: Rect) -> Option<Region> {
        let area = self.video_area(rect)?;
        let [w, h] = self.crop_space()?;
        let to_px = |p: Pos2| {
            let u = ((p.x - area.left()) / area.width()).clamp(0.0, 1.0);
            let v = ((p.y - area.top()) / area.height()).clamp(0.0, 1.0);
            (u as f64 * w as f64, v as f64 * h as f64)
        };
        let (ax, ay) = to_px(a);
        let (bx, by) = to_px(b);
        // Even sizes and offsets: most encoders (yuv420p) require them.
        let even = |v: f64| ((v / 2.0).round() * 2.0) as i64;
        let (x0, x1) = (even(ax.min(bx)), even(ax.max(bx)));
        let (y0, y1) = (even(ay.min(by)), even(ay.max(by)));
        (x1 - x0 >= 2 && y1 - y0 >= 2).then_some(Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// Region rectangle on screen, for drawing.
    pub fn region_rect(&self, r: Region, rect: Rect) -> Option<Rect> {
        let area = self.video_area(rect)?;
        let [w, h] = self.crop_space()?;
        let sx = area.width() / w as f32;
        let sy = area.height() / h as f32;
        Some(Rect::from_min_size(
            egui::pos2(area.left() + r.x as f32 * sx, area.top() + r.y as f32 * sy),
            egui::vec2(r.w as f32 * sx, r.h as f32 * sy),
        ))
    }

    pub fn is_fullscreen(&self) -> bool {
        self.ctx.input(|i| i.viewport().fullscreen) == Some(true)
    }
}

impl eframe::App for FlickApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.startup.is_empty() {
            for argv in std::mem::take(&mut self.startup) {
                if let Err(e) = self.exec(ctx, &argv, None) {
                    self.toast(e);
                }
            }
        }
        self.handle_ipc(ctx);
        self.handle_tray(ctx);
        while let Ok(paths) = self.dialog_rx.try_recv() {
            if !paths.is_empty() {
                self.open_list(paths);
                self.show_window(ctx);
            }
        }
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && self.settings.background {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_window(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.handle_drops(&ctx);

        let rect = ui.max_rect();
        self.video_rect = rect;
        ui.painter().rect_filled(rect, 0.0, CANVAS);
        let show_video = self.current.is_some() && self.error.is_none() && self.player.pb.has_video();
        if let (Some(s), true) = (&self.surface, show_video) {
            video::paint(ui.painter(), rect, s, CANVAS);
        }
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down()) {
            self.last_activity = self.last_time;
        }
        self.handle_keys(&ctx);
        self.handle_video_input(&ctx, &resp, rect);
        self.draw_region(ui, rect);
        self.draw_empty_or_error(ui, rect);
        self.draw_controls(ui, rect);
        self.draw_overlays(&ctx, rect);
        self.context_menu(&resp);
        self.palette_ui(&ctx);
        self.modals(&ctx);
        self.update_title(&ctx);
        self.finish_snaps(&ctx);
        if self.hide_in > 0 {
            self.hide_in -= 1;
            if self.hide_in == 0 && self.hidden {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.request_repaint();
            }
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.save_position(true);
        self.settings.volume = self.player.pb.volume;
        self.settings.mute = self.player.pb.mute;
        self.save_settings();
        crate::win::keep_awake(false);
        // The renderer must go before mpv, with the GL context still alive.
        if let (Some(s), Some(gl)) = (self.surface.take(), gl) {
            if let Ok(mut s) = s.lock() {
                s.destroy(gl);
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        CANVAS.to_normalized_gamma_f32()
    }
}

pub fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Media files directly inside a folder, naturally sorted.
fn folder_media(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && crate::media::is_media_path(p)).collect())
        .unwrap_or_default();
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    v.sort_by(|a, b| crate::media::natural_cmp(&name(a), &name(b)));
    v
}






