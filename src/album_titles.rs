//! Album names for the album column. VK's website rows name an album only
//! by ID, so the rows on screen that lack a name are looked up, 100 at a
//! time and apart from other background fills, and one answer names every
//! track of that album at once. Track data calls an EP or single
//! "Unnamed"; its real title is then read from the release itself.
use super::*;

pub(super) type AlbumKey = (i64, i64);

pub(super) struct Request {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Vec<Track>, WebAudioError>>,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

/// Titles read from releases themselves; `None` when VK gave none.
pub(super) struct ReleaseRequest {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Vec<(AlbumKey, Option<String>)>>,
}

impl Drop for ReleaseRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

/// Releases read per request: each is its own call.
const RELEASES_PER_REQUEST: usize = 8;

#[derive(Default)]
pub(super) struct State {
    /// Names learned from any list, by album.
    names: HashMap<AlbumKey, String>,
    /// Rows drawn this frame whose album still has no name.
    wanted: Vec<(i64, i64)>,
    attempted: HashSet<(i64, i64)>,
    request: Option<Request>,
    /// Releases drawn this frame that track data calls "Unnamed".
    releases: Vec<AlbumRef>,
    releases_attempted: HashSet<AlbumKey>,
    release_request: Option<ReleaseRequest>,
}

fn unnamed(title: &str) -> bool {
    title.trim().eq_ignore_ascii_case("unnamed")
}

impl State {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// The album text of a drawn row, asking VK when it only gave an ID or
    /// called the release "Unnamed".
    pub(super) fn label(&mut self, track: &Track) -> Option<String> {
        let album = track.album.as_ref()?;
        let key = (album.owner_id, album.id);
        if let Some(name) = self.names.get(&key) {
            return Some(name.clone());
        }
        let raw = album.title.trim();
        if raw.is_empty() {
            if !self.attempted.contains(&track.key()) {
                self.wanted.push(track.key());
            }
            return None;
        }
        if !unnamed(raw) {
            self.names.insert(key, raw.to_owned());
            return Some(raw.to_owned());
        }
        if self.releases_attempted.contains(&key) {
            // VK gave no title: the song stands in, as in the player.
            return album_text(track, raw);
        }
        self.releases.push(album.clone());
        None
    }

