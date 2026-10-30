use super::*;
use crate::app::tests::{page, search_test_app};

fn rows() -> Vec<Track> {
    let mut tracks = page(3, &[1, 2, 3]).items;
    tracks[1].unavailable = Some(true);
    tracks
}

#[test]
fn playback_entry_skips_a_known_restriction_before_resolving_it_and_clears_its_seek() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.demo = false;
    let tracks = rows();
    app.library.tracks = tracks.clone();
    app.library.exhausted = true;
    app.play_queue = Some(tracks.clone());
    app.queue_origin = Some("library".into());
    app.play_track_at(&ctx, tracks[1].key(), 73.0);
    assert_eq!(app.playing.as_ref().unwrap().key(), tracks[2].key());
    assert_eq!(app.resume_position, Some(0.0));
    assert!(app.playback_pending.is_some() && app.playback_error.is_none());
    assert_eq!(app.player.decoder_starts(), 0);
}

#[test]
fn an_unresolved_stream_does_not_mark_the_library_or_scan_other_tracks() {
    for permanent in [false, true] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        let tracks = page(3, &[1, 2, 3]).items;
        app.library.tracks = tracks.clone();
        app.library.exhausted = true;
        app.play_queue = Some(tracks.clone());
        app.playing = Some(tracks[1].clone());
        app.resume_position = Some(73.0);
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Err(UiError::Music(if permanent {
                WebAudioError::NoStream
            } else {
                WebAudioError::Network
            })))
            .ok()
            .unwrap();
        app.playback_pending = Some(receiver);
        app.poll_playback(&ctx);
        assert_eq!(app.library.tracks[1].unavailable, None);
        assert_eq!(app.playing.as_ref().unwrap().key(), tracks[1].key());
        assert_eq!(app.resume_position, Some(73.0));
        assert!(app.playback_pending.is_none() && app.workers.is_empty());
        if permanent {
            assert!(matches!(
                app.playback_error,
                Some(UiError::Music(WebAudioError::NoStream))
            ));
        } else {
            assert!(matches!(
                app.playback_error,
                Some(UiError::Music(WebAudioError::Network))
            ));
        }
        assert_eq!(app.player.decoder_starts(), 0);
    }
}

#[test]
fn manual_previous_and_next_skip_known_restrictions_including_a_stopped_failed_anchor() {
    for (current, next, expected) in [(2, false, 0), (1, false, 0), (1, true, 2), (0, true, 2)] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        let tracks = rows();
        app.library.tracks = tracks.clone();
        app.library.exhausted = true;
        app.play_queue = Some(tracks.clone());
        app.playing = Some(tracks[current].clone());
        if next {
            app.next_track(&ctx);
        } else {
            app.previous_track(&ctx, false);
        }
        assert_eq!(app.playing.as_ref().unwrap().key(), tracks[expected].key());
        assert_eq!(app.player.decoder_starts(), 0);
    }
}

#[test]
fn candidate_keeps_the_failed_position_and_bypasses_loop_one_without_changing_the_queue() {
    for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
        let tracks = rows();
        let anchor = tracks[1].key();
        let mut order = PlaybackOrder::default();
        while order.repeat() != repeat {
            order.cycle_repeat();
        }
        let rejected = HashSet::from([anchor]);
        assert_eq!(
            next_candidate(&mut order, &tracks, anchor, true, &rejected),
            Some(tracks[2].key())
        );
        assert_eq!(
            tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(order.repeat(), repeat);
    }
}

