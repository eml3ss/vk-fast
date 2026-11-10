//! Safe egui window chrome. Native movement and resizing stay in the window
//! manager so snapping, monitor scale and Wayland pointer serials remain native.
use eframe::egui::{self, Align, Color32, Context, Id, Layout, Rect, Response, Sense, Stroke};

use crate::{
    i18n::Language,
    icons::{self, Icon},
    theme,
};

pub const CONTROL_HEIGHT: f32 = 38.0;
pub const CONTROLS_WIDTH: f32 = 108.0;
const BUTTON_WIDTH: f32 = CONTROLS_WIDTH / 3.0;
const EDGE: f32 = 3.0;
const CORNER: f32 = 8.0;
/// How far caption buttons reach across a gap to the window's edge.
const EDGE_REACH: f32 = 12.0;

pub fn controls_width(ctx: &Context) -> f32 {
    if ctx.input(|input| input.viewport().fullscreen == Some(true)) {
        0.0
    } else {
        CONTROLS_WIDTH
    }
}

fn send_once(ctx: &Context, command: egui::ViewportCommand) {
    // Read context state before the data write; context calls inside data_mut
    // would recursively acquire the same lock.
    let frame = ctx.cumulative_frame_nr();
    let already_sent = ctx.data_mut(|data| {
        let id = Id::new("vk-fast-window-command-frame");
        let previous = data.get_temp::<u64>(id);
        data.insert_temp(id, frame);
        previous == Some(frame)
    });
    if !already_sent {
        ctx.send_viewport_cmd(command);
    }
}

/// Draw inline caption buttons without a separate title row or idle frames.
/// The caller reserves controls_width(ctx) and places this at the header edge.
pub fn controls(ui: &mut egui::Ui, language: Language) -> Rect {
    if controls_width(ui.ctx()) == 0.0 {
        return Rect::from_min_size(ui.cursor().min, egui::Vec2::ZERO);
    }
    let maximized = ui.input(|input| input.viewport().maximized == Some(true));
    // The buttons take clicks up to the window's top edge, and Close up to
    // its right edge too. A window that can be resized keeps its thin edge
    // and corner for resizing, as no native border lies outside it.
    let window = ui.ctx().viewport_rect();
    let resizable = resizable(ui.ctx());
    let inset = if resizable { EDGE } else { 0.0 };
    let resizing = resizable
        && ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|point| resize_direction(window, point).is_some());
    ui.allocate_ui_with_layout(
        egui::vec2(CONTROLS_WIDTH, CONTROL_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, label, command) in [
                (
                    0,
                    language.text("Свернуть окно", "Minimize window"),
                    egui::ViewportCommand::Minimized(true),
                ),
                (
                    1,
                    if maximized {
                        language.text("Восстановить окно", "Restore window")
                    } else {
                        language.text("Развернуть окно", "Maximize window")
                    },
                    egui::ViewportCommand::Maximized(!maximized),
                ),
                (
                    2,
                    language.text("Закрыть окно", "Close window"),
                    egui::ViewportCommand::Close,
                ),
            ] {
                let (rect, _) = ui
                    .allocate_exact_size(egui::vec2(BUTTON_WIDTH, CONTROL_HEIGHT), Sense::hover());
                // Only across the gap to the edge: buttons placed elsewhere
                // never grow over the rest of the caption.
                let top = window.top() + inset;
                let right = window.right() - inset;
                let hit = Rect::from_min_max(
                    egui::pos2(
                        rect.left(),
                        if rect.top() - top <= EDGE_REACH {
                            top.min(rect.top())
                        } else {
                            rect.top()
                        },
                    ),
                    egui::pos2(
                        if index == 2 && right - rect.right() <= EDGE_REACH {
                            right.max(rect.right())
                        } else {
                            rect.right()
                        },
                        rect.bottom(),
                    ),
                );
                let response =
                    ui.interact(hit, ui.id().with(("caption-button", index)), Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
                });
                let highlighted = (response.hovered() && !resizing) || response.has_focus();
                let background = if highlighted {
                    if index == 2 {
                        Color32::from_rgb(139, 47, 62)
                    } else {
                        theme::SELECTED
                    }
                } else {
                    theme::BACKGROUND
                };
                if highlighted {
                    ui.painter()
                        .rect_filled(rect.shrink2(egui::vec2(1.0, 3.0)), 7.0, background);
                }
                let color = if highlighted {
                    ui.visuals().text_color()
                } else {
                    theme::MUTED
                };
                let icon = Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0));
                match index {
                    0 => {
                        ui.painter().rect_filled(
                            Rect::from_center_size(icon.center(), egui::vec2(12.0, 1.8)),
                            0.9,
                            color,
                        );
                    }
                    1 => {
                        let stroke = Stroke::new(1.6, color);
                        let square = Rect::from_center_size(icon.center(), egui::vec2(11.0, 11.0));
                        if maximized {
                            ui.painter().rect_stroke(
                                square.translate(egui::vec2(1.5, -1.5)),
                                3.0,
                                stroke,
                                egui::StrokeKind::Inside,
                            );
                            let front = square.translate(egui::vec2(-1.5, 1.5));
                            ui.painter().rect_filled(front, 3.0, background);
                            ui.painter()
                                .rect_stroke(front, 3.0, stroke, egui::StrokeKind::Inside);
                        } else {
                            ui.painter()
                                .rect_stroke(square, 3.0, stroke, egui::StrokeKind::Inside);
                        }
                    }
                    _ => icons::paint(ui.painter(), icon, Icon::Close, color),
                }
                if response.clicked() && !resizing {
                    ui.ctx()
                        .memory_mut(|memory| memory.surrender_focus(response.id));
                    send_once(ui.ctx(), command);
                }
                response.on_hover_text(label);
            }
        },
    )
    .response
    .rect
}

