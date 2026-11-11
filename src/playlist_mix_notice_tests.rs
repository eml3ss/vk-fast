use super::*;
use crate::app::tests::{page, search_test_app};
use crate::web_audio::MixPage;

fn reply(
    owner: i64,
    start: bool,
    seed: Option<MixSeed>,
    result: Result<MixPage, WebAudioError>,
) -> MixRequest {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(result).ok().unwrap();
    MixRequest {
        owner,
        start,
        seed,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    }
}

fn frame(
    ctx: &egui::Context,
    app: &mut MusicApp,
    tick: &mut u32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    *tick += 1;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1040.0, 720.0),
            )),
            time: Some(f64::from(*tick) * 0.1),
            events,
            ..Default::default()
        },
        |ui| app.render(ui),
    );
    output.textures_delta.clear();
    assert!(app.workers.is_empty(), "Notice UI started a background job");
    output
}

fn text_rect(output: &egui::FullOutput, caption: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, caption: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == caption => {
                Some(text.visual_bounding_rect())
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, caption)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, caption))
}

#[test]
fn failed_or_empty_playlist_mix_start_uses_a_three_second_notice_and_keeps_current_music() {
    for result in [
        Err(WebAudioError::Network),
        Err(WebAudioError::Format),
        Err(WebAudioError::Rejected),
        Ok(MixPage {
            tracks: Vec::new(),
            has_more: false,
        }),
    ] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let playing = page(1, &[91]).items.remove(0);
        app.playing = Some(playing.clone());
        app.play_queue = Some(vec![playing.clone()]);
        app.queue_origin = Some("track-mix:-8_91".into());
        app.mix_active = true;
        app.mix_seed = Some(MixSeed::Track((-8, 91)));
        app.mix_has_more = true;
        app.mix_request = Some(reply(42, true, Some(MixSeed::Playlist((42, 5))), result));
        let before = Instant::now();
        app.poll_mix(&ctx);
        let after = Instant::now();
        let until = app
            .playlist_controls
            .mix_notice
            .expect("Playlist mix notice");
        assert!(until >= before + Duration::from_secs(3));
        assert!(until <= after + Duration::from_secs(3));
        assert!(app.track_menu_error.is_none() && app.mix_error.is_none());
        assert_eq!(app.mix_seed, Some(MixSeed::Track((-8, 91))));
        assert!(app.mix_active && app.mix_has_more);
        assert_eq!(app.queue_origin.as_deref(), Some("track-mix:-8_91"));
        assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
        assert!(app.mix_request.is_none() && app.workers.is_empty());
    }
}

#[test]
fn playlist_continuation_errors_stop_retries_and_show_only_the_simple_notice_on_home() {
    for repeated_batch in [false, true] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.page = MusicPage::Home;
        app.home_requested = true;
        app.mix_active = true;
        app.mix_seed = Some(MixSeed::Playlist((42, 5)));
        app.mix_has_more = true;
        app.play_queue = Some(page(2, &[91, 92]).items);
        app.playing = app.play_queue.as_ref().unwrap().first().cloned();
        let result = if repeated_batch {
            Ok(MixPage {
                tracks: page(2, &[91, 92]).items,
                has_more: true,
            })
        } else {
            Err(WebAudioError::Format)
        };
        app.mix_request = Some(reply(42, false, app.mix_seed, result));
        app.poll_mix(&ctx);
        let expected = if repeated_batch {
            WebAudioError::Pagination
        } else {
            WebAudioError::Format
        };
        assert_eq!(app.mix_error.as_ref(), Some(&expected));
        assert!(app.track_menu_error.is_none());
        assert!(app.playlist_controls.mix_notice.is_some());
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 2);
        let mut tick = 0;
        frame(&ctx, &mut app, &mut tick, Vec::new());
        let output = frame(&ctx, &mut app, &mut tick, Vec::new());
        assert!(text_rect(&output, "Playlist mix not found").is_some());
        assert!(text_rect(&output, "Cancel").is_some());
        assert!(text_rect(&output, &expected.localized(Language::English)).is_none());
        assert!(text_rect(&output, "Could not complete the action").is_none());
    }
}

#[test]
fn notice_cancel_is_localized_and_rendering_does_not_extend_its_deadline_or_delay_expiry() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.language = language;
        // A long synthetic deadline makes hit-testing independent of CI load;
        // the real three-second duration is checked in the request test above.
        let until = Instant::now() + Duration::from_secs(60);
        app.playlist_controls.mix_notice = Some(until);
        let mut tick = 0;
        frame(&ctx, &mut app, &mut tick, Vec::new());
        let output = frame(&ctx, &mut app, &mut tick, Vec::new());
        let caption = language.text("Микс по плейлисту не найден", "Playlist mix not found");
        assert!(text_rect(&output, caption).is_some());
        assert_eq!(app.playlist_controls.mix_notice, Some(until));
        let cancel = text_rect(&output, language.text("Отмена", "Cancel"))
            .unwrap()
            .center();
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut app,
                &mut tick,
                vec![
                    egui::Event::PointerMoved(cancel),
                    egui::Event::PointerButton {
                        pos: cancel,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        // Cancel fades the notice out like it faded in, then hides it.
        let fading = app.playlist_controls.mix_notice.unwrap();
        assert!(fading < until);
        assert!(
            fading <= Instant::now() + Duration::from_secs_f32(ctx.global_style().animation_time)
        );
        std::thread::sleep(fading.saturating_duration_since(Instant::now()));
        let output = frame(&ctx, &mut app, &mut tick, Vec::new());
        assert!(app.playlist_controls.mix_notice.is_none());
        assert!(text_rect(&output, caption).is_none());
        app.playlist_controls.mix_notice = Some(Instant::now() - Duration::from_millis(1));
        let output = frame(&ctx, &mut app, &mut tick, Vec::new());
        assert!(app.playlist_controls.mix_notice.is_none());
        assert!(text_rect(&output, caption).is_none());
        app.playlist_mix_unavailable(&ctx);
        app.reset();
        assert!(app.playlist_controls.mix_notice.is_none());
    }
}

