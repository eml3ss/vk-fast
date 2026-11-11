//! The Appearance settings: theme cards with a small picture of the window
//! in each theme, and the accent colour as ready swatches or any colour.

use eframe::egui::{self, Color32, Sense, Stroke};

use crate::{
    i18n::Language,
    palette::{Accent, Theme},
    theme,
};

const CARD: egui::Vec2 = egui::vec2(132.0, 84.0);
const SWATCH: f32 = 28.0;
/// The custom accent's colour picker popup.
pub const PICKER: &str = "vk-fast-accent-picker";

/// The theme cards; returns whether another theme was chosen.
pub fn theme_cards(
    ui: &mut egui::Ui,
    current: &mut Theme,
    accent: Color32,
    language: Language,
) -> bool {
    let mut changed = false;
    let gap = 12.0;
    // Rows of whole cards: as many as fit in the width, the rest below.
    let per_row = (((ui.available_width() + gap) / (CARD.x + gap)).floor() as usize).max(1);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        for row in Theme::ALL.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &choice in row {
                    let title = choice.title(language);
                    let response = ui
                        .push_id(("theme-card", choice.code()), |ui| {
                            theme_card(ui, choice, accent, title, *current == choice)
                        })
                        .inner;
                    if response.clicked() && *current != choice {
                        *current = choice;
                        changed = true;
                    }
                }
            });
        }
    });
    changed
}

fn theme_card(
    ui: &mut egui::Ui,
    choice: Theme,
    accent: Color32,
    title: &str,
    selected: bool,
) -> egui::Response {
    let label = egui::FontId::proportional(13.0);
    let label_height = 22.0;
    let (rect, response) =
        ui.allocate_exact_size(CARD + egui::vec2(0.0, label_height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, title)
    });
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let preview = egui::Rect::from_min_size(rect.min, CARD);
    paint_preview(ui.painter(), preview, &choice.palette().with_accent(accent));
    // The outline keeps a light card apart from a light page, and shows the
    // chosen one in the accent.
    let (width, color) = if selected {
        (2.0, theme::accent())
    } else if response.hovered() || response.has_focus() {
        (1.0, theme::tone(Color32::from_rgb(69, 87, 112)))
    } else {
        (1.0, theme::tone(Color32::from_rgb(43, 50, 61)))
    };
    ui.painter().rect_stroke(
        preview,
        10.0,
        Stroke::new(width, color),
        egui::StrokeKind::Outside,
    );
    let text = ui.painter().layout_no_wrap(
        title.to_owned(),
        label,
        if selected {
            theme::strong()
        } else {
            ui.visuals().text_color()
        },
    );
    ui.painter().galley(
        egui::pos2(
            rect.center().x - text.size().x * 0.5,
            preview.bottom() + (label_height - text.size().y) * 0.5 + 2.0,
        ),
        text,
        Color32::PLACEHOLDER,
    );
    response
}

/// A small picture of the window in `palette`: sidebar, page with a card and
/// the playing row, and the player bar.
fn paint_preview(painter: &egui::Painter, rect: egui::Rect, palette: &crate::palette::Palette) {
    let painter = painter.with_clip_rect(rect);
    painter.rect_filled(rect, 10.0, palette.background);
    let side = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 34.0, rect.bottom()));
    let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 16.0), rect.max);
    painter.rect_filled(
        side,
        egui::CornerRadius {
            nw: 10,
            sw: 10,
            ..Default::default()
        },
        palette.panel,
    );
    painter.rect_filled(
        bar,
        egui::CornerRadius {
            sw: 10,
            se: 10,
            ..Default::default()
        },
        palette.panel,
    );
    if let Some(line) = palette.panel_line {
        painter.line_segment(
            [side.right_top(), egui::pos2(side.right(), bar.top())],
            Stroke::new(1.0, line),
        );
        painter.line_segment([bar.left_top(), bar.right_top()], Stroke::new(1.0, line));
    }
    // The sidebar: a selected item and two more.
    painter.rect_filled(
        egui::Rect::from_min_size(side.min + egui::vec2(5.0, 8.0), egui::vec2(24.0, 7.0)),
        3.0,
        palette.selected,
    );
    for row in 0..2 {
        painter.rect_filled(
            egui::Rect::from_min_size(
                side.min + egui::vec2(8.0, 21.0 + row as f32 * 9.0),
                egui::vec2(16.0, 3.0),
            ),
            1.5,
            palette.muted.gamma_multiply(0.7),
        );
    }
    // The page: a heading, a card with the playing row in the accent.
    let page = egui::Rect::from_min_max(
        egui::pos2(side.right() + 8.0, rect.top() + 8.0),
        egui::pos2(rect.right() - 8.0, bar.top() - 6.0),
    );
    painter.rect_filled(
        egui::Rect::from_min_size(page.min, egui::vec2(38.0, 5.0)),
        2.5,
        palette.text,
    );
    let card = egui::Rect::from_min_max(page.min + egui::vec2(0.0, 11.0), page.max);
    painter.rect_filled(card, 5.0, palette.surface);
    if palette.surface == palette.background || palette.light {
        painter.rect_stroke(
            card,
            5.0,
            Stroke::new(1.0, palette.tone(Color32::from_rgb(43, 50, 61))),
            egui::StrokeKind::Inside,
        );
    }
    for row in 0..3 {
        let top = card.top() + 7.0 + row as f32 * 10.0;
        let color = if row == 0 {
            palette.accent
        } else {
            palette.text.gamma_multiply(0.85)
        };
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(card.left() + 6.0, top), egui::vec2(34.0, 3.0)),
            1.5,
            color,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(card.left() + 44.0, top), egui::vec2(22.0, 3.0)),
            1.5,
            palette.muted.gamma_multiply(0.7),
        );
    }
    // The player bar: its Play button and the passed part of the seek bar.
    painter.circle_filled(bar.center(), 5.0, palette.primary);
    let seek = egui::Rect::from_center_size(
        egui::pos2(bar.center().x + 30.0, bar.center().y),
        egui::vec2(36.0, 2.0),
    );
    painter.rect_filled(seek, 1.0, palette.tone(Color32::from_rgb(43, 49, 59)));
    painter.rect_filled(
        egui::Rect::from_min_size(seek.min, egui::vec2(16.0, 2.0)),
        1.0,
        palette.accent,
    );
}

