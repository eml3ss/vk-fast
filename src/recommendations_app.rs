//! "Configure recommendations": loads VK's artist suggestions and searches,
//! keeps the picks local and applies them as follows/unfollows on Save.
use super::*;
use crate::web_audio::{ArtistPicks, FollowedArtist};

/// Search waits for typing to pause this long.
const SEARCH_DELAY: Duration = Duration::from_millis(350);

type Pending<T> = Option<(Arc<AtomicBool>, Receiver<Result<T, WebAudioError>>)>;
/// A search in flight: its query, cancellation and reply.
type Search = (
    String,
    Arc<AtomicBool>,
    Receiver<Result<Vec<FollowedArtist>, WebAudioError>>,
);

#[derive(Default)]
pub(super) struct State {
    pub(super) open: bool,
    query: String,
    /// The query the shown results belong to.
    searched: String,
    search_due: Option<Instant>,
    picks: Vec<FollowedArtist>,
    results: Vec<FollowedArtist>,
    next_from: Option<String>,
    started: bool,
    exhausted: bool,
    picks_request: Pending<ArtistPicks>,
    search_request: Option<Search>,
    /// Picked artists, newest first; `initial` is what the account follows.
    picked: Vec<FollowedArtist>,
    initial: Option<Vec<FollowedArtist>>,
    save_request: Pending<()>,
    error: Option<WebAudioError>,
}

impl State {
    /// A screenshot of the dialog with made-up artists and no requests.
    #[cfg(feature = "screenshot")]
    pub(super) fn demo(&mut self) {
        let artist = |index: usize| FollowedArtist {
            artist: ArtistRef {
                id: format!("demo-artist-{index}"),
                name: [
                    "Северный ветер",
                    "Paper Planes",
                    "Комната 7",
                    "Blue Avenue",
                    "Радиоволны",
                    "Тихий берег",
                    "После полуночи",
                    "Morning Lines",
                ][index % 8]
                    .to_owned(),
            },
            image_url: None,
        };
        self.open = true;
        self.started = true;
        self.exhausted = true;
        self.picks = (0..24).map(artist).collect();
        self.picked = vec![artist(2), artist(5)];
        self.initial = Some(vec![artist(2)]);
    }

    pub(super) fn close(&mut self) {
        for canceled in [
            self.picks_request.as_ref().map(|(canceled, _)| canceled),
            self.search_request
                .as_ref()
                .map(|(_, canceled, _)| canceled),
            self.save_request.as_ref().map(|(canceled, _)| canceled),
        ]
        .into_iter()
        .flatten()
        {
            canceled.store(true, Ordering::Release);
        }
        *self = Self::default();
    }

    fn changed(&self) -> bool {
        let Some(initial) = &self.initial else {
            return false;
        };
        initial.len() != self.picked.len()
            || initial.iter().any(|artist| {
                !self
                    .picked
                    .iter()
                    .any(|item| item.artist.id == artist.artist.id)
            })
    }

    fn toggle(&mut self, id: &str) {
        if let Some(index) = self.picked.iter().position(|item| item.artist.id == id) {
            self.picked.remove(index);
        } else if let Some(artist) = self
            .picks
            .iter()
            .chain(&self.results)
            .find(|item| item.artist.id == id)
        {
            self.picked.insert(0, artist.clone());
        }
    }
}

impl MusicApp {
    pub(super) fn open_recommendation_settings(&mut self, ctx: &egui::Context) {
        if self.demo
            || self
                .session
                .as_ref()
                .and_then(|s| s.music.as_ref())
                .is_none()
        {
            return;
        }
        self.recommendations.close();
        self.recommendations.open = true;
        // The picks start from the account's followed artists.
        self.load_library_artists(ctx, false);
        self.load_artist_picks(ctx);
    }

    fn load_artist_picks(&mut self, ctx: &egui::Context) {
        let state = &self.recommendations;
        if state.picks_request.is_some()
            || state.exhausted
            || (state.started && state.next_from.is_none())
        {
            return;
        }
        let Some((owner, client)) = self
            .session
            .as_ref()
            .and_then(|session| Some((session.profile.id, session.music.clone()?)))
        else {
            return;
        };
        let cursor = self.recommendations.next_from.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-artist-picks".into())
            .spawn(move || {
                let _ = sender.send(client.artist_picks(owner, cursor.as_deref(), &worker_cancel));
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.recommendations.started = true;
                self.recommendations.picks_request = Some((canceled, receiver));
            }
            Err(_) => self.recommendations.error = Some(WebAudioError::Network),
        }
    }

