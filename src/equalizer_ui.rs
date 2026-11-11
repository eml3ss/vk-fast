//! The equalizer's settings card: a curve whose band knobs are dragged right
//! on the graph, and a preset picker with the user's own presets.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};

use crate::{
    equalizer::{self, BANDS, Builtin, MAX_DB, Settings},
    i18n::Language,
    theme,
};

const GRAPH_HEIGHT: f32 = 190.0;
const KNOB: f32 = 6.0;
/// The lowest and highest frequency drawn, in Hz: half an octave beyond the
/// outer bands on both sides, so the end knobs sit as far from either edge.
const LOW: f32 = BANDS[0] * std::f32::consts::FRAC_1_SQRT_2;
const HIGH: f32 = BANDS[BANDS.len() - 1] * std::f32::consts::SQRT_2;

#[derive(Default)]
pub struct State {
    /// The name being typed for a new preset, while that row is open.
    naming: Option<String>,
    /// Saving failed because the preset list is full.
    full: bool,
    /// The name field takes focus once, when its row opens.
    focus_name: bool,
}

/// What changed this frame.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Outcome {
    /// The sound changed: apply it to the player.
    pub sound: bool,
    /// Anything kept in the file changed.
    pub save: bool,
}

impl Outcome {
    fn sound() -> Self {
        Self {
            sound: true,
            save: true,
        }
    }
}

fn frequency_label(freq: f32) -> String {
    if freq >= 1_000.0 {
        format!("{}k", (freq / 1_000.0).round())
    } else {
        format!("{}", freq.round())
    }
}

/// "+3.5", "0", "−2" with a real minus sign.
pub fn gain_label(gain: f32) -> String {
    let text = if gain.fract() == 0.0 {
        format!("{}", gain.abs())
    } else {
        format!("{:.1}", gain.abs())
    };
    if gain > 0.0 {
        format!("+{text}")
    } else if gain < 0.0 {
        format!("−{text}")
    } else {
        text
    }
}

pub fn show(
    ui: &mut egui::Ui,
    language: Language,
    settings: &mut Settings,
    state: &mut State,
) -> Outcome {
    let t = |ru, en| language.text(ru, en);
    let mut outcome = Outcome::default();
    if theme::switch(ui, &mut settings.enabled, t("Эквалайзер", "Equalizer")).changed() {
        outcome = Outcome::sound();
    }
    ui.label(
        RichText::new(t(
            "Меняет звук только в VK Fast. Тяните точки на графике, двойной щелчок возвращает полосу к нулю. Чтобы звук не хрипел, подъёмы немного снижают общую громкость.",
            "Changes the sound in VK Fast only. Drag the points on the graph; a double click resets a band. Boosts lower the overall level a little so the sound does not distort.",
        ))
        .small()
        .color(theme::muted()),
    );
    ui.add_space(14.0);
    if graph(ui, language, settings) {
        // Shaping the sound means wanting to hear it.
        settings.enabled = true;
        outcome = Outcome::sound();
    }
    ui.add_space(14.0);
    let picked = presets(ui, language, settings, state);
    if picked.sound {
        settings.enabled = true;
    }
    outcome.sound |= picked.sound;
    outcome.save |= picked.save;
    outcome
}

