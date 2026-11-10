//! Rendered regressions for the current settings and bottom list viewport.
use super::*;
use crate::app::tests::{
    account_ui_app, account_ui_click, account_ui_frame, account_ui_shapes, account_ui_text_rect,
    page,
};

fn warm(
    ctx: &egui::Context,
    app: &mut MusicApp,
    size: egui::Vec2,
    tick: &mut u32,
) -> egui::FullOutput {
    account_ui_frame(ctx, app, size, tick, Vec::new());
    account_ui_frame(ctx, app, size, tick, Vec::new());
    account_ui_frame(ctx, app, size, tick, Vec::new())
}

fn content_text(output: &egui::FullOutput, caption: &str) -> egui::Rect {
    account_ui_shapes(output)
        .into_iter()
        .find_map(|shape| match shape {
            egui::Shape::Text(text)
                if text.galley.text() == caption && text.visual_bounding_rect().left() > 252.0 =>
            {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing settings content caption: {caption}"))
}

#[test]
fn settings_heading_shares_the_music_page_top_and_keeps_its_search_below() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = crate::app::tests::search_test_app(&ctx);
        app.language = language;
        app.demo = false;
        let size = egui::vec2(1120.0, 850.0);
        let mut tick = 0;
        app.page = MusicPage::Library;
        let library = warm(&ctx, &mut app, size, &mut tick);
        let music = content_text(&library, language.text("Моя музыка", "My music"));
        let search_top = |output: &egui::FullOutput| {
            account_ui_shapes(output)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == SURFACE
                            && rect.rect.left() > 252.0
                            && rect.rect.width() > 500.0
                            && (30.0..=40.0).contains(&rect.rect.height())
                            && rect.rect.top() > music.bottom() =>
                    {
                        Some(rect.rect.top())
                    }
                    _ => None,
                })
                .min_by(f32::total_cmp)
                .unwrap()
        };
        app.settings_open = true;
        let settings = warm(&ctx, &mut app, size, &mut tick);
        let heading = content_text(&settings, language.text("Настройки", "Settings"));
        let input = content_text(
            &settings,
            language.text("Поиск по настройкам", "Search settings"),
        );
        assert!(
            (heading.top() - music.top()).abs() <= 2.0
                && (heading.bottom() - music.bottom()).abs() < 0.5,
            "Settings title: {heading:?}; music title: {music:?}"
        );
        assert!((heading.left() - music.left()).abs() < 1.0);
        assert!(
            (search_top(&settings) - search_top(&library)).abs() < 0.5,
            "Search surfaces should align: settings={}, library={}",
            search_top(&settings),
            search_top(&library)
        );
        assert!(input.top() > heading.bottom() + 8.0);
        assert!(app.workers.is_empty());
    }
}

#[test]
fn discord_presence_switch_is_searchable_and_does_not_change_playback_or_start_user_ipc_in_tests() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, language);
        app.settings_open = true;
        app.settings_section = SettingsSection::Interface;
        app.settings_query = "discord rpc".into();
        app.discord_rpc_enabled = true;
        app.search.input = "keep music query".into();
        let size = egui::vec2(1120.0, 850.0);
        let mut tick = 0;
        let output = warm(&ctx, &mut app, size, &mut tick);
        let point = content_text(&output, "Discord Rich Presence").center();
        assert!(account_ui_text_rect(&output, language.text("Общие", "General")).is_some());
        assert!(
            account_ui_text_rect(&output, language.text("Включить логи", "Enable logging"))
                .is_none()
        );
        assert!(app.discord_rpc_enabled);
        assert!(
            account_ui_text_rect(
                &output,
                language.text(
                    "Показывать текущий трек в Discord.",
                    "Show the current track in Discord."
                )
            )
            .is_some()
        );
        let volume = app.player.snapshot().volume;
        account_ui_click(&ctx, &mut app, size, &mut tick, point);
        assert!(!app.discord_rpc_enabled && !app.discord_rpc_save_failed);
        assert_eq!(app.search.input, "keep music query");
        assert_eq!(app.player.snapshot().volume, volume);
        assert!(app.workers.is_empty() && app.cache_pending.is_none());
        account_ui_click(&ctx, &mut app, size, &mut tick, point);
        assert!(app.discord_rpc_enabled && !app.discord_rpc_save_failed);
    }
}

