//! The command language shared by `flick ctl`, the command palette and keybindings.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{FlickApp, Region, absolute};
use crate::media::{clock, is_url, parse_time, timestamp};

/// (usage, description) — shown by `flick ctl help` and searched by the palette.
pub const COMMANDS: &[(&str, &str)] = &[
    ("open <file|folder|url>...", "Play files (several = a playlist of just those)"),
    ("next | prev", "Next / previous file in the folder"),
    ("goto <index|file>", "Jump to a playlist entry (0-based index)"),
    ("play [on|off|toggle]", "Play or pause"),
    ("seek <time|+secs|-secs|N%>", "Seek: 83.5, 1:23, 1m23s, +5, -10, 50%"),
    ("frame <next|prev>", "Step one frame (pauses)"),
    ("speed <x|up|down|1>", "Playback speed (0.25 – 4)"),
    ("volume <0-150|+N|-N>", "Volume in percent"),
    ("mute [on|off|toggle]", "Mute"),
    ("loop [on|off|toggle]", "Loop the current file"),
    ("clip <start> <end>", "Mark a clip range (loops between the marks)"),
    ("clip start|end [time]", "Set one mark (default: the current position)"),
    ("clip off", "Clear the clip marks"),
    ("select [x,y,w,h | off]", "Mark a region in video pixels (no args: draw it with the mouse)"),
    ("audio <id|next|off>", "Audio track"),
    ("sub <id|next|off>", "Subtitle track"),
    ("sub-add <file>", "Load a subtitle file"),
    ("rotate <0|90|180|270|next>", "Rotate the picture"),
    ("fullscreen [on|off|toggle]", "Fullscreen"),
    ("ontop [on|off|toggle]", "Keep the window above others"),
    ("info [on|off|toggle]", "Details panel (codec, resolution, bitrate...)"),
    ("autonext [on|off|toggle]", "Play the next file in the folder at the end"),
    ("screenshot [file]", "Save the current frame (default: Pictures\\Flick)"),
    ("copy <time|path|frame|crop|clip>", "Copy to the clipboard"),
    ("reveal", "Show the file in Explorer"),
    ("reload", "Reopen the file at the same position"),
    ("stop", "Stop and close the file"),
    ("state", "Player state as JSON"),
    ("snap <out.png> [--ui] [--subs]", "Save the current video frame (--ui: the whole window)"),
    ("focus", "Bring the window to the front"),
    ("open-dialog", "Show the Open file dialog"),
    ("background <on|off|toggle>", "Keep running in the tray when the window is closed"),
    ("hide", "Hide the window (to the tray in background mode)"),
    ("quit", "Close the player"),
];

const SPEEDS: [f64; 11] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0];

fn toggle(arg: Option<&str>, cur: bool) -> Result<bool, String> {
    match arg {
        None | Some("toggle") => Ok(!cur),
        Some("on" | "true" | "1" | "yes") => Ok(true),
        Some("off" | "false" | "0" | "no") => Ok(false),
        Some(o) => Err(format!("expected on/off/toggle, got '{o}'")),
    }
}

fn resolve(p: &str, cwd: Option<&Path>) -> PathBuf {
    let p = PathBuf::from(p);
    match cwd {
        Some(c) if p.is_relative() => c.join(p),
        _ => absolute(&p),
    }
}

/// `snap [path] [--ui] [--subs]` → (path, ui, subs)
pub fn parse_snap(args: &[String], cwd: Option<&Path>) -> Result<(PathBuf, (bool, bool)), String> {
    let mut path = None;
    let (mut ui, mut subs) = (false, false);
    for a in args {
        match a.as_str() {
            "--ui" => ui = true,
            "--subs" => subs = true,
            p => path = Some(resolve(p, cwd)),
        }
    }
    let path = path.unwrap_or_else(|| std::env::temp_dir().join("flick").join("snap.png"));
    Ok((path, (ui, subs)))
}

/// Split palette input like a shell: `open "C:\my file.mp4"`.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn parse_region(s: &str) -> Option<Region> {
    let v: Vec<i64> = s.split([',', ':', ' ']).filter(|p| !p.is_empty()).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match v[..] {
        [x, y, w, h] if w > 0 && h > 0 && x >= 0 && y >= 0 => Some(Region { x, y, w, h }),
        _ => None,
    }
}

