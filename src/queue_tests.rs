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

#[test]
fn navigation_and_queue_panels_resize_by_dragging_their_edges_and_stay_open() {
    fn drag(
        ctx: &egui::Context,
        app: &mut MusicApp,
        size: egui::Vec2,
        frame: &mut u32,
        from: egui::Pos2,
        to: egui::Pos2,
    ) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        account_ui_frame(ctx, app, size, frame, vec![egui::Event::PointerMoved(from)]);
        account_ui_frame(ctx, app, size, frame, vec![button(from, true)]);
        for step in 1..=4 {
            let at = from.lerp(to, step as f32 / 4.0);
            account_ui_frame(ctx, app, size, frame, vec![egui::Event::PointerMoved(at)]);
        }
        account_ui_frame(ctx, app, size, frame, vec![button(to, false)]);
        for _ in 0..3 {
            account_ui_frame(ctx, app, size, frame, Vec::new());
        }
    }
    fn queue_left(output: &egui::FullOutput) -> f32 {
        account_ui_text_rect(output, "Queue").unwrap().left()
    }
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    app.language = Language::English;
    app.page = MusicPage::Library;
    app.queue_ui.open = true;
    let size = egui::vec2(1280.0, 800.0);
    let mut frame = 0;
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..5 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    assert_eq!(app.panel_widths(), crate::app::DEFAULT_PANEL_WIDTHS);
    let before = queue_left(&output);

    let edge = size.x - crate::app::DEFAULT_PANEL_WIDTHS[1];
    drag(
        &ctx,
        &mut app,
        size,
        &mut frame,
        egui::pos2(edge, 400.0),
        egui::pos2(edge - 100.0, 400.0),
    );
    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    assert!(
        (app.panel_widths()[1] - 440.0).abs() < 2.0,
        "{:?}",
        app.panel_widths()
    );
    assert!((queue_left(&output) - (before - 100.0)).abs() < 2.0);

    // Dragging past the minimum width only narrows the panel; it stays open.
    let edge = size.x - app.panel_widths()[1];
    drag(
        &ctx,
        &mut app,
        size,
        &mut frame,
        egui::pos2(edge, 400.0),
        egui::pos2(size.x - 20.0, 400.0),
    );
    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    assert!(app.queue_ui.open);
    assert_eq!(app.panel_widths()[1], crate::app::RIGHT_PANEL_WIDTHS.0);
    assert!(account_ui_text_rect(&output, "Queue").is_some());

    let edge = crate::app::DEFAULT_PANEL_WIDTHS[0];
    drag(
        &ctx,
        &mut app,
        size,
        &mut frame,
        egui::pos2(edge, 400.0),
        egui::pos2(edge + 80.0, 400.0),
    );
    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    assert!(
        (app.panel_widths()[0] - 332.0).abs() < 2.0,
        "{:?}",
        app.panel_widths()
    );
    let header = account_ui_shapes(&output)
        .into_iter()
        .find_map(|shape| match shape {
            egui::Shape::Rect(rect)
                if rect.fill == theme::background()
                    && rect.rect.top() == 0.0
                    && rect.rect.width() > 300.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .unwrap();
    assert!((header.left() - app.panel_widths()[0]).abs() < 1.5);

    // A narrow window squeezes the panels without forgetting the chosen widths.
    let narrow = egui::vec2(800.0, 540.0);
    for _ in 0..3 {
        account_ui_frame(&ctx, &mut app, narrow, &mut frame, Vec::new());
    }
    assert!((app.panel_widths()[0] - 332.0).abs() < 2.0);
    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    let header = account_ui_shapes(&output)
        .into_iter()
        .find_map(|shape| match shape {
            egui::Shape::Rect(rect)
                if rect.fill == theme::background()
                    && rect.rect.top() == 0.0
                    && rect.rect.width() > 300.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .unwrap();
    assert!((header.left() - 332.0).abs() < 2.0, "{header:?}");
    let mut restored = prepared(&ctx);
    restored.restore_panel_widths([9999.0, f32::MIN]);
    assert_eq!(
        restored.panel_widths(),
        [
            crate::app::LEFT_PANEL_WIDTHS.1,
            crate::app::RIGHT_PANEL_WIDTHS.0
        ]
    );
}

#[test]
fn added_tracks_drag_into_a_new_order_with_the_source_rows_fixed() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    app.page = MusicPage::Library;
    for index in 0..3 {
        let mut track = external();
        track.id = 20 + index;
        track.title = format!("Added {index}");
        app.play_order.enqueue(track.key());
        app.queued_tracks.insert(track.key(), track);
    }
    let ids: Vec<_> = app
        .play_order
        .queued()
        .iter()
        .map(|entry| entry.id)
        .collect();
    app.queue_ui.open = true;
    let size = egui::vec2(1280.0, 800.0);
    let mut frame = 0;
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..4 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    let first = account_ui_text_rect(&output, "Added 0").unwrap();
    let last = account_ui_text_rect(&output, "Added 2").unwrap();
    let source = account_ui_text_rect(&output, &app.library.tracks[1].title).unwrap();
    assert!(source.top() > last.bottom(), "the source continues below");
    let pointer = |pos, pressed| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    // Press on the first added row and drag it below the last one; a drop
    // past the added tracks stays within them.
    let start = egui::pos2(first.right() + 30.0, first.center().y);
    account_ui_frame(&ctx, &mut app, size, &mut frame, pointer(start, true));
    let mut at = start;
    for _ in 0..12 {
        at.y += 20.0;
        let output = account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::PointerMoved(at)],
        );
        assert!(
            account_ui_text_rect(&output, "Added 0").is_some(),
            "the lifted row stays visible"
        );
    }
    account_ui_frame(&ctx, &mut app, size, &mut frame, pointer(at, false));
    let moved: Vec<_> = app
        .play_order
        .queued()
        .iter()
        .map(|entry| entry.id)
        .collect();
    assert_eq!(moved, [ids[1], ids[2], ids[0]]);
    assert_eq!(
        app.playing.as_ref().unwrap().key(),
        app.library.tracks[0].key(),
        "dragging never plays a row"
    );
    assert_eq!(app.play_queue.as_ref().unwrap().len(), 3);
}

