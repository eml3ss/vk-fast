//! Search presentation only. Queries and canonical results are supplied by VK;
//! the application owns requests, keyboard selection, navigation and playback.
use eframe::egui::{self, Align, Color32, Layout, Response, RichText, Sense};

use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    model::{AlbumRef, ArtistRef},
    theme,
    web_audio::UserPlaylist,
};

pub const MAX_HINTS: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Search(String),
    OpenArtist(ArtistRef),
    OpenAlbum(AlbumRef),
    OpenPlaylist(AlbumRef),
}

#[derive(Clone, Copy)]
pub enum QueryKind {
    Popular,
    Suggestions,
}

#[derive(Default)]
pub struct SearchInput {
    pub changed: bool,
    pub submitted: bool,
    pub focused: bool,
}

pub fn input(ui: &mut egui::Ui, query: &mut String, language: Language) -> SearchInput {
    let mut state = SearchInput::default();
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().interact_size.y = 24.0;
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Search, theme::MUTED, 18.0);
                let show_clear = !query.is_empty();
                let width = (ui.available_width() - if show_clear { 32.0 } else { 0.0 }).max(0.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(query)
                        .id_salt("global-search-input")
                        .char_limit(512)
                        .frame(egui::Frame::NONE)
                        .desired_width(width)
                        .hint_text(language.text(
                            "Треки, исполнители, альбомы, плейлисты",
                            "Tracks, artists, albums, playlists",
                        )),
                );
                state.changed = edit.changed();
                state.focused = edit.has_focus();
                state.submitted = (edit.has_focus() || edit.lost_focus())
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if show_clear {
                    let (rect, clear) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
                    let color = if clear.hovered() {
                        ui.visuals().text_color()
                    } else {
                        theme::MUTED
                    };
                    icons::paint(ui.painter(), rect.shrink(2.0), Icon::Close, color);
                    clear.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            language.text("Очистить поиск", "Clear search"),
                        )
                    });
                    if clear
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(language.text("Очистить поиск", "Clear search"))
                        .clicked()
                    {
                        query.clear();
                        state.changed = true;
                        state.submitted = false;
                        state.focused = true;
                        edit.request_focus();
                    }
                }
            });
        });
    state
}

pub fn queries(
    ui: &mut egui::Ui,
    language: Language,
    kind: QueryKind,
    queries: &[String],
    loading: bool,
    error: Option<&str>,
    selected: Option<usize>,
) -> Option<Action> {
    ui.spacing_mut().item_spacing.y = 4.0;
    let popular = matches!(kind, QueryKind::Popular);
    if popular && !queries.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new(language.text("Популярные запросы", "Popular searches"))
                        .size(18.0)
                        .family(theme::semibold()),
                )
                .selectable(false),
            );
            if loading {
                ui.add(egui::Spinner::new().size(15.0));
            }
        });
        ui.add_space(2.0);
    } else if loading && queries.is_empty() {
        ui.add(egui::Spinner::new().size(15.0));
    }
    if let Some(error) = error {
        ui.add(
            egui::Label::new(RichText::new(error).color(Color32::from_rgb(255, 158, 153)))
                .wrap()
                .selectable(false),
        );
        ui.add_space(6.0);
    }
    if queries.is_empty() {
        if !loading && error.is_none() {
            ui.add(
                egui::Label::new(
                    RichText::new(if popular {
                        language.text(
                            "VK пока не вернул популярные запросы.",
                            "VK has not returned popular searches yet.",
                        )
                    } else {
                        language.text(
                            "Нажмите Enter, чтобы найти музыку.",
                            "Press Enter to search for music.",
                        )
                    })
                    .color(theme::MUTED),
                )
                .wrap()
                .selectable(false),
            );
        }
        return None;
    }
    let mut action = None;
    ui.spacing_mut().item_spacing.y = 4.0;
    let maximum = if popular { 20 } else { MAX_HINTS };
    for (index, query) in queries.iter().take(maximum).enumerate() {
        ui.push_id(
            (
                if popular {
                    "popular-search"
                } else {
                    "search-hint"
                },
                index,
            ),
            |ui| {
                if query_item(
                    ui,
                    query,
                    ui.available_width(),
                    !popular && selected == Some(index),
                )
                .clicked()
                {
                    action = Some(Action::Search(query.clone()));
                }
            },
        );
    }
    action
}