#[test]
fn normalize_switch_replaces_settings_volume_and_does_not_change_the_master_level() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, language);
        app.settings_open = true;
        app.settings_section = SettingsSection::Playback;
        let size = egui::vec2(1120.0, 850.0);
        let mut tick = 0;
        let output = warm(&ctx, &mut app, size, &mut tick);
        let label = language.text("Нормализация громкости", "Normalize volume");
        let position = content_text(&output, label).center();
        assert!(!app.player.normalize_volume());
        assert!(account_ui_text_rect(&output, language.text("Громкость", "Volume")).is_none());
        let volume = app.player.snapshot().volume;
        account_ui_click(&ctx, &mut app, size, &mut tick, position);
        assert!(app.player.normalize_volume());
        assert_eq!(app.player.snapshot().volume, volume);
        assert!(app.volume_save_at.is_none() && !app.normalization_save_failed);
        account_ui_click(&ctx, &mut app, size, &mut tick, position);
        assert!(!app.player.normalize_volume());
        assert_eq!(app.player.snapshot().volume, volume);
        assert!(app.volume_save_at.is_none());
    }
}

#[test]
fn tray_switch_is_in_general_and_search_keeps_only_the_matching_new_setting() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, language);
        app.settings_open = true;
        app.settings_section = SettingsSection::General;
        let size = egui::vec2(1120.0, 850.0);
        let mut tick = 0;
        let output = warm(&ctx, &mut app, size, &mut tick);
        let tray = language.text(
            "Сворачивать в системный трей при закрытии",
            "Minimize to system tray when closing",
        );
        let normalize = language.text("Нормализация громкости", "Normalize volume");
        let position = content_text(&output, tray).center();
        assert!(!app.minimize_to_tray);
        account_ui_click(&ctx, &mut app, size, &mut tick, position);
        assert!(app.minimize_to_tray && !app.tray_save_failed);
        account_ui_click(&ctx, &mut app, size, &mut tick, position);
        assert!(!app.minimize_to_tray);
        assert!(app.workers.is_empty());
        app.settings_query = "normalize volume".into();
        let output = warm(&ctx, &mut app, size, &mut tick);
        assert!(account_ui_text_rect(&output, normalize).is_some());
        assert!(account_ui_text_rect(&output, tray).is_none());
        assert!(account_ui_text_rect(&output, language.text("Громкость", "Volume")).is_none());
        app.settings_query = "minimize tray closing".into();
        let output = warm(&ctx, &mut app, size, &mut tick);
        assert!(account_ui_text_rect(&output, tray).is_some());
        assert!(account_ui_text_rect(&output, normalize).is_none());
        assert!(!app.settings_open || app.cache_pending.is_none());
        assert!(app.workers.is_empty());
    }
}

#[test]
fn logging_is_an_opt_in_general_switch_and_search_does_not_touch_music_or_other_settings() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, language);
        app.settings_open = true;
        app.settings_section = SettingsSection::General;
        app.search.input = "keep the music query".into();
        let size = egui::vec2(1120.0, 950.0);
        let mut tick = 0;
        let logging = language.text("Включить логи", "Enable logging");
        let folder = language.text("Открыть папку логов", "Open logs folder");
        let output = warm(&ctx, &mut app, size, &mut tick);
        assert!(account_ui_text_rect(&output, logging).is_some());
        assert!(account_ui_text_rect(&output, folder).is_some());
        assert!(!app.logging_enabled);
        let volume = app.player.snapshot().volume;
        let native_logging = crate::diagnostics::enabled();

        // Searching from another selected section must still find General's
        // matching field, without changing the navigation or music query.
        app.settings_section = SettingsSection::Interface;
        app.settings_query = language.text("логи", "logging debug").into();
        let output = warm(&ctx, &mut app, size, &mut tick);
        let position = content_text(&output, logging).center();
        for caption in [
            language.text("Язык интерфейса", "Interface language"),
            language.text("Системный заголовок окна", "System title bar"),
            language.text("Нормализация громкости", "Normalize volume"),
            language.text(
                "Сворачивать в системный трей при закрытии",
                "Minimize to system tray when closing",
            ),
        ] {
            assert!(
                account_ui_text_rect(&output, caption).is_none(),
                "Unexpected {caption}"
            );
        }
        account_ui_click(&ctx, &mut app, size, &mut tick, position);
        assert!(app.logging_enabled);
        assert!(!app.logging_runtime_failed && !app.logging_save_failed);
        assert!(app.settings_section == SettingsSection::Interface);
        assert_eq!(app.search.input, "keep the music query");
        assert_eq!(app.player.snapshot().volume, volume);
        assert_eq!(
            crate::diagnostics::enabled(),
            native_logging,
            "The test toggle must not open real diagnostic files"
        );
        assert!(app.workers.is_empty() && app.cache_pending.is_none());
        assert!(app.volume_save_at.is_none() && !app.minimize_to_tray);
        assert!(!app.player.normalize_volume());

        account_ui_click(&ctx, &mut app, size, &mut tick, position);
        assert!(!app.logging_enabled);
        assert!(!app.logging_runtime_failed && !app.logging_save_failed);
        assert_eq!(crate::diagnostics::enabled(), native_logging);
        assert!(app.workers.is_empty() && app.cache_pending.is_none());
    }
}