#[test]
fn source_rows_drag_into_a_new_play_order_without_touching_the_list_itself() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    app.page = MusicPage::Library;
    app.queue_ui.open = true;
    let library: Vec<_> = app.library.tracks.iter().map(Track::key).collect();
    let size = egui::vec2(1280.0, 800.0);
    let mut frame = 0;
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..4 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    // The drawer lists the two source tracks after the current one; the
    // last is the lower of the two rows with its title.
    let title = app.library.tracks[2].title.clone();
    let row = account_ui_shapes(&output)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) if text.galley.text() == title => {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        })
        .filter(|rect| rect.left() > size.x * 0.5)
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap();
    let pointer = |pos, pressed| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let start = egui::pos2(row.right() + 30.0, row.center().y);
    account_ui_frame(&ctx, &mut app, size, &mut frame, pointer(start, true));
    let mut at = start;
    for _ in 0..5 {
        at.y -= 14.0;
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::PointerMoved(at)],
        );
    }
    account_ui_frame(&ctx, &mut app, size, &mut frame, pointer(at, false));
    let order: Vec<_> = app
        .play_queue
        .as_ref()
        .unwrap()
        .iter()
        .map(Track::key)
        .collect();
    assert_eq!(order, [library[0], library[2], library[1]]);
    assert_eq!(
        app.play_order.next(library[0], false),
        Some(library[2]),
        "the dragged track plays next"
    );
    let unchanged: Vec<_> = app.library.tracks.iter().map(Track::key).collect();
    assert_eq!(unchanged, library, "My music keeps its own order");
    assert_eq!(app.playing.as_ref().unwrap().key(), library[0]);
}

#[test]
fn a_track_coming_back_after_being_hidden_appears_in_place_instead_of_flying() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    app.library.tracks = page(8, &[1, 2, 3, 4, 5, 6, 7, 8]).items;
    for (index, track) in app.library.tracks.iter_mut().enumerate() {
        track.title = format!("Song {index}");
    }
    app.play_queue = Some(app.library.tracks.clone());
    let play = |app: &mut MusicApp, index: usize| {
        let track = app.library.tracks[index].clone();
        app.selected = Some(track.key());
        app.play_order.select_source(track.key());
        app.playing = Some(track);
        app.synchronize_order();
    };
    play(&mut app, 4);
    app.page = MusicPage::Library;
    app.queue_ui.open = true;
    let size = egui::vec2(1280.0, 800.0);
    let mut frame = 0;
    let row = |output: &egui::FullOutput| {
        account_ui_shapes(output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == "Song 5" => {
                    let rect = text.visual_bounding_rect();
                    (rect.left() > size.x * 0.5).then_some(rect.top())
                }
                _ => None,
            })
    };
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..4 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    let first_place = row(&output).unwrap();
    // Hidden while the queue is closed, Song 5 moves four rows down.
    app.queue_ui.open = false;
    for _ in 0..6 {
        account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    play(&mut app, 0);
    app.queue_ui.open = true;
    let mut places = Vec::new();
    for _ in 0..6 {
        let output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        places.extend(row(&output));
    }
    let settled = *places.last().unwrap();
    assert!(settled > first_place + 150.0);
    assert!(
        places.iter().all(|y| (y - settled).abs() < 0.5),
        "Song 5 flew from its old place: {places:?}"
    );
}

