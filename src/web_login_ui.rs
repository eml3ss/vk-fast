//! Native forms for the private website helper. No password persistence.
use crate::{
    app_icon,
    i18n::Language,
    icons::{self, Icon},
    theme,
    web_login_state::{Action, Control, Step},
};
use eframe::egui::{self, Align, Color32, FontId, Rect, RichText, Sense};
use zeroize::Zeroize;
#[derive(Default)]
pub struct State {
    key: Option<(String, u64, Step)>,
    input: String,
    identifier: String,
    show_password: bool,
    focus_next: bool,
}
impl Drop for State {
    fn drop(&mut self) {
        self.input.zeroize();
        self.identifier.zeroize();
    }
}
impl State {
    pub fn clear(&mut self) {
        self.input.zeroize();
        self.identifier.zeroize();
        self.key = None;
        self.show_password = false;
        self.focus_next = false;
    }
}
/// True means the user canceled the whole login, including any CAPTCHA window.
pub fn show(ctx: &egui::Context, state: &mut State, control: &Control, language: Language) -> bool {
    let view = control.snapshot();
    let t = |ru, en| language.text(ru, en);
    let key = (view.document.clone(), view.revision, view.step);
    let changed = state.key.as_ref() != Some(&key);
    if changed {
        state.input.zeroize();
        state.show_password = false;
        state.focus_next = true;
        state.key = Some(key);
    }
    let width = (ctx.content_rect().width() - 80.0).clamp(280.0, 404.0);
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("vk-native-sign-in"))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::SURFACE)
                .inner_margin(24)
                .corner_radius(20)
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(44, 53, 67))),
        )
        .backdrop_color(Color32::from_black_alpha(180))
        .show(ctx, |ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
            let (body, _) = ui.allocate_exact_size(egui::vec2(width, 432.0), Sense::hover());
            let rect = |y, w, h| {
                Rect::from_min_size(
                    egui::pos2(body.center().x - w * 0.5, body.top() + y),
                    egui::vec2(w, h),
                )
            };
            let text = |ui: &mut egui::Ui, y, h, value: &str, size, color| {
                ui.put(
                    rect(y, width, h),
                    egui::Label::new(RichText::new(value).size(size).color(color))
                        .halign(Align::Center)
                        .wrap()
                        .selectable(false),
                );
            };
            ui.painter().text(
                rect(0.0, width, 30.0).center(),
                egui::Align2::CENTER_CENTER,
                t("Вход в VK", "Sign in to VK"),
                FontId::new(24.0, theme::semibold()),
                Color32::from_rgb(230, 235, 244),
            );
            let close = Rect::from_min_size(
                body.right_top() - egui::vec2(28.0, 0.0),
                egui::vec2(28.0, 28.0),
            );
            let response = ui.put(close, egui::Button::new("").frame(false));
            icons::paint(ui.painter(), close.shrink(7.0), Icon::Close, theme::MUTED);
            cancel |= response.on_hover_text(t("Закрыть", "Close")).clicked();
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(rect(48.0, 40.0, 40.0)),
                |ui| {
                    app_icon::show(ui, 40.0);
                },
            );
            let (heading, description) = match view.step {
                Step::Identifier | Step::Password => (
                    t("Ваш аккаунт", "Your account"),
                    t(
                        "Введите телефон или почту и пароль от VK.",
                        "Enter your phone or email and your VK password.",
                    ),
                ),
                Step::Code => (
                    t("Введите код", "Enter your code"),
                    t(
                        "Используйте код, который показал или прислал VK.",
                        "Use the code displayed or sent by VK.",
                    ),
                ),
                Step::Approval => (
                    t("Подтвердите вход", "Approve sign-in"),
                    t(
                        "Подтвердите запрос на телефоне. Здесь всё продолжится автоматически.",
                        "Approve the request on your phone. Sign-in will continue automatically.",
                    ),
                ),
                Step::Captcha => (
                    t("Проверка безопасности", "Security check"),
                    t(
                        "Пройдите капчу в небольшом окне VK поверх клиента.",
                        "Complete the CAPTCHA in the small VK window over the player.",
                    ),
                ),
                Step::Methods => (
                    t("Способ входа", "Sign-in method"),
                    t(
                        "Выберите вариант, который предлагает VK.",
                        "Choose a method offered by VK.",
                    ),
                ),
                Step::Manual => (
                    t("Дополнительный шаг VK", "Another step from VK"),
                    t(
                        "Этот шаг нужно завершить на странице VK.",
                        "Complete this step on the VK page.",
                    ),
                ),
                Step::Finishing => (
                    t("Вход подтверждён", "Sign-in approved"),
                    t("Подключаем вашу музыку…", "Connecting your music…"),
                ),
                _ => (
                    t("Подключаем VK…", "Connecting to VK…"),
                    t(
                        "Подготавливаем защищённый вход.",
                        "Preparing secure sign-in.",
                    ),
                ),
            };
            text(
                ui,
                104.0,
                26.0,
                heading,
                20.0,
                Color32::from_rgb(230, 235, 244),
            );
            text(ui, 138.0, 36.0, description, 14.0, theme::MUTED);
            let controls = width.min(320.0);
            if view.accepts_input() {
                let credentials = matches!(view.step, Step::Identifier | Step::Password);
                let identifier = credentials.then(|| {
                    let field = ui
                        .add_enabled_ui(view.step == Step::Identifier && !view.busy, |ui| {
                            ui.put(
                                rect(180.0, controls, 42.0),
                                egui::TextEdit::singleline(&mut state.identifier)
                                    .id_salt("vk-native-identifier")
                                    .font(FontId::proportional(16.0))
                                    .vertical_align(Align::Center)
                                    .char_limit(512)
                                    .hint_text(if view.step == Step::Password {
                                        t("Аккаунт выбран в VK", "Account selected in VK")
                                    } else {
                                        t("Телефон или почта", "Phone or email")
                                    })
                                    .frame(credential_frame()),
                            )
                        })
                        .inner;
                    clear_undo(ctx, field.id);
                    field
                });
                let field = ui
                    .add_enabled_ui(!view.busy, |ui| {
                        ui.put(
                            if credentials {
                                rect(232.0, controls, 42.0)
                            } else {
                                rect(192.0, controls, 48.0)
                            },
                            egui::TextEdit::singleline(&mut state.input)
                                .id_salt("vk-native-credential")
                                .password(credentials && !state.show_password)
                                .font(if view.step == Step::Code {
                                    FontId::monospace(24.0)
                                } else {
                                    FontId::proportional(16.0)
                                })
                                .horizontal_align(if view.step == Step::Code {
                                    Align::Center
                                } else {
                                    Align::Min
                                })
                                .vertical_align(Align::Center)
                                .char_limit(512)
                                .hint_text(match view.step {
                                    Step::Identifier | Step::Password => t("Пароль", "Password"),
                                    _ => t("Код", "Code"),
                                })
                                .frame(credential_frame()),
                        )
                    })
                    .inner;
                if state.focus_next && !view.busy {
                    if view.step == Step::Identifier && state.identifier.is_empty() {
                        if let Some(identifier) = &identifier {
                            identifier.request_focus();
                        }
                    } else {
                        field.request_focus();
                    }
                    state.focus_next = false;
                }
                if view.step == Step::Code && field.changed() {
                    state.input.retain(|c| c.is_ascii_digit());
                    state.input.truncate(16);
                }
                clear_undo(ctx, field.id);
                if credentials {
                    ui.put(
                        rect(278.0, 190.0, 26.0),
                        egui::Checkbox::new(
                            &mut state.show_password,
                            t("Показать пароль", "Show password"),
                        ),
                    );
                }
                if view.error {
                    text(
                        ui,
                        if credentials { 304.0 } else { 276.0 },
                        26.0,
                        t(
                            "Проверьте данные и повторите вход.",
                            "Check your details and try again.",
                        ),
                        12.0,
                        Color32::from_rgb(255, 158, 153),
                    );
                }
                let valid = if view.step == Step::Code {
                    state.input.len() == usize::from(view.code_length.max(4))
                } else {
                    !state.input.is_empty()
                        && (view.step != Step::Identifier || !state.identifier.trim().is_empty())
                };
                let submit = ui
                    .add_enabled_ui(valid && !view.busy, |ui| {
                        ui.put(
                            rect(if credentials { 332.0 } else { 310.0 }, controls, 40.0),
                            egui::Button::new(if credentials {
                                t("Войти", "Sign in")
                            } else {
                                t("Продолжить", "Continue")
                            })
                            .fill(theme::PRIMARY)
                            .corner_radius(10),
                        )
                    })
                    .inner;
                let identifier_focused = identifier
                    .as_ref()
                    .is_some_and(|r| r.has_focus() || r.lost_focus());
                let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                if identifier_focused
                    && enter_pressed
                    && !view.busy
                    && !state.identifier.trim().is_empty()
                    && state.input.is_empty()
                {
                    field.request_focus();
                }
                let enter = valid
                    && !view.busy
                    && (field.has_focus() || field.lost_focus() || identifier_focused)
                    && enter_pressed;
                if submit.clicked() || enter {
                    if view.step == Step::Identifier {
                        control.send_credentials(
                            state.identifier.trim().to_owned(),
                            std::mem::take(&mut state.input),
                        );
                    } else {
                        control.send(Action::Submit, std::mem::take(&mut state.input));
                    }
                }
            }
            if matches!(view.step, Step::Captcha | Step::Manual)
                && ui
                    .add_enabled_ui(!view.busy, |ui| {
                        ui.put(
                            rect(224.0, controls, 42.0),
                            egui::Button::new(if view.step == Step::Captcha {
                                t("Открыть капчу", "Open CAPTCHA")
                            } else {
                                t("Открыть VK", "Open VK")
                            })
                            .fill(theme::PRIMARY)
                            .corner_radius(10),
                        )
                    })
                    .inner
                    .clicked()
            {
                control.send(Action::OpenPage, String::new());
            }
            if matches!(
                view.step,
                Step::Loading | Step::Entry | Step::Approval | Step::Finishing
            ) {
                ui.put(
                    rect(232.0, 24.0, 24.0),
                    egui::Spinner::new().size(24.0).color(theme::ACCENT),
                );
            } else if view.busy {
                ui.put(
                    rect(
                        if matches!(view.step, Step::Identifier | Step::Password) {
                            308.0
                        } else {
                            280.0
                        },
                        18.0,
                        18.0,
                    ),
                    egui::Spinner::new().size(18.0).color(theme::ACCENT),
                );
            }
            if view.step == Step::Methods {
                for (index, (available, action, label)) in [
                    (
                        view.password_option,
                        Action::Password,
                        t("Войти с паролем", "Use a password"),
                    ),
                    (
                        view.other_option,
                        Action::Other,
                        t("Другой способ", "Another method"),
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    if available
                        && ui
                            .add_enabled_ui(!view.busy, |ui| {
                                ui.put(
                                    rect(196.0 + index as f32 * 52.0, controls, 40.0),
                                    egui::Button::new(label)
                                        .fill(theme::PRIMARY)
                                        .corner_radius(10),
                                )
                            })
                            .inner
                            .clicked()
                    {
                        state.input.zeroize();
                        control.send(action, String::new());
                    }
                }
            }
            let methods: Vec<_> = [
                (
                    view.password_option && view.step != Step::Methods,
                    Action::Password,
                    t("Войти с паролем", "Use a password"),
                ),
                (
                    view.other_option && view.step != Step::Methods,
                    Action::Other,
                    t("Другой способ", "Another method"),
                ),
                (view.back_option, Action::Back, t("Назад", "Back")),
            ]
            .into_iter()
            .filter(|m| m.0)
            .map(|(_, a, l)| {
                let w = ui
                    .painter()
                    .layout_no_wrap(l.into(), FontId::proportional(13.0), theme::MUTED)
                    .size()
                    .x
                    + 16.0;
                (a, l, w)
            })
            .collect();
            let total = methods.iter().map(|m| m.2).sum::<f32>()
                + 8.0 * methods.len().saturating_sub(1) as f32;
            let mut left = body.center().x - total * 0.5;
            for (action, label, w) in methods {
                let r =
                    Rect::from_min_size(egui::pos2(left, body.top() + 374.0), egui::vec2(w, 28.0));
                left += w + 8.0;
                if ui
                    .add_enabled_ui(!view.busy, |ui| {
                        ui.put(
                            r,
                            egui::Button::new(RichText::new(label).size(13.0)).frame(false),
                        )
                    })
                    .inner
                    .clicked()
                {
                    state.input.zeroize();
                    control.send(action, String::new());
                }
            }
            cancel |= ui
                .put(
                    rect(404.0, controls, 28.0),
                    egui::Button::new(RichText::new(t("Отмена", "Cancel")).color(theme::MUTED))
                        .frame(false),
                )
                .clicked();
        });
    if modal.is_top_modal
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        cancel = true;
    }
    if cancel {
        state.clear();
    }
    cancel
}

