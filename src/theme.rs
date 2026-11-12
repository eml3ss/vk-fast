use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

#[cfg(test)]
pub use crate::palette::Accent;
pub use crate::palette::Theme;

// The colours of the active theme (`crate::palette`), read on every paint so
// a theme switch shows at once.
pub fn accent() -> Color32 {
    crate::palette::current().accent
}
pub fn muted() -> Color32 {
    crate::palette::current().muted
}
/// The page behind everything.
pub fn background() -> Color32 {
    crate::palette::current().background
}
/// Cards, fields, menus and dialogs.
pub fn surface() -> Color32 {
    crate::palette::current().surface
}
/// The sidebar, player bar and the lyrics/queue panel.
pub fn panel() -> Color32 {
    crate::palette::current().panel
}
/// The brightest text and icons (white in dark themes), for hovered icons
/// and titles drawn on the page. White on the accent or a cover stays white.
pub fn strong() -> Color32 {
    crate::palette::current().strong
}
pub fn error() -> Color32 {
    crate::palette::current().error
}
pub fn warning() -> Color32 {
    crate::palette::current().warning
}
/// Light themes draw dark text on light surfaces.
pub fn is_light() -> bool {
    crate::palette::current().light
}
pub fn primary() -> Color32 {
    crate::palette::current().primary
}
pub fn selected() -> Color32 {
    crate::palette::current().selected
}
/// The chosen item of a navigation list: `selected` unless the theme sets it.
pub fn active() -> Color32 {
    let palette = crate::palette::current();
    palette.active.unwrap_or(palette.selected)
}
/// Text and icons on [`active`] in place of `usual`: the theme's own
/// colour, or one that reads on its own `active` fill; `usual` otherwise.
pub fn on_active(usual: Color32) -> Color32 {
    let palette = crate::palette::current();
    palette.on_active.unwrap_or_else(|| {
        palette
            .active
            .map_or(usual, |fill| palette.readable_on(fill, usual))
    })
}
/// Text and icons on a primary or accent `fill` in place of `usual`.
pub fn on_fill(fill: Color32, usual: Color32) -> Color32 {
    crate::palette::current().readable_on(fill, usual)
}
/// Widgets in `style` filled with [`primary`] at rest and the accent when
/// hovered (Play, the home card's action): their text and icons switch
/// only where the usual text would not read on those fills.
pub fn on_primary_widgets(style: &mut egui::Style) {
    let palette = crate::palette::current();
    let at_rest = palette.readable_on(palette.primary, palette.text);
    let hovered = palette.readable_on(palette.accent, palette.text);
    if at_rest == palette.text && hovered == palette.text {
        return;
    }
    let widgets = &mut style.visuals.widgets;
    widgets.inactive.fg_stroke.color = at_rest;
    widgets.hovered.fg_stroke.color = hovered;
    widgets.active.fg_stroke.color = hovered;
    style.visuals.override_text_color = Some(at_rest);
}
/// A filled button's label on [`primary`]: recoloured only where the
/// usual text would not read, so other themes keep egui's own text colour.
pub fn on_primary_text(text: impl Into<egui::RichText>) -> egui::RichText {
    let text = text.into();
    let palette = crate::palette::current();
    let on = palette.readable_on(palette.primary, palette.text);
    if on == palette.text {
        text
    } else {
        text.color(on)
    }
}
/// A hovered item in a panel: darker than the panel, or lighter where the
/// panel is already as dark as the page.
pub fn panel_hover() -> Color32 {
    let palette = crate::palette::current();
    if palette.panel == palette.background {
        palette.surface
    } else {
        palette.background
    }
}
/// `color`, a grey of the original dark look, in the active theme.
pub fn tone(color: Color32) -> Color32 {
    crate::palette::current().tone(color)
}

/// `color`, a shade of the original blue accent, in the active accent.
pub fn accent_tone(color: Color32) -> Color32 {
    crate::palette::current().accent_tone(color)
}

/// `color`, a light shade of the original blue for dark backgrounds, in the
/// active accent; light themes use the accent itself, which reads on them.
pub fn accent_light(color: Color32) -> Color32 {
    if is_light() {
        accent()
    } else {
        accent_tone(color)
    }
}

/// Draw `contents` as a dark scene in a light theme: views over a cover or
/// VK Mix's colours keep white text on dark shades. Other themes draw it as
/// usual.
pub fn dark_scene<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if !is_light() {
        return contents(ui);
    }
    crate::palette::dark_scene(|| {
        ui.scope(|ui| {
            *ui.visuals_mut() = visuals(&crate::palette::current());
            contents(ui)
        })
        .inner
    })
}

