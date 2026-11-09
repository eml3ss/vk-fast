//! Project only public music metadata into the optional Discord presence.
use super::*;
use crate::discord_rpc::{DiscordRpc, Snapshot, State as Playback};
use std::{io, sync::mpsc::SyncSender};

const APPLICATION_ID: &str = match option_env!("VK_FAST_DISCORD_APPLICATION_ID") {
    Some(id) => id,
    None => "",
};

fn track_page_key(track: &Track) -> Option<(i64, i64)> {
    let key = track
        .release_key
        .filter(|key| key.0 != 0 && key.1 > 0)
        .unwrap_or(track.key());
    (key.0 != 0 && key.1 > 0).then_some(key)
}

fn track_page_url(track: &Track) -> Option<String> {
    track_page_key(track).map(|key| format!("https://vk.ru/audio{}_{}", key.0, key.1))
}

fn album_page_url(track: &Track) -> Option<String> {
    let album = track.album.as_ref()?;
    (album.owner_id != 0 && album.id > 0)
        .then(|| format!("https://vk.ru/music/album/{}_{}", album.owner_id, album.id))
}

#[derive(Default)]
pub(super) struct State {
    service: Option<DiscordRpc>,
    links: LinkChecks,
}

impl State {
    pub(super) fn detach(&mut self) {
        self.links.invalidate();
        self.service = None;
    }

    pub(super) fn clear(&mut self) {
        self.links.invalidate();
        if let Some(service) = &self.service {
            service.clear();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinkTarget {
    account: i64,
    track: Option<(i64, i64)>,
    album: Option<(i64, i64)>,
}

impl LinkTarget {
    fn new(account: i64, track: &Track) -> Self {
        Self {
            account,
            track: track_page_key(track),
            album: track.album.as_ref().and_then(|album| {
                (album.owner_id != 0 && album.id > 0).then_some((album.owner_id, album.id))
            }),
        }
    }
}

struct Verification {
    track: Option<Result<bool, WebAudioError>>,
    album: Option<Result<bool, WebAudioError>>,
}

impl Verification {
    fn unknown(target: LinkTarget) -> Self {
        Self {
            track: target.track.map(|_| Err(WebAudioError::Network)),
            album: target.album.map(|_| Err(WebAudioError::Network)),
        }
    }
}

struct PendingVerification {
    target: LinkTarget,
    generation: u64,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Verification>,
}

impl Drop for PendingVerification {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Default)]
struct LinkChecks {
    target: Option<LinkTarget>,
    generation: u64,
    track: Option<bool>,
    album: Option<bool>,
    next_check: Option<Instant>,
    pending: Option<PendingVerification>,
}

impl LinkChecks {
    fn invalidate(&mut self) {
        self.target = None;
        self.track = None;
        self.album = None;
        self.next_check = None;
        self.generation = self.generation.wrapping_add(1);
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        // Keep a canceled worker's receiver until it finishes. Rapid changes
        // of track/account or RPC on/off must not spawn unbounded VK queries.
    }

    fn tick_with(
        &mut self,
        target: Option<LinkTarget>,
        now: Instant,
        launch: impl FnOnce(LinkTarget, Arc<AtomicBool>, SyncSender<Verification>) -> io::Result<()>,
    ) {
        if self.target != target {
            self.invalidate();
            self.target = target;
        }
        if let Some(pending) = &self.pending {
            let result = match pending.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Verification::unknown(pending.target)),
            };
            if let Some(result) = result {
                let current = self.target == Some(pending.target)
                    && self.generation == pending.generation
                    && !pending.cancel.load(Ordering::Acquire);
                self.pending = None;
                if current {
                    let mut failed = false;
                    for (proof, result) in [
                        (&mut self.track, result.track),
                        (&mut self.album, result.album),
                    ] {
                        match result {
                            Some(Ok(exists)) => *proof = Some(exists),
                            Some(Err(_)) => failed = true,
                            None => {}
                        }
                    }
                    // Transient failures preserve prior confirmation. An
                    // explicit missing page is the only negative proof.
                    self.next_check =
                        Some(now + Duration::from_secs(if failed { 60 } else { 300 }));
                }
            }
        }
        let Some(target) = self.target else { return };
        if self.pending.is_some()
            || self.next_check.is_some_and(|deadline| now < deadline)
            || (target.track.is_none() && target.album.is_none())
        {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending = Some(PendingVerification {
            target,
            generation: self.generation,
            cancel: cancel.clone(),
            receiver,
        });
        if launch(target, cancel, sender).is_err() {
            self.pending = None;
            self.next_check = Some(now + Duration::from_secs(60));
        }
    }

    fn apply(&self, snapshot: &mut Snapshot) {
        if self.track != Some(true) {
            snapshot.track_url = None;
        }
        if self.album != Some(true) {
            snapshot.album_url = None;
        }
    }
}

#[cfg(not(test))]
fn start_verification(
    client: WebAudioClient,
    target: LinkTarget,
    cancel: Arc<AtomicBool>,
    sender: SyncSender<Verification>,
    repaint: RepaintTarget,
) -> io::Result<()> {
    thread::Builder::new()
        .name("vk-discord-links".into())
        .spawn(move || {
            let track = target
                .track
                .map(|key| client.verify_track_page(target.account, key, &cancel));
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let album = target
                .album
                .map(|(owner, id)| client.verify_album_page(target.account, owner, id, &cancel));
            if !cancel.load(Ordering::Acquire) {
                let _ = sender.send(Verification { track, album });
                repaint.request_repaint();
            }
        })
        .map(drop)
}

impl MusicApp {
    pub(super) fn set_discord_rpc_enabled(&mut self, enabled: bool) {
        self.discord_rpc_enabled = enabled;
        self.discord_rpc_save_failed = !save_discord_preference(enabled);
        self.publish_discord_rpc();
    }

