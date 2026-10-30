//! Presentation-only horizontal transition between search result categories.
#[derive(Clone, Default)]
pub struct Swipe {
    active: Option<Transition>,
}

#[derive(Clone, Copy)]
struct Transition {
    from: usize,
    to: usize,
    started: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub outgoing: usize,
    pub incoming: usize,
    pub outgoing_x: f32,
    pub incoming_x: f32,
}

impl Swipe {
    pub fn start(&mut self, from: usize, to: usize, now: f64) {
        self.active = (from != to && now.is_finite()).then_some(Transition {
            from,
            to,
            started: now,
        });
    }

    pub fn clear(&mut self) {
        self.active = None;
    }

    pub fn frame(&mut self, now: f64, width: f32) -> Option<Frame> {
        let active = self.active?;
        if !now.is_finite() || !width.is_finite() || width <= 0.0 || now - active.started >= 0.18 {
            self.clear();
            return None;
        }
        let t = ((now - active.started) / 0.18).clamp(0.0, 1.0) as f32;
        let progress = 1.0 - (1.0 - t).powi(3);
        let direction = if active.to > active.from { 1.0 } else { -1.0 };
        Some(Frame {
            outgoing: active.from,
            incoming: active.to,
            outgoing_x: -direction * width * progress,
            incoming_x: direction * width * (1.0 - progress),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_slide_in_opposite_directions_and_finish_without_a_timer() {
        let mut swipe = Swipe::default();
        swipe.start(0, 2, 10.0);
        let first = swipe.frame(10.0, 500.0).unwrap();
        assert_eq!((first.outgoing, first.incoming), (0, 2));
        assert_eq!((first.outgoing_x, first.incoming_x), (0.0, 500.0));
        let middle = swipe.frame(10.09, 500.0).unwrap();
        assert!(middle.outgoing_x < 0.0 && middle.incoming_x > 0.0);
        assert!((middle.incoming_x - middle.outgoing_x - 500.0).abs() < 0.01);
        assert!(swipe.frame(10.19, 500.0).is_none());
        swipe.start(3, 1, 11.0);
        let reverse = swipe.frame(11.09, 500.0).unwrap();
        assert!(reverse.outgoing_x > 0.0 && reverse.incoming_x < 0.0);
    }

    #[test]
    fn a_new_selection_retargets_and_invalid_geometry_or_queries_cancel() {
        let mut swipe = Swipe::default();
        swipe.start(0, 1, 0.0);
        swipe.start(1, 3, 0.05);
        let frame = swipe.frame(0.05, 300.0).unwrap();
        assert_eq!((frame.outgoing, frame.incoming), (1, 3));
        assert!(swipe.frame(0.06, f32::NAN).is_none());
        swipe.start(1, 1, 0.0);
        assert!(swipe.frame(0.0, 300.0).is_none());
        swipe.start(1, 2, 0.0);
        swipe.clear();
        assert!(swipe.frame(0.1, 300.0).is_none());
    }
}
