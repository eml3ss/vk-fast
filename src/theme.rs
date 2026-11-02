use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub const ACCENT: Color32 = Color32::from_rgb(54, 112, 196);
pub const MUTED: Color32 = Color32::from_rgb(156, 166, 181);
pub const BACKGROUND: Color32 = Color32::from_rgb(15, 17, 21);
pub const SURFACE: Color32 = Color32::from_rgb(23, 27, 33);
pub const PRIMARY: Color32 = Color32::from_rgb(34, 77, 139);
pub const SELECTED: Color32 = Color32::from_rgb(27, 44, 67);
pub const SCROLL_HANDLE: Color32 = Color32::from_rgb(57, 65, 77);

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