fn credential_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::BACKGROUND)
        .inner_margin(10)
        .corner_radius(10)
        .stroke(egui::Stroke::new(1.0, theme::SCROLL_HANDLE))
}

fn clear_undo(ctx: &egui::Context, id: egui::Id) {
    if let Some(mut edit) = egui::TextEdit::load_state(ctx, id) {
        edit.clear_undoer();
        egui::TextEdit::store_state(ctx, id, edit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render(
        ctx: &egui::Context,
        state: &mut State,
        control: &Control,
        language: Language,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, bool) {
        let mut canceled = false;
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 540.0),
                )),
                time: Some(time),
                focused: true,
                events,
                ..Default::default()
            },
            |_| {
                canceled = show(ctx, state, control, language);
            },
        );
        out.textures_delta.clear();
        (out, canceled)
    }
    fn texts(shape: &egui::Shape, callback: &mut impl FnMut(&egui::epaint::TextShape)) {
        match shape {
            egui::Shape::Text(t) => callback(t),
            egui::Shape::Vec(v) => {
                for s in v {
                    texts(s, callback);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn every_native_step_fits_the_minimum_window_in_both_languages() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let control = Control::default();
            let mut state = State::default();
            for (index, step) in [
                Step::Loading,
                Step::Identifier,
                Step::Password,
                Step::Code,
                Step::Approval,
                Step::Methods,
                Step::Captcha,
                Step::Manual,
                Step::Finishing,
            ]
            .into_iter()
            .enumerate()
            {
                control.publish(crate::web_login_state::Snapshot {
                    document: "synthetic".into(),
                    revision: index as u64,
                    step,
                    error: true,
                    code_length: 6,
                    password_option: true,
                    other_option: true,
                    back_option: true,
                    ..Default::default()
                });
                for frame in 0..3 {
                    let (output, cancel) = render(
                        &ctx,
                        &mut state,
                        &control,
                        language,
                        (index * 3 + frame) as f64 * 0.2,
                        Vec::new(),
                    );
                    assert!(!cancel);
                    if frame == 2 {
                        let screen =
                            Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 540.0))
                                .shrink(4.0);
                        for shape in &output.shapes {
                            texts(&shape.shape, &mut |text| {
                                assert!(
                                    text.galley.text().is_empty()
                                        || screen.contains_rect(text.visual_bounding_rect()),
                                    "Text outside minimum window: {}",
                                    text.galley.text()
                                )
                            });
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn identifier_and_password_share_one_form_and_enter_submits_both() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let control = Control::default();
            let mut state = State::default();
            control.publish(crate::web_login_state::Snapshot {
                document: "synthetic".into(),
                revision: 1,
                step: Step::Identifier,
                ..Default::default()
            });
            for i in 0..3 {
                render(
                    &ctx,
                    &mut state,
                    &control,
                    language,
                    i as f64 * 0.2,
                    Vec::new(),
                );
            }
            let (empty, _) = render(&ctx, &mut state, &control, language, 0.8, Vec::new());
            let mut hints = Vec::new();
            for shape in &empty.shapes {
                texts(&shape.shape, &mut |text| {
                    hints.push(text.galley.text().to_owned())
                });
            }
            for label in [
                language.text("Телефон или почта", "Phone or email"),
                language.text("Пароль", "Password"),
                language.text("Войти", "Sign in"),
            ] {
                assert!(hints.iter().any(|text| text == label), "Missing: {label}");
            }
            state.identifier = "user@example.test".into();
            state.input = "synthetic-secret".into();
            let (filled, _) = render(&ctx, &mut state, &control, language, 1.0, Vec::new());
            for shape in &filled.shapes {
                texts(&shape.shape, &mut |text| {
                    assert!(!text.galley.text().contains("synthetic-secret"));
                });
            }
            render(
                &ctx,
                &mut state,
                &control,
                language,
                1.2,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(state.input.is_empty());
            assert_eq!(state.identifier, "user@example.test");
            assert_eq!(control.take_command().unwrap().value, "user@example.test");
            control.publish(crate::web_login_state::Snapshot {
                document: "password-page".into(),
                revision: 4,
                step: Step::Password,
                ..Default::default()
            });
            assert_eq!(control.take_command().unwrap().value, "synthetic-secret");
        }
    }
    #[test]
    fn password_stays_masked_and_is_cleared_after_submit_and_cancel() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let control = Control::default();
        let mut state = State::default();
        control.publish(crate::web_login_state::Snapshot {
            document: "synthetic".into(),
            revision: 1,
            step: Step::Password,
            ..Default::default()
        });
        for i in 0..3 {
            render(
                &ctx,
                &mut state,
                &control,
                Language::English,
                i as f64 * 0.2,
                Vec::new(),
            );
        }
        state.input = "synthetic-secret".into();
        let (out, _) = render(
            &ctx,
            &mut state,
            &control,
            Language::English,
            1.0,
            Vec::new(),
        );
        for shape in &out.shapes {
            texts(&shape.shape, &mut |text| {
                assert!(!text.galley.text().contains("synthetic-secret"))
            });
        }
        let (_, cancel) = render(
            &ctx,
            &mut state,
            &control,
            Language::English,
            1.2,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!cancel);
        assert!(state.input.is_empty());
        let cmd = control.take_command().unwrap();
        assert_eq!(cmd.value, "synthetic-secret");
        let (_, cancel) = render(
            &ctx,
            &mut state,
            &control,
            Language::English,
            1.4,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(cancel);
        assert!(state.input.is_empty() && state.identifier.is_empty());
    }
}
