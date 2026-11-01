//! `wl_surface.frame` 콜백 감시 (docs/protocols.md#1차-wl_surfaceframe-core).
//!
//! 시각만 다루는 순수 로직이다. 콜백 요청·타이머는 daemon이 한다.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    Hidden,
    Visible,
}

#[derive(Debug, Clone, Default)]
pub struct FrameWatch {
    /// 응답을 기다리는 콜백을 요청한 시각.
    requested: Option<Instant>,
    last_callback: Option<Instant>,
    slow_streak: u32,
    hidden: bool,
}

impl FrameWatch {
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    pub fn pending(&self) -> bool {
        self.requested.is_some()
    }

    /// 재생을 멈추거나 다시 시작할 때: 판정을 처음부터.
    pub fn reset(&mut self) -> Change {
        let was = self.hidden;
        *self = Self::default();
        if was { Change::Visible } else { Change::None }
    }

    /// 콜백을 요청했다. 이미 기다리는 중이면 처음 요청 시각을 유지한다.
    pub fn requested(&mut self, now: Instant) {
        self.requested.get_or_insert(now);
    }

    /// 타이머가 이 시각을 확인해야 한다.
    pub fn deadline(&self, timeout: Duration) -> Option<Instant> {
        self.requested.filter(|_| !self.hidden).map(|t| t + timeout)
    }

    pub fn on_callback(&mut self, now: Instant, timeout: Duration, slow_count: u32) -> Change {
        self.requested = None;
        let slow = self
            .last_callback
            .is_some_and(|last| now.duration_since(last) >= timeout);
        self.last_callback = Some(now);
        if self.hidden {
            if slow {
                return Change::None;
            }
            self.hidden = false;
            self.slow_streak = 0;
            return Change::Visible;
        }
        if slow {
            self.slow_streak += 1;
            if self.slow_streak >= slow_count {
                self.hidden = true;
                return Change::Hidden;
            }
        } else {
            self.slow_streak = 0;
        }
        Change::None
    }

    /// 타이머: 요청한 콜백이 시간 안에 오지 않았으면 Hidden.
    pub fn on_timer(&mut self, now: Instant, timeout: Duration) -> Change {
        match self.requested {
            Some(at) if !self.hidden && now.duration_since(at) >= timeout => {
                self.hidden = true;
                Change::Hidden
            }
            _ => Change::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_millis(1500);

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn missing_callback_hides_and_fast_callback_shows() {
        let t0 = Instant::now();
        let mut w = FrameWatch::default();
        w.requested(t0);
        assert_eq!(w.on_callback(ms(t0, 16), T, 3), Change::None);
        w.requested(ms(t0, 16));
        assert_eq!(w.on_timer(ms(t0, 1000), T), Change::None);
        assert_eq!(w.on_timer(ms(t0, 1600), T), Change::Hidden);
        assert_eq!(w.deadline(T), None, "no timer while hidden");
        // 가려진 동안 대기하던 콜백이 늦게 온다: 직전 콜백과 간격이 길어 아직 Hidden.
        assert_eq!(w.on_callback(ms(t0, 5000), T, 3), Change::None);
        assert!(w.hidden());
        // 이어서 빠른 콜백이 오면 보이는 것.
        w.requested(ms(t0, 5000));
        assert_eq!(w.on_callback(ms(t0, 5016), T, 3), Change::Visible);
    }

    #[test]
    fn slow_callbacks_hide() {
        let t0 = Instant::now();
        let mut w = FrameWatch::default();
        let mut t = 0;
        assert_eq!(w.on_callback(ms(t0, t), T, 3), Change::None);
        for i in 0..3 {
            t += 2000;
            let c = w.on_callback(ms(t0, t), T, 3);
            assert_eq!(c, if i == 2 { Change::Hidden } else { Change::None });
        }
    }

    #[test]
    fn reset_clears_hidden() {
        let t0 = Instant::now();
        let mut w = FrameWatch::default();
        w.requested(t0);
        w.on_timer(ms(t0, 2000), T);
        assert_eq!(w.reset(), Change::Visible);
        assert!(!w.pending());
    }
}
