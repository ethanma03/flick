//! Seek-bar scrubbing. While the user drags, one seek is in flight at a time (the next
//! goes out when mpv reports the last one's frame is up), so mpv never abandons a
//! half-decoded seek for a newer one and the picture keeps moving. Each seek's kind is
//! picked so the picture follows the pointer as closely as decoding allows and never
//! moves the wrong way: a keyframe seek is nearly free but lands on the keyframe before
//! the target, while an exact seek shows the frame under the pointer but has to decode
//! from that keyframe up to it.

use std::sync::{Arc, OnceLock};

use super::player::Player;

/// FLICK_SCRUB_LOG=path appends every scrub seek and when it landed, for tuning.
fn log(line: std::fmt::Arguments) {
    static FILE: OnceLock<Option<std::sync::Mutex<std::fs::File>>> = OnceLock::new();
    let f = FILE.get_or_init(|| {
        let p = std::env::var_os("FLICK_SCRUB_LOG")?;
        std::fs::OpenOptions::new().create(true).append(true).open(p).ok().map(std::sync::Mutex::new)
    });
    if let Some(Ok(mut f)) = f.as_ref().map(|f| f.lock()) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// An exact seek predicted to be this quick is as good as a keyframe seek: always take it.
const CHEAP: f64 = 0.07;
/// While the pointer moves, don't start an exact seek predicted to take longer than this
/// (the picture would freeze meanwhile); wait for the pointer to rest instead.
const PATIENCE: f64 = 0.4;
/// Without a keyframe index, nor one the pointer would outrun by more than this many
/// pixels before it lands: a fast drag is a search, and keyframes keep the picture coming.
const DRIFT_PX: f64 = 40.0;
/// The pointer counts as resting after this long without moving: go exact.
pub const REST: f64 = 0.08;
/// Fixed part of an exact seek's cost (demuxer seek, first decode, upload).
const OVERHEAD: f64 = 0.02;
/// On release, an exact seek predicted slower than this shows its keyframe first, so a
/// click deep into a long GOP answers at once instead of after the decode.
const SLOW_RELEASE: f64 = 0.15;
/// Give up waiting on a seek's restart event after this long.
const STUCK: f64 = 2.0;

/// What one file has taught us about seeking in it. Kept across drags, reset per file.
#[derive(Default)]
pub struct SeekModel {
    /// Keyframe times on mpv's timeline, filled in by a background ffprobe.
    pub keyframes: Arc<OnceLock<Vec<f64>>>,
    /// Exact-seek cost: seconds of waiting per second decoded past the keyframe (EMA).
    rate: Option<f64>,
    /// Exact-seek latency (EMA), for when there's no keyframe index.
    latency: Option<f64>,
}

impl SeekModel {
    fn keyframe_before(&self, t: f64) -> Option<f64> {
        let k = self.keyframes.get().filter(|k| !k.is_empty())?;
        let i = k.partition_point(|&x| x <= t + 1e-3);
        Some(if i == 0 { 0.0 } else { k[i - 1] })
    }

    fn keyframe_after(&self, t: f64) -> Option<f64> {
        let k = self.keyframes.get()?;
        k.get(k.partition_point(|&x| x <= t + 1e-3)).copied()
    }

    /// How long an exact seek to `t` should take, once there's anything to go on.
    fn exact_cost(&self, t: f64) -> Option<f64> {
        match (self.keyframe_before(t), self.rate) {
            (Some(k), Some(r)) => Some(OVERHEAD + r * (t - k).max(0.0)),
            _ => self.latency,
        }
    }

    fn learn_exact(&mut self, t: f64, took: f64) {
        let ema = |old: Option<f64>, new: f64| Some(old.map_or(new, |o| o * 0.6 + new * 0.4));
        self.latency = ema(self.latency, took);
        // Too close to the keyframe and the overhead swamps the per-second rate.
        if let Some(span) = self.keyframe_before(t).map(|k| t - k).filter(|s| *s > 0.25) {
            self.rate = ema(self.rate, (took - OVERHEAD).max(0.0) / span);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seek {
    Exact(f64),
    Keyframe(f64),
}

impl Seek {
    fn send(self, player: &Player) {
        let (t, how) = match self {
            Seek::Exact(t) => (t, "absolute+exact"),
            Seek::Keyframe(t) => (t, "absolute+keyframes"),
        };
        let _ = player.cmd(&["seek", &format!("{t:.3}"), how]);
    }
}

/// What's on screen: its time, and whether it is the exact frame for that time (as
/// opposed to the keyframe some seek fell back to).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub pos: f64,
    pub exact: bool,
}

/// Where the pointer is and how it's moving, in media seconds.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub target: f64,
    /// Media seconds per second, signed (0 when resting).
    pub velocity: f64,
    pub resting: bool,
    /// Media seconds per pixel of seek bar.
    pub per_px: f64,
}

impl Pointer {
    /// How far apart two times can be and still count as the same spot.
    fn tol(&self) -> f64 {
        (self.per_px / 2.0).max(0.01)
    }
}

/// The seek to send next, given what's on screen and where the pointer is.
pub fn next_seek(model: &SeekModel, shown: Shown, p: Pointer) -> Option<Seek> {
    let (target, tol) = (p.target, p.tol());
    if shown.exact && (shown.pos - target).abs() <= tol {
        return None;
    }
    let cost = model.exact_cost(target);
    if p.resting || cost.is_some_and(|c| c <= CHEAP) {
        return Some(Seek::Exact(target));
    }
    // Mid-drag an exact frame beats waiting, unless the pointer will cross a keyframe
    // before it lands: that keyframe's seek is nearly free and lands closer.
    let outrun = |c: f64| match (model.keyframes.get(), p.velocity > 0.0) {
        (Some(_), true) => model.keyframe_after(target).is_some_and(|k| (k - target) / p.velocity < c),
        (Some(_), false) => false,
        (None, _) => p.velocity.abs() * c > DRIFT_PX * p.per_px,
    };
    let affordable = cost.is_none_or(|c| c <= PATIENCE && !outrun(c));
    match model.keyframe_before(target) {
        Some(k) if toward(shown, k, target, tol) => Some(Seek::Keyframe(target)),
        Some(_) => affordable.then_some(Seek::Exact(target)),
        // No index (yet): a keyframe seek could fall back behind an exact frame on screen.
        None if shown.exact => affordable.then_some(Seek::Exact(target)),
        None => ((shown.pos - target).abs() > tol).then_some(Seek::Keyframe(target)),
    }
}

/// A keyframe seek is only worth it if its keyframe `k` lies between what's shown and
/// the target: anything else is no progress, or a jump the wrong way.
fn toward(shown: Shown, k: f64, target: f64, tol: f64) -> bool {
    (k - target).abs() + tol < (shown.pos - target).abs() && (k - shown.pos) * (target - shown.pos) > 0.0
}

struct Inflight {
    seek: Seek,
    at: f64,
    restarts: u64,
}

/// A seek-bar drag in progress, or released and waiting for its last seek to land.
pub struct Scrub {
    /// The time being scrubbed to (NaN until the first pointer frame).
    pub t: f64,
    /// Playback was running when the drag began: resume it on release.
    pub resume: bool,
    /// The unpause has been queued.
    resumed: bool,
    /// Released, and the exact seek to the target still has to go out (after the
    /// keyframe seek in flight).
    final_exact: bool,
    /// When `t` last changed.
    moved: f64,
    /// Pointer velocity in media seconds per second (smoothed).
    velocity: f64,
    /// When the button came up.
    pub released: Option<f64>,
    inflight: Option<Inflight>,
    /// What the last completed seek put on screen.
    shown: Option<Shown>,
}

impl Scrub {
    /// The button went down on the seek bar. A drag that's still settling is picked up
    /// where it was, keeping its resume and its seek in flight.
    pub fn grab(prev: Option<Scrub>, player: &Player, now: f64) -> Scrub {
        if let Some(mut s) = prev {
            s.released = None;
            s.final_exact = false;
            if s.resumed {
                let _ = player.set("pause", "yes");
                s.resumed = false;
            }
            s.moved = now;
            return s;
        }
        let resume = !player.pb.pause && !player.pb.eof;
        if resume {
            let _ = player.set("pause", "yes");
        }
        Scrub { t: f64::NAN, resume, resumed: false, final_exact: false, moved: now, velocity: 0.0, released: None, inflight: None, shown: None }
    }

    pub fn dragging(&self) -> bool {
        self.released.is_none()
    }

    pub fn move_to(&mut self, t: f64, now: f64) {
        if t != self.t {
            if !self.t.is_nan() {
                let v = (t - self.t) / (now - self.moved).max(0.008);
                self.velocity = if now - self.moved > REST { v } else { self.velocity * 0.5 + v * 0.5 };
            }
            self.t = t;
            self.moved = now;
        }
    }

    fn shown_or_playing(&self, player: &Player) -> Shown {
        self.shown.unwrap_or(Shown { pos: player.pb.time_pos.unwrap_or(0.0), exact: false })
    }

    fn send(&mut self, seek: Seek, player: &Player, now: f64) {
        seek.send(player);
        self.inflight = Some(Inflight { seek, at: now, restarts: player.restarts });
    }

    /// The exact seek to the target, then the unpause: queued after the seek, so playback
    /// never runs on from the old spot (or the keyframe) for a moment.
    fn send_final(&mut self, player: &Player, now: f64) {
        log(format_args!("{now:.3} final Exact({:.3})", self.t));
        self.send(Seek::Exact(self.t), player, now);
        self.final_exact = false;
        self.resume_playback(player);
    }

    fn resume_playback(&mut self, player: &Player) {
        if self.resume && !self.resumed {
            let _ = player.set("pause", "no");
            self.resumed = true;
        }
    }

    /// The button came up: make sure the exact frame is coming, then resume.
    pub fn release(&mut self, player: &Player, model: &SeekModel, now: f64, per_px: f64) {
        if self.released.is_some() || self.t.is_nan() {
            return;
        }
        self.released = Some(now);
        let tol = (per_px / 2.0).max(0.01);
        let near = |x: f64| (x - self.t).abs() <= tol;
        let shown = self.shown_or_playing(player);
        let flying = self.inflight.as_ref().map(|f| f.seek);
        if (shown.exact && near(shown.pos)) || matches!(flying, Some(Seek::Exact(x)) if near(x)) {
            self.resume_playback(player);
            return;
        }
        self.final_exact = true;
        if matches!(flying, Some(Seek::Keyframe(x)) if near(x)) {
            return; // its keyframe is coming; the exact seek follows when it lands
        }
        let slow = model.exact_cost(self.t).is_none_or(|c| c > SLOW_RELEASE);
        let keyframe = model.keyframe_before(self.t).filter(|&k| toward(shown, k, self.t, tol));
        if slow && keyframe.is_some() {
            log(format_args!("{now:.3} release, keyframe first"));
            self.send(Seek::Keyframe(self.t), player, now);
        } else {
            self.send_final(player, now);
        }
    }

    /// Per frame: note a finished seek, send the next one. Returns false once a released
    /// drag has landed and can go.
    pub fn step(&mut self, player: &Player, model: &mut SeekModel, now: f64, per_px: f64) -> bool {
        if let Some(f) = &self.inflight {
            let landed = player.restarts > f.restarts;
            if landed || now - f.at > STUCK {
                log(format_args!("{now:.3} landed {:?} after {:.0} ms{}", f.seek, (now - f.at) * 1000.0, if landed { "" } else { " (gave up)" }));
                self.shown = Some(match f.seek {
                    Seek::Exact(t) => {
                        if landed {
                            model.learn_exact(t, now - f.at);
                        }
                        Shown { pos: t, exact: true }
                    }
                    Seek::Keyframe(t) => {
                        // time-pos lags the restart event by a poll; the index doesn't.
                        let pos = model.keyframe_before(t).or(player.pb.time_pos).unwrap_or(t);
                        Shown { pos, exact: false }
                    }
                });
                self.inflight = None;
            }
        }
        if let Some(at) = self.released {
            if self.inflight.is_none() && self.final_exact {
                self.send_final(player, now);
            }
            // Hold the knob on the target until mpv's position has caught up too, or the
            // stale time-pos shows for a frame.
            let behind = player.pb.time_pos.is_none_or(|p| (p - self.t).abs() > 0.25);
            let keep = (self.inflight.is_some() || behind) && now - at < STUCK;
            if !keep {
                self.resume_playback(player);
            }
            return keep;
        }
        if self.inflight.is_none() && !self.t.is_nan() {
            let shown = self.shown_or_playing(player);
            let resting = now - self.moved >= REST;
            let p = Pointer { target: self.t, velocity: if resting { 0.0 } else { self.velocity }, resting, per_px };
            if let Some(seek) = next_seek(model, shown, p) {
                log(format_args!("{now:.3} send {seek:?} shown {shown:?} v {:.1} cost {:?}", p.velocity, model.exact_cost(self.t)));
                self.send(seek, player, now);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(keys: &[f64], rate: Option<f64>) -> SeekModel {
        let m = SeekModel { rate, latency: rate.map(|r| OVERHEAD + r * 5.0), ..Default::default() };
        if !keys.is_empty() {
            m.keyframes.set(keys.to_vec()).unwrap();
        }
        m
    }
    const KEYS: &[f64] = &[0.0, 10.0, 20.0, 30.0];
    /// A pointer at `t`: resting or moving slowly, on a bar where a pixel is 0.04 s.
    fn at(t: f64, resting: bool) -> Pointer {
        Pointer { target: t, velocity: if resting { 0.0 } else { 2.0 }, resting, per_px: 0.04 }
    }

    #[test]
    fn keyframe_lookup() {
        let m = model(KEYS, None);
        assert_eq!(m.keyframe_before(-1.0), Some(0.0));
        assert_eq!(m.keyframe_before(10.0), Some(10.0));
        assert_eq!(m.keyframe_before(19.99), Some(10.0));
        assert_eq!(m.keyframe_before(99.0), Some(30.0));
        assert_eq!(model(&[], None).keyframe_before(5.0), None);
    }

    #[test]
    fn exact_frame_on_screen_needs_nothing() {
        let m = model(KEYS, Some(0.05));
        assert_eq!(next_seek(&m, Shown { pos: 15.0, exact: true }, at(15.01, true)), None);
    }

    #[test]
    fn rest_goes_exact() {
        let m = model(KEYS, Some(0.05));
        assert_eq!(next_seek(&m, Shown { pos: 10.0, exact: false }, at(18.0, true)), Some(Seek::Exact(18.0)));
    }

    #[test]
    fn cheap_files_always_exact() {
        // Short GOPs: an exact seek costs about as much as a keyframe one.
        let m = model(&[0.0, 0.5, 1.0, 1.5, 2.0], Some(0.03));
        assert_eq!(next_seek(&m, Shown { pos: 0.0, exact: false }, at(1.7, false)), Some(Seek::Exact(1.7)));
    }

    #[test]
    fn keyframe_toward_target() {
        let m = model(KEYS, Some(0.05));
        assert_eq!(next_seek(&m, Shown { pos: 12.0, exact: true }, at(25.0, false)), Some(Seek::Keyframe(25.0)));
        // Backwards too: keyframe 0 lies between 12 and the target.
        assert_eq!(next_seek(&m, Shown { pos: 12.0, exact: true }, at(5.0, false)), Some(Seek::Keyframe(5.0)));
    }

    #[test]
    fn never_back_past_an_exact_frame() {
        // 15 is on screen exactly; moving on to 17 must not drop back to keyframe 10.
        let m = model(KEYS, Some(0.05));
        assert_eq!(next_seek(&m, Shown { pos: 15.0, exact: true }, at(17.0, false)), Some(Seek::Exact(17.0)));
        // Same when the cost is too high to go exact while moving: wait instead.
        let slow = model(KEYS, Some(0.2));
        assert_eq!(next_seek(&slow, Shown { pos: 15.0, exact: true }, at(17.0, false)), None);
        // Dragging back within the GOP: keyframe 10 is behind the target, not between.
        assert_eq!(next_seek(&m, Shown { pos: 15.0, exact: true }, at(13.0, false)), Some(Seek::Exact(13.0)));
    }

    #[test]
    fn fast_drags_stay_on_keyframes() {
        // Exact to 17 costs 0.37 s; at 30 s/s the pointer passes keyframe 20 in 0.1 s,
        // and that keyframe's seek is the better picture.
        let m = model(KEYS, Some(0.05));
        let fast = Pointer { velocity: 30.0, ..at(17.0, false) };
        assert_eq!(next_seek(&m, Shown { pos: 15.0, exact: true }, fast), None);
        let fast = Pointer { velocity: 30.0, ..at(27.0, false) };
        assert_eq!(next_seek(&m, Shown { pos: 15.0, exact: true }, fast), Some(Seek::Keyframe(27.0)));
        // Slow and far from the next keyframe: exact.
        let slow = Pointer { velocity: 2.0, ..at(12.0, false) };
        assert_eq!(next_seek(&m, Shown { pos: 11.0, exact: true }, slow), Some(Seek::Exact(12.0)));
        // No index: the drift in pixels decides.
        let mut n = model(&[], None);
        n.latency = Some(0.2);
        let fast = Pointer { velocity: 30.0, ..at(17.0, false) };
        assert_eq!(next_seek(&n, Shown { pos: 15.0, exact: true }, fast), None);
    }

    #[test]
    fn same_keyframe_again_is_no_progress() {
        let m = model(KEYS, Some(0.05));
        assert_eq!(next_seek(&m, Shown { pos: 10.0, exact: false }, at(14.0, false)), Some(Seek::Exact(14.0)));
    }

    #[test]
    fn without_an_index() {
        let m = model(&[], None);
        // Keyframe seeks while only keyframes are on screen...
        assert_eq!(next_seek(&m, Shown { pos: 10.0, exact: false }, at(25.0, false)), Some(Seek::Keyframe(25.0)));
        // ...but exact after an exact frame, which a keyframe seek could fall behind.
        assert_eq!(next_seek(&m, Shown { pos: 25.0, exact: true }, at(26.0, false)), Some(Seek::Exact(26.0)));
    }

    #[test]
    fn learns_the_decode_rate() {
        let mut m = model(KEYS, None);
        m.learn_exact(15.0, 0.02 + 5.0 * 0.04);
        assert!((m.exact_cost(18.0).unwrap() - (0.02 + 8.0 * 0.04)).abs() < 1e-9);
        // Right at a keyframe the rate isn't learnt, only the plain latency.
        let mut n = model(KEYS, None);
        n.learn_exact(10.1, 0.05);
        assert_eq!(n.rate, None);
        assert_eq!(n.latency, Some(0.05));
    }
}

/// Scrubs a real (headless) mpv: the app's options with `vo=null`, so decoding and seek
/// timing are genuine but nothing shows. Skipped without libmpv or ffmpeg.
#[cfg(test)]
mod live {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::mpv::Mpv;

    const DUR: f64 = 30.0;
    const FPS: f64 = 30.0;

    /// A 30 s clip with a keyframe every 10 s: the hard case for scrubbing.
    fn clip() -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("flick-scrub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let out = dir.join("long_gop.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30", "-t", "30"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-g", "300", "-keyint_min", "300", "-sc_threshold", "0", "-pix_fmt", "yuv420p"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(out)
    }

    fn player(dir: &std::path::Path) -> Option<Player> {
        crate::mpv::available().ok()?;
        let mut opts = super::super::player::options(dir, 100.0, true);
        for (k, v) in opts.iter_mut() {
            match k.as_str() {
                "vo" => *v = "null".into(),
                "resume-playback" => *v = "no".into(),
                _ => {}
            }
        }
        opts.push(("ao".into(), "null".into()));
        let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        Some(Player::new(Arc::new(Mpv::new(&opts).ok()?)))
    }

    /// Pointer path: (x as a fraction of the bar, button down), or None when over.
    type Script = fn(f64) -> Option<(f64, bool)>;

    fn slow(ms: f64) -> Option<(f64, bool)> {
        match ms {
            m if m < 2000.0 => Some((0.2 + 0.5 * m / 2000.0, true)),
            m if m < 2400.0 => Some((0.7, true)),
            m if m < 3400.0 => Some((0.7, false)),
            _ => None,
        }
    }

    fn steps(ms: f64) -> Option<(f64, bool)> {
        // 125 ms moves of 2% each, then 375 ms rests.
        if ms >= 4400.0 {
            return None;
        }
        let k = (ms / 500.0).min(8.0);
        let (i, f) = (k.floor(), k.fract());
        Some((0.3 + 0.02 * (i + (f * 4.0).min(1.0)), ms < 4000.0))
    }

    fn back(ms: f64) -> Option<(f64, bool)> {
        match ms {
            m if m < 2000.0 => Some((0.6 - 0.4 * m / 2000.0, true)),
            m if m < 3000.0 => Some((0.2, false)),
            _ => None,
        }
    }

    struct Run {
        /// (ms, target, time-pos) at every playback restart while the button was down.
        pictures: Vec<(f64, f64, f64)>,
        /// The same after release.
        after: Vec<(f64, f64, f64)>,
        /// Rests of 300 ms+ that showed the exact frame, out of all such rests.
        rests: (usize, usize),
        final_target: f64,
        resumed: bool,
    }

    fn run(p: &mut Player, model: &mut SeekModel, script: Script, playing: bool) -> Run {
        p.mpv.set("pause", if playing { "no" } else { "yes" }).unwrap();
        p.mpv.command(&["seek", "1", "absolute+exact"]).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        p.poll();
        let per_px = DUR / 1500.0;
        let t0 = Instant::now();
        let mut scrub: Option<Scrub> = None;
        let mut seen = p.restarts;
        let mut out = Run { pictures: vec![], after: vec![], rests: (0, 0), final_target: 0.0, resumed: false };
        // The current rest: since when the pointer has been still, and whether it hit.
        let (mut last_x, mut still_since, mut hit) = (f64::NAN, 0.0, false);
        loop {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let Some((x, down)) = script(ms) else { break };
            let now = ms / 1000.0;
            p.poll();
            let target = (x * DUR).min(DUR - 0.05);
            if down {
                let mut s = match scrub.take() {
                    Some(s) if s.dragging() => s,
                    settling => Scrub::grab(settling, p, now),
                };
                s.move_to(target, now);
                scrub = Some(s);
            } else if let Some(s) = &mut scrub {
                s.release(p, model, now, per_px);
            }
            if let Some(s) = &mut scrub {
                out.final_target = s.t;
                if !s.step(p, model, now, per_px) {
                    scrub = None;
                }
            }
            if p.restarts != seen {
                seen = p.restarts;
                let pos = p.mpv.get_double("time-pos").unwrap_or(f64::NAN);
                if down { out.pictures.push((ms, target, pos)) } else { out.after.push((ms, target, pos)) }
            }
            let still = down && x == last_x;
            if still {
                let pos = p.mpv.get_double("time-pos").unwrap_or(f64::NAN);
                hit |= (pos - target).abs() < 1.5 / FPS;
            } else {
                if ms - still_since > 300.0 && last_x.is_finite() {
                    out.rests.1 += 1;
                    out.rests.0 += hit as usize;
                }
                still_since = ms;
                hit = false;
            }
            last_x = if down { x } else { f64::NAN };
            std::thread::sleep(Duration::from_millis(16));
        }
        p.poll();
        out.resumed = p.mpv.get_flag("pause") == Some(false);
        out
    }

    /// The picture never steps away from the pointer by more than a frame or two.
    fn assert_no_backsteps(name: &str, r: &Run) {
        for w in r.pictures.windows(2) {
            let ((_, _, a), (ms, target, b)) = (w[0], w[1]);
            assert!(
                (b - target).abs() <= (a - target).abs() + 2.0 / FPS,
                "{name}: picture went {a:.3} -> {b:.3} at {ms:.0} ms with the pointer at {target:.3}"
            );
        }
    }

    #[test]
    fn scrubs_a_long_gop_file() {
        let Some(path) = clip() else { return eprintln!("ffmpeg not found; skipping") };
        let dir = path.parent().unwrap().to_path_buf();
        let Some(mut p) = player(&dir) else { return eprintln!("libmpv not found; skipping") };
        p.mpv.command(&["loadfile", &path.to_string_lossy()]).unwrap();
        let t = Instant::now();
        while p.pb.duration.is_none() && t.elapsed() < Duration::from_secs(10) {
            p.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut model = SeekModel::default();
        model.keyframes.set(crate::ops::keyframes(&path).unwrap()).unwrap();
        assert_eq!(model.keyframes.get().unwrap(), &vec![0.0, 10.0, 20.0]);

        for playing in [true, false] {
            for (name, script) in [("slow", slow as Script), ("steps", steps), ("back", back)] {
                let r = run(&mut p, &mut model, script, playing);
                let name = format!("{name}{}", if playing { "" } else { " (paused)" });
                eprintln!("{name}: {} pictures while held, exact on {}/{} rests", r.pictures.len(), r.rests.0, r.rests.1);
                assert!(r.pictures.len() >= 5, "{name}: the picture barely moved ({} updates)", r.pictures.len());
                assert_no_backsteps(&name, &r);
                // Released: the exact frame lands, and playback picks up only if it was running.
                let pos = p.mpv.get_double("time-pos").unwrap();
                // (It may already have landed while the pointer rested before release.)
                let last_held = r.pictures.last().map(|&(_, _, at)| at);
                let landed = last_held.into_iter().chain(r.after.iter().map(|&(_, _, at)| at)).any(|at| (at - r.final_target).abs() < 1.5 / FPS);
                assert!(landed, "{name}: never landed on {:.3}", r.final_target);
                assert_eq!(r.resumed, playing, "{name}: resume");
                if playing {
                    assert!(pos >= r.final_target - 1.0 / FPS, "{name}: resumed from {pos:.3}, not {:.3}", r.final_target);
                }
                if name.starts_with("steps") {
                    assert!(r.rests.0 * 4 >= r.rests.1 * 3, "{name}: exact frame on only {}/{} rests", r.rests.0, r.rests.1);
                }
            }
        }
    }
}