/// The curve with a knob per band; true when a band changed.
fn graph(ui: &mut egui::Ui, language: Language, settings: &mut Settings) -> bool {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, GRAPH_HEIGHT), Sense::hover());
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(14.0, 16.0),
        rect.max - egui::vec2(14.0, 16.0),
    );
    let x_of = |freq: f32| plot.left() + plot.width() * (freq / LOW).ln() / (HIGH / LOW).ln();
    // Between bands the curve may pass ±12 dB a little; it stays in the panel.
    let y_of = |gain: f32| {
        (plot.center().y - gain / MAX_DB * plot.height() * 0.5)
            .clamp(rect.top() + 3.0, rect.bottom() - 3.0)
    };
    let db_at = |y: f32| (plot.center().y - y) / (plot.height() * 0.5) * MAX_DB;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, theme::background());
    let small = egui::FontId::proportional(11.0);
    for gain in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = y_of(gain);
        let color = if gain == 0.0 {
            theme::tone(Color32::from_rgb(64, 73, 86))
        } else {
            theme::tone(Color32::from_rgb(36, 42, 51))
        };
        // No scale labels: every band's value is written under the graph.
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0, color),
        );
    }
    let enabled = settings.enabled;
    let line = if enabled {
        theme::accent()
    } else {
        theme::tone(Color32::from_rgb(88, 98, 112))
    };
    // Columns: each band owns the strip around its frequency.
    let spacing = x_of(BANDS[1]) - x_of(BANDS[0]);
    let mut changed = false;
    let mut active = None;
    for (band, freq) in BANDS.into_iter().enumerate() {
        let x = x_of(freq);
        let column =
            egui::Rect::from_x_y_ranges((x - spacing * 0.5)..=(x + spacing * 0.5), rect.y_range());
        let id = ui.id().with(("equalizer-band", band));
        let response = ui.interact(column, id, Sense::click_and_drag());
        let label = format!("{} {}", frequency_label(freq), language.text("Гц", "Hz"));
        let gain = settings.gains[band];
        response.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(gain), &label));
        let mut next = gain;
        if response.double_clicked() {
            next = 0.0;
        } else if (response.dragged() || response.clicked())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            next = equalizer::clean_gain(db_at(pointer.y));
        }
        if response.has_focus() {
            ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowUp) {
                    next = equalizer::clean_gain(gain + equalizer::STEP_DB);
                }
                if input.key_pressed(egui::Key::ArrowDown) {
                    next = equalizer::clean_gain(gain - equalizer::STEP_DB);
                }
            });
        }
        if next != gain {
            settings.gains[band] = next;
            changed = true;
        }
        if response.hovered() || response.dragged() || response.has_focus() {
            active = Some(band);
        }
        response.on_hover_cursor(egui::CursorIcon::ResizeVertical);
    }
    // The curve as it sounds, with a soft fill toward 0 dB.
    let zero = y_of(0.0);
    // Each knob shows its own band's value; the filters make up for their
    // overlap, so the curve passes through every knob and moving one band
    // never moves another.
    let curve = equalizer::Curve::new(&settings.gains);
    let knob = |band: usize| egui::pos2(x_of(BANDS[band]), y_of(settings.gains[band]));
    // The line passes exactly through every knob's centre.
    let bands = BANDS.map(x_of);
    let mut xs: Vec<f32> = (0..=160)
        .map(|step| plot.left() + plot.width() * step as f32 / 160.0)
        .filter(|x| bands.iter().all(|band| (band - x).abs() >= 0.5))
        .chain(bands)
        .collect();
    xs.sort_by(f32::total_cmp);
    let points: Vec<egui::Pos2> = xs
        .into_iter()
        .map(|x| {
            let freq = LOW * (HIGH / LOW).powf((x - plot.left()) / plot.width());
            egui::pos2(x, y_of(curve.db(freq)))
        })
        .collect();
    let mut fill = egui::Mesh::default();
    let fill_color = line.gamma_multiply(0.18);
    for pair in points.windows(2) {
        let index = fill.vertices.len() as u32;
        for pos in [
            pair[0],
            egui::pos2(pair[0].x, zero),
            pair[1],
            egui::pos2(pair[1].x, zero),
        ] {
            fill.colored_vertex(pos, fill_color);
        }
        fill.add_triangle(index, index + 1, index + 2);
        fill.add_triangle(index + 1, index + 3, index + 2);
    }
    painter.add(fill);
    painter.add(egui::Shape::line(points, Stroke::new(2.0, line)));
    for band in 0..BANDS.len() {
        let center = knob(band);
        let lit = active == Some(band);
        painter.line_segment(
            [egui::pos2(center.x, zero), center],
            Stroke::new(2.0, line.gamma_multiply(if lit { 0.9 } else { 0.45 })),
        );
        painter.circle(
            center,
            if lit { KNOB + 1.5 } else { KNOB },
            if enabled {
                Color32::WHITE
            } else {
                theme::tone(Color32::from_rgb(170, 178, 190))
            },
            Stroke::new(2.0, line),
        );
    }
    if let Some(band) = active {
        let center = knob(band);
        let text = format!(
            "{} {} · {} {}",
            gain_label(settings.gains[band]),
            language.text("дБ", "dB"),
            frequency_label(BANDS[band]),
            language.text("Гц", "Hz"),
        );
        let galley = painter.layout_no_wrap(text, small.clone(), Color32::WHITE);
        let above = center.y - KNOB - 14.0 > plot.top();
        let anchor = if above {
            center - egui::vec2(0.0, KNOB + 12.0)
        } else {
            center + egui::vec2(0.0, KNOB + 12.0)
        };
        let bubble = egui::Rect::from_center_size(anchor, galley.size() + egui::vec2(12.0, 6.0));
        let shift = (rect.left() + 4.0 - bubble.left()).max(0.0)
            + (rect.right() - 4.0 - bubble.right()).min(0.0);
        let bubble = bubble.translate(egui::vec2(shift, 0.0));
        painter.rect_filled(bubble, 6.0, theme::tone(Color32::from_rgb(45, 52, 63)));
        painter.galley(
            bubble.center() - galley.size() * 0.5,
            galley,
            Color32::WHITE,
        );
    }
    // Frequencies and their gains under the graph.
    let (labels, _) = ui.allocate_exact_size(egui::vec2(width, 32.0), Sense::hover());
    for (band, freq) in BANDS.into_iter().enumerate() {
        let x = x_of(freq);
        ui.painter().text(
            egui::pos2(x, labels.top() + 7.0),
            egui::Align2::CENTER_CENTER,
            frequency_label(freq),
            egui::FontId::proportional(12.0),
            ui.visuals().text_color(),
        );
        let gain = settings.gains[band];
        ui.painter().text(
            egui::pos2(x, labels.top() + 23.0),
            egui::Align2::CENTER_CENTER,
            gain_label(gain),
            small.clone(),
            if gain == 0.0 { theme::muted() } else { line },
        );
    }
    changed
}