impl FlickApp {
    fn need_file(&self) -> Result<(), String> {
        if self.current.is_some() { Ok(()) } else { Err("nothing is playing".into()) }
    }

    fn time_arg(&self, s: Option<&str>) -> Result<f64, String> {
        match s {
            None => self.player.pb.time_pos.ok_or_else(|| "no position yet".to_string()),
            Some(s) => parse_time(s).ok_or_else(|| format!("bad time '{s}' (try 83.5, 1:23 or 1m23s)")),
        }
    }

    fn set_clip(&mut self, a: Option<f64>, b: Option<f64>) -> Result<(), String> {
        let fmt = |v: Option<f64>| v.map_or("no".to_string(), |t| format!("{t:.3}"));
        self.player.set("ab-loop-a", &fmt(a))?;
        self.player.set("ab-loop-b", &fmt(b))?;
        self.player.pb.ab = [a, b];
        Ok(())
    }

    fn cycle_track(&mut self, kind: &str, prop: &str, arg: Option<&str>) -> Result<Value, String> {
        self.need_file()?;
        let tracks = self.player.tracks_of(kind);
        let target = match arg {
            None | Some("next") => {
                // Cycle through tracks, with "off" in the loop for subtitles.
                let cur = tracks.iter().position(|t| t.selected);
                match (cur, kind) {
                    (Some(i), _) if i + 1 < tracks.len() => tracks[i + 1].id.to_string(),
                    (Some(_), "sub") => "no".into(),
                    (None, _) if !tracks.is_empty() => tracks[0].id.to_string(),
                    (_, _) if !tracks.is_empty() => tracks[0].id.to_string(),
                    _ => return Err(format!("no {} tracks", if kind == "sub" { "subtitle" } else { kind })),
                }
            }
            Some("off" | "no") => "no".into(),
            Some(id) => id.to_string(),
        };
        self.player.set(prop, &target)?;
        let label = tracks.iter().find(|t| t.id.to_string() == target).map(|t| t.label.clone()).unwrap_or_else(|| "off".into());
        let what = if kind == "sub" { "Subtitles" } else { "Audio" };
        self.toast(format!("{what}: {label}"));
        Ok(json!({ "track": target, "label": label }))
    }

