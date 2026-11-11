//! Playlist creation form. The application owns the request and account state.
use eframe::egui::{self, Align, Color32, Layout, RichText};

use crate::{i18n::Language, theme};

pub struct View<'a> {
    pub language: Language,
    pub name: &'a mut String,
    pub creating: bool,
    pub error: Option<&'a str>,
    /// Set once when opening the dialog, rather than stealing focus each frame.
    pub focus_name: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Cancel,
    Submit,
}

pub fn show(ctx: &egui::Context, view: View<'_>) -> Option<Action> {
    let language = view.language;
    let width = (ctx.content_rect().width() - 80.0).clamp(216.0, 400.0);
    let mut action = None;
    let modal = egui::Modal::new(egui::Id::new("create-playlist-dialog"))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::surface())
                .inner_margin(20)
                .corner_radius(14)
                .stroke(egui::Stroke::new(
                    1.0,
                    theme::tone(Color32::from_rgb(44, 51, 62)),
                )),
        )
        .backdrop_color(Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(
                RichText::new(language.text("Новый плейлист", "New playlist"))
                    .size(22.0)
                    .family(theme::semibold()),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(language.text("Название", "Name"))
                    .small()
                    .color(theme::muted()),
            );
            let field = ui
                .add_enabled_ui(!view.creating, |ui| {
                    ui.add_sized(
                        egui::vec2(width, 36.0),
                        egui::TextEdit::singleline(view.name)
                            .id_salt("new-playlist-name")
                            .hint_text(language.text("Название плейлиста", "Playlist name"))
                            .char_limit(100)
                            .margin(egui::vec2(10.0, 8.0))
                            .vertical_align(Align::Center),
                    )
                })
                .inner;
            if view.focus_name && !view.creating {
                field.request_focus();
            }
            let valid = valid_name(view.name);
            let enter = !view.creating
                && valid
                && (field.has_focus() || field.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if let Some(error) = view.error {
                ui.add(
                    egui::Label::new(
                        RichText::new(error)
                            .small()
                            .color(Color32::from_rgb(255, 158, 153)),
                    )
                    .wrap()
                    .selectable(false),
                );
            }
            ui.add_space(18.0);
            if view.creating {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(
                        RichText::new(language.text("Создаём…", "Creating…"))
                            .small()
                            .color(theme::muted()),
                    );
                });
                ui.add_space(4.0);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let create = ui
                    .add_enabled_ui(valid && !view.creating, |ui| {
                        ui.add_sized(
                            egui::vec2(104.0, 34.0),
                            egui::Button::new(language.text("Создать", "Create"))
                                .fill(theme::primary())
                                .corner_radius(8),
                        )
                    })
                    .inner;
                if create.clicked() || enter {
                    action = Some(Action::Submit);
                }
                if ui
                    .add_enabled_ui(!view.creating, |ui| {
                        ui.add_sized(
                            egui::vec2(96.0, 34.0),
                            egui::Button::new(language.text("Отмена", "Cancel")).corner_radius(8),
                        )
                    })
                    .inner
                    .clicked()
                {
                    action = Some(Action::Cancel);
                }
            });
        });
    // An in-flight write cannot reliably be undone. Wait for its result instead
    // of dismissing it and presenting a misleading cancellation.
    if !view.creating && modal.should_close() {
        action = Some(Action::Cancel);
    }
    action
}

fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        name: String,
        language: Language,
        creating: bool,
        error: Option<String>,
        tick: u32,
        action: Option<Action>,
    }

    impl Harness {
        fn new(language: Language) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            Self {
                ctx,
                name: String::new(),
                language,
                creating: false,
                error: None,
                tick: 0,
                action: None,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.tick += 1;
            let ctx = self.ctx.clone();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    time: Some(f64::from(self.tick) * 0.1),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |_| {
                    self.action = show(
                        &ctx,
                        View {
                            language: self.language,
                            name: &mut self.name,
                            creating: self.creating,
                            error: self.error.as_deref(),
                            focus_name: self.tick == 1,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            output
        }

        fn warm(&mut self) -> egui::FullOutput {
            self.frame(Vec::new());
            self.frame(Vec::new());
            self.frame(Vec::new())
        }

        fn click(&mut self, point: egui::Pos2) -> egui::FullOutput {
            self.frame(pointer(point, true));
            self.frame(pointer(point, false))
        }
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn key(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    fn shapes(output: &egui::FullOutput) -> Vec<&egui::Shape> {
        fn flatten<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::Shape>) {
            match shape {
                egui::Shape::Vec(children) => {
                    for child in children {
                        flatten(child, out);
                    }
                }
                _ => out.push(shape),
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            flatten(&shape.shape, &mut out);
        }
        out
    }

    fn label_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        shapes(output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing {label}"))
    }

    #[test]
    fn creation_dialog_is_centered_with_name_and_blue_primary_button_in_both_languages() {
        for language in [Language::Russian, Language::English] {
            let mut h = Harness::new(language);
            h.name = "Evening tracks".into();
            let output = h.warm();
            let panel = shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == theme::surface() && rect.corner_radius.nw == 14 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("Rounded form surface");
            assert!(panel.center().distance(egui::pos2(400.0, 300.0)) <= 1.0);
            assert!(panel.contains_rect(label_rect(
                &output,
                language.text("Новый плейлист", "New playlist")
            )));
            assert!(panel.contains_rect(label_rect(&output, language.text("Название", "Name"))));
            let create = label_rect(&output, language.text("Создать", "Create"));
            let cancel = label_rect(&output, language.text("Отмена", "Cancel"));
            assert!(cancel.right() < create.left());
            let button = |text: egui::Rect| {
                shapes(&output)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if (32.0..=40.0).contains(&rect.rect.height())
                                && rect.rect.contains_rect(text) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Button background")
            };
            assert!((button(cancel).center().y - button(create).center().y).abs() <= 1.0);
            assert!(shapes(&output).into_iter().any(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == theme::primary() && rect.rect.contains_rect(create))));
            h.click(create.center());
            assert_eq!(h.action, Some(Action::Submit));
        }
    }

    #[test]
    fn blank_names_cannot_submit_and_keyboard_input_can_create_or_cancel() {
        let mut h = Harness::new(Language::English);
        h.name = "   ".into();
        let output = h.warm();
        let create = label_rect(&output, "Create");
        h.click(create.center());
        assert!(h.action.is_none());
        h.frame(key(egui::Key::Enter));
        assert!(h.action.is_none());

        let mut h = Harness::new(Language::English);
        h.warm();
        h.frame(vec![egui::Event::Text("Keyboard playlist".into())]);
        assert_eq!(h.name, "Keyboard playlist");
        h.frame(key(egui::Key::Enter));
        assert_eq!(h.action, Some(Action::Submit));

        let mut h = Harness::new(Language::English);
        h.warm();
        h.frame(key(egui::Key::Escape));
        assert_eq!(h.action, Some(Action::Cancel));
        assert!(valid_name(&"я".repeat(100)));
        assert!(!valid_name(&"я".repeat(101)));
        assert!(!valid_name("Hidden\u{0}control"));
    }

    #[test]
    fn creating_prevents_duplicate_submit_and_dismissal_while_failure_keeps_the_name() {
        let mut h = Harness::new(Language::English);
        h.name = "Preserve this name".into();
        h.creating = true;
        let output = h.warm();
        label_rect(&output, "Creating…");
        for label in ["Create", "Cancel"] {
            h.click(label_rect(&output, label).center());
            assert!(h.action.is_none());
        }
        h.frame(key(egui::Key::Enter));
        assert!(h.action.is_none());
        h.frame(key(egui::Key::Escape));
        assert!(h.action.is_none());
        h.click(egui::pos2(12.0, 12.0));
        assert!(h.action.is_none());
        h.creating = false;
        h.error = Some("VK rejected the request".into());
        let output = h.frame(Vec::new());
        label_rect(&output, "VK rejected the request");
        assert_eq!(h.name, "Preserve this name");
        h.click(label_rect(&output, "Cancel").center());
        assert_eq!(h.action, Some(Action::Cancel));
    }
}
