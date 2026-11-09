use super::*;
use crate::app::tests::{account_ui_app, page, search_test_app};

fn statuses(library: &Library) -> Vec<(i64, Option<bool>)> {
    library
        .tracks
        .iter()
        .map(|track| (track.id, track.unavailable))
        .collect()
}

#[test]
fn sparse_library_pages_preserve_known_restrictions_in_both_fresh_prefix_and_cached_tail() {
    let mut cached = page(4, &[1, 2, 3, 4]);
    cached.items[0].unavailable = Some(true);
    cached.items[1].unavailable = Some(false);
    cached.items[2].unavailable = Some(true);
    let mut library = Library::from_cached(cached.items);

    library.refresh(page(4, &[1, 2]));
    assert_eq!(library.fresh_prefix, 2);
    assert_eq!(
        statuses(&library),
        [
            (1, Some(true)),
            (2, Some(false)),
            (3, Some(true)),
            (4, None)
        ]
    );
    library.append(page(4, &[1, 3]));
    assert_eq!(library.fresh_prefix, 3);
    assert_eq!(
        statuses(&library),
        [
            (1, Some(true)),
            (2, Some(false)),
            (3, Some(true)),
            (4, None)
        ]
    );

    let mut cleared = page(4, &[1, 3]);
    for track in &mut cleared.items {
        track.unavailable = Some(false);
    }
    library.append(cleared);
    assert_eq!(
        statuses(&library),
        [
            (1, Some(false)),
            (2, Some(false)),
            (3, Some(false)),
            (4, None)
        ]
    );
    library.refresh(page(4, &[3, 4]));
    assert_eq!(
        statuses(&library),
        [
            (3, Some(false)),
            (4, None),
            (1, Some(false)),
            (2, Some(false))
        ]
    );
}

fn metadata_reply(owner: i64, tracks: Vec<Track>) -> MetadataRequest {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(Ok(tracks)).ok().unwrap();
    MetadataRequest {
        owner,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    }
}

#[test]
fn metadata_updates_only_known_availability_and_keeps_all_playback_copies_in_sync() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let mut playing = page(1, &[1]).items.remove(0);
    playing.unavailable = Some(true);
    let other = page(1, &[2]).items.remove(0);
    app.library = Library::from_cached(vec![playing.clone(), other.clone()]);
    app.playing = Some(playing.clone());
    app.play_queue = Some(vec![playing.clone(), other]);
    app.recent = vec![playing.clone()];
    app.queue_origin = Some("library".into());
    for (owner, fresh, expected) in [
        (43, Some(false), Some(true)),
        (42, None, Some(true)),
        (42, Some(false), Some(false)),
        (42, None, Some(false)),
        (42, Some(true), Some(true)),
    ] {
        let mut metadata = playing.clone();
        metadata.unavailable = fresh;
        app.metadata_request = Some(metadata_reply(owner, vec![metadata]));
        app.poll_metadata(&ctx);
        assert!(app.metadata_request.is_none());
        assert_eq!(app.library.tracks[0].unavailable, expected);
        assert_eq!(app.playing.as_ref().unwrap().unavailable, expected);
        assert_eq!(app.play_queue.as_ref().unwrap()[0].unavailable, expected);
        assert_eq!(app.recent[0].unavailable, expected);
        assert_eq!(app.library.tracks[1].unavailable, None);
        assert_eq!(app.play_queue.as_ref().unwrap()[1].unavailable, None);
        assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
        assert_eq!(app.queue_origin.as_deref(), Some("library"));
        assert!(app.playback_pending.is_none());
        assert_eq!(app.player.snapshot().state, PlaybackState::Stopped);
    }
}

fn row_frame(
    ctx: &egui::Context,
    app: &mut MusicApp,
    track: &Track,
    tick: &mut u32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<TrackAction>) {
    *tick += 1;
    let mut action = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 120.0),
            )),
            time: Some(f64::from(*tick) * 0.1),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            action = app.track_row(ui, track, 0, false, false);
        },
    );
    output.textures_delta.clear();
    assert!(
        app.workers.is_empty(),
        "A row widget started a background operation"
    );
    (output, action)
}

fn text_shape<'a>(output: &'a egui::FullOutput, caption: &str) -> &'a egui::epaint::TextShape {
    fn find<'a>(shape: &'a egui::Shape, caption: &str) -> Option<&'a egui::epaint::TextShape> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == caption => Some(text),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, caption)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, caption))
        .unwrap_or_else(|| panic!("Missing {caption}"))
}

fn painted_text_color(text: &egui::epaint::TextShape) -> Color32 {
    text.override_text_color.unwrap_or_else(|| {
        text.galley
            .rows
            .iter()
            .flat_map(|row| row.visuals.mesh.vertices.iter())
            .map(|vertex| vertex.color)
            .find(|color| color.a() != 0)
            .expect("Visible text has a painted glyph")
    })
}

