//! Album names for the album column. VK's website rows name an album only
//! by ID, so the rows on screen that lack a name are looked up, 100 at a
//! time and apart from other background fills, and one answer names every
//! track of that album at once.
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

#[derive(Default)]
pub(super) struct State {
    /// Names learned from any list, by album.
    names: HashMap<AlbumKey, String>,
    /// Rows drawn this frame whose album still has no name.
    wanted: Vec<(i64, i64)>,
    attempted: HashSet<(i64, i64)>,
    request: Option<Request>,
}

impl State {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// The album text of a drawn row, asking for it when VK only gave an ID.
    pub(super) fn label(&mut self, track: &Track) -> Option<String> {
        let album = track.album.as_ref()?;
        let key = (album.owner_id, album.id);
        let raw = album.title.trim();
        if !raw.is_empty() {
            self.names.entry(key).or_insert_with(|| raw.to_owned());
            return track.display_album_title().map(str::to_owned);
        }
        if let Some(name) = self.names.get(&key) {
            return album_text(track, name);
        }
        if !self.attempted.contains(&track.key()) {
            self.wanted.push(track.key());
        }
        None
    }

    /// The album a row's link opens, with its name when known.
    pub(super) fn named(&self, track: &Track) -> Option<AlbumRef> {
        let mut album = track.album.clone()?;
        if album.title.trim().is_empty()
            && let Some(name) = self.names.get(&(album.owner_id, album.id))
        {
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

    pub(super) fn poll_album_titles(&mut self, ctx: &egui::Context) {
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
                by_album.insert((album.owner_id, album.id), album.clone());
                self.album_titles
                    .names
                    .insert((album.owner_id, album.id), album.title.trim().to_owned());
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
    fn unnamed_singles_show_the_song_like_the_player() {
        let mut state = State::default();
        let mut single = with_album(1, 5, "Unnamed");
        single.title = "Song (Slowed)".into();
        single.subtitle = Some("Slowed".into());
        assert_eq!(state.label(&single).as_deref(), Some("Song"));
        let mut later = with_album(2, 5, "");
        later.title = "Other".into();
        assert_eq!(state.label(&later).as_deref(), Some("Other"));
    }
}
