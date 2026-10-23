//! Playback state mirrored from mpv through property observers, plus small helpers
//! around the mpv handle.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::mpv::{self, Event, Mpv, PropValue};

/// Observed properties: (id, name, format). The id indexes `Playback::apply`.
const OBSERVED: &[(u64, &str, i32)] = &[
    (1, "time-pos", mpv::FORMAT_DOUBLE),
    (2, "duration", mpv::FORMAT_DOUBLE),
    (3, "pause", mpv::FORMAT_FLAG),
    (4, "volume", mpv::FORMAT_DOUBLE),
    (5, "mute", mpv::FORMAT_FLAG),
    (6, "speed", mpv::FORMAT_DOUBLE),
    (7, "eof-reached", mpv::FORMAT_FLAG),
    (8, "path", mpv::FORMAT_STRING),
    (9, "media-title", mpv::FORMAT_STRING),
    (10, "video-params/dw", mpv::FORMAT_INT64),
    (11, "video-params/dh", mpv::FORMAT_INT64),
    (12, "video-params/w", mpv::FORMAT_INT64),
    (13, "video-params/h", mpv::FORMAT_INT64),
    (14, "video-params/rotate", mpv::FORMAT_INT64),
    (15, "ab-loop-a", mpv::FORMAT_STRING),
    (16, "ab-loop-b", mpv::FORMAT_STRING),
    (17, "loop-file", mpv::FORMAT_STRING),
    (18, "paused-for-cache", mpv::FORMAT_FLAG),
    (19, "seeking", mpv::FORMAT_FLAG),
    (20, "demuxer-cache-time", mpv::FORMAT_DOUBLE),
    (21, "sid", mpv::FORMAT_STRING),
    (22, "aid", mpv::FORMAT_STRING),
    (23, "video-rotate", mpv::FORMAT_INT64),
    (24, "chapter-list/count", mpv::FORMAT_INT64),
    (25, "vid", mpv::FORMAT_STRING),
];

#[derive(Default, Clone, Debug)]
pub struct Playback {
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub pause: bool,
    pub volume: f64,
    pub mute: bool,
    pub speed: f64,
    pub eof: bool,
    pub path: Option<String>,
    pub title: String,
    /// Display size (aspect-corrected, rotated): video-params/dw, dh.
    pub dw: Option<i64>,
    pub dh: Option<i64>,
    /// Stored frame size, before aspect correction: video-params/w, h.
    pub w: Option<i64>,
    pub h: Option<i64>,
    pub rotate: i64,
    pub user_rotate: i64,
    pub ab: [Option<f64>; 2],
    pub loop_file: bool,
    pub buffering: bool,
    pub seeking: bool,
    pub cache_time: Option<f64>,
    pub sid: String,
    pub aid: String,
    pub vid: String,
    pub chapters: i64,
}

impl Playback {
    fn apply(&mut self, id: u64, v: PropValue) {
        let f = |v: &PropValue| match v {
            PropValue::Double(d) => Some(*d),
            PropValue::Int(i) => Some(*i as f64),
            _ => None,
        };
        let b = |v: &PropValue| matches!(v, PropValue::Flag(true));
        let s = |v: &PropValue| match v {
            PropValue::Str(s) => s.clone(),
            _ => String::new(),
        };
        let i = |v: &PropValue| match v {
            PropValue::Int(i) => Some(*i),
            _ => None,
        };
        // "no" / absent for an unset A-B point.
        let t = |v: &PropValue| s(v).parse::<f64>().ok();
        match id {
            1 => self.time_pos = f(&v),
            2 => self.duration = f(&v).filter(|d| *d > 0.0),
            3 => self.pause = b(&v),
            4 => self.volume = f(&v).unwrap_or(self.volume),
            5 => self.mute = b(&v),
            6 => self.speed = f(&v).unwrap_or(1.0),
            7 => self.eof = b(&v),
            8 => self.path = Some(s(&v)).filter(|p| !p.is_empty()),
            9 => self.title = s(&v),
            10 => self.dw = i(&v),
            11 => self.dh = i(&v),
            12 => self.w = i(&v),
            13 => self.h = i(&v),
            14 => self.rotate = i(&v).unwrap_or(0),
            15 => self.ab[0] = t(&v),
            16 => self.ab[1] = t(&v),
            17 => self.loop_file = !matches!(s(&v).as_str(), "no" | ""),
            18 => self.buffering = b(&v),
            19 => self.seeking = b(&v),
            20 => self.cache_time = f(&v),
            21 => self.sid = s(&v),
            22 => self.aid = s(&v),
            23 => self.user_rotate = i(&v).unwrap_or(0),
            24 => self.chapters = i(&v).unwrap_or(0),
            25 => self.vid = s(&v),
            _ => {}
        }
    }