#[test]
fn every_settings_title_has_balanced_space_between_search_and_its_card() {
    for language in [Language::Russian, Language::English] {
        for section in [
            SettingsSection::General,
            SettingsSection::Interface,
            SettingsSection::Playback,
            SettingsSection::Cache,
            SettingsSection::About,
        ] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.settings_open = true;
            app.settings_section = section;
            let size = egui::vec2(1120.0, 950.0);
            let mut tick = 0;
            let output = warm(&ctx, &mut app, size, &mut tick);
            let heading = content_text(&output, section.title(language));
            let surfaces: Vec<_> = account_ui_shapes(&output)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == SURFACE
                            && rect.rect.left() > 252.0
                            && rect.rect.width() > 500.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect();
            let input = surfaces
                .iter()
                .filter(|rect| rect.bottom() < heading.top())
                .max_by(|a, b| a.bottom().total_cmp(&b.bottom()))
                .expect("Settings search surface");
            let card = surfaces
                .iter()
                .filter(|rect| rect.top() > heading.bottom() && rect.height() > 50.0)
                .min_by(|a, b| a.top().total_cmp(&b.top()))
                .expect("Settings card");
            let above = heading.top() - input.bottom();
            let below = card.top() - heading.bottom();
            // Inter's visible glyph bounds differ slightly from its line box.
            assert!(
                (above - below).abs() <= 3.0,
                "{} title is off-center: above={above}, below={below}",
                section.title(language)
            );
            assert!((16.0..=24.0).contains(&above) && (16.0..=24.0).contains(&below));
        }
    }
}

#[test]
fn a_full_library_scroll_view_reaches_the_player_separator_without_a_fixed_bottom_gap() {
    let ctx = egui::Context::default();
    let mut app = account_ui_app(&ctx, Language::English);
    app.demo = true;
    app.library.tracks = page(40, &(1..=40).collect::<Vec<_>>()).items;
    app.library.exhausted = true;
    for track in &mut app.library.tracks {
        track.title = format!("Viewport row {}", track.id);
    }
    let size = egui::vec2(1120.0, 850.0);
    let mut tick = 0;
    let output = warm(&ctx, &mut app, size, &mut tick);
    let separator = account_ui_shapes(&output)
        .into_iter()
        .find_map(|shape| match shape {
            egui::Shape::LineSegment { points, .. }
                if (points[1].x - points[0].x).abs() > size.x * 0.9
                    && (points[1].y - points[0].y).abs() < 0.5
                    && points[0].y > size.y * 0.5 =>
            {
                Some(points[0].y)
            }
            _ => None,
        })
        .expect("Player separator line");
    let viewport_bottom = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().starts_with("Viewport row ") => {
                Some(shape.clip_rect.bottom())
            }
            _ => None,
        })
        .max_by(f32::total_cmp)
        .expect("Visible library row clipping");
    assert!(
        (separator - viewport_bottom).abs() <= 2.0,
        "List ends at {viewport_bottom}, player begins at {separator}"
    );
}

#[test]
fn settings_navigation_uses_main_tab_colors_icons_and_retains_36_point_rows() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, language);
        app.settings_open = true;
        let size = egui::vec2(1120.0, 850.0);
        let mut tick = 0;
        for section in [
            SettingsSection::General,
            SettingsSection::Interface,
            SettingsSection::Playback,
            SettingsSection::Cache,
            SettingsSection::About,
        ] {
            app.settings_section = section;
            let output = warm(&ctx, &mut app, size, &mut tick);
            let selected: Vec<_> = account_ui_shapes(&output)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == theme::SELECTED
                            && rect.rect.right() < 252.0
                            && (rect.rect.width() - 216.0).abs() < 0.1 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(selected.len(), 1);
            assert!((selected[0].height() - 36.0).abs() < 0.1);
            assert!((selected[0].width() - 216.0).abs() < 0.1);
            let caption = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text)
                        if text.galley.text() == section.title(language)
                            && text.visual_bounding_rect().left() < 252.0 =>
                    {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap();
            assert!(caption.left() > selected[0].left() + 40.0);
        }
    }
}

