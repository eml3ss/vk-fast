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

/// The theme cards; returns whether another theme was chosen. The current
/// one is drawn in `accent`, the others in their own accents, as picking
/// them shows them.
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
    let mut choices = Theme::all().peekable();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        while choices.peek().is_some() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for choice in choices.by_ref().take(per_row) {
                    let title = choice.title(language);
                    let response = ui
                        .push_id(("theme-card", choice.code()), |ui| {
                            let selected = *current == choice;
                            let accent = if selected {
                                accent
                            } else {
                                crate::palette::THEME_ACCENT
                            };
                            theme_card(ui, choice, accent, title, selected)
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
    let mut layout = egui::text::LayoutJob::simple(
        title.to_owned(),
        label,
        if selected {
            theme::strong()
        } else {
            ui.visuals().text_color()
        },
        CARD.x,
    );
    layout.wrap.max_rows = 1;
    layout.wrap.break_anywhere = true;
    let text = ui.painter().layout_job(layout);
    let response = if text.elided {
        response.on_hover_text(title)
    } else {
        response
    };
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

/// Ready accents as round swatches, then the custom colour. `own` is the
/// active theme's own accent when it is not blue: with no accent picked it
/// shows, ringed, as the custom colour; with none, blue stands for the
/// theme's own accent.
pub fn accent_picker(
    ui: &mut egui::Ui,
    accent: &mut Color32,
    own: Option<Color32>,
    language: Language,
) -> AccentOutcome {
    let mut outcome = AccentOutcome::default();
    let theme_accent = *accent == crate::palette::THEME_ACCENT;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        for choice in Accent::ALL {
            let title = choice.title(language);
            let selected = *accent == choice.color()
                || (theme_accent && own.is_none() && choice == Accent::Blue);
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
                // Blue in a blue theme is the theme's own accent, as before.
                *accent = if choice == Accent::Blue && own.is_none() {
                    crate::palette::THEME_ACCENT
                } else {
                    choice.color()
                };
                outcome.changed = true;
            }
        }
        ui.add_space(6.0);
        // The theme's own accent counts as a custom colour of its own.
        let shown = if theme_accent {
            own.unwrap_or(Accent::Blue.color())
        } else {
            *accent
        };
        let custom = Accent::of(*accent).is_none() && (!theme_accent || own.is_some());
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
            ui.painter().rect_filled(plate, plate.height() * 0.5, shown);
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
                let mut picked = shown;
                if color_picker(ui, &mut picked) {
                    *accent = picked;
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

/// A hex colour: `#6f2fdb` or `6f2fdb`, `0x6f2fdb`, and, with `#` or `0x`,
/// the short `#fff` and `#6f2fdbff` (its alpha is dropped).
fn parse_hex(text: &str) -> Option<Color32> {
    let text = text.trim();
    let (digits, prefixed) = match text
        .strip_prefix('#')
        .or_else(|| text.strip_prefix("0x"))
        .or_else(|| text.strip_prefix("0X"))
    {
        Some(digits) => (digits, true),
        None => (text, false),
    };
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let pair = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    let single = |at: usize| {
        u8::from_str_radix(&digits[at..=at], 16)
            .ok()
            .map(|c| c * 17)
    };
    match digits.len() {
        6 => Some(Color32::from_rgb(pair(0)?, pair(2)?, pair(4)?)),
        8 if prefixed => Some(Color32::from_rgb(pair(0)?, pair(2)?, pair(4)?)),
        3 if prefixed => Some(Color32::from_rgb(single(0)?, single(1)?, single(2)?)),
        _ => None,
    }
}

/// A `#rrggbb` (or `#rrggbbaa`) inside other text, such as `color: #6F2FDB;`.
fn hex_inside(text: &str) -> Option<Color32> {
    text.match_indices('#').find_map(|(at, _)| {
        let digits: String = text[at + 1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        matches!(digits.len(), 6 | 8)
            .then(|| parse_hex(&format!("#{digits}")))
            .flatten()
    })
}

/// A whole colour in pasted text: a hex colour (also inside other text),
/// `111, 47, 219`, `111 47 219` or `rgb(111, 47, 219)`. One number alone is
/// not a colour.
fn parse_color(text: &str) -> Option<Color32> {
    let text = text.trim();
    if let Some(color) = parse_hex(text) {
        return Some(color);
    }
    let numbers: Vec<&str> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .collect();
    let rest = text
        .to_ascii_lowercase()
        .trim_start_matches("rgb")
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace() || ",;()".contains(c));
    if numbers.len() == 3 && rest {
        let channel = |index: usize| numbers[index].parse::<u8>().ok();
        return Some(Color32::from_rgb(channel(0)?, channel(1)?, channel(2)?));
    }
    hex_inside(text)
}

/// A colour typed into a field: only a full six-digit hex, so `#6f2` or
/// `111 47 2` on the way to a longer colour is not taken early.
fn typed_color(text: &str) -> Option<Color32> {
    let text = text.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() == 6 {
        return parse_hex(text);
    }
    hex_inside(text)
}

/// What an edit put into `old` to make `new`: the middle left when their
/// shared start and end are cut off.
fn inserted<'a>(old: &str, new: &'a str) -> &'a str {
    let start = old
        .char_indices()
        .zip(new.char_indices())
        .find(|((_, a), (_, b))| a != b)
        .map_or(old.len().min(new.len()), |((at, _), _)| at);
    let end = old[start..]
        .chars()
        .rev()
        .zip(new[start..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    &new[start..new.len() - end]
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

    // A whole colour pasted while no field (and no other text) is focused
    // goes to the picker too.
    let mut whole = if ui.memory(|memory| memory.focused().is_none()) {
        take_pasted_color(ui)
    } else {
        None
    };
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
                        whole = whole.or_else(|| take_pasted_color(ui));
                    }
                    let old = state.fields[index].clone();
                    let response = ui.add_sized(
                        egui::vec2(ui.available_width(), 26.0),
                        egui::TextEdit::singleline(&mut state.fields[index])
                            .id(field)
                            .horizontal_align(egui::Align::Center)
                            .vertical_align(egui::Align::Center),
                    );
                    if response.changed() {
                        let text = state.fields[index].trim().to_owned();
                        // A hex typed in, or a colour put in beside the
                        // number already there, fills all three.
                        let full = typed_color(&text).or_else(|| {
                            let put = inserted(&old, &state.fields[index]);
                            (put.chars().count() > 1)
                                .then(|| parse_color(put))
                                .flatten()
                        });
                        if let Some(full) = full {
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

/// Removes this frame's pasted text if it is a whole colour, and returns it.
fn take_pasted_color(ui: &egui::Ui) -> Option<Color32> {
    ui.input_mut(|input| {
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
    })
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
                theme: Theme::default(),
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
                    accent_picker(ui, &mut self.accent, None, Language::English);
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
        for theme in Theme::all() {
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
        assert_eq!(page.theme.code(), "light");
        assert_eq!(crate::palette::active(), Theme::default());
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
            "0x6f2fdb",
            "#6F2FDBFF",
            "color: #6F2FDB;",
            "RGB(111,47,219)",
        ] {
            assert_eq!(parse_color(text), violet, "{text:?}");
        }
        assert_eq!(parse_color("#fa0"), Some(Color32::from_rgb(255, 170, 0)));
        for text in [
            "111",
            "fa0",
            "111 47",
            "111 47 219 5",
            "300 47 219",
            "#6f2fd",
            "6f2fdbff",
            "red",
            "",
        ] {
            assert_eq!(parse_color(text), None, "{text:?}");
        }
    }

    #[test]
    fn typing_takes_a_colour_only_once_its_hex_is_whole() {
        let violet = Some(Color32::from_rgb(111, 47, 219));
        for text in ["#6f2fdb", "6f2fdb", "111#6f2fdb", "#6F2FDB "] {
            assert_eq!(typed_color(text), violet, "{text:?}");
        }
        for text in ["#6f2", "#6f2fd", "111 47 2", "111 47 219", "11"] {
            assert_eq!(typed_color(text), None, "{text:?}");
        }
        assert_eq!(inserted("111", "111#6f2fdb"), "#6f2fdb");
        assert_eq!(inserted("111", "1#6f2fdb11"), "#6f2fdb");
        assert_eq!(inserted("47", "200"), "200");
        assert_eq!(inserted("11", "111"), "1");
        assert_eq!(inserted("", "ё#6f2fdb"), "ё#6f2fdb");
    }

    #[test]
    fn a_colour_pasted_with_no_field_focused_goes_to_the_picker() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut color = Accent::Blue.color();
        picker_frame(&ctx, &mut color, Vec::new());
        picker_frame(
            &ctx,
            &mut color,
            vec![egui::Event::Paste("#6F2FDB".to_owned())],
        );
        assert_eq!(color, Color32::from_rgb(111, 47, 219));
        // Other pasted text is left alone.
        picker_frame(
            &ctx,
            &mut color,
            vec![egui::Event::Paste("hello".to_owned())],
        );
        assert_eq!(color, Color32::from_rgb(111, 47, 219));
    }

    #[test]
    fn hex_typed_after_the_number_in_a_field_converts_to_all_three() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut color = Color32::from_rgb(10, 20, 30);
        picker_frame(&ctx, &mut color, Vec::new());
        let id = egui::Id::new(PICKER).with("state").with(("channel", 0));
        ctx.memory_mut(|memory| memory.request_focus(id));
        picker_frame(&ctx, &mut color, Vec::new());
        let mut seen = Vec::new();
        for character in "#6f2fdb".chars() {
            picker_frame(
                &ctx,
                &mut color,
                vec![egui::Event::Text(character.to_string())],
            );
            seen.push(color);
        }
        // Nothing changes before the hex is whole, then all three do.
        assert!(
            seen[..6]
                .iter()
                .all(|c| *c == Color32::from_rgb(10, 20, 30)),
            "{seen:?}"
        );
        assert_eq!(color, Color32::from_rgb(111, 47, 219));
        let state = ctx
            .data(|data| data.get_temp::<PickerState>(egui::Id::new(PICKER).with("state")))
            .unwrap();
        assert_eq!(state.fields, ["111", "47", "219"].map(str::to_owned));
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
    fn a_theme_with_its_own_accent_shows_it_ringed_as_the_custom_colour() {
        let green = Color32::from_rgb(0x67, 0xe4, 0x95);
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut accent = crate::palette::THEME_ACCENT;
        let run = |accent: &mut Color32, events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 200.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    accent_picker(ui, accent, Some(green), Language::English);
                },
            );
            output.textures_delta.clear();
            let mut shapes = Vec::new();
            for shape in output.shapes {
                let mut stack = vec![shape.shape];
                while let Some(shape) = stack.pop() {
                    match shape {
                        egui::Shape::Vec(inner) => stack.extend(inner),
                        other => shapes.push(other),
                    }
                }
            }
            shapes
        };
        let shapes = run(&mut accent, Vec::new());
        let plate = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == green => Some(rect.rect),
                _ => None,
            })
            .expect("the theme's green as the custom colour");
        assert!(
            shapes.iter().any(|shape| matches!(shape,
                egui::Shape::Rect(rect) if rect.stroke.width == 2.0 && rect.rect.contains_rect(plate))),
            "ringed as chosen"
        );
        assert!(
            !shapes.iter().any(
                |shape| matches!(shape, egui::Shape::Circle(circle) if circle.stroke.width == 2.0)
            ),
            "no ready swatch is ringed, blue included"
        );
        // Blue is a real choice here: it turns the theme blue.
        let blue = egui::pos2(SWATCH * 0.5, SWATCH * 0.5);
        for pressed in [true, false] {
            run(
                &mut accent,
                vec![
                    egui::Event::PointerMoved(blue),
                    egui::Event::PointerButton {
                        pos: blue,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(accent, Accent::Blue.color());
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
        assert_eq!(
            crate::palette::active_accent(),
            crate::palette::THEME_ACCENT
        );
    }
}
