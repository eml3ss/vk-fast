use crate::{
    audio_upload::{self, Source},
    i18n::Language,
    theme,
};
use eframe::egui::{self, Align, Color32, RichText};

pub struct View<'a> {
    pub title: &'a mut String,
    pub artist: &'a mut String,
    pub source: Option<&'a Source>,
    pub language: Language,
    pub picking: bool,
    pub progress: Option<f32>,
    pub error: Option<&'a str>,
    pub uncertain: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Choose,
    Upload,
    Cancel,
    Refresh,
}

pub fn show(ctx: &egui::Context, view: View<'_>) -> Option<Action> {
    let language = view.language;
    let width = (ctx.content_rect().width() - 80.0).clamp(240.0, 440.0);
    let busy = view.picking || view.progress.is_some();
    let saving = view.progress.is_some_and(|value| value >= 1.0);
    let valid = view.source.is_some()
        && audio_upload::valid_text(view.title)
        && audio_upload::valid_text(view.artist);
    let mut action = None;
    let modal = egui::Modal::new(egui::Id::new("audio-upload-dialog"))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::surface())
                .inner_margin(20)
                .corner_radius(14),
        )
        .backdrop_color(Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.style_mut().interaction.selectable_labels = false;
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(
                RichText::new(language.text("Загрузить свой трек", "Upload your track"))
                    .size(22.0)
                    .family(theme::semibold()),
            );
            ui.label(
                RichText::new(language.text(
                    "MP3 до 200 МБ · в вашу музыку VK",
                    "MP3 up to 200 MB · to your VK music",
                ))
                .small()
                .color(theme::muted()),
            );
            ui.add_space(4.0);
            if let Some(source) = view.source {
                ui.horizontal(|ui| {
                    let label = ui.add_sized(
                        egui::vec2((width - 110.0).max(120.0), 36.0),
                        egui::Label::new(&source.name).truncate().selectable(false),
                    );
                    label.on_hover_text(&source.name);
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(language.text("Выбрать…", "Choose…")),
                        )
                        .clicked()
                    {
                        action = Some(Action::Choose);
                    }
                });
            } else if ui
                .add_enabled_ui(!busy, |ui| {
                    ui.add_sized(
                        egui::vec2(width, 36.0),
                        egui::Button::new(language.text("Выбрать MP3", "Choose MP3"))
                            .corner_radius(8),
                    )
                })
                .inner
                .clicked()
            {
                action = Some(Action::Choose);
            }
            for (id, label, value) in [
                (
                    "upload-title",
                    language.text("Название трека", "Track title"),
                    view.title,
                ),
                (
                    "upload-artist",
                    language.text("Исполнитель", "Artist"),
                    view.artist,
                ),
            ] {
                ui.label(RichText::new(label).small().color(theme::muted()));
                ui.add_enabled_ui(!busy, |ui| {
                    ui.add_sized(
                        egui::vec2(width, 38.0),
                        egui::TextEdit::singleline(value)
                            .id_salt(id)
                            .char_limit(audio_upload::MAX_TEXT)
                            .margin(egui::vec2(10.0, 8.0))
                            .vertical_align(Align::Center),
                    )
                });
            }
            if let Some(error) = view.error {
                ui.add(
                    egui::Label::new(RichText::new(error).color(Color32::from_rgb(255, 158, 153)))
                        .wrap()
                        .selectable(false),
                );
            }
            if let Some(progress) = view.progress {
                ui.label(language.text(
                    if saving {
                        "Сохраняем трек в VK…"
                    } else {
                        "Загружаем в VK…"
                    },
                    if saving {
                        "Saving track to VK…"
                    } else {
                        "Uploading to VK…"
                    },
                ));
                ui.add(
                    egui::ProgressBar::new(progress)
                        .desired_width(width)
                        .show_percentage(),
                );
            } else if view.picking {
                ui.spinner();
            }
            ui.add_space(8.0);
            if ui
                .add_enabled_ui(!busy && (valid || view.uncertain), |ui| {
                    ui.add_sized(
                        egui::vec2(width, 38.0),
                        egui::Button::new(if view.uncertain {
                            language.text("Обновить библиотеку", "Refresh library")
                        } else {
                            language.text("Загрузить в VK", "Upload to VK")
                        })
                        .fill(theme::primary())
                        .corner_radius(8),
                    )
                })
                .inner
                .clicked()
            {
                action = Some(if view.uncertain {
                    Action::Refresh
                } else {
                    Action::Upload
                });
            }
            if ui
                .add_enabled_ui(!saving, |ui| {
                    ui.add_sized(
                        egui::vec2(width, 34.0),
                        egui::Button::new(language.text("Отмена", "Cancel")).corner_radius(8),
                    )
                })
                .inner
                .clicked()
            {
                action = Some(Action::Cancel);
            }
        });
    if modal.should_close() && !saving {
        action = Some(Action::Cancel);
    }
    action
}