    pub(super) fn publish_discord_rpc(&mut self) {
        if !self.discord_rpc_enabled || self.demo {
            self.discord_rpc.detach();
            return;
        }
        if self.session.is_some()
            && matches!(self.playback_snapshot().state, PlaybackState::Loading)
        {
            // Buffering and an uncached seek are transient. Keep the previous
            // presence until committed playback resumes instead of clearing it.
            return;
        }
        let Some(mut snapshot) = self.discord_rpc_snapshot() else {
            // Pause clears activity, but retains this target's cached proof
            // and any in-flight check for an immediate, query-free resume.
            if let Some(service) = &self.discord_rpc.service {
                service.clear();
            }
            return;
        };
        let connection = self
            .session
            .as_ref()
            .and_then(|session| Some((session.profile.id, session.music.clone()?)));
        let target = self
            .playing
            .as_ref()
            .and_then(|track| Some(LinkTarget::new(connection.as_ref()?.0, track)));
        #[cfg(not(test))]
        {
            let repaint = self.repaint_target.clone();
            self.discord_rpc.links.tick_with(
                target,
                Instant::now(),
                move |target, cancel, sender| {
                    let client = connection
                        .as_ref()
                        .ok_or_else(|| io::Error::other("Music connection unavailable"))?
                        .1
                        .clone();
                    start_verification(client, target, cancel, sender, repaint)
                },
            );
        }
        #[cfg(test)]
        self.discord_rpc
            .links
            .tick_with(target, Instant::now(), |_, _, _| {
                // UI tests never query VK or connect to real Discord.
                Err(io::Error::other("Synthetic UI does not verify VK pages"))
            });
        self.discord_rpc.links.apply(&mut snapshot);
        #[cfg(not(test))]
        if self.discord_rpc.service.is_none() {
            self.discord_rpc.service = Some(DiscordRpc::new(APPLICATION_ID.into()));
        }
        #[cfg(test)]
        let _ = (APPLICATION_ID, DiscordRpc::new);
        // UI tests never connect to the user's real Discord client.
        if let Some(service) = &self.discord_rpc.service {
            service.publish(snapshot);
        }
    }