#[test]
fn stale_or_canceled_mix_replies_are_silent_and_track_mix_failures_keep_their_existing_feedback() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    for (owner, canceled, error) in [
        (43, false, WebAudioError::Network),
        (42, true, WebAudioError::Network),
        (42, false, WebAudioError::Canceled),
    ] {
        let request = reply(owner, true, Some(MixSeed::Playlist((42, 5))), Err(error));
        request.canceled.store(canceled, Ordering::Release);
        app.mix_request = Some(request);
        app.poll_mix(&ctx);
        assert!(app.playlist_controls.mix_notice.is_none());
        assert!(app.track_menu_error.is_none() && app.mix_error.is_none());
    }
    app.mix_request = Some(reply(
        42,
        true,
        Some(MixSeed::Track((-8, 91))),
        Err(WebAudioError::Network),
    ));
    app.poll_mix(&ctx);
    assert!(app.playlist_controls.mix_notice.is_none());
    assert_eq!(app.track_menu_error, Some(WebAudioError::Network));
    assert_eq!(app.mix_error, Some(WebAudioError::Network));
}

#[test]
fn a_track_mix_with_nothing_new_goes_on_from_the_newest_track_and_keeps_a_pressed_next() {
    for result in [
        // The same tracks again, or VK's very same batch.
        Ok(MixPage {
            tracks: page(3, &[91, 92, 93]).items,
            has_more: true,
        }),
        Err(WebAudioError::Pagination),
    ] {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let queue = page(3, &[91, 92, 93]).items;
        let keys: Vec<_> = queue.iter().map(Track::key).collect();
        app.playing = Some(queue[2].clone());
        app.play_queue = Some(queue);
        app.queue_origin = Some("track-mix:-8_91".into());
        app.mix_active = true;
        app.mix_seed = Some(MixSeed::Track((-8, 91)));
        app.mix_track_seeds = [(-8, 91), keys[2]].into_iter().collect();
        app.mix_has_more = true;
        // Next was pressed on the last track while the batch was loading.
        app.mix_advance = true;
        app.mix_request = Some(reply(42, false, app.mix_seed, result));
        app.poll_mix(&ctx);
        // The newest track not yet a source: the playing one already was.
        assert_eq!(app.mix_seed, Some(MixSeed::Track(keys[1])));
        assert!(app.mix_track_seeds.contains(&keys[1]));
        assert!(app.mix_advance, "the pressed Next waits for the new tracks");
        assert!(app.mix_active && app.mix_has_more);
        assert_eq!(app.queue_origin.as_deref(), Some("track-mix:-8_91"));
        // This test session has no VK client: the request stops there.
        assert_eq!(app.mix_error, Some(WebAudioError::NoSession));
        assert!(app.mix_request.is_none() && app.workers.is_empty());
    }
}

#[test]
fn a_track_mix_stops_once_every_queued_track_was_a_source() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let queue = page(2, &[91, 92]).items;
    app.mix_track_seeds = queue.iter().map(Track::key).collect();
    app.playing = Some(queue[1].clone());
    app.play_queue = Some(queue);
    app.mix_active = true;
    app.mix_seed = Some(MixSeed::Track((-8, 91)));
    app.mix_has_more = true;
    app.mix_request = Some(reply(
        42,
        false,
        app.mix_seed,
        Err(WebAudioError::Pagination),
    ));
    app.poll_mix(&ctx);
    assert_eq!(app.mix_seed, Some(MixSeed::Track((-8, 91))));
    assert_eq!(app.mix_error, Some(WebAudioError::Pagination));
}

#[test]
fn next_on_the_last_queued_track_starts_the_mix_when_the_setting_is_on() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.demo = false;
    // A synthetic client: nothing here asks VK, the check only reads state.
    app.session.as_mut().unwrap().music = Some(
        WebAudioClient::new(vec![crate::web_session::SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-next-test".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap(),
    );
    app.library.exhausted = true;
    let queue = page(2, &[91, 92]).items;
    app.playing = Some(queue[1].clone());
    app.selected = app.playing.as_ref().map(Track::key);
    app.play_queue = Some(queue);
    app.queue_origin = Some("playlist:42_9".into());
    app.synchronize_order();
    assert_eq!(
        app.adjacent_track(true),
        None,
        "the last track of the queue"
    );
    app.mix_after_queue = false;
    assert!(!app.next_available());
    app.mix_after_queue = true;
    assert!(
        app.next_available(),
        "Next starts the mix of the last track"
    );
    assert!(app.workers.is_empty());
}
