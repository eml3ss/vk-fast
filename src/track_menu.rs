//! Track menu presentation. The caller owns playback, remote writes and the
//! clipboard; this module only emits actions after an explicit selection.
use std::collections::HashSet;

use eframe::egui::{self, Response, RichText, Sense};

use crate::{
    i18n::Language,
    icons::{self, Icon},
    model::{AlbumRef, ArtistRef, Track},
    theme,
};

const WIDTH: f32 = 264.0;
const ROW_HEIGHT: f32 = 30.0;

pub struct View<'a> {
    pub language: Language,
    pub track: &'a Track,
    pub liked: bool,
    pub like_enabled: bool,
    pub like_pending: bool,
    /// Editable playlists only, already verified and filtered by the caller.
    pub playlists: &'a [AlbumRef],
    pub playlists_loading: bool,
    pub playlist_busy: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    AddToQueue,
    ToggleLike,
    AddToPlaylist(AlbumRef),
    TrackMix,
    Artist(ArtistRef),
    Album,
    CopyLink,
    RefreshPlaylists,
}

pub fn show(ui: &mut egui::Ui, view: View<'_>) -> Option<Action> {
    let language = view.language;
    let mut action = None;
    ui.set_width(WIDTH);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.style_mut().interaction.selectable_labels = false;

    if row(
        ui,
        Icon::QueueAdd,
        language.text("Добавить в очередь", "Add to queue"),
        view.connected && view.track.unavailable != Some(true),
        false,
        false,
    )
    .clicked()
    {
        action = Some(Action::AddToQueue);
    }
    let label = if view.liked {
        language.text("Убрать из моей музыки", "Remove from My music")
    } else {
        language.text("Добавить в мою музыку", "Add to My music")
    };
    if row(
        ui,
        if view.liked {
            Icon::HeartFilled
        } else {
            Icon::Heart
        },
        label,
        view.like_enabled && !view.like_pending,
        false,
        view.like_pending,
    )
    .clicked()
    {
        action = Some(Action::ToggleLike);
    }
    let playlist_enabled = view.connected && !view.playlist_busy;
    ui.add_enabled_ui(playlist_enabled, |ui| {
        let response = row(
            ui,
            Icon::PlaylistAdd,
            language.text("Добавить в плейлист", "Add to playlist"),
            playlist_enabled,
            true,
            false,
        );
        if let Some(result) = egui::containers::menu::SubMenu::new().show(ui, &response, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut action = None;
            if view.playlists_loading {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(
                        RichText::new(language.text("Загружаем плейлисты…", "Loading playlists…"))
                            .color(theme::MUTED),
                    );
                });
            } else if view.playlists.is_empty() {
                ui.add(theme::wrapped_body(
                    ui,
                    language.text(
                        "Нет доступных для изменения плейлистов",
                        "No editable playlists",
                    ),
                    ui.available_width(),
                    egui::Align::Min,
                    theme::MUTED,
                ));
            } else {
                theme::scroll_area(
                    ui,
                    egui::ScrollArea::vertical()
                        .id_salt("context-playlists")
                        .max_height(300.0)
                        .auto_shrink([false, true]),
                    |ui| {
                        for playlist in view.playlists {
                            ui.push_id((playlist.owner_id, playlist.id), |ui| {
                                let title = if playlist.title.trim().is_empty() {
                                    language.text("Плейлист", "Playlist")
                                } else {
                                    &playlist.title
                                };
                                if row(ui, Icon::Library, title, true, false, false)
                                    .on_hover_text(title)
                                    .clicked()
                                {
                                    action = Some(Action::AddToPlaylist(playlist.clone()));
                                }
                            });
                        }
                    },
                );
            }
            separator(ui);
            if row(
                ui,
                Icon::Refresh,
                language.text("Обновить плейлисты", "Refresh playlists"),
                !view.playlists_loading,
                false,
                false,
            )
            .clicked()
            {
                action = Some(Action::RefreshPlaylists);
            }
            if action.is_some() {
                ui.close();
            }
            action
        }) {
            action = result.inner.or(action.take());
        }
    });

    separator(ui);
    if row(
        ui,
        Icon::Radio,
        language.text("Микс по треку", "Go to track mix"),
        view.connected,
        false,
        false,
    )
    .clicked()
    {
        action = Some(Action::TrackMix);
    }

    let artists = artists(view.track);
    let label = language.text("К исполнителю", "Go to artist");
    if artists.len() > 1 {
        let response = row(ui, Icon::Account, label, true, true, false);
        if let Some(result) = egui::containers::menu::SubMenu::new().show(ui, &response, |ui| {
            ui.set_width(WIDTH);
            let mut action = None;
            for artist in &artists {
                ui.push_id((&artist.id, &artist.name), |ui| {
                    if row(ui, Icon::Account, &artist.name, true, false, false)
                        .on_hover_text(&artist.name)
                        .clicked()
                    {
                        action = Some(Action::Artist(artist.clone()));
                    }
                });
            }
            if action.is_some() {
                ui.close();
            }
            action
        }) {
            action = result.inner.or(action.take());
        }
    } else if row(ui, Icon::Account, label, !artists.is_empty(), false, false).clicked()
        && let Some(artist) = artists.first()
    {
        action = Some(Action::Artist(artist.clone()));
    }
    if row(
        ui,
        Icon::Album,
        language.text("К альбому", "Go to album"),
        true,
        false,
        false,
    )
    .clicked()
    {
        action = Some(Action::Album);
    }
    separator(ui);
    if row(
        ui,
        Icon::CopyLink,
        language.text("Копировать ссылку", "Copy link"),
        true,
        false,
        false,
    )
    .clicked()
    {
        action = Some(Action::CopyLink);
    }
    if action.is_some() {
        ui.close();
    }
    action
}

