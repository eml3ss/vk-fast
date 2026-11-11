//! A compact, staged QR sign-in dialog. Authentication stays in the app worker.
use eframe::egui::{self, Align, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Vec2};

use crate::{
    i18n::Language,
    icons::{self, Icon},
    qr_auth::Stage,
    theme,
};

const TEXT: Color32 = Color32::from_rgb(230, 235, 244);
const BORDER: Color32 = Color32::from_rgb(44, 53, 67);
const SOFT_BLUE: Color32 = Color32::from_rgb(149, 185, 236);

pub struct View<'a> {
    pub language: Language,
    pub texture: Option<&'a egui::TextureHandle>,
    pub stage: Stage,
    pub pending: bool,
    pub remaining: Option<u64>,
    pub code_entry: bool,
    pub code: &'a mut String,
    pub error: Option<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Cancel,
    Refresh,
    Submit,
}

pub type Layout = (Rect, Rect, Option<[Rect; 2]>, [Rect; 2]);

pub struct Output {
    pub action: Option<Action>,
    pub layout: Layout,
}

fn label(ui: &mut egui::Ui, rect: Rect, text: &str, size: f32, color: Color32, centered: bool) {
    if !centered {
        let galley =
            ui.painter()
                .layout(text.into(), FontId::proportional(size), color, rect.width());
        let position = egui::pos2(
            rect.left(),
            rect.top() + (rect.height() - galley.size().y).max(0.0) * 0.5,
        );
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .galley(position, galley, color);
        return;
    }
    ui.put(
        rect,
        egui::Label::new(RichText::new(text).size(size).color(color))
            .halign(if centered { Align::Center } else { Align::Min })
            .wrap()
            .selectable(false),
    );
}

fn button(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &str,
    primary: bool,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        ui.put(
            rect,
            egui::Button::new(RichText::new(text).size(14.0).family(theme::semibold()))
                .fill(if primary {
                    theme::primary()
                } else {
                    theme::tone(Color32::from_rgb(31, 38, 48))
                })
                .stroke(if primary {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, theme::tone(BORDER))
                })
                .corner_radius(10),
        )
    })
    .inner
}

fn cancel_button(ui: &mut egui::Ui, rect: Rect, language: Language) -> egui::Response {
    ui.put(
        rect,
        egui::Button::new(
            RichText::new(language.text("Отмена", "Cancel"))
                .size(14.0)
                .color(theme::muted()),
        )
        .frame(false)
        .corner_radius(10),
    )
}

fn step(ui: &mut egui::Ui, rect: Rect, number: &str, title: &str, detail: &str) {
    let badge = Rect::from_min_size(rect.min, Vec2::splat(26.0));
    ui.painter().rect_filled(badge, 9, theme::selected());
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        number,
        FontId::new(13.0, theme::semibold()),
        theme::accent_light(SOFT_BLUE),
    );
    let left = rect.left() + 38.0;
    label(
        ui,
        Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(rect.right(), rect.top() + 24.0),
        ),
        title,
        15.0,
        TEXT,
        false,
    );
    label(
        ui,
        Rect::from_min_max(egui::pos2(left, rect.top() + 29.0), rect.max),
        detail,
        13.0,
        theme::muted(),
        false,
    );
}

fn phone(ui: &egui::Ui, center: egui::Pos2) {
    let body = Rect::from_center_size(center, egui::vec2(26.0, 42.0));
    ui.painter()
        .rect_filled(body.expand2(egui::vec2(17.0, 9.0)), 18, theme::selected());
    ui.painter().rect_stroke(
        body,
        6,
        Stroke::new(2.0, theme::accent_light(SOFT_BLUE)),
        egui::StrokeKind::Inside,
    );
    ui.painter().rect_filled(
        Rect::from_center_size(center + egui::vec2(0.0, 14.0), egui::vec2(8.0, 2.0)),
        1,
        theme::accent_light(SOFT_BLUE),
    );
}

