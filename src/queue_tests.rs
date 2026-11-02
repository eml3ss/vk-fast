use super::*;
use crate::app::tests::{
    account_ui_click, account_ui_frame, account_ui_shapes, account_ui_text_rect, page,
    search_test_app,
};
use crate::system_media::Command;

fn prepared(ctx: &egui::Context) -> MusicApp {
    let mut app = search_test_app(ctx);
    app.demo = false;
    app.library.tracks = page(3, &[1, 2, 3]).items;
    app.library.exhausted = true;
    app.play_queue = Some(app.library.tracks.clone());
    app.queue_origin = Some("playlist:42_9".into());
    app.playing = Some(app.library.tracks[0].clone());
    app.selected = app.playing.as_ref().map(Track::key);
    app.resume_position = Some(12.0);
    app.synchronize_order();
    app
}

fn external() -> Track {
    let mut track = page(1, &[7]).items.remove(0);
    track.owner_id = 99;
    track.title = "Queued entry".into();
    track.artist = "Queue artist".into();
    track
}

#[test]
fn queue_actions_and_media_navigation_work_without_ui_frames_or_changing_the_source() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    let source = app.library.tracks[0].key();
    let next_source = app.library.tracks[1].key();
    let queued = external();
    let key = queued.key();
    for _ in 0..2 {
        app.handle_track_menu_action(&ctx, &queued, track_menu::Action::AddToQueue);
    }
    assert_eq!(app.playing.as_ref().unwrap().key(), source);
    assert_eq!(app.play_order.queued().len(), 2);
    for expected in [key, key, next_source] {
        app.handle_system_media_command(&ctx, Command::Next);
        assert_eq!(app.playing.as_ref().unwrap().key(), expected);
        assert_eq!(app.resume_position, Some(0.0));
        assert_eq!(app.queue_origin.as_deref(), Some("playlist:42_9"));
        assert_eq!(app.player.decoder_starts(), 0);
    }
    app.handle_system_media_command(&ctx, Command::Previous);
    assert_eq!(app.playing.as_ref().unwrap().key(), key);
    assert!(app.workers.is_empty());
    app.handle_system_media_command(&ctx, Command::Next);
    assert_eq!(app.playing.as_ref().unwrap().key(), next_source);
    app.stop_playback();
    assert!(app.play_order.queued().is_empty() && app.queued_tracks.is_empty());
    app.selected = None;
    app.add_to_queue(&ctx, external());
    app.handle_system_media_command(&ctx, Command::Next);
    assert_eq!(app.playing.as_ref().unwrap().key(), key);
    assert!(app.play_order.queued().is_empty());
    assert_eq!(app.player.decoder_starts(), 0);
}

#[test]
fn natural_end_uses_the_manual_queue_and_a_restricted_queued_track_is_skipped() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    let queued = external();
    let key = queued.key();
    app.add_to_queue(&ctx, queued.clone());
    app.add_to_queue(&ctx, queued);
    app.advance_track(&ctx, true);
    assert_eq!(app.playing.as_ref().unwrap().key(), key);
    assert_eq!(app.play_order.queued().len(), 1);
    app.reject_unavailable_track(&ctx, key, true);
    assert_eq!(
        app.playing.as_ref().unwrap().key(),
        app.library.tracks[1].key()
    );
    assert!(app.play_order.queued().is_empty());
    assert_eq!(app.adjacent_track(false), Some(app.library.tracks[0].key()));
    assert_eq!(app.queue_origin.as_deref(), Some("playlist:42_9"));
}

#[test]
fn known_restrictions_in_a_long_manual_queue_skip_without_exhausting_network_recovery() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    for index in 0..40 {
        let mut track = external();
        track.id += index;
        app.add_to_queue(&ctx, track);
    }
    for track in app.queued_tracks.values_mut() {
        track.unavailable = Some(true);
    }
    app.advance_track(&ctx, true);
    assert_eq!(
        app.playing.as_ref().unwrap().key(),
        app.library.tracks[1].key()
    );
    assert!(app.play_order.queued().is_empty());
    assert!(!app.unavailable_playback.halted());
    assert_eq!(app.player.decoder_starts(), 0);
}