#[test]
fn track_list_empty_notices_follow_vk_counts_and_align_with_heading_but_not_footer() {
    for language in [Language::Russian, Language::English] {
        for playlist in [true, false] {
            for count in [Some(0), Some(7), None] {
                let ctx = egui::Context::default();
                let mut app = account_ui_app(&ctx, language);
                let reference = AlbumRef {
                    id: 71,
                    owner_id: 42,
                    title: "Empty state fixture".into(),
                    access_key: None,
                    cover_url: None,
                };
                let page = if playlist {
                    MusicPage::Playlist(reference.clone())
                } else {
                    MusicPage::Album(reference.clone())
                };
                let data = crate::web_audio::AlbumPage {
                    album: reference,
                    artists: Vec::new(),
                    year: None,
                    description: None,
                    creator: None,
                    tracks: Vec::new(),
                    total_tracks: count,
                    stats: crate::web_audio::AlbumStats::default(),
                };
                app.details.insert(
                    page.detail_key().unwrap(),
                    Arc::new(if playlist {
                        DetailPage::Playlist(data)
                    } else {
                        DetailPage::Album(data)
                    }),
                );
                app.page = page;
                let size = egui::vec2(1120.0, 850.0);
                let mut tick = 0;
                let output = warm(&ctx, &mut app, size, &mut tick);
                let notice = match (playlist, count == Some(0)) {
                    (true, true) => language.text(
                        "В этом плейлисте пока нет треков.",
                        "This playlist is empty.",
                    ),
                    (false, true) => {
                        language.text("В этом альбоме пока нет треков.", "This album is empty.")
                    }
                    (true, false) => language.text(
                        "VK не вернул треки плейлиста.",
                        "VK did not return the playlist tracks.",
                    ),
                    (false, false) => language.text(
                        "VK не вернул треки альбома.",
                        "VK did not return the album tracks.",
                    ),
                };
                let heading = content_text(&output, language.text("Треки", "Tracks"));
                let label = content_text(&output, notice);
                assert!(
                    (heading.left() - label.left()).abs() < 1.5,
                    "{heading:?} {label:?}"
                );
                let footer = content_text(&output, language.text("0 треков", "0 tracks"));
                assert!((heading.left() - footer.left() - 8.0).abs() < 1.5);
                assert!(
                    account_ui_text_rect(&output, language.text("Обновить", "Refresh")).is_none()
                );
            }
        }
    }
}

#[test]
fn track_hover_shows_only_the_clipped_title_and_no_hint_for_a_fitting_title() {
    for long in [false, true] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let mut track = page(1, &[1]).items.remove(0);
        track.title = if long {
            "A very long track title that cannot fit into this narrow music list ".repeat(4)
        } else {
            "Short track".into()
        };
        track.artist = "Artist".into();
        app.library.tracks = vec![track.clone()];
        let size = egui::vec2(800.0, 600.0);
        let mut tick = 0;
        let output = warm(&ctx, &mut app, size, &mut tick);
        let title = account_ui_shapes(&output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == track.display_title() => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .expect("track title");
        let point = egui::pos2(title.left() + 12.0, title.center().y);
        let mut output = output;
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut tick,
            vec![egui::Event::PointerMoved(point)],
        );
        for _ in 0..12 {
            output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        }
        let labels: Vec<_> = account_ui_shapes(&output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(!labels.contains(&format!("{} — {}", track.artist, track.display_title())));
        let titles = labels
            .iter()
            .filter(|label| label.as_str() == track.display_title())
            .count();
        assert_eq!(titles, if long { 2 } else { 1 }, "{labels:?}");
    }
}

