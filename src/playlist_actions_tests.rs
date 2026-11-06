use super::*;
use crate::app::tests::{page, search_test_app, search_test_playlist};
use crate::web_audio::{AlbumPage, AlbumStats};

fn reference(id: i64) -> AlbumRef {
    let mut reference = search_test_playlist(id).album;
    reference.owner_id = 42;
    reference.access_key = None;
    reference
}

fn item(id: i64) -> playlist_ui::PlaylistItem {
    playlist_ui::PlaylistItem {
        reference: reference(id),
        subtitle: String::new(),
        can_edit: true,
        pinned: false,
    }
}

fn form(reference: AlbumRef, delete: bool) -> Form {
    Form {
        name: "Draft name".into(),
        reference,
        delete,
        description: "Draft description".into(),
        discoverable: None,
        can_edit: true,
        can_delete: true,
        remove_saved: false,
        loading: false,
        focus_name: false,
        error: None,
        cover: None,
        remove_cover: false,
        preview: None,
    }
}

fn metadata(reference: AlbumRef, can_edit: bool) -> PlaylistEditDetails {
    PlaylistEditDetails {
        playlist: reference,
        description: "Fresh description".into(),
        can_edit,
        can_delete: true,
        is_following: !can_edit,
        discoverable: None,
    }
}

fn managed_reply(
    owner: i64,
    target: Key,
    job: Job,
    result: Result<Managed, WebAudioError>,
) -> ManageRequest {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(result).ok().unwrap();
    ManageRequest {
        owner,
        target,
        job,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    }
}

fn play_reply(owner: i64, reference: AlbumRef, shuffle: bool) -> PlayRequest {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(Ok(page(2, &[4, 5]).items)).ok().unwrap();
    PlayRequest {
        owner,
        reference,
        shuffle,
        canceled: Arc::new(AtomicBool::new(false)),
        receiver,
    }
}

fn detail(reference: AlbumRef, album: bool) -> Arc<DetailPage> {
    let page = AlbumPage {
        album: reference,
        artists: Vec::new(),
        year: None,
        description: Some("Old description".into()),
        creator: None,
        tracks: page(1, &[99]).items,
        total_tracks: None,
        stats: AlbumStats::default(),
    };
    Arc::new(if album {
        DetailPage::Album(page)
    } else {
        DetailPage::Playlist(page)
    })
}

fn ids(app: &MusicApp) -> Vec<i64> {
    app.playlist_items
        .iter()
        .map(|item| item.reference.id)
        .collect()
}

#[test]
fn playlist_radio_has_its_own_source_and_only_accepts_matching_continuations() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let seed = Some(MixSeed::Playlist((42, 5)));
    let reply = |start, seed, tracks: &[i64], has_more| {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(crate::web_audio::MixPage {
                tracks: page(tracks.len(), tracks).items,
                has_more,
            }))
            .ok()
            .unwrap();
        MixRequest {
            owner: 42,
            start,
            seed,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    };
    app.mix_request = Some(reply(true, seed, &[10, 11], true));
    app.poll_mix(&ctx);
    assert_eq!(app.mix_seed, seed);
    assert_eq!(app.queue_origin.as_deref(), Some("playlist-mix:42_5"));
    app.mix_request = Some(reply(false, Some(MixSeed::Track((42, 5))), &[99], false));
    app.poll_mix(&ctx);
    assert!(app.mix_has_more);
    assert_eq!(app.play_queue.as_ref().unwrap().len(), 2);
    app.mix_request = Some(reply(false, seed, &[11, 12], false));
    app.poll_mix(&ctx);
    assert!(!app.mix_has_more);
    assert_eq!(
        app.play_queue
            .as_ref()
            .unwrap()
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        [10, 11, 12]
    );
}