#[test]
fn all_bad_empty_and_shuffle_sources_finish_without_retrying_the_anchor() {
    let mut tracks = rows();
    for track in &mut tracks {
        track.unavailable = Some(true);
    }
    let anchor = tracks[1].key();
    for shuffle in [false, true] {
        let mut order = PlaybackOrder::default();
        order.cycle_repeat();
        order.set_shuffle(
            shuffle,
            &tracks.iter().map(Track::key).collect::<Vec<_>>(),
            None,
        );
        assert_eq!(
            next_candidate(&mut order, &tracks, anchor, true, &HashSet::from([anchor])),
            None
        );
        assert_eq!(order.shuffle(), shuffle);
    }
    assert_eq!(
        next_candidate(
            &mut PlaybackOrder::default(),
            &[],
            anchor,
            true,
            &HashSet::new()
        ),
        None
    );
    let tracks = rows();
    let mut order = PlaybackOrder::default();
    order.set_shuffle(
        true,
        &tracks.iter().map(Track::key).collect::<Vec<_>>(),
        Some(anchor),
    );
    assert!(
        matches!(next_candidate(&mut order, &tracks, anchor, true, &HashSet::from([anchor])), Some(key) if key == tracks[0].key() || key == tracks[2].key())
    );
}

#[test]
fn recovery_budget_counts_unique_failures_and_empty_page_continuations_and_can_be_reset() {
    let mut state = State::default();
    assert!(state.allow_continuation()); // Ordinary pagination has no recovery budget.
    assert!(!state.active());
    for id in 1..=MAX_RECOVERY_STEPS as i64 {
        assert!(state.reject((1, id)));
    }
    assert!(!state.reject((1, 99)));
    assert!(state.halted());
    assert_eq!(state.rejected.len(), MAX_RECOVERY_STEPS);
    state.clear();
    assert!(!state.active() && !state.halted());
    assert!(state.reject((1, 1)));
    for _ in 1..MAX_RECOVERY_STEPS {
        assert!(state.allow_continuation());
    }
    assert!(!state.allow_continuation());
    state.clear();
    assert!(state.reject((1, 1)));
    assert!(
        !state.reject((1, 1)),
        "One failed identity must not be retried in a loop"
    );
}

#[test]
fn clicking_a_restricted_middle_track_selects_the_next_track_and_drops_the_old_seek() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.demo = false;
    let tracks = rows();
    let bad = tracks[1].key();
    let next = tracks[2].key();
    app.library.tracks = tracks.clone();
    app.library.exhausted = true;
    app.play_queue = Some(tracks.clone());
    app.queue_origin = Some("playlist:42_5".into());
    app.playing = Some(tracks[0].clone());
    app.resume_position = Some(73.0);
    app.reject_unavailable_track(&ctx, bad, false);
    assert_eq!(app.playing.as_ref().unwrap().key(), next);
    assert_eq!(app.selected, Some(next));
    assert_eq!(app.resume_position, Some(0.0));
    assert_eq!(app.queue_origin.as_deref(), Some("playlist:42_5"));
    assert_eq!(
        app.play_queue
            .as_ref()
            .unwrap()
            .iter()
            .map(Track::key)
            .collect::<Vec<_>>(),
        tracks.iter().map(Track::key).collect::<Vec<_>>()
    );
    assert!(app.playback_error.is_none());
    // Resolution can run, but this fixture has no music client or stream URL:
    // it cannot access VK, launch FFmpeg or open an audio device.
    assert!(app.playback_pending.is_some());
}