fn query_item(ui: &mut egui::Ui, query: &str, width: f32, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width.max(1.0), 38.0), Sense::click());
    if response.clicked() {
        response.request_focus();
    }
    let fill = if selected || response.hovered() || response.has_focus() {
        theme::SELECTED
    } else {
        theme::SURFACE
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(18.0, 0.0),
            egui::vec2(15.0, 15.0),
        ),
        Icon::Search,
        theme::MUTED,
    );
    let text = egui::Rect::from_min_max(
        rect.min + egui::vec2(34.0, 0.0),
        rect.max - egui::vec2(12.0, 0.0),
    );
    paint_text(
        ui,
        text,
        query,
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), query)
    });
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(query)
}

pub fn results_metadata(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    artists: &[ArtistRef],
    albums: &[AlbumRef],
) -> Option<Action> {
    results_metadata_sections(ui, covers, language, artists, albums, true)
}

/// Category tabs already name their content; combined results keep section
/// headings so artists and albums remain distinguishable in the overview.
pub fn results_metadata_sections(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    artists: &[ArtistRef],
    albums: &[AlbumRef],
    show_headings: bool,
) -> Option<Action> {
    let mut action = None;
    let columns = ((ui.available_width() + 12.0) / 230.0)
        .floor()
        .clamp(1.0, 3.0) as usize;
    let width = ((ui.available_width() - 12.0 * columns.saturating_sub(1) as f32) / columns as f32)
        .max(1.0);
    if !artists.is_empty() {
        if show_headings {
            heading(ui, language.text("Исполнители", "Artists"));
        }
        for row in artists[..artists.len().min(6)].chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for artist in row {
                    ui.push_id(("search-artist", &artist.id), |ui| {
                        let (rect, response) = card(ui, width, &artist.name);
                        let art = egui::Rect::from_center_size(
                            rect.left_center() + egui::vec2(34.0, 0.0),
                            egui::vec2(44.0, 44.0),
                        );
                        ui.painter()
                            .circle_filled(art.center(), 22.0, theme::SELECTED);
                        let initials: String = artist
                            .name
                            .split_whitespace()
                            .filter_map(|word| word.chars().next())
                            .take(2)
                            .flat_map(char::to_uppercase)
                            .collect();
                        ui.painter().text(
                            art.center(),
                            egui::Align2::CENTER_CENTER,
                            initials,
                            egui::FontId::new(16.0, theme::semibold()),
                            theme::MUTED,
                        );
                        caption(
                            ui,
                            rect,
                            &artist.name,
                            language.text("Исполнитель", "Artist"),
                        );
                        if response.clicked() {
                            action = Some(Action::OpenArtist(artist.clone()));
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
        if show_headings {
            ui.add_space(14.0);
        }
    }
    if !albums.is_empty() {
        if show_headings {
            heading(ui, language.text("Альбомы", "Albums"));
        }
        for row in albums[..albums.len().min(6)].chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for album in row {
                    ui.push_id(("search-album", album.owner_id, album.id), |ui| {
                        let title = if album.title.is_empty() {
                            language.text("Альбом", "Album")
                        } else {
                            &album.title
                        };
                        let (rect, response) = card(ui, width, title);
                        let mut image = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt("search-album-art")
                                .max_rect(egui::Rect::from_min_size(
                                    rect.min + egui::vec2(12.0, 10.0),
                                    egui::vec2(44.0, 44.0),
                                ))
                                .layout(Layout::top_down(Align::Min)),
                        );
                        covers.show(&mut image, album.cover_url.as_deref(), 44.0);
                        caption(ui, rect, title, language.text("Альбом", "Album"));
                        if response.clicked() {
                            action = Some(Action::OpenAlbum(album.clone()));
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
        if show_headings {
            ui.add_space(14.0);
        }
    }
    action
}

/// The combined overview previews a bounded set; the dedicated category can
/// show every playlist returned by VK without repeating its tab heading.
pub fn playlist_results(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    playlists: &[UserPlaylist],
    show_heading: bool,
    limit: Option<usize>,
) -> Option<Action> {
    let playlists = &playlists[..limit.unwrap_or(playlists.len()).min(playlists.len())];
    if playlists.is_empty() {
        return None;
    }
    if show_heading {
        heading(ui, language.text("Плейлисты", "Playlists"));
    }
    let columns = ((ui.available_width() + 12.0) / 230.0)
        .floor()
        .clamp(1.0, 3.0) as usize;
    let width = ((ui.available_width() - 12.0 * columns.saturating_sub(1) as f32) / columns as f32)
        .max(1.0);
    let mut action = None;
    for row in playlists.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            for playlist in row {
                let reference = &playlist.album;
                ui.push_id(
                    ("search-playlist", reference.owner_id, reference.id),
                    |ui| {
                        let title = if reference.title.is_empty() {
                            language.text("Плейлист", "Playlist")
                        } else {
                            &reference.title
                        };
                        let (rect, response) = card(ui, width, title);
                        let mut image = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt("search-playlist-art")
                                .max_rect(egui::Rect::from_min_size(
                                    rect.min + egui::vec2(12.0, 10.0),
                                    egui::vec2(44.0, 44.0),
                                ))
                                .layout(Layout::top_down(Align::Min)),
                        );
                        covers.show(&mut image, reference.cover_url.as_deref(), 44.0);
                        caption(ui, rect, title, &playlist_caption(language, playlist));
                        if response.clicked() {
                            action = Some(Action::OpenPlaylist(reference.clone()));
                        }
                    },
                );
            }
        });
        ui.add_space(4.0);
    }
    if show_heading {
        ui.add_space(14.0);
    }
    action
}

fn playlist_caption(language: Language, playlist: &UserPlaylist) -> String {
    let kind = language.text("Плейлист", "Playlist");
    if let Some(count) = playlist.track_count {
        let noun = match language {
            Language::Russian => match (count % 100, count % 10) {
                (11..=14, _) => "треков",
                (_, 1) => "трек",
                (_, 2..=4) => "трека",
                _ => "треков",
            },
            Language::English => {
                if count == 1 {
                    "track"
                } else {
                    "tracks"
                }
            }
        };
        format!("{kind} · {count} {noun}")
    } else if !playlist.subtitle.trim().is_empty() {
        format!("{kind} · {}", playlist.subtitle)
    } else {
        kind.into()
    }
}

fn heading(ui: &mut egui::Ui, title: &str) {
    theme::inset_heading(ui, |ui| {
        ui.add(
            egui::Label::new(RichText::new(title).size(20.0).family(theme::semibold()))
                .selectable(false),
        )
    });
    ui.add_space(8.0);
}

fn card(ui: &mut egui::Ui, width: f32, label: &str) -> (egui::Rect, Response) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 64.0), Sense::click());
    if response.clicked() {
        response.request_focus();
    }
    ui.painter().rect_filled(
        rect,
        10.0,
        if response.hovered() || response.has_focus() {
            Color32::from_rgb(31, 38, 49)
        } else {
            theme::SURFACE
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    (
        rect,
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
    )
}

fn caption(ui: &egui::Ui, rect: egui::Rect, title: &str, subtitle: &str) {
    let left = rect.left() + 68.0;
    paint_text(
        ui,
        egui::Rect::from_min_max(
            egui::pos2(left, rect.top() + 12.0),
            egui::pos2(rect.right() - 10.0, rect.top() + 34.0),
        ),
        title,
        egui::FontId::new(14.0, theme::semibold()),
        ui.visuals().text_color(),
    );
    paint_text(
        ui,
        egui::Rect::from_min_max(
            egui::pos2(left, rect.top() + 34.0),
            egui::pos2(rect.right() - 10.0, rect.bottom() - 10.0),
        ),
        subtitle,
        egui::TextStyle::Small.resolve(ui.style()),
        theme::MUTED,
    );
}

fn paint_text(ui: &egui::Ui, rect: egui::Rect, text: &str, font: egui::FontId, color: Color32) {
    if !rect.is_positive() {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        context: &egui::Context,
        width: f32,
        tick: u32,
        events: Vec<egui::Event>,
        mut render: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 1200.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| {
                render(ui);
            },
        );
        output.textures_delta.clear();
        output
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

    fn key(key: egui::Key, pressed: bool) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    fn text_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing label {label}"))
    }

    #[test]
    fn toolbar_input_is_compact_and_clear_fits_narrow_fields() {
        for language in [Language::Russian, Language::English] {
            for width in [200.0, 300.0, 600.0] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut query = String::from("Known query");
                let mut state = SearchInput::default();
                let mut output = frame(&context, width, 1, vec![], |ui| {
                    state = input(ui, &mut query, language);
                });
                for tick in 2..4 {
                    output = frame(&context, width, tick, vec![], |ui| {
                        state = input(ui, &mut query, language);
                    });
                }
                let field = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == theme::SURFACE && rect.rect.width() > 100.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Search field background");
                assert!(
                    (field.height() - 38.0).abs() < 0.5,
                    "Toolbar field must match the icon buttons: {field:?}"
                );
                assert!(
                    field.width() <= width + 0.5,
                    "Field overflowed the toolbar: {field:?}"
                );
                let clear = egui::pos2(field.right() - 24.0, field.center().y);
                frame(&context, width, 4, pointer(clear, true), |ui| {
                    state = input(ui, &mut query, language);
                });
                frame(&context, width, 5, pointer(clear, false), |ui| {
                    state = input(ui, &mut query, language);
                });
                assert!(
                    query.is_empty(),
                    "Clear control must remain reachable at width {width}"
                );
                assert!(state.changed && state.focused && !state.submitted);
            }
        }
    }

    #[test]
    fn text_input_reports_edits_and_enter_once_and_limits_pasted_queries() {
        for language in [Language::Russian, Language::English] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut query = String::from("Known query");
            let mut state = SearchInput::default();
            let mut output = frame(&context, 600.0, 1, vec![], |ui| {
                state = input(ui, &mut query, language);
            });
            for tick in 2..4 {
                output = frame(&context, 600.0, tick, vec![], |ui| {
                    state = input(ui, &mut query, language);
                });
            }
            let point = text_rect(&output, "Known query").center();
            frame(&context, 600.0, 4, pointer(point, true), |ui| {
                state = input(ui, &mut query, language);
            });
            frame(&context, 600.0, 5, pointer(point, false), |ui| {
                state = input(ui, &mut query, language);
            });
            assert!(state.focused);
            frame(
                &context,
                600.0,
                6,
                vec![egui::Event::Paste("я".repeat(600))],
                |ui| {
                    state = input(ui, &mut query, language);
                },
            );
            assert!(state.changed);
            assert_eq!(query.chars().count(), 512);
            frame(&context, 600.0, 7, key(egui::Key::Enter, true), |ui| {
                state = input(ui, &mut query, language);
            });
            assert!(state.submitted);
            frame(&context, 600.0, 8, key(egui::Key::Enter, false), |ui| {
                state = input(ui, &mut query, language);
            });
            assert!(!state.submitted);
        }
    }

    #[test]
    fn only_nonempty_popular_searches_have_a_heading() {
        for language in [Language::Russian, Language::English] {
            for (kind, values, heading) in [
                (QueryKind::Popular, Vec::<String>::new(), false),
                (QueryKind::Popular, vec!["A real query".into()], true),
                (QueryKind::Suggestions, vec!["A real query".into()], false),
            ] {
                let context = egui::Context::default();
                theme::install(&context);
                let output = frame(&context, 600.0, 1, vec![], |ui| {
                    queries(ui, language, kind, &values, false, None, None);
                });
                let contains = |caption: &str| {
                    output.shapes.iter().any(|shape| {
                        matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text() == caption)
                    })
                };
                assert_eq!(
                    contains(language.text("Популярные запросы", "Popular searches")),
                    heading
                );
                assert!(!contains(language.text("Подсказки", "Suggestions")));
            }
        }
    }

    #[test]
    fn both_popular_queries_and_suggestions_return_the_clicked_text() {
        let values = [
            String::from("First query"),
            String::from("Name, With & Punctuation"),
        ];
        for kind in [QueryKind::Popular, QueryKind::Suggestions] {
            for language in [Language::Russian, Language::English] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut action = None;
                let mut output = frame(&context, 600.0, 1, vec![], |ui| {
                    action = queries(ui, language, kind, &values, false, None, Some(1));
                });
                for tick in 2..4 {
                    output = frame(&context, 600.0, tick, vec![], |ui| {
                        action = queries(ui, language, kind, &values, false, None, Some(1));
                    });
                }
                let point = text_rect(&output, &values[1]).center();
                frame(&context, 600.0, 4, pointer(point, true), |ui| {
                    action = queries(ui, language, kind, &values, false, None, Some(1));
                });
                frame(&context, 600.0, 5, pointer(point, false), |ui| {
                    action = queries(ui, language, kind, &values, false, None, Some(1));
                });
                assert_eq!(action, Some(Action::Search(values[1].clone())));
            }
        }
    }

    fn playlist(id: i64) -> UserPlaylist {
        UserPlaylist {
            album: AlbumRef {
                owner_id: -42,
                id,
                title: format!("Playlist {id}"),
                access_key: Some(format!("synthetic-{id}")),
                cover_url: None,
            },
            subtitle: "Native playlist description".into(),
            track_count: Some(24),
            can_add_tracks: false,
        }
    }

    #[test]
    fn playlist_cards_open_the_exact_playlist_and_do_not_turn_it_into_an_album() {
        for language in [Language::Russian, Language::English] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let items = [playlist(17)];
            let mut action = None;
            let mut start = 0.0;
            let mut output = frame(&context, 600.0, 1, vec![], |ui| {
                start = ui.cursor().top();
                action = playlist_results(ui, &mut covers, language, &items, false, None);
            });
            for tick in 2..4 {
                output = frame(&context, 600.0, tick, vec![], |ui| {
                    start = ui.cursor().top();
                    action = playlist_results(ui, &mut covers, language, &items, false, None);
                });
            }
            let card = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == theme::SURFACE && rect.rect.height() == 64.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(
                (card.top() - start).abs() < 0.5,
                "The tab must not leave a hidden heading gap"
            );
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|shape| matches!(&shape.shape,egui::Shape::Text(text)
                if text.galley.text()==language.text("Плейлисты","Playlists")))
            );
            assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text)
                if text.galley.text()==language.text("Плейлист · 24 трека","Playlist · 24 tracks"))));
            let target = text_rect(&output, &items[0].album.title).center();
            frame(&context, 600.0, 4, pointer(target, true), |ui| {
                action = playlist_results(ui, &mut covers, language, &items, false, None);
            });
            frame(&context, 600.0, 5, pointer(target, false), |ui| {
                action = playlist_results(ui, &mut covers, language, &items, false, None);
            });
            assert_eq!(action, Some(Action::OpenPlaylist(items[0].album.clone())));
        }
    }

    #[test]
    fn playlist_overview_previews_six_while_the_category_shows_all_and_fits_narrow_widths() {
        for width in [160.0, 320.0, 740.0] {
            for overview in [true, false] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut covers = CoverCache::new();
                let mut items: Vec<_> = (1..=10).map(playlist).collect();
                items[0].album.title = "Очень длинное название плейлиста ".repeat(10);
                let mut output = frame(&context, width, 1, vec![], |ui| {
                    playlist_results(
                        ui,
                        &mut covers,
                        Language::Russian,
                        &items,
                        overview,
                        overview.then_some(6),
                    );
                });
                for tick in 2..4 {
                    output = frame(&context, width, tick, vec![], |ui| {
                        playlist_results(
                            ui,
                            &mut covers,
                            Language::Russian,
                            &items,
                            overview,
                            overview.then_some(6),
                        );
                    });
                }
                let cards = output
                    .shapes
                    .iter()
                    .filter(|shape| {
                        matches!(&shape.shape,egui::Shape::Rect(rect)
                    if rect.fill==theme::SURFACE && rect.rect.height()==64.0)
                    })
                    .count();
                assert_eq!(cards, if overview { 6 } else { 10 });
                let headings = output
                    .shapes
                    .iter()
                    .filter(|shape| {
                        matches!(&shape.shape,egui::Shape::Text(text)
                    if text.galley.text()=="Плейлисты")
                    })
                    .count();
                assert_eq!(headings, usize::from(overview));
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        assert!(
                            text.visual_bounding_rect().right() <= width + 1.0,
                            "Overflow at {width}: {:?}",
                            text.visual_bounding_rect()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn playlist_captions_localize_counts_and_keep_missing_counts_absent() {
        let mut item = playlist(1);
        for (count, expected) in [
            (0, "0 треков"),
            (1, "1 трек"),
            (2, "2 трека"),
            (11, "11 треков"),
            (21, "21 трек"),
            (24, "24 трека"),
        ] {
            item.track_count = Some(count);
            assert_eq!(
                playlist_caption(Language::Russian, &item),
                format!("Плейлист · {expected}")
            );
        }
        item.track_count = Some(1);
        assert_eq!(
            playlist_caption(Language::English, &item),
            "Playlist · 1 track"
        );
        item.track_count = Some(2);
        assert_eq!(
            playlist_caption(Language::English, &item),
            "Playlist · 2 tracks"
        );
        item.track_count = None;
        assert_eq!(
            playlist_caption(Language::English, &item),
            "Playlist · Native playlist description"
        );
        item.subtitle.clear();
        assert_eq!(playlist_caption(Language::Russian, &item), "Плейлист");
    }

    #[test]
    fn category_metadata_starts_at_the_cursor_without_repeating_the_tab_heading() {
        let artist = ArtistRef {
            id: "canonical-artist".into(),
            name: "Canonical artist".into(),
        };
        let album = AlbumRef {
            id: 7,
            owner_id: -42,
            title: "Canonical release".into(),
            access_key: None,
            cover_url: None,
        };
        for language in [Language::Russian, Language::English] {
            for artist_tab in [true, false] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut covers = CoverCache::new();
                let mut start = 0.0;
                let mut action = None;
                let artists = if artist_tab {
                    std::slice::from_ref(&artist)
                } else {
                    &[]
                };
                let albums = if artist_tab {
                    &[]
                } else {
                    std::slice::from_ref(&album)
                };
                let mut output = frame(&context, 600.0, 1, vec![], |ui| {
                    start = ui.cursor().min.y;
                    action = results_metadata_sections(
                        ui,
                        &mut covers,
                        language,
                        artists,
                        albums,
                        false,
                    );
                });
                for tick in 2..4 {
                    output = frame(&context, 600.0, tick, vec![], |ui| {
                        start = ui.cursor().min.y;
                        action = results_metadata_sections(
                            ui,
                            &mut covers,
                            language,
                            artists,
                            albums,
                            false,
                        );
                    });
                }
                let card = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == theme::SURFACE && rect.rect.height() == 64.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Category card");
                assert!(
                    (card.top() - start).abs() < 0.5,
                    "A hidden category heading must not leave a blank gap"
                );
                for heading in [
                    language.text("Исполнители", "Artists"),
                    language.text("Альбомы", "Albums"),
                ] {
                    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == heading)));
                }
                let label = if artist_tab {
                    &artist.name
                } else {
                    &album.title
                };
                let point = text_rect(&output, label).center();
                frame(&context, 600.0, 4, pointer(point, true), |ui| {
                    action = results_metadata_sections(
                        ui,
                        &mut covers,
                        language,
                        artists,
                        albums,
                        false,
                    );
                });
                frame(&context, 600.0, 5, pointer(point, false), |ui| {
                    action = results_metadata_sections(
                        ui,
                        &mut covers,
                        language,
                        artists,
                        albums,
                        false,
                    );
                });
                let expected = if artist_tab {
                    Action::OpenArtist(artist.clone())
                } else {
                    Action::OpenAlbum(album.clone())
                };
                assert_eq!(action, Some(expected));
            }
        }
    }

    #[test]
    fn result_cards_open_exact_canonical_artist_and_album_references() {
        let artist = ArtistRef {
            id: "canonical-artist".into(),
            name: "Canonical artist".into(),
        };
        let album = AlbumRef {
            id: -7,
            owner_id: 42,
            title: "Canonical release".into(),
            access_key: Some("synthetic".into()),
            cover_url: None,
        };
        for (label, expected) in [
            (&artist.name, Action::OpenArtist(artist.clone())),
            (&album.title, Action::OpenAlbum(album.clone())),
        ] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let mut action = None;
            let mut output = frame(&context, 600.0, 1, vec![], |ui| {
                action = results_metadata(
                    ui,
                    &mut covers,
                    Language::English,
                    std::slice::from_ref(&artist),
                    std::slice::from_ref(&album),
                );
            });
            for tick in 2..4 {
                output = frame(&context, 600.0, tick, vec![], |ui| {
                    action = results_metadata(
                        ui,
                        &mut covers,
                        Language::English,
                        std::slice::from_ref(&artist),
                        std::slice::from_ref(&album),
                    );
                });
            }
            let point = text_rect(&output, label).center();
            frame(&context, 600.0, 4, pointer(point, true), |ui| {
                action = results_metadata(
                    ui,
                    &mut covers,
                    Language::English,
                    std::slice::from_ref(&artist),
                    std::slice::from_ref(&album),
                );
            });
            frame(&context, 600.0, 5, pointer(point, false), |ui| {
                action = results_metadata(
                    ui,
                    &mut covers,
                    Language::English,
                    std::slice::from_ref(&artist),
                    std::slice::from_ref(&album),
                );
            });
            assert_eq!(action, Some(expected));
        }
    }

    #[test]
    fn long_queries_and_metadata_fit_narrow_widths_and_suggestions_are_bounded() {
        for width in [160.0, 320.0, 740.0] {
            let context = egui::Context::default();
            theme::install(&context);
            let values: Vec<_> = (0..30)
                .map(|index| format!("{index} Очень длинный музыкальный запрос ").repeat(10))
                .collect();
            let mut covers = CoverCache::new();
            let artists = [ArtistRef {
                id: "long-name".into(),
                name: "Очень длинное имя исполнителя ".repeat(10),
            }];
            let albums = [AlbumRef {
                id: 1,
                owner_id: -42,
                title: "Очень длинное имя релиза ".repeat(10),
                access_key: None,
                cover_url: None,
            }];
            let mut output = frame(&context, width, 1, vec![], |ui| {
                queries(
                    ui,
                    Language::Russian,
                    QueryKind::Suggestions,
                    &values,
                    false,
                    None,
                    None,
                );
                results_metadata(ui, &mut covers, Language::Russian, &artists, &albums);
            });
            for tick in 2..4 {
                output = frame(&context, width, tick, vec![], |ui| {
                    queries(
                        ui,
                        Language::Russian,
                        QueryKind::Suggestions,
                        &values,
                        false,
                        None,
                        None,
                    );
                    results_metadata(ui, &mut covers, Language::Russian, &artists, &albums);
                });
            }
            let queries_shown = output.shapes.iter().filter(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if values.iter().any(|value| text.galley.text() == *value)
            )).count();
            assert_eq!(queries_shown, MAX_HINTS);
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    assert!(
                        text.visual_bounding_rect().right() <= width + 1.0,
                        "Overflow at {width}: {:?}",
                        text.visual_bounding_rect()
                    );
                }
            }
        }
    }
}
