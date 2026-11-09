//! "Удалить из плейлиста": the row leaves the open playlist at once and a
//! notice offers Undo. VK is asked only after the notice ends, so Undo just
//! puts the row back and nothing is sent.
use super::*;

/// How long the removal notice, and with it Undo, stays.
const UNDO_TIME: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct Removal {
    owner: i64,
    playlist: AlbumRef,
    track: Track,
    index: usize,
    pub(super) until: Instant,
}

pub(super) struct Request {
    removal: Removal,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<(), WebAudioError>>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) pending: Option<Removal>,
    request: Option<Request>,
}

impl State {
    /// Account changes drop an unsent removal and cancel a sent one.
    pub(super) fn reset(&mut self) {
        self.pending = None;
        if let Some(request) = self.request.take() {
            request.canceled.store(true, Ordering::Release);
        }
    }
}

impl MusicApp {
    /// The open playlist a row can be removed from: the account's own or
    /// one VK lets it edit.
    pub(super) fn removable_playlist(&self, track: &Track) -> Option<AlbumRef> {
        if self.demo {
            return None;
        }
        let MusicPage::Playlist(reference) = &self.page else {
            return None;
        };
        if self.own_playlist().is_none() && !self.can_edit_playlist(reference) {
            return None;
        }
        let page = self.details.get(&self.page.detail_key()?)?;
        let DetailPage::Playlist(data) = page.as_ref() else {
            return None;
        };
        data.tracks
            .iter()
            .any(|row| row.key() == track.key())
            .then(|| reference.clone())
    }

    pub(super) fn remove_from_playlist(
        &mut self,
        ctx: &egui::Context,
        track: &Track,
        playlist: AlbumRef,
    ) {
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        // An earlier removal is sent now; this one gets the notice.
        self.send_playlist_removal();
        let Some(index) = self.playlist_rows(&playlist, |tracks| {
            let index = tracks.iter().position(|row| row.key() == track.key())?;
            tracks.remove(index);
            Some(index)
        }) else {
            return;
        };
        let until = Instant::now() + UNDO_TIME;
        self.playlist_removal.pending = Some(Removal {
            owner,
            playlist,
            track: track.clone(),
            index,
            until,
        });
        self.track_menu_notice = Some((
            self.language
                .text("Трек удалён из плейлиста", "Removed from playlist")
                .into(),
            until,
        ));
        ctx.request_repaint_after(UNDO_TIME);
    }

    /// Undo in the notice: the row returns to its place, VK is never asked.
    pub(super) fn undo_playlist_removal(&mut self) {
        if let Some(removal) = self.playlist_removal.pending.take() {
            self.restore_playlist_row(&removal);
        }
    }

