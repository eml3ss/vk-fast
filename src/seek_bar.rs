//! Matching rounded seek and volume controls without a separate buffer layer.
use eframe::egui::{self, Color32, Response, Sense};

use crate::theme;

pub const CONTROL_HEIGHT: f32 = 22.0;
const TRACK_HEIGHT: f32 = 7.0;
const KNOB_RADIUS: f32 = 8.0;
const REMAINING: Color32 = Color32::from_rgb(43, 49, 59);

pub struct Progress<'a> {
    pub duration: f64,
    pub width: f32,
    pub label: &'a str,
    /// The track is being fetched or buffered and isn't playing yet: a
    /// soft glow sweeps along the bar.
    pub loading: bool,
}

/// One sweep of the loading glow, in seconds.
const LOADING_SWEEP: f64 = 1.3;

pub fn show(ui: &mut egui::Ui, position: &mut f64, progress: Progress<'_>) -> Response {
    let duration = if progress.duration.is_finite() {
        progress.duration.max(1.0)
    } else {
        1.0
    };
    let id = ui.id().with("seek-loading");
    let loading = ui.ctx().animate_bool_with_time(id, progress.loading, 0.25);
    if loading > 0.0 {
        ui.ctx().request_repaint();
    }
    control(
        ui,
        position,
        duration,
        5.0,
        progress.width,
        progress.label,
        loading,
    )
}

/// Scrolled points per 5 % of volume: about one wheel notch.
const WHEEL_POINTS_PER_STEP: f32 = 40.0;

pub fn volume(ui: &mut egui::Ui, value: &mut f32, width: f32, label: &str) -> Response {
    let mut precise = f64::from(*value);
    let mut response = control(ui, &mut precise, 1.0, 0.05, width, label, 0.0);
    if response.changed() {
        *value = precise as f32;
    }
    // The wheel over the slider turns the volume: up louder, down quieter,
    // a notch per 5 %, and a touchpad in proportion. Ctrl + wheel stays zoom.
    if ui.is_enabled() && response.hovered() && !response.dragged() {
        let delta: f32 = ui.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                        ..
                    } if !modifiers.command && !modifiers.ctrl => {
                        let points = match unit {
                            egui::MouseWheelUnit::Point => 1.0,
                            egui::MouseWheelUnit::Line => WHEEL_POINTS_PER_STEP,
                            egui::MouseWheelUnit::Page => WHEEL_POINTS_PER_STEP * 4.0,
                        };
                        Some((delta.y + delta.x) * points)
                    }
                    _ => None,
                })
                .sum()
        });
        if delta != 0.0 {
            // Taken here: the page under the player does not scroll too.
            ui.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
            let next = (*value + delta / WHEEL_POINTS_PER_STEP * 0.05).clamp(0.0, 1.0);
            if next != *value {
                *value = next;
                response.mark_changed();
            }
        }
    }
    response
}

/// Converts the final pointer location of an already captured seek gesture.
/// The caller decides whether a release belongs to this control.
pub(crate) fn position_at_pointer(response: &Response, duration: f64) -> Option<f64> {
    if !duration.is_finite() || duration <= 0.0 {
        return None;
    }
    let position = response.ctx.input(|input| input.pointer.interact_pos())?;
    let position = response
        .ctx
        .layer_transform_from_global(response.layer_id)
        .map_or(position, |transform| transform * position);
    let track = track_rect(response.rect);
    Some(f64::from(((position.x - track.left()) / track.width()).clamp(0.0, 1.0)) * duration)
}

fn track_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(
        rect.center(),
        egui::vec2((rect.width() - KNOB_RADIUS * 2.0).max(1.0), TRACK_HEIGHT),
    )
}