/// The outline of a white handle in light themes, where white alone is lost
/// on white cards.
pub fn handle_stroke() -> egui::Stroke {
    if is_light() {
        egui::Stroke::new(1.0, crate::palette::current().control_hovered_stroke)
    } else {
        egui::Stroke::NONE
    }
}

/// The line between a panel and the page, in themes whose panels are as dark
/// as the page.
pub fn panel_line() -> Option<egui::Stroke> {
    crate::palette::current()
        .panel_line
        .map(|color| egui::Stroke::new(1.0, color))
}

/// Draw the app in `theme` with `accent` from this frame on, with egui's own
/// widgets.
pub fn apply(ctx: &egui::Context, theme: Theme, accent: Color32) {
    crate::palette::set_active(theme);
    crate::palette::set_accent(accent);
    install_style(ctx);
    ctx.request_repaint();
}

pub fn wrapped_body(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    align: egui::Align,
    color: Color32,
) -> egui::Label {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        FontId::proportional(14.0),
        color,
        width.max(1.0),
    );
    job.justify = false;
    job.halign = align;
    egui::Label::new(ui.painter().layout_job(job)).selectable(false)
}

/// One interpolated surface with a rounded, antialiased perimeter. Vertex
/// colors avoid the seams produced by clipping separate solid-color strips.
pub fn horizontal_gradient(
    painter: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    left: Color32,
    right: Color32,
) {
    if !rect.is_positive() {
        return;
    }
    let feather = 0.5 / painter.ctx().pixels_per_point();
    let radius = radius
        .min(rect.width().min(rect.height()) * 0.5)
        .max(feather);
    let color_at = |x: f32| {
        let fraction = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let a = left.to_array();
        let b = right.to_array();
        let channels = std::array::from_fn::<_, 4, _>(|i| {
            egui::lerp(f32::from(a[i])..=f32::from(b[i]), fraction).round() as u8
        });
        Color32::from_rgba_premultiplied(channels[0], channels[1], channels[2], channels[3])
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center().x));
    for (center, start) in [
        (
            egui::pos2(rect.right() - radius, rect.top() + radius),
            -90.0_f32,
        ),
        (
            egui::pos2(rect.right() - radius, rect.bottom() - radius),
            0.0,
        ),
        (
            egui::pos2(rect.left() + radius, rect.bottom() - radius),
            90.0,
        ),
        (egui::pos2(rect.left() + radius, rect.top() + radius), 180.0),
    ] {
        for step in 0..=8 {
            let angle = (start + step as f32 * 90.0 / 8.0).to_radians();
            let normal = egui::vec2(angle.cos(), angle.sin());
            let inner = center + normal * (radius - feather);
            mesh.colored_vertex(inner, color_at(inner.x));
            mesh.colored_vertex(center + normal * (radius + feather), Color32::TRANSPARENT);
        }
    }
    let count = (mesh.vertices.len() - 1) as u32 / 2;
    for index in 0..count {
        let a = 1 + index * 2;
        let b = 1 + ((index + 1) % count) * 2;
        mesh.add_triangle(0, a, b);
        mesh.add_triangle(a, a + 1, b);
        mesh.add_triangle(b, a + 1, b + 1);
    }
    painter.add(egui::Shape::mesh(mesh));
}
/// A rounded rectangle filled left to right through several evenly spaced
/// colors. Vertical strips carry the colors; columns are dense inside the
/// corner arcs, and a thin transparent fringe antialiases the outline.
pub fn gradient_stops(painter: &egui::Painter, rect: egui::Rect, radius: f32, stops: &[Color32]) {
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return;
    };
    if stops.len() == 1 || !rect.is_positive() {
        horizontal_gradient(painter, rect, radius, *first, *last);
        return;
    }
    // One physical pixel of edge, split across the outline as egui's own
    // shapes do, so the rounded corners are as smooth as any other card.
    let half = 0.5 / painter.ctx().pixels_per_point();
    let radius = radius.min(rect.width().min(rect.height()) * 0.5).max(0.0);
    let color_at = |x: f32| {
        let position =
            ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) * (stops.len() - 1) as f32;
        let index = (position.floor() as usize).min(stops.len() - 2);
        let fraction = position - index as f32;
        let (a, b) = (stops[index].to_array(), stops[index + 1].to_array());
        let channels = std::array::from_fn::<_, 4, _>(|i| {
            egui::lerp(f32::from(a[i])..=f32::from(b[i]), fraction).round() as u8
        });
        Color32::from_rgba_premultiplied(channels[0], channels[1], channels[2], channels[3])
    };
    // Columns across the card: the top outline point of each, with its
    // outward normal. Corners are sampled by angle, so their steep ends are
    // as dense as their flat tops; the middle keeps the gradient's stops.
    let mut columns: Vec<(egui::Pos2, egui::Vec2)> = Vec::new();
    let arc_steps = ((radius * 1.5).ceil() as usize).clamp(1, 64);
    let left_center = egui::pos2(rect.left() + radius, rect.top() + radius);
    for step in 0..=arc_steps {
        let angle = std::f32::consts::PI * (1.0 + 0.5 * step as f32 / arc_steps as f32);
        let normal = egui::vec2(angle.cos(), angle.sin());
        columns.push((left_center + normal * radius, normal));
    }
    let middle = (rect.width() - 2.0 * radius).max(0.0);
    let middle_steps = ((middle / 8.0).ceil() as usize).max(1);
    for step in 1..middle_steps {
        let x = rect.left() + radius + middle * step as f32 / middle_steps as f32;
        columns.push((egui::pos2(x, rect.top()), egui::vec2(0.0, -1.0)));
    }
    let right_center = egui::pos2(rect.right() - radius, rect.top() + radius);
    for step in 0..=arc_steps {
        let angle = std::f32::consts::PI * (1.5 + 0.5 * step as f32 / arc_steps as f32);
        let normal = egui::vec2(angle.cos(), angle.sin());
        columns.push((right_center + normal * radius, normal));
    }
    columns.dedup_by(|a, b| (a.0 - b.0).length() < 0.01);
    let mirror = |point: egui::Pos2| egui::pos2(point.x, rect.top() + rect.bottom() - point.y);
    let mut mesh = egui::Mesh::default();
    for (top, normal) in &columns {
        let color = color_at(top.x);
        let bottom = mirror(*top);
        let down = egui::vec2(normal.x, -normal.y);
        // Per column: outer top, top, bottom, outer bottom.
        mesh.colored_vertex(*top + *normal * half, Color32::TRANSPARENT);
        mesh.colored_vertex(*top - *normal * half, color);
        mesh.colored_vertex(bottom - down * half, color);
        mesh.colored_vertex(bottom + down * half, Color32::TRANSPARENT);
    }
    let count = columns.len() as u32;
    for column in 0..count.saturating_sub(1) {
        let a = column * 4;
        let b = a + 4;
        for row in 0..3 {
            mesh.add_triangle(a + row, b + row, a + row + 1);
            mesh.add_triangle(a + row + 1, b + row, b + row + 1);
        }
    }
    // The straight left and right sides fade out over the same pixel.
    if count > 0 {
        for column in [0, count - 1] {
            let a = column * 4;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a, a + 2, a + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("Inter Semibold".into())
}

/// Offset a heading without indenting the inputs, cards or rows following it.
pub fn inset_heading<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut rect = ui.available_rect_before_wrap();
    rect.min.x = (rect.min.x + 8.0).min(rect.max.x);
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()),
        contents,
    )
    .inner
}