const DOUBLE_PRESS_SECONDS: f64 = 0.45;
const DOUBLE_PRESS_DISTANCE: f32 = 8.0;

/// Register only a blank caption area, before placing interactive header
/// widgets on it. Movement begins on the press frame, not a later drag frame.
pub fn drag_region(ui: &egui::Ui, rect: Rect, id: Id) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    if !ui.is_enabled() || ui.input(|input| input.viewport().fullscreen == Some(true)) {
        return response;
    }
    if response.is_pointer_button_down_on()
        && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary))
    {
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let maximized = ui.input(|input| input.viewport().maximized == Some(true));
        // Count presses, not egui's clicks: the window drag that starts on a
        // press can swallow releases, and egui then reads a quick double
        // click as a triple one. Each double press is consumed, so a third
        // press starts over instead of cancelling it.
        let now = ui.input(|input| input.time);
        let last_key = id.with("last-press");
        let last: Option<(f64, egui::Pos2)> = ui.ctx().data(|data| data.get_temp(last_key));
        let double = last.zip(pointer).is_some_and(|((at, from), point)| {
            now - at <= DOUBLE_PRESS_SECONDS && from.distance(point) <= DOUBLE_PRESS_DISTANCE
        });
        if double {
            ui.ctx()
                .data_mut(|data| data.remove::<(f64, egui::Pos2)>(last_key));
            send_once(ui.ctx(), egui::ViewportCommand::Maximized(!maximized));
            return response;
        }
        if let Some(point) = pointer {
            ui.ctx()
                .data_mut(|data| data.insert_temp(last_key, (now, point)));
        }
        if maximized
            || pointer
                .is_none_or(|point| resize_direction(ui.ctx().viewport_rect(), point).is_none())
        {
            send_once(ui.ctx(), egui::ViewportCommand::StartDrag);
        }
    }
    response
}

