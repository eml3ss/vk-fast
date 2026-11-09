use super::*;
use crate::app::tests::{
    account_ui_click, account_ui_frame, account_ui_shapes, account_ui_text_rect, page,
    search_test_app,
};

fn prepared(ctx: &egui::Context, language: Language) -> MusicApp {
    let mut app = search_test_app(ctx);
    app.demo = false;
    app.language = language;
    let mut track = page(1, &[7]).items.remove(0);
    track.duration = 180;
    track.title = "LYRICS_TRACK".into();
    track.artist = "Synthetic artist".into();
    app.library.tracks = vec![track.clone()];
    app.library.exhausted = true;
    app.playing = Some(track.clone());
    app.selected = Some(track.key());
    app.resume_position = Some(20.0);
    app.lyrics.query = Some(Query::from_track(&track));
    app.lyrics.data = Some(Arc::new(lyrics::test_data(18, true)));
    app
}

#[test]
fn right_panel_layout_survives_reopening_and_restoring_the_last_track() {
    for lyrics_open in [false, true] {
        for queue_open in [false, true] {
            let ctx = egui::Context::default();
            let mut app = prepared(&ctx, Language::English);
            let state = crate::window_state::RightPanel {
                lyrics_open,
                queue_open,
                split: 0.37,
            };
            app.restore_right_panel_state(state);
            let saved = app.right_panel_state();
            assert_eq!(saved, state);

            let mut reopened = prepared(&ctx, Language::English);
            reopened.restore_right_panel_state(saved);
            let snapshot = playback_resume::Snapshot {
                queue: None,
                track: reopened.playing.clone().unwrap(),
                position: 25.0,
            };
            reopened.restore_playback_resume(42, Some(snapshot));
            assert_eq!(reopened.right_panel_state(), saved);
            assert_eq!(reopened.resume_position, Some(25.0));
            assert!(!reopened.lyrics.fullscreen);
        }
    }
}

#[test]
fn lyrics_open_below_the_queue_and_fullscreen_restores_the_drawer_on_escape() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = prepared(&ctx, language);
        app.queue_ui.open = true;
        app.toggle_lyrics(&ctx);
        assert!(app.lyrics.open && app.lyrics.active && app.queue_ui.open);
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..4 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let heading = account_ui_text_rect(&output, language.text("Текст", "Lyrics")).unwrap();
        assert!(heading.left() > size.x * 0.5);
        // Both share the right panel: the queue on top, the lyrics below.
        let queue = account_ui_text_rect(&output, language.text("Очередь", "Queue")).unwrap();
        assert!(queue.left() > size.x * 0.5 && queue.bottom() < heading.top());
        app.set_lyrics_fullscreen(&ctx, true);
        let command = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        assert!(
            command.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| if cfg!(windows) {
                    matches!(command, egui::ViewportCommand::Maximized(true))
                } else {
                    matches!(command, egui::ViewportCommand::Fullscreen(true))
                })
        );
        for _ in 0..4 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        assert!(app.lyrics.fullscreen);
        assert!(account_ui_text_rect(&output, language.text("Главная", "Home")).is_none());
        assert!(account_ui_text_rect(&output, language.text("Моя музыка", "My music")).is_none());
        let heading = account_ui_text_rect(&output, language.text("Текст", "Lyrics")).unwrap();
        assert!(heading.left() < 60.0 && heading.top() < 60.0);
        let text = account_ui_text_rect(&output, "Synthetic lyric line 2").unwrap();
        assert!(text.left() > size.x * 0.4 && text.bottom() < size.y - 96.0);
        let command = account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            command.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| if cfg!(windows) {
                    matches!(command, egui::ViewportCommand::Maximized(false))
                } else {
                    matches!(command, egui::ViewportCommand::Fullscreen(false))
                })
        );
        assert!(!app.lyrics.fullscreen && app.lyrics.open);
        app.toggle_queue(&ctx);
        assert!(!app.queue_ui.open && app.lyrics.open && app.lyrics.active);
    }
}

