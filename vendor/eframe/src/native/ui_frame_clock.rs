//! Predict UI animation time from actual UI frames, independently of logic wakes.

pub(super) struct UiFrameClock {
    last_ui_time: Option<f64>,
    last_ui_immediate: bool,
    cadence: f32,
}

impl UiFrameClock {
    pub(super) fn new(refresh_millihertz: Option<u32>) -> Self {
        let cadence = refresh_millihertz
            .filter(|rate| *rate > 0)
            .map_or(1.0 / 60.0, |rate| 1000.0 / rate as f32);
        Self {
            last_ui_time: None,
            last_ui_immediate: false,
            cadence,
        }
    }

    pub(super) fn begin_ui(&mut self, now: Option<f64>) -> f32 {
        let now = now.filter(|time| time.is_finite());
        if self.last_ui_immediate
            && let (Some(now), Some(previous)) = (now, self.last_ui_time)
        {
            let interval = now - previous;
            // Do not turn a pause, occlusion or a playback-only timer into an
            // animation timestep. The last continuous UI cadence survives it.
            if interval > 0.0 && interval <= 0.05 {
                self.cadence = interval as f32;
            }
        }
        self.last_ui_time = now;
        self.cadence
    }

    pub(super) fn end_ui(&mut self, requested_immediate: bool) {
        self.last_ui_immediate = requested_immediate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_refresh_ui_cadence_survives_many_logic_wakes() {
        for hz in [60.0, 120.0, 144.0, 240.0, 280.0, 360.0] {
            let mut clock = UiFrameClock::new(Some((hz * 1000.0) as u32));
            let _ = clock.begin_ui(Some(0.0));
            clock.end_ui(true);
            for frame in 1..20 {
                // CPU-only logic never calls either UI-clock method.
                let predicted = clock.begin_ui(Some(f64::from(frame) / hz));
                assert!((f64::from(predicted) - 1.0 / hz).abs() < 0.000_001);
                clock.end_ui(true);
            }
        }
    }

    #[test]
    fn idle_and_playback_timers_do_not_replace_the_learned_animation_cadence() {
        let mut clock = UiFrameClock::new(Some(240_000));
        let _ = clock.begin_ui(Some(0.0));
        clock.end_ui(true);
        let expected = clock.begin_ui(Some(1.0 / 240.0));
        clock.end_ui(false);
        for tick in 1..20 {
            assert_eq!(clock.begin_ui(Some(f64::from(tick) * 0.1)), expected);
            clock.end_ui(false);
        }
        assert_eq!(clock.begin_ui(Some(30.0)), expected);
        clock.end_ui(true);
        let resumed = clock.begin_ui(Some(30.0 + 1.0 / 240.0));
        assert!((resumed - expected).abs() < 0.000_001);
    }

    #[test]
    fn invalid_or_paused_time_does_not_poison_the_prediction() {
        let mut clock = UiFrameClock::new(None);
        let expected = clock.begin_ui(None);
        clock.end_ui(true);
        assert_eq!(clock.begin_ui(Some(f64::NAN)), expected);
        clock.end_ui(true);
        assert_eq!(clock.begin_ui(Some(1.0)), expected);
        clock.end_ui(true);
        assert_eq!(clock.begin_ui(Some(100.0)), expected);
    }
}