fn resize_direction(bounds: Rect, pointer: egui::Pos2) -> Option<egui::ResizeDirection> {
    use egui::ResizeDirection::*;
    if !bounds.is_positive() || !bounds.contains(pointer) {
        return None;
    }
    let left = pointer.x - bounds.left();
    let right = bounds.right() - pointer.x;
    let top = pointer.y - bounds.top();
    let bottom = bounds.bottom() - pointer.y;
    if left <= CORNER && top <= CORNER {
        Some(NorthWest)
    } else if right <= CORNER && top <= CORNER {
        Some(NorthEast)
    } else if left <= CORNER && bottom <= CORNER {
        Some(SouthWest)
    } else if right <= CORNER && bottom <= CORNER {
        Some(SouthEast)
    } else if left <= EDGE {
        Some(West)
    } else if right <= EDGE {
        Some(East)
    } else if top <= EDGE {
        Some(North)
    } else if bottom <= EDGE {
        Some(South)
    } else {
        None
    }
}

/// Run after normal widgets, only while custom chrome is enabled. Hit-testing
/// narrow empty edges directly avoids a transparent foreground window that
/// would otherwise intercept the search field, music rows or playback controls.
/// Whether the window's edges resize it: a focused window that is neither
/// maximized nor fullscreen.
fn resizable(ctx: &Context) -> bool {
    ctx.input(|input| {
        input.focused
            && input.viewport().fullscreen != Some(true)
            && input.viewport().maximized != Some(true)
    })
}