#[test]
fn queued_track_can_start_without_a_current_track_and_selecting_a_row_skips_without_spawning_each_track()
 {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    app.stop_playback();
    app.selected = None;
    app.add_to_queue(&ctx, external());
    app.toggle_playback(&ctx);
    assert_eq!(app.playing.as_ref().unwrap().key(), external().key());
    assert!(app.play_order.queued().is_empty());
    let mut later = external();
    later.id = 8;
    app.add_to_queue(&ctx, later.clone());
    let mut last = later.clone();
    last.id = 9;
    app.add_to_queue(&ctx, last.clone());
    app.play_queue_position(&ctx, 1);
    assert_eq!(app.playing.as_ref().unwrap().key(), last.key());
    assert!(app.play_order.queued().is_empty());
    assert_eq!(app.adjacent_track(false), Some(external().key()));
}

#[test]
fn drawer_is_on_the_right_above_the_player_and_large_queues_only_render_visible_rows() {
    for language in [Language::Russian, Language::English] {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1280.0, 800.0)] {
            let ctx = egui::Context::default();
            let mut app = prepared(&ctx);
            app.language = language;
            app.page = MusicPage::Library;
            for index in 0..2000 {
                let mut track = external();
                track.id = index + 10;
                track.title = format!("Queued entry {index}");
                app.play_order.enqueue(track.key());
                app.queued_tracks.insert(track.key(), track);
            }
            app.queue_ui.open = true;
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..5 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let title = account_ui_text_rect(&output, language.text("Очередь", "Queue")).unwrap();
            assert!(title.left() > size.x * 0.5 && title.right() < size.x - 8.0);
            assert!(title.top() < 150.0);
            let manual: Vec<_> = account_ui_shapes(&output)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text().starts_with("Queued entry") => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .collect();
            assert!(!manual.is_empty() && manual.len() < 18);
            for rect in &manual {
                assert!(rect.left() >= title.left() - 1.0);
                assert!(rect.right() < size.x - 8.0);
                assert!(rect.bottom() < size.y - 95.0);
            }
            let stamp = app.queue_ui.stamp;
            let remaining = app.play_order.queued().len();
            account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            assert_eq!(app.queue_ui.stamp, stamp);
            assert_eq!(app.play_order.queued().len(), remaining);
            app.queue_ui.open = false;
            for _ in 0..6 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            assert!(account_ui_text_rect(&output, language.text("Очередь", "Queue")).is_none());
        }
    }
}

#[test]
fn the_button_left_of_volume_toggles_the_right_drawer_and_clear_only_removes_manual_entries() {
    for compact in [false, true] {
        let ctx = egui::Context::default();
        let mut app = prepared(&ctx);
        app.page = MusicPage::Library;
        app.compact_seek_bar = compact;
        let track = external();
        app.play_order.enqueue(track.key());
        app.queued_tracks.insert(track.key(), track);
        let size = egui::vec2(800.0, 540.0);
        let mut frame = 0;
        for _ in 0..3 {
            account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let button = egui::pos2(size.x - if compact { 172.0 } else { 192.0 }, size.y - 50.0);
        account_ui_click(&ctx, &mut app, size, &mut frame, button);
        assert!(app.queue_ui.open);
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..4 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let heading = account_ui_text_rect(&output, "Queue").unwrap();
        assert!(heading.left() > 400.0 && heading.top() < 150.0);
        let clear = account_ui_text_rect(&output, "Clear added")
            .unwrap()
            .center();
        account_ui_click(&ctx, &mut app, size, &mut frame, clear);
        assert!(app.play_order.queued().is_empty());
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 3);
        assert_eq!(
            app.playing.as_ref().unwrap().key(),
            app.library.tracks[0].key()
        );
        account_ui_click(&ctx, &mut app, size, &mut frame, button);
        assert!(!app.queue_ui.open);
    }
}