    fn discord_rpc_snapshot(&self) -> Option<Snapshot> {
        if self.demo || self.session.is_none() {
            return None;
        }
        let track = self.playing.as_ref()?;
        let playback = self.playback_snapshot();
        let state = match playback.state {
            PlaybackState::Playing => Playback::Playing,
            _ => return None,
        };
        Some(Snapshot {
            title: track.display_title().into_owned(),
            artist: if track.artists.is_empty() {
                track.artist.clone()
            } else {
                track
                    .artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            album: self.player_album(track),
            cover_url: track
                .cover_url
                .as_deref()
                .and_then(crate::discord_rpc::public_cover)
                .or_else(|| {
                    track
                        .album
                        .as_ref()?
                        .cover_url
                        .as_deref()
                        .and_then(crate::discord_rpc::public_cover)
                }),
            track_url: track_page_url(track),
            album_url: album_page_url(track),
            duration: f64::from(track.duration),
            position: bounded_track_position(playback.position, track.duration),
            state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{page, search_test_app};

    fn candidate_snapshot() -> Snapshot {
        Snapshot {
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            cover_url: Some("https://sun9.vkuserphoto.ru/synthetic/cover.jpg".into()),
            track_url: Some("https://vk.ru/audio-200_123".into()),
            album_url: Some("https://vk.ru/music/album/-5_9".into()),
            duration: 180.0,
            position: 12.0,
            state: Playback::Playing,
        }
    }

    fn target(account: i64) -> LinkTarget {
        LinkTarget {
            account,
            track: Some((-200, 123)),
            album: Some((-5, 9)),
        }
    }

    fn fake_job(
        checks: &mut LinkChecks,
        target: LinkTarget,
        now: Instant,
    ) -> (Arc<AtomicBool>, SyncSender<Verification>) {
        let mut job = None;
        checks.tick_with(Some(target), now, |actual, cancel, sender| {
            assert_eq!(actual, target);
            job = Some((cancel, sender));
            Ok(())
        });
        job.expect("Expected one verification job")
    }

    fn poll_only(checks: &mut LinkChecks, target: LinkTarget, now: Instant) {
        checks.tick_with(Some(target), now, |_, _, _| {
            panic!("Unexpected VK verification")
        });
    }

    #[test]
    fn rpc_links_require_exists_proof_and_transient_errors_preserve_previous_confirmation() {
        let now = Instant::now();
        let target = target(42);
        let mut checks = LinkChecks::default();
        let (_, sender) = fake_job(&mut checks, target, now);
        let original = candidate_snapshot();
        let mut snapshot = original.clone();
        checks.apply(&mut snapshot);
        assert!(snapshot.track_url.is_none() && snapshot.album_url.is_none());
        assert_eq!(snapshot.title, original.title);
        assert_eq!(snapshot.cover_url, original.cover_url);
        sender
            .send(Verification {
                track: Some(Ok(true)),
                album: Some(Ok(false)),
            })
            .unwrap();
        poll_only(&mut checks, target, now + Duration::from_secs(1));
        let mut snapshot = original.clone();
        checks.apply(&mut snapshot);
        assert_eq!(snapshot.track_url, original.track_url);
        assert!(snapshot.album_url.is_none());
        poll_only(&mut checks, target, now + Duration::from_secs(299));

        let (_, retry) = fake_job(&mut checks, target, now + Duration::from_secs(302));
        retry
            .send(Verification {
                track: Some(Err(WebAudioError::Network)),
                album: Some(Err(WebAudioError::Timeout)),
            })
            .unwrap();
        poll_only(&mut checks, target, now + Duration::from_secs(303));
        let mut snapshot = original.clone();
        checks.apply(&mut snapshot);
        assert_eq!(snapshot.track_url, original.track_url);
        assert!(snapshot.album_url.is_none());
        poll_only(&mut checks, target, now + Duration::from_secs(362));

        let (_, retry) = fake_job(&mut checks, target, now + Duration::from_secs(364));
        retry
            .send(Verification {
                track: Some(Ok(false)),
                album: Some(Ok(true)),
            })
            .unwrap();
        poll_only(&mut checks, target, now + Duration::from_secs(365));
        let mut snapshot = original.clone();
        checks.apply(&mut snapshot);
        assert!(snapshot.track_url.is_none());
        assert_eq!(snapshot.album_url, original.album_url);
    }

    #[test]
    fn rpc_target_changes_and_detach_cancel_and_discard_stale_proofs_without_parallel_queries() {
        let now = Instant::now();
        let first = target(42);
        let second = target(99);
        let mut checks = LinkChecks::default();
        let (canceled, old) = fake_job(&mut checks, first, now);
        poll_only(&mut checks, second, now);
        assert!(canceled.load(Ordering::Acquire));
        let mut snapshot = candidate_snapshot();
        checks.apply(&mut snapshot);
        assert!(snapshot.track_url.is_none() && snapshot.album_url.is_none());
        old.send(Verification {
            track: Some(Ok(true)),
            album: Some(Ok(true)),
        })
        .unwrap();
        let (canceled, next) = fake_job(&mut checks, second, now);
        assert!(checks.track.is_none() && checks.album.is_none());
        checks.invalidate();
        assert!(canceled.load(Ordering::Acquire));
        poll_only(&mut checks, second, now);
        next.send(Verification {
            track: Some(Ok(true)),
            album: Some(Ok(true)),
        })
        .unwrap();
        let (_, current) = fake_job(&mut checks, second, now);
        assert!(checks.track.is_none() && checks.album.is_none());
        current
            .send(Verification {
                track: Some(Ok(false)),
                album: Some(Err(WebAudioError::Format)),
            })
            .unwrap();
        poll_only(&mut checks, second, now);
        assert_eq!(checks.track, Some(false));
        assert!(checks.album.is_none());
        let changed = LinkTarget {
            track: Some((-200, 124)),
            album: Some((-5, 10)),
            ..second
        };
        let _ = fake_job(&mut checks, changed, now);
        assert!(checks.track.is_none() && checks.album.is_none());
    }

    #[test]
    fn rpc_verification_target_uses_canonical_release_and_never_access_keys() {
        let mut track = page(1, &[7]).items.remove(0);
        track.release_key = Some((-200, 123));
        track.album = Some(AlbumRef {
            owner_id: -5,
            id: 9,
            title: "Album".into(),
            access_key: Some("private".into()),
            cover_url: None,
        });
        assert_eq!(LinkTarget::new(42, &track), target(42));
        track.id += 1;
        track.owner_id = 99;
        track.album.as_mut().unwrap().access_key = Some("another-private-key".into());
        assert_eq!(LinkTarget::new(42, &track), target(42));
    }

    #[test]
    fn pause_and_resume_keep_confirmed_links_without_starting_another_verification() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        app.discord_rpc_enabled = true;
        let mut track = page(1, &[7]).items.remove(0);
        track.release_key = Some((-200, 123));
        track.album = Some(AlbumRef {
            owner_id: -5,
            id: 9,
            title: "Album".into(),
            access_key: None,
            cover_url: None,
        });
        let account = app.session.as_ref().unwrap().profile.id;
        app.session.as_mut().unwrap().music = Some(
            WebAudioClient::new(vec![SessionCookie {
                domain: ".vk.ru".into(),
                path: "/".into(),
                name: "remixsid6".into(),
                value: "synthetic".into(),
                secure: true,
                expires_unix: None,
            }])
            .unwrap(),
        );
        let now = Instant::now();
        let target = LinkTarget::new(account, &track);
        let (_, sender) = fake_job(&mut app.discord_rpc.links, target, now);
        sender
            .send(Verification {
                track: Some(Ok(true)),
                album: Some(Ok(true)),
            })
            .unwrap();
        poll_only(&mut app.discord_rpc.links, target, now);
        let generation = app.discord_rpc.links.generation;
        app.playing = Some(track);
        let (paused, _control) = Player::test_paused(180, 12.0);
        app.player = paused;
        app.publish_discord_rpc();
        assert_eq!(app.discord_rpc.links.generation, generation);
        assert_eq!(app.discord_rpc.links.track, Some(true));
        assert_eq!(app.discord_rpc.links.album, Some(true));
        let (playing, _control) = Player::test_playing(180, 12.0);
        app.player = playing;
        app.publish_discord_rpc();
        assert_eq!(app.discord_rpc.links.generation, generation);
        assert_eq!(app.discord_rpc.links.track, Some(true));
        assert_eq!(app.discord_rpc.links.album, Some(true));
        assert!(app.discord_rpc.links.pending.is_none());
        assert!(app.discord_rpc.service.is_none() && app.workers.is_empty());
    }

    #[test]
    fn vk_photo_artwork_reaches_the_rpc_payload_with_album_fallback_and_pause_clears_it() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        let (player, _playing) = Player::test_playing(180, 12.0);
        app.player = player;
        let track_cover = "https://sun9-85.vkuserphoto.ru/synthetic/track.jpg?quality=96&sign=synthetic&size=600x600&type=album&from=bu";
        let album_cover = "https://sun1-15.vkuserphoto.ru/synthetic/album.jpg?signature=synthetic&blur=0&type=album";
        let mut track = page(1, &[7]).items.remove(0);
        track.cover_url = Some(track_cover.into());
        track.release_key = Some((-200, 123));
        track.artists = vec![
            ArtistRef {
                id: "artist-main".into(),
                name: "Main Artist".into(),
            },
            ArtistRef {
                id: "artist-guest".into(),
                name: "Guest".into(),
            },
        ];
        track.album = Some(AlbumRef {
            id: 9,
            owner_id: -5,
            title: "Album".into(),
            access_key: Some("private-access-key".into()),
            cover_url: Some(album_cover.into()),
        });
        app.playing = Some(track);
        let payload =
            crate::discord_rpc::test_activity(app.discord_rpc_snapshot().unwrap()).unwrap();
        assert_eq!(payload["assets"]["large_image"], track_cover);
        assert_eq!(payload["details_url"], "https://vk.ru/audio-200_123");
        assert!(payload.get("state_url").is_none());
        assert_eq!(
            payload["assets"]["large_url"],
            "https://vk.ru/music/album/-5_9"
        );

        for rejected in [
            None,
            Some("file:///private/cover.jpg"),
            Some("https://sun9-85.vkuserphoto.ru/synthetic/cover.jpg?access_token=private"),
        ] {
            app.playing.as_mut().unwrap().cover_url = rejected.map(str::to_owned);
            let payload =
                crate::discord_rpc::test_activity(app.discord_rpc_snapshot().unwrap()).unwrap();
            assert_eq!(payload["assets"]["large_image"], album_cover);
        }
        let (paused, _paused) = Player::test_paused(180, 12.0);
        app.player = paused;
        assert!(app.discord_rpc_snapshot().is_none());
    }

    #[test]
    fn single_placeholder_uses_the_same_release_title_in_presence_and_system_media() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        let mut track = page(1, &[7]).items.remove(0);
        track.title = "Сингл (Slowed)".into();
        track.subtitle = Some("Slowed".into());
        track.album = Some(AlbumRef {
            owner_id: -5,
            id: 9,
            title: "Unnamed".into(),
            access_key: None,
            cover_url: None,
        });
        app.playing = Some(track);
        let (player, _commands) = Player::test_playing(180, 42.5);
        app.player = player;
        for (source, expected) in [
            ("Unnamed", "Сингл"),
            (" unnamed ", "Сингл"),
            ("UNNAMED", "Сингл"),
            ("Official single", "Official single"),
            ("Unnamed Stories", "Unnamed Stories"),
        ] {
            app.playing.as_mut().unwrap().album.as_mut().unwrap().title = source.into();
            let snapshot = app.discord_rpc_snapshot().unwrap();
            assert_eq!(snapshot.album.as_deref(), Some(expected));
            assert_eq!(
                app.system_media_snapshot().track.unwrap().album.as_deref(),
                Some(expected)
            );
            let payload = crate::discord_rpc::test_activity(snapshot).unwrap();
            assert_eq!(payload["assets"]["large_text"], expected);
            assert_eq!(
                app.playing.as_ref().unwrap().album.as_ref().unwrap().title,
                source
            );
        }
        app.playing.as_mut().unwrap().album = None;
        assert!(app.discord_rpc_snapshot().unwrap().album.is_none());
        assert!(app.system_media_snapshot().track.unwrap().album.is_none());
        assert!(app.workers.is_empty() && app.discord_rpc.service.is_none());
    }

    #[test]
    fn presence_uses_committed_music_metadata_and_never_exposes_session_or_streams() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        let mut track = page(1, &[7]).items.remove(0);
        track.title = "Song".into();
        track.subtitle = Some("Slowed".into());
        track.artist = "Fallback".into();
        track.artists = vec![ArtistRef {
            id: "canonical-artist".into(),
            name: "Artist".into(),
        }];
        track.duration = 180;
        track.url = Some("https://stream.example.test/private?access_token=private-stream".into());
        track.album = Some(AlbumRef {
            id: 9,
            owner_id: -5,
            title: "Album".into(),
            access_key: Some("private-album".into()),
            cover_url: None,
        });
        track.cover_url = Some("https://sun9-85.vkuserphoto.ru/synthetic/cover.jpg?quality=96&sign=synthetic&size=600x600&type=album".into());
        let cover = track.cover_url.clone();
        let title = track.display_title().into_owned();
        app.playing = Some(track);
        let (player, _control) = Player::test_playing(180, 42.5);
        app.player = player;
        app.seek_position = Some(123.0); // Uncommitted UI drag must not change presence.
        let snapshot = app.discord_rpc_snapshot().unwrap();
        let exposed = format!("{snapshot:?}");
        for secret in ["private-stream", "private-album", "access_token"] {
            assert!(!exposed.contains(secret));
        }
        assert_eq!(snapshot.title, title);
        assert_eq!(snapshot.artist, "Artist");
        assert_eq!(snapshot.album.as_deref(), Some("Album"));
        assert_eq!(snapshot.position, 42.5);
        assert!(matches!(snapshot.state, Playback::Playing));
        assert_eq!(snapshot.cover_url.as_deref(), cover.as_deref());
        app.playing.as_mut().unwrap().cover_url = Some("file:///private/avatar.jpg".into());
        assert!(app.discord_rpc_snapshot().unwrap().cover_url.is_none());
        app.resume_position = Some(42.5); // A logical pause also clears the RPC.
        assert!(app.discord_rpc_snapshot().is_none());
        app.resume_position = None;
        app.demo = true;
        assert!(app.discord_rpc_snapshot().is_none());
        app.demo = false;
        app.session = None;
        assert!(app.discord_rpc_snapshot().is_none());
        assert!(app.discord_rpc.service.is_none() && app.workers.is_empty());
    }
}