/// What the accent picker did this frame.
#[derive(Default)]
pub struct AccentOutcome {
    pub changed: bool,
    /// A colour is being dragged in the picker: save once it is let go.
    pub dragging: bool,
}

/// Ready accents as round swatches, then egui's own colour button for any
/// other colour.
pub fn accent_picker(ui: &mut egui::Ui, accent: &mut Color32, language: Language) -> AccentOutcome {
    let mut outcome = AccentOutcome::default();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        for choice in Accent::ALL {
            let title = choice.title(language);
            let selected = *accent == choice.color();
            let (rect, response) =
                ui.allocate_exact_size(egui::Vec2::splat(SWATCH), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, title)
            });
            let response = response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(title);
            if ui.is_rect_visible(rect) {
                let color = choice.color();
                let radius = SWATCH * 0.5 - 4.0;
                let radius = if response.hovered() {
                    radius + 1.0
                } else {
                    radius
                };
                ui.painter().circle_filled(rect.center(), radius, color);
                if selected {
                    ui.painter().circle_stroke(
                        rect.center(),
                        SWATCH * 0.5 - 1.0,
                        Stroke::new(2.0, theme::strong()),
                    );
                }
            }
            if response.clicked() && !selected {
                *accent = choice.color();
                outcome.changed = true;
            }
        }
        ui.add_space(6.0);
        let custom = Accent::of(*accent).is_none();
        // The picked colour as a rounded plate drawn like the swatches
        // (egui's own colour button keeps its corners nearly square); it
        // opens egui's colour picker.
        let popup = egui::Id::new(PICKER);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(SWATCH * 1.5, SWATCH), Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if ui.is_rect_visible(rect) {
            let hovered = response.hovered() || egui::Popup::is_id_open(ui.ctx(), popup);
            let plate = rect.shrink(if hovered { 3.0 } else { 4.0 });
            ui.painter()
                .rect_filled(plate, plate.height() * 0.5, *accent);
            if custom {
                // The picked colour is ringed like a chosen swatch.
                let ring = rect.shrink(1.0);
                ui.painter().rect_stroke(
                    ring,
                    ring.height() * 0.5,
                    Stroke::new(2.0, theme::strong()),
                    egui::StrokeKind::Middle,
                );
            }
        }
        egui::Popup::menu(&response)
            .id(popup)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.spacing_mut().slider_width = 275.0;
                if egui::color_picker::color_picker_color32(
                    ui,
                    accent,
                    egui::color_picker::Alpha::Opaque,
                ) {
                    outcome.changed = true;
                }
            });
        ui.label(language.text("Свой цвет", "Custom colour"));
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::ColorButton,
                true,
                custom,
                language.text("Свой цвет", "Custom colour"),
            )
        });
        outcome.dragging = ui.ctx().egui_is_using_pointer();
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Page {
        ctx: egui::Context,
        theme: Theme,
        accent: Color32,
    }

    impl Page {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            Self {
                ctx,
                theme: Theme::Dark,
                accent: Accent::Blue.color(),
            }
        }

        /// One frame of both pickers in a 600-point column; the picked
        /// theme and accent stay local (the global theme is never switched).
        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    theme_cards(ui, &mut self.theme, self.accent, Language::English);
                    accent_picker(ui, &mut self.accent, Language::English);
                },
            );
            output.textures_delta.clear();
            output
        }

        fn click(&mut self, point: egui::Pos2) {
            let press = |pressed| egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(point), press(true)]);
            self.frame(vec![press(false)]);
        }
    }

    #[test]
    fn theme_cards_wrap_to_the_width_and_a_click_picks_a_theme() {
        let mut page = Page::new();
        let output = page.frame(Vec::new());
        // Every card's title is shown, and no card reaches past the column.
        for theme in Theme::ALL {
            let title = theme.title(Language::English);
            let shape = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == title => Some(text.pos),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{title} is shown"));
            assert!(shape.x < 600.0 - CARD.x * 0.25, "{title} at {shape:?}");
        }
        // 600 points hold four cards: the light theme starts a second row.
        let light = egui::pos2(CARD.x * 0.5, CARD.y * 1.5 + 22.0 + 12.0);
        page.click(light);
        assert_eq!(page.theme, Theme::Light);
        assert_eq!(crate::palette::active(), Theme::Dark);
    }

    #[test]
    fn a_swatch_click_picks_its_accent_and_any_colour_counts_as_custom() {
        let mut page = Page::new();
        page.frame(Vec::new());
        // The second swatch (violet), in the row under two rows of cards.
        let top = 2.0 * (CARD.y + 22.0) + 2.0 * 12.0;
        let violet = egui::pos2(SWATCH * 1.5 + 10.0, top + SWATCH * 0.5);
        page.click(violet);
        assert_eq!(page.accent, Accent::Violet.color());
        assert_eq!(Accent::of(page.accent), Some(Accent::Violet));
        page.accent = Color32::from_rgb(1, 2, 3);
        assert_eq!(Accent::of(page.accent), None);
        assert_eq!(crate::palette::active_accent(), Accent::Blue.color());
    }
}
