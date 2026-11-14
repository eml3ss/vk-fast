//! A failed track is tried once more, then skipped only when the queue moved
//! on by itself, and never more than a few times in a row.
use super::*;
use crate::app::tests::{page, search_test_app};

/// Three queued tracks, the first just started; resolving asks nothing.
fn queued(ctx: &egui::Context, automatic: bool) -> (MusicApp, Vec<(i64, i64)>) {
    let mut app = search_test_app(ctx);
    app.demo = false;
    let queue = page(3, &[91, 92, 93]).items;
    let keys: Vec<_> = queue.iter().map(Track::key).collect();
    app.play_queue = Some(queue);
    app.queue_origin = Some("playlist:42_9".into());
    app.synchronize_order();
    app.playback_retry.advancing(automatic);
    app.play_track(ctx, keys[0]);
    app.playback_retry.advancing(false);
    (app, keys)
}

/// The track's resolver finishes, and then its playback fails.
fn fail(app: &mut MusicApp) {
    for worker in std::mem::take(&mut app.workers) {
        worker.join().unwrap();
    }
    let (player, commands) = Player::test_failed(200, 0.0);
    std::mem::forget(commands);
    app.player = player;
}

/// The retry's moment comes, as the scheduled wake would bring it.
fn retry_now(app: &mut MusicApp) {
    let due = app
        .playback_retry
        .due
        .as_mut()
        .expect("a retry is scheduled");
    due.2 = Instant::now();
}

fn playing(app: &MusicApp) -> Option<(i64, i64)> {
    app.playing.as_ref().map(Track::key)
}

#[test]
fn a_failed_track_is_tried_again_with_a_fresh_link_after_a_pause() {
    let ctx = egui::Context::default();
    let (mut app, keys) = queued(&ctx, false);
    fail(&mut app);
    app.poll_playback(&ctx);
    assert!(
        app.playback_pending.is_none(),
        "not at once: the server needs a moment"
    );
    assert!(app.playback_retry.due.is_some());
    app.poll_playback(&ctx);
    assert!(app.playback_pending.is_none());

    retry_now(&mut app);
    app.poll_playback(&ctx);
    assert_eq!(playing(&app), Some(keys[0]));
    assert!(
        app.playback_pending.is_some(),
        "the same track resolves again"
    );
    assert!(app.playback_retry.retried);

    // A track the user chose stays put after its second failure.
    fail(&mut app);
    app.poll_playback(&ctx);
    app.poll_playback(&ctx);
    assert_eq!(playing(&app), Some(keys[0]));
    assert!(app.playback_pending.is_none());
    assert!(app.playback_retry.due.is_none());
}

#[test]
fn an_automatic_advance_moves_on_past_a_track_that_fails_twice_but_not_forever() {
    let ctx = egui::Context::default();
    let (mut app, keys) = queued(&ctx, true);
    fail(&mut app);
    app.poll_playback(&ctx);
    retry_now(&mut app);
    app.poll_playback(&ctx);
    fail(&mut app);
    app.poll_playback(&ctx);
    assert_eq!(
        playing(&app),
        Some(keys[1]),
        "the next track starts by itself"
    );
    assert_eq!(app.playback_retry.skipped, 1);
    assert!(app.playback_retry.automatic);

    // Once a few in a row have failed, the network is likely down: stop.
    app.playback_retry.skipped = MAX_SKIPS;
    fail(&mut app);
    app.poll_playback(&ctx);
    retry_now(&mut app);
    app.poll_playback(&ctx);
    fail(&mut app);
    app.poll_playback(&ctx);
    assert_eq!(playing(&app), Some(keys[1]));

    // A track that plays starts the count again.
    let (player, commands) = Player::test_playing(200, 1.0);
    std::mem::forget(commands);
    app.player = player;
    app.poll_playback(&ctx);
    assert_eq!(app.playback_retry.skipped, 0);
}