#[test]
fn explicit_tracks_show_a_badge_before_their_artists_until_settings_turn_it_off() {
    let ctx = egui::Context::default();
    let mut app = account_ui_app(&ctx, Language::English);
    app.page = MusicPage::Library;
    let mut tracks = page(2, &[1, 2]).items;
    tracks[0].title = "Loud song".into();
    tracks[0].artist = "Rude".into();
    tracks[0].explicit = true;
    tracks[1].title = "Quiet song".into();
    tracks[1].artist = "Polite".into();
    app.library.tracks = tracks;
    let size = egui::vec2(1120.0, 850.0);
    let mut tick = 0;
    let badges = |output: &egui::FullOutput| -> Vec<egui::Rect> {
        account_ui_shapes(output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == "E" => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .collect()
    };
    let output = warm(&ctx, &mut app, size, &mut tick);
    let rude = content_text(&output, "Rude");
    let polite = content_text(&output, "Polite");
    let shown = badges(&output);
    assert_eq!(shown.len(), 1, "only the explicit track: {shown:?}");
    let badge = shown[0];
    // The letter sits in the middle of its box.
    let boxed = account_ui_shapes(&output)
        .into_iter()
        .find_map(|shape| match shape {
            egui::Shape::Rect(rect) if rect.fill == MUTED && rect.rect.contains_rect(badge) => {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("the badge's box");
    assert!(
        (boxed.center() - badge.center()).length() <= 0.25,
        "E off centre: {badge:?} in {boxed:?}"
    );
    // The box is centred on the artist name, capitals to baseline.
    assert!(
        (boxed.center().y - rude.center().y).abs() <= 0.5,
        "badge {boxed:?}, artist {rude:?}"
    );
    assert!(badge.right() < rude.left() && rude.left() - badge.right() < 14.0);
    assert!(
        (badge.center().y - rude.center().y).abs() < 3.0,
        "{badge:?} {rude:?}"
    );
    // The badge (14px for small captions) and its 6px gap come first.
    assert!(
        (rude.left() - polite.left() - 20.0).abs() < 1.5,
        "the artists move right by the badge: {rude:?} {polite:?}"
    );

    app.settings_open = true;
    app.settings_section = SettingsSection::Interface;
    app.settings_query = "explicit".into();
    let settings = warm(&ctx, &mut app, size, &mut tick);
    let point = content_text(&settings, "Explicit badge on tracks").center();
    assert!(app.explicit_badge);
    account_ui_click(&ctx, &mut app, size, &mut tick, point);
    assert!(!app.explicit_badge && !app.explicit_badge_save_failed);
    app.settings_open = false;
    let output = warm(&ctx, &mut app, size, &mut tick);
    assert!(badges(&output).is_empty());
    assert!((content_text(&output, "Rude").left() - polite.left()).abs() < 1.0);
}

#[test]
fn track_titles_and_artists_are_centred_on_the_cover() {
    let ctx = egui::Context::default();
    let mut app = account_ui_app(&ctx, Language::English);
    app.page = MusicPage::Library;
    let mut tracks = page(2, &[1, 2]).items;
    tracks[0].title = "Loud song".into();
    tracks[0].artist = "Rude".into();
    tracks[1].title = "Quiet song".into();
    tracks[1].artist = "Polite".into();
    app.library.tracks = tracks;
    let size = egui::vec2(1120.0, 850.0);
    let mut tick = 0;
    let output = warm(&ctx, &mut app, size, &mut tick);
    let title = content_text(&output, "Loud song");
    let artist = content_text(&output, "Rude");
    let cover = account_ui_shapes(&output)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::Shape::Rect(rect)
                if (rect.rect.width() - 40.0).abs() < 0.1
                    && (rect.rect.height() - 40.0).abs() < 0.1
                    && rect.rect.right() < title.left()
                    && rect.rect.bottom() > title.top()
                    && rect.rect.top() < artist.bottom() =>
            {
                Some(rect.rect)
            }
            egui::Shape::Mesh(mesh) => {
                let r = mesh.calc_bounds();
                ((r.width() - 40.0).abs() < 0.6
                    && (r.height() - 40.0).abs() < 0.6
                    && r.right() < title.left()
                    && r.bottom() > title.top()
                    && r.top() < artist.bottom())
                .then_some(r)
            }
            _ => None,
        })
        .next();
    // From the title's capitals to the artists' baseline, the same space
    // above and below inside the cover.
    let cover = cover.expect("the row's cover");
    let top = title.top() - cover.top();
    let bottom = cover.bottom() - artist.bottom();
    assert!((top - bottom).abs() <= 1.0, "top {top}, bottom {bottom}");
}