#[test]
fn copy_playlist_link_preserves_the_native_identity_and_encodes_the_share_key() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let mut source = reference(9);
    source.access_key = Some("synthetic/key value".into());
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
        app.playlist_sidebar_action(&ctx, playlist_ui::Action::CopyLink(source.clone()));
    });
    assert!(output.platform_output.commands.iter().any(|command| matches!(command,
        egui::OutputCommand::CopyText(text) if text == "https://vk.ru/music/playlist/42_9_synthetic%2Fkey%20value"
    )));
    assert!(app.playing.is_none() && app.playlist_controls.request.is_none());
    output.textures_delta.clear();
}

#[test]
fn pins_keep_their_order_after_refresh_and_unpin_restores_the_native_order() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.playlist_items = vec![item(5), item(6), item(7)];
    app.remember_playlist_order();
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(6)));
    assert_eq!(ids(&app), [6, 5, 7]);
    assert!(app.playlist_items[0].pinned);
    app.playlist_items = vec![item(7), item(8), item(5), item(6)];
    app.remember_playlist_order();
    app.sync_playlist_pins();
    assert_eq!(ids(&app), [6, 7, 8, 5]);
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(6)));
    assert_eq!(ids(&app), [7, 8, 5, 6]);
    assert!(app.playlist_items.iter().all(|item| !item.pinned));
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(8)));
    assert_eq!(app.playlist_controls.pins, [(42, 8)]);
    app.session.as_mut().unwrap().profile.id = 43;
    app.sync_playlist_pins();
    assert_eq!(app.playlist_controls.pins_owner, Some(43));
    assert!(app.playlist_controls.pins.is_empty());
    assert!(app.playlist_items.iter().all(|item| !item.pinned));
    app.reset();
    assert!(app.playlist_controls.pins_owner.is_none());
    assert!(app.playlist_controls.base_order.is_empty());
    assert!(app.workers.is_empty());
}

#[test]
fn manually_reordered_playlists_keep_their_order_across_refresh_new_items_and_unpin() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.playlist_items = vec![item(5), item(6), item(7), item(8)];
    app.remember_playlist_order();
    app.sync_playlist_pins();
    let playing = page(1, &[99]).items.remove(0);
    app.playing = Some(playing.clone());
    app.play_queue = Some(vec![playing.clone()]);
    app.queue_origin = Some("kept-audio-source".into());

    app.playlist_sidebar_action(
        &ctx,
        playlist_ui::Action::Reorder {
            playlist: reference(7),
            before: Some(reference(5)),
        },
    );
    assert_eq!(ids(&app), [7, 5, 6, 8]);
    assert_eq!(
        app.playlist_controls.base_order,
        [(42, 7), (42, 5), (42, 6), (42, 8)]
    );

    // A fresh VK response may have a different ordering. The user's actual
    // drag remains authoritative, while genuinely new playlists stay visible.
    app.playlist_items = vec![item(8), item(6), item(5), item(7)];
    app.remember_playlist_order();
    app.sync_playlist_pins();
    assert_eq!(ids(&app), [7, 5, 6, 8]);
    app.playlist_items = vec![item(9), item(8), item(6), item(5), item(7)];
    app.remember_playlist_order();
    app.sync_playlist_pins();
    assert_eq!(ids(&app), [9, 7, 5, 6, 8]);
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(6)));
    assert_eq!(ids(&app), [6, 9, 7, 5, 8]);
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(6)));
    assert_eq!(ids(&app), [9, 7, 5, 6, 8]);

    assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
    assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
    assert_eq!(app.queue_origin.as_deref(), Some("kept-audio-source"));
    assert!(app.playback_pending.is_none() && app.workers.is_empty());
}

