//! Bottom-right notices fade out the same way egui fades their area in:
//! over the style's animation time, with the fade-in curve run backwards.
use super::*;

fn fade_time(ctx: &egui::Context) -> f32 {
    ctx.global_style().animation_time.max(0.0)
}

/// The notice's opacity before it hides at `until`. Schedules the repaint
/// that starts the fade and keeps repainting while it runs.
pub(super) fn opacity(ctx: &egui::Context, until: Instant) -> f32 {
    let time = fade_time(ctx);
    let remaining = until.saturating_duration_since(Instant::now());
    let steady = remaining.saturating_sub(Duration::from_secs_f32(time));
    if !steady.is_zero() || time == 0.0 {
        ctx.request_repaint_after(if time == 0.0 { remaining } else { steady });
        return 1.0;
    }
    ctx.request_repaint();
    egui::emath::easing::quadratic_out(remaining.as_secs_f32() / time)
}

/// A dismissed notice hides after the fade, unless it ends sooner anyway.
pub(super) fn dismiss(ctx: &egui::Context, until: &mut Instant) {
    *until = (*until).min(Instant::now() + Duration::from_secs_f32(fade_time(ctx)));
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_stay_opaque_then_fade_with_the_fade_in_curve() {
        let ctx = egui::Context::default();
        let time = fade_time(&ctx);
        assert!(time > 0.0);
        let now = Instant::now();
        assert_eq!(opacity(&ctx, now + Duration::from_secs(3)), 1.0);
        let half = opacity(&ctx, now + Duration::from_secs_f32(time / 2.0));
        assert!(half > 0.0 && half < 1.0);
        assert_eq!(opacity(&ctx, now), 0.0);
        let mut until = now + Duration::from_secs(3);
        dismiss(&ctx, &mut until);
        assert!(until <= Instant::now() + Duration::from_secs_f32(time));
        let mut soon = now;
        dismiss(&ctx, &mut soon);
        assert_eq!(soon, now, "dismissing never extends a notice");
    }
}