    fn search_artist_picks(&mut self, ctx: &egui::Context, query: String) {
        let Some((owner, client)) = self
            .session
            .as_ref()
            .and_then(|session| Some((session.profile.id, session.music.clone()?)))
        else {
            return;
        };
        if let Some((_, canceled, _)) = self.recommendations.search_request.take() {
            canceled.store(true, Ordering::Release);
        }
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let worker_query = query.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-artist-search".into())
            .spawn(move || {
                let _ =
                    sender.send(client.search_artist_picks(owner, &worker_query, &worker_cancel));
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.recommendations.search_request = Some((query, canceled, receiver));
            }
            Err(_) => self.recommendations.error = Some(WebAudioError::Network),
        }
    }

    fn save_recommendation_settings(&mut self, ctx: &egui::Context) {
        let state = &self.recommendations;
        let Some(initial) = &state.initial else {
            return;
        };
        if state.save_request.is_some() || !state.changed() {
            return;
        }
        let Some((owner, client)) = self
            .session
            .as_ref()
            .and_then(|session| Some((session.profile.id, session.music.clone()?)))
        else {
            return;
        };
        let follow: Vec<ArtistRef> = state
            .picked
            .iter()
            .rev()
            .filter(|item| {
                !initial
                    .iter()
                    .any(|known| known.artist.id == item.artist.id)
            })
            .map(|item| item.artist.clone())
            .collect();
        let unfollow: Vec<ArtistRef> = initial
            .iter()
            .filter(|known| {
                !state
                    .picked
                    .iter()
                    .any(|item| item.artist.id == known.artist.id)
            })
            .map(|item| item.artist.clone())
            .collect();
        crate::diagnostics::event(
            "recommendations",
            &format!("save follow={} unfollow={}", follow.len(), unfollow.len()),
        );
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-artist-picks-save".into())
            .spawn(move || {
                let _ = sender.send(client.update_followed_artists(
                    owner,
                    &follow,
                    &unfollow,
                    &worker_cancel,
                ));
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.recommendations.error = None;
                self.recommendations.save_request = Some((canceled, receiver));
            }
            Err(_) => self.recommendations.error = Some(WebAudioError::Network),
        }
    }

