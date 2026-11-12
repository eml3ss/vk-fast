//! Middle-click autoscroll, as in browsers: a middle click on the page sets
//! an anchor, and the list under the anchor scrolls toward the pointer,
//! faster the farther the pointer is, wherever the pointer goes meanwhile.
//! Holding the middle button and dragging works the same and ends on
//! release. Any other click, a second middle click or Esc stops it. Only the
//! system cursor shows it is on. Off unless enabled in Settings.
use eframe::egui::{self, Context, PointerButton, Pos2};

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
    /// The pointer's last place, kept while it is outside the window.
    pointer: Option<Pos2>,
    cursor: Option<egui::CursorIcon>,
}

/// This frame's scroll for the list under the anchor.
#[derive(Clone, Copy)]
struct Pending {
    anchor: Pos2,
    delta: f32,
}

fn pending_id() -> egui::Id {
    egui::Id::new("middle-autoscroll-pending")
}

/// Page Up/Down or Home, asked for at the end of a frame and carried out in
/// the next one by the innermost vertical list that shows `anchor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyScroll {
    /// About a screen down (1) or up (-1).
    Page(f32),
    Top,
}

#[derive(Clone, Copy)]
struct KeyRequest {
    anchor: Pos2,
    scroll: KeyScroll,
    frame: u64,
}

fn key_request_id() -> egui::Id {
    egui::Id::new("keyboard-page-scroll")
}

/// Ask the list at `anchor` to scroll by a page or to its top next frame.
pub fn request_key_scroll(ctx: &Context, anchor: Pos2, scroll: KeyScroll) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        data.insert_temp(
            key_request_id(),
            KeyRequest {
                anchor,
                scroll,
                frame,
            },
        );
    });
    ctx.request_repaint();
}

/// The keyboard scroll for this list, if it is the innermost one that shows
/// the anchor and can scroll vertically: call it after the list's contents.
/// A request no list took in the frame after it is dropped.
pub fn take_key_scroll_for(ui: &egui::Ui) -> Option<KeyScroll> {
    let request = ui
        .ctx()
        .data(|data| data.get_temp::<KeyRequest>(key_request_id()))?;
    if request.frame + 1 < ui.ctx().cumulative_frame_nr() {
        ui.ctx()
            .data_mut(|data| data.remove::<KeyRequest>(key_request_id()));
        return None;
    }
    // A sideways carousel shows the anchor too, but has nothing to scroll
    // up or down.
    let vertical = ui.min_rect().height() > ui.clip_rect().height() + 1.0;
    if !vertical || !ui.clip_rect().contains(request.anchor) {
        return None;
    }
    ui.ctx()
        .data_mut(|data| data.remove::<KeyRequest>(key_request_id()));
    Some(request.scroll)
}

/// This frame's autoscroll, for the innermost list that shows the anchor:
/// call it after the list's contents. Other lists get nothing.
pub fn take_for(ui: &egui::Ui) -> Option<f32> {
    let pending = ui
        .ctx()
        .data(|data| data.get_temp::<Pending>(pending_id()))?;
    if !ui.clip_rect().contains(pending.anchor) {
        return None;
    }
    ui.ctx()
        .data_mut(|data| data.remove::<Pending>(pending_id()));
    Some(pending.delta)
}

impl Autoscroll {
    #[cfg(test)]
    pub fn active(&self) -> bool {
        self.anchor.is_some()
    }

    /// Call first in the frame, before any list is laid out. `area` is
    /// where a middle click may start it.
    pub fn apply(&mut self, ctx: &Context, enabled: bool, area: egui::Rect) {
        ctx.data_mut(|data| data.remove::<Pending>(pending_id()));
        self.cursor = None;
        if !enabled {
            self.anchor = None;
            return;
        }
        let (hover, middle_pressed, other_pressed, middle_released, middle_down, escape, dt) = ctx
            .input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.pointer.button_pressed(PointerButton::Middle),
                    input.pointer.button_pressed(PointerButton::Primary)
                        || input.pointer.button_pressed(PointerButton::Secondary),
                    input.pointer.button_released(PointerButton::Middle),
                    input.pointer.button_down(PointerButton::Middle),
                    input.key_pressed(egui::Key::Escape),
                    input.stable_dt.min(0.1),
                )
            });
        if hover.is_some() {
            self.pointer = hover;
        }
        let Some(anchor) = self.anchor else {
            if middle_pressed && let Some(point) = hover.filter(|point| area.contains(*point)) {
                self.anchor = Some(point);
                self.dragged = false;
                self.cursor = Some(egui::CursorIcon::AllScroll);
                ctx.request_repaint();
            }
            return;
        };
        let stop = escape || other_pressed || middle_pressed || (middle_released && self.dragged);
        let Some(pointer) = self.pointer.filter(|_| !stop) else {
            self.anchor = None;
            return;
        };
        let offset = pointer.y - anchor.y;
        if offset.abs() > DRAG && middle_down {
            self.dragged = true;
        }
        let distance = (offset.abs() - DEAD_ZONE).max(0.0);
        if distance > 0.0 {
            // Below the anchor scrolls down, like a wheel pulled toward you.
            let delta = -offset.signum() * distance * SPEED * dt;
            ctx.data_mut(|data| data.insert_temp(pending_id(), Pending { anchor, delta }));
        }
        self.cursor = Some(if distance == 0.0 {
            egui::CursorIcon::AllScroll
        } else if offset > 0.0 {
            egui::CursorIcon::ResizeSouth
        } else {
            egui::CursorIcon::ResizeNorth
        });
        ctx.request_repaint();
    }

    /// Call last in the frame: the system cursor shows the autoscroll over
    /// whatever widget the pointer is on.
    pub fn show_cursor(&self, ctx: &Context) {
        if let Some(cursor) = self.cursor {
            ctx.set_cursor_icon(cursor);
        }
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
                delta = take_for(ui).unwrap_or(0.0);
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
