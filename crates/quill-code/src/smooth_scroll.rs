use std::time::Instant;

use gpui_kit::{Pixels, px};

const RATE: f32 = 12.0;
const SETTLE: f32 = 0.5;

#[derive(Default)]
pub struct SmoothScroll {
    target: Option<Pixels>,
    last: Option<Instant>,
}

impl SmoothScroll {
    pub fn moving(&self) -> bool {
        self.target.is_some()
    }

    pub fn nudge(&mut self, current: Pixels, delta: Pixels, max: Pixels) {
        let from = self.target.unwrap_or(current);
        self.target = Some((from + delta).clamp(-max, px(0.)));
    }

    pub fn cancel(&mut self) {
        self.target = None;
        self.last = None;
    }

    pub fn step(&mut self, current: Pixels, now: Instant) -> Option<Pixels> {
        let target = self.target?;
        let dt = self.last.map_or(1.0 / 60.0, |last| (now - last).as_secs_f32().min(0.05));
        self.last = Some(now);
        let next = current + (target - current) * (1.0 - (-dt * RATE).exp());
        if (target - next).abs() < px(SETTLE) {
            self.cancel();
            return Some(target);
        }
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn glides_to_the_target_and_settles() {
        let mut scroll = SmoothScroll::default();
        scroll.nudge(px(0.), px(-300.), px(1000.));
        scroll.nudge(px(0.), px(-200.), px(1000.));
        let mut now = Instant::now();
        let mut offset = px(0.);
        while let Some(next) = scroll.step(offset, now) {
            offset = next;
            now += Duration::from_millis(16);
        }
        assert_eq!(offset, px(-500.));
        assert!(!scroll.moving());
    }

    #[test]
    fn targets_stay_within_the_scroll_range() {
        let mut scroll = SmoothScroll::default();
        scroll.nudge(px(-10.), px(50.), px(100.));
        assert_eq!(scroll.target, Some(px(0.)));
        scroll.nudge(px(0.), px(-500.), px(100.));
        assert_eq!(scroll.target, Some(px(-100.)));
    }
}