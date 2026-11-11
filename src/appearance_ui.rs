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
                if color_picker(ui, accent) {
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

const PICKER_WIDTH: f32 = 260.0;
/// The side of the saturation and value square's texture, in pixels.
const PLANE: usize = 64;

/// The picker between frames: its hue survives greys and black, which have
/// none, and a field keeps what is being typed in it.
#[derive(Clone)]
struct PickerState {
    /// The colour the hue and fields below were made for.
    color: Color32,
    hsv: [f32; 3],
    fields: [String; 3],
}

impl PickerState {
    fn new(color: Color32, hue: Option<f32>) -> Self {
        let mut hsv = hsv(color);
        if hsv[1] == 0.0 || hsv[2] == 0.0 {
            hsv[0] = hue.unwrap_or(hsv[0]);
        }
        Self {
            color,
            hsv,
            fields: channels(color),
        }
    }
}

fn channels(color: Color32) -> [String; 3] {
    [color.r(), color.g(), color.b()].map(|channel| channel.to_string())
}

/// Hue (0..1), saturation and value of `color`.
fn hsv(color: Color32) -> [f32; 3] {
    let [r, g, b] = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    [hue, saturation, max]
}

fn from_hsv([hue, saturation, value]: [f32; 3]) -> Color32 {
    let sector = hue.rem_euclid(1.0) * 6.0;
    let chroma = value * saturation;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let channel = |c: f32| ((c + value - chroma) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(channel(r), channel(g), channel(b))
}

/// A whole colour in text: `#6f2fdb`, `6f2fdb`, `111, 47, 219`,
/// `111 47 219` or `rgb(111, 47, 219)`. One number alone is not a colour.
fn parse_color(text: &str) -> Option<Color32> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        return Some(Color32::from_rgb(channel(0)?, channel(2)?, channel(4)?));
    }
    let numbers: Vec<&str> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .collect();
    let rest = text
        .trim_start_matches("rgb")
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace() || ",;()".contains(c));
    if numbers.len() != 3 || !rest {
        return None;
    }
    let channel = |index: usize| numbers[index].parse::<u8>().ok();
    Some(Color32::from_rgb(channel(0)?, channel(1)?, channel(2)?))
}

