//! Seek-bar previews: small keyframe thumbnails from a background ffmpeg, shown above
//! the seek bar while hovering or dragging. The pointer gets a picture at once, even
//! where mpv's own seek is still decoding (fast sweeps, long GOPs).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};

/// Preview size in pixels along the longer side.
const LONG_SIDE: f32 = 224.0;
/// At most about this many previews per file.
const MAX: f64 = 300.0;

type Frames = Arc<Mutex<Vec<(f64, Vec<u8>)>>>;

pub struct Previews {
    frames: Frames,
    size: [usize; 2],
    stop: Arc<AtomicBool>,
    textures: HashMap<usize, TextureHandle>,
}

impl Previews {
    /// Start generating previews for `path` (`aspect` = display width / height).
    pub fn start(path: &Path, duration: f64, aspect: f32, ctx: egui::Context) -> Previews {
        let (w, h) = if aspect >= 1.0 { (LONG_SIDE, LONG_SIDE / aspect) } else { (LONG_SIDE * aspect, LONG_SIDE) };
        let even = |v: f32| ((v / 2.0).round() as u32 * 2).max(16);
        let (w, h) = (even(w), even(h));
        let frames: Frames = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (f2, s2, path) = (frames.clone(), stop.clone(), path.to_path_buf());
        let every = (duration / MAX).max(1.0);
        std::thread::spawn(move || {
            let _ = crate::ops::thumbnails(&path, every, w, h, |t, rgba| {
                if s2.load(Ordering::Relaxed) {
                    return false;
                }
                if let Ok(mut f) = f2.lock() {
                    f.push((t, rgba));
                }
                ctx.request_repaint();
                true
            });
        });
        Previews { frames, size: [w as usize, h as usize], stop, textures: HashMap::new() }
    }

    /// Size in pixels.
    pub fn size(&self) -> [usize; 2] {
        self.size
    }

    /// The preview nearest `t`, once there is one.
    pub fn at(&mut self, ctx: &egui::Context, t: f64) -> Option<TextureHandle> {
        let frames = self.frames.lock().ok()?;
        let i = frames.partition_point(|f| f.0 <= t);
        let near = [i.checked_sub(1), (i < frames.len()).then_some(i)]
            .into_iter()
            .flatten()
            .min_by(|&a, &b| (frames[a].0 - t).abs().total_cmp(&(frames[b].0 - t).abs()))?;
        let tex = self.textures.entry(near).or_insert_with(|| {
            let img = ColorImage::from_rgba_unmultiplied(self.size, &frames[near].1);
            ctx.load_texture(format!("preview-{near}"), img, TextureOptions::LINEAR)
        });
        Some(tex.clone())
    }
}

impl Drop for Previews {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(times: &[f64]) -> Previews {
        let frames = times.iter().map(|&t| (t, vec![0u8; 4 * 4 * 4])).collect();
        Previews { frames: Arc::new(Mutex::new(frames)), size: [4, 4], stop: Arc::default(), textures: HashMap::new() }
    }

    #[test]
    fn nearest_preview() {
        let ctx = egui::Context::default();
        let mut p = fake(&[0.0, 10.0, 20.0]);
        let id = |p: &mut Previews, t: f64| p.at(&ctx, t).map(|t| t.id());
        let (a, b, c) = (id(&mut p, 0.0), id(&mut p, 10.0), id(&mut p, 20.0));
        assert!(a != b && b != c);
        assert_eq!(id(&mut p, 4.9), a);
        assert_eq!(id(&mut p, 5.1), b);
        assert_eq!(id(&mut p, 99.0), c);
        assert_eq!(id(&mut p, -1.0), a);
        // Textures are made once per preview.
        assert_eq!(p.textures.len(), 3);
        assert!(fake(&[]).at(&ctx, 1.0).is_none());
    }

    #[test]
    fn generates_from_a_file() {
        let dir = std::env::temp_dir().join(format!("flick-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("gop10.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-t", "25"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-g", "300", "-keyint_min", "300", "-sc_threshold", "0", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            return eprintln!("ffmpeg not found; skipping");
        }
        let p = Previews::start(&clip, 25.0, 16.0 / 9.0, egui::Context::default());
        assert_eq!(p.size(), [224, 126]);
        let t = std::time::Instant::now();
        while p.frames.lock().unwrap().len() < 3 && t.elapsed().as_secs() < 20 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let times: Vec<f64> = p.frames.lock().unwrap().iter().map(|f| f.0).collect();
        assert_eq!(times, vec![0.0, 10.0, 20.0]);
        assert!(p.frames.lock().unwrap().iter().all(|f| f.1.len() == 224 * 126 * 4));
        let _ = std::fs::remove_dir_all(dir);
    }
}