/// The preset picker, reset, and saving or deleting the user's presets.
fn presets(
    ui: &mut egui::Ui,
    language: Language,
    settings: &mut Settings,
    state: &mut State,
) -> Outcome {
    let t = |ru, en| language.text(ru, en);
    let mut outcome = Outcome::default();
    let builtin = settings.builtin();
    let custom = settings.custom();
    let current = builtin.map_or_else(
        || {
            custom.map_or_else(
                || t("Своя настройка", "Custom").to_owned(),
                |index| settings.presets[index].name.clone(),
            )
        },
        |preset| preset.name(language).to_owned(),
    );
    ui.horizontal_wrapped(|ui| {
        ui.label(t("Пресет", "Preset"));
        egui::ComboBox::from_id_salt("equalizer-preset")
            .width(210.0)
            // Every built-in preset without scrolling.
            .height(520.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                for preset in Builtin::ALL {
                    if ui
                        .selectable_label(builtin == Some(preset), preset.name(language))
                        .clicked()
                        && builtin != Some(preset)
                    {
                        settings.gains = preset.gains();
                        outcome = Outcome::sound();
                    }
                }
                if !settings.presets.is_empty() {
                    ui.separator();
                    ui.label(
                        RichText::new(t("Мои пресеты", "My presets"))
                            .small()
                            .color(theme::muted()),
                    );
                }
                let mut picked = None;
                for (index, preset) in settings.presets.iter().enumerate() {
                    if ui
                        .selectable_label(custom == Some(index), &preset.name)
                        .clicked()
                        && custom != Some(index)
                    {
                        picked = Some(preset.gains);
                    }
                }
                if let Some(gains) = picked {
                    settings.gains = gains;
                    outcome = Outcome::sound();
                }
            });
        if ui
            .add_enabled(
                builtin != Some(Builtin::Flat),
                egui::Button::new(t("Сбросить", "Reset")),
            )
            .clicked()
        {
            settings.gains = Builtin::Flat.gains();
            outcome = Outcome::sound();
        }
        if let Some(index) = custom {
            if ui.button(t("Удалить пресет", "Delete preset")).clicked() {
                settings.presets.remove(index);
                state.full = false;
                outcome.save = true;
            }
        } else if builtin.is_none()
            && state.naming.is_none()
            && ui
                .button(t("Сохранить как пресет", "Save as preset"))
                .clicked()
        {
            state.naming = Some(String::new());
            state.full = false;
            state.focus_name = true;
        }
    });
    let focus = std::mem::take(&mut state.focus_name);
    if let Some(name) = &mut state.naming {
        ui.add_space(8.0);
        let mut save = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            // Drawn like the music search field: a rounded fill without an
            // outline (here the page's colour, so it shows on the card),
            // exactly as tall as the buttons beside it and spaced like them.
            let height = (ui.text_style_height(&egui::TextStyle::Button)
                + 2.0 * ui.spacing().button_padding.y)
                .max(ui.spacing().interact_size.y);
            let field = ui
                .scope(|ui| {
                    let visuals = ui.visuals_mut();
                    visuals.extreme_bg_color = theme::background();
                    visuals.selection.stroke = Stroke::NONE;
                    for widget in [
                        &mut visuals.widgets.inactive,
                        &mut visuals.widgets.hovered,
                        &mut visuals.widgets.active,
                    ] {
                        widget.bg_stroke = Stroke::NONE;
                        widget.corner_radius = egui::CornerRadius::same(8);
                    }
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .id_salt("equalizer-preset-name")
                            .margin(egui::Margin::symmetric(12, 0))
                            .min_size(egui::vec2(244.0, height))
                            .desired_width(220.0)
                            .vertical_align(egui::Align::Center)
                            .hint_text(t("Название пресета", "Preset name"))
                            .char_limit(equalizer::MAX_NAME),
                    )
                })
                .inner;
            if focus {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                save = true;
            }
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            let ready = !equalizer::clean_name(name).is_empty();
            if ui
                .add_enabled(ready, egui::Button::new(t("Сохранить", "Save")))
                .clicked()
            {
                save = true;
            }
            if ui.button(t("Отмена", "Cancel")).clicked() {
                cancel = true;
            }
        });
        if save {
            if settings.save_preset(name) {
                state.naming = None;
                state.full = false;
                outcome.save = true;
            } else if !equalizer::clean_name(name).is_empty() {
                state.full = true;
            }
        } else if cancel {
            state.naming = None;
            state.full = false;
        }
    }
    if state.full {
        ui.colored_label(
            Color32::from_rgb(255, 158, 153),
            t(
                "Больше пресетов не сохранить: удалите ненужные.",
                "No room for more presets: delete some first.",
            ),
        );
    }
    // A preset saved or picked closes the naming row.
    if state.naming.is_some() && (settings.builtin().is_some() || settings.custom().is_some()) {
        state.naming = None;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_sit_on_the_drawn_curve_and_a_drag_lands_where_the_cursor_is() {
        let ctx = egui::Context::default();
        let mut settings = Settings {
            enabled: true,
            gains: [5.0, 4.5, 2.0, 0.0, -1.5, -1.0, 1.0, 3.0, 4.0, 4.5],
            presets: Vec::new(),
        };
        let frame = |events: Vec<egui::Event>, settings: &mut Settings| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    graph(ui, Language::English, settings);
                },
            );
            output.textures_delta.clear();
            let mut circles = Vec::new();
            let mut line = Vec::new();
            for shape in output.shapes {
                let mut stack = vec![shape.shape];
                while let Some(shape) = stack.pop() {
                    match shape {
                        egui::Shape::Vec(shapes) => stack.extend(shapes),
                        egui::Shape::Circle(circle) => circles.push(circle.center),
                        egui::Shape::Path(path) if path.points.len() > 100 => line = path.points,
                        _ => {}
                    }
                }
            }
            (circles, line)
        };
        let (circles, line) = frame(Vec::new(), &mut settings);
        assert_eq!(circles.len(), BANDS.len());
        for center in &circles {
            assert!(
                line.iter().any(|point| point.distance(*center) < 0.01),
                "knob {center:?} is on the line"
            );
        }
        // Dragging the 8 kHz knob 30 points up moves it 30 points up.
        let mut knobs = circles.clone();
        knobs.sort_by(|a, b| a.x.total_cmp(&b.x));
        let start = knobs[8];
        let to = start - egui::vec2(0.0, 30.0);
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            vec![egui::Event::PointerMoved(start), press(start, true)],
            &mut settings,
        );
        frame(vec![egui::Event::PointerMoved(to)], &mut settings);
        frame(vec![press(to, false)], &mut settings);
        let (circles, _) = frame(Vec::new(), &mut settings);
        let moved = circles
            .into_iter()
            .min_by(|a, b| a.distance(to).total_cmp(&b.distance(to)))
            .unwrap();
        // Within half a decibel step of the cursor.
        assert!((moved.y - to.y).abs() < 4.0, "{moved:?} vs {to:?}");
        assert!((moved.x - to.x).abs() < 0.01);
        // No other knob moved.
        let (mut after, _) = frame(Vec::new(), &mut settings);
        after.sort_by(|a, b| a.x.total_cmp(&b.x));
        for (band, (before, after)) in knobs.iter().zip(&after).enumerate() {
            if band != 8 {
                assert!(before.distance(*after) < 0.01, "band {band} moved");
            }
        }
    }

    #[test]
    fn the_end_knobs_sit_as_far_from_either_edge_of_the_graph() {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            },
            |ui| {
                graph(ui, Language::English, &mut settings);
            },
        );
        output.textures_delta.clear();
        let mut panel = None;
        let mut knobs = Vec::new();
        for shape in output.shapes {
            let mut stack = vec![shape.shape];
            while let Some(shape) = stack.pop() {
                match shape {
                    egui::Shape::Vec(shapes) => stack.extend(shapes),
                    egui::Shape::Circle(circle) => knobs.push(circle.center.x),
                    egui::Shape::Rect(rect) if rect.fill == theme::background() => {
                        panel = Some(rect.rect)
                    }
                    _ => {}
                }
            }
        }
        let panel = panel.unwrap();
        let (first, last) = (
            knobs.iter().copied().fold(f32::MAX, f32::min),
            knobs.iter().copied().fold(f32::MIN, f32::max),
        );
        assert!(((first - panel.left()) - (panel.right() - last)).abs() < 0.5);
    }

    #[test]
    fn labels_use_a_real_minus_and_short_frequencies() {
        assert_eq!(gain_label(3.5), "+3.5");
        assert_eq!(gain_label(-2.0), "−2");
        assert_eq!(gain_label(0.0), "0");
        assert_eq!(frequency_label(31.0), "31");
        assert_eq!(frequency_label(1_000.0), "1k");
        assert_eq!(frequency_label(16_000.0), "16k");
    }
}
