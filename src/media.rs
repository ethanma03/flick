//! What counts as media, folder playlists, and time formatting/parsing.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Every extension Flick registers for. Keep in sync with scripts/install.ps1 and set-default.ps1.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "m2ts", "mts", "ts", "3gp", "ogv",
    "vob", "divx", "asf", "rm", "rmvb", "f4v", "y4m", "gifv",
];
pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "m4a", "aac", "flac", "wav", "ogg", "oga", "opus", "wma", "aiff", "aif", "ape", "wv", "mka", "ac3", "dts"];
/// Sidecar subtitle files: dropping one onto the player adds it to the current video.
pub const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "sup", "idx"];

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn is_media_path(p: &Path) -> bool {
    let e = ext(p);
    VIDEO_EXTENSIONS.contains(&e.as_str()) || AUDIO_EXTENSIONS.contains(&e.as_str())
}

pub fn is_subtitle_path(p: &Path) -> bool {
    SUBTITLE_EXTENSIONS.contains(&ext(p).as_str())
}

pub fn is_url(s: &str) -> bool {
    let s = s.trim();
    ["http://", "https://", "rtmp://", "rtsp://", "udp://", "ytdl://"].iter().any(|p| s.starts_with(p))
}

/// Media files in the same folder as `path`, naturally sorted. `path` itself is
/// always included, even with an unusual extension.
pub fn siblings(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else { return vec![path.to_path_buf()] };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| is_media_path(p) || p == path)
                .collect()
        })
        .unwrap_or_default();
    if !files.iter().any(|p| p == path) {
        files.push(path.to_path_buf());
    }
    files.sort_by(|a, b| natural_cmp(&lower_name(a), &lower_name(b)));
    files
}

fn lower_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// "ep2" < "ep10", case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let db = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (na, nb) = (trim_zeros(&a[..da]), trim_zeros(&b[..db]));
                let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb)).then_with(|| da.cmp(&db));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[da..];
                b = &b[db..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let n = d.iter().take_while(|&&c| c == b'0').count();
    &d[n.min(d.len().saturating_sub(1))..]
}

/// 75.5 → "1:15", 3725 → "1:02:05". Seconds are floored, so the clock never runs ahead.
pub fn clock(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Precise timestamp for tools: 75.5 → "00:01:15.500" (what ffmpeg's -ss/-to accept).
pub fn timestamp(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Parse "90", "1:30", "01:02:03.5", "1m30s", "2.5s", "1h". Returns seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        let mut total = 0.0;
        for part in s.split(':') {
            total = total * 60.0 + part.parse::<f64>().ok().filter(|v| *v >= 0.0)?;
        }
        return Some(total);
    }
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    // Unit suffixes: 1h2m3.5s
    let (mut total, mut num) = (0.0, String::new());
    for c in s.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'h' | 'm' | 's' => {
                let v: f64 = num.parse().ok()?;
                total += v * match c {
                    'h' => 3600.0,
                    'm' => 60.0,
                    _ => 1.0,
                };
                num.clear();
            }
            _ => return None,
        }
    }
    if !num.is_empty() {
        total += num.parse::<f64>().ok()?;
    }
    Some(total)
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["ep10.mkv", "ep2.mkv", "Ep1.mkv", "ep02b.mkv", "a.mkv"];
        v.sort_by(|a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
        assert_eq!(v, ["a.mkv", "Ep1.mkv", "ep2.mkv", "ep02b.mkv", "ep10.mkv"]);
    }

    #[test]
    fn clocks() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(75.9), "1:15");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(timestamp(75.5), "00:01:15.500");
        assert_eq!(timestamp(3725.0421), "01:02:05.042");
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("90"), Some(90.0));
        assert_eq!(parse_time("1:30"), Some(90.0));
        assert_eq!(parse_time("01:02:03.5"), Some(3723.5));
        assert_eq!(parse_time("1m30s"), Some(90.0));
        assert_eq!(parse_time("2.5s"), Some(2.5));
        assert_eq!(parse_time("1h"), Some(3600.0));
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("1:-2"), None);
    }

    #[test]
    fn kinds() {
        assert!(is_media_path(Path::new("a/B.MKV")));
        assert!(is_media_path(Path::new("song.flac")));
        assert!(!is_media_path(Path::new("pic.png")));
        assert!(is_subtitle_path(Path::new("x.srt")));
        assert!(is_url("https://youtu.be/x"));
        assert!(!is_url("C:\\v.mp4"));
    }
}
