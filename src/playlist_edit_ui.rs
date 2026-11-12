//! Playlist edit and removal dialogs. The caller owns the draft, image picker,
//! permissions and requests; rendering never reads files or writes to VK.
use eframe::egui::{self, Align, Color32, Layout, RichText, TextureHandle};

use crate::{cover_art::CoverCache, i18n::Language, theme};

pub struct View<'a> {
    pub language: Language,
    pub name: &'a mut String,
    pub description: &'a mut String,
    pub cover_url: Option<&'a str>,
    pub cover_preview: Option<&'a TextureHandle>,
    /// Native discovery control; this is not a private/public playlist switch.
    pub discoverable: Option<&'a mut bool>,
    pub can_change_cover: bool,
    pub can_edit: bool,
    pub loading: bool,
    pub busy: bool,
    pub picking_cover: bool,
    pub error: Option<&'a str>,
    pub focus_name: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Cancel,
    Save,
    ChangeCover,
    RemoveCover,
}

pub struct DeleteView<'a> {
    pub language: Language,
    pub title: &'a str,
    pub remove_saved: bool,
    pub removing: bool,
    pub loading: bool,
    pub can_delete: bool,
    pub error: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteAction {
    Cancel,
    Confirm,
}

pub fn show(ctx: &egui::Context, covers: &mut CoverCache, mut view: View<'_>) -> Option<Action> {
    let language = view.language;
    let width = (ctx.content_rect().width() - 80.0).clamp(216.0, 400.0);
    let enabled = view.can_edit && !view.loading && !view.busy;
    let mut action = None;
    let modal = egui::Modal::new(egui::Id::new("edit-playlist-dialog"))
        .frame(frame(ctx))
        .backdrop_color(Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(
                RichText::new(language.text("Редактирование плейлиста", "Edit details"))
                    .size(22.0)
                    .family(theme::semibold()),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("edit-playlist-fields")
                .max_height((ctx.content_rect().height() - 140.0).max(180.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        if let Some(texture) = view.cover_preview {
                            ui.add(
                                egui::Image::from_texture(texture)
                                    .fit_to_exact_size(egui::vec2(72.0, 72.0))
                                    .corner_radius(8),
                            );
                        } else {
                            covers.show(ui, view.cover_url, 72.0);
                        }
                        if view.can_change_cover {
                            ui.vertical(|ui| {
                                if ui
                                    .add_enabled(
                                        enabled,
                                        egui::Button::new(
                                            language.text("Изменить обложку", "Change cover"),
                                        )
                                        .corner_radius(8),
                                    )
                                    .clicked()
                                {
                                    action = Some(Action::ChangeCover);
                                }
                                if (view.cover_preview.is_some() || view.cover_url.is_some())
                                    && ui
                                        .add_enabled(
                                            enabled,
                                            egui::Button::new(
                                                language.text("Убрать обложку", "Remove cover"),
                                            )
                                            .corner_radius(8),
                                        )
                                        .clicked()
                                {
                                    action = Some(Action::RemoveCover);
                                }
                            });
                        }
                    });
                    if view.loading {
                        status(
                            ui,
                            language.text("Загружаем плейлист…", "Loading playlist…"),
                        );
                    }
                    ui.add_space(4.0);
                    label(ui, language.text("Название", "Name"));
                    let name = ui
                        .add_enabled_ui(enabled, |ui| {
                            ui.add_sized(
                                egui::vec2(width, 36.0),
                                egui::TextEdit::singleline(view.name)
                                    .id_salt("edit-playlist-name")
                                    .char_limit(100)
                                    .hint_text(language.text("Название плейлиста", "Playlist name"))
                                    .margin(egui::vec2(10.0, 8.0))
                                    .vertical_align(Align::Center),
                            )
                        })
                        .inner;
                    if view.focus_name && enabled {
                        name.request_focus();
                    }
                    if enabled
                        && valid_name(view.name)
                        && valid_description(view.description)
                        && (name.has_focus() || name.lost_focus())
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        action = Some(Action::Save);
                    }
                    label(ui, language.text("Описание", "Description"));
                    ui.add_enabled_ui(enabled, |ui| {
                        ui.add_sized(
                            egui::vec2(width, 82.0),
                            egui::TextEdit::multiline(view.description)
                                .id_salt("edit-playlist-description")
                                .char_limit(4096)
                                .hint_text(
                                    language
                                        .text("Необязательное описание", "Optional description"),
                                )
                                .margin(egui::vec2(10.0, 8.0)),
                        );
                    });
                    if let Some(discoverable) = view.discoverable.as_deref_mut() {
                        ui.add_space(4.0);
                        ui.add_enabled_ui(enabled, |ui| {
                            theme::switch(
                                ui,
                                discoverable,
                                language.text(
                                    "Показывать в поиске и рекомендациях",
                                    "Show in search and recommendations",
                                ),
                            );
                        });
                    }
                    if let Some(error) = view.error {
                        error_label(ui, error);
                    }
                });
            if view.picking_cover {
                status(ui, language.text("Выбираем обложку…", "Choosing cover…"));
            } else if view.busy {
                status(ui, language.text("Сохраняем…", "Saving…"));
            }
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled_ui(
                        enabled && valid_name(view.name) && valid_description(view.description),
                        |ui| {
                            ui.add_sized(
                                egui::vec2(104.0, 34.0),
                                egui::Button::new(theme::on_primary_text(
                                    language.text("Сохранить", "Save"),
                                ))
                                .fill(theme::primary())
                                .corner_radius(8),
                            )
                        },
                    )
                    .inner
                    .clicked()
                {
                    action = Some(Action::Save);
                }
                if ui
                    .add_enabled_ui(!view.busy, |ui| {
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
    if !view.busy && modal.should_close() {
        action = Some(Action::Cancel);
    }
    action
}

pub fn show_delete(ctx: &egui::Context, view: DeleteView<'_>) -> Option<DeleteAction> {
    let width = (ctx.content_rect().width() - 80.0).clamp(216.0, 400.0);
    let language = view.language;
    let mut action = None;
    let modal = egui::Modal::new(egui::Id::new("delete-playlist-dialog"))
        .frame(frame(ctx))
        .backdrop_color(Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 12.0;
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(RichText::new(if view.remove_saved { language.text("Убрать плейлист?", "Remove playlist?") } else { language.text("Удалить плейлист?", "Delete playlist?") }).size(22.0).family(theme::semibold()));
            ui.add(egui::Label::new(RichText::new(view.title).family(theme::semibold())).wrap().selectable(false));
            if view.remove_saved {
                ui.add(egui::Label::new(RichText::new(language.text("Плейлист исчезнет из вашей коллекции. Оригинал останется у автора.", "The playlist will be removed from your collection. Its creator keeps the original.")).color(theme::muted())).wrap().selectable(false));
            }
            if let Some(error) = view.error { error_label(ui, error); }
            if view.loading { status(ui, language.text("Загружаем плейлист…", "Loading playlist…")); }
            if view.removing { status(ui, language.text("Удаляем…", "Removing…")); }
            ui.add_space(4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled_ui(!view.removing && !view.loading && view.can_delete, |ui| {
                    ui.add_sized(egui::vec2(104.0, 34.0), egui::Button::new(if view.remove_saved { language.text("Убрать", "Remove") } else { language.text("Удалить", "Delete") }).fill(theme::primary()).corner_radius(8))
                }).inner.clicked() { action = Some(DeleteAction::Confirm); }
                if ui.add_enabled_ui(!view.removing, |ui| {
                    ui.add_sized(egui::vec2(96.0, 34.0), egui::Button::new(language.text("Отмена", "Cancel")).corner_radius(8))
                }).inner.clicked() { action = Some(DeleteAction::Cancel); }
            });
        });
    if !view.removing && modal.should_close() {
        action = Some(DeleteAction::Cancel);
    }
    action
}

