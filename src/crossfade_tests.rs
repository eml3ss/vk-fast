//! Crossfading into the next track: when it starts, and when it does not.
use super::*;
use crate::app::tests::{page, search_test_app};

/// The first of two queued 200 s tracks plays at `position`, crossfade on.
fn playing_at(ctx: &egui::Context, position: f64) -> MusicApp {
    let mut app = search_test_app(ctx);
    app.demo = false;
    let mut queue = page(2, &[91, 92]).items;
    for track in &mut queue {
        track.duration = 200;
    }
    app.playing = Some(queue[0].clone());
    app.selected = app.playing.as_ref().map(Track::key);
    app.play_queue = Some(queue);
    app.queue_origin = Some("playlist:42_9".into());
    app.synchronize_order();
    let (player, commands) = Player::test_playing(200, position);
    app.player = player;
    // The commands are not inspected; the shared state shows a stop.
    std::mem::forget(commands);
    app.crossfade = true;
    app.crossfade_seconds = 6;
    app
}

/// Without a VK client, resolving finds nothing and asks nothing.
fn finish_resolvers(app: &mut MusicApp) {
    for worker in std::mem::take(&mut app.workers) {
        assert_eq!(worker.thread().name(), Some("vk-resolve-audio"));
        worker.join().unwrap();
    }
}

#[test]
fn the_last_seconds_start_the_next_track_without_stopping_the_playing_one() {
    let ctx = egui::Context::default();
    let mut app = playing_at(&ctx, 195.0);
    let next = app.play_queue.as_ref().unwrap()[1].key();
    app.poll_playback(&ctx);
    assert_eq!(app.playing.as_ref().map(Track::key), Some(next));
    assert!(
        app.playback_pending.is_some(),
        "the next track loads only now"
    );
    assert!(app.crossfading());
    assert_eq!(
        app.player.snapshot().state,
        PlaybackState::Playing,
        "the ending track keeps playing"
    );
    // The next frames start nothing more.
    app.poll_playback(&ctx);
    finish_resolvers(&mut app);
    assert_eq!(app.playing.as_ref().map(Track::key), Some(next));
}

#[test]
fn no_crossfade_before_its_seconds_when_off_on_repeat_one_or_for_short_tracks() {
    let ctx = egui::Context::default();
    let first = |app: &MusicApp| app.play_queue.as_ref().unwrap()[0].key();
    // Too early.
    let mut app = playing_at(&ctx, 193.0);
    app.poll_playback(&ctx);
    assert_eq!(app.playing.as_ref().map(Track::key), Some(first(&app)));
    assert!(app.playback_pending.is_none() && !app.crossfading());
    // Off.
    let mut app = playing_at(&ctx, 197.0);
    app.crossfade = false;
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_none());
    // Repeat one plays the same track again from its end, as before.
    let mut app = playing_at(&ctx, 197.0);
    while app.play_order.repeat() != crate::play_order::RepeatMode::One {
        app.play_order.cycle_repeat();
    }
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_none());
    // A track shorter than two crossfades and a bit just ends.
    let mut app = playing_at(&ctx, 197.0);
    if let Some(track) = &mut app.playing {
        track.duration = 16;
    }
    app.player = Player::test_playing(16, 13.0).0;
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_none());
    // Paused: nothing starts.
    let mut app = playing_at(&ctx, 197.0);
    app.player = Player::test_paused(200, 197.0).0;
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_none());
    assert_eq!(app.playing.as_ref().map(Track::key), Some(first(&app)));
}

#[test]
fn the_crossfade_setting_shows_its_length_only_while_on_and_is_found_by_search() {
    use crate::app::tests::{account_ui_app, account_ui_frame, account_ui_text_rect};
    let ctx = egui::Context::default();
    let mut app = account_ui_app(&ctx, Language::English);
    app.settings_open = true;
    app.settings_query = "crossfade".into();
    let size = egui::vec2(1120.0, 950.0);
    let mut tick = 0;
    let mut output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
    for _ in 0..2 {
        output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
    }
    assert!(account_ui_text_rect(&output, "Crossfade between tracks").is_some());
    assert!(account_ui_text_rect(&output, "Crossfade length: 6 s").is_none());
    app.crossfade = true;
    for _ in 0..2 {
        output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
    }
    assert!(account_ui_text_rect(&output, "Crossfade length: 6 s").is_some());
}

#[test]
fn seeking_into_the_crossfade_seconds_cancels_it_and_seeking_back_restores_it() {
    let ctx = egui::Context::default();
    let mut app = playing_at(&ctx, 100.0);
    let first = app.play_queue.as_ref().unwrap()[0].key();
    // Into the last 6 s: no crossfade, the track plays to its end.
    app.commit_playback_seek(197.0);
    app.player = Player::test_playing(200, 197.0).0;
    app.poll_playback(&ctx);
    assert_eq!(app.playing.as_ref().map(Track::key), Some(first));
    assert!(app.playback_pending.is_none() && !app.crossfading());
    // Back before them: it crossfades again when they come.
    app.commit_playback_seek(150.0);
    app.player = Player::test_playing(200, 195.0).0;
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_some() && app.crossfading());
    finish_resolvers(&mut app);
}

#[test]
fn a_seek_while_the_next_track_loads_ends_the_crossfade() {
    let ctx = egui::Context::default();
    let mut app = playing_at(&ctx, 195.0);
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_some() && app.crossfading());
    app.commit_playback_seek(30.0);
    assert!(
        !app.crossfading(),
        "the next track starts without a crossfade"
    );
    finish_resolvers(&mut app);
}
