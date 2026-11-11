use super::*;
use crate::app::tests::{account_ui_frame, account_ui_text_rect, page, search_test_app};

/// The last of two queued tracks plays, repeat off, the queue-end mix on;
/// the session has no VK client, so nothing here asks VK.
fn last_track_playing(ctx: &egui::Context) -> MusicApp {
    let mut app = search_test_app(ctx);
    app.demo = false;
    app.library.exhausted = true;
    let queue = page(2, &[91, 92]).items;
    app.playing = Some(queue[1].clone());
    app.selected = app.playing.as_ref().map(Track::key);
    app.play_queue = Some(queue);
    app.queue_origin = Some("playlist:42_9".into());
    app.mix_after_queue = true;
    app.synchronize_order();
    app
}

/// Playing a track starts only the local resolver; without a VK client it
/// finds no stream and asks nothing. Waits for it.
fn only_local_resolvers(app: &mut MusicApp) {
    for worker in std::mem::take(&mut app.workers) {
        assert_eq!(worker.thread().name(), Some("vk-resolve-audio"));
        worker.join().unwrap();
    }
}

fn mix_tracks() -> Vec<Track> {
    let mut tracks = page(3, &[201, 202, 203]).items;
    for (track, title) in tracks.iter_mut().zip(["Mix A", "Mix B", "Mix C"]) {
        track.owner_id = 77;
        track.title = title.into();
    }
    tracks
}

#[test]
fn the_queue_end_mix_shows_in_the_queue_and_plays_from_there_at_the_end() {
    let ctx = egui::Context::default();
    let mut app = last_track_playing(&ctx);
    app.queue_ui.open = true;
    let mix = mix_tracks();
    app.set_prepared_mix_for_test(mix.clone());
    let size = egui::vec2(1200.0, 760.0);
    let mut frame = 0;
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..2 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    for title in ["Mix A", "Mix B", "Mix C", "then a track mix"] {
        assert!(
            account_ui_text_rect(&output, title).is_some(),
            "the queue shows {title}"
        );
    }
    // The track ends: the mix starts from the tracks shown, asking VK nothing.
    let last = app.playing.as_ref().unwrap().key();
    app.advance_track(&ctx, true);
    assert_eq!(app.playing.as_ref().unwrap().key(), mix[0].key());
    assert!(app.mix_active);
    assert_eq!(app.mix_seed, Some(MixSeed::Track(last)));
    let queued: Vec<_> = app
        .play_queue
        .as_ref()
        .unwrap()
        .iter()
        .map(Track::key)
        .collect();
    assert_eq!(queued, mix.iter().map(Track::key).collect::<Vec<_>>());
    assert!(app.prepared_mix.is_none() && app.mix_request.is_none());
    only_local_resolvers(&mut app);
}

#[test]
fn a_click_on_a_mix_row_in_the_queue_starts_the_mix_there() {
    let ctx = egui::Context::default();
    let mut app = last_track_playing(&ctx);
    app.queue_ui.open = true;
    let mix = mix_tracks();
    app.set_prepared_mix_for_test(mix.clone());
    let size = egui::vec2(1200.0, 760.0);
    let mut frame = 0;
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    for _ in 0..2 {
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
    }
    let second = account_ui_text_rect(&output, "Mix B").unwrap();
    // Clicked by hand: the shared frame helper takes any worker for a fault.
    for pressed in [true, false] {
        frame += 1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(f64::from(frame) * 0.1),
                events: vec![
                    egui::Event::PointerMoved(second.center()),
                    egui::Event::PointerButton {
                        pos: second.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        output.textures_delta.clear();
    }
    assert_eq!(app.playing.as_ref().unwrap().key(), mix[1].key());
    assert!(app.mix_active && app.prepared_mix.is_none());
    only_local_resolvers(&mut app);
}

#[test]
fn only_the_last_track_without_repeat_and_with_the_setting_gets_a_mix_ahead() {
    let ctx = egui::Context::default();
    let mut app = last_track_playing(&ctx);
    app.set_prepared_mix_for_test(mix_tracks());
    assert_eq!(app.prepared_mix_tracks().len(), 3);
    // Repeat all: the queue starts over, nothing follows it.
    while app.play_order.repeat() != crate::play_order::RepeatMode::All {
        app.play_order.cycle_repeat();
    }
    assert!(app.prepared_mix_tracks().is_empty());
    while app.play_order.repeat() != crate::play_order::RepeatMode::Off {
        app.play_order.cycle_repeat();
    }
    assert_eq!(app.prepared_mix_tracks().len(), 3);
    // Setting off.
    app.mix_after_queue = false;
    assert!(app.prepared_mix_tracks().is_empty());
    app.prepare_queue_end_mix(&ctx);
    assert!(
        app.prepared_mix.is_none(),
        "dropped once it no longer follows"
    );
    // Not the last track: nothing is fetched ahead.
    app.mix_after_queue = true;
    app.playing = app.play_queue.as_ref().unwrap().first().cloned();
    app.selected = app.playing.as_ref().map(Track::key);
    app.prepare_queue_end_mix(&ctx);
    assert!(app.prepared_mix.is_none());
    assert!(app.workers.is_empty());
}