pub fn resize(ctx: &Context) {
    let (pointer, pressed) = ctx.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.button_pressed(egui::PointerButton::Primary),
        )
    });
    if !resizable(ctx) {
        return;
    }
    let Some(direction) = pointer.and_then(|point| resize_direction(ctx.viewport_rect(), point))
    else {
        return;
    };
    let cursor = match direction {
        egui::ResizeDirection::NorthWest | egui::ResizeDirection::SouthEast => {
            egui::CursorIcon::ResizeNwSe
        }
        egui::ResizeDirection::NorthEast | egui::ResizeDirection::SouthWest => {
            egui::CursorIcon::ResizeNeSw
        }
        egui::ResizeDirection::North | egui::ResizeDirection::South => {
            egui::CursorIcon::ResizeVertical
        }
        egui::ResizeDirection::West | egui::ResizeDirection::East => {
            egui::CursorIcon::ResizeHorizontal
        }
    };
    ctx.set_cursor_icon(cursor);
    if pressed {
        send_once(ctx, egui::ViewportCommand::BeginResize(direction));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &Context,
        tick: u32,
        events: Vec<egui::Event>,
        maximized: bool,
        fullscreen: bool,
        ui: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            time: Some(f64::from(tick) * 0.1),
            focused: true,
            events,
            ..Default::default()
        };
        let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
        viewport.maximized = Some(maximized);
        viewport.fullscreen = Some(fullscreen);
        let mut output = ctx.run_ui(input, ui);
        output.textures_delta.clear();
        output
    }

    fn pointer(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn commands(output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .filter(|command| !matches!(command, egui::ViewportCommand::SetTheme(_)))
            .cloned()
            .collect()
    }

    #[test]
    fn caption_buttons_emit_native_commands_and_restore_maximized_windows() {
        for (index, maximized, expected) in [
            (0, false, egui::ViewportCommand::Minimized(true)),
            (1, false, egui::ViewportCommand::Maximized(true)),
            (1, true, egui::ViewportCommand::Maximized(false)),
            (2, false, egui::ViewportCommand::Close),
        ] {
            let ctx = Context::default();
            let render = |ui: &mut egui::Ui| {
                controls(ui, Language::English);
            };
            frame(&ctx, 1, Vec::new(), maximized, false, render);
            frame(&ctx, 2, Vec::new(), maximized, false, render);
            let point = egui::pos2(BUTTON_WIDTH * (index as f32 + 0.5), CONTROL_HEIGHT * 0.5);
            frame(&ctx, 3, pointer(point, true), maximized, false, render);
            let output = frame(&ctx, 4, pointer(point, false), maximized, false, render);
            assert_eq!(commands(&output), [expected]);
        }
    }

    #[test]
    fn caption_buttons_take_clicks_up_to_the_window_edges() {
        // Placed as in the toolbar: inset from the window's top-right corner.
        let render = |ui: &mut egui::Ui| {
            let rect = Rect::from_min_size(
                egui::pos2(640.0 - 6.0 - CONTROLS_WIDTH, 6.0),
                egui::vec2(CONTROLS_WIDTH, CONTROL_HEIGHT),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                controls(ui, Language::English);
            });
            resize(ui.ctx());
        };
        let click = |maximized: bool, point: egui::Pos2| {
            let ctx = Context::default();
            frame(&ctx, 1, Vec::new(), maximized, false, render);
            frame(&ctx, 2, Vec::new(), maximized, false, render);
            let mut sent = commands(&frame(
                &ctx,
                3,
                pointer(point, true),
                maximized,
                false,
                render,
            ));
            sent.extend(commands(&frame(
                &ctx,
                4,
                pointer(point, false),
                maximized,
                false,
                render,
            )));
            sent
        };
        use egui::{ResizeDirection, ViewportCommand};
        // Maximized: nothing resizes, so the very corner closes the window
        // and the very top edge reaches every button.
        assert_eq!(
            click(true, egui::pos2(639.5, 0.5)),
            [ViewportCommand::Close]
        );
        assert_eq!(
            click(true, egui::pos2(639.5, 25.0)),
            [ViewportCommand::Close]
        );
        assert_eq!(
            click(true, egui::pos2(640.0 - 6.0 - BUTTON_WIDTH * 1.5, 0.5)),
            [ViewportCommand::Maximized(false)]
        );
        assert_eq!(
            click(true, egui::pos2(640.0 - 6.0 - BUTTON_WIDTH * 2.5, 0.5)),
            [ViewportCommand::Minimized(true)]
        );
        // A resizable window keeps its thin edges and corner for resizing,
        // and the buttons take everything up to them.
        assert_eq!(
            click(false, egui::pos2(630.0, 4.0)),
            [ViewportCommand::Close]
        );
        assert_eq!(
            click(false, egui::pos2(636.0, 25.0)),
            [ViewportCommand::Close]
        );
        assert_eq!(
            click(false, egui::pos2(639.0, 1.0)),
            [ViewportCommand::BeginResize(ResizeDirection::NorthEast)]
        );
        assert_eq!(
            click(false, egui::pos2(639.0, 25.0)),
            [ViewportCommand::BeginResize(ResizeDirection::East)]
        );
        assert_eq!(
            click(false, egui::pos2(600.0, 1.0)),
            [ViewportCommand::BeginResize(ResizeDirection::North)]
        );
    }

    #[test]
    fn caption_drag_starts_on_press_and_double_click_toggles_maximization() {
        let ctx = Context::default();
        let render = |ui: &mut egui::Ui| {
            drag_region(
                ui,
                Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(300.0, 38.0)),
                Id::new("test-caption"),
            );
        };
        frame(&ctx, 1, Vec::new(), false, false, render);
        let point = egui::pos2(100.0, 20.0);
        let press = frame(&ctx, 2, pointer(point, true), false, false, render);
        assert_eq!(commands(&press), [egui::ViewportCommand::StartDrag]);
        frame(&ctx, 3, pointer(point, false), false, false, render);
        // The second press toggles at once, without another drag.
        let second = frame(&ctx, 4, pointer(point, true), false, false, render);
        assert_eq!(commands(&second), [egui::ViewportCommand::Maximized(true)]);
        let release = frame(&ctx, 5, pointer(point, false), false, false, render);
        assert!(commands(&release).is_empty());
        // A quick third press starts over instead of undoing the toggle.
        let third = frame(&ctx, 6, pointer(point, true), true, false, render);
        assert_eq!(commands(&third), [egui::ViewportCommand::StartDrag]);
        frame(&ctx, 7, pointer(point, false), true, false, render);
    }

    #[test]
    fn caption_double_press_toggles_even_when_the_drag_swallowed_a_release() {
        let ctx = Context::default();
        let render = |ui: &mut egui::Ui| {
            drag_region(
                ui,
                Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(300.0, 38.0)),
                Id::new("test-caption"),
            );
        };
        frame(&ctx, 1, Vec::new(), false, false, render);
        let point = egui::pos2(100.0, 20.0);
        let press = frame(&ctx, 2, pointer(point, true), false, false, render);
        assert_eq!(commands(&press), [egui::ViewportCommand::StartDrag]);
        // The native move loop kept the release; the next event is a press.
        let second = frame(&ctx, 3, pointer(point, true), false, false, render);
        assert_eq!(commands(&second), [egui::ViewportCommand::Maximized(true)]);
        // Presses farther apart in time are two separate drags.
        frame(&ctx, 4, pointer(point, false), true, false, render);
        let late = frame(&ctx, 12, pointer(point, true), true, false, render);
        assert_eq!(commands(&late), [egui::ViewportCommand::StartDrag]);
    }

    #[test]
    fn caption_click_ownership_keeps_header_buttons_out_of_native_dragging() {
        for (point, pressed_command, released_command) in [
            (
                egui::pos2(18.0, 19.0),
                None,
                Some(egui::ViewportCommand::Minimized(true)),
            ),
            (egui::pos2(170.0, 20.0), None, None),
            (
                egui::pos2(400.0, 20.0),
                Some(egui::ViewportCommand::StartDrag),
                None,
            ),
        ] {
            let ctx = Context::default();
            let render = |ui: &mut egui::Ui| {
                drag_region(
                    ui,
                    Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 50.0)),
                    Id::new("background-caption"),
                );
                controls(ui, Language::English);
                ui.put(
                    Rect::from_min_size(egui::pos2(120.0, 5.0), egui::vec2(100.0, 30.0)),
                    egui::Button::new("Home"),
                );
            };
            frame(&ctx, 1, Vec::new(), false, false, render);
            frame(&ctx, 2, Vec::new(), false, false, render);
            let press = frame(&ctx, 3, pointer(point, true), false, false, render);
            assert_eq!(
                commands(&press),
                pressed_command.into_iter().collect::<Vec<_>>()
            );
            let release = frame(&ctx, 4, pointer(point, false), false, false, render);
            assert_eq!(
                commands(&release),
                released_command.into_iter().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn resize_captures_only_edges_and_skips_maximized_and_fullscreen_windows() {
        for (point, expected) in [
            (egui::pos2(2.0, 180.0), Some(egui::ResizeDirection::West)),
            (
                egui::pos2(639.0, 2.0),
                Some(egui::ResizeDirection::NorthEast),
            ),
            (egui::pos2(200.0, 358.0), Some(egui::ResizeDirection::South)),
            (egui::pos2(100.0, 20.0), None),
        ] {
            for (maximized, fullscreen) in [(false, false), (true, false), (false, true)] {
                let ctx = Context::default();
                let output = frame(&ctx, 1, pointer(point, true), maximized, fullscreen, |ui| {
                    resize(ui.ctx())
                });
                let wanted = expected
                    .filter(|_| !maximized && !fullscreen)
                    .map(egui::ViewportCommand::BeginResize);
                assert_eq!(commands(&output), wanted.into_iter().collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn fullscreen_does_not_reserve_buttons_or_start_caption_drags() {
        let ctx = Context::default();
        let output = frame(
            &ctx,
            1,
            pointer(egui::pos2(80.0, 20.0), true),
            false,
            true,
            |ui| {
                assert_eq!(controls_width(ui.ctx()), 0.0);
                assert_eq!(controls(ui, Language::English).width(), 0.0);
                drag_region(
                    ui,
                    Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 40.0)),
                    Id::new("fullscreen-caption"),
                );
                resize(ui.ctx());
            },
        );
        assert!(commands(&output).is_empty());
    }
}
