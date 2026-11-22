use std::time::{Duration, Instant};

use gpui::Rgba;

use super::office_art::CELL_W;

pub const DUR: Duration = Duration::from_millis(200);
const MAX_COLS: usize = 4;
pub const SCENE_PAD: f32 = 24.0;

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn cols_for(width: f32) -> usize {
    (((width - 2.0 * SCENE_PAD) / CELL_W).floor() as usize).clamp(1, MAX_COLS)
}

pub fn fade(mut c: Rgba, t: f32) -> Rgba {
    c.alpha *= t.clamp(0.0, 1.0);
    c
}

#[derive(Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
}

impl Tween {
    pub fn at(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
        }
    }

    pub fn between(from: f32, to: f32, now: Instant) -> Self {
        Self { from, to, start: now }
    }

    fn progress(&self, now: Instant) -> f32 {
        let t = now.saturating_duration_since(self.start).as_secs_f32() / DUR.as_secs_f32();
        t.clamp(0.0, 1.0)
    }

    pub fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress(now))
    }

    pub fn active(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    pub fn retarget(&mut self, to: f32, now: Instant) {
        if (to - self.to).abs() > 0.01 {
            *self = Self::between(self.value(now), to, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_follow_width_within_bounds() {
        assert_eq!(cols_for(0.0), 1);
        assert_eq!(cols_for(2.0 * SCENE_PAD + CELL_W * 2.0), 2);
        assert_eq!(cols_for(10_000.0), MAX_COLS);
    }

    #[test]
    fn retarget_continues_from_the_current_value() {
        let t0 = Instant::now();
        let mut t = Tween::between(0.0, 100.0, t0);
        let mid = t0 + DUR / 2;
        let here = t.value(mid);
        assert!(here > 0.0 && here < 100.0);
        t.retarget(0.0, mid);
        assert!((t.value(mid) - here).abs() < 0.01);
        assert!(t.value(mid + DUR) < 0.01);
    }
}
