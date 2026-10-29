//! Headless operations through ffprobe/ffmpeg: describe a file, grab frames, contact sheets.
//! No player window needed, so Claude can look at a video without disturbing playback.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn tool(name: &str) -> Command {
    let mut c = Command::new(name);
    c.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn run(mut c: Command, what: &str) -> Result<Vec<u8>, String> {
    let out = c.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{what} not found on PATH (winget install Gyan.FFmpeg)")
        } else {
            format!("{what}: {e}")
        }
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed");
        return Err(format!("{what}: {last}"));
    }
    Ok(out.stdout)
}

pub fn ffprobe_raw(path: &Path) -> Result<Value, String> {
    let mut c = tool("ffprobe");
    c.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-show_chapters"]).arg(path);
    let out = run(c, "ffprobe")?;
    serde_json::from_slice(&out).map_err(|e| format!("ffprobe output: {e}"))
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// "30000/1001" → 29.97
fn rate(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let (a, b) = s.split_once('/')?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (b > 0.0 && a > 0.0).then(|| (a / b * 1000.0).round() / 1000.0)
}

/// A compact description: container, duration, size, and one entry per stream.
pub fn info(path: &Path) -> Result<Value, String> {
    let raw = ffprobe_raw(path)?;
    let fmt = &raw["format"];
    let empty = vec![];
    let streams = raw["streams"].as_array().unwrap_or(&empty);
    let lang = |s: &Value| s["tags"]["language"].as_str().map(String::from);
    let title = |s: &Value| s["tags"]["title"].as_str().map(String::from);
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subs = Vec::new();
    for s in streams {
        let idx = s["index"].as_u64();
        match s["codec_type"].as_str() {
            Some("video") if s["disposition"]["attached_pic"].as_u64() != Some(1) => {
                let rotation = s["side_data_list"]
                    .as_array()
                    .and_then(|l| l.iter().find_map(|d| num(&d["rotation"])))
                    .or_else(|| num(&s["tags"]["rotate"]));
                video.push(json!({
                    "index": idx,
                    "codec": s["codec_name"],
                    "profile": s["profile"],
                    "width": s["width"],
                    "height": s["height"],
                    "fps": rate(&s["avg_frame_rate"]).or_else(|| rate(&s["r_frame_rate"])),
                    "pix_fmt": s["pix_fmt"],
                    "bit_rate": num(&s["bit_rate"]),
                    "frames": num(&s["nb_frames"]),
                    "rotation": rotation,
                    "color": s["color_transfer"],
                }));
            }
            Some("audio") => audio.push(json!({
                "index": idx,
                "codec": s["codec_name"],
                "channels": s["channels"],
                "layout": s["channel_layout"],
                "sample_rate": num(&s["sample_rate"]),
                "bit_rate": num(&s["bit_rate"]),
                "language": lang(s),
                "title": title(s),
            })),
            Some("subtitle") => subs.push(json!({
                "index": idx,
                "codec": s["codec_name"],
                "language": lang(s),
                "title": title(s),
            })),
            _ => {}
        }
    }
    let size = num(&fmt["size"]).or_else(|| std::fs::metadata(path).ok().map(|m| m.len() as f64));
    Ok(json!({
        "path": std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
        "container": fmt["format_name"],
        "duration": num(&fmt["duration"]),
        "size_bytes": size,
        "size": size.map(|s| crate::media::human_size(s as u64)),
        "bit_rate": num(&fmt["bit_rate"]),
        "video": video,
        "audio": audio,
        "subtitles": subs,
        "chapters": raw["chapters"].as_array().map(|c| c.iter().map(|ch| json!({
            "start": num(&ch["start_time"]),
            "end": num(&ch["end_time"]),
            "title": ch["tags"]["title"],
        })).collect::<Vec<_>>()),
    }))
}

pub fn duration(path: &Path) -> Result<f64, String> {
    let mut c = tool("ffprobe");
    c.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"]).arg(path);
    let out = run(c, "ffprobe")?;
    String::from_utf8_lossy(&out).trim().parse().map_err(|_| "couldn't read the duration".to_string())
}

/// Keyframe times of the first video stream, sorted, on mpv's timeline (relative to the
/// container start time). Reads packet headers only, no decoding.
pub fn keyframes(path: &Path) -> Result<Vec<f64>, String> {
    let mut c = tool("ffprobe");
    c.args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags:format=start_time", "-of", "csv=p=1"])
        .arg(path);
    let out = run(c, "ffprobe")?;
    Ok(parse_keyframes(&String::from_utf8_lossy(&out)))
}

fn parse_keyframes(csv: &str) -> Vec<f64> {
    let mut start = 0.0;
    let mut keys = Vec::new();
    for line in csv.lines() {
        let mut f = line.trim().split(',');
        match (f.next(), f.next(), f.next()) {
            (Some("packet"), Some(pts), Some(flags)) if flags.starts_with('K') => keys.extend(pts.parse::<f64>().ok()),
            (Some("format"), Some(s), _) => start = s.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    let mut keys: Vec<f64> = keys.into_iter().map(|k| k - start).collect();
    keys.sort_by(f64::total_cmp);
    keys.dedup();
    keys
}

/// Small frames across a video for the seek-bar preview, streamed to `each(time, rgba)`
/// as ffmpeg decodes them (stop early by returning false). Only keyframes are decoded,
/// which is cheap, and kept at least `every` seconds apart. Each frame is `w`x`h` RGBA,
/// letterboxed. Runs at below-normal priority so playback never waits on it.
pub fn thumbnails(path: &Path, every: f64, w: u32, h: u32, mut each: impl FnMut(f64, Vec<u8>) -> bool) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    let mut c = tool("ffmpeg");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        c.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    // showinfo reports each kept frame's time on stderr, in the same order as stdout.
    let vf = format!(
        "select='isnan(prev_selected_t)+gte(t-prev_selected_t,{every})',showinfo,\
         scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
    );
    c.args(["-hide_banner", "-nostats", "-loglevel", "info", "-skip_frame", "nokey", "-threads", "2"]).arg("-i").arg(path);
    c.args(["-an", "-sn", "-vf", &vf, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("ffmpeg: {e}"))?;
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !line.contains("Parsed_showinfo") {
                continue;
            }
            let t = line.split("pts_time:").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<f64>().ok());
            if let Some(t) = t
                && tx.send(t).is_err()
            {
                break;
            }
        }
    });
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let result = loop {
        if out.read_exact(&mut buf).is_err() {
            break Ok(());
        }
        let Ok(t) = rx.recv() else { break Ok(()) };
        if !each(t, buf.clone()) {
            break Ok(());
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// One frame at `at` seconds as an image file (format from the extension). `width` scales, keeping aspect.
pub fn frame(path: &Path, at: f64, out: &Path, width: Option<u32>) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut c = tool("ffmpeg");
    // -ss before -i seeks by keyframe then decodes forward: fast and still frame-accurate.
    c.args(["-v", "error", "-y", "-ss", &format!("{at:.3}")]).arg("-i").arg(path);
    c.args(["-frames:v", "1", "-an", "-sn"]);
    if let Some(w) = width {
        c.args(["-vf", &format!("scale={w}:-2")]);
    }
    c.arg(out);
    run(c, "ffmpeg")?;
    if !out.exists() {
        return Err(format!("no frame at {at:.3}s (past the end?)"));
    }
    Ok(())
}

/// Evenly spaced frames tiled into one image. Returns the timestamp of each tile, row by row.
pub fn contact(path: &Path, out: &Path, cols: u32, rows: u32, tile_width: u32) -> Result<Vec<f64>, String> {
    let dur = duration(path)?;
    let n = (cols * rows).max(1);
    let times: Vec<f64> = (0..n).map(|i| dur * (i as f64 + 0.5) / n as f64).collect();
    let tmp = std::env::temp_dir().join("flick").join(format!("contact-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    // Extract in parallel; each ffmpeg seeks straight to its timestamp.
    let jobs: Vec<(PathBuf, std::thread::JoinHandle<Result<(), String>>)> = times
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let (p, f) = (path.to_path_buf(), tmp.join(format!("{i:03}.png")));
            let f2 = f.clone();
            (f, std::thread::spawn(move || frame(&p, t, &f2, Some(tile_width))))
        })
        .collect();
    let mut tiles = Vec::new();
    for (f, j) in jobs {
        j.join().map_err(|_| "frame worker panicked".to_string())??;
        tiles.push(image::open(&f).map_err(|e| format!("{}: {e}", f.display()))?.to_rgba8());
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let (tw, th) = tiles.iter().fold((0, 0), |(w, h), t| (w.max(t.width()), h.max(t.height())));
    let gap = 4;
    let mut sheet = image::RgbaImage::from_pixel(cols * tw + (cols + 1) * gap, rows * th + (rows + 1) * gap, image::Rgba([0x13, 0x13, 0x15, 255]));
    for (i, t) in tiles.iter().enumerate() {
        let (cx, cy) = (i as u32 % cols, i as u32 / cols);
        image::imageops::overlay(&mut sheet, t, (gap + cx * (tw + gap)) as i64, (gap + cy * (th + gap)) as i64);
    }
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    sheet.save(out).map_err(|e| format!("couldn't write {}: {e}", out.display()))?;
    Ok(times)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        assert_eq!(rate(&json!("30000/1001")), Some(29.97));
        assert_eq!(rate(&json!("25/1")), Some(25.0));
        assert_eq!(rate(&json!("0/0")), None);
    }

    #[test]
    fn keyframe_csv() {
        let csv = "packet,1.5,K__\npacket,2.0,__\npacket,N/A,K__\npacket,11.5,K_\npacket,1.5,K__\nformat,1.500000\n";
        assert_eq!(parse_keyframes(csv), vec![0.0, 10.0]);
    }
}