pub fn switch(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
        response.request_focus();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    let pill = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 19.0, rect.center().y),
        egui::vec2(38.0, 22.0),
    );
    let fill = if *value {
        primary()
    } else {
        tone(Color32::from_rgb(49, 56, 67))
    };
    ui.painter().rect_filled(pill, 11.0, fill);
    let progress = ui.ctx().animate_bool_responsive(response.id, *value);
    ui.painter().circle_filled(
        egui::pos2(
            egui::lerp((pill.left() + 11.0)..=(pill.right() - 11.0), progress),
            pill.center().y,
        ),
        8.0,
        Color32::WHITE,
    );
    let label_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 50.0, rect.top()), rect.max);
    let mut text = egui::text::LayoutJob::simple_singleline(
        label.to_owned(),
        TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    );
    text.wrap = egui::text::TextWrapping::truncate_at_width(label_rect.width().max(0.0));
    let galley = ui.painter().layout_job(text);
    ui.painter().galley(
        egui::pos2(
            label_rect.left(),
            label_rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        ui.visuals().text_color(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// egui's slider for settings, with its upright rounded handle in white and
/// the passed part of the rail filled with the accent blue, ending under the
/// handle.
pub fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
) -> egui::Response {
    // A handle wider than the widgets' rounding (8): egui ends the blue fill
    // that far past the handle's centre, so it stays hidden under it.
    const HEIGHT: f32 = 28.0;
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = HEIGHT;
        ui.visuals_mut().selection.bg_fill = accent();
        let response = ui.add(
            egui::Slider::new(value, range.clone())
                .step_by(step)
                .show_value(false)
                .trailing_fill(true),
        );
        // egui paints its handle in the rail's colour; this one is white, in
        // the same place and shape.
        let aspect = match ui.style().visuals.handle_shape {
            egui::style::HandleShape::Rect { aspect_ratio } => aspect_ratio,
            egui::style::HandleShape::Circle => 1.0,
        };
        let radius = response.rect.height() / 2.5;
        let (low, high) = (range.start().to_f64(), range.end().to_f64());
        let fraction = if high > low {
            ((value.to_f64() - low) / (high - low)).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let track = response.rect.x_range().shrink(radius * aspect);
        let x = egui::lerp(track.min..=track.max, fraction);
        let grow = ui.style().interact(&response).expansion;
        let half = egui::vec2(radius * aspect, radius) + egui::Vec2::splat(grow);
        let handle =
            egui::Rect::from_center_size(egui::pos2(x, response.rect.center().y), half * 2.0);
        // Exactly over egui's own handle: its rect and rounding.
        let rounding = ui.style().interact(&response).corner_radius;
        ui.painter().rect_filled(handle, rounding, Color32::WHITE);
        let outline = handle_stroke();
        if outline != egui::Stroke::NONE {
            ui.painter()
                .rect_stroke(handle, rounding, outline, egui::StrokeKind::Inside);
        }
        response
    })
    .inner
}

const EXPLICIT_BADGES: &str = "vk-fast-explicit-badges";
/// The gap between the explicit badge and the artists after it.
pub const EXPLICIT_GAP: f32 = 6.0;

/// Whether Settings show the explicit badge, for every list drawn this frame.
pub fn set_explicit_badges(ctx: &egui::Context, shown: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(EXPLICIT_BADGES), shown));
}

