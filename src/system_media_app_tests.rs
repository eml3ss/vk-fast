use super::*;
use crate::app::tests::{page, search_test_app};
use crate::model::ArtistRef;

fn app_with_track(ctx: &egui::Context) -> MusicApp {
    let mut app = search_test_app(ctx);
    app.demo = false;
    let mut tracks = page(3, &[1, 2, 3]).items;
    for track in &mut tracks {
        track.duration = 200;
    }
    app.library.tracks = tracks.clone();
    app.library.exhausted = true;
    app.play_queue = Some(tracks.clone());
    app.queue_origin = Some("playlist:42_9".into());
    app.playing = Some(tracks[0].clone());
    app.selected = Some(tracks[0].key());
    app.resume_position = Some(42.75);
    app
}

#[test]
fn restored_metadata_uses_logical_time_versions_and_account_bound_opaque_ids_without_credentials() {
    let ctx = egui::Context::default();
    let mut app = app_with_track(&ctx);
    let track = app.playing.as_mut().unwrap();
    track.title = "Song".into();
    track.subtitle = Some("Slowed".into());
    track.artist = "Display fallback".into();
    track.artists = vec![
        ArtistRef {
            id: "one".into(),
            name: "First, Artist".into(),
        },
        ArtistRef {
            id: "two".into(),
            name: "Second & Artist".into(),
        },
    ];
    track.url = Some("https://audio.invalid/stream?access_token=private-stream".into());
    track.cover_url = Some("https://sun9-1.userapi.com/example/cover.jpg".into());
    track.album = Some(AlbumRef {
        id: 9,
        owner_id: -5,
        title: "Release".into(),
        access_key: Some("private-album-key".into()),
        cover_url: None,
    });
    let expected_title = track.display_title().into_owned();
    let snapshot = app.system_media_snapshot();
    assert_eq!(snapshot.status, Status::Paused);
    assert_eq!(snapshot.position, Duration::from_secs_f64(42.75));
    let metadata = snapshot.track.unwrap();
    assert_eq!(metadata.title, expected_title);
    assert_eq!(metadata.artist, "First, Artist, Second & Artist");
    assert_eq!(metadata.album.as_deref(), Some("Release"));
    assert_eq!(metadata.duration, Some(Duration::from_secs(200)));
    assert_eq!(
        metadata.cover_url.as_deref(),
        Some("https://sun9-1.userapi.com/example/cover.jpg")
    );
    assert!(metadata.id.starts_with('/'));
    assert!(metadata.id[1..].split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }));
    assert_eq!(app.system_media_snapshot().track.unwrap().id, metadata.id);
    let exposed = format!("{metadata:?}");
    for secret in [
        "private-stream",
        "private-album-key",
        "access_token",
        "audio.invalid",
    ] {
        assert!(!exposed.contains(secret));
    }
    app.session.as_mut().unwrap().profile.id = 43;
    assert_ne!(app.system_media_snapshot().track.unwrap().id, metadata.id);
    app.playing.as_mut().unwrap().cover_url = Some("file:///private/profile/image.jpg".into());
    assert!(
        app.system_media_snapshot()
            .track
            .unwrap()
            .cover_url
            .is_none()
    );
    assert!(app.workers.is_empty() && app.playback_pending.is_none());
    assert!(app.system_media.service.is_none() && !app.system_media.attempted);
}

#[test]
fn media_play_does_not_toggle_a_resolver_and_pause_cancels_it_into_a_resumable_logical_pause() {
    let ctx = egui::Context::default();
    let mut app = app_with_track(&ctx);
    let (sender, receiver) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    app.playback_pending = Some(receiver);
    app.playback_cancel = Some(cancel.clone());
    app.resume_position = Some(73.5);
    app.home_autoplay = Some("old-collection".into());
    app.advance_on_load = true;
    assert_eq!(app.system_media_snapshot().status, Status::Playing);
    app.handle_system_media_command(&ctx, Command::Play);
    assert!(app.playback_pending.is_some() && !cancel.load(Ordering::Acquire));
    app.handle_system_media_command(&ctx, Command::Pause);
    assert!(cancel.load(Ordering::Acquire));
    assert!(app.playback_pending.is_none() && app.playback_cancel.is_none());
    assert!(
        sender.send(Err(UiError::NoStream)).is_err(),
        "The late resolver reply must be discarded"
    );
    let paused = app.system_media_snapshot();
    assert_eq!(paused.status, Status::Paused);
    assert_eq!(paused.position, Duration::from_secs_f64(73.5));
    assert!(app.home_autoplay.is_none() && !app.advance_on_load);
    app.handle_system_media_command(&ctx, Command::Pause);
    assert_eq!(app.system_media_snapshot(), paused);
    assert!(app.workers.is_empty() && app.player.decoder_starts() == 0);
}

