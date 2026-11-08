//! Music publisher pages use the same artwork cache and theme as release pages.
use crate::{
    cover_art::CoverCache,
    cover_play::{self, Playback},
    detail_ui,
    i18n::Language,
    icons::{self, Icon},
    model::AlbumRef,
    search_ui, theme,
    web_audio::{OwnerPage, PlaylistCreator, UserPlaylist, WebAudioError},
};
use eframe::egui::{self, Align, FontId, Layout, RichText, Sense};

pub enum Action {
    OpenPlaylist(AlbumRef),
    PlayPlaylist(AlbumRef),
    More,
    Retry,
}

pub fn show(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    reference: &PlaylistCreator,
    page: Option<&OwnerPage>,
    loading: bool,
    playback: Playback,
) -> Option<Action> {
    let mut action = None;
    theme::scroll_area(
        ui,
        egui::ScrollArea::vertical()
            .id_salt(("playlist-owner", reference.owner_id))
            .auto_shrink([false, false]),
        |ui| {
            header(
                ui,
                covers,
                language,
                page.map_or(reference, |page| &page.creator),
                page,
            );
            if let Some(description) = page.and_then(|page| page.description.as_deref()) {
                detail_ui::playlist_description(ui, description);
            }
            ui.add_space(20.0);
            theme::inset_heading(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(language.text("Плейлисты", "Playlists"))
                            .size(20.0)
                            .family(theme::semibold()),
                    )
                    .selectable(false),
                )
            });
            ui.add_space(10.0);
            let Some(page) = page else {
                if loading {
                    ui.spinner();
                }
                return;
            };
            if page.unavailable {
                ui.label(
                    RichText::new(language.text(
                        "Плейлисты этого профиля скрыты или недоступны.",
                        "This profile's playlists are private or unavailable.",
                    ))
                    .color(theme::MUTED),
                );
                return;
            }
            action = grid(ui, covers, language, &page.playlists, playback);
            if let Some(error) = &page.playlists_error {
                ui.add_space(12.0);
                let message = if *error == WebAudioError::Rejected {
                    language
                        .text(
                            "VK не предоставил доступ к плейлистам.",
                            "VK did not grant access to these playlists.",
                        )
                        .into()
                } else {
                    error.localized(language)
                };
                ui.label(RichText::new(message).color(theme::MUTED));
                if ui
                    .add_enabled(
                        !loading,
                        egui::Button::new(language.text("Повторить", "Retry")),
                    )
                    .clicked()
                {
                    action = Some(Action::Retry);
                }
            } else if page.playlists.is_empty() && !loading {
                ui.label(
                    RichText::new(
                        language.text("Нет доступных плейлистов.", "No available playlists."),
                    )
                    .color(theme::MUTED),
                );
            }
            if page.next_offset.is_some() || loading {
                let (marker, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::hover());
                if loading {
                    ui.put(marker, egui::Spinner::new().size(20.0));
                } else if page.playlists_error.is_none()
                    && ui.is_rect_visible(marker)
                    && action.is_none()
                {
                    action = Some(Action::More);
                }
            }
            // The same space from the bottom edge as between page blocks.
            ui.add_space(22.0);
        },
    );
    action
}