/// The size of `track`'s explicit badge at the start of an artists line in
/// `font_size`, when VK marks it explicit and Settings show the badge.
pub fn explicit_badge(
    ctx: &egui::Context,
    track: Option<&crate::model::Track>,
    font_size: f32,
) -> Option<f32> {
    let shown = ctx.data(|data| data.get_temp::<bool>(egui::Id::new(EXPLICIT_BADGES)));
    (track.is_some_and(|track| track.explicit) && shown.unwrap_or(true))
        .then(|| (font_size + 2.0).round().clamp(12.0, 16.0))
}

/// A boxed "E", like the explicit mark of other music services.
/// The letter is centred by its own outline, not by the text line around
/// it (which adds the font's ascent and descent and sat it a pixel low), and
/// the box and the letter sit on whole physical pixels.
pub fn paint_explicit_badge(painter: &egui::Painter, rect: egui::Rect) {
    let pixels = painter.pixels_per_point();
    let snap = |value: f32| (value * pixels).round() / pixels;
    let rect = egui::Rect::from_min_size(
        egui::pos2(snap(rect.left()), snap(rect.top())),
        egui::vec2(snap(rect.width()), snap(rect.height())),
    );
    painter.rect_filled(rect, 3.0, muted());
    let galley = painter.layout_no_wrap(
        "E".into(),
        FontId::new((rect.height() * 0.7).round(), semibold()),
        background(),
    );
    let offset = rect.center() - galley.mesh_bounds.center();
    painter.galley(
        egui::pos2(snap(offset.x), snap(offset.y)),
        galley,
        background(),
    );
}

/// The artists line of a track: the explicit badge first, then `contents`
/// in the width left, laid out as without the badge.
pub fn badged_line<R>(
    ui: &mut egui::Ui,
    badge: Option<f32>,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let Some(size) = badge else {
        return contents(ui);
    };
    let width = ui.available_width().max(0.0);
    let left = ui.cursor().left();
    let line = ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.add_space(size + EXPLICIT_GAP);
            ui.allocate_ui_with_layout(
                egui::vec2((width - size - EXPLICIT_GAP).max(0.0), 0.0),
                egui::Layout::top_down(egui::Align::Min),
                contents,
            )
        },
    );
    // Centred on the line drawn after it, whatever its height.
    let rows = line.inner.response.rect;
    paint_explicit_badge(
        &ui.painter().with_clip_rect(ui.clip_rect()),
        egui::Rect::from_center_size(
            egui::pos2(left + size * 0.5, rows.center().y),
            egui::vec2(size, size),
        ),
    );
    line.inner.inner
}