    pub fn dsize(&self) -> Option<[i64; 2]> {
        self.dw.zip(self.dh).map(|(w, h)| [w, h]).filter(|d| d[0] > 0 && d[1] > 0)
    }

    pub fn size(&self) -> Option<[i64; 2]> {
        self.w.zip(self.h).map(|(w, h)| [w, h]).filter(|d| d[0] > 0 && d[1] > 0)
    }

    pub fn has_video(&self) -> bool {
        self.dsize().is_some() && !matches!(self.vid.as_str(), "no" | "")
    }

}

/// What happened since the last poll that the app cares about.
#[derive(Default)]
pub struct Changes {
    pub file_loaded: bool,
    pub ended_eof: bool,
    pub error: Option<String>,
    pub command_errors: Vec<String>,
    pub any: bool,
}

/// Descriptive properties, observed as strings and cached. The UI reads these every
/// frame, so they must never be fetched synchronously: a sync read waits for mpv's
/// core, which stalls for hundreds of ms while a file opens (decoder and hwdec init).
const INFO: &[&str] = &[
    "track-list",
    "video-format",
    "container-fps",
    "hwdec-current",
    "audio-codec-name",
    "audio-params",
    "file-size",
    "file-format",
    "video-bitrate",
    "audio-bitrate",
    "frame-drop-count",
    "video-params/pixelformat",
    "video-params/hw-pixelformat",
    "video-params/gamma",
];
const INFO_ID: u64 = 1000;
/// Reply id for async commands whose failure should be shown to the user.
const USER_CMD: u64 = 1;

pub struct Player {
    pub mpv: Arc<Mpv>,
    pub pb: Playback,
    info: HashMap<&'static str, String>,
    tracks: Vec<Value>,
}

impl Player {
    pub fn new(mpv: Arc<Mpv>) -> Self {
        for (id, name, fmt) in OBSERVED {
            mpv.observe(*id, name, *fmt);
        }
        for (i, name) in INFO.iter().enumerate() {
            mpv.observe(INFO_ID + i as u64, name, mpv::FORMAT_STRING);
        }
        Self {
            mpv,
            pb: Playback { speed: 1.0, volume: 100.0, pause: true, ..Default::default() },
            info: HashMap::new(),
            tracks: Vec::new(),
        }
    }

    pub fn poll(&mut self) -> Changes {
        let mut ch = Changes::default();
        while let Some(ev) = self.mpv.next_event() {
            ch.any = true;
            match ev {
                Event::Property { id, value } if id >= INFO_ID => {
                    let Some(name) = INFO.get((id - INFO_ID) as usize) else { continue };
                    let s = match value {
                        PropValue::Str(s) => s,
                        _ => String::new(),
                    };
                    if *name == "track-list" {
                        self.tracks = match serde_json::from_str(&s) {
                            Ok(Value::Array(a)) => a,
                            _ => Vec::new(),
                        };
                    }
                    if s.is_empty() {
                        self.info.remove(name);
                    } else {
                        self.info.insert(name, s);
                    }
                }
                Event::Property { id, value } => self.pb.apply(id, value),
                Event::FileLoaded => {
                    ch.file_loaded = true;
                    ch.error = None;
                }
                Event::EndFile { eof, error } => {
                    if eof {
                        ch.ended_eof = true;
                    }
                    if error.is_some() {
                        ch.error = error;
                    }
                }
                Event::CommandReply { id: USER_CMD, error: Some(e) } => ch.command_errors.push(e),
                _ => {}
            }
        }
        ch
    }

    /// Queue a command. Failures come back through `Changes::command_errors`.
    pub fn cmd(&self, args: &[&str]) -> Result<(), String> {
        self.mpv.command_async(USER_CMD, args)
    }

    /// Run a command and wait for it: only for things whose result the caller needs
    /// right away (screenshots for `snap`, saving the resume position on exit).
    pub fn cmd_sync(&self, args: &[&str]) -> Result<(), String> {
        self.mpv.command(args)
    }

