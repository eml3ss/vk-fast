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