/// The accent's own colour picker, rounded like the rest of the app (egui's
/// keeps square corners): R, G and B fields, the colour, the saturation and
/// value square and the hue bar. A whole colour pasted into any field fills
/// all three.
fn color_picker(ui: &mut egui::Ui, color: &mut Color32) -> bool {
    let id = egui::Id::new(PICKER).with("state");
    let saved = ui.data(|data| data.get_temp::<PickerState>(id));
    let mut state = match saved {
        Some(state) if state.color == *color => state,
        saved => PickerState::new(*color, saved.map(|state| state.hsv[0])),
    };
    let before = *color;
    ui.set_width(PICKER_WIDTH);
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

    let mut whole = None;
    ui.horizontal(|ui| {
        // Three equal cells across the picker, each a name and its field.
        let gap = 8.0;
        ui.spacing_mut().item_spacing.x = gap;
        let cell = (PICKER_WIDTH - 2.0 * gap) / 3.0;
        for (index, name) in ["R", "G", "B"].into_iter().enumerate() {
            ui.allocate_ui_with_layout(
                egui::vec2(cell, 26.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(egui::RichText::new(name).color(theme::muted()));
                    let field = id.with(("channel", index));
                    if ui.memory(|memory| memory.has_focus(field)) {
                        // Taken before the field sees it: it would hold all of it.
                        let pasted = ui.input_mut(|input| {
                            let mut found = None;
                            input.events.retain(|event| match event {
                                egui::Event::Paste(text) => match parse_color(text) {
                                    Some(color) => {
                                        found = Some(color);
                                        false
                                    }
                                    None => true,
                                },
                                _ => true,
                            });
                            found
                        });
                        whole = whole.or(pasted);
                    }
                    let response = ui.add_sized(
                        egui::vec2(ui.available_width(), 26.0),
                        egui::TextEdit::singleline(&mut state.fields[index])
                            .id(field)
                            .horizontal_align(egui::Align::Center)
                            .vertical_align(egui::Align::Center),
                    );
                    if response.changed() {
                        let text = state.fields[index].trim().to_owned();
                        if let Some(full) = parse_color(&text) {
                            whole = Some(full);
                        } else if let Ok(value) = text.parse::<u8>() {
                            let mut channels = [color.r(), color.g(), color.b()];
                            channels[index] = value;
                            *color = Color32::from_rgb(channels[0], channels[1], channels[2]);
                            let hue = state.hsv[0];
                            let fields = state.fields.clone();
                            state = PickerState::new(*color, Some(hue));
                            // The field keeps what is typed in it, such as "007".
                            state.fields = fields;
                        }
                    }
                    if response.lost_focus() {
                        state.fields[index] = channels(*color)[index].clone();
                    }
                },
            );
        }
    });
    if let Some(full) = whole {
        *color = full;
        state = PickerState::new(full, Some(state.hsv[0]));
    }

    let (preview, _) = ui.allocate_exact_size(egui::vec2(PICKER_WIDTH, 28.0), Sense::hover());
    ui.painter().rect_filled(preview, 8.0, *color);

    // Saturation from left to right, value from top to bottom.
    let (plane, response) = ui.allocate_exact_size(
        egui::vec2(PICKER_WIDTH, PICKER_WIDTH * 0.8),
        Sense::click_and_drag(),
    );
    if let Some(point) = response.interact_pointer_pos() {
        state.hsv[1] = ((point.x - plane.left()) / plane.width()).clamp(0.0, 1.0);
        state.hsv[2] = 1.0 - ((point.y - plane.top()) / plane.height()).clamp(0.0, 1.0);
        *color = from_hsv(state.hsv);
    }
    let texture = plane_texture(ui.ctx(), state.hsv[0]);
    let texel = 0.5 / PLANE as f32;
    ui.painter().add(
        egui::epaint::RectShape::filled(plane, 10.0, Color32::WHITE).with_texture(
            texture.id(),
            egui::Rect::from_min_max(
                egui::pos2(texel, texel),
                egui::pos2(1.0 - texel, 1.0 - texel),
            ),
        ),
    );
    knob(
        ui.painter(),
        egui::pos2(
            egui::lerp(plane.x_range(), state.hsv[1]),
            egui::lerp(plane.y_range(), 1.0 - state.hsv[2]),
        ),
        *color,
    );

    let (bar, response) =
        ui.allocate_exact_size(egui::vec2(PICKER_WIDTH, 20.0), Sense::click_and_drag());
    // Knobs at the ends stay inside the rounded bar.
    let track = bar.x_range().shrink(bar.height() * 0.5);
    if let Some(point) = response.interact_pointer_pos() {
        state.hsv[0] = ((point.x - track.min) / track.span()).clamp(0.0, 1.0);
        *color = from_hsv(state.hsv);
    }
    let hues: Vec<Color32> = (0..=6)
        .map(|step| from_hsv([step as f32 / 6.0, 1.0, 1.0]))
        .collect();
    theme::gradient_stops(ui.painter(), bar, bar.height() * 0.5, &hues);
    knob(
        ui.painter(),
        egui::pos2(egui::lerp(track, state.hsv[0]), bar.center().y),
        from_hsv([state.hsv[0], 1.0, 1.0]),
    );

    if *color != before {
        let typing =
            (0..3).find(|index| ui.memory(|memory| memory.has_focus(id.with(("channel", *index)))));
        let fields = state.fields.clone();
        state.fields = channels(*color);
        // The field being typed in keeps its text.
        if let Some(index) = typing.filter(|_| whole.is_none()) {
            state.fields[index] = fields[index].clone();
        }
    }
    state.color = *color;
    ui.data_mut(|data| data.insert_temp(id, state));
    *color != before
}

/// A round handle filled with the colour under it, ringed in white with a
/// soft dark edge that keeps it visible on white.
fn knob(painter: &egui::Painter, center: egui::Pos2, fill: Color32) {
    painter.circle_filled(center, 10.5, Color32::from_black_alpha(70));
    painter.circle(center, 9.0, fill, Stroke::new(2.0, Color32::WHITE));
}

