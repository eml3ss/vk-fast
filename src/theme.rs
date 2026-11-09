use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub const ACCENT: Color32 = Color32::from_rgb(54, 112, 196);
pub const MUTED: Color32 = Color32::from_rgb(156, 166, 181);
pub const BACKGROUND: Color32 = Color32::from_rgb(15, 17, 21);
pub const SURFACE: Color32 = Color32::from_rgb(23, 27, 33);
pub const PRIMARY: Color32 = Color32::from_rgb(34, 77, 139);
pub const SELECTED: Color32 = Color32::from_rgb(27, 44, 67);
pub const SCROLL_HANDLE: Color32 = Color32::from_rgb(57, 65, 77);

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
        PRIMARY
    } else {
        Color32::from_rgb(49, 56, 67)
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
                color: MUTED,
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
        visuals.widgets.inactive.bg_fill = SCROLL_HANDLE;
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(75, 85, 99);
        visuals.widgets.active.bg_fill = Color32::from_rgb(91, 104, 122);
        render(ui, content_style)
    })
    .inner
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
                contents(ui)
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
                contents(ui, range)
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
                contents(ui, viewport)
            })
    })
}

pub fn install(ctx: &egui::Context) {
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
    ctx.set_visuals(egui::Visuals::dark());
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
            bar_outer_margin: 2.0,
            handle_min_length: 28.0,
            ..egui::style::ScrollStyle::solid()
        };
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.window_fill = SURFACE;
        style.visuals.extreme_bg_color = Color32::from_rgb(18, 21, 27);
        style.visuals.faint_bg_color = SURFACE;
        style.visuals.override_text_color = Some(Color32::from_rgb(231, 235, 242));
        style.visuals.selection.bg_fill = SELECTED;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
        style.visuals.hyperlink_color = Color32::from_rgb(98, 148, 219);
        style.visuals.window_corner_radius = egui::CornerRadius::same(12);
        style.visuals.menu_corner_radius = egui::CornerRadius::same(8);
        for visual in [
            &mut style.visuals.widgets.noninteractive,
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            visual.corner_radius = egui::CornerRadius::same(8);
        }
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(32, 38, 47);
        style.visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(43, 50, 61));
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 50, 63);
        style.visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(69, 87, 112));
        style.visuals.widgets.active.bg_fill = SELECTED;
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
                            ui.add(wrapped_body(ui, text, 240.0, align, MUTED));
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