#[test]
fn pinned_drag_reorders_the_pinned_prefix_and_group_end_drops_preserve_priority() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.playlist_items = vec![item(5), item(6), item(7), item(8)];
    app.remember_playlist_order();
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(5)));
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(7)));
    assert_eq!(ids(&app), [7, 5, 6, 8]);

    app.playlist_sidebar_action(
        &ctx,
        playlist_ui::Action::Reorder {
            playlist: reference(5),
            before: Some(reference(7)),
        },
    );
    assert_eq!(ids(&app), [5, 7, 6, 8]);
    assert_eq!(app.playlist_controls.pins, [(42, 5), (42, 7)]);
    app.playlist_sidebar_action(
        &ctx,
        playlist_ui::Action::Reorder {
            playlist: reference(5),
            before: None,
        },
    );
    assert_eq!(ids(&app), [7, 5, 6, 8]);
    assert_eq!(app.playlist_controls.pins, [(42, 7), (42, 5)]);
    assert!(app.playlist_items[..2].iter().all(|item| item.pinned));
    assert!(app.playlist_items[2..].iter().all(|item| !item.pinned));

    app.playlist_sidebar_action(
        &ctx,
        playlist_ui::Action::Reorder {
            playlist: reference(6),
            before: None,
        },
    );
    assert_eq!(ids(&app), [7, 5, 8, 6]);
    app.playlist_items = vec![item(6), item(5), item(8), item(7)];
    app.remember_playlist_order();
    app.sync_playlist_pins();
    assert_eq!(ids(&app), [7, 5, 8, 6]);
    assert!(app.playing.is_none() && app.play_queue.is_none() && app.workers.is_empty());
}

#[test]
fn cross_group_self_and_unknown_reorder_targets_leave_the_sidebar_and_audio_unchanged() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.playlist_items = vec![item(5), item(6), item(7)];
    app.remember_playlist_order();
    app.playlist_sidebar_action(&ctx, playlist_ui::Action::TogglePin(reference(5)));
    let playing = page(1, &[99]).items.remove(0);
    app.playing = Some(playing.clone());
    app.play_queue = Some(vec![playing.clone()]);
    let before_ids = ids(&app);
    let before_order = app.playlist_controls.base_order.clone();
    let before_pins = app.playlist_controls.pins.clone();
    for (playlist, before) in [
        (reference(6), Some(reference(5))),
        (reference(5), Some(reference(7))),
        (reference(6), Some(reference(6))),
        (reference(99), Some(reference(6))),
        (reference(6), Some(reference(99))),
    ] {
        app.playlist_sidebar_action(&ctx, playlist_ui::Action::Reorder { playlist, before });
        assert_eq!(ids(&app), before_ids);
        assert_eq!(app.playlist_controls.base_order, before_order);
        assert_eq!(app.playlist_controls.pins, before_pins);
        assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
        assert!(app.playback_pending.is_none() && app.workers.is_empty());
    }
}

#[test]
fn delayed_playlist_play_is_account_bound_cancelable_and_superseded_by_manual_play() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let old = page(1, &[99]).items.remove(0);
    app.playing = Some(old.clone());
    app.play_queue = Some(vec![old.clone()]);
    app.queue_origin = Some("old-source".into());
    for (owner, canceled) in [(43, false), (42, true)] {
        let request = play_reply(owner, reference(5), true);
        request.canceled.store(canceled, Ordering::Release);
        app.playlist_controls.autoplay = Some(request);
        app.poll_playlist_actions(&ctx);
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), old.key());
        assert_eq!(app.queue_origin.as_deref(), Some("old-source"));
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let canceled = Arc::new(AtomicBool::new(false));
    app.playlist_controls.autoplay = Some(PlayRequest {
        owner: 42,
        reference: reference(5),
        shuffle: false,
        canceled: canceled.clone(),
        receiver,
    });
    app.play_track(&ctx, old.key());
    assert!(canceled.load(Ordering::Acquire));
    assert!(sender.send(Ok(page(2, &[4, 5]).items)).is_err());
    assert!(!app.playlist_controls.loading_play());
    app.playlist_controls.autoplay = Some(play_reply(42, reference(5), true));
    app.poll_playlist_actions(&ctx);
    assert_eq!(
        app.play_queue
            .as_ref()
            .unwrap()
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        [4, 5]
    );
    assert!(app.play_order.shuffle());
    assert_eq!(
        app.queue_origin,
        MusicPage::Playlist(reference(5)).detail_key()
    );
    assert!(app.workers.is_empty());
}