#[test]
fn seeks_use_committed_resume_time_and_ignore_stale_track_ids_invalid_values_and_out_of_range_positions()
 {
    let ctx = egui::Context::default();
    let mut app = app_with_track(&ctx);
    let id = app.system_media_snapshot().track.unwrap().id;
    app.handle_system_media_command(
        &ctx,
        Command::SetPosition {
            position: Duration::from_secs(90),
            track_id: Some("/old/track".into()),
        },
    );
    assert_eq!(app.resume_position, Some(42.75));
    app.handle_system_media_command(
        &ctx,
        Command::SetPosition {
            position: Duration::from_secs(90),
            track_id: Some(id.clone()),
        },
    );
    assert_eq!(app.resume_position, Some(90.0));
    app.handle_system_media_command(
        &ctx,
        Command::SetPosition {
            position: Duration::from_secs(201),
            track_id: Some(id),
        },
    );
    assert_eq!(app.resume_position, Some(90.0));
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        app.handle_system_media_command(&ctx, Command::SeekBy(bad));
    }
    assert_eq!(app.resume_position, Some(90.0));
    app.seek_position = Some(150.0);
    app.handle_system_media_command(&ctx, Command::SeekBy(-5.0));
    assert_eq!(app.resume_position, Some(85.0));
    assert!(app.seek_position.is_none());
    app.handle_system_media_command(&ctx, Command::SeekBy(-1000.0));
    assert_eq!(app.resume_position, Some(0.0));
    assert_eq!(app.system_media_snapshot().status, Status::Paused);
    app.playing.as_mut().unwrap().unavailable = Some(true);
    app.handle_system_media_command(&ctx, Command::SeekBy(10.0));
    assert_eq!(app.resume_position, Some(0.0));
    assert!(app.workers.is_empty() && app.player.decoder_starts() == 0);
}

#[test]
fn stop_keeps_track_and_source_but_cancels_pending_work_and_resets_system_time() {
    let ctx = egui::Context::default();
    let mut app = app_with_track(&ctx);
    let track = app.playing.as_ref().unwrap().key();
    let queue: Vec<_> = app
        .play_queue
        .as_ref()
        .unwrap()
        .iter()
        .map(Track::key)
        .collect();
    let (sender, receiver) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    app.playback_pending = Some(receiver);
    app.playback_cancel = Some(cancel.clone());
    app.seek_position = Some(123.0);
    app.handle_system_media_command(&ctx, Command::Stop);
    assert!(cancel.load(Ordering::Acquire));
    assert!(sender.send(Err(UiError::NoStream)).is_err());
    assert_eq!(app.playing.as_ref().unwrap().key(), track);
    assert_eq!(
        app.play_queue
            .as_ref()
            .unwrap()
            .iter()
            .map(Track::key)
            .collect::<Vec<_>>(),
        queue
    );
    assert_eq!(app.queue_origin.as_deref(), Some("playlist:42_9"));
    let stopped = app.system_media_snapshot();
    assert_eq!(stopped.status, Status::Stopped);
    assert_eq!(stopped.position, Duration::ZERO);
    assert!(stopped.track.is_some());
    assert!(app.resume_position.is_none() && app.seek_position.is_none() && app.finished_handled);
    app.handle_system_media_command(&ctx, Command::Stop);
    assert_eq!(app.system_media_snapshot(), stopped);
    assert!(app.workers.is_empty());
}

#[test]
fn system_and_tray_previous_restart_playing_and_restored_tracks_without_needing_history() {
    for via_tray in [false, true] {
        for paused in [false, true] {
            for current in [0, 1] {
                let ctx = egui::Context::default();
                let mut app = app_with_track(&ctx);
                let keys: Vec<_> = app
                    .play_queue
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(Track::key)
                    .collect();
                app.playing = Some(app.library.tracks[current].clone());
                app.selected = Some(keys[current]);
                let (player, _commands) = Player::test_playing(200, 42.75);
                if !paused {
                    app.player = player;
                    app.resume_position = None;
                }
                app.seek_position = Some(100.0);
                app.finished_handled = true;
                app.home_autoplay = Some("pending-collection".into());
                if via_tray {
                    app.handle_tray_command(&ctx, crate::system_tray::Command::Previous);
                } else {
                    app.handle_system_media_command(&ctx, Command::Previous);
                }
                assert_eq!(app.playing.as_ref().unwrap().key(), keys[current]);
                assert_eq!(app.selected, Some(keys[current]));
                let snapshot = app.playback_snapshot();
                assert_eq!(snapshot.position, 0.0);
                assert_eq!(
                    snapshot.state,
                    if paused {
                        PlaybackState::Paused
                    } else {
                        PlaybackState::Playing
                    }
                );
                assert_eq!(
                    app.play_queue
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(Track::key)
                        .collect::<Vec<_>>(),
                    keys
                );
                assert!(app.seek_position.is_none() && app.home_autoplay.is_none());
                assert!(!app.finished_handled);
                assert!(app.workers.is_empty() && app.playback_pending.is_none());
                assert_eq!(app.player.decoder_starts(), 0);
            }
        }
    }
}

