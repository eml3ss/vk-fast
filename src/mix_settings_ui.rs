//! The "Configure VK Mix" dialog. It shows VK's own categories and options;
//! the application loads them and applies the chosen ones to the mix.
use eframe::egui::{self, Color32, FontId, RichText, Sense, Stroke};

use crate::{
    i18n::Language,
    icons::{self, Icon},
    theme,
    web_audio::MixSettings,
};

pub struct View<'a> {
    pub language: Language,
    pub settings: Option<&'a MixSettings>,
    pub loading: bool,
    pub error: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    None,
    Close,
    Apply,
}

const OPTION_HEIGHT: f32 = 36.0;

/// `selection` holds one (category, option) pair per category, in the
/// settings' category order.
pub fn show(ctx: &egui::Context, view: View<'_>, selection: &mut Vec<(String, String)>) -> Output {
    let t = |ru, en| view.language.text(ru, en);
    let width = (ctx.content_rect().width() - 80.0).clamp(300.0, 460.0);
    let mut output = Output::None;
    let modal = egui::Modal::new(egui::Id::new("vk-mix-settings"))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::surface())
                .inner_margin(24)
                .corner_radius(20)
                .stroke(Stroke::new(1.0, theme::tone(Color32::from_rgb(44, 53, 67)))),
        )
        .backdrop_color(Color32::from_black_alpha(180))
        .show(ctx, |ui| {
            ui.set_width(width);
            let title = view
                .settings
                .map(|settings| settings.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or(t("Настроить VK Микс", "Configure VK Mix"));
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(title)
                            .size(20.0)
                            .family(theme::semibold())
                            .color(theme::tone(Color32::from_rgb(230, 235, 244))),
                    )
                    .truncate()
                    .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
                    if response.hovered() || response.has_focus() {
                        ui.painter().rect_filled(rect, 8.0, theme::selected());
                    }
                    icons::paint(ui.painter(), rect.shrink(7.0), Icon::Close, theme::muted());
                    let label = t("Закрыть", "Close");
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label)
                    });
                    if response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(label)
                        .clicked()
                    {
                        output = Output::Close;
                    }
                });
            });
            if let Some(subtitle) = view
                .settings
                .map(|settings| settings.subtitle.as_str())
                .filter(|subtitle| !subtitle.is_empty())
            {
                ui.label(RichText::new(subtitle).color(theme::muted()));
            }
            ui.add_space(8.0);
            match (view.settings, view.loading) {
                (_, true) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t("Загружаем настройки VK…", "Loading VK settings…"));
                    });
                }
                (Some(settings), false) if !settings.categories.is_empty() => {
                    for category in &settings.categories {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(category.title.to_uppercase())
                                .small()
                                .family(theme::semibold())
                                .color(theme::muted()),
                        );
                        ui.add_space(6.0);
                        let chosen = selection
                            .iter()
                            .find(|(id, _)| *id == category.id)
                            .map(|(_, option)| option.clone());
                        let mut spans = Vec::with_capacity(category.options.len());
                        let mut x = 0.0;
                        for option in &category.options {
                            let width = option_width(ui, &option.title);
                            spans.push((x, width));
                            x += width + 8.0;
                        }
                        let mut clicked = None;
                        crate::carousel::carousel(
                            ui,
                            &format!("vk-mix-settings-{}", category.id),
                            &spans,
                            OPTION_HEIGHT,
                            view.language,
                            |ui, index| {
                                let option = &category.options[index];
                                if option_chip(
                                    ui,
                                    &option.title,
                                    chosen.as_deref() == Some(option.id.as_str()),
                                )
                                .clicked()
                                {
                                    clicked = Some(option.id.clone());
                                }
                            },
                        );
                        if let Some(option) = clicked {
                            // One option per category; picking it again clears it.
                            let position = selection.iter().position(|(id, _)| *id == category.id);
                            match position {
                                Some(index) if selection[index].1 == option => {
                                    selection.remove(index);
                                }
                                Some(index) => selection[index].1 = option,
                                None => selection.push((category.id.clone(), option)),
                            }
                            let order = |id: &str| {
                                settings
                                    .categories
                                    .iter()
                                    .position(|category| category.id == id)
                            };
                            selection.sort_by_key(|(id, _)| order(id));
                        }
                    }
                }
                _ => {
                    ui.label(
                        RichText::new(view.error.unwrap_or(t(
                            "VK не предложил настроек для этого микса.",
                            "VK offered no settings for this mix.",
                        )))
                        .color(if view.error.is_some() {
                            theme::error()
                        } else {
                            theme::muted()
                        }),
                    );
                }
            }
            ui.add_space(20.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let ready =
                    !view.loading && view.settings.is_some_and(|s| !s.categories.is_empty());
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(t("Применить", "Apply"))
                            .fill(theme::primary())
                            .corner_radius(10)
                            .min_size(egui::vec2(120.0, 36.0)),
                    )
                    .clicked()
                {
                    output = Output::Apply;
                }
                if ui
                    .add_enabled(
                        ready && !selection.is_empty(),
                        egui::Button::new(t("Сбросить", "Reset"))
                            .frame(false)
                            .min_size(egui::vec2(90.0, 36.0)),
                    )
                    .clicked()
                {
                    selection.clear();
                }
            });
        });
    if modal.should_close() && output == Output::None {
        output = Output::Close;
    }
    output
}

fn option_width(ui: &egui::Ui, title: &str) -> f32 {
    let width = ui
        .painter()
        .layout_no_wrap(title.to_owned(), FontId::proportional(14.0), Color32::WHITE)
        .size()
        .x;
    (width + 28.0).max(84.0)
}

fn option_chip(ui: &mut egui::Ui, title: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(option_width(ui, title), OPTION_HEIGHT),
        Sense::click(),
    );
    let hovered = response.hovered() || response.has_focus();
    let fill = match (selected, hovered) {
        (true, false) => theme::accent(),
        (true, true) => theme::accent_tone(Color32::from_rgb(72, 132, 216)),
        (false, false) => theme::tone(Color32::from_rgb(35, 41, 50)),
        (false, true) => theme::tone(Color32::from_rgb(45, 53, 65)),
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        FontId::proportional(14.0),
        if selected {
            Color32::WHITE
        } else {
            ui.visuals().text_color()
        },
    );
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, selected, title)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