    /// The album a row's link opens, with its name when known.
    pub(super) fn named(&self, track: &Track) -> Option<AlbumRef> {
        let mut album = track.album.clone()?;
        if let Some(name) = self.names.get(&(album.owner_id, album.id)) {
            album.title = name.clone();
        }
        Some(album)
    }
}

/// VK names a single's release "Unnamed": show the song instead, as the
/// player does.
fn album_text(track: &Track, name: &str) -> Option<String> {
    let name = name.trim();
    let text = if name.eq_ignore_ascii_case("unnamed") {
        track.title_parts().0.trim()
    } else {
        name
    };
    (!text.is_empty()).then(|| text.to_owned())
}

impl MusicApp {
    /// Asks VK for the names the drawn rows are missing.
    pub(super) fn request_album_titles(&mut self, ctx: &egui::Context) {
        self.request_release_titles(ctx);
        let wanted = std::mem::take(&mut self.album_titles.wanted);
        if self.demo || self.album_titles.request.is_some() || wanted.is_empty() {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let mut unique = HashSet::new();
        let keys: Vec<_> = wanted
            .into_iter()
            .filter(|key| {
                key.0 != 0
                    && key.1 > 0
                    && !self.album_titles.attempted.contains(key)
                    && unique.insert(*key)
            })
            .take(100)
            .collect();
        if keys.is_empty() {
            return;
        }
        let owner = session.profile.id;
        self.album_titles.attempted.extend(keys.iter().copied());
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-album-titles".into())
            .spawn(move || {
                let _ = sender.send(client.tracks_metadata(owner, &keys, &worker_cancel));
                repaint.request_repaint();
            })
        {
            self.workers.push(worker);
            self.album_titles.request = Some(Request {
                owner,
                canceled,
                receiver,
            });
        }
    }

    fn request_release_titles(&mut self, ctx: &egui::Context) {
        // The playing track's release too: the player, system media and
        // Discord name it as well.
        if let Some(album) = self
            .playing
            .as_ref()
            .and_then(|track| track.album.as_ref())
            .filter(|album| unnamed(&album.title))
        {
            self.album_titles.releases.push(album.clone());
        }
        let wanted = std::mem::take(&mut self.album_titles.releases);
        if self.demo || self.album_titles.release_request.is_some() || wanted.is_empty() {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let mut unique = HashSet::new();
        let albums: Vec<_> = wanted
            .into_iter()
            .filter(|album| {
                let key = (album.owner_id, album.id);
                album.owner_id != 0
                    && album.id > 0
                    && !self.album_titles.releases_attempted.contains(&key)
                    && unique.insert(key)
            })
            .take(RELEASES_PER_REQUEST)
            .collect();
        if albums.is_empty() {
            return;
        }
        let owner = session.profile.id;
        self.album_titles
            .releases_attempted
            .extend(albums.iter().map(|album| (album.owner_id, album.id)));
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-album-names".into())
            .spawn(move || {
                let mut titles = Vec::with_capacity(albums.len());
                for album in &albums {
                    if worker_cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let title = client
                        .collection_title(owner, album, &worker_cancel)
                        .ok()
                        .flatten();
                    titles.push(((album.owner_id, album.id), title));
                }
                let _ = sender.send(titles);
                repaint.request_repaint();
            })
        {
            self.workers.push(worker);
            self.album_titles.release_request = Some(ReleaseRequest {
                owner,
                canceled,
                receiver,
            });
        }
    }

    fn poll_release_titles(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.album_titles.release_request else {
            return;
        };
        let titles = match request.receiver.try_recv() {
            Ok(titles) => titles,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Vec::new(),
        };
        let request = self.album_titles.release_request.take().unwrap();
        if self.session.as_ref().map(|session| session.profile.id) != Some(request.owner) {
            return;
        }
        let named: HashMap<_, _> = titles
            .into_iter()
            .filter_map(|(key, title)| title.map(|title| (key, title)))
            .collect();
        if named.is_empty() {
            ctx.request_repaint();
            return;
        }
        self.album_titles
            .names
            .extend(named.iter().map(|(key, title)| (*key, title.clone())));
        self.name_tracks(ctx, &named);
    }

    /// Writes release titles into every known track of those releases, so
    /// the player and saved lists show them too.
    fn name_tracks(&mut self, ctx: &egui::Context, named: &HashMap<AlbumKey, String>) {
        let mut library_changed = false;
        let fill = |track: &mut Track| {
            if let Some(album) = &mut track.album
                && (album.title.trim().is_empty() || unnamed(&album.title))
                && let Some(title) = named.get(&(album.owner_id, album.id))
            {
                album.title.clone_from(title);
                return true;
            }
            false
        };
        for track in &mut self.library.tracks {
            library_changed |= fill(track);
        }
        self.update_known_tracks(|track| {
            fill(track);
        });
        if library_changed {
            self.save_library(ctx);
        }
        ctx.request_repaint();
    }

    pub(super) fn poll_album_titles(&mut self, ctx: &egui::Context) {
        self.poll_release_titles(ctx);
        let Some(request) = &self.album_titles.request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self.album_titles.request.take().unwrap();
        if self.session.as_ref().map(|session| session.profile.id) != Some(request.owner) {
            return;
        }
        let Ok(fresh) = result else {
            // Failed rows may be asked again when they are drawn later.
            return;
        };
        self.learn_album_titles(ctx, &fresh);
    }

    /// Names every known track of the albums `fresh` names.
    fn learn_album_titles(&mut self, ctx: &egui::Context, fresh: &[Track]) {
        let mut by_album = HashMap::new();
        for album in fresh.iter().filter_map(|track| track.album.as_ref()) {
            if !album.title.trim().is_empty() {
                // "Unnamed" still moves the row on to reading the release.
                by_album.insert((album.owner_id, album.id), album.clone());
                if !unnamed(&album.title) {
                    self.album_titles
                        .names
                        .insert((album.owner_id, album.id), album.title.trim().to_owned());
                }
            }
        }
        if by_album.is_empty() {
            return;
        }
        let fill = |track: &mut Track| {
            if let Some(album) = &mut track.album
                && album.title.trim().is_empty()
                && let Some(named) = by_album.get(&(album.owner_id, album.id))
            {
                album.title.clone_from(&named.title);
                if album.cover_url.is_none() {
                    album.cover_url.clone_from(&named.cover_url);
                }
            }
        };
        let saved_before = self
            .library
            .tracks
            .iter()
            .filter(|track| has_name(track))
            .count();
        self.update_known_tracks(fill);
        if self
            .library
            .tracks
            .iter()
            .filter(|track| has_name(track))
            .count()
            != saved_before
        {
            self.save_library(ctx);
        }
        ctx.request_repaint();
    }
}

fn has_name(track: &Track) -> bool {
    track
        .album
        .as_ref()
        .is_some_and(|album| !album.title.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{page, search_test_app};

    fn with_album(id: i64, album: i64, title: &str) -> Track {
        let mut track = page(1, &[id]).items.remove(0);
        track.title = format!("Song {id}");
        track.album = Some(AlbumRef {
            owner_id: -9,
            id: album,
            title: title.into(),
            access_key: Some("a".into()),
            cover_url: None,
        });
        track
    }

    #[test]
    fn nameless_rows_are_asked_once_and_one_answer_names_the_whole_album() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.tracks = vec![
            with_album(1, 5, ""),
            with_album(2, 5, ""),
            with_album(3, 6, ""),
            with_album(4, 7, "Known"),
        ];
        let mut state = State::default();
        let labels: Vec<_> = app
            .library
            .tracks
            .iter()
            .map(|track| state.label(track))
            .collect();
        assert_eq!(labels, [None, None, None, Some("Known".into())]);
        let nameless: Vec<_> = app.library.tracks[..3].iter().map(Track::key).collect();
        assert_eq!(state.wanted, nameless);
        app.album_titles = state;
        // One answer for track 1 names track 2 as well: same album.
        app.learn_album_titles(&ctx, &[with_album(1, 5, "Record")]);
        let titles: Vec<_> = app
            .library
            .tracks
            .iter()
            .map(|track| track.album.as_ref().unwrap().title.clone())
            .collect();
        assert_eq!(titles, ["Record", "Record", "", "Known"]);
        // A later row of that album is named from memory, without asking.
        let mut later = with_album(9, 5, "");
        later.title = "Later".into();
        app.album_titles.wanted.clear();
        assert_eq!(app.album_titles.label(&later).as_deref(), Some("Record"));
        assert!(app.album_titles.wanted.is_empty());
        assert_eq!(app.album_titles.named(&later).unwrap().title, "Record");
    }

    #[test]
    fn unnamed_releases_are_read_from_the_release_and_fall_back_to_the_song() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        // As VK's EP "BANANATEKK (Versions)": track data says "Unnamed".
        let mut slowed = with_album(1, 5, "Unnamed");
        slowed.title = "BANANATEKK - Slowed".into();
        let mut original = with_album(2, 5, "Unnamed");
        original.title = "BANANATEKK".into();
        app.library.tracks = vec![slowed.clone(), original];
        assert_eq!(
            app.album_titles.label(&slowed),
            None,
            "waits for the release"
        );
        assert_eq!(app.album_titles.releases.len(), 1);
        let named = HashMap::from([((-9, 5), "BANANATEKK (Versions)".to_string())]);
        app.album_titles.names.extend(named.clone());
        app.name_tracks(&ctx, &named);
        for track in &app.library.tracks {
            assert_eq!(track.album.as_ref().unwrap().title, "BANANATEKK (Versions)");
            assert_eq!(track.display_album_title(), Some("BANANATEKK (Versions)"));
            assert_eq!(
                app.album_titles.label(track).as_deref(),
                Some("BANANATEKK (Versions)")
            );
        }
        // A release VK gives no title for shows the song, as the player does.
        let mut single = with_album(3, 6, "Unnamed");
        single.title = "Song (Slowed)".into();
        single.subtitle = Some("Slowed".into());
        app.album_titles.releases_attempted.insert((-9, 6));
        assert_eq!(app.album_titles.label(&single).as_deref(), Some("Song"));
    }

    #[test]
    fn unnamed_answers_never_become_a_shown_name() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.tracks = vec![with_album(1, 5, "")];
        app.learn_album_titles(&ctx, &[with_album(1, 5, "Unnamed")]);
        let track = app.library.tracks[0].clone();
        assert_eq!(track.album.as_ref().unwrap().title, "Unnamed");
        assert_eq!(app.album_titles.label(&track), None);
        assert_eq!(
            app.album_titles.releases.len(),
            1,
            "the release is read next"
        );
    }
}
