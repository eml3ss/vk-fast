use super::*;
use crate::web_audio::{AlbumPage, AlbumStats, OwnerPage, PlaylistCreator, UserPlaylist};

fn creator(id: i64) -> PlaylistCreator {
    PlaylistCreator {
        owner_id: id,
        name: "Publisher".into(),
    }
}

fn page(id: i64, offset: usize, items: &[i64]) -> OwnerPage {
    OwnerPage {
        creator: creator(id),
        image_url: Some("https://images.example.test/owner.jpg".into()),
        description: Some("Public music".into()),
        closed: None,
        tracks: Vec::new(),
        tracks_error: None,
        playlists: items
            .iter()
            .map(|item| UserPlaylist {
                kind: crate::web_audio::CollectionKind::Playlist,
                byline: String::new(),
                original: None,
                album: AlbumRef {
                    owner_id: id,
                    id: *item,
                    title: format!("Playlist {item}"),
                    access_key: None,
                    cover_url: None,
                },
                subtitle: String::new(),
                track_count: Some(4),
                can_add_tracks: false,
            })
            .collect(),
        total: Some(500),
        offset,
        next_offset: Some(offset + 100),
        playlists_error: None,
        retry_offset: None,
    }
}

fn reply(page: OwnerPage) -> DetailRequest {
    let (sender, receiver) = mpsc::sync_channel(1);
    let key = MusicPage::Owner(page.creator.clone()).detail_key().unwrap();
    sender.send(Ok(DetailPage::Owner(page))).ok().unwrap();
    DetailRequest {
        owner: 42,
        key,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    }
}

#[test]
fn owner_links_navigate_inside_the_player_and_back_without_changing_playback() {
    for owner in [71, -71] {
        let ctx = egui::Context::default();
        let mut app = tests::search_test_app(&ctx);
        let album = AlbumRef {
            id: 8,
            owner_id: 42,
            title: "Followed playlist".into(),
            access_key: None,
            cover_url: None,
        };
        let source = MusicPage::Playlist(album.clone());
        app.page = source.clone();
        app.details.insert(
            source.detail_key().unwrap(),
            Arc::new(DetailPage::Playlist(AlbumPage {
                album,
                artists: Vec::new(),
                year: None,
                description: None,
                creator: Some(creator(owner)),
                tracks: Vec::new(),
                total_tracks: None,
                stats: AlbumStats::default(),
            })),
        );
        let playing = demo_tracks().remove(0);
        app.playing = Some(playing.clone());
        app.play_queue = Some(vec![playing.clone()]);
        let mut tick = 0;
        let mut render = |app: &mut MusicApp, events| {
            tick += 1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 760.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| app.detail_content(ui),
            );
            output.textures_delta.clear();
            output
        };
        render(&mut app, Vec::new());
        let output = render(&mut app, Vec::new());
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Publisher" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap();
        for pressed in [true, false] {
            let output = render(
                &mut app,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert!(
                output.platform_output.commands.is_empty(),
                "Owner clicks must not launch a browser"
            );
        }
        assert!(app.page == MusicPage::Owner(creator(owner)));
        assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
        assert!(app.view_tracks().is_empty());
        app.go_back(&ctx);
        assert!(app.page == source);
        render(&mut app, Vec::new());
        app.go_forward(&ctx);
        assert!(app.page == MusicPage::Owner(creator(owner)));
    }
}

#[test]
fn owner_page_continuations_preserve_headers_and_errors_and_reject_stale_destinations() {
    let ctx = egui::Context::default();
    let mut app = tests::search_test_app(&ctx);
    app.page = MusicPage::Owner(creator(-71));
    let key = app.page.detail_key().unwrap();
    app.detail_request = Some(reply(page(-71, 0, &[1, 2])));
    app.poll_detail(&ctx);
    let mut failed = page(-71, 100, &[]);
    failed.playlists_error = Some(WebAudioError::Network);
    failed.retry_offset = Some(100);
    app.detail_request = Some(reply(failed));
    app.poll_detail(&ctx);
    let DetailPage::Owner(cached) = app.details[&key].as_ref() else {
        panic!()
    };
    assert_eq!(cached.playlists.len(), 2);
    assert_eq!(cached.retry_offset, Some(100));
    assert_eq!(cached.next_offset, Some(100));
    let mut next = page(-71, 100, &[2, 3]);
    next.image_url = None;
    next.description = None;
    app.detail_request = Some(reply(next));
    app.poll_detail(&ctx);
    let DetailPage::Owner(cached) = app.details[&key].as_ref() else {
        panic!()
    };
    assert_eq!(cached.playlists.len(), 3);
    assert_eq!(cached.next_offset, Some(200));
    assert!(cached.image_url.is_some() && cached.description.is_some());
    assert!(cached.playlists_error.is_none());
    app.detail_request = Some(reply(page(-71, 200, &[4])));
    app.page = MusicPage::Owner(creator(72));
    app.poll_detail(&ctx);
    assert!(!app.details.contains_key(&app.page.detail_key().unwrap()));
    assert_eq!(app.session.as_ref().unwrap().profile.id, 42);
    assert!(app.library.tracks.is_empty());
}
