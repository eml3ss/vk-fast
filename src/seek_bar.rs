//! Matching rounded seek and volume controls without a separate buffer layer.
use eframe::egui::{self, Color32, Response, Sense};
use std::f32::consts::FRAC_PI_2;

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
    // Each wait sweeps from the left edge: the sweep's clock starts when
    // loading does, not where the last one stopped.
    let now = ui.input(|input| input.time);
    let started = ui.ctx().data_mut(|data| {
        let state = data.get_temp_mut_or(id.with("start"), (false, now));
        if progress.loading && !state.0 {
            state.1 = now;
        }
        state.0 = progress.loading;
        state.1
    });
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
        (loading, now - started),
    )
}

/// Scrolled points per 5 % of volume: about one wheel notch.
const WHEEL_POINTS_PER_STEP: f32 = 40.0;

pub fn volume(ui: &mut egui::Ui, value: &mut f32, width: f32, label: &str) -> Response {
    let mut precise = f64::from(*value);
    let mut response = control(ui, &mut precise, 1.0, 0.05, width, label, (0.0, 0.0));
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
    // The loading glow's strength and how long this wait has lasted.
    (loading, elapsed): (f32, f64),
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
        let feather = 1.0 / ui.ctx().pixels_per_point();
        let remaining = theme::tone(REMAINING).gamma_multiply(opacity);
        painter.add(capsule(track, feather, 0, |_| remaining));
        if fraction > 0.0 {
            let end = track.left() + track.width() * fraction;
            let played = theme::accent().gamma_multiply(opacity);
            painter.add(played_part(track, end, feather, played));
        }
        if loading > 0.0 {
            painter.add(loading_glow(track, feather, elapsed, loading));
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

/// The loading glow `time` seconds into a wait: an accent band with soft
/// edges that sweeps left to right across the bar, eased so it slows at
/// both ends. `strength` fades it in and out.
fn loading_glow(track: egui::Rect, feather: f32, time: f64, strength: f32) -> egui::Shape {
    let phase = (time / LOADING_SWEEP).fract() as f32;
    let eased = phase * phase * (3.0 - 2.0 * phase);
    let band = (track.width() * 0.4).max(TRACK_HEIGHT * 2.0);
    let center = track.left() - band * 0.5 + (track.width() + band) * eased;
    let color = theme::accent();
    capsule(track, feather, 96, |x| {
        let distance = ((x - center) / (band * 0.5)).abs();
        let glow = if distance < 1.0 {
            let fall = 1.0 - distance;
            fall * fall * (3.0 - 2.0 * fall)
        } else {
            0.0
        };
        color.gamma_multiply(glow * strength * 0.85)
    })
}

/// A capsule filling `rect` (its ends half circles), coloured along its
/// length by `color`, with `columns` even steps for a gradient. egui keeps
/// corner radii as whole points and draws small ones with a few segments,
/// so a 7-point bar's ends came out cut; this one follows the circle
/// closely.
fn capsule(
    rect: egui::Rect,
    feather: f32,
    columns: usize,
    color: impl Fn(f32) -> Color32,
) -> egui::Shape {
    const CAP_STEPS: usize = 16;
    let radius = (rect.height() * 0.5).min(rect.width() * 0.5);
    let circle = |offset: f32| (radius * radius - offset * offset).max(0.0).sqrt();
    let mut outline: Vec<f32> = Vec::new();
    for step in 0..=CAP_STEPS {
        let inset = radius * (1.0 - (FRAC_PI_2 * step as f32 / CAP_STEPS as f32).cos());
        outline.push(rect.left() + inset);
        outline.push(rect.right() - inset);
    }
    for column in 1..columns {
        outline.push(rect.left() + rect.width() * column as f32 / columns as f32);
    }
    let half = |x: f32| {
        if x < rect.left() + radius {
            circle(rect.left() + radius - x)
        } else if x > rect.right() - radius {
            circle(x - (rect.right() - radius))
        } else {
            radius
        }
    };
    strip(outline, half, rect.center().y, feather, color)
}

/// A shape symmetric about the line `middle`: `half` its half height at
/// each x of `columns` (zero at both tips), coloured by `color` along its
/// length, its edge blurred over one pixel (`feather`) as egui does. egui's
/// own smoothing pushes a polygon's corners out along the bisector, which
/// on a sliver of a few hundredths of a point shot spikes far past the bar;
/// here every edge point moves by `feather / 2` along its own normal.
fn strip(
    mut columns: Vec<f32>,
    half: impl Fn(f32) -> f32,
    middle: f32,
    feather: f32,
    color: impl Fn(f32) -> Color32,
) -> egui::Shape {
    columns.sort_by(f32::total_cmp);
    columns.dedup_by(|a, b| (*a - *b).abs() < 0.001);
    if columns.len() < 2 {
        return egui::Shape::Noop;
    }
    let top: Vec<egui::Pos2> = columns
        .iter()
        .map(|&x| egui::pos2(x, middle - half(x)))
        .collect();
    let last = top.len() - 1;
    let mut mesh = egui::Mesh::default();
    let half_feather = feather * 0.5;
    for (index, &point) in top.iter().enumerate() {
        // The top edge's outward normal: across its neighbours, or straight
        // out at a tip.
        let normal = if index == 0 && middle - point.y < 0.001 {
            egui::vec2(-1.0, 0.0)
        } else if index == last && middle - point.y < 0.001 {
            egui::vec2(1.0, 0.0)
        } else {
            let tangent = top[(index + 1).min(last)] - top[index.saturating_sub(1)];
            egui::vec2(tangent.y, -tangent.x).normalized()
        };
        let flip = egui::vec2(normal.x, -normal.y);
        let bottom = egui::pos2(point.x, 2.0 * middle - point.y);
        let fill = color(point.x);
        let clear = Color32::TRANSPARENT;
        let base = mesh.vertices.len() as u32;
        // Outer top, inner top, inner bottom, outer bottom.
        mesh.colored_vertex(point + normal * half_feather, clear);
        mesh.colored_vertex(point - normal * half_feather, fill);
        mesh.colored_vertex(bottom - flip * half_feather, fill);
        mesh.colored_vertex(bottom + flip * half_feather, clear);
        if index > 0 {
            let previous = base - 4;
            for lane in 0..3 {
                let (a, b) = (previous + lane, previous + lane + 1);
                let (c, d) = (base + lane, base + lane + 1);
                mesh.add_triangle(a, b, c);
                mesh.add_triangle(b, c, d);
            }
        }
    }
    egui::Shape::mesh(mesh)
}

/// The played part up to `end`: round at both ends however short it is.
/// It is the bar's capsule cut by a half circle ending at `end`, so a
/// part shorter than the bar is thick starts as a small rounded lens in the
/// bar's round end instead of a sliver with a square edge; thinner than a
/// pixel, it fades in rather than flicker.
fn played_part(track: egui::Rect, end: f32, feather: f32, color: Color32) -> egui::Shape {
    let radius = (track.height() * 0.5).min(track.width() * 0.5);
    let end = end.clamp(track.left(), track.right());
    let length = end - track.left();
    if length <= 0.0 {
        return egui::Shape::Noop;
    }
    let circle = |offset: f32| (radius * radius - offset * offset).max(0.0).sqrt();
    let half = |x: f32| {
        let bar = if x < track.left() + radius {
            circle(track.left() + radius - x)
        } else if x > track.right() - radius {
            circle(x - (track.right() - radius))
        } else {
            radius
        };
        let cut = if x > end - radius {
            circle(x - (end - radius))
        } else {
            radius
        };
        bar.min(cut)
    };
    // Dense steps over the rounded stretches, by angle so the arcs stay
    // smooth; the flat middle needs only its ends.
    const STEPS: usize = 24;
    let mut columns = vec![track.left(), end];
    for step in 0..=STEPS {
        let inset = radius * (1.0 - (FRAC_PI_2 * step as f32 / STEPS as f32).cos());
        for x in [track.left() + inset, end - inset] {
            if x > track.left() && x < end {
                columns.push(x);
            }
        }
    }
    let color = color.gamma_multiply((length / feather).clamp(0.0, 1.0));
    strip(columns, half, track.center().y, feather, |_| color)
}

/// For tests: the rectangle and fill of a bar part `capsule` drew, from
/// its mesh (halfway between the feather's outer and inner edges).
#[cfg(test)]
pub(crate) fn painted_capsule(shape: &egui::Shape) -> Option<(egui::Rect, Color32)> {
    let egui::Shape::Mesh(mesh) = shape else {
        return None;
    };
    if mesh.vertices.len() < 8 || mesh.vertices.len() % 4 != 0 {
        return None;
    }
    let bounds = |inner: bool| {
        egui::Rect::from_points(
            &mesh
                .vertices
                .iter()
                .enumerate()
                .filter(|(index, _)| matches!(index % 4, 1 | 2) == inner)
                .map(|(_, vertex)| vertex.pos)
                .collect::<Vec<_>>(),
        )
    };
    let (outer, inner) = (bounds(false), bounds(true));
    let rect = egui::Rect::from_min_max(
        outer.min + (inner.min - outer.min) * 0.5,
        outer.max + (inner.max - outer.max) * 0.5,
    );
    Some((rect, mesh.vertices[1].color))
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
        // The glow is the capsule whose colour changes along the bar; the
        // bar under it is one colour.
        let inner = |mesh: &egui::Mesh| -> Vec<egui::epaint::Vertex> {
            mesh.vertices
                .iter()
                .enumerate()
                .filter(|(index, _)| matches!(index % 4, 1 | 2))
                .map(|(_, vertex)| *vertex)
                .collect()
        };
        let glow = |output: &egui::FullOutput| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh)
                    if inner(mesh)
                        .windows(2)
                        .any(|pair| pair[0].color != pair[1].color) =>
                {
                    Some(mesh.clone())
                }
                _ => None,
            })
        };
        let alpha_at = |mesh: &egui::Mesh, x: f32| {
            let vertices = inner(mesh);
            let nearest = vertices
                .iter()
                .map(|vertex| (vertex.pos.x - x).abs())
                .fold(f32::MAX, f32::min);
            vertices
                .iter()
                .filter(|vertex| (vertex.pos.x - x).abs() <= nearest + 0.01)
                .map(|vertex| vertex.color.a())
                .max()
                .unwrap()
        };
        // The area a capsule's mesh covers over one rounded end.
        let cap_area = |mesh: &egui::Mesh, track: egui::Rect, left: bool| {
            let radius = track.height() * 0.5;
            mesh.indices
                .chunks(3)
                .map(|triangle| {
                    let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize].pos);
                    let inside = [a, b, c].iter().all(|point| {
                        if left {
                            point.x <= track.left() + radius + 0.01
                        } else {
                            point.x >= track.right() - radius - 0.01
                        }
                    });
                    if inside {
                        ((b - a).x * (c - a).y - (b - a).y * (c - a).x).abs() * 0.5
                    } else {
                        0.0
                    }
                })
                .sum::<f32>()
        };
        let quiet = run(false, 0.0);
        assert!(glow(&quiet).is_none());
        // The bar itself: a capsule with properly round ends, its
        // feathered edge reaching past the half disc of each end.
        let (track, _) = quiet
            .shapes
            .iter()
            .find_map(|shape| painted_capsule(&shape.shape))
            .unwrap();
        let radius = track.height() * 0.5;
        let disc = std::f32::consts::PI * radius * radius * 0.5;
        let bar = match &quiet.shapes[0].shape {
            egui::Shape::Mesh(mesh) => mesh.clone(),
            _ => unreachable!(),
        };
        for left in [true, false] {
            let area = cap_area(&bar, track, left);
            assert!(area > disc, "{area} of {disc}");
            let near = inner(&bar)
                .iter()
                .filter(|vertex| {
                    let edge = (vertex.pos.x - track.left()).min(track.right() - vertex.pos.x);
                    edge < radius && (left == (vertex.pos.x < track.center().x))
                })
                .count();
            assert!(near >= 30, "a round end, not a few corners: {near}");
        }
        // Fade in, then look at it mid-sweep: the sweep's clock starts with
        // the wait, at 0.1 s.
        for step in 1..6 {
            run(true, f64::from(step) * 0.1);
        }
        let output = run(true, 0.75);
        let mesh = glow(&output).expect("a glow while loading");
        let painted = |output: &egui::FullOutput| {
            output
                .viewport_output
                .values()
                .any(|viewport| viewport.repaint_delay.is_zero())
        };
        assert!(painted(&output), "it keeps animating");
        let bounds = mesh.calc_bounds();
        assert!(
            track.expand(0.51).contains_rect(bounds),
            "{bounds:?} in {track:?}"
        );
        // Bright in the band, clear far from it: at mid-sweep the band is
        // in the middle and the ends are untouched.
        assert!(alpha_at(&mesh, track.center().x) > 150);
        assert_eq!(alpha_at(&mesh, track.left() + 2.0), 0);
        assert_eq!(alpha_at(&mesh, track.right() - 2.0), 0);
        // The ends are round: the glow covers each half-disc end of the
        // bar (no unfilled pixels where it starts) and nothing it fills is
        // outside.
        for left in [true, false] {
            let area = cap_area(&mesh, track, left);
            assert!(area > disc, "{area} of {disc}");
        }
        for vertex in inner(&mesh) {
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

        // The next wait starts its sweep from the left again, whatever the
        // clock: 0.65 s into it the band is back in the middle.
        for step in 0..6 {
            run(true, 2.0 + f64::from(step) * 0.1);
        }
        let again = glow(&run(true, 2.65)).unwrap();
        assert!(alpha_at(&again, track.center().x) > 150);
        assert_eq!(alpha_at(&again, track.left() + 2.0), 0);
        // And right at its start the band sits off the left edge.
        run(false, 3.0);
        for step in 0..4 {
            run(false, 3.1 + f64::from(step) * 0.1);
        }
        run(true, 4.0);
        let start = glow(&run(true, 4.05));
        assert!(start.is_none_or(|mesh| inner(&mesh).iter().all(|vertex| {
            vertex.color.a() == 0 || vertex.pos.x < track.left() + track.width() * 0.2
        })));
    }

    #[test]
    fn tiny_progress_preserves_the_outer_left_cap_and_never_overruns_its_position() {
        let context = egui::Context::default();
        let mut position = 0.0;
        let (output, response) = frame(&context, Kind::Seek, &mut position, true, 1, Vec::new());
        let track = track_rect(response.rect);
        let empty = context.tessellate(output.shapes, output.pixels_per_point);
        for (index, width) in [0.0001, 0.01, 0.05, 0.25, 0.75, 1.0, 2.5, 6.9, 7.0, 8.0]
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
            // Nothing pokes out above or below the bar: egui's smoothing of
            // a sliver this thin shot spikes far past it.
            for step in 0..=20 {
                let x = track.left() - 1.0 + (width + 2.0) * step as f32 / 20.0;
                for y in [track.top() - 1.0, track.bottom() + 1.0, track.top() - 4.0] {
                    let point = egui::pos2(x, y);
                    assert_eq!(
                        color_at(&painted, point),
                        color_at(&empty, point),
                        "Spike at {point:?}, prefix width {width}"
                    );
                }
            }
            // Past the half-pixel smoothing of its edge, nothing is filled.
            let after = egui::pos2(track.left() + width + 0.6, track.center().y);
            if width < TRACK_HEIGHT {
                assert_eq!(
                    color_at(&painted, after),
                    color_at(&empty, after),
                    "Tiny fill extends past its actual progress"
                );
            }
            // Its right end is round too, not cut square: just inside the
            // end, the corner near the bar's edge stays unfilled.
            if width >= 2.5 {
                let corner = egui::pos2(track.left() + width - 0.4, track.top() + 0.3);
                assert_eq!(
                    color_at(&painted, corner),
                    color_at(&empty, corner),
                    "Square right end at prefix width {width}"
                );
                let middle = egui::pos2(track.left() + width - 0.8, track.center().y);
                assert_ne!(color_at(&painted, middle), color_at(&empty, middle));
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