pub fn show(ctx: &egui::Context, view: View<'_>) -> Output {
    let t = |ru, en| view.language.text(ru, en);
    let mut action = None;
    let mut confirmation = None;
    let mut actions = [Rect::NOTHING; 2];
    let width = (ctx.content_rect().width() - 80.0).clamp(360.0, 512.0);
    let height = (ctx.content_rect().height() - 96.0).clamp(360.0, 432.0);
    let modal = egui::Modal::new(egui::Id::new("vk-fast-qr-login"))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::surface())
                .stroke(Stroke::new(1.0, theme::tone(BORDER)))
                .corner_radius(20)
                .inner_margin(24),
        )
        .backdrop_color(Color32::from_black_alpha(180))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ui.spacing_mut().interact_size.y = 28.0;
            ui.style_mut().interaction.selectable_labels = false;
            let (canvas, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
            let header = Rect::from_min_size(canvas.min, egui::vec2(width, 32.0));
            ui.painter().text(
                header.center(),
                Align2::CENTER_CENTER,
                t("Вход через QR", "Sign in via QR"),
                FontId::new(24.0, theme::semibold()),
                TEXT,
            );
            let close = Rect::from_center_size(
                header.right_center() - egui::vec2(14.0, 0.0),
                Vec2::splat(28.0),
            );
            let response = ui.put(close, egui::Button::new("").frame(false).corner_radius(8));
            icons::paint(ui.painter(), close.shrink(7.0), Icon::Close, theme::muted());
            if response.on_hover_text(t("Закрыть", "Close")).clicked() {
                action = Some(Action::Cancel);
            }

            let finishing = view.stage == Stage::Finishing && view.error.is_none();
            let confirming = view.code_entry || finishing;
            let progress = Rect::from_center_size(
                egui::pos2(canvas.center().x, canvas.top() + 61.0),
                egui::vec2(312.0, 28.0),
            );
            for (index, title) in [
                t("1  Сканирование", "1  Scan code"),
                t("2  Подтверждение", "2  Confirm"),
            ]
            .into_iter()
            .enumerate()
            {
                let pill = Rect::from_min_size(
                    progress.min + egui::vec2(index as f32 * 164.0, 0.0),
                    egui::vec2(148.0, 28.0),
                );
                let active = confirming == (index == 1);
                ui.painter().rect_filled(
                    pill,
                    9,
                    if active {
                        theme::selected()
                    } else {
                        theme::surface()
                    },
                );
                ui.painter().text(
                    pill.center(),
                    Align2::CENTER_CENTER,
                    title,
                    FontId::proportional(12.0),
                    if active {
                        theme::accent_light(SOFT_BLUE)
                    } else {
                        theme::muted()
                    },
                );
            }
            let body = Rect::from_min_max(canvas.min + egui::vec2(0.0, 96.0), canvas.max);
            let code_mode = view.code_entry;
            let focus = ui.make_persistent_id("qr-code-step-visible");
            let was_code = ctx
                .data(|data| data.get_temp::<bool>(focus))
                .unwrap_or(false);
            ctx.data_mut(|data| data.insert_temp(focus, code_mode));
            if code_mode {
                let center = body.center().x;
                let top = body.top();
                phone(ui, egui::pos2(center, top + 23.0));
                label(
                    ui,
                    Rect::from_center_size(egui::pos2(center, top + 71.0), egui::vec2(width, 24.0)),
                    t("Введите код", "Enter your code"),
                    18.0,
                    TEXT,
                    true,
                );
                label(
                    ui,
                    Rect::from_center_size(
                        egui::pos2(center, top + 105.0),
                        egui::vec2(width - 36.0, 36.0),
                    ),
                    t(
                        "Подтвердите вход на телефоне. Если VK показал код, введите его здесь.",
                        "Approve sign-in on your phone. If VK shows a code, enter it here.",
                    ),
                    13.0,
                    theme::muted(),
                    true,
                );
                let controls = 216.0;
                let field_rect = Rect::from_center_size(
                    egui::pos2(center, top + 155.0),
                    egui::vec2(controls, 48.0),
                );
                let field = ui.put(
                    field_rect,
                    egui::TextEdit::singleline(view.code)
                        .id_salt("qr-verification-code")
                        .password(false)
                        .font(FontId::monospace(26.0))
                        .horizontal_align(Align::Center)
                        .vertical_align(Align::Center)
                        .hint_text("000000")
                        .char_limit(16)
                        .frame(
                            egui::Frame::new()
                                .fill(theme::background())
                                .inner_margin(8)
                                .corner_radius(10)
                                .stroke(Stroke::new(
                                    1.0,
                                    if view.error.is_some() {
                                        Color32::from_rgb(170, 91, 92)
                                    } else {
                                        theme::tone(BORDER)
                                    },
                                )),
                        ),
                );
                if field.changed() {
                    view.code.retain(|ch| ch.is_ascii_digit());
                    view.code.truncate(6);
                }
                if !was_code {
                    field.request_focus();
                }
                if let Some(mut state) = egui::TextEdit::load_state(ctx, field.id) {
                    state.clear_undoer();
                    egui::TextEdit::store_state(ctx, field.id, state);
                }
                let valid = view.code.len() == 6 && view.code.bytes().all(|ch| ch.is_ascii_digit());
                let submit_rect = Rect::from_center_size(
                    egui::pos2(center, top + 227.0),
                    egui::vec2(controls, 40.0),
                );
                let submit = button(ui, submit_rect, t("Подтвердить", "Confirm"), true, valid);
                if submit.clicked()
                    || (valid
                        && (field.has_focus() || field.lost_focus())
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                {
                    action = Some(Action::Submit);
                }
                confirmation = Some([field.rect, submit.rect]);
                if let Some(error) = view.error {
                    let rect = Rect::from_center_size(
                        egui::pos2(center, top + 193.0),
                        egui::vec2(width, 20.0),
                    );
                    ui.put(
                        rect,
                        egui::Label::new(RichText::new(error).size(12.0).color(theme::error()))
                            .halign(Align::Center)
                            .truncate(),
                    )
                    .on_hover_text(error);
                }
                let refresh_rect = Rect::from_center_size(
                    egui::pos2(center, body.bottom() - 53.0),
                    egui::vec2(controls, 34.0),
                );
                let cancel_rect = Rect::from_center_size(
                    egui::pos2(center, body.bottom() - 14.0),
                    egui::vec2(controls, 28.0),
                );
                let refresh = button(
                    ui,
                    refresh_rect,
                    t("Обновить QR", "Refresh QR"),
                    false,
                    true,
                );
                let cancel = cancel_button(ui, cancel_rect, view.language);
                actions = [refresh.rect, cancel.rect];
                if refresh.clicked() {
                    action = Some(Action::Refresh);
                }
                if cancel.clicked() {
                    action = Some(Action::Cancel);
                }
            } else if finishing {
                let center = body.center();
                ui.painter()
                    .circle_filled(center - egui::vec2(0.0, 55.0), 34.0, theme::selected());
                let check = center - egui::vec2(0.0, 55.0);
                let points = [
                    check + egui::vec2(-12.0, 0.0),
                    check + egui::vec2(-3.0, 8.0),
                    check + egui::vec2(13.0, -9.0),
                ];
                for edge in points.windows(2) {
                    ui.painter().line_segment(
                        [edge[0], edge[1]],
                        Stroke::new(3.0, theme::accent_light(SOFT_BLUE)),
                    );
                }
                for point in points {
                    ui.painter()
                        .circle_filled(point, 1.5, theme::accent_light(SOFT_BLUE));
                }
                label(
                    ui,
                    Rect::from_center_size(center, egui::vec2(width, 28.0)),
                    t("Вход подтверждён", "Sign-in approved"),
                    20.0,
                    TEXT,
                    true,
                );
                label(
                    ui,
                    Rect::from_center_size(center + egui::vec2(0.0, 34.0), egui::vec2(width, 24.0)),
                    t("Подключаем вашу музыку…", "Connecting your music…"),
                    14.0,
                    theme::muted(),
                    true,
                );
                ui.put(
                    Rect::from_center_size(center + egui::vec2(0.0, 72.0), Vec2::splat(18.0)),
                    egui::Spinner::new()
                        .size(18.0)
                        .color(theme::accent_light(SOFT_BLUE)),
                );
                let cancel = cancel_button(
                    ui,
                    Rect::from_center_size(
                        egui::pos2(center.x, body.bottom() - 14.0),
                        egui::vec2(216.0, 28.0),
                    ),
                    view.language,
                );
                actions = [cancel.rect, cancel.rect];
                if cancel.clicked() {
                    action = Some(Action::Cancel);
                }
            } else {
                let gap = 28.0;
                let side = ((width - gap) * 0.44).clamp(148.0, 216.0);
                let cluster_height = side + 40.0;
                let top = body.center().y - cluster_height * 0.5;
                let qr_rect =
                    Rect::from_min_size(egui::pos2(canvas.left(), top), Vec2::splat(side));
                let expired = view.remaining == Some(0);
                let usable = !expired && view.error.is_none();
                ui.painter().rect_filled(
                    qr_rect,
                    16,
                    if usable && view.texture.is_some() {
                        Color32::WHITE
                    } else {
                        theme::background()
                    },
                );
                if let Some(texture) = view.texture.filter(|_| usable) {
                    egui::Image::from_texture(texture)
                        .corner_radius(12)
                        .paint_at(ui, qr_rect.shrink(4.0));
                } else if view.pending && view.error.is_none() && !expired {
                    ui.put(
                        Rect::from_center_size(qr_rect.center(), Vec2::splat(28.0)),
                        egui::Spinner::new()
                            .size(28.0)
                            .color(theme::accent_light(SOFT_BLUE)),
                    );
                } else {
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(qr_rect.center(), Vec2::splat(42.0)),
                        Icon::Refresh,
                        theme::muted(),
                    );
                }
                let timer = Rect::from_center_size(
                    egui::pos2(qr_rect.center().x, qr_rect.bottom() + 25.0),
                    egui::vec2(side, 26.0),
                );
                let status = if expired {
                    t("QR-код устарел", "QR code expired").into()
                } else if view.error.is_some() {
                    t("QR недоступен", "QR unavailable").into()
                } else if let Some(seconds) = view.remaining {
                    format!(
                        "{} {}:{:02}",
                        t("Действует", "Expires in"),
                        seconds / 60,
                        seconds % 60
                    )
                } else {
                    t("Создаём QR-код…", "Creating QR code…").into()
                };
                label(ui, timer, &status, 12.0, theme::muted(), true);
                let right = Rect::from_min_max(
                    egui::pos2(qr_rect.right() + gap, top),
                    egui::pos2(canvas.right(), top + cluster_height),
                );
                if usable {
                    step(
                        ui,
                        Rect::from_min_size(right.min, egui::vec2(right.width(), 78.0)),
                        "1",
                        t("Отсканируйте QR", "Scan the QR code"),
                        t(
                            "Камерой телефона или сканером в приложении VK.",
                            "Use your phone camera or the scanner in the VK app.",
                        ),
                    );
                    step(
                        ui,
                        Rect::from_min_size(
                            right.min + egui::vec2(0.0, 92.0),
                            egui::vec2(right.width(), 70.0),
                        ),
                        "2",
                        t("Разрешите вход", "Approve sign-in"),
                        t(
                            "Подтвердите запрос на телефоне.",
                            "Confirm the request on your phone.",
                        ),
                    );
                } else {
                    label(
                        ui,
                        Rect::from_min_size(right.min, egui::vec2(right.width(), 30.0)),
                        if expired {
                            t("Нужен новый QR", "Time for a new QR")
                        } else {
                            t("Не удалось войти", "Could not sign in")
                        },
                        18.0,
                        TEXT,
                        false,
                    );
                    label(
                        ui,
                        Rect::from_min_size(
                            right.min + egui::vec2(0.0, 44.0),
                            egui::vec2(right.width(), 100.0),
                        ),
                        view.error.unwrap_or(t(
                            "Обновите код и отсканируйте его ещё раз.",
                            "Refresh the code and scan it again.",
                        )),
                        14.0,
                        theme::muted(),
                        false,
                    );
                }
                let refresh_rect = Rect::from_min_size(
                    egui::pos2(right.left(), right.bottom() - 80.0),
                    egui::vec2(right.width(), 40.0),
                );
                let cancel_rect = Rect::from_min_size(
                    egui::pos2(right.left(), right.bottom() - 32.0),
                    egui::vec2(right.width(), 32.0),
                );
                let refresh = button(
                    ui,
                    refresh_rect,
                    t("Обновить QR", "Refresh QR"),
                    !usable,
                    !(view.stage == Stage::Creating && view.pending && view.error.is_none()),
                );
                let cancel = cancel_button(ui, cancel_rect, view.language);
                actions = [refresh.rect, cancel.rect];
                if refresh.clicked() {
                    action = Some(Action::Refresh);
                }
                if cancel.clicked() {
                    action = Some(Action::Cancel);
                }
            }
            canvas
        });
    if modal.is_top_modal
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        action = Some(Action::Cancel);
    }
    Output {
        action,
        layout: (modal.response.rect, modal.inner, confirmation, actions),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_supports_paste_enter_and_cancel_without_submitting_on_other_steps() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut code = String::new();
        let mut time = 0.0;
        let mut frame = |entry: bool, events: Vec<egui::Event>| {
            time += 0.2;
            let mut result = None;
            let mut output = ctx.run_ui(
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
                    result = Some(show(
                        &ctx,
                        View {
                            language: Language::English,
                            texture: None,
                            stage: if entry {
                                Stage::Confirming
                            } else {
                                Stage::Waiting
                            },
                            pending: true,
                            remaining: Some(120),
                            code_entry: entry,
                            code: &mut code,
                            error: None,
                        },
                    ));
                },
            );
            output.textures_delta.clear();
            (result.unwrap(), code.clone())
        };
        for _ in 0..3 {
            assert_eq!(frame(true, Vec::new()).0.action, None);
        }
        let (pasted, value) = frame(true, vec![egui::Event::Paste("01 23 45".into())]);
        assert_eq!(value, "012345");
        assert_eq!(pasted.action, None);
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(
            frame(true, vec![enter.clone()]).0.action,
            Some(Action::Submit)
        );
        assert_eq!(frame(true, Vec::new()).0.action, None);
        let waiting = frame(false, vec![enter]).0;
        assert!(waiting.layout.2.is_none());
        assert_eq!(waiting.action, None);
        let click = |position, pressed| {
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let outside = egui::pos2(2.0, 2.0);
        frame(false, click(outside, true));
        assert_eq!(frame(false, click(outside, false)).0.action, None);
        let target = frame(false, Vec::new()).0.layout.3[1].center();
        assert_eq!(frame(false, click(target, true)).0.action, None);
        assert_eq!(
            frame(false, click(target, false)).0.action,
            Some(Action::Cancel)
        );
    }
}
