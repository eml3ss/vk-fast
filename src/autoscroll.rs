//! Middle-click autoscroll, as in browsers: a middle click on the page sets
//! an anchor, and the page under it scrolls toward the pointer, faster the
//! farther the pointer is. Any click, Esc, or releasing the middle button
//! after dragging stops it. Off unless enabled in Settings.
use eframe::egui::{self, Color32, Context, PointerButton, Pos2, Stroke};

/// Pointer distance around the anchor that does not scroll, in points.
const DEAD_ZONE: f32 = 10.0;
/// Scroll speed per point of distance past the dead zone, points per second.
const SPEED: f32 = 7.0;
/// A middle press dragged farther than this ends on release.
const DRAG: f32 = 12.0;

#[derive(Default)]
pub struct Autoscroll {
    anchor: Option<Pos2>,
    dragged: bool,
}

impl Autoscroll {
    #[cfg(test)]
    pub fn active(&self) -> bool {
        self.anchor.is_some()
    }

    /// Call first in the frame, after wheel handling and before any scroll
    /// area reads the input. `area` is where a middle click may start it.
    pub fn apply(&mut self, ctx: &Context, enabled: bool, area: egui::Rect) {
        if !enabled {
            self.anchor = None;
            return;
        }
        let (pointer, middle_pressed, other_pressed, middle_released, escape, dt) =
            ctx.input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.pointer.button_pressed(PointerButton::Middle),
                    input.pointer.button_pressed(PointerButton::Primary)
                        || input.pointer.button_pressed(PointerButton::Secondary),
                    input.pointer.button_released(PointerButton::Middle),
                    input.key_pressed(egui::Key::Escape),
                    input.stable_dt.min(0.1),
                )
            });
        let Some(anchor) = self.anchor else {
            if middle_pressed && let Some(point) = pointer.filter(|point| area.contains(*point)) {
                self.anchor = Some(point);
                self.dragged = false;
                ctx.request_repaint();
            }
            return;
        };
        let stop = escape
            || other_pressed
            || middle_pressed
            || (middle_released && self.dragged)
            || pointer.is_none();
        if stop {
            self.anchor = None;
            return;
        }
        let Some(pointer) = pointer else { return };
        let offset = pointer.y - anchor.y;
        if offset.abs() > DRAG {
            self.dragged |= ctx.input(|input| input.pointer.button_down(PointerButton::Middle));
        }
        let distance = (offset.abs() - DEAD_ZONE).max(0.0);
        if distance > 0.0 {
            // Below the anchor scrolls down, like a wheel pulled toward you.
            let delta = -offset.signum() * distance * SPEED * dt;
            ctx.input_mut(|input| input.smooth_scroll_delta.y += delta);
        }
        ctx.set_cursor_icon(if distance == 0.0 {
            egui::CursorIcon::AllScroll
        } else if offset > 0.0 {
            egui::CursorIcon::ResizeSouth
        } else {
            egui::CursorIcon::ResizeNorth
        });
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("middle-autoscroll"),
        ));
        painter.circle(
            anchor,
            11.0,
            Color32::from_black_alpha(170),
            Stroke::new(1.5, Color32::from_white_alpha(200)),
        );
        for direction in [-1.0_f32, 1.0] {
            let tip = anchor + egui::vec2(0.0, 6.5 * direction);
            let base = anchor + egui::vec2(0.0, 2.5 * direction);
            painter.line_segment(
                [base + egui::vec2(-3.0, 0.0), tip],
                Stroke::new(1.5, Color32::WHITE),
            );
            painter.line_segment(
                [base + egui::vec2(3.0, 0.0), tip],
                Stroke::new(1.5, Color32::WHITE),
            );
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &Context,
        autoscroll: &mut Autoscroll,
        time: f64,
        events: Vec<egui::Event>,
    ) -> f32 {
        let mut delta = 0.0;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                autoscroll.apply(ui.ctx(), true, ui.ctx().content_rect());
                delta = ui.input(|input| input.smooth_scroll_delta.y);
            },
        );
        output.textures_delta.clear();
        delta
    }

    fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn middle_click_scrolls_toward_the_pointer_until_another_click() {
        let ctx = Context::default();
        let mut autoscroll = Autoscroll::default();
        let anchor = egui::pos2(300.0, 200.0);
        frame(
            &ctx,
            &mut autoscroll,
            0.0,
            button(anchor, PointerButton::Middle, true),
        );
        frame(
            &ctx,
            &mut autoscroll,
            0.02,
            button(anchor, PointerButton::Middle, false),
        );
        assert!(autoscroll.active(), "a click without dragging keeps it on");
        let still = frame(
            &ctx,
            &mut autoscroll,
            0.04,
            vec![egui::Event::PointerMoved(anchor)],
        );
        assert_eq!(still, 0.0, "inside the dead zone nothing scrolls");
        let below = egui::pos2(300.0, 300.0);
        let down = frame(
            &ctx,
            &mut autoscroll,
            0.06,
            vec![egui::Event::PointerMoved(below)],
        );
        assert!(down < 0.0, "below the anchor scrolls the page down");
        let up = frame(
            &ctx,
            &mut autoscroll,
            0.08,
            vec![egui::Event::PointerMoved(egui::pos2(300.0, 100.0))],
        );
        assert!(up > 0.0);
        frame(
            &ctx,
            &mut autoscroll,
            0.1,
            button(below, PointerButton::Primary, true),
        );
        assert!(!autoscroll.active(), "any click stops it");
    }

    #[test]
    fn disabled_or_outside_the_page_never_starts() {
        let ctx = Context::default();
        let mut autoscroll = Autoscroll::default();
        let anchor = egui::pos2(300.0, 200.0);
        ctx.run_ui(
            egui::RawInput {
                events: button(anchor, PointerButton::Middle, true),
                ..Default::default()
            },
            |ui| autoscroll.apply(ui.ctx(), false, ui.ctx().content_rect()),
        )
        .textures_delta
        .clear();
        assert!(!autoscroll.active());
        ctx.run_ui(
            egui::RawInput {
                events: button(anchor, PointerButton::Middle, true),
                ..Default::default()
            },
            |ui| {
                autoscroll.apply(
                    ui.ctx(),
                    true,
                    egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(10.0, 10.0)),
                )
            },
        )
        .textures_delta
        .clear();
        assert!(!autoscroll.active());
    }
}