    pub fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.mpv.set_async(name, value)
    }

    pub fn set_flag(&self, name: &str, value: bool) -> Result<(), String> {
        self.set(name, if value { "yes" } else { "no" })
    }

    pub fn set_double(&self, name: &str, value: f64) -> Result<(), String> {
        self.set(name, &value.to_string())
    }

    /// Tracks of one kind ("video" | "audio" | "sub").
    pub fn tracks_of(&self, kind: &str) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|t| t["type"].as_str() == Some(kind))
            .map(|t| Track {
                id: t["id"].as_i64().unwrap_or(0),
                label: track_label(t),
                selected: t["selected"].as_bool().unwrap_or(false),
                external: t["external"].as_bool().unwrap_or(false),
                lang: t["lang"].as_str().map(String::from),
                codec: t["codec"].as_str().map(String::from),
            })
            .collect()
    }

    /// A cached `INFO` property.
    pub fn str(&self, name: &str) -> Option<String> {
        debug_assert!(INFO.contains(&name), "{name} is not observed");
        self.info.get(name).cloned()
    }

    pub fn num(&self, name: &str) -> Option<f64> {
        self.str(name)?.parse().ok()
    }

    /// A field of `audio-params` ("channel-count", "samplerate"...). Its sub-properties
    /// don't send change notifications, so the whole node is observed instead.
    pub fn audio_param(&self, key: &str) -> Option<f64> {
        let v: Value = serde_json::from_str(self.info.get("audio-params")?).ok()?;
        v[key].as_f64()
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Track {
    pub id: i64,
    pub label: String,
    pub selected: bool,
    pub external: bool,
    pub lang: Option<String>,
    pub codec: Option<String>,
}

fn track_label(t: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = t["title"].as_str() {
        parts.push(title.to_string());
    }
    if let Some(lang) = t["lang"].as_str() {
        parts.push(lang.to_uppercase());
    }
    if parts.is_empty() {
        parts.push(format!("Track {}", t["id"].as_i64().unwrap_or(0)));
    }
    if let Some(c) = t["codec"].as_str() {
        parts.push(c.to_string());
    }
    if t["type"].as_str() == Some("audio") {
        if let Some(ch) = t["demux-channel-count"].as_i64() {
            parts.push(match ch {
                1 => "mono".into(),
                2 => "stereo".into(),
                6 => "5.1".into(),
                8 => "7.1".into(),
                n => format!("{n} ch"),
            });
        }
    }
    if t["external"].as_bool() == Some(true) {
        parts.push("external".into());
    }
    parts.join(" · ")
}

/// Options set before mpv initializes. Paths go under %LOCALAPPDATA%\Flick.
pub fn options(data_dir: &std::path::Path, volume: f64, mute: bool) -> Vec<(String, String)> {
    let watch = data_dir.join("watch_later");
    let shots = pictures().join("Flick");
    let c = super::CANVAS;
    let bg = format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
    let mut o: Vec<(String, String)> = vec![
        ("vo", "libmpv"),
        ("hwdec", "auto-safe"),
        ("keep-open", "yes"),
        ("idle", "yes"),
        ("terminal", "no"),
        ("input-default-bindings", "no"),
        ("input-vo-keyboard", "no"),
        ("osc", "no"),
        ("osd-level", "0"),
        ("load-scripts", "no"),
        ("volume-max", "150"),
        ("sub-auto", "fuzzy"),
        ("audio-file-auto", "fuzzy"),
        ("resume-playback", "yes"),
        ("resume-playback-check-mtime", "yes"),
        ("watch-later-options", "start,aid,sid,sub-delay,audio-delay,video-rotate"),
        ("screenshot-format", "png"),
        ("screenshot-template", "%F %wH.%wM.%wS.%wT"),
        ("background", "color"),
        ("background-color", bg.as_str()),
        ("cursor-autohide", "no"),
        ("demuxer-max-back-bytes", "50MiB"),
        ("hr-seek", "default"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    // The youtube-dl hook costs ~250 ms on every file change, even for local files,
    // so only load it when there's a yt-dlp to call.
    o.push(("ytdl".into(), if on_path("yt-dlp.exe") || on_path("youtube-dl.exe") { "yes" } else { "no" }.into()));
    o.push(("watch-later-dir".into(), watch.display().to_string()));
    o.push(("screenshot-dir".into(), shots.display().to_string()));
    o.push(("volume".into(), format!("{volume:.0}")));
    o.push(("mute".into(), if mute { "yes" } else { "no" }.into()));
    // FLICK_MPV_LOG=path writes mpv's verbose log there, for debugging playback.
    if let Some(log) = std::env::var_os("FLICK_MPV_LOG") {
        o.push(("log-file".into(), log.to_string_lossy().into_owned()));
        o.push(("msg-level".into(), "all=v".into()));
    }
    o
}

fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

pub fn pictures() -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default();
    home.join("Pictures")
}