fn frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.global_style())
        .fill(theme::surface())
        .inner_margin(20)
        .corner_radius(14)
        .stroke(egui::Stroke::new(
            1.0,
            theme::tone(Color32::from_rgb(44, 51, 62)),
        ))
}

fn label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::muted()));
}
fn error_label(ui: &mut egui::Ui, error: &str) {
    ui.add(
        egui::Label::new(RichText::new(error).small().color(theme::error()))
            .wrap()
            .selectable(false),
    );
}
fn status(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(14.0));
        label(ui, text);
    });
}
fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control)
}
fn valid_description(description: &str) -> bool {
    description.chars().count() <= 4096
        && !description
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        covers: CoverCache,
        language: Language,
        name: String,
        description: String,
        discoverable: bool,
        show_discovery: bool,
        preview: Option<TextureHandle>,
        loading: bool,
        busy: bool,
        picking_cover: bool,
        can_edit: bool,
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
                covers: CoverCache::new(),
                language,
                name: "My playlist".into(),
                description: "First paragraph\nSecond paragraph".into(),
                discoverable: true,
                show_discovery: true,
                preview: None,
                loading: false,
                busy: false,
                picking_cover: false,
                can_edit: true,
                error: None,
                tick: 0,
                action: None,
            }
        }
        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.tick += 1;
            let context = self.ctx.clone();
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 540.0),
                    )),
                    time: Some(f64::from(self.tick) * 0.1),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |_| {
                    self.action = show(
                        &context,
                        &mut self.covers,
                        View {
                            language: self.language,
                            name: &mut self.name,
                            description: &mut self.description,
                            cover_url: None,
                            cover_preview: self.preview.as_ref(),
                            discoverable: self.show_discovery.then_some(&mut self.discoverable),
                            can_change_cover: true,
                            can_edit: self.can_edit,
                            loading: self.loading,
                            busy: self.busy,
                            picking_cover: self.picking_cover,
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
        fn click(&mut self, point: egui::Pos2) {
            self.frame(pointer(point, true));
            self.frame(pointer(point, false));
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
    fn text_rect(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
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

    #[test]
    fn edit_form_fits_the_minimum_window_and_returns_cover_save_cancel_and_discovery_intents() {
        for language in [Language::Russian, Language::English] {
            let mut h = Harness::new(language);
            let output = h.warm();
            let save = text_rect(&output, language.text("Сохранить", "Save")).unwrap();
            let cancel = text_rect(&output, language.text("Отмена", "Cancel")).unwrap();
            assert!(save.bottom() < 540.0 && (save.top() - cancel.top()).abs() < 0.5);
            assert!(save.left() > cancel.right());
            h.click(
                text_rect(&output, language.text("Изменить обложку", "Change cover"))
                    .unwrap()
                    .center(),
            );
            assert_eq!(h.action, Some(Action::ChangeCover));
            let output = h.frame(Vec::new());
            h.click(
                text_rect(
                    &output,
                    language.text(
                        "Показывать в поиске и рекомендациях",
                        "Show in search and recommendations",
                    ),
                )
                .unwrap()
                .center(),
            );
            assert!(!h.discoverable);
            let output = h.frame(Vec::new());
            h.click(
                text_rect(&output, language.text("Сохранить", "Save"))
                    .unwrap()
                    .center(),
            );
            assert_eq!(h.action, Some(Action::Save));
            h.frame(key(egui::Key::Escape));
            assert_eq!(h.action, Some(Action::Cancel));
        }
    }

    #[test]
    fn edit_form_validates_name_preserves_the_draft_on_errors_and_blocks_writes_while_loading_or_busy()
     {
        assert!(!valid_name("\t\n"));
        assert!(!valid_name("bad\0name"));
        assert!(!valid_description("bad\0description"));
        assert!(!valid_description("bad\u{0001}description"));
        assert!(valid_description("paragraph\nnext\r\n\tindent"));
        assert!(valid_name(&"Ж".repeat(100)));
        assert!(!valid_name(&"Ж".repeat(101)));
        for (loading, busy, can_edit) in [
            (true, false, false),
            (false, true, true),
            (false, false, false),
        ] {
            let mut h = Harness::new(Language::English);
            h.loading = loading;
            h.busy = busy;
            h.can_edit = can_edit;
            h.error = Some("Native request failed".into());
            let output = h.warm();
            h.click(text_rect(&output, "Save").unwrap().center());
            assert!(h.action.is_none());
            assert_eq!(h.name, "My playlist");
            assert_eq!(h.description, "First paragraph\nSecond paragraph");
            assert!(text_rect(&h.frame(Vec::new()), "Native request failed").is_some());
            h.frame(key(egui::Key::Escape));
            assert_eq!(h.action, (!busy).then_some(Action::Cancel));
        }
        let mut h = Harness::new(Language::English);
        h.show_discovery = false;
        h.name = " ".into();
        let output = h.warm();
        assert!(text_rect(&output, "Show in search and recommendations").is_none());
        h.click(text_rect(&output, "Save").unwrap().center());
        assert!(h.action.is_none());
    }

    #[test]
    fn delete_waits_for_permissions_and_distinguishes_a_followed_playlist_from_its_original() {
        for (remove_saved, loading, can_delete, removing) in [
            (true, false, true, false),
            (false, false, true, false),
            (false, true, false, false),
            (true, false, false, false),
            (false, false, true, true),
        ] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut tick = 0;
            let mut action = None;
            let mut frame = |events| {
                tick += 1;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800.0, 540.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        events,
                        ..Default::default()
                    },
                    |_| {
                        action = show_delete(
                            &ctx,
                            DeleteView {
                                language: Language::English,
                                title: "Exact playlist",
                                remove_saved,
                                removing,
                                loading,
                                can_delete,
                                error: None,
                            },
                        );
                    },
                );
                output.textures_delta.clear();
                output
            };
            frame(Vec::new());
            frame(Vec::new());
            let output = frame(Vec::new());
            assert!(
                text_rect(
                    &output,
                    if remove_saved {
                        "Remove playlist?"
                    } else {
                        "Delete playlist?"
                    }
                )
                .is_some()
            );
            let point = text_rect(&output, if remove_saved { "Remove" } else { "Delete" })
                .unwrap()
                .center();
            frame(pointer(point, true));
            frame(pointer(point, false));
            assert_eq!(
                action,
                (can_delete && !loading && !removing).then_some(DeleteAction::Confirm)
            );
        }
    }

    #[test]
    fn removing_a_selected_cover_is_a_separate_intent_and_waits_for_save() {
        let mut h = Harness::new(Language::English);
        let output = h.warm();
        assert!(text_rect(&output, "Remove cover").is_none());
        h.preview = Some(h.ctx.load_texture(
            "synthetic-cover",
            egui::ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255]),
            egui::TextureOptions::LINEAR,
        ));
        let output = h.frame(Vec::new());
        h.click(text_rect(&output, "Remove cover").unwrap().center());
        assert_eq!(h.action, Some(Action::RemoveCover));
        assert_eq!(h.name, "My playlist");
        h.busy = true;
        h.picking_cover = true;
        let output = h.frame(Vec::new());
        assert!(text_rect(&output, "Choosing cover…").is_some());
        assert!(text_rect(&output, "Saving…").is_none());
        h.click(text_rect(&output, "Remove cover").unwrap().center());
        assert!(h.action.is_none());
    }
}