#[test]
fn a_new_playlist_play_intent_discards_older_delayed_sources_without_waiting_for_audio() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.home_autoplay = Some("old-collection".into());
    app.advance_on_load = true;
    app.mix_advance = true;
    app.play_sidebar_playlist(&ctx, reference(5), false);
    assert!(app.home_autoplay.is_none());
    assert!(!app.advance_on_load && !app.mix_advance);
    assert!(app.workers.is_empty());
}

#[test]
fn management_replies_require_the_current_owner_target_and_uncanceled_request() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.playlist_items = vec![item(5), item(6)];
    for (owner, target, canceled) in [
        (43, (42, 5), false),
        (42, (42, 6), false),
        (42, (42, 5), true),
    ] {
        app.playlist_controls.form = Some(form(reference(5), false));
        let request = managed_reply(
            owner,
            target,
            Job::Load,
            Ok(Managed::Loaded(metadata(reference(5), false))),
        );
        request.canceled.store(canceled, Ordering::Release);
        app.playlist_controls.request = Some(request);
        app.poll_playlist_manage(&ctx);
        assert_eq!(
            app.playlist_controls.form.as_ref().unwrap().name,
            "Draft name"
        );
        assert!(app.playlist_items[0].can_edit);
    }
    app.playlist_controls.form = Some(form(reference(5), true));
    app.playlist_controls.request = Some(managed_reply(
        42,
        (42, 5),
        Job::Load,
        Ok(Managed::Loaded(metadata(reference(5), false))),
    ));
    app.poll_playlist_manage(&ctx);
    let form = app.playlist_controls.form.as_ref().unwrap();
    assert!(form.remove_saved && form.can_delete && !form.can_edit && !form.loading);
    assert_eq!(form.description, "Fresh description");
    assert!(!app.playlist_items[0].can_edit);
    app.playlist_controls.request = Some(managed_reply(
        42,
        (42, 5),
        Job::Save,
        Err(WebAudioError::Rejected),
    ));
    app.poll_playlist_manage(&ctx);
    assert!(app.playlist_controls.form.as_ref().unwrap().error.is_some());
    let request = managed_reply(
        42,
        (42, 5),
        Job::Load,
        Ok(Managed::Loaded(metadata(reference(5), true))),
    );
    let canceled = request.canceled.clone();
    app.playlist_controls.request = Some(request);
    app.reset();
    assert!(canceled.load(Ordering::Acquire));
    assert!(!app.playlist_controls.has_modal());
    assert!(app.workers.is_empty());
}