/// The saturation and value square of `hue`, made again only when the hue
/// changes.
fn plane_texture(ctx: &egui::Context, hue: f32) -> egui::TextureHandle {
    let id = egui::Id::new(PICKER).with("plane");
    let key = (hue * 3600.0).round() as u32;
    if let Some((cached, texture)) =
        ctx.data(|data| data.get_temp::<(u32, egui::TextureHandle)>(id))
        && cached == key
    {
        return texture;
    }
    let pixels = (0..PLANE * PLANE)
        .map(|index| {
            let (x, y) = (index % PLANE, index / PLANE);
            let edge = (PLANE - 1) as f32;
            from_hsv([hue, x as f32 / edge, 1.0 - y as f32 / edge])
        })
        .collect();
    let texture = ctx.load_texture(
        "vk-fast-accent-plane",
        egui::ColorImage::new([PLANE, PLANE], pixels),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, (key, texture.clone())));
    texture
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
    fn whole_colours_are_read_from_hex_and_three_numbers_but_not_one_number() {
        let violet = Some(Color32::from_rgb(111, 47, 219));
        for text in [
            "#6f2fdb",
            "6F2FDB",
            " #6f2fdb\n",
            "111, 47, 219",
            "111 47 219",
            "111;47;219",
            "rgb(111, 47, 219)",
        ] {
            assert_eq!(parse_color(text), violet, "{text:?}");
        }
        for text in [
            "111",
            "111 47",
            "111 47 219 5",
            "300 47 219",
            "#6f2fd",
            "red",
            "",
        ] {
            assert_eq!(parse_color(text), None, "{text:?}");
        }
    }

    #[test]
    fn hsv_round_trips_every_channel_value() {
        for value in (0..=255u8).step_by(5) {
            for color in [
                Color32::from_rgb(value, 47, 219),
                Color32::from_rgb(219, value, 47),
                Color32::from_gray(value),
            ] {
                assert_eq!(from_hsv(hsv(color)), color);
            }
        }
    }

    /// The picker alone in a popup-sized column.
    fn picker_frame(ctx: &egui::Context, color: &mut Color32, events: Vec<egui::Event>) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 500.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                color_picker(ui, color);
            },
        );
        output.textures_delta.clear();
    }

    #[test]
    fn a_whole_colour_pasted_into_one_field_fills_all_three() {
        for (field, text) in [(0, "#6f2fdb"), (1, "111, 47, 219"), (2, "111 47 219")] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut color = Accent::Blue.color();
            picker_frame(&ctx, &mut color, Vec::new());
            let id = egui::Id::new(PICKER).with("state").with(("channel", field));
            ctx.memory_mut(|memory| memory.request_focus(id));
            picker_frame(&ctx, &mut color, Vec::new());
            picker_frame(&ctx, &mut color, vec![egui::Event::Paste(text.to_owned())]);
            assert_eq!(color, Color32::from_rgb(111, 47, 219), "{text}");
            let state = ctx
                .data(|data| data.get_temp::<PickerState>(egui::Id::new(PICKER).with("state")))
                .unwrap();
            assert_eq!(state.fields, ["111", "47", "219"].map(str::to_owned));
        }
    }

    #[test]
    fn one_number_pasted_or_typed_changes_only_its_own_channel() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut color = Color32::from_rgb(10, 20, 30);
        picker_frame(&ctx, &mut color, Vec::new());
        let id = egui::Id::new(PICKER).with("state").with(("channel", 1));
        ctx.memory_mut(|memory| memory.request_focus(id));
        picker_frame(&ctx, &mut color, Vec::new());
        // Select the field's text, then paste one number over it.
        picker_frame(
            &ctx,
            &mut color,
            vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
        );
        picker_frame(&ctx, &mut color, vec![egui::Event::Paste("200".to_owned())]);
        assert_eq!(color, Color32::from_rgb(10, 200, 30));
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