#[test]
fn late_lyrics_replies_never_replace_the_current_track_or_restart_after_close() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx, Language::English);
    let query = app.lyrics.query.clone().unwrap();
    app.lyrics.data = None;
    app.lyrics.pending = Some(8);
    app.apply_lyrics_event(Event::Lyrics {
        id: 7,
        query: query.clone(),
        result: Ok(Arc::new(lyrics::test_data(2, true))),
    });
    assert!(app.lyrics.data.is_none() && app.lyrics.pending == Some(8));
    let mut other = query.clone();
    other.key = (42, 9);
    app.apply_lyrics_event(Event::Lyrics {
        id: 8,
        query: other,
        result: Ok(Arc::new(lyrics::test_data(2, true))),
    });
    assert!(app.lyrics.data.is_none());
    app.apply_lyrics_event(Event::Lyrics {
        id: 8,
        query,
        result: Ok(Arc::new(lyrics::test_data(2, true))),
    });
    assert_eq!(app.lyrics.data.as_ref().unwrap().lines.len(), 2);
    app.handle_lyrics_action(&ctx, lyrics_ui::Action::Close);
    app.poll_lyrics(&ctx);
    assert!(!app.lyrics.open && app.lyrics.pending.is_none() && app.lyrics.service.is_none());
}

#[test]
fn lyric_seek_uses_the_shared_player_position_and_key_input_stays_masked() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx, Language::English);
    app.handle_lyrics_action(&ctx, lyrics_ui::Action::Seek(42.5));
    assert_eq!(app.resume_position, Some(42.5));
    app.settings_open = true;
    app.settings_section = SettingsSection::Playback;
    app.settings_query = "musixmatch".into();
    app.lyrics.status = Some(Status {
        mode: Mode::ApiKey,
        ..Default::default()
    });
    app.lyrics.settings_error = Some(Error::Storage);
    *app.lyrics.key_input = "synthetic-private-key".into();
    let mut frame = 0;
    let size = egui::vec2(1040.0, 720.0);
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..3 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    assert!(account_ui_text_rect(&output, "Personal API key").is_some());
    assert!(account_ui_text_rect(&output, "Normalize volume").is_none());
    for shape in account_ui_shapes(&output) {
        if let egui::Shape::Text(text) = shape {
            assert!(!text.galley.text().contains("synthetic-private-key"));
        }
    }
}

#[test]
fn long_lyrics_only_paint_visible_lines_in_the_drawer_and_fullscreen() {
    for fullscreen in [false, true] {
        let ctx = egui::Context::default();
        let mut app = prepared(&ctx, Language::English);
        app.lyrics.data = Some(Arc::new(lyrics::test_data(1000, true)));
        app.lyrics.active = true;
        app.lyrics.open = true;
        app.lyrics.fullscreen = fullscreen;
        let size = egui::vec2(800.0, 540.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..4 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let rows: Vec<_> = account_ui_shapes(&output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text)
                    if text.galley.text().starts_with("Synthetic lyric line") =>
                {
                    Some(text)
                }
                _ => None,
            })
            .collect();
        assert!(!rows.is_empty() && rows.len() < 24);
        for row in rows {
            assert!(row.visual_bounding_rect().right() <= size.x + 1.0);
        }
    }
}

#[test]
fn lyric_button_sits_to_the_left_of_queue_in_both_player_layouts() {
    for compact in [false, true] {
        let ctx = egui::Context::default();
        let mut app = prepared(&ctx, Language::English);
        app.compact_seek_bar = compact;
        app.queue_ui.open = true;
        let size = egui::vec2(800.0, 540.0);
        let mut frame = 0;
        for _ in 0..4 {
            account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let lyrics = egui::pos2(size.x - if compact { 220.0 } else { 240.0 }, size.y - 50.0);
        account_ui_click(&ctx, &mut app, size, &mut frame, lyrics);
        assert!(app.lyrics.open && app.lyrics.active && app.queue_ui.open);
        let queue = egui::pos2(size.x - if compact { 172.0 } else { 192.0 }, size.y - 50.0);
        account_ui_click(&ctx, &mut app, size, &mut frame, queue);
        assert!(!app.queue_ui.open && app.lyrics.open && app.lyrics.active);
    }
}