#[test]
fn learned_restrictions_update_exact_aliases_and_schedule_cache_updates_without_title_guesses() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let mut source = page(1, &[9]).items.remove(0);
    source.owner_id = -7;
    let mut owned = source.clone();
    owned.owner_id = 42;
    owned.id = 99;
    let mut alias = source.clone();
    alias.owner_id = -8;
    alias.id = 10;
    let mut unrelated = source.clone();
    unrelated.id = 11;
    app.like_aliases.insert(source.key(), owned.key());
    app.like_aliases.insert(alias.key(), owned.key());
    app.library.tracks = vec![owned.clone(), unrelated.clone()];
    app.library.exhausted = true;
    app.recent = vec![alias];
    app.play_queue = Some(vec![source.clone()]);
    app.queue_origin = Some("search:exact".into());
    app.playing = Some(source.clone());
    app.resume_position = Some(67.0);
    app.reject_unavailable_track(&ctx, source.key(), true);
    assert_eq!(app.library.tracks[0].unavailable, Some(true));
    assert_eq!(app.library.tracks[1].unavailable, None);
    assert_eq!(app.recent[0].unavailable, Some(true));
    assert_eq!(app.play_queue.as_ref().unwrap()[0].unavailable, Some(true));
    assert_eq!(app.playing.as_ref().unwrap().unavailable, Some(true));
    assert!(app.library_save_pending.is_some() && app.recent_save_pending.is_some());
    assert!(app.unavailable_playback.halted());
    assert!(app.playback_pending.is_none() && app.playback_error.is_none());
    assert!(app.resume_position.is_none() && app.seek_position.is_none());
    assert!(app.finished_handled);
    assert_eq!(app.queue_origin.as_deref(), Some("search:exact"));
    assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), source.key());
}

#[test]
fn all_restricted_app_queue_stops_with_repeat_and_shuffle_and_preserves_the_source() {
    for shuffle in [false, true] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let mut tracks = rows();
        for track in &mut tracks {
            track.unavailable = Some(true);
        }
        app.library.tracks = tracks.clone();
        app.library.exhausted = true;
        app.play_queue = Some(tracks.clone());
        app.queue_origin = Some("mix".into());
        app.playing = Some(tracks[0].clone());
        app.play_order.cycle_repeat();
        app.play_order.set_shuffle(
            shuffle,
            &tracks.iter().map(Track::key).collect::<Vec<_>>(),
            None,
        );
        app.reject_unavailable_track(&ctx, tracks[0].key(), false);
        assert!(app.unavailable_playback.halted());
        assert!(app.playback_pending.is_none() && app.playback_error.is_none());
        assert!(app.workers.is_empty());
        assert_eq!(app.play_order.repeat(), RepeatMode::All);
        assert_eq!(app.play_order.shuffle(), shuffle);
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 3);
        assert_eq!(app.queue_origin.as_deref(), Some("mix"));
    }
}

#[test]
fn continuation_reuses_the_failed_anchor_after_new_metadata_arrives() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let mut tracks = rows();
    tracks[0].unavailable = Some(true);
    app.playing = Some(tracks[0].clone());
    app.selected = Some(tracks[0].key());
    app.play_queue = Some(vec![tracks[0].clone(), tracks[1].clone()]);
    app.library.exhausted = true;
    assert!(app.unavailable_playback.reject(tracks[0].key()));
    // Simulate metadata arrival and the parent's filtered-order sync.
    app.play_queue.as_mut().unwrap().push(tracks[2].clone());
    app.play_order
        .sync(&[tracks[2].key()], Some(tracks[0].key()));
    app.continue_unavailable_recovery(&ctx);
    assert!(!app.play_order.shuffle());
    assert_eq!(
        app.play_order.next(tracks[0].key(), false),
        Some(tracks[2].key())
    );
    assert!(!app.unavailable_playback.halted());
    assert_eq!(app.unavailable_playback.rejected.len(), 1);
}

#[test]
fn an_older_resolver_failure_cannot_replace_a_newer_source_selection() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let tracks = rows();
    app.playing = Some(tracks[0].clone());
    app.play_queue = Some(tracks.clone());
    app.queue_origin = Some("old-source".into());
    app.home_autoplay = Some("new-collection".into());
    app.reject_unavailable_track(&ctx, tracks[0].key(), true);
    assert_eq!(app.home_autoplay.as_deref(), Some("new-collection"));
    assert_eq!(app.playing.as_ref().unwrap().key(), tracks[0].key());
    assert_eq!(app.playing.as_ref().unwrap().unavailable, Some(true));
    assert!(!app.unavailable_playback.active());
    assert!(app.playback_pending.is_none());
    assert_eq!(app.queue_origin.as_deref(), Some("old-source"));
}
