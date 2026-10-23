//! Headless ops against a small generated clip. Skipped when ffmpeg isn't installed.

use std::path::PathBuf;
use std::process::Command;

fn has_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// A 4-second 320x180 clip with a 440 Hz tone, generated once per test run.
fn sample() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flick-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("sample.mp4");
    if !out.exists() {
        let st = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25", "-f", "lavfi", "-i", "sine=frequency=440"])
            .args(["-t", "4", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(st.success());
    }
    out
}

#[test]
fn info_frame_contact() {
    if !has_ffmpeg() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let clip = sample();

    let info = flick::ops::info(&clip).unwrap();
    assert_eq!(info["video"][0]["width"], 320);
    assert_eq!(info["video"][0]["height"], 180);
    assert_eq!(info["video"][0]["fps"], 25.0);
    assert_eq!(info["audio"][0]["codec"], "aac");
    let d = info["duration"].as_f64().unwrap();
    assert!((d - 4.0).abs() < 0.1, "duration {d}");

    let frame = clip.with_file_name("frame.png");
    flick::ops::frame(&clip, 2.0, &frame, Some(160)).unwrap();
    let img = image::open(&frame).unwrap();
    assert_eq!((img.width(), img.height()), (160, 90));

    let sheet = clip.with_file_name("contact.png");
    let times = flick::ops::contact(&clip, &sheet, 3, 2, 100).unwrap();
    assert_eq!(times.len(), 6);
    assert!(times.windows(2).all(|w| w[0] < w[1]));
    let img = image::open(&sheet).unwrap();
    // 3 tiles of 100x56 with 4 px gaps around and between them.
    assert_eq!((img.width(), img.height()), (3 * 100 + 4 * 4, 2 * 56 + 3 * 4));

    let _ = std::fs::remove_dir_all(clip.parent().unwrap());
}