#[test]
fn explicitly_unavailable_rows_are_muted_even_when_selected_while_unknown_rows_keep_normal_style() {
    for unavailable in [None, Some(false), Some(true)] {
        for selected in [false, true] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            let mut track = page(1, &[1]).items.remove(0);
            track.title = "Availability title".into();
            track.artist = "Availability artist".into();
            track.unavailable = unavailable;
            app.selected = selected.then_some(track.key());
            let mut tick = 0;
            row_frame(&ctx, &mut app, &track, &mut tick, Vec::new());
            let (output, action) = row_frame(&ctx, &mut app, &track, &mut tick, Vec::new());
            assert!(action.is_none());
            let title = text_shape(&output, &track.title);
            let artist = text_shape(&output, &track.artist);
            let title_color = title.galley.job.sections[0].format.color;
            if unavailable == Some(true) {
                assert_eq!(title_color, MUTED);
                // Painter::set_opacity adjusts mesh/override colors in gamma
                // space; TextShape.opacity_factor itself remains unchanged.
                assert_eq!(painted_text_color(title), MUTED.gamma_multiply(0.65));
                assert_eq!(painted_text_color(artist), MUTED.gamma_multiply(0.65));
            } else {
                assert_eq!(title.opacity_factor, 1.0);
                assert_eq!(artist.opacity_factor, 1.0);
                if selected {
                    assert_eq!(title_color, ACCENT);
                    assert_eq!(painted_text_color(title), ACCENT);
                } else {
                    assert_ne!(title_color, MUTED);
                    assert_eq!(painted_text_color(title), title_color);
                }
                assert_eq!(painted_text_color(artist), MUTED);
            }
        }
    }
}

#[test]
fn muted_rows_still_emit_the_original_play_or_artist_intent_for_the_caller_to_handle() {
    for target in ["title", "index", "artist"] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let mut track = page(1, &[1]).items.remove(0);
        track.title = "Unavailable song".into();
        track.artist = "Canonical artist".into();
        track.artists = vec![ArtistRef {
            id: "canonical".into(),
            name: track.artist.clone(),
        }];
        track.unavailable = Some(true);
        let mut tick = 0;
        row_frame(&ctx, &mut app, &track, &mut tick, Vec::new());
        let (output, _) = row_frame(&ctx, &mut app, &track, &mut tick, Vec::new());
        let caption: &str = match target {
            "title" => &track.title,
            "artist" => &track.artist,
            _ => "1",
        };
        let point = text_shape(&output, caption).visual_bounding_rect().center();
        let mut action = None;
        for pressed in [true, false] {
            action = row_frame(
                &ctx,
                &mut app,
                &track,
                &mut tick,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            )
            .1;
        }
        if target == "artist" {
            assert!(
                matches!(action, Some(TrackAction::Artist(ref artist)) if *artist == track.artists[0])
            );
        } else {
            assert!(matches!(action, Some(TrackAction::Play(key)) if key == track.key()));
        }
        assert!(app.selected.is_none() && app.playback_pending.is_none());
    }
}

#[test]
fn rows_the_heart_shows_as_not_saved_stay_out_of_my_music() {
    let mut played = page(2, &[7, 8]);
    for track in &mut played.items {
        track.owner_id = -100;
    }
    let mut saved = page(1, &[9]).items.remove(0);
    saved.owner_id = 42;
    let mut marked = page(1, &[10]).items.remove(0);
    marked.library_liked = Some(true);
    played.items.extend([saved, marked]);
    played.count = Some(4);
    played.exhausted = Some(true);
    let mut library = Library::from_cached(page(1, &[7]).items).for_owner(42);
    assert!(
        library.tracks.is_empty(),
        "cached unsaved rows are dropped too"
    );
    library.refresh(played);
    let ids: Vec<_> = library.tracks.iter().map(|track| track.id).collect();
    assert_eq!(ids, [9, 10]);
    assert_eq!(library.total, Some(4), "the counter stays VK's own");
    assert_eq!(library.offset, 4, "VK's offset still counts every row");
    let mut only_played = page(2, &[7, 8]);
    for track in &mut only_played.items {
        track.owner_id = 55;
    }
    let mut partial = page(3, &[11, 12]);
    partial.items[0].owner_id = 42;
    partial.items[1].owner_id = 55;
    partial.exhausted = Some(false);
    let mut library = Library::default().for_owner(42);
    library.refresh(partial);
    assert_eq!(library.tracks.len(), 1);
    assert_eq!(library.total, Some(3));
    let mut library = Library::default().for_owner(42);
    library.refresh(only_played);
    assert!(library.tracks.is_empty());
    assert_eq!(library.total, Some(2));
    assert!(library.exhausted);
}

#[test]
fn loaded_dates_survive_a_refreshed_page_without_them() {
    let mut library = Library::default();
    let mut first = page(2, &[1, 2]);
    first.items[0].added = Some(1_660_000_000);
    library.refresh(first);
    // The website's next answer for the same tracks has no dates.
    library.refresh(page(2, &[1, 2]));
    assert_eq!(library.tracks[0].added, Some(1_660_000_000));
    assert_eq!(library.tracks[1].added, None);
}