    /// Sends the removal once its notice has ended. Runs without the window.
    pub(super) fn poll_playlist_removal(&mut self, ctx: &egui::Context) {
        if let Some(request) = &self.playlist_removal.request {
            let result = match request.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(WebAudioError::Network)),
            };
            if let Some(result) = result {
                let request = self.playlist_removal.request.take().unwrap();
                let current = self.session.as_ref().map(|session| session.profile.id)
                    == Some(request.removal.owner);
                if current && !request.canceled.load(Ordering::Acquire) {
                    match result {
                        Ok(()) => self.load_playlists(ctx, true),
                        Err(WebAudioError::Canceled) => {}
                        Err(error) => {
                            self.restore_playlist_row(&request.removal);
                            self.track_menu_error = Some(error);
                        }
                    }
                }
            }
        }
        if self
            .playlist_removal
            .pending
            .as_ref()
            .is_some_and(|removal| Instant::now() >= removal.until)
        {
            self.send_playlist_removal();
        }
    }

    /// Sends the waiting removal now, unless another one is still on its way;
    /// the poll above sends it after that one.
    pub(super) fn send_playlist_removal(&mut self) {
        if self.playlist_removal.request.is_some() {
            return;
        }
        let Some(removal) = self.playlist_removal.pending.take() else {
            return;
        };
        let client = self
            .session
            .as_ref()
            .filter(|session| session.profile.id == removal.owner)
            .and_then(|session| session.music.clone());
        let Some(client) = client else {
            self.restore_playlist_row(&removal);
            return;
        };
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.clone();
        let (owner, playlist, key) = (removal.owner, removal.playlist.clone(), removal.track.key());
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-playlist-remove".into())
            .spawn(move || {
                let result = client.remove_from_playlist(owner, &playlist, key, &worker_cancel);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_removal.request = Some(Request {
                    removal,
                    canceled,
                    receiver,
                });
            }
            Err(_) => {
                self.restore_playlist_row(&removal);
                self.track_menu_error = Some(WebAudioError::Network);
            }
        }
    }

    fn restore_playlist_row(&mut self, removal: &Removal) {
        self.playlist_rows(&removal.playlist, |tracks| {
            if tracks.iter().any(|row| row.key() == removal.track.key()) {
                return None;
            }
            tracks.insert(removal.index.min(tracks.len()), removal.track.clone());
            Some(())
        });
    }

    /// Edits the rows of a loaded playlist page and its reported count.
    fn playlist_rows<T>(
        &mut self,
        playlist: &AlbumRef,
        edit: impl FnOnce(&mut Vec<Track>) -> Option<T>,
    ) -> Option<T> {
        let key = MusicPage::Playlist(playlist.clone()).detail_key()?;
        let page = self.details.get_mut(&key)?;
        let DetailPage::Playlist(data) = Arc::make_mut(page) else {
            return None;
        };
        let before = data.tracks.len();
        let result = edit(&mut data.tracks)?;
        let after = data.tracks.len();
        if let Some(total) = &mut data.total_tracks {
            *total = (*total + after).saturating_sub(before);
        }
        self.track_view_cache.invalidate();
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{page, search_test_app};
    use crate::web_audio::{AlbumPage, AlbumStats};

    fn open_playlist(app: &mut MusicApp, ids: &[i64]) -> AlbumRef {
        app.demo = false;
        let album = AlbumRef {
            owner_id: 42,
            id: 7,
            title: "Mine".into(),
            access_key: None,
            cover_url: None,
        };
        app.page = MusicPage::Playlist(album.clone());
        app.details.insert(
            app.page.detail_key().unwrap(),
            Arc::new(DetailPage::Playlist(AlbumPage {
                album: album.clone(),
                artists: Vec::new(),
                year: None,
                description: None,
                creator: None,
                tracks: page(ids.len(), ids).items,
                total_tracks: Some(ids.len()),
                stats: AlbumStats::default(),
            })),
        );
        album
    }

    fn rows(app: &MusicApp) -> (Vec<i64>, Option<usize>) {
        let DetailPage::Playlist(data) = app.details[&app.page.detail_key().unwrap()].as_ref()
        else {
            panic!()
        };
        (
            data.tracks.iter().map(|track| track.id).collect(),
            data.total_tracks,
        )
    }

    #[test]
    fn removal_waits_for_the_notice_and_undo_puts_the_row_back_without_vk() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let album = open_playlist(&mut app, &[1, 2, 3]);
        let track = page(3, &[1, 2, 3]).items.remove(1);
        assert_eq!(app.removable_playlist(&track), Some(album.clone()));
        app.remove_from_playlist(&ctx, &track, album.clone());
        assert_eq!(rows(&app), (vec![1, 3], Some(2)));
        let until = app.playlist_removal.pending.as_ref().unwrap().until;
        assert_eq!(app.track_menu_notice.as_ref().unwrap().1, until);
        assert!(app.removable_playlist(&track).is_none());
        // Polling before the deadline sends nothing.
        app.poll_playlist_removal(&ctx);
        assert!(app.playlist_removal.request.is_none() && app.workers.is_empty());
        app.undo_playlist_removal();
        assert_eq!(rows(&app), (vec![1, 2, 3], Some(3)));
        assert!(app.playlist_removal.pending.is_none());
        assert!(app.workers.is_empty(), "Undo never reaches VK");
    }

    #[test]
    fn failed_or_signed_out_removals_return_the_row_and_reset_drops_them() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let album = open_playlist(&mut app, &[1, 2, 3]);
        let track = page(3, &[1, 2, 3]).items.remove(0);
        app.remove_from_playlist(&ctx, &track, album.clone());
        let removal = app.playlist_removal.pending.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err(WebAudioError::Network)).ok().unwrap();
        app.playlist_removal.request = Some(Request {
            removal: removal.clone(),
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.poll_playlist_removal(&ctx);
        assert_eq!(rows(&app), (vec![1, 2, 3], Some(3)));
        assert_eq!(app.track_menu_error, Some(WebAudioError::Network));
        // The test session has no VK client: sending restores the row.
        app.remove_from_playlist(&ctx, &track, album);
        app.playlist_removal.pending.as_mut().unwrap().until = Instant::now();
        app.poll_playlist_removal(&ctx);
        assert_eq!(rows(&app), (vec![1, 2, 3], Some(3)));
        app.playlist_removal.pending = Some(removal);
        app.reset();
        assert!(app.playlist_removal.pending.is_none());
    }
}