#[test]
fn saved_playlist_refreshes_the_active_album_alias_and_cancels_reads_started_before_save() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let reference = reference(5);
    app.playlist_items = vec![item(5)];
    app.editable_playlists.push(reference.clone());
    let album_key = MusicPage::Album(reference.clone()).detail_key().unwrap();
    let playlist_key = MusicPage::Playlist(reference.clone()).detail_key().unwrap();
    app.page = MusicPage::Album(reference.clone());
    app.details
        .insert(album_key.clone(), detail(reference.clone(), true));
    app.details
        .insert(playlist_key.clone(), detail(reference.clone(), false));
    let playing = page(1, &[99]).items.remove(0);
    app.playing = Some(playing.clone());
    app.play_queue = Some(vec![playing.clone()]);
    app.queue_origin = Some("active-source".into());
    let (old_sender, old_receiver) = mpsc::sync_channel(1);
    let old_canceled = Arc::new(AtomicBool::new(false));
    app.detail_request = Some(DetailRequest {
        owner: 42,
        key: album_key.clone(),
        canceled: old_canceled.clone(),
        receiver: old_receiver,
    });
    app.playlist_controls.form = Some(form(reference.clone(), false));
    let mut fresh = reference.clone();
    fresh.title = "Saved name".into();
    app.playlist_controls.request = Some(managed_reply(
        42,
        key(&reference),
        Job::Save,
        Ok(Managed::Saved(Some(metadata(fresh, false)))),
    ));
    app.poll_playlist_manage(&ctx);
    assert!(app.playlist_controls.form.is_none());
    assert!(old_canceled.load(Ordering::Acquire));
    assert!(
        old_sender
            .send(Ok(DetailPage::Album(AlbumPage {
                album: reference,
                artists: Vec::new(),
                year: None,
                description: Some("Old".into()),
                creator: None,
                tracks: Vec::new(),
                total_tracks: None,
                stats: AlbumStats::default(),
            })))
            .is_err()
    );
    assert!(!app.details.contains_key(&playlist_key));
    let DetailPage::Album(saved) = app.details.get(&album_key).unwrap().as_ref() else {
        panic!()
    };
    assert_eq!(saved.album.title, "Saved name");
    assert_eq!(saved.description.as_deref(), Some("Fresh description"));
    assert_eq!(saved.tracks[0].id, 99);
    assert!(!app.playlist_items[0].can_edit && app.editable_playlists.is_empty());
    assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
    assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
    assert_eq!(app.queue_origin.as_deref(), Some("active-source"));
    assert!(app.workers.is_empty());
}

#[test]
fn deletion_removes_only_the_target_sidebar_cache_and_history_and_keeps_current_audio() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let reference = reference(5);
    let target_routes = [
        MusicPage::Playlist(reference.clone()),
        MusicPage::Album(reference.clone()),
    ];
    app.playlist_items = vec![item(5), item(6)];
    app.editable_playlists = vec![reference.clone(), self::reference(6)];
    app.remember_playlist_order();
    app.sync_playlist_pins();
    app.playlist_controls.pins = vec![(42, 5), (42, 6)];
    for route in &target_routes {
        app.details.insert(
            route.detail_key().unwrap(),
            detail(reference.clone(), matches!(route, MusicPage::Album(_))),
        );
    }
    app.page = target_routes[0].clone();
    let other = MusicPage::Playlist(self::reference(6));
    for route in target_routes.iter().chain(std::iter::once(&other)) {
        let location = NavigationLocation {
            page: route.clone(),
            settings: false,
            section: SettingsSection::default(),
            filter: String::new(),
        };
        app.back_history.push(location.clone());
        app.forward_history.push(location);
    }
    let playing = page(1, &[99]).items.remove(0);
    app.playing = Some(playing.clone());
    app.play_queue = Some(vec![playing.clone()]);
    app.queue_origin = target_routes[0].detail_key();
    app.playlist_controls.form = Some(form(reference.clone(), true));
    app.playlist_controls.request = Some(managed_reply(
        42,
        key(&reference),
        Job::Delete,
        Ok(Managed::Deleted),
    ));
    app.poll_playlist_manage(&ctx);
    assert_eq!(ids(&app), [6]);
    assert_eq!(app.editable_playlists, [self::reference(6)]);
    assert_eq!(app.playlist_controls.pins, [(42, 6)]);
    assert!(
        target_routes
            .iter()
            .all(|route| !app.details.contains_key(&route.detail_key().unwrap()))
    );
    assert!(app.page == MusicPage::Library);
    assert!(
        app.back_history
            .iter()
            .chain(&app.forward_history)
            .all(|location| !target_routes.contains(&location.page))
    );
    assert!(
        app.back_history
            .iter()
            .any(|location| location.page == other)
    );
    assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
    assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
    assert_eq!(app.queue_origin, target_routes[0].detail_key());
    assert!(app.workers.is_empty());
}
