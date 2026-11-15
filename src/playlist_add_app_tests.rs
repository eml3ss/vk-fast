//! The "Add tracks" window: what it lists, what a plus sends, and closing.
use super::*;
use crate::app::tests::{page, search_test_app};

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
        |ui| app.add_tracks_window(ui.ctx()),
    );
    output.textures_delta.clear();
    output
}

fn text(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.visual_bounding_rect())
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, label))
}

#[test]
fn a_plus_adds_one_track_at_a_time_and_tracks_already_there_are_marked() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.demo = false;
    let mut tracks = page(3, &[61, 62, 63]).items;
    for (track, title) in tracks
        .iter_mut()
        .zip(["Alpha song", "Beta song", "Gamma tune"])
    {
        track.title = title.into();
    }
    app.library.tracks = tracks.clone();
    let playlist = AlbumRef {
        id: 14,
        owner_id: 42,
        title: "Own playlist".into(),
        access_key: None,
        cover_url: None,
    };
    let present = HashSet::from([tracks[0].key()]);
    let (dialog, work, reply) = playlist_add_app::AddTracks::new(playlist, present);
    app.add_tracks = Some(dialog);
    let mut tick = 0;
    frame(&ctx, &mut app, &mut tick, Vec::new());
    let output = frame(&ctx, &mut app, &mut tick, Vec::new());
    for title in ["Alpha song", "Beta song", "Gamma tune"] {
        assert!(text(&output, title).is_some(), "{title}");
    }
    // The plus sits right of the duration on the track's own row.
    let beta = text(&output, "Beta song").unwrap();
    let duration = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if text.galley.text() == tracks[1].duration_label()
                    && (text.visual_bounding_rect().center().y - beta.center().y).abs() < 16.0 =>
            {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        })
        .unwrap();
    // Title and artists sit in the middle of the row, like its time.
    let artist = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if text.galley.text() == tracks[1].artist
                    && text.visual_bounding_rect().top() > beta.top() =>
            {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        })
        .unwrap();
    let middle = (beta.top() + artist.bottom()) * 0.5;
    assert!(
        (middle - duration.center().y).abs() < 2.5,
        "text centred: {middle} vs {}",
        duration.center().y
    );
    let plus = egui::pos2(duration.right() + 10.0 + 11.0, duration.center().y);
    for pressed in [true, false] {
        frame(
            &ctx,
            &mut app,
            &mut tick,
            vec![
                egui::Event::PointerMoved(plus),
                egui::Event::PointerButton {
                    pos: plus,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(work.try_recv().unwrap().key(), tracks[1].key());
    assert!(work.try_recv().is_err(), "one press, one addition");
    // A track the playlist has is never sent.
    playlist_add_app::AddTracks::send_for_test(app.add_tracks.as_mut().unwrap(), &tracks[0]);
    assert!(work.try_recv().is_err());

    // VK's answer marks it added.
    reply.send((tracks[1].key(), Ok((7, 1)))).unwrap();
    frame(&ctx, &mut app, &mut tick, Vec::new());
    assert_eq!(app.add_tracks.as_ref().unwrap().added_count(), 1);

    // A query keeps only matching tracks of My music.
    app.add_tracks.as_mut().unwrap().set_query("gamma");
    let output = frame(&ctx, &mut app, &mut tick, Vec::new());
    assert!(text(&output, "Gamma tune").is_some());
    assert!(text(&output, "Alpha song").is_none());
    assert!(text(&output, "VK search").is_some());

    // A click on a row plays it, in My music's matches.
    app.double_click_to_play = false;
    let output = frame(&ctx, &mut app, &mut tick, Vec::new());
    let gamma = text(&output, "Gamma tune").unwrap().center();
    for pressed in [true, false] {
        frame(
            &ctx,
            &mut app,
            &mut tick,
            vec![
                egui::Event::PointerMoved(gamma),
                egui::Event::PointerButton {
                    pos: gamma,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(app.playing.as_ref().map(Track::key), Some(tracks[2].key()));
    assert_eq!(
        app.play_queue.as_ref().map(|queue| queue.len()),
        Some(1),
        "the matches it was clicked in"
    );
    assert!(work.try_recv().is_err(), "playing adds nothing");

    // Escape closes it and says how many were added.
    frame(
        &ctx,
        &mut app,
        &mut tick,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame(&ctx, &mut app, &mut tick, Vec::new());
    assert!(app.add_tracks.is_none());
    assert_eq!(app.track_menu_notice.as_ref().unwrap().0, "Tracks added: 1");
}
