//! A single horizontally sliding row with VK-style edge arrows on hover,
//! Shift + wheel scrolling and an eased slide. Used by Home and artist pages.
use eframe::egui::{self, Align, Color32, Layout, Sense, Stroke};

use crate::i18n::Language;

/// Time constant of the slide; it settles in about three of these.
const CAROUSEL_EASE: f32 = 0.055;

/// The next stop starts at the first item not fully visible.
pub(crate) fn next_stop(spans: &[(f32, f32)], target: f32, visible: f32, max: f32) -> f32 {
    spans
        .iter()
        .find(|(x, width)| x + width > target + visible + 0.5)
        .map_or(max, |(x, _)| x.min(max))
}

/// The previous stop shows the page that ends just before the current one:
/// the first item from which everything up to the current stop fits.
pub(crate) fn previous_stop(spans: &[(f32, f32)], target: f32, visible: f32) -> f32 {
    let gap = match spans {
        [(x, width), (next, _), ..] => (next - x - width).max(0.0),
        _ => 0.0,
    };
    let wanted = target - visible - gap;
    spans
        .iter()
        .map(|(x, _)| *x)
        .find(|x| *x >= wanted - 0.5 && *x < target - 0.5)
        .unwrap_or(0.0)
        .max(0.0)
}

/// One horizontally sliding row. The scroll target lives in egui memory
/// and the drawn offset eases toward it. Like VK's sliders, round edge
/// arrows appear while the pointer is over the row.
pub fn carousel(
    ui: &mut egui::Ui,
    key: &str,
    spans: &[(f32, f32)],
    height: f32,
    language: Language,
    mut item: impl FnMut(&mut egui::Ui, usize),
) {
    let visible = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(visible, height), Sense::hover());
    let content = spans.last().map_or(0.0, |(x, width)| x + width);
    let max = (content - visible).max(0.0);
    let id = egui::Id::new(("home-carousel", key));
    let mut target = ui
        .data(|data| data.get_temp::<f32>(id))
        .unwrap_or(0.0)
        .clamp(0.0, max);
    let hovered = ui.rect_contains_pointer(rect);
    // Shift + wheel arrives as horizontal scroll; it moves only this row.
    let wheel = if hovered && max > 0.0 {
        ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.x))
    } else {
        0.0
    };
    if wheel != 0.0 {
        target = (target - wheel).clamp(0.0, max);
        ui.data_mut(|data| data.insert_temp(id, target));
    }
    let offset_id = id.with("offset");
    let previous = ui
        .data(|data| data.get_temp::<f32>(offset_id))
        .unwrap_or(target);
    let offset = if wheel != 0.0 {
        // The wheel is already smoothed by egui; follow it without lag.
        target
    } else {
        let dt = ui.input(|input| input.stable_dt).clamp(0.0, 0.1);
        let eased = previous + (target - previous) * (1.0 - (-dt / CAROUSEL_EASE).exp());
        if (target - eased).abs() < 0.5 {
            target
        } else {
            ui.ctx().request_repaint();
            eased
        }
    };
    ui.data_mut(|data| data.insert_temp(offset_id, offset));
    // Keep card outlines and focus rings, but never draw past the row ends.
    let clip = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() - 2.0),
        egui::pos2(rect.right(), rect.bottom() + 2.0),
    )
    .intersect(ui.clip_rect());
    for (index, (x, width)) in spans.iter().enumerate() {
        let left = rect.left() + x - offset;
        if left + width < rect.left() || left > rect.right() {
            continue;
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt((key, index))
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(left, rect.top()),
                    egui::vec2(*width, height),
                ))
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(clip);
        item(&mut child, index);
    }

    let mut moved = None;
    for (next, available) in [(false, target > 0.5), (true, target < max - 0.5)] {
        if !available {
            continue;
        }
        let x = if next {
            rect.right() - 20.0
        } else {
            rect.left() + 20.0
        };
        let button =
            egui::Rect::from_center_size(egui::pos2(x, rect.center().y), egui::vec2(28.0, 28.0));
        let response = ui.interact(button, id.with(("arrow", next)), Sense::click());
        let label = if next {
            language.text("Дальше", "Next")
        } else {
            language.text("Назад", "Back")
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        if hovered || response.has_focus() {
            arrow(ui, button, next, response.hovered() || response.has_focus());
        }
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            moved = Some(if next {
                next_stop(spans, target, visible, max)
            } else {
                previous_stop(spans, target, visible)
            });
        }
    }
    if let Some(stop) = moved {
        ui.data_mut(|data| data.insert_temp(id, stop));
        ui.ctx().request_repaint();
    }
}

/// A round chevron button drawn over the row's edge.
fn arrow(ui: &egui::Ui, rect: egui::Rect, next: bool, hovered: bool) {
    let painter = ui.painter();
    let center = rect.center();
    let fill = if hovered {
        crate::theme::tone(Color32::from_rgb(52, 60, 74))
    } else {
        crate::theme::tone(Color32::from_rgba_unmultiplied(36, 42, 52, 238))
    };
    painter.circle_filled(center, 13.0, fill);
    painter.circle_stroke(
        center,
        13.0,
        Stroke::new(1.0, crate::theme::tone(Color32::from_rgb(64, 73, 88))),
    );
    let direction = if next { 1.0 } else { -1.0 };
    let tip = center + egui::vec2(2.5 * direction, 0.0);
    let stroke = Stroke::new(1.8, crate::theme::tone(Color32::from_rgb(226, 232, 240)));
    painter.line_segment([tip + egui::vec2(-4.5 * direction, -5.0), tip], stroke);
    painter.line_segment([tip, tip + egui::vec2(-4.5 * direction, 5.0)], stroke);
}
