//! Project only public music metadata into the optional Discord presence.
use super::*;
use crate::discord_rpc::{DiscordRpc, Snapshot, State as Playback};

const APPLICATION_ID: &str = match option_env!("VK_FAST_DISCORD_APPLICATION_ID") {
    Some(id) => id,
    None => "",
};

#[derive(Default)]
pub(super) struct State {
    service: Option<DiscordRpc>,
}

impl State {
    pub(super) fn detach(&mut self) {
        self.service = None;
    }

    pub(super) fn clear(&self) {
        if let Some(service) = &self.service {
            service.clear();
        }
    }
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
        let snapshot = self.discord_rpc_snapshot();
        #[cfg(not(test))]
        if self.discord_rpc.service.is_none() && snapshot.is_some() {
            self.discord_rpc.service = Some(DiscordRpc::new(APPLICATION_ID.into()));
        }
        #[cfg(test)]
        let _ = (APPLICATION_ID, DiscordRpc::new);
        // UI tests never connect to the user's real Discord client.
        if let Some(service) = &self.discord_rpc.service {
            match snapshot {
                Some(snapshot) => service.publish(snapshot),
                None => service.clear(),
            }
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
            PlaybackState::Paused => Playback::Paused,
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
            album: track
                .album
                .as_ref()
                .map(|album| album.title.clone())
                .filter(|title| !title.is_empty()),
            cover_url: track
                .cover_url
                .as_deref()
                .and_then(crate::model::valid_cover_url),
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
        track.cover_url = Some("https://images.example.test/cover.jpg".into());
        let title = track.display_title().into_owned();
        app.playing = Some(track);
        app.resume_position = Some(42.5);
        app.seek_position = Some(123.0); // Uncommitted UI drag must not change presence.
        let snapshot = app.discord_rpc_snapshot().unwrap();
        let exposed = format!(
            "{} {} {:?} {:?}",
            snapshot.title, snapshot.artist, snapshot.album, snapshot.cover_url
        );
        for secret in [
            "private-stream",
            "private-album",
            "access_token",
            "canonical-artist",
        ] {
            assert!(!exposed.contains(secret));
        }
        assert_eq!(snapshot.title, title);
        assert_eq!(snapshot.artist, "Artist");
        assert_eq!(snapshot.album.as_deref(), Some("Album"));
        assert_eq!(snapshot.position, 42.5);
        assert!(matches!(snapshot.state, Playback::Paused));
        assert_eq!(
            snapshot.cover_url.as_deref(),
            Some("https://images.example.test/cover.jpg")
        );
        app.playing.as_mut().unwrap().cover_url = Some("file:///private/avatar.jpg".into());
        assert!(app.discord_rpc_snapshot().unwrap().cover_url.is_none());
        app.resume_position = None;
        assert!(app.discord_rpc_snapshot().is_none());
        app.resume_position = Some(1.0);
        app.demo = true;
        assert!(app.discord_rpc_snapshot().is_none());
        app.demo = false;
        app.session = None;
        assert!(app.discord_rpc_snapshot().is_none());
        assert!(app.discord_rpc.service.is_none() && app.workers.is_empty());
    }
}
