//! Scripted seek-bar input for automated scrub tests, only with FLICK_DEV set:
//! `flick ctl drive <ms:x:down:shift,...>` replays a pointer path through egui's own
//! input, so a test never touches the real mouse or keyboard (and the window never
//! captures the mouse). `x` is a fraction of the seek bar; between points it moves
//! linearly, the button and Shift change at each point.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    ms: f64,
    x: f32,
    down: bool,
    shift: bool,
}

pub struct Drive {
    points: Vec<Point>,
    start: Option<f64>,
    down: bool,
    shift: bool,
}

impl Drive {
    pub fn parse(s: &str) -> Result<Drive, String> {
        let bad = || format!("drive: expected ms:x:down:shift,... got {s:?}");
        let points = s
            .split(',')
            .map(|p| {
                let f: Vec<&str> = p.trim().split(':').collect();
                let [ms, x, down, shift] = f[..] else { return Err(bad()) };
                Ok(Point {
                    ms: ms.parse().map_err(|_| bad())?,
                    x: x.parse().map_err(|_| bad())?,
                    down: down == "1",
                    shift: shift == "1",
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if points.is_empty() || points.windows(2).any(|w| w[1].ms < w[0].ms) {
            return Err(bad());
        }
        Ok(Drive { points, start: None, down: false, shift: false })
    }

    /// The pointer state at `ms` into the script.
    fn at(&self, ms: f64) -> Point {
        let i = self.points.partition_point(|p| p.ms <= ms);
        let prev = self.points[i.saturating_sub(1)];
        match self.points.get(i) {
            Some(next) if i > 0 => {
                let f = ((ms - prev.ms) / (next.ms - prev.ms).max(1e-9)) as f32;
                Point { x: prev.x + (next.x - prev.x) * f, ..prev }
            }
            _ => prev,
        }
    }

    /// Replace this frame's real pointer input with the script's, for the seek bar at
    /// `bar`. Returns false once the script is over (the button is let go first).
    pub fn inject(&mut self, raw: &mut RawInput, bar: Rect) -> bool {
        let now = raw.time.unwrap_or(0.0);
        let ms = (now - *self.start.get_or_insert(now)) * 1000.0;
        let done = ms > self.points.last().map_or(0.0, |p| p.ms);
        let p = self.at(ms);
        let down = p.down && !done;
        let pos = Pos2::new(bar.left() + p.x.clamp(0.0, 1.0) * bar.width(), bar.center().y);
        raw.events.retain(|e| {
            !matches!(e, Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone | Event::MouseMoved(_) | Event::ModifiersChanged(_))
        });
        let shift = p.shift && !done;
        let modifiers = Modifiers { shift, ..Default::default() };
        if shift != self.shift {
            raw.events.push(Event::ModifiersChanged(modifiers));
            self.shift = shift;
        }
        raw.events.push(Event::PointerMoved(pos));
        if down != self.down {
            raw.events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: down, modifiers });
            self.down = down;
        }
        !done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_interpolates() {
        let d = Drive::parse("0:0.2:0:0, 100:0.2:1:0, 300:0.6:1:1").unwrap();
        assert_eq!(d.at(50.0), Point { ms: 0.0, x: 0.2, down: false, shift: false });
        let p = d.at(200.0);
        assert!((p.x - 0.4).abs() < 1e-6 && p.down && !p.shift);
        assert!(d.at(400.0).shift);
        assert!(Drive::parse("0:0.2:0").is_err());
        assert!(Drive::parse("10:0:0:0,5:0:0:0").is_err());
    }
}