fn control(
    ui: &mut egui::Ui,
    value: &mut f64,
    maximum: f64,
    step: f64,
    width: f32,
    label: &str,
    loading: f32,
) -> Response {
    let width = if width.is_finite() {
        width.max(KNOB_RADIUS * 2.0 + 1.0)
    } else {
        ui.spacing().slider_width
    };
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), Sense::click_and_drag());
    let track = track_rect(rect);
    let previous = *value;
    let primary_released =
        ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary));
    let pointer_owned = (response.is_pointer_button_down_on()
        && ui.input(|input| input.pointer.primary_down()))
        || response.dragged_by(egui::PointerButton::Primary);
    let final_pointer = primary_released && response.drag_stopped();
    if ui.is_enabled() {
        *value = bounded(*value, maximum);
        if (pointer_owned || response.clicked() || final_pointer)
            && let Some(pointer) = response.interact_pointer_pos()
        {
            *value =
                f64::from(((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0)) * maximum;
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            ui.input(|input| {
                let decrement = input.num_presses(egui::Key::ArrowLeft)
                    + input.num_presses(egui::Key::ArrowDown);
                let increment = input.num_presses(egui::Key::ArrowRight)
                    + input.num_presses(egui::Key::ArrowUp);
                *value += (increment as f64 - decrement as f64) * step;
                if input.key_pressed(egui::Key::Home) {
                    *value = 0.0;
                }
                if input.key_pressed(egui::Key::End) {
                    *value = maximum;
                }
            });
        }
        ui.input(|input| {
            use egui::accesskit::{Action, ActionData};
            let decrement = input.num_accesskit_action_requests(response.id, Action::Decrement);
            let increment = input.num_accesskit_action_requests(response.id, Action::Increment);
            *value += (increment as f64 - decrement as f64) * step;
            for request in input.accesskit_action_requests(response.id, Action::SetValue) {
                if let Some(ActionData::NumericValue(requested)) = request.data {
                    *value = bounded(requested, maximum);
                }
            }
        });
        *value = bounded(*value, maximum);
        if *value != previous {
            response.mark_changed();
        }
    }

    let fraction = (bounded(*value, maximum) / maximum) as f32;
    let has_focus = response.has_focus();
    let keyboard_navigation = has_focus
        && ui.input(|input| {
            [
                egui::Key::Tab,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::Home,
                egui::Key::End,
                egui::Key::Enter,
                egui::Key::Space,
            ]
            .into_iter()
            .any(|key| input.key_pressed(key))
        });
    let focus_origin = response.id.with("pointer-focus");
    let pointer_interaction = ui.is_enabled()
        && (pointer_owned || response.clicked_by(egui::PointerButton::Primary) || final_pointer);
    let pointer_focus = ui.ctx().data_mut(|data| {
        if !has_focus {
            data.remove_temp::<bool>(focus_origin);
            false
        } else if pointer_interaction {
            data.insert_temp(focus_origin, true);
            true
        } else if keyboard_navigation {
            data.insert_temp(focus_origin, false);
            false
        } else {
            data.get_temp::<bool>(focus_origin).unwrap_or(false)
        }
    });
    let pointer_over_control = response.hovered()
        && ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|position| {
                let local = ui
                    .ctx()
                    .layer_transform_from_global(response.layer_id)
                    .map_or(position, |transform| transform * position);
                response.rect.contains(local)
            });
    if ui.is_rect_visible(rect) {
        let opacity = if ui.is_enabled() { 1.0 } else { 0.4 };
        let painter = ui.painter();
        painter.rect_filled(
            track,
            TRACK_HEIGHT * 0.5,
            theme::tone(REMAINING).gamma_multiply(opacity),
        );
        if fraction > 0.0 {
            let end = track.left() + track.width() * fraction;
            let cap_width = TRACK_HEIGHT.min(track.width());
            let prefix = egui::Rect::from_min_max(
                track.min,
                egui::pos2(end.max(track.left() + cap_width), track.bottom()),
            );
            // A narrow rectangle would clamp the corner radius and paint blue
            // outside the track's left cap. Keep the full capsule geometry and
            // reveal only the actual played width until the prefix is wider.
            let played_painter = if end < track.left() + cap_width {
                let mut clip = painter.clip_rect();
                clip.max.x = clip.max.x.min(end);
                painter.with_clip_rect(clip)
            } else {
                painter.clone()
            };
            played_painter.rect_filled(
                prefix,
                TRACK_HEIGHT * 0.5,
                theme::accent().gamma_multiply(opacity),
            );
        }
        if loading > 0.0 {
            let time = ui.input(|input| input.time);
            painter.add(loading_glow(track, time, loading));
        }
        let show_knob = ui.is_enabled()
            && (pointer_over_control
                || response.dragged_by(egui::PointerButton::Primary)
                || (has_focus && !pointer_focus));
        if show_knob {
            let center = egui::pos2(track.left() + track.width() * fraction, track.center().y);
            painter.circle_filled(center, KNOB_RADIUS, Color32::WHITE.gamma_multiply(opacity));
            let outline = theme::handle_stroke();
            if outline != egui::Stroke::NONE {
                painter.circle_stroke(center, KNOB_RADIUS - 0.5, outline);
            }
        }
    }
    response.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *value, label));
    ui.ctx().accesskit_node_builder(response.id, |builder| {
        use egui::accesskit::Action;
        builder.set_min_numeric_value(0.0);
        builder.set_max_numeric_value(maximum);
        builder.set_numeric_value_step(step);
        if ui.is_enabled() {
            builder.add_action(Action::SetValue);
            if *value > 0.0 {
                builder.add_action(Action::Decrement);
            }
            if *value < maximum {
                builder.add_action(Action::Increment);
            }
        }
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The loading glow at `time`: an accent band with soft edges that sweeps
/// left to right across the bar, eased so it slows at both ends, drawn as
/// a mesh following the bar's capsule so its rounded ends stay round.
/// `strength` fades it in and out.
fn loading_glow(track: egui::Rect, time: f64, strength: f32) -> egui::Shape {
    const COLUMNS: usize = 96;
    let phase = (time / LOADING_SWEEP).fract() as f32;
    let eased = phase * phase * (3.0 - 2.0 * phase);
    let band = (track.width() * 0.4).max(TRACK_HEIGHT * 2.0);
    let center = track.left() - band * 0.5 + (track.width() + band) * eased;
    let radius = (track.height() * 0.5).min(track.width() * 0.5);
    let color = theme::accent();
    let mut mesh = egui::Mesh::default();
    for column in 0..=COLUMNS {
        let x = track.left() + track.width() * column as f32 / COLUMNS as f32;
        // The capsule's half height here: full in the middle, round at the
        // ends.
        let edge = (x - track.left()).min(track.right() - x).clamp(0.0, radius);
        let half = (radius * radius - (radius - edge).powi(2)).max(0.0).sqrt();
        let distance = ((x - center) / (band * 0.5)).abs();
        let glow = if distance < 1.0 {
            let fall = 1.0 - distance;
            fall * fall * (3.0 - 2.0 * fall)
        } else {
            0.0
        };
        let tint = color.gamma_multiply(glow * strength * 0.85);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(x, track.center().y - half), tint);
        mesh.colored_vertex(egui::pos2(x, track.center().y + half), tint);
        if column > 0 {
            mesh.add_triangle(base - 2, base - 1, base);
            mesh.add_triangle(base - 1, base, base + 1);
        }
    }
    egui::Shape::mesh(mesh)
}

fn bounded(value: f64, maximum: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, maximum)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum Kind {
        Seek,
        Volume,
    }

    fn frame(
        context: &egui::Context,
        kind: Kind,
        value: &mut f64,
        enabled: bool,
        tick: u32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Response) {
        let mut response = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(240.0, 100.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| {
                response = Some(
                    ui.add_enabled_ui(enabled, |ui| match kind {
                        Kind::Seek => show(
                            ui,
                            value,
                            Progress {
                                duration: 100.0,
                                width: 200.0,
                                label: "Seek",
                                loading: false,
                            },
                        ),
                        Kind::Volume => {
                            let mut level = *value as f32;
                            let response = volume(ui, &mut level, 200.0, "Volume");
                            *value = f64::from(level);
                            response
                        }
                    })
                    .inner,
                );
            },
        );
        output.textures_delta.clear();
        (output, response.unwrap())
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
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

    fn white_knob(shapes: &[egui::epaint::ClippedShape]) -> bool {
        shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Circle(circle) => circle.fill == Color32::WHITE && circle.radius == 8.0,
            _ => false,
        })
    }

    // Sample tessellated triangles in paint order, including their clipping.
    // This tests the visible join rather than relying on a particular Shape.
    fn color_at(primitives: &[egui::ClippedPrimitive], point: egui::Pos2) -> Color32 {
        let mut destination = [0.0_f32; 4];
        for primitive in primitives {
            if !primitive.clip_rect.contains(point) {
                continue;
            }
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let a = mesh.vertices[triangle[0] as usize];
                let b = mesh.vertices[triangle[1] as usize];
                let c = mesh.vertices[triangle[2] as usize];
                let cross =
                    |left: egui::Vec2, right: egui::Vec2| left.x * right.y - left.y * right.x;
                let area = cross(b.pos - a.pos, c.pos - a.pos);
                if area.abs() < 1e-6 {
                    continue;
                }
                let weights = [
                    cross(b.pos - point, c.pos - point) / area,
                    cross(c.pos - point, a.pos - point) / area,
                    cross(a.pos - point, b.pos - point) / area,
                ];
                if weights.iter().any(|weight| *weight < -1e-5) {
                    continue;
                }
                let mut source = [0.0; 4];
                for (vertex, weight) in [a, b, c].iter().zip(weights) {
                    for (channel, value) in vertex.color.to_array().into_iter().enumerate() {
                        source[channel] += f32::from(value) * weight;
                    }
                }
                let inverse_alpha = 1.0 - source[3] / 255.0;
                for channel in 0..4 {
                    destination[channel] = source[channel] + destination[channel] * inverse_alpha;
                }
            }
        }
        Color32::from_rgba_premultiplied(
            destination[0].round() as u8,
            destination[1].round() as u8,
            destination[2].round() as u8,
            destination[3].round() as u8,
        )
    }

    #[test]
    fn a_loading_bar_sweeps_a_glow_inside_its_capsule_and_keeps_repainting() {
        let context = egui::Context::default();
        let run = |loading: bool, time: f64| {
            let mut value = 0.0;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(240.0, 100.0),
                    )),
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    show(
                        ui,
                        &mut value,
                        Progress {
                            duration: 100.0,
                            width: 200.0,
                            label: "Seek",
                            loading,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            output
        };
        let glow = |output: &egui::FullOutput| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.clone()),
                _ => None,
            })
        };
        let quiet = run(false, 0.0);
        assert!(glow(&quiet).is_none());
        // Fade in, then look at it mid-sweep.
        for step in 1..6 {
            run(true, f64::from(step) * 0.1);
        }
        let output = run(true, 0.65);
        let mesh = glow(&output).expect("a glow while loading");
        let painted = |output: &egui::FullOutput| {
            output
                .viewport_output
                .values()
                .any(|viewport| viewport.repaint_delay.is_zero())
        };
        assert!(painted(&output), "it keeps animating");
        let bounds = mesh.calc_bounds();
        let track = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        assert!(
            track.expand(0.01).contains_rect(bounds),
            "{bounds:?} in {track:?}"
        );
        // Bright in the band, clear far from it: at mid-sweep the band is
        // in the middle and the ends are untouched.
        let alpha_at = |x: f32| {
            mesh.vertices
                .iter()
                .min_by(|a, b| (a.pos.x - x).abs().total_cmp(&(b.pos.x - x).abs()))
                .unwrap()
                .color
                .a()
        };
        assert!(alpha_at(track.center().x) > 150);
        assert_eq!(alpha_at(track.left() + 2.0), 0);
        assert_eq!(alpha_at(track.right() - 2.0), 0);
        // The ends are round: no vertex outside the capsule.
        let radius = track.height() * 0.5;
        for vertex in &mesh.vertices {
            let edge = (vertex.pos.x - track.left()).min(track.right() - vertex.pos.x);
            if edge < radius {
                let dy = (vertex.pos.y - track.center().y).abs();
                assert!(dy <= radius * (1.0 - ((radius - edge) / radius).powi(2)).sqrt() + 0.01);
            }
        }
        // Done loading: it fades away and stops asking for frames.
        for step in 0..6 {
            run(false, 0.7 + f64::from(step) * 0.1);
        }
        let done = run(false, 1.4);
        assert!(glow(&done).is_none());
    }

    #[test]
    fn tiny_progress_preserves_the_outer_left_cap_and_never_overruns_its_position() {
        let context = egui::Context::default();
        let mut position = 0.0;
        let (output, response) = frame(&context, Kind::Seek, &mut position, true, 1, Vec::new());
        let track = track_rect(response.rect);
        let empty = context.tessellate(output.shapes, output.pixels_per_point);
        for (index, width) in [0.0001, 0.25, 0.75, 1.0, 2.5, 6.9, 7.0, 8.0]
            .into_iter()
            .enumerate()
        {
            position = f64::from(width / track.width()) * 100.0;
            let (output, _) = frame(
                &context,
                Kind::Seek,
                &mut position,
                true,
                index as u32 + 2,
                Vec::new(),
            );
            let painted = context.tessellate(output.shapes, output.pixels_per_point);
            for y in [track.top() + 0.15, track.bottom() - 0.15] {
                let corner = egui::pos2(track.left() + width.min(0.15) * 0.5, y);
                assert_eq!(
                    color_at(&painted, corner),
                    color_at(&empty, corner),
                    "Square left corner at prefix width {width}"
                );
            }
            let after = egui::pos2(track.left() + width + 0.2, track.center().y);
            if width < TRACK_HEIGHT {
                assert_eq!(
                    color_at(&painted, after),
                    color_at(&empty, after),
                    "Tiny fill extends past its actual progress"
                );
            }
        }
        position = 99.999;
        let (output, _) = frame(&context, Kind::Seek, &mut position, true, 20, Vec::new());
        let painted = context.tessellate(output.shapes, output.pixels_per_point);
        let corner = egui::pos2(track.right() - 0.15, track.top() + 0.15);
        assert_eq!(color_at(&painted, corner), color_at(&empty, corner));
    }

    #[test]
    fn clicking_seek_does_not_leave_the_knob_visible_after_the_pointer_leaves() {
        for kind in [Kind::Seek, Kind::Volume] {
            let context = egui::Context::default();
            let mut value = 0.5;
            let (_, response) = frame(&context, kind, &mut value, true, 1, Vec::new());
            let target = response.rect.center();
            frame(&context, kind, &mut value, true, 2, pointer(target, true));
            let (output, response) =
                frame(&context, kind, &mut value, true, 3, pointer(target, false));
            assert!(white_knob(&output.shapes));
            assert!(response.has_focus());
            let (output, response) = frame(
                &context,
                kind,
                &mut value,
                true,
                4,
                vec![egui::Event::PointerGone],
            );
            assert!(
                response.has_focus(),
                "Hiding a pointer knob must preserve keyboard focus"
            );
            assert!(!white_knob(&output.shapes));
            let (output, _) = frame(
                &context,
                kind,
                &mut value,
                true,
                5,
                vec![key(egui::Key::ArrowRight)],
            );
            assert!(
                white_knob(&output.shapes),
                "Keyboard adjustment must reveal the focused control"
            );
        }
    }

    #[test]
    fn dragging_outside_keeps_the_knob_until_release() {
        let context = egui::Context::default();
        let mut value = 50.0;
        let (_, response) = frame(&context, Kind::Seek, &mut value, true, 1, Vec::new());
        frame(
            &context,
            Kind::Seek,
            &mut value,
            true,
            2,
            pointer(response.rect.center(), true),
        );
        let outside = response.rect.right_bottom() + egui::vec2(30.0, 30.0);
        let (output, response) = frame(
            &context,
            Kind::Seek,
            &mut value,
            true,
            3,
            vec![egui::Event::PointerMoved(outside)],
        );
        assert!(response.dragged_by(egui::PointerButton::Primary));
        assert!(white_knob(&output.shapes));
        let (output, _) = frame(
            &context,
            Kind::Seek,
            &mut value,
            true,
            4,
            pointer(outside, false),
        );
        assert!(!white_knob(&output.shapes));
    }

    #[test]
    fn rounded_played_end_keeps_continuous_remaining_track_without_buffer_color() {
        let context = egui::Context::default();
        let mut position = 50.0;
        let (output, response) = frame(&context, Kind::Seek, &mut position, true, 1, Vec::new());
        assert!(!white_knob(&output.shapes));
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        let center = response.rect.center();
        assert_eq!(
            color_at(&primitives, center - egui::vec2(1.0, 0.0)),
            theme::accent()
        );
        for offset in [-2.0, 2.0] {
            // The rounded blue cap leaves the solid remaining color visible
            // beside its upper/lower tip, rather than a transparent notch.
            let corner = color_at(&primitives, center + egui::vec2(-0.1, offset));
            assert_ne!(corner, theme::accent());
            assert_eq!(
                corner.a(),
                255,
                "The remaining layer must fill the rounded tip"
            );
            assert!((REMAINING.r()..=theme::accent().r()).contains(&corner.r()));
            assert!((REMAINING.g()..=theme::accent().g()).contains(&corner.g()));
            assert!((REMAINING.b()..=theme::accent().b()).contains(&corner.b()));
        }
        for offset in [-1.0, 0.0, 1.0] {
            assert_eq!(
                color_at(&primitives, center + egui::vec2(1.0, offset)),
                REMAINING
            );
        }
    }

    #[test]
    fn seek_and_volume_keep_click_keyboard_and_accessibility_values() {
        for kind in [Kind::Seek, Kind::Volume] {
            let maximum = match kind {
                Kind::Seek => 100.0,
                Kind::Volume => 1.0,
            };
            let context = egui::Context::default();
            let mut value = maximum * 0.5;
            let (_, response) = frame(&context, kind, &mut value, true, 1, Vec::new());
            frame(&context, kind, &mut value, true, 2, Vec::new());
            let target = egui::pos2(
                response.rect.left() + 8.0 + (response.rect.width() - 16.0) * 0.75,
                response.rect.center().y,
            );
            frame(&context, kind, &mut value, true, 3, pointer(target, true));
            let (output, response) =
                frame(&context, kind, &mut value, true, 4, pointer(target, false));
            assert!((value - maximum * 0.75).abs() < 1e-5);
            assert!(response.has_focus());
            assert!(output.platform_output.events.iter().any(|event| {
                matches!(event, egui::output::OutputEvent::Clicked(info)
                    if info.typ == egui::WidgetType::Slider && info.value.is_some())
            }));
            for (tick, key, expected, changed) in [
                (5, egui::Key::ArrowRight, maximum * 0.8, true),
                (6, egui::Key::End, maximum, true),
                (7, egui::Key::ArrowRight, maximum, false),
                (8, egui::Key::Home, 0.0, true),
                (9, egui::Key::ArrowLeft, 0.0, false),
            ] {
                let (_, response) =
                    frame(&context, kind, &mut value, true, tick, vec![self::key(key)]);
                assert!((value - expected).abs() < 1e-5);
                assert_eq!(response.changed(), changed);
            }
            value = maximum * 0.5;
            frame(&context, kind, &mut value, false, 10, pointer(target, true));
            let (_, response) = frame(
                &context,
                kind,
                &mut value,
                false,
                11,
                vec![key(egui::Key::End)],
            );
            assert!((value - maximum * 0.5).abs() < 1e-5);
            assert!(!response.changed());
        }
    }

    #[test]
    fn the_wheel_over_the_volume_slider_turns_it_by_notches() {
        let wheel = |at: egui::Pos2, lines: f32, modifiers: egui::Modifiers| {
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, lines * WHEEL_POINTS_PER_STEP),
                    modifiers,
                    phase: egui::TouchPhase::Move,
                },
            ]
        };
        let context = egui::Context::default();
        let mut value = 0.5;
        let (_, response) = frame(&context, Kind::Volume, &mut value, true, 1, Vec::new());
        let over = response.rect.center();
        let outside = egui::pos2(over.x, response.rect.bottom() + 30.0);
        frame(
            &context,
            Kind::Volume,
            &mut value,
            true,
            2,
            vec![egui::Event::PointerMoved(over)],
        );
        let none = egui::Modifiers::NONE;
        let (_, response) = frame(
            &context,
            Kind::Volume,
            &mut value,
            true,
            3,
            wheel(over, 1.0, none),
        );
        assert!(
            (value - 0.55).abs() < 1e-5,
            "up is louder by a notch: {value}"
        );
        assert!(response.changed());
        frame(
            &context,
            Kind::Volume,
            &mut value,
            true,
            4,
            wheel(over, -2.0, none),
        );
        assert!((value - 0.45).abs() < 1e-5);
        frame(
            &context,
            Kind::Volume,
            &mut value,
            true,
            5,
            wheel(over, 40.0, none),
        );
        assert_eq!(value, 1.0, "bounded");
        // Elsewhere, with Ctrl (zoom) or disabled, the wheel leaves it alone.
        frame(
            &context,
            Kind::Volume,
            &mut value,
            true,
            6,
            wheel(outside, -1.0, none),
        );
        frame(
            &context,
            Kind::Volume,
            &mut value,
            true,
            7,
            wheel(over, -1.0, egui::Modifiers::CTRL),
        );
        frame(
            &context,
            Kind::Volume,
            &mut value,
            false,
            8,
            wheel(over, -1.0, none),
        );
        assert_eq!(value, 1.0);
    }

    #[test]
    fn seek_and_volume_knobs_appear_only_on_hover_or_keyboard_focus() {
        for kind in [Kind::Seek, Kind::Volume] {
            let context = egui::Context::default();
            let mut value = 0.5;
            let (output, response) = frame(&context, kind, &mut value, true, 1, Vec::new());
            assert!(!white_knob(&output.shapes));
            let (output, _) = frame(
                &context,
                kind,
                &mut value,
                true,
                2,
                vec![egui::Event::PointerMoved(response.rect.center())],
            );
            assert!(white_knob(&output.shapes));
            context.memory_mut(|memory| memory.request_focus(response.id));
            let (output, _) = frame(
                &context,
                kind,
                &mut value,
                true,
                3,
                vec![egui::Event::PointerGone],
            );
            assert!(white_knob(&output.shapes));
            assert!(output.shapes.iter().all(|shape| {
                !matches!(&shape.shape, egui::Shape::Circle(circle) if circle.stroke.width > 0.0)
            }), "The focused knob must not gain an outline");
        }
    }
}