fn header(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    creator: &PlaylistCreator,
    page: Option<&OwnerPage>,
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 182.0), Sense::hover());
    let image = page.and_then(|page| page.image_url.as_deref());
    let color = ui
        .is_rect_visible(rect)
        .then(|| covers.header_color(ui.ctx(), image, 128.0))
        .flatten();
    detail_ui::background(ui.painter(), rect, color);
    let art_size = if width >= 420.0 { 128.0 } else { 80.0 };
    let art = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 20.0 + art_size * 0.5, rect.center().y),
        egui::Vec2::splat(art_size),
    );
    let mut avatar = ui.new_child(egui::UiBuilder::new().id_salt("owner-avatar").max_rect(art));
    avatar.set_clip_rect(ui.clip_rect().intersect(art));
    if image.is_some() {
        covers.show_rounded(&mut avatar, image, art_size, (art_size * 0.5) as u8);
    } else {
        ui.painter()
            .circle_filled(art.center(), art_size * 0.5, theme::SURFACE);
        icons::paint(
            ui.painter(),
            art.shrink(art_size * 0.22),
            Icon::Account,
            theme::MUTED,
        );
    }
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(art.right() + 20.0, rect.top() + 28.0),
        rect.max - egui::vec2(20.0, 20.0),
    );
    let mut text = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("owner-heading")
            .max_rect(text_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    text.set_clip_rect(ui.clip_rect().intersect(text_rect));
    let group = creator.owner_id < 0;
    text.label(
        RichText::new(if group {
            language.text("СООБЩЕСТВО VK", "VK COMMUNITY")
        } else {
            language.text("ПОЛЬЗОВАТЕЛЬ VK", "VK USER")
        })
        .small()
        .color(theme::MUTED),
    );
    text.add_space(12.0);
    let name = if creator.name.trim().is_empty() {
        language.text("Владелец плейлиста", "Playlist owner")
    } else {
        &creator.name
    };
    text.add(
        egui::Label::new(
            RichText::new(name)
                .size(if width >= 520.0 { 30.0 } else { 24.0 })
                .family(theme::semibold()),
        )
        .truncate()
        .selectable(false),
    )
    .on_hover_text(name);
    text.add_space(6.0);
    if let Some(total) = page.and_then(|page| page.total) {
        text.label(
            RichText::new(format!(
                "{}: {total}",
                language.text("Плейлисты", "Playlists")
            ))
            .small()
            .color(theme::MUTED),
        );
    }
}