#[test]
fn no_two_rows_share_a_place_after_stepping_back_over_a_queued_playlist_track() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    app.library.tracks = page(6, &[1, 2, 3, 4, 5, 6]).items;
    for (index, track) in app.library.tracks.iter_mut().enumerate() {
        track.title = format!("Song {index}");
    }
    app.play_queue = Some(app.library.tracks.clone());
    app.playing = Some(app.library.tracks[0].clone());
    app.selected = app.playing.as_ref().map(Track::key);
    app.synchronize_order();
    // A track of the playlist is also added to the queue, played, and left
    // with Previous: it is listed once, and no rows share a place.
    let queued = app.library.tracks[3].clone();
    app.add_to_queue(&ctx, queued);
    app.page = MusicPage::Library;
    app.queue_ui.open = true;
    let size = egui::vec2(1280.0, 900.0);
    let mut frame = 0;
    for _ in 0..3 {
        account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    for command in [
        Command::Next,
        Command::Next,
        Command::Previous,
        Command::Previous,
    ] {
        app.handle_system_media_command(&ctx, command);
        for _ in 0..4 {
            account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
    }
    let output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    let mut tops: Vec<_> = account_ui_shapes(&output)
        .into_iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) if text.galley.text().starts_with("Song") => {
                let rect = text.visual_bounding_rect();
                (rect.left() > size.x * 0.5).then_some(rect.top())
            }
            _ => None,
        })
        .collect();
    let upcoming = app
        .play_order
        .upcoming(app.playing.as_ref().map(Track::key), false);
    let mut keys: Vec<_> = upcoming.iter().map(|entry| entry.key).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(
        keys.len(),
        upcoming.len(),
        "a played row is not listed again in the source"
    );
    tops.sort_by(f32::total_cmp);
    assert!(
        tops.windows(2).all(|pair| pair[1] - pair[0] > 30.0),
        "rows overlap: {tops:?}"
    );
}

#[test]
fn a_track_in_both_playlists_does_not_jump_when_playback_moves_to_the_other() {
    let ctx = egui::Context::default();
    let mut app = prepared(&ctx);
    let mut first = page(4, &[1, 2, 3, 4]).items;
    for track in &mut first {
        track.title = format!("Song {}", track.id);
    }
    let shared = first[1].clone();
    let mut second = page(4, &[7, 8, 9, 10]).items;
    for track in &mut second {
        track.title = format!("Song {}", track.id);
    }
    // In the second playlist the shared track sits further down.
    second.insert(3, shared.clone());
    let start = |app: &mut MusicApp, tracks: &[Track], origin: &str| {
        app.play_queue = Some(tracks.to_vec());
        app.queue_origin = Some(origin.into());
        app.playing = Some(tracks[0].clone());
        app.selected = Some(tracks[0].key());
        app.play_order.reset_source(
            &tracks.iter().map(Track::key).collect::<Vec<_>>(),
            Some(tracks[0].key()),
        );
    };
    start(&mut app, &first, "playlist:1_1");
    app.page = MusicPage::Library;
    app.queue_ui.open = true;
    let size = egui::vec2(1280.0, 900.0);
    let mut frame = 0;
    for _ in 0..4 {
        account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    start(&mut app, &second, "playlist:1_2");
    let place = |output: &egui::FullOutput| {
        account_ui_shapes(output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == shared.title => {
                    let rect = text.visual_bounding_rect();
                    (rect.left() > size.x * 0.5).then_some(rect.top())
                }
                _ => None,
            })
            .unwrap()
    };
    let places: Vec<_> = (0..4)
        .map(|_| {
            place(&account_ui_frame(
                &ctx,
                &mut app,
                size,
                &mut frame,
                Vec::new(),
            ))
        })
        .collect();
    assert!(
        places.iter().all(|y| (y - places[3]).abs() < 0.5),
        "the shared track jumped: {places:?}"
    );
}
