//! The track that plays next is readied while the current one plays, when
//! volume normalization is on: its audio is cached and its loudness
//! measured ahead, so it starts at once at its normalized level instead of
//! settling seconds into the track.
use super::*;

/// How far into the current track the next one is readied: its own download
/// has usually finished by then, so the two do not share the connection.
const READY_AFTER: f64 = 20.0;

/// The next track's address being asked of VK, and how to cancel it.
type AddressRequest = (Arc<AtomicBool>, Receiver<Result<String, WebAudioError>>);

pub(super) struct NextPreload {
    key: (i64, i64),
    request: Option<AddressRequest>,
}

impl Drop for NextPreload {
    fn drop(&mut self) {
        if let Some((canceled, _)) = &self.request {
            canceled.store(true, Ordering::Release);
        }
    }
}

impl MusicApp {
    /// The next track to ready now, if any.
    fn preload_candidate(&self) -> Option<Track> {
        if self.demo || !self.player.normalize_volume() {
            return None;
        }
        let playing = self.playing.as_ref()?;
        let snapshot = self.player.snapshot();
        if !matches!(
            snapshot.state,
            PlaybackState::Playing | PlaybackState::Paused
        ) {
            return None;
        }
        let duration = f64::from(playing.duration);
        if snapshot.position < READY_AFTER.min(duration * 0.5) {
            return None;
        }
        let next = self.adjacent_track(true)?;
        if next == playing.key() {
            return None;
        }
        self.play_queue
            .iter()
            .flatten()
            .chain(self.library.tracks.iter())
            .chain(self.queued_tracks.get(&next))
            .find(|track| track.key() == next)
            .filter(|track| track.unavailable != Some(true) && track.duration > 0)
            .cloned()
    }

    /// Runs with the background tick, without UI frames too.
    pub(super) fn preload_next_track(&mut self, ctx: &egui::Context) {
        let Some(next) = self.preload_candidate() else {
            // Nothing to ready, or no longer this one.
            if self
                .next_preload
                .as_ref()
                .is_some_and(|preload| preload.request.is_some())
            {
                self.next_preload = None;
            }
            return;
        };
        if let Some(preload) = &mut self.next_preload
            && preload.key == next.key()
        {
            let Some((_, receiver)) = &preload.request else {
                return;
            };
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
            };
            preload.request = None;
            match (result, &self.session) {
                (Ok(url), Some(session)) => {
                    crate::diagnostics::event("playback", "next track address ready");
                    self.player.preload(
                        CacheKey {
                            account_id: session.profile.id,
                            owner_id: next.owner_id,
                            track_id: next.id,
                            duration: next.duration,
                        },
                        url,
                    );
                }
                (Err(error), _) => crate::diagnostics::event(
                    "playback",
                    &format!("next track address failed error={error:?}"),
                ),
                _ => {}
            }
            return;
        }
        let key = next.key();
        if cfg!(test) {
            // Tests never ask VK; nothing answers this request.
            let (_, receiver) = mpsc::sync_channel(1);
            self.next_preload = Some(NextPreload {
                key,
                request: Some((Arc::new(AtomicBool::new(false)), receiver)),
            });
            return;
        }
        let Some(music) = self
            .session
            .as_ref()
            .and_then(|session| session.music.clone())
        else {
            return;
        };
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-resolve-audio".into())
            .spawn(move || {
                let _ = sender.send(music.resolve_cached(key, &worker_cancel));
                repaint.request_repaint();
            })
        {
            self.workers.push(worker);
            self.next_preload = Some(NextPreload {
                key,
                request: Some((canceled, receiver)),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{page, search_test_app};

    fn playing_first_of_two(ctx: &egui::Context, position: f64) -> (MusicApp, impl Send) {
        let mut app = search_test_app(ctx);
        app.demo = false;
        let mut queue = page(2, &[91, 92]).items;
        for track in &mut queue {
            track.duration = 180;
        }
        app.playing = Some(queue[0].clone());
        app.selected = app.playing.as_ref().map(Track::key);
        app.play_queue = Some(queue);
        app.queue_origin = Some("playlist:42_9".into());
        app.synchronize_order();
        let (player, control) = crate::playback::Player::test_playing(180, position);
        app.player = player;
        (app, control)
    }

    #[test]
    fn the_next_track_is_readied_only_with_normalization_and_after_the_first_seconds() {
        let ctx = egui::Context::default();
        let (mut app, _control) = playing_first_of_two(&ctx, 30.0);
        let next = app.play_queue.as_ref().unwrap()[1].key();
        // Normalization off: nothing is readied.
        app.preload_next_track(&ctx);
        assert!(app.next_preload.is_none());
        app.player.set_normalize_volume(true);
        app.preload_next_track(&ctx);
        assert_eq!(
            app.next_preload.as_ref().map(|preload| preload.key),
            Some(next)
        );
        assert!(app.workers.is_empty(), "tests never ask VK");

        // Early in the track, the current download still has the connection.
        let (mut app, _control) = playing_first_of_two(&ctx, 5.0);
        app.player.set_normalize_volume(true);
        app.preload_next_track(&ctx);
        assert!(app.next_preload.is_none());

        // The last track has no next one to ready.
        let (mut app, _control) = playing_first_of_two(&ctx, 30.0);
        app.player.set_normalize_volume(true);
        app.playing = app.play_queue.as_ref().unwrap().get(1).cloned();
        app.selected = app.playing.as_ref().map(Track::key);
        app.synchronize_order();
        app.preload_next_track(&ctx);
        assert!(app.next_preload.is_none());

        // Demo never readies anything.
        let (mut app, _control) = playing_first_of_two(&ctx, 30.0);
        app.player.set_normalize_volume(true);
        app.demo = true;
        app.preload_next_track(&ctx);
        assert!(app.next_preload.is_none());
    }
}