fn grid(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    playlists: &[UserPlaylist],
    playback: Playback,
) -> Option<Action> {
    if playlists.is_empty() {
        return None;
    }
    let columns = ((ui.available_width() + 14.0) / 174.0)
        .floor()
        .clamp(1.0, 16.0) as usize;
    let width = ((ui.available_width() - 14.0 * columns.saturating_sub(1) as f32) / columns as f32)
        .max(1.0);
    let art_size = (width - 24.0).clamp(1.0, 200.0);
    let height = art_size + 76.0;
    let rows = playlists.len().div_ceil(columns);
    let stride = height + 10.0;
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), rows as f32 * stride - 10.0),
        Sense::hover(),
    );
    let start = (((ui.clip_rect().top() - area.top()) / stride)
        .floor()
        .max(0.0) as usize)
        .min(rows);
    let end = (((ui.clip_rect().bottom() - area.top()) / stride)
        .ceil()
        .max(0.0) as usize)
        .min(rows);
    let mut selected = None;
    for row in start..end {
        for col in 0..columns {
            let Some(playlist) = playlists.get(row * columns + col) else {
                break;
            };
            let rect = egui::Rect::from_min_size(
                area.min + egui::vec2(col as f32 * (width + 14.0), row as f32 * stride),
                egui::vec2(width, height),
            );
            let response = ui.interact(
                rect,
                ui.id()
                    .with(("owner-playlist", playlist.album.owner_id, playlist.album.id)),
                Sense::click(),
            );
            ui.painter().rect_filled(
                rect,
                12,
                if response.hovered() || response.has_focus() {
                    theme::SELECTED
                } else {
                    theme::SURFACE
                },
            );
            let art = egui::Rect::from_min_size(
                rect.min + egui::vec2((width - art_size) * 0.5, 12.0),
                egui::Vec2::splat(art_size),
            );
            let mut image = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("owner-playlist-art", row, col))
                    .max_rect(art),
            );
            image.set_clip_rect(ui.clip_rect().intersect(art));
            covers.show(&mut image, playlist.album.cover_url.as_deref(), art_size);
            let key = (playlist.album.owner_id, playlist.album.id);
            let state = playback.state(key);
            // Like VK: only while the pointer is over the card, even when
            // this playlist is the one playing.
            let play = ui
                .rect_contains_pointer(rect)
                .then(|| {
                    cover_play::button(
                        ui,
                        cover_play::corner(art),
                        cover_play::radius(art),
                        ui.id().with(("owner-playlist-play", key)),
                        state,
                        language.text("Слушать", "Play"),
                    )
                })
                .is_some_and(|play| play.clicked());
            if play {
                selected = Some(Action::PlayPlaylist(playlist.album.clone()));
            }
            for (line, label, font, color) in [
                (
                    0.0,
                    playlist.album.title.clone(),
                    FontId::new(14.0, theme::semibold()),
                    ui.visuals().text_color(),
                ),
                (
                    22.0,
                    search_ui::playlist_caption(language, playlist),
                    FontId::proportional(12.0),
                    theme::MUTED,
                ),
            ] {
                let mut job = egui::text::LayoutJob::simple_singleline(label, font, color);
                job.wrap = egui::text::TextWrapping::truncate_at_width((width - 24.0).max(1.0));
                let galley = ui.painter().layout_job(job);
                ui.painter()
                    .with_clip_rect(ui.clip_rect().intersect(rect))
                    .galley(
                        egui::pos2(rect.left() + 12.0, art.bottom() + 10.0 + line),
                        galley,
                        color,
                    );
            }
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    &playlist.album.title,
                )
            });
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                && !play
            {
                selected = Some(Action::OpenPlaylist(playlist.album.clone()));
            }
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(count: usize) -> OwnerPage {
        OwnerPage {
            creator: PlaylistCreator {
                owner_id: -71,
                name: "Test community".into(),
            },
            image_url: None,
            description: None,
            unavailable: false,
            playlists: (0..count)
                .map(|index| UserPlaylist {
                    kind: crate::web_audio::CollectionKind::Playlist,
                    byline: String::new(),
                    original: None,
                    album: AlbumRef {
                        owner_id: -71,
                        id: index as i64 + 1,
                        title: format!("Collection {index}"),
                        access_key: Some("synthetic-key".into()),
                        cover_url: None,
                    },
                    subtitle: String::new(),
                    track_count: Some(12),
                    can_add_tracks: false,
                })
                .collect(),
            total: Some(count),
            offset: 0,
            next_offset: None,
            playlists_error: None,
            retry_offset: None,
        }
    }

    fn pointer(point: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn owner_cards_open_exact_playlists_and_large_profiles_only_draw_visible_rows() {
        for width in [320.0, 600.0, 1000.0] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut covers = CoverCache::new();
            let page = page(2000);
            let mut tick = 0;
            let mut render = |events| {
                tick += 1;
                let mut action = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 650.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        action = show(
                            ui,
                            &mut covers,
                            Language::English,
                            &page.creator,
                            Some(&page),
                            false,
                            Playback::default(),
                        )
                    },
                );
                output.textures_delta.clear();
                (output, action)
            };
            render(Vec::new());
            let (output, _) = render(Vec::new());
            let titles: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text().starts_with("Collection ") => {
                        Some(text)
                    }
                    _ => None,
                })
                .collect();
            assert!(!titles.is_empty() && titles.len() < 20);
            let point = titles
                .iter()
                .find(|text| text.galley.text() == "Collection 0")
                .unwrap()
                .visual_bounding_rect()
                .center();
            render(pointer(point, true));
            let (output, action) = render(pointer(point, false));
            assert!(output.platform_output.commands.is_empty());
            assert!(
                matches!(action, Some(Action::OpenPlaylist(album)) if album == page.playlists[0].album)
            );
        }
    }

    #[test]
    fn pagination_stops_on_failure_and_private_profiles_never_offer_more() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut covers = CoverCache::new();
        let mut page = page(1);
        page.next_offset = Some(100);
        for (error, private, expected) in [
            (None, false, true),
            (Some(WebAudioError::Network), false, false),
            (None, true, false),
        ] {
            page.playlists_error = error;
            page.unavailable = private;
            let mut more = false;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 1000.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    more = matches!(
                        show(
                            ui,
                            &mut covers,
                            Language::Russian,
                            &page.creator,
                            Some(&page),
                            false,
                            Playback::default(),
                        ),
                        Some(Action::More)
                    )
                },
            );
            output.textures_delta.clear();
            assert_eq!(more, expected);
        }
    }
}