pub fn track_title(
    track: &crate::model::Track,
    color: Color32,
    font: FontId,
) -> egui::text::LayoutJob {
    let mut title = egui::text::LayoutJob::default();
    let (base, version) = track.title_parts();
    title.append(
        base,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
    if let Some(version) = version {
        title.append(
            &format!(" {version}"),
            0.0,
            egui::TextFormat {
                font_id: font,
                color: muted(),
                ..Default::default()
            },
        );
    }
    title
}

fn scrollbar_scope<R>(
    ui: &mut egui::Ui,
    render: impl FnOnce(&mut egui::Ui, std::sync::Arc<egui::Style>) -> R,
) -> R {
    // Apply the lighter colors to the bar while keeping content controls unchanged.
    let content_style = ui.style().clone();
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        let palette = crate::palette::current();
        visuals.widgets.inactive.bg_fill = palette.scroll_handle;
        visuals.widgets.hovered.bg_fill = palette.scroll_hovered;
        visuals.widgets.active.bg_fill = palette.scroll_active;
        render(ui, content_style)
    })
    .inner
}

/// egui gives the wheel to nothing while a widget is dragged. A dragged row
/// (moved within a list or carried to a playlist or the queue) still lets
/// the wheel scroll the list under the pointer. Call it after the contents,
/// so a list inside another one scrolls first.
fn wheel_while_dragging(ui: &mut egui::Ui) {
    if ui.ctx().dragged_id().is_none() {
        return;
    }
    let over = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|point| ui.clip_rect().contains(point));
    let delta = ui.input(|input| input.smooth_scroll_delta.y);
    if !over || delta == 0.0 {
        return;
    }
    ui.scroll_with_delta_animation(egui::vec2(0.0, delta), egui::style::ScrollAnimation::none());
    ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
}

/// After a list's contents: the wheel during a drag, and the middle-click
/// autoscroll of a list that shows its anchor, wherever the pointer is.
fn after_contents(ui: &mut egui::Ui) {
    wheel_while_dragging(ui);
    if let Some(delta) = crate::autoscroll::take_for(ui) {
        ui.scroll_with_delta_animation(
            egui::vec2(0.0, delta),
            egui::style::ScrollAnimation::none(),
        );
    }
    match crate::autoscroll::take_key_scroll_for(ui) {
        // Most of a screen, leaving a little of the previous one in view.
        Some(crate::autoscroll::KeyScroll::Page(direction)) => {
            ui.scroll_with_delta(egui::vec2(0.0, -direction * ui.clip_rect().height() * 0.85));
        }
        Some(crate::autoscroll::KeyScroll::Glide(points)) => {
            ui.scroll_with_delta_animation(
                egui::vec2(0.0, -points),
                egui::style::ScrollAnimation::none(),
            );
        }
        Some(crate::autoscroll::KeyScroll::Top) => {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(ui.min_rect().min, egui::vec2(1.0, 1.0)),
                Some(egui::Align::Min),
            );
        }
        None => {}
    }
}

pub fn scroll_area<R>(
    ui: &mut egui::Ui,
    area: egui::ScrollArea,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrollbar_scope(ui, |ui, content_style| {
        area.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_style(content_style);
                let inner = contents(ui);
                after_contents(ui);
                inner
            })
    })
}

pub fn scroll_rows<R>(
    ui: &mut egui::Ui,
    area: egui::ScrollArea,
    row_height: f32,
    rows: usize,
    contents: impl FnOnce(&mut egui::Ui, std::ops::Range<usize>) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrollbar_scope(ui, |ui, content_style| {
        area.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show_rows(ui, row_height, rows, |ui, range| {
                ui.set_style(content_style);
                let inner = contents(ui, range);
                after_contents(ui);
                inner
            })
    })
}

pub fn scroll_viewport<R>(
    ui: &mut egui::Ui,
    area: egui::ScrollArea,
    contents: impl FnOnce(&mut egui::Ui, egui::Rect) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    scrollbar_scope(ui, |ui, content_style| {
        area.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show_viewport(ui, |ui, viewport| {
                ui.set_style(content_style);
                let inner = contents(ui, viewport);
                after_contents(ui);
                inner
            })
    })
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    install_style(ctx);
}

