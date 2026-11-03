use super::*;
use crate::app::tests::search_test_app;

fn versions() -> Vec<Track> {
    let base = Track {
        owner_id: -10,
        id: 100,
        artist: "Same artist".into(),
        title: "Same title".into(),
        duration: 120,
        subtitle: None,
        release_key: Some((-10, 100)),
        library_liked: Some(false),
        ..crate::model::demo_tracks().remove(0)
    };
    let mut tracks = vec![base.clone(); 4];
    tracks[1].id = 101;
    tracks[1].subtitle = Some("Slowed".into());
    tracks[2].id = 102;
    tracks[2].subtitle = Some("Sped Up".into());
    // Even the numeric audio ID, title and duration can match another owner.
    tracks[3].id = 101;
    tracks[3].owner_id = -11;
    tracks[3].subtitle = Some("Slowed".into());
    tracks
}

fn assert_hearts(app: &MusicApp, expected: [bool; 4]) {
    for tracks in [
        app.recent.as_slice(),
        app.play_queue.as_ref().unwrap().as_slice(),
        app.search.results.as_ref().unwrap().tracks.as_slice(),
    ] {
        assert_eq!(
            tracks
                .iter()
                .map(|track| app.track_is_liked(track))
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.library_liked.unwrap())
                .collect::<Vec<_>>(),
            expected
        );
    }
    assert_eq!(
        app.track_is_liked(app.playing.as_ref().unwrap()),
        expected[1]
    );
    for track in app.queued_tracks.values() {
        let index = versions()
            .iter()
            .position(|source| source.key() == track.key())
            .unwrap();
        assert_eq!(app.track_is_liked(track), expected[index]);
    }
}

#[test]
fn likes_do_not_spread_to_same_title_versions_or_other_owners_and_survive_reload() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let mut tracks = versions();
    tracks[2].library_liked = Some(true);
    let source = tracks[1].clone();
    let mut already_saved = tracks[2].clone();
    already_saved.owner_id = 42;
    already_saved.id = 902;
    already_saved.library_source_key = Some(tracks[2].key());
    app.library = Library::from_cached(vec![already_saved]);
    app.recent = tracks.clone();
    app.play_queue = Some(tracks.clone());
    app.queued_tracks = tracks
        .iter()
        .cloned()
        .map(|track| (track.key(), track))
        .collect();
    app.playing = Some(source.clone());
    app.search.results = Some(Arc::new(crate::web_audio::SearchResults {
        tracks,
        artists: Vec::new(),
        albums: Vec::new(),
        playlists: Vec::new(),
        has_more: false,
        next_offset: 4,
    }));
    assert_hearts(&app, [false, false, true, false]);

    app.apply_library_change(&ctx, &source, LibraryChange::Added((42, 901)));
    assert_hearts(&app, [false, true, true, false]);
    assert_eq!(app.owned_library_key(&source), Some((42, 901)));
    assert_eq!(app.owned_library_key(&versions()[0]), None);
    assert_eq!(app.owned_library_key(&versions()[3]), None);
    assert!(!app.like_aliases.contains_key(&(-10, 100)));

    // Restart restores only the acknowledged source/owned-copy relationship.
    let bytes = library_cache::encode_bounded(42, &app.library.tracks, 256 * 1024, 32).unwrap();
    app.library =
        Library::from_cached(library_cache::decode_bounded(&bytes, 42, 256 * 1024, 32).unwrap());
    app.like_aliases.clear();
    app.track_view_cache.invalidate();
    assert_hearts(&app, [false, true, true, false]);
    assert_eq!(app.owned_library_key(&source), Some((42, 901)));

    // A sparse fresh library page must not erase that confirmed association.
    let fresh: Vec<_> = app
        .library
        .tracks
        .iter()
        .cloned()
        .map(|mut track| {
            track.library_source_key = None;
            track
        })
        .collect();
    app.library.refresh(AudioPage {
        count: Some(2),
        exhausted: Some(true),
        items: fresh,
    });
    app.track_view_cache.invalidate();
    assert_eq!(app.owned_library_key(&source), Some((42, 901)));

    let saved = app
        .library
        .tracks
        .iter()
        .find(|track| track.key() == (42, 901))
        .unwrap()
        .clone();
    app.apply_library_change(&ctx, &saved, LibraryChange::Removed(vec![saved.key()]));
    assert_hearts(&app, [false, false, true, false]);
    assert_eq!(
        app.library
            .tracks
            .iter()
            .map(Track::key)
            .collect::<Vec<_>>(),
        [(42, 902)]
    );
    assert_eq!(app.playing.as_ref().unwrap().key(), source.key());
}

#[test]
fn acknowledged_membership_and_late_metadata_only_change_exact_audio_ids() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.recent = versions();
    let source = app.recent[1].clone();
    app.apply_library_change(&ctx, &source, LibraryChange::AlreadyAdded);
    assert_eq!(
        app.recent
            .iter()
            .map(|track| track.library_liked)
            .collect::<Vec<_>>(),
        [Some(false), Some(true), Some(false), Some(false)]
    );

    app.apply_library_change(&ctx, &source, LibraryChange::Added((42, 901)));
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(Ok(versions())).ok().unwrap();
    app.metadata_request = Some(MetadataRequest {
        owner: 42,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    app.poll_metadata(&ctx);
    assert_eq!(
        app.recent
            .iter()
            .map(|track| track.library_liked)
            .collect::<Vec<_>>(),
        [Some(false), Some(true), Some(false), Some(false)]
    );

    app.apply_library_change(&ctx, &source, LibraryChange::Removed(vec![(42, 901)]));
    let mut fresh = versions();
    fresh[0].library_liked = Some(true);
    fresh[1].library_liked = Some(true);
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(Ok(fresh)).ok().unwrap();
    app.metadata_request = Some(MetadataRequest {
        owner: 42,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    app.poll_metadata(&ctx);
    assert_eq!(
        app.recent
            .iter()
            .map(|track| track.library_liked)
            .collect::<Vec<_>>(),
        [Some(true), Some(false), Some(false), Some(false)]
    );
}