fn artists(track: &Track) -> Vec<ArtistRef> {
    let mut seen = HashSet::new();
    let artists: Vec<_> = track
        .artists
        .iter()
        .filter(|artist| !artist.name.trim().is_empty())
        .filter(|artist| {
            seen.insert(if artist.id.is_empty() {
                format!("name:{}", artist.name.to_lowercase())
            } else {
                format!("id:{}", artist.id)
            })
        })
        .cloned()
        .collect();
    if artists.is_empty() && !track.artist.trim().is_empty() {
        vec![ArtistRef {
            id: String::new(),
            name: track.artist.clone(),
        }]
    } else {
        artists
    }
}

fn separator(ui: &mut egui::Ui) {
    ui.add_space(5.0);
    ui.separator();
    ui.add_space(5.0);
}

fn row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    submenu: bool,
    pending: bool,
) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let width = ui.available_width().max(1.0);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click());
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 6.0, theme::SELECTED);
        }
        let color = if ui.is_enabled() {
            ui.visuals().text_color()
        } else {
            theme::MUTED
        };
        let icon_rect = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(15.0, 0.0),
            egui::vec2(17.0, 17.0),
        );
        if pending {
            ui.put(icon_rect, egui::Spinner::new().size(14.0));
        } else {
            let glyph_color = if matches!(icon, Icon::HeartFilled) {
                theme::ACCENT
            } else if response.hovered() || response.has_focus() {
                color
            } else {
                theme::MUTED
            };
            icons::paint(ui.painter(), icon_rect, icon, glyph_color);
        }
        let mut job = egui::text::LayoutJob::simple_singleline(
            label.into(),
            egui::TextStyle::Body.resolve(ui.style()),
            color,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(
            (rect.width() - if submenu { 56.0 } else { 40.0 }).max(0.0),
        );
        let text = ui.painter().layout_job(job);
        ui.painter().galley(
            egui::pos2(rect.left() + 34.0, rect.center().y - text.size().y * 0.5),
            text,
            color,
        );
        if submenu {
            let chevron =
                ui.painter()
                    .layout_no_wrap("›".into(), egui::FontId::proportional(19.0), color);
            ui.painter().galley(
                egui::pos2(
                    rect.right() - 16.0,
                    rect.center().y - chevron.size().y * 0.5,
                ),
                chevron,
                color,
            );
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: 8,
            owner_id: -9,
            artist: "Fallback Artist".into(),
            title: "Song".into(),
            subtitle: None,
            duration: 180,
            url: None,
            cover_url: None,
            artists: vec![ArtistRef {
                id: "canonical".into(),
                name: "Hidden Artist Name".into(),
            }],
            album: Some(AlbumRef {
                id: 7,
                owner_id: -9,
                title: "Hidden Album Name".into(),
                access_key: None,
                cover_url: None,
            }),
            library_liked: None,
            release_key: None,
            library_source_key: None,
            unavailable: None,
        }
    }

    fn playlist() -> AlbumRef {
        AlbumRef {
            id: 14,
            owner_id: 42,
            title: "Writable playlist".into(),
            access_key: Some("synthetic-key".into()),
            cover_url: None,
        }
    }

    struct Harness {
        context: egui::Context,
        language: Language,
        track: Track,
        playlists: Vec<AlbumRef>,
        tick: u32,
        action: Option<Action>,
        liked: bool,
        pending: bool,
    }

    impl Harness {
        fn new(language: Language) -> Self {
            let context = egui::Context::default();
            theme::install(&context);
            Self {
                context,
                language,
                track: track(),
                playlists: vec![playlist()],
                tick: 0,
                action: None,
                liked: true,
                pending: false,
            }
        }
        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.tick += 1;
            let context = self.context.clone();
            let mut output = context.run_ui(
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
                |ui| {
                    egui::Popup::new(
                        egui::Id::new("track-menu-test"),
                        ui.ctx().clone(),
                        egui::pos2(12.0, 12.0),
                        ui.layer_id(),
                    )
                    .kind(egui::PopupKind::Menu)
                    .show(|ui| {
                        self.action = show(
                            ui,
                            View {
                                language: self.language,
                                track: &self.track,
                                liked: self.liked,
                                like_enabled: true,
                                like_pending: self.pending,
                                playlists: &self.playlists,
                                playlists_loading: false,
                                playlist_busy: false,
                                connected: true,
                            },
                        );
                    });
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
        fn click(&mut self, position: egui::Pos2) {
            self.frame(pointer(position, true));
            self.frame(pointer(position, false));
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
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        })
    }

    #[test]
    fn main_menu_has_every_generic_item_two_separators_and_adds_to_queue() {
        for language in [Language::Russian, Language::English] {
            let mut h = Harness::new(language);
            let output = h.warm();
            for caption in [
                language.text("Добавить в очередь", "Add to queue"),
                language.text("Убрать из моей музыки", "Remove from My music"),
                language.text("Добавить в плейлист", "Add to playlist"),
                language.text("Микс по треку", "Go to track mix"),
                language.text("К исполнителю", "Go to artist"),
                language.text("К альбому", "Go to album"),
                language.text("Копировать ссылку", "Copy link"),
            ] {
                assert!(text_rect(&output, caption).is_some(), "Missing {caption}");
            }
            assert!(text_rect(&output, "Hidden Artist Name").is_none());
            assert!(text_rect(&output, "Hidden Album Name").is_none());
            assert_eq!(output.shapes.iter().filter(|shape|matches!(&shape.shape,egui::Shape::LineSegment {points,..} if points[0].distance(points[1])>200.0)).count(),2);
            h.click(
                text_rect(&output, language.text("Добавить в очередь", "Add to queue"))
                    .unwrap()
                    .center(),
            );
            assert_eq!(h.action, Some(Action::AddToQueue));
        }
    }

    #[test]
    fn direct_actions_preserve_the_single_artist_and_likes_wait_without_falling_through() {
        for (caption, expected) in [
            ("Remove from My music", Action::ToggleLike),
            ("Go to track mix", Action::TrackMix),
            ("Go to artist", Action::Artist(track().artists[0].clone())),
            ("Go to album", Action::Album),
            ("Copy link", Action::CopyLink),
        ] {
            let mut h = Harness::new(Language::English);
            let output = h.warm();
            h.click(text_rect(&output, caption).unwrap().center());
            assert_eq!(h.action, Some(expected));
        }
        let mut h = Harness::new(Language::English);
        h.liked = false;
        let output = h.warm();
        assert!(text_rect(&output, "Add to My music").is_some());
        h.pending = true;
        let output = h.frame(Vec::new());
        h.click(text_rect(&output, "Add to My music").unwrap().center());
        assert!(h.action.is_none());
    }

    #[test]
    fn submenu_selects_exact_editable_playlist_and_specific_canonical_artist() {
        let mut h = Harness::new(Language::English);
        let output = h.warm();
        let parent = text_rect(&output, "Add to playlist").unwrap().center();
        h.frame(vec![egui::Event::PointerMoved(parent)]);
        let mut output = h.frame(vec![egui::Event::PointerMoved(parent)]);
        for _ in 0..2 {
            output = h.frame(vec![egui::Event::PointerMoved(parent)]);
        }
        let choice = text_rect(&output, "Writable playlist")
            .expect("Playlist submenu")
            .center();
        h.click(choice);
        assert_eq!(h.action, Some(Action::AddToPlaylist(playlist())));

        let mut h = Harness::new(Language::English);
        h.track.artists.push(ArtistRef {
            id: "guest-id".into(),
            name: "Specific Guest".into(),
        });
        let output = h.warm();
        let parent = text_rect(&output, "Go to artist").unwrap().center();
        h.frame(vec![egui::Event::PointerMoved(parent)]);
        let mut output = h.frame(vec![egui::Event::PointerMoved(parent)]);
        for _ in 0..2 {
            output = h.frame(vec![egui::Event::PointerMoved(parent)]);
        }
        let guest = text_rect(&output, "Specific Guest")
            .expect("Artist submenu")
            .center();
        h.click(guest);
        assert_eq!(h.action, Some(Action::Artist(h.track.artists[1].clone())));
    }

    #[test]
    fn missing_artist_metadata_uses_the_whole_fallback_name_without_guessing_comma_splits() {
        let mut track = track();
        track.artists.clear();
        track.artist = "A, B & C".into();
        assert_eq!(
            artists(&track),
            vec![ArtistRef {
                id: String::new(),
                name: "A, B & C".into()
            }]
        );
        track.artist.clear();
        assert!(artists(&track).is_empty());
    }
}