/// Inter, plus the script fallbacks once they are loaded.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for (name, weight) in [("Inter", 400.0), ("Inter Semibold", 600.0)] {
        let mut data = FontData::from_static(include_bytes!("../assets/fonts/InterVariable.ttf"));
        data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
        fonts.font_data.insert(name.into(), data.into());
        let mut family = fallback.clone();
        family.insert(0, name.into());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .insert(0, "Inter".into());
    crate::ui_fonts::append(&mut fonts);
    ctx.set_fonts(fonts);
    crate::ui_fonts::mark(ctx);
}

fn install_style(ctx: &egui::Context) {
    ctx.set_visuals(visuals(&crate::palette::current()));
    ctx.global_style_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(14.0, semibold()));
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::new(26.0, semibold()));
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size.y = 34.0;
        style.spacing.scroll = egui::style::ScrollStyle {
            bar_width: 8.0,
            bar_inner_margin: 6.0,
            // The page's own 8-point padding is the bar's gap to the window
            // edge, the same as the content's gap on the left.
            bar_outer_margin: 0.0,
            handle_min_length: 28.0,
            ..egui::style::ScrollStyle::solid()
        };
    });
}

/// egui's widget colours in `palette`: its dark look with the app's own
/// colours, and every other grey toned to the theme.
pub fn visuals(palette: &crate::palette::Palette) -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    if palette.light {
        let light = egui::Visuals::light();
        visuals.dark_mode = false;
        visuals.window_shadow = light.window_shadow;
        visuals.popup_shadow = light.popup_shadow;
        visuals.warn_fg_color = palette.warning;
        visuals.error_fg_color = palette.error;
        visuals.text_cursor.stroke.color = palette.text;
    }
    let tone = |color: &mut Color32| *color = palette.tone(*color);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        tone(&mut widget.bg_fill);
        tone(&mut widget.weak_bg_fill);
        tone(&mut widget.bg_stroke.color);
        tone(&mut widget.fg_stroke.color);
        widget.corner_radius = egui::CornerRadius::same(8);
    }
    tone(&mut visuals.window_stroke.color);
    tone(&mut visuals.code_bg_color);
    if let Some(line) = palette.panel_line {
        // Separators, such as the player bar's top edge.
        visuals.widgets.noninteractive.bg_stroke.color = line;
    }
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.field;
    visuals.faint_bg_color = palette.surface;
    visuals.override_text_color = Some(palette.text);
    visuals.selection.bg_fill = palette.selected;
    visuals.selection.stroke = egui::Stroke::new(1.0, palette.accent);
    visuals.hyperlink_color = palette.link;
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);
    visuals.widgets.inactive.bg_fill = palette.control;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, palette.control_stroke);
    visuals.widgets.hovered.bg_fill = palette.control_hovered;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, palette.control_hovered_stroke);
    visuals.widgets.active.bg_fill = palette.selected;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, palette.accent);
    visuals
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_keys_scroll_the_list_under_the_pointer_and_home_returns_to_the_top() {
        use crate::autoscroll::{KeyScroll, request_key_scroll};
        let ctx = egui::Context::default();
        install(&ctx);
        let mut tick = 0;
        let mut frame = |ask: Option<KeyScroll>| {
            tick += 1;
            let mut offset = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    // Long frames let egui's scroll animation finish.
                    time: Some(f64::from(tick)),
                    events: vec![egui::Event::PointerMoved(egui::pos2(100.0, 100.0))],
                    ..Default::default()
                },
                |ui| {
                    // A sideways strip above the list never takes the scroll.
                    let output =
                        scroll_area(ui, egui::ScrollArea::vertical().id_salt("page"), |ui| {
                            scroll_area(
                                ui,
                                egui::ScrollArea::horizontal().id_salt("strip"),
                                |ui| {
                                    ui.allocate_exact_size(
                                        egui::vec2(900.0, 140.0),
                                        egui::Sense::hover(),
                                    );
                                },
                            );
                            ui.allocate_exact_size(egui::vec2(380.0, 3000.0), egui::Sense::hover());
                        });
                    offset = output.state.offset.y;
                    if let Some(scroll) = ask {
                        request_key_scroll(ui.ctx(), egui::pos2(100.0, 100.0), scroll);
                    }
                },
            );
            output.textures_delta.clear();
            offset
        };
        frame(None);
        frame(Some(KeyScroll::Page(1.0)));
        let mut offset = 0.0;
        for _ in 0..5 {
            offset = frame(None);
        }
        // About a screen, keeping a little of the previous one.
        assert!((offset - 300.0 * 0.85).abs() < 2.0, "{offset}");
        frame(Some(KeyScroll::Page(1.0)));
        for _ in 0..5 {
            offset = frame(None);
        }
        assert!((offset - 600.0 * 0.85).abs() < 2.0, "{offset}");
        frame(Some(KeyScroll::Page(-1.0)));
        for _ in 0..5 {
            offset = frame(None);
        }
        assert!((offset - 300.0 * 0.85).abs() < 2.0, "{offset}");
        frame(Some(KeyScroll::Top));
        for _ in 0..5 {
            offset = frame(None);
        }
        assert_eq!(offset, 0.0);
    }

    use super::*;

    #[test]
    fn default_theme_draws_egui_widgets_exactly_as_before_themes() {
        // The visuals the app always installed, written out as they were.
        let mut original = egui::Visuals::dark();
        original.panel_fill = Color32::from_rgb(15, 17, 21);
        original.window_fill = Color32::from_rgb(23, 27, 33);
        original.extreme_bg_color = Color32::from_rgb(18, 21, 27);
        original.faint_bg_color = Color32::from_rgb(23, 27, 33);
        original.override_text_color = Some(Color32::from_rgb(231, 235, 242));
        original.selection.bg_fill = Color32::from_rgb(27, 44, 67);
        original.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(54, 112, 196));
        original.hyperlink_color = Color32::from_rgb(98, 148, 219);
        original.window_corner_radius = egui::CornerRadius::same(12);
        original.menu_corner_radius = egui::CornerRadius::same(8);
        for visual in [
            &mut original.widgets.noninteractive,
            &mut original.widgets.inactive,
            &mut original.widgets.hovered,
            &mut original.widgets.active,
            &mut original.widgets.open,
        ] {
            visual.corner_radius = egui::CornerRadius::same(8);
        }
        original.widgets.inactive.bg_fill = Color32::from_rgb(32, 38, 47);
        original.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(43, 50, 61));
        original.widgets.hovered.bg_fill = Color32::from_rgb(42, 50, 63);
        original.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(69, 87, 112));
        original.widgets.active.bg_fill = Color32::from_rgb(27, 44, 67);
        original.widgets.active.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(54, 112, 196));
        assert_eq!(visuals(crate::themes::original_dark()), original);
        assert_eq!(visuals(Theme::default().palette()), original);
        // Tests never switch the global theme, so this is the default too.
        let context = egui::Context::default();
        install(&context);
        assert_eq!(context.global_style().visuals, original);
        assert_eq!(panel_line(), None);
        assert_eq!(panel_hover(), Color32::from_rgb(15, 17, 21));
    }

    #[test]
    fn oled_theme_paints_page_and_panels_black_and_separates_them_by_lines() {
        let oled = Theme::from_code("oled").unwrap().palette();
        let visuals = visuals(oled);
        assert_eq!(visuals.panel_fill, Color32::BLACK);
        assert_eq!(oled.panel, Color32::BLACK);
        // Separators (the player bar's edge) use the panel line.
        assert_eq!(
            Some(visuals.widgets.noninteractive.bg_stroke.color),
            oled.panel_line
        );
        // Menus, cards and controls still stand out from the black page.
        assert_ne!(visuals.window_fill, Color32::BLACK);
        assert_ne!(visuals.widgets.inactive.bg_fill, Color32::BLACK);
        assert_ne!(visuals.widgets.inactive.weak_bg_fill, Color32::BLACK);
        for theme in Theme::all() {
            let visuals = super::visuals(theme.palette());
            assert_eq!(visuals.panel_fill, theme.palette().background);
            assert_eq!(visuals.window_fill, theme.palette().surface);
            assert_eq!(visuals.override_text_color, Some(theme.palette().text));
        }
    }

    #[test]
    fn gradient_card_edges_fade_all_around_like_other_rounded_shapes() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.25);
        let rect = egui::Rect::from_min_size(egui::pos2(10.3, 10.6), egui::vec2(300.0, 120.0));
        let mut output = ctx.run_ui(Default::default(), |ui| {
            gradient_stops(
                ui.painter(),
                rect,
                16.0,
                &[Color32::RED, Color32::GREEN, Color32::BLUE],
            );
        });
        output.textures_delta.clear();
        let mesh = output
            .shapes
            .into_iter()
            .find_map(|shape| match shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh),
                _ => None,
            })
            .unwrap();
        let half = 0.5 / 1.25;
        let rounded = |point: egui::Pos2| {
            // Distance outside the rounded rectangle (negative inside).
            let inner = rect.shrink(16.0);
            let dx = (inner.left() - point.x)
                .max(point.x - inner.right())
                .max(0.0);
            let dy = (inner.top() - point.y)
                .max(point.y - inner.bottom())
                .max(0.0);
            let corner = egui::vec2(dx, dy).length() - 16.0;
            if dx > 0.0 && dy > 0.0 {
                corner
            } else {
                (rect.left() - point.x)
                    .max(point.x - rect.right())
                    .max(rect.top() - point.y)
                    .max(point.y - rect.bottom())
            }
        };
        for vertex in &mesh.vertices {
            let distance = rounded(vertex.pos);
            if vertex.color.a() == 0 {
                assert!((distance - half).abs() < 0.05, "outer {:?}", vertex.pos);
            } else {
                assert!((distance + half).abs() < 0.05, "inner {:?}", vertex.pos);
            }
        }
        // The straight sides fade too, not only the top and bottom.
        let side = |x: f32| {
            mesh.indices.chunks(3).any(|triangle| {
                triangle.iter().any(|i| {
                    let vertex = mesh.vertices[*i as usize];
                    vertex.color.a() == 0
                        && (vertex.pos.x - x).abs() < 0.05
                        && (vertex.pos.y - rect.center().y).abs() < rect.height() * 0.4
                })
            })
        };
        assert!(side(rect.left() - half) && side(rect.right() + half));
    }

    fn heading_layout(inset: bool) -> (egui::Rect, egui::Rect, egui::Rect) {
        let context = egui::Context::default();
        install(&context);
        let mut heading = egui::Rect::NOTHING;
        let mut action = egui::Rect::NOTHING;
        let mut row = egui::Rect::NOTHING;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                ui.horizontal(|ui| {
                    heading = if inset {
                        inset_heading(ui, |ui| ui.heading("Page heading")).rect
                    } else {
                        ui.heading("Page heading").rect
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        action = ui
                            .add_sized([100.0, 36.0], egui::Button::new("Refresh"))
                            .rect;
                    });
                });
                row = ui
                    .allocate_exact_size(egui::vec2(500.0, 54.0), egui::Sense::hover())
                    .0;
            },
        );
        output.textures_delta.clear();
        (heading, action, row)
    }

    #[test]
    fn heading_inset_moves_only_the_heading_and_preserves_following_rows_and_right_actions() {
        let normal = heading_layout(false);
        let inset = heading_layout(true);
        assert!((inset.0.left() - normal.0.left() - 8.0).abs() < 0.01);
        assert_eq!(inset.0.y_range(), normal.0.y_range());
        assert_eq!(inset.1, normal.1);
        assert_eq!(inset.2, normal.2);
    }

    #[test]
    fn wrapped_russian_body_keeps_regular_font_and_natural_spacing_in_justified_layouts() {
        let text = "Источник вернул неподходящий текст. Попробуйте позже.";
        for align in [egui::Align::Min, egui::Align::Center] {
            let context = egui::Context::default();
            install(&context);
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 180.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.style_mut().override_font_id = Some(FontId::monospace(14.0));
                    ui.with_layout(
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.add(wrapped_body(ui, text, 240.0, align, muted()));
                        },
                    );
                },
            );
            output.textures_delta.clear();
            let galley = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(shape) if shape.galley.text() == text => Some(&shape.galley),
                    _ => None,
                })
                .unwrap();
            assert!(!galley.job.justify);
            assert_eq!(
                galley.job.sections[0].format.font_id.family,
                FontFamily::Proportional
            );
            assert_eq!(galley.job.sections[0].format.font_id.size, 14.0);
        }
    }

    #[test]
    fn section_heading_aligns_with_the_existing_popular_searches_indent() {
        let context = egui::Context::default();
        install(&context);
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                inset_heading(ui, |ui| {
                    ui.label(egui::RichText::new("Section heading").size(20.0))
                });
                crate::search_ui::queries(
                    ui,
                    crate::i18n::Language::English,
                    crate::search_ui::QueryKind::Popular,
                    &["Query".into()],
                    false,
                    None,
                    None,
                );
            },
        );
        output.textures_delta.clear();
        let left = |caption: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == caption => Some(text.pos.x),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(left("Section heading"), 8.0);
        assert_eq!(left("Popular searches"), 8.0);
    }
}