#[test]
fn system_and_tray_previous_before_threshold_move_back_and_preserve_pause() {
    for via_tray in [false, true] {
        for position in [0.0, 2.999, 3.0] {
            let ctx = egui::Context::default();
            let mut app = app_with_track(&ctx);
            let previous = app.library.tracks[0].key();
            app.playing = Some(app.library.tracks[1].clone());
            app.selected = app.playing.as_ref().map(Track::key);
            app.resume_position = Some(position);
            if via_tray {
                app.handle_tray_command(&ctx, crate::system_tray::Command::Previous);
            } else {
                app.handle_system_media_command(&ctx, Command::Previous);
            }
            assert_eq!(app.playing.as_ref().unwrap().key(), previous);
            assert_eq!(app.selected, Some(previous));
            assert_eq!(app.resume_position, Some(0.0));
            assert_eq!(app.playback_snapshot().state, PlaybackState::Paused);
            assert_eq!(app.queue_origin.as_deref(), Some("playlist:42_9"));
            assert!(app.workers.is_empty() && app.playback_pending.is_none());
            assert_eq!(app.player.decoder_starts(), 0);
        }
    }
}

#[test]
fn paused_and_stopped_navigation_skip_restricted_rows_without_starting_audio_or_replacing_the_source()
 {
    let ctx = egui::Context::default();
    let mut app = app_with_track(&ctx);
    app.play_queue.as_mut().unwrap()[1].unavailable = Some(true);
    let keys: Vec<_> = app
        .play_queue
        .as_ref()
        .unwrap()
        .iter()
        .map(Track::key)
        .collect();
    app.handle_system_media_command(&ctx, Command::Next);
    assert_eq!(app.playing.as_ref().unwrap().key(), keys[2]);
    assert_eq!(app.system_media_snapshot().status, Status::Paused);
    assert_eq!(app.resume_position, Some(0.0));
    app.handle_system_media_command(&ctx, Command::Previous);
    assert_eq!(app.playing.as_ref().unwrap().key(), keys[0]);
    assert_eq!(app.system_media_snapshot().status, Status::Paused);
    app.handle_system_media_command(&ctx, Command::Stop);
    app.handle_system_media_command(&ctx, Command::Next);
    assert_eq!(app.playing.as_ref().unwrap().key(), keys[2]);
    assert_eq!(app.system_media_snapshot().status, Status::Stopped);
    assert_eq!(app.system_media_snapshot().position, Duration::ZERO);
    assert_eq!(app.queue_origin.as_deref(), Some("playlist:42_9"));
    assert_eq!(
        app.play_queue
            .as_ref()
            .unwrap()
            .iter()
            .map(Track::key)
            .collect::<Vec<_>>(),
        keys
    );
    assert!(
        app.workers.is_empty()
            && app.playback_pending.is_none()
            && app.player.decoder_starts() == 0
    );
}

#[test]
fn system_commands_work_while_unfocused_and_modal_and_volume_is_bounded_and_saved() {
    let ctx = egui::Context::default();
    let mut app = app_with_track(&ctx);
    app.qr_open = true;
    app.settings_open = true;
    let mut output = ctx.run_ui(
        egui::RawInput {
            focused: false,
            ..Default::default()
        },
        |ui| {
            let mut text = String::from("Editing");
            ui.text_edit_singleline(&mut text).request_focus();
            app.handle_system_media_command(&ctx, Command::SeekBy(5.0));
            app.handle_system_media_command(&ctx, Command::SetVolume(1.5));
            app.handle_system_media_command(&ctx, Command::Raise);
            app.handle_system_media_command(&ctx, Command::Quit);
        },
    );
    assert_eq!(app.resume_position, Some(47.75));
    assert_eq!(app.player.snapshot().volume, 1.0);
    assert!(app.volume_save_at.is_some());
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(commands.contains(&egui::ViewportCommand::Visible(true)));
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
    assert!(commands.contains(&egui::ViewportCommand::Focus));
    assert!(commands.contains(&egui::ViewportCommand::Close));
    let deadline = app.volume_save_at;
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        app.handle_system_media_command(&ctx, Command::SetVolume(invalid));
    }
    assert_eq!(app.player.snapshot().volume, 1.0);
    assert_eq!(app.volume_save_at, deadline);
    app.handle_system_media_command(&ctx, Command::SetVolume(-0.5));
    assert_eq!(app.player.snapshot().volume, 0.0);
    app.volume_save_at = None;
    assert!(app.workers.is_empty());
    output.textures_delta.clear();
}
