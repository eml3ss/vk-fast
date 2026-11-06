//! Library subscriptions and their worker lifecycle, separate from track lists.
use super::*;
use crate::web_audio::FollowedArtist;

#[derive(Default)]
pub(super) struct State {
    pub(super) sidebar: playlist_ui::SearchState,
    pub(super) artists: Vec<FollowedArtist>,
    pub(super) error: Option<WebAudioError>,
    requested: bool,
    pending: Option<Request>,
    follow: Option<FollowRequest>,
    refresh_due: bool,
}
struct Request {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Vec<FollowedArtist>, WebAudioError>>,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}
struct FollowRequest {
    owner: i64,
    item: FollowedArtist,
    follow: bool,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<(), WebAudioError>>,
}
impl Drop for FollowRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}
impl State {
    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        Self {
            requested: true,
            ..Default::default()
        }
    }

    pub(super) fn loading(&self) -> bool {
        self.pending.is_some() || !self.requested
    }
}

impl MusicApp {
    pub(super) fn load_library_artists(&mut self, ctx: &egui::Context, refresh: bool) {
        if self.library_browser.pending.is_some()
            || (!refresh && self.library_browser.requested)
            || !self.playlist_ready()
        {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        self.library_browser.requested = true;
        self.library_browser.error = None;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-library-artists".into())
            .spawn(move || {
                let result = client.followed_artists(owner, &worker_cancel);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.library_browser.pending = Some(Request {
                    owner,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.library_browser.error = Some(WebAudioError::Network),
        }
    }

    pub(super) fn artist_follow_view(&self, artist: &ArtistRef) -> Option<detail_ui::FollowView> {
        let session = self.session.as_ref()?;
        let saved = session
            .music
            .as_ref()?
            .artist_followed(session.profile.id, artist)?;
        Some(detail_ui::FollowView {
            saved,
            busy: self.library_browser.follow.is_some(),
        })
    }

    pub(super) fn toggle_artist_follow(&mut self, ctx: &egui::Context, artist: ArtistRef) {
        let Some(view) = self.artist_follow_view(&artist).filter(|view| !view.busy) else {
            return;
        };
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let image_url = self.details.values().find_map(|page| match page.as_ref() {
            DetailPage::Artist(page) if page.artist.id == artist.id => page.image_url.clone(),
            _ => None,
        });
        let item = FollowedArtist { artist, image_url };
        let worker_artist = item.artist.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-artist-follow".into())
            .spawn(move || {
                let result =
                    client.set_artist_follow(owner, &worker_artist, !view.saved, &worker_cancel);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.library_browser.follow = Some(FollowRequest {
                    owner,
                    item,
                    follow: !view.saved,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.track_menu_error = Some(WebAudioError::Network),
        }
    }

    pub(super) fn poll_library_artists(&mut self, ctx: &egui::Context) {
        let result = self.library_browser.pending.as_ref().and_then(|request| {
            match request.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(WebAudioError::Network)),
            }
        });
        if let Some(result) = result {
            let request = self.library_browser.pending.take().unwrap();
            if !request.canceled.load(Ordering::Acquire)
                && self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.profile.id == request.owner)
            {
                match result {
                    Ok(mut items) => {
                        let previous: HashMap<_, _> = self
                            .library_browser
                            .artists
                            .iter()
                            .filter_map(|item| {
                                item.image_url
                                    .as_ref()
                                    .map(|image| (item.artist.id.as_str(), image))
                            })
                            .collect();
                        for item in &mut items {
                            if item.image_url.is_none() {
                                item.image_url =
                                    previous.get(item.artist.id.as_str()).cloned().cloned();
                            }
                        }
                        self.library_browser.artists = items;
                        self.library_browser.error = None;
                    }
                    Err(WebAudioError::Canceled) => {}
                    Err(error) => self.library_browser.error = Some(error),
                }
                ctx.request_repaint();
            }
        }
        let result = self.library_browser.follow.as_ref().and_then(|request| {
            match request.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(WebAudioError::Network)),
            }
        });
        if let Some(result) = result {
            let request = self.library_browser.follow.take().unwrap();
            if !request.canceled.load(Ordering::Acquire)
                && self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.profile.id == request.owner)
            {
                match result {
                    Ok(()) => {
                        self.library_browser
                            .artists
                            .retain(|item| item.artist.id != request.item.artist.id);
                        if request.follow {
                            self.library_browser.artists.insert(0, request.item.clone());
                        }
                        self.library_browser.refresh_due = true;
                    }
                    Err(WebAudioError::Canceled) => {}
                    Err(error) => self.track_menu_error = Some(error),
                }
                ctx.request_repaint();
            }
        }
        if self.library_browser.refresh_due && self.library_browser.pending.is_none() {
            self.library_browser.refresh_due = false;
            self.load_library_artists(ctx, true);
        } else {
            #[cfg(not(test))]
            self.load_library_artists(ctx, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::search_test_app;

    fn artist() -> FollowedArtist {
        FollowedArtist {
            artist: ArtistRef {
                id: "subscribed".into(),
                name: "Subscribed".into(),
            },
            image_url: Some("https://images.example.test/artist.jpg".into()),
        }
    }
    fn reply(owner: i64, result: Result<Vec<FollowedArtist>, WebAudioError>) -> Request {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        Request {
            owner,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    #[test]
    fn subscription_replies_preserve_previous_data_and_artwork_on_failures_and_reject_other_accounts()
     {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library_browser.pending = Some(reply(42, Ok(vec![artist()])));
        app.poll_library_artists(&ctx);
        assert_eq!(app.library_browser.artists.len(), 1);
        app.library_browser.pending = Some(reply(42, Err(WebAudioError::Network)));
        app.poll_library_artists(&ctx);
        assert_eq!(app.library_browser.artists[0].artist.id, "subscribed");
        assert!(matches!(
            app.library_browser.error,
            Some(WebAudioError::Network)
        ));
        let mut updated = artist();
        updated.image_url = None;
        app.library_browser.pending = Some(reply(42, Ok(vec![updated])));
        app.poll_library_artists(&ctx);
        assert_eq!(app.library_browser.artists[0].image_url, artist().image_url);
        app.library_browser.pending = Some(reply(99, Ok(vec![])));
        app.poll_library_artists(&ctx);
        assert_eq!(app.library_browser.artists.len(), 1);
        assert!(app.workers.is_empty());
    }

    #[test]
    fn confirmed_follow_adds_one_artist_and_reset_cancels_the_remaining_request() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Ok(())).unwrap();
        app.library_browser.follow = Some(FollowRequest {
            owner: 42,
            item: artist(),
            follow: true,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        let (_sender, receiver) = mpsc::sync_channel(1);
        let canceled = Arc::new(AtomicBool::new(false));
        app.library_browser.pending = Some(Request {
            owner: 42,
            canceled: canceled.clone(),
            receiver,
        });
        app.poll_library_artists(&ctx);
        assert_eq!(app.library_browser.artists.len(), 1);
        assert!(app.library_browser.refresh_due && app.library_browser.pending.is_some());
        app.reset();
        assert!(canceled.load(Ordering::Acquire));
        assert!(app.library_browser.artists.is_empty() && app.library_browser.follow.is_none());
        assert!(app.workers.is_empty());
    }
}
