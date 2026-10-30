use super::*;
use serde_json::json;

fn client() -> WebAudioClient {
    let client = WebAudioClient::new(vec![SessionCookie {
        domain: "vk.ru".into(),
        path: "/".into(),
        name: "remixsid".into(),
        value: "synthetic-playlist-mix-session".into(),
        secure: true,
        expires_unix: None,
    }])
    .unwrap();
    client.accept_profile(42).unwrap();
    let mut state = client.state.lock().unwrap();
    state.cursor = Some(Cursor {
        owner: 42,
        offset: 17,
        next: Some("library-cursor".into()),
        ..Default::default()
    });
    state.mix = Some(MixSession {
        token: Zeroizing::new("synthetic-web-token".into()),
        expires: now() + 3600,
        api_host: "web.api.vk.ru",
        previous_batch: Vec::new(),
        previous_source: MixSource::Personal,
    });
    drop(state);
    client
}

fn playlist() -> AlbumRef {
    AlbumRef {
        owner_id: -9,
        id: 11,
        title: "Playlist".into(),
        access_key: Some("synthetic-access-key-must-stay-private".into()),
        cover_url: None,
    }
}

fn batch(ids: &[i64]) -> Zeroizing<String> {
    Zeroizing::new(
        json!({"response": ids.iter().map(|id| json!({
            "owner_id":-7,"id":id,"title":"Recommendation","artist":"Artist","duration":180,
            "url":"https://audio.example.test/playlist-mix.mp3"
        })).collect::<Vec<_>>()})
        .to_string(),
    )
}

#[test]
fn native_playlist_mix_uses_playlist_identity_and_keeps_its_continuation_separate() {
    let client = client();
    let canceled = AtomicBool::new(false);
    let reference = playlist();
    let seed = MixSource::Playlist((reference.owner_id, reference.id));
    let first = client
        .stream_mix_request(42, seed, &canceled, |session, params| {
            assert!(session.previous_batch.is_empty());
            assert_eq!(
                params,
                vec![
                    ("mix_id", "playlist_mix".into()),
                    ("entity_id", "-9_11".into()),
                ]
            );
            assert!(!params.iter().any(|(_, value)| value.contains("access-key")));
            Ok(batch(&[101]))
        })
        .unwrap();
    assert!(first.has_more);
    assert_eq!(first.tracks[0].key(), (-7, 101));
    assert!(first.tracks[0].url.is_none());
    client
        .stream_mix_request(42, seed, &canceled, |session, params| {
            assert_eq!(session.previous_batch, [(-7, 101)]);
            assert_eq!(params, seed.params());
            Ok(batch(&[102]))
        })
        .unwrap();
    // Equal owner/id pairs in different domains are not the same radio source.
    client
        .stream_mix_request(42, MixSource::Track((-9, 11)), &canceled, |session, _| {
            assert!(session.previous_batch.is_empty());
            Ok(batch(&[102]))
        })
        .unwrap();
    client
        .stream_mix_request(42, seed, &canceled, |session, _| {
            assert!(session.previous_batch.is_empty());
            Ok(batch(&[102]))
        })
        .unwrap();
    assert!(matches!(
        client.stream_mix_request(42, seed, &canceled, |_, _| Ok(batch(&[102]))),
        Err(WebAudioError::Pagination)
    ));
    let state = client.state.lock().unwrap();
    assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
    assert_eq!(
        state.cursor.as_ref().unwrap().next.as_deref(),
        Some("library-cursor")
    );
    assert!(state.streams[&(-7, 101)].original_url.is_some());
    assert!(state.mix.as_ref().unwrap().previous_source == seed);
}

#[test]
fn playlist_mix_validates_source_account_cancellation_and_generation_before_committing() {
    let client = client();
    for (owner, id) in [(0, 1), (i64::MIN, 1), (42, 0), (-9, -1)] {
        let mut reference = playlist();
        reference.owner_id = owner;
        reference.id = id;
        assert!(matches!(
            client.playlist_mix(42, &reference, &AtomicBool::new(false)),
            Err(WebAudioError::Format)
        ));
    }
    assert!(matches!(
        client.playlist_mix(99, &playlist(), &AtomicBool::new(false)),
        Err(WebAudioError::WrongAccount)
    ));
    assert!(matches!(
        client.playlist_mix(42, &playlist(), &AtomicBool::new(true)),
        Err(WebAudioError::Canceled)
    ));
    let seed = MixSource::Playlist((-9, 11));
    let canceled = AtomicBool::new(false);
    assert!(matches!(
        client.stream_mix_request(42, seed, &canceled, |_, _| {
            canceled.store(true, Ordering::Release);
            Ok(batch(&[201]))
        }),
        Err(WebAudioError::Canceled)
    ));
    assert!(client.state.lock().unwrap().streams.is_empty());
    canceled.store(false, Ordering::Release);
    assert!(matches!(
        client.stream_mix_request(42, seed, &canceled, |_, _| {
            client.accept_profile(99).unwrap();
            Ok(batch(&[202]))
        }),
        Err(WebAudioError::Unverified)
    ));
    assert!(client.state.lock().unwrap().streams.is_empty());
}

#[test]
fn verified_viewer_refresh_preserves_the_active_playlist_radio_and_prefetched_streams() {
    let client = client();
    let seed = MixSource::Playlist((-9, 11));
    client
        .stream_mix_request(42, seed, &AtomicBool::new(false), |_, _| Ok(batch(&[301])))
        .unwrap();
    let generation = client.state.lock().unwrap().generation;
    client
        .refresh_profile_request(42, || {
            Ok(Profile {
                id: 42,
                first_name: "Synthetic".into(),
                last_name: "User".into(),
                avatar_url: None,
            })
        })
        .unwrap();
    let state = client.state.lock().unwrap();
    assert_eq!(state.generation, generation);
    assert!(state.mix.as_ref().unwrap().previous_source == seed);
    assert_eq!(state.mix.as_ref().unwrap().previous_batch, [(-7, 301)]);
    assert!(state.streams[&(-7, 301)].original_url.is_some());
    assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
}

#[test]
#[ignore = "requires a saved VK Fast session; only fetches playlist recommendations"]
fn native_playlist_mix_readonly() {
    let (profile, cookies) = crate::music_session_store::restore_profile()
        .unwrap()
        .unwrap();
    let client = WebAudioClient::new(cookies).unwrap();
    client.verify_owner(profile.id).unwrap();
    let canceled = AtomicBool::new(false);
    let playlists = client.playlists(profile.id, &canceled).unwrap();
    let reference = &playlists
        .iter()
        .find(|playlist| playlist.track_count.is_some_and(|count| count > 0))
        .expect("A nonempty saved playlist")
        .album;
    let mix = client
        .playlist_mix(profile.id, reference, &canceled)
        .unwrap();
    let next = client
        .playlist_mix(profile.id, reference, &canceled)
        .unwrap();
    println!(
        "playlist mix recommendations: first={}, next={}, has_more={}",
        mix.tracks.len(),
        next.tracks.len(),
        next.has_more
    );
    assert!(!mix.tracks.is_empty());
    client.resolve(mix.tracks[0].key()).unwrap();
    println!("playlist mix stream resolved without playback");
}