    pub(super) fn poll_recommendation_settings(&mut self, ctx: &egui::Context) {
        if !self.recommendations.open {
            return;
        }
        if self.recommendations.initial.is_none() && !self.library_browser.loading() {
            if self.library_browser.error.is_some() {
                self.recommendations.error = self.library_browser.error.clone();
            } else {
                let followed = self.library_browser.artists.clone();
                self.recommendations.picked = followed.clone();
                self.recommendations.initial = Some(followed);
            }
        }
        if let Some((_, receiver)) = &self.recommendations.picks_request {
            match receiver.try_recv() {
                Ok(result) => {
                    self.recommendations.picks_request = None;
                    match result {
                        Ok(page) => {
                            let state = &mut self.recommendations;
                            let before = state.picks.len();
                            for artist in page.artists {
                                if !state
                                    .picks
                                    .iter()
                                    .any(|item| item.artist.id == artist.artist.id)
                                {
                                    state.picks.push(artist);
                                }
                            }
                            // A page that brings nothing new ends the list.
                            state.exhausted =
                                page.next_from.is_none() || state.picks.len() == before;
                            state.next_from = page.next_from;
                        }
                        Err(error) => self.recommendations.error = Some(error),
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.recommendations.picks_request = None;
                    self.recommendations.error = Some(WebAudioError::Network);
                }
            }
        }
        if let Some((query, _, receiver)) = &self.recommendations.search_request {
            match receiver.try_recv() {
                Ok(result) => {
                    let query = query.clone();
                    self.recommendations.search_request = None;
                    match result {
                        Ok(results) => {
                            self.recommendations.results = results;
                            self.recommendations.searched = query;
                        }
                        Err(error) => self.recommendations.error = Some(error),
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.recommendations.search_request = None,
            }
        }
        let query = self.recommendations.query.trim().to_owned();
        if query.is_empty() {
            self.recommendations.search_due = None;
            self.recommendations.results.clear();
            self.recommendations.searched.clear();
        } else if query != self.recommendations.searched
            && self
                .recommendations
                .search_request
                .as_ref()
                .is_none_or(|(pending, _, _)| *pending != query)
        {
            let due = *self
                .recommendations
                .search_due
                .get_or_insert_with(|| Instant::now() + SEARCH_DELAY);
            if Instant::now() >= due {
                self.recommendations.search_due = None;
                self.search_artist_picks(ctx, query);
            } else {
                ctx.request_repaint_after(due - Instant::now());
            }
        }
        if let Some((_, receiver)) = &self.recommendations.save_request {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    crate::diagnostics::event("recommendations", "saved");
                    self.recommendations.close();
                    // The sidebar's artists and VK's picks for you change with them.
                    self.load_library_artists(ctx, true);
                    self.load_home(ctx);
                }
                Ok(Err(error)) => {
                    self.recommendations.save_request = None;
                    // Some changes may have gone through: start again from VK's list.
                    self.recommendations.error = Some(error);
                    self.load_library_artists(ctx, true);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.recommendations.save_request = None;
                    self.recommendations.error = Some(WebAudioError::Network);
                }
            }
        }
    }

    pub(super) fn recommendation_settings_window(&mut self, ctx: &egui::Context) {
        if !self.recommendations.open {
            return;
        }
        let state = &self.recommendations;
        let searching = !state.query.trim().is_empty();
        let error = state
            .error
            .as_ref()
            .map(|error| error.localized(self.language));
        let view = crate::recommendations_ui::View {
            language: self.language,
            picks: if searching {
                &state.results
            } else {
                &state.picks
            },
            picked: &state.picked,
            loading: if searching {
                state.search_request.is_some() || state.searched != state.query.trim()
            } else {
                state.picks_request.is_some() || !state.started
            },
            more: !searching && !state.exhausted && state.next_from.is_some(),
            searching,
            saving: state.save_request.is_some(),
            changed: state.changed(),
            error: error.as_deref(),
        };
        let mut query = self.recommendations.query.clone();
        let output = crate::recommendations_ui::show(ctx, &mut self.covers, view, &mut query);
        self.recommendations.query = query;
        match output {
            crate::recommendations_ui::Output::None => {}
            crate::recommendations_ui::Output::Close => {
                if self.recommendations.save_request.is_none() {
                    self.recommendations.close();
                }
            }
            crate::recommendations_ui::Output::Toggle(id) => {
                if self.recommendations.initial.is_some()
                    && self.recommendations.save_request.is_none()
                {
                    self.recommendations.toggle(&id);
                }
            }
            crate::recommendations_ui::Output::LoadMore => self.load_artist_picks(ctx),
            crate::recommendations_ui::Output::Save => self.save_recommendation_settings(ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::State;
    use crate::{model::ArtistRef, web_audio::FollowedArtist};

    fn artist(id: &str) -> FollowedArtist {
        FollowedArtist {
            artist: ArtistRef {
                id: id.into(),
                name: id.into(),
            },
            image_url: None,
        }
    }

    #[test]
    fn picks_toggle_locally_and_count_as_changed_only_when_they_differ() {
        let mut state = State {
            picks: vec![artist("a"), artist("b")],
            picked: vec![artist("a")],
            initial: Some(vec![artist("a")]),
            ..Default::default()
        };
        assert!(!state.changed());
        state.toggle("b");
        assert_eq!(state.picked[0].artist.id, "b", "newest first");
        assert!(state.changed());
        state.toggle("b");
        assert!(!state.changed());
        state.toggle("a");
        assert!(state.picked.is_empty() && state.changed());
        // An artist only known from search results can be picked too.
        state.results = vec![artist("c")];
        state.toggle("c");
        assert_eq!(state.picked[0].artist.id, "c");
        // Unknown ids are ignored.
        state.toggle("zzz");
        assert_eq!(state.picked.len(), 1);
    }
}