    pub fn exec(&mut self, ctx: &egui::Context, argv: &[String], cwd: Option<&Path>) -> Result<Value, String> {
        let Some(cmd) = argv.first() else { return Err("empty command".into()) };
        let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        let a0 = args.first().copied();
        let ok = || Ok(json!(true));
        match cmd.as_str() {
            "ping" => Ok(json!({ "pid": std::process::id() })),
            "help" if cwd.is_none() => {
                self.help = true;
                ok()
            }
            "help" => Ok(json!(COMMANDS.iter().map(|(u, d)| json!({"usage": u, "description": d})).collect::<Vec<_>>())),
            "palette" => {
                self.palette = super::Palette { open: true, just_opened: true, ..Default::default() };
                ok()
            }
            "open" | "url" => {
                if args.is_empty() {
                    return Err("open: give a file, folder or URL".into());
                }
                if args.len() == 1 && is_url(args[0]) {
                    self.open_url(args[0]);
                } else {
                    let paths: Vec<PathBuf> = args.iter().map(|a| resolve(a, cwd)).collect();
                    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
                        return Err(format!("not found: {}", missing.display()));
                    }
                    self.open_list(paths);
                }
                Ok(json!({ "file": self.current }))
            }
            "open-dialog" => {
                self.open_dialog();
                ok()
            }
            "paste" => {
                let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
                if let Ok(files) = cb.get().file_list() {
                    if !files.is_empty() {
                        self.open_list(files);
                        return ok();
                    }
                }
                let text = cb.get_text().map_err(|_| "the clipboard has no file or link".to_string())?;
                let t = text.trim().trim_matches('"');
                if is_url(t) {
                    self.open_url(t);
                } else if Path::new(t).exists() {
                    self.open(Path::new(t));
                } else {
                    return Err("the clipboard has no file or link".into());
                }
                ok()
            }
            "next" => {
                if !self.step(1) {
                    self.toast("No other videos in this folder");
                }
                ok()
            }
            "prev" => {
                // Like most players: restart the current file first if we're past the beginning.
                if self.player.pb.time_pos.is_some_and(|t| t > 4.0) && cwd.is_none() {
                    self.player.cmd(&["seek", "0", "absolute"])?;
                } else if !self.step(-1) {
                    self.toast("No other videos in this folder");
                }
                ok()
            }
            "goto" => {
                let a = a0.ok_or("goto <index|file>")?;
                if let Ok(i) = a.parse::<usize>() {
                    if i >= self.files.len() {
                        return Err(format!("index {i} out of range (0..{})", self.files.len()));
                    }
                    self.goto_index(i);
                } else {
                    self.open(&resolve(a, cwd));
                }
                ok()
            }
            "play" | "pause" => {
                self.need_file()?;
                let playing = !self.player.pb.pause;
                let want = match (cmd.as_str(), a0) {
                    ("pause", None) => false,
                    (_, a) => toggle(a, playing)?,
                };
                // At the end, "play" starts over.
                if want && self.player.pb.eof {
                    self.player.cmd(&["seek", "0", "absolute"])?;
                }
                self.player.set_flag("pause", !want)?;
                self.player.pb.pause = !want;
                Ok(json!({ "playing": want }))
            }
            "seek" => {
                self.need_file()?;
                let a = a0.ok_or("seek <time|+secs|-secs|N%>")?;
                if let Some(p) = a.strip_suffix('%') {
                    let p: f64 = p.parse().map_err(|_| format!("bad percent '{a}'"))?;
                    self.player.cmd(&["seek", &p.clamp(0.0, 100.0).to_string(), "absolute-percent+exact"])?;
                } else if a.starts_with('+') || a.starts_with('-') {
                    let d = parse_time(&a[1..]).ok_or_else(|| format!("bad offset '{a}'"))?;
                    let d = if a.starts_with('-') { -d } else { d };
                    self.player.cmd(&["seek", &d.to_string(), "relative+exact"])?;
                    if cwd.is_none() {
                        let t = (self.player.pb.time_pos.unwrap_or(0.0) + d).max(0.0);
                        self.toast(format!("{}{}s   {}", if d < 0.0 { "−" } else { "+" }, d.abs(), clock(t)));
                    }
                } else {
                    let t = parse_time(a).ok_or_else(|| format!("bad time '{a}'"))?;
                    self.player.cmd(&["seek", &t.to_string(), "absolute+exact"])?;
                }
                ok()
            }
            "frame" => {
                self.need_file()?;
                match a0 {
                    None | Some("next") => self.player.cmd(&["frame-step"])?,
                    Some("prev") => self.player.cmd(&["frame-back-step"])?,
                    Some(o) => return Err(format!("frame next|prev, got '{o}'")),
                }
                ok()
            }
            "speed" => {
                let cur = self.player.pb.speed;
                let v = match a0.ok_or("speed <x|up|down|1>")? {
                    "up" => SPEEDS.iter().copied().find(|s| *s > cur + 1e-6).unwrap_or(4.0),
                    "down" => SPEEDS.iter().rev().copied().find(|s| *s < cur - 1e-6).unwrap_or(0.25),
                    "reset" => 1.0,
                    s => s.trim_end_matches(['x', '×']).parse::<f64>().map_err(|_| format!("bad speed '{s}'"))?,
                };
                let v = v.clamp(0.1, 8.0);
                self.player.set_double("speed", v)?;
                self.player.pb.speed = v;
                if cwd.is_none() {
                    self.toast(format!("Speed {}×", trim_float(v)));
                }
                Ok(json!({ "speed": v }))
            }
            "volume" => {
                let cur = self.player.pb.volume;
                let a = a0.ok_or("volume <0-150|+N|-N>")?;
                let v = if a.starts_with('+') || a.starts_with('-') {
                    cur + a.parse::<f64>().map_err(|_| format!("bad volume '{a}'"))?
                } else {
                    a.trim_end_matches('%').parse::<f64>().map_err(|_| format!("bad volume '{a}'"))?
                };
                let v = v.clamp(0.0, 150.0).round();
                self.player.set_double("volume", v)?;
                if self.player.pb.mute && v > cur {
                    self.player.set_flag("mute", false)?;
                }
                self.player.pb.volume = v;
                self.settings.volume = v;
                if cwd.is_none() {
                    self.toast(format!("Volume {v:.0}%"));
                }
                Ok(json!({ "volume": v }))
            }
            "mute" => {
                let m = toggle(a0, self.player.pb.mute)?;
                self.player.set_flag("mute", m)?;
                self.player.pb.mute = m;
                self.settings.mute = m;
                self.toast(if m { "Muted" } else { "Unmuted" });
                Ok(json!({ "muted": m }))
            }
            "loop" => {
                let l = toggle(a0, self.player.pb.loop_file)?;
                self.player.set("loop-file", if l { "inf" } else { "no" })?;
                self.player.pb.loop_file = l;
                self.toast(if l { "Loop on" } else { "Loop off" });
                Ok(json!({ "loop": l }))
            }
            "clip" => {
                self.need_file()?;
                let [a, b] = self.player.pb.ab;
                let (na, nb) = match a0 {
                    Some("off" | "clear") => (None, None),
                    Some("start" | "a" | "in") => (Some(self.time_arg(args.get(1).copied())?), b),
                    Some("end" | "b" | "out") => (a, Some(self.time_arg(args.get(1).copied())?)),
                    Some(s) => (Some(self.time_arg(Some(s))?), Some(self.time_arg(Some(args.get(1).ok_or("clip <start> <end>")?))?)),
                    None => return Err("clip <start> <end> | start [t] | end [t] | off".into()),
                };
                // Keep the marks ordered: setting an end before the start moves the start.
                let (na, nb) = match (na, nb) {
                    (Some(x), Some(y)) if y < x => (Some(y), Some(x)),
                    v => v,
                };
                self.set_clip(na, nb)?;
                if cwd.is_none() {
                    self.toast(match (na, nb) {
                        (None, None) => "Clip marks cleared".to_string(),
                        (Some(x), None) => format!("Clip start {}", timestamp(x)),
                        (None, Some(y)) => format!("Clip end {}", timestamp(y)),
                        (Some(x), Some(y)) => format!("Clip {} → {}  ({:.1}s, looping)", clock(x), clock(y), y - x),
                    });
                }
                Ok(self.clip_json())
            }
            "select" => {
                match a0 {
                    None => {
                        self.need_file()?;
                        if !self.player.pb.has_video() {
                            return Err("no video to select on".into());
                        }
                        self.selecting = !self.selecting;
                        if self.selecting {
                            self.toast("Drag over the video to mark a region");
                        }
                    }
                    Some("off" | "clear") => {
                        self.region = None;
                        self.selecting = false;
                    }
                    Some(_) => {
                        let r = parse_region(&args.join(",")).ok_or("select x,y,w,h (video pixels)")?;
                        self.region = Some(r);
                    }
                }
                Ok(self.region.map_or(Value::Null, |r| json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h, "ffmpeg": r.ffmpeg() })))
            }
            "audio" => self.cycle_track("audio", "aid", a0),
            "sub" | "subs" => self.cycle_track("sub", "sid", a0),
            "sub-add" => {
                self.need_file()?;
                let p = resolve(a0.ok_or("sub-add <file>")?, cwd);
                self.player.cmd(&["sub-add", &p.display().to_string(), "select"])?;
                ok()
            }
            "rotate" => {
                self.need_file()?;
                let cur = self.player.pb.user_rotate;
                let v = match a0.ok_or("rotate <0|90|180|270|next>")? {
                    "next" => (cur + 90).rem_euclid(360),
                    s => s.parse::<i64>().map_err(|_| format!("bad angle '{s}'"))?.rem_euclid(360),
                };
                if v % 90 != 0 {
                    return Err("rotate by multiples of 90".into());
                }
                self.player.set("video-rotate", &v.to_string())?;
                self.player.pb.user_rotate = v;
                self.region = None;
                self.toast(format!("Rotation {v}°"));
                ok()
            }
            "fullscreen" => {
                let fs = toggle(a0, self.is_fullscreen())?;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fs));
                ok()
            }
            "ontop" => {
                self.ontop = toggle(a0, self.ontop)?;
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.ontop {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                }));
                self.toast(if self.ontop { "Always on top" } else { "Normal window" });
                ok()
            }
            "info" => {
                self.settings.info = toggle(a0, self.settings.info)?;
                self.save_settings();
                ok()
            }
            "autonext" => {
                self.settings.autonext = toggle(a0, self.settings.autonext)?;
                self.save_settings();
                self.toast(if self.settings.autonext { "Auto-play next: on" } else { "Auto-play next: off" });
                ok()
            }
            "background" => {
                self.settings.background = toggle(a0, self.settings.background)?;
                self.save_settings();
                Ok(json!({ "background": self.settings.background }))
            }
            "screenshot" => {
                self.need_file()?;
                if let Some(p) = a0 {
                    let p = resolve(p, cwd);
                    self.player.cmd_sync(&["screenshot-to-file", &p.display().to_string(), "subtitles"])?;
                    self.toast(format!("Saved {}", p.display()));
                    return Ok(json!({ "path": p }));
                }
                let dir = super::player::pictures().join("Flick");
                let _ = std::fs::create_dir_all(&dir);
                self.player.cmd(&["screenshot", "subtitles"])?;
                self.toast(format!("Screenshot saved to {}", dir.display()));
                Ok(json!({ "dir": dir }))
            }
            "snap" => {
                self.need_file()?;
                if !self.player.pb.has_video() {
                    return Err("no video frame to capture (audio only?)".into());
                }
                let (path, (_, subs)) = parse_snap(&argv[1..], cwd)?;
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                self.player.cmd_sync(&["screenshot-to-file", &path.display().to_string(), if subs { "subtitles" } else { "video" }])?;
                let pb = &self.player.pb;
                Ok(json!({
                    "path": path,
                    "position": pb.time_pos,
                    "timestamp": pb.time_pos.map(timestamp),
                    "size": self.crop_space(),
                }))
            }
            "copy" => {
                let text = match a0.unwrap_or("time") {
                    "time" | "timestamp" => timestamp(self.player.pb.time_pos.ok_or("nothing is playing")?),
                    "path" => self.current.clone().ok_or("nothing is playing")?,
                    "crop" | "region" => self.region.map(|r| r.ffmpeg()).ok_or("no region marked (press C and drag)")?,
                    "clip" => match self.player.pb.ab {
                        [Some(a), Some(b)] => format!("-ss {} -to {}", timestamp(a), timestamp(b)),
                        _ => return Err("no clip marked (use [ and ])".into()),
                    },
                    "frame" => {
                        self.need_file()?;
                        let tmp = std::env::temp_dir().join("flick").join("clipboard.png");
                        let _ = std::fs::create_dir_all(tmp.parent().unwrap());
                        self.player.cmd_sync(&["screenshot-to-file", &tmp.display().to_string(), "subtitles"])?;
                        let img = image::open(&tmp).map_err(|e| e.to_string())?.to_rgba8();
                        let (w, h) = img.dimensions();
                        arboard::Clipboard::new()
                            .and_then(|mut c| {
                                c.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: img.into_raw().into() })
                            })
                            .map_err(|e| e.to_string())?;
                        self.toast("Copied frame");
                        return ok();
                    }
                    o => return Err(format!("copy time|path|frame|crop|clip, got '{o}'")),
                };
                arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())).map_err(|e| e.to_string())?;
                self.toast(format!("Copied {text}"));
                Ok(json!({ "copied": text }))
            }
            "reveal" => {
                let p = self.current_path().ok_or("nothing is playing")?;
                crate::win::reveal_in_explorer(&p);
                ok()
            }
            "reload" => {
                self.need_file()?;
                self.reload();
                ok()
            }
            "stop" => {
                self.stop();
                ok()
            }
            "delete" => {
                self.confirm_delete = Some(self.current_path().ok_or("nothing is playing")?);
                ok()
            }
            "escape" => {
                if self.selecting {
                    self.selecting = false;
                } else if self.region.is_some() {
                    self.region = None;
                } else if self.settings.info {
                    self.settings.info = false;
                } else if self.is_fullscreen() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
                ok()
            }
            "state" => Ok(self.state_json(ctx)),
            "focus" => ok(),
            "hide" => {
                if self.settings.background {
                    self.hide_window(ctx);
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                ok()
            }
            "quit" => {
                self.quit = true;
                ctx.request_repaint();
                ok()
            }
            other => Err(format!("unknown command '{other}' (try `flick ctl help`)")),
        }
    }

    fn clip_json(&self) -> Value {
        let [a, b] = self.player.pb.ab;
        if a.is_none() && b.is_none() {
            return Value::Null;
        }
        json!({
            "start": a,
            "end": b,
            "start_ts": a.map(timestamp),
            "end_ts": b.map(timestamp),
            "ffmpeg": match (a, b) {
                (Some(a), Some(b)) => Some(format!("-ss {} -to {}", timestamp(a), timestamp(b))),
                _ => None,
            },
        })
    }

    pub fn state_json(&self, ctx: &egui::Context) -> Value {
        let pb = &self.player.pb;
        let p = &self.player;
        let tracks = |kind: &str| serde_json::to_value(p.tracks_of(kind)).unwrap_or_default();
        let video = pb.has_video().then(|| {
            json!({
                "width": pb.w,
                "height": pb.h,
                "display_width": pb.dw,
                "display_height": pb.dh,
                "rotate": pb.rotate,
                "user_rotate": pb.user_rotate,
                "codec": p.str("video-format"),
                "fps": p.num("container-fps"),
                "hwdec": p.str("hwdec-current"),
                "bitrate": p.num("video-bitrate"),
            })
        });
        let (w, h) = ctx.input(|i| {
            let r = i.viewport().inner_rect.unwrap_or(egui::Rect::ZERO);
            (r.width(), r.height())
        });
        json!({
            "file": self.current,
            "title": self.current.as_deref().map(|c| self.display_name(c)),
            "loading": self.loading,
            "error": self.error,
            "playing": self.current.is_some() && !pb.pause && !pb.eof,
            "paused": pb.pause,
            "eof": pb.eof,
            "position": pb.time_pos,
            "position_ts": pb.time_pos.map(timestamp),
            "duration": pb.duration,
            "duration_ts": pb.duration.map(timestamp),
            "speed": pb.speed,
            "volume": pb.volume,
            "muted": pb.mute,
            "loop": pb.loop_file,
            "clip": self.clip_json(),
            "region": self.region.map(|r| json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h, "ffmpeg": r.ffmpeg() })),
            "video": video,
            "audio": p.str("audio-codec-name").map(|c| json!({
                "codec": c,
                "channels": p.audio_param("channel-count"),
                "samplerate": p.audio_param("samplerate"),
            })),
            "tracks": { "audio": tracks("audio"), "sub": tracks("sub") },
            "chapters": pb.chapters,
            "playlist": {
                "index": self.index,
                "count": self.files.len(),
                "next": self.files.get(self.index + 1),
            },
            "fullscreen": self.is_fullscreen(),
            "ontop": self.ontop,
            "window": [w, h],
            "visible": !self.hidden,
            "autonext": self.settings.autonext,
        })
    }
}

pub fn trim_float(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(split_args(r#"open "C:\my file.mp4" x"#), vec!["open", r"C:\my file.mp4", "x"]);
        assert_eq!(split_args("  seek   +5 "), vec!["seek", "+5"]);
    }

    #[test]
    fn regions() {
        assert_eq!(parse_region("10,20,300,200"), Some(Region { x: 10, y: 20, w: 300, h: 200 }));
        assert_eq!(parse_region("10:20:300:200"), Some(Region { x: 10, y: 20, w: 300, h: 200 }));
        assert_eq!(parse_region("1,2,0,4"), None);
        assert_eq!(parse_region("1,2,3"), None);
        assert_eq!(Region { x: 10, y: 20, w: 300, h: 200 }.ffmpeg(), "crop=300:200:10:20");
    }

    #[test]
    fn floats() {
        assert_eq!(trim_float(1.0), "1");
        assert_eq!(trim_float(1.25), "1.25");
        assert_eq!(trim_float(0.5), "0.5");
    }
}


