//! Sidebar playlist actions. Network jobs keep their account and target identity.
#[cfg(test)]
#[path = "playlist_mix_notice_tests.rs"]
mod mix_notice_tests;
#[cfg(test)]
#[path = "playlist_actions_tests.rs"]
mod tests;

use super::*;
use crate::web_audio::{PlaylistCoverChange, PlaylistEditDetails, PlaylistEditUpdate};
use crate::{playlist_cover, playlist_edit_ui, playlist_pins};

type Key = (i64, i64);

#[derive(Default)]
pub(super) struct State {
    pins_owner: Option<i64>,
    pins: Vec<Key>,
    base_order: Vec<Key>,
    manual_order: bool,
    form: Option<Form>,
    request: Option<ManageRequest>,
    picker:
        Option<Receiver<Result<Option<playlist_cover::ChosenCover>, playlist_cover::CoverError>>>,
    autoplay: Option<PlayRequest>,
    mix_notice: Option<Instant>,
    track_notice: Option<Instant>,
    follow: Option<FollowRequest>,
    refresh_followed: bool,
}

struct FollowRequest {
    owner: i64,
    kind: crate::web_audio::CollectionKind,
    byline: String,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<crate::web_audio::PlaylistFollowChange, WebAudioError>>,
}
impl Drop for FollowRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

struct Form {
    reference: AlbumRef,
    delete: bool,
    name: String,
    description: String,
    discoverable: Option<bool>,
    can_edit: bool,
    can_delete: bool,
    remove_saved: bool,
    loading: bool,
    focus_name: bool,
    error: Option<String>,
    cover: Option<playlist_cover::ChosenCover>,
    remove_cover: bool,
    preview: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Load,
    Save,
    Delete,
}
enum Managed {
    Loaded(PlaylistEditDetails),
    Saved(Option<PlaylistEditDetails>),
    Deleted,
}
struct ManageRequest {
    owner: i64,
    target: Key,
    job: Job,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Managed, WebAudioError>>,
}
impl Drop for ManageRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}
struct PlayRequest {
    owner: i64,
    reference: AlbumRef,
    shuffle: bool,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Vec<Track>, WebAudioError>>,
}
impl Drop for PlayRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

fn key(reference: &AlbumRef) -> Key {
    (reference.owner_id, reference.id)
}
fn read_pins(owner: i64) -> Vec<Key> {
    #[cfg(not(test))]
    {
        playlist_pins::load(owner).unwrap_or_default()
    }
    #[cfg(test)]
    {
        let _ = (owner, playlist_pins::load);
        Vec::new()
    }
}
fn write_pins(owner: i64, pins: &[Key]) -> bool {
    #[cfg(not(test))]
    {
        playlist_pins::save(owner, pins).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (owner, pins, playlist_pins::save);
        true
    }
}

fn read_order(owner: i64) -> Vec<Key> {
    #[cfg(not(test))]
    {
        playlist_pins::load_order(owner).unwrap_or_default()
    }
    #[cfg(test)]
    {
        let _ = (owner, playlist_pins::load_order);
        Vec::new()
    }
}

fn write_order(owner: i64, order: &[Key]) -> bool {
    #[cfg(not(test))]
    {
        playlist_pins::save_order(owner, order).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (owner, order, playlist_pins::save_order);
        true
    }
}

impl State {
    pub(super) fn has_modal(&self) -> bool {
        self.form.is_some()
    }
    pub(super) fn cancel_play(&mut self) {
        self.autoplay = None;
    }
    pub(super) fn loading_play(&self) -> bool {
        self.autoplay.is_some()
    }
    pub(super) fn rebind(&mut self, ctx: &egui::Context) {
        if let Some(form) = &mut self.form {
            form.preview = form.cover.as_ref().map(|cover| {
                ctx.load_texture(
                    "playlist-cover-preview",
                    cover.image.clone(),
                    egui::TextureOptions::LINEAR,
                )
            });
            form.focus_name = true;
        }
    }
}

impl MusicApp {
    pub(super) fn playlist_follow_view(&self, target: &AlbumRef) -> Option<detail_ui::FollowView> {
        if self.can_edit_playlist(target) {
            return None;
        }
        let session = self.session.as_ref()?;
        let saved = session
            .music
            .as_ref()?
            .playlist_followed(session.profile.id, target)?;
        if !saved && target.owner_id == session.profile.id {
            return None;
        }
        Some(detail_ui::FollowView {
            saved,
            busy: self.playlist_controls.follow.is_some(),
        })
    }

    pub(super) fn toggle_playlist_follow(&mut self, ctx: &egui::Context, target: AlbumRef) {
        let Some(view) = self.playlist_follow_view(&target).filter(|view| !view.busy) else {
            return;
        };
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let kind = if matches!(self.page, MusicPage::Album(_) | MusicPage::TrackAlbum(_, _)) {
            crate::web_audio::CollectionKind::Album
        } else {
            crate::web_audio::CollectionKind::Playlist
        };
        let byline = self
            .page
            .detail_key()
            .and_then(|key| self.details.get(&key))
            .and_then(|page| match page.as_ref() {
                DetailPage::Album(page) => Some(
                    page.artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                DetailPage::Playlist(page) => {
                    page.creator.as_ref().map(|creator| creator.name.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-playlist-follow".into())
            .spawn(move || {
                let result =
                    client.set_playlist_follow(owner, &target, !view.saved, &worker_cancel);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_controls.follow = Some(FollowRequest {
                    owner,
                    kind,
                    byline,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.track_menu_error = Some(WebAudioError::Network),
        }
    }

    fn poll_playlist_follow(&mut self, ctx: &egui::Context) {
        let result = self.playlist_controls.follow.as_ref().and_then(|request| {
            match request.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(WebAudioError::Network)),
            }
        });
        if let Some(result) = result {
            let request = self.playlist_controls.follow.take().unwrap();
            if !request.canceled.load(Ordering::Acquire)
                && self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.profile.id == request.owner)
            {
                match result {
                    Ok(change) => {
                        if let Some(removed) = change.removed {
                            self.playlist_items
                                .retain(|item| key(&item.reference) != removed);
                            self.playlist_controls.pins.retain(|pin| *pin != removed);
                            let _ = write_pins(request.owner, &self.playlist_controls.pins);
                            self.repoint_unfollowed_playlist(removed, &change.original);
                        }
                        if let Some(saved) = change.saved
                            && !self
                                .playlist_items
                                .iter()
                                .any(|item| key(&item.reference) == key(&saved))
                        {
                            self.playlist_items.push(playlist_ui::PlaylistItem {
                                kind: request.kind,
                                reference: saved,
                                subtitle: request.byline.clone(),
                                can_edit: false,
                                pinned: false,
                            });
                            self.remember_playlist_order();
                            self.sync_playlist_pins();
                        }
                        self.playlist_controls.refresh_followed = true;
                    }
                    Err(WebAudioError::Canceled) => {}
                    Err(error) => self.track_menu_error = Some(error),
                }
                ctx.request_repaint();
            }
        }
        if self.playlist_controls.refresh_followed
            && self.playlist_request.is_none()
            && self.playlist_ready()
        {
            self.playlist_controls.refresh_followed = false;
            self.load_playlists(ctx, true);
        }
    }

    fn repoint_unfollowed_playlist(&mut self, removed: Key, original: &AlbumRef) {
        if key(original) == removed {
            return;
        }
        for is_album in [false, true] {
            let route = |reference| {
                if is_album {
                    MusicPage::Album(reference)
                } else {
                    MusicPage::Playlist(reference)
                }
            };
            let old_key = route(AlbumRef {
                owner_id: removed.0,
                id: removed.1,
                ..original.clone()
            })
            .detail_key()
            .unwrap();
            let replacement = route(original.clone());
            if let Some(mut data) = self.details.remove(&old_key) {
                if let DetailPage::Playlist(page) | DetailPage::Album(page) =
                    Arc::make_mut(&mut data)
                {
                    page.album = original.clone();
                }
                self.details.insert(replacement.detail_key().unwrap(), data);
            }
            if self.page.detail_key().as_ref() == Some(&old_key) {
                self.page = replacement.clone();
                self.detail_request = None;
            }
            for location in self
                .back_history
                .iter_mut()
                .chain(self.forward_history.iter_mut())
            {
                if location.page.detail_key().as_ref() == Some(&old_key) {
                    location.page = replacement.clone();
                }
            }
        }
    }

    fn sidebar_collection_page(&self, reference: AlbumRef) -> MusicPage {
        if self.playlist_items.iter().any(|item| {
            key(&item.reference) == key(&reference)
                && item.kind == crate::web_audio::CollectionKind::Album
        }) {
            MusicPage::Album(reference)
        } else {
            MusicPage::Playlist(reference)
        }
    }

    pub(super) fn playlist_mix_unavailable(&mut self, ctx: &egui::Context) {
        self.playlist_controls.mix_notice = Some(Instant::now() + Duration::from_secs(3));
        ctx.request_repaint();
    }

    pub(super) fn clear_playlist_mix_notice(&mut self) {
        self.playlist_controls.mix_notice = None;
    }

    /// One deadline per failed request; drawing the notice never extends it.
    pub(super) fn playlist_mix_notice(&mut self, ctx: &egui::Context) -> bool {
        timed_notice(
            ctx,
            self.language,
            &mut self.playlist_controls.mix_notice,
            self.language
                .text("Микс по плейлисту не найден", "Playlist mix not found"),
            "playlist-mix-not-found",
        )
    }

    pub(super) fn track_unavailable_notice(&mut self, ctx: &egui::Context) {
        self.playlist_controls.track_notice = Some(Instant::now() + Duration::from_secs(3));
        ctx.request_repaint();
    }

    pub(super) fn render_track_unavailable_notice(&mut self, ctx: &egui::Context) -> bool {
        timed_notice(
            ctx,
            self.language,
            &mut self.playlist_controls.track_notice,
            self.language.text("Трек недоступен", "Track unavailable"),
            "track-unavailable",
        )
    }

    pub(super) fn remember_playlist_order(&mut self) {
        self.ensure_playlist_preferences();
        let incoming: Vec<_> = self
            .playlist_items
            .iter()
            .map(|item| key(&item.reference))
            .collect();
        if !self.playlist_controls.manual_order {
            self.playlist_controls.base_order = incoming;
            return;
        }
        let order = &mut self.playlist_controls.base_order;
        let incoming_keys: HashSet<_> = incoming.iter().copied().collect();
        order.retain(|key| incoming_keys.contains(key));
        let known: HashSet<_> = order.iter().copied().collect();
        let new: Vec<_> = incoming
            .into_iter()
            .filter(|key| !known.contains(key))
            .collect();
        order.splice(..0, new);
    }

    fn ensure_playlist_preferences(&mut self) {
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        let state = &mut self.playlist_controls;
        if state.pins_owner != Some(owner) {
            state.pins = read_pins(owner);
            state.base_order = read_order(owner);
            state.manual_order = !state.base_order.is_empty();
            state.pins_owner = Some(owner);
        }
    }

    pub(super) fn sync_playlist_pins(&mut self) {
        self.ensure_playlist_preferences();
        let state = &mut self.playlist_controls;
        let known: HashSet<_> = state.base_order.iter().copied().collect();
        let new_keys: Vec<_> = self
            .playlist_items
            .iter()
            .map(|item| key(&item.reference))
            .filter(|key| !known.contains(key))
            .collect();
        state.base_order.splice(..0, new_keys);
        let pin_positions: HashMap<_, _> = state
            .pins
            .iter()
            .copied()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect();
        let base_positions: HashMap<_, _> = state
            .base_order
            .iter()
            .copied()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect();
        for item in &mut self.playlist_items {
            item.pinned = pin_positions.contains_key(&key(&item.reference));
        }
        self.playlist_items.sort_by_key(|item| {
            (
                pin_positions
                    .get(&key(&item.reference))
                    .copied()
                    .unwrap_or(usize::MAX),
                base_positions
                    .get(&key(&item.reference))
                    .copied()
                    .unwrap_or(usize::MAX),
            )
        });
    }

    pub(super) fn can_edit_playlist(&self, reference: &AlbumRef) -> bool {
        self.editable_playlists
            .iter()
            .any(|editable| key(editable) == key(reference))
    }

    fn playlist_notice(&mut self, text: &str) {
        self.track_menu_notice = Some((text.into(), Instant::now() + Duration::from_secs(3)));
    }

    pub(super) fn playlist_sidebar_action(
        &mut self,
        ctx: &egui::Context,
        action: playlist_ui::Action,
    ) {
        match action {
            playlist_ui::Action::Reorder { playlist, before } => {
                self.reorder_sidebar_playlist(ctx, &playlist, before.as_ref());
            }
            playlist_ui::Action::Open(reference) => {
                self.navigate_music(ctx, MusicPage::Playlist(reference))
            }
            playlist_ui::Action::OpenAlbum(reference) => {
                self.navigate_music(ctx, MusicPage::Album(reference))
            }
            playlist_ui::Action::OpenArtist(artist) => {
                self.navigate_music(ctx, MusicPage::Artist(artist))
            }
            playlist_ui::Action::UnfollowArtist(artist) => {
                if self
                    .artist_follow_view(&artist)
                    .is_some_and(|view| view.saved)
                {
                    self.toggle_artist_follow(ctx, artist);
                }
            }
            playlist_ui::Action::Refresh => {
                self.load_playlists(ctx, true);
                self.load_library_artists(ctx, true);
            }
            playlist_ui::Action::Create => {
                self.playlist_create_form = Some(PlaylistCreateForm {
                    name: String::new(),
                    focus_name: true,
                    error: None,
                })
            }
            playlist_ui::Action::Play(reference) => {
                self.play_sidebar_playlist(ctx, reference, false)
            }
            playlist_ui::Action::Shuffle(reference) => {
                self.play_sidebar_playlist(ctx, reference, true)
            }
            playlist_ui::Action::Edit(reference) => {
                self.open_playlist_editor(ctx, reference, false)
            }
            playlist_ui::Action::Delete(reference) => {
                self.open_playlist_editor(ctx, reference, true)
            }
            playlist_ui::Action::TogglePin(reference) => {
                self.sync_playlist_pins();
                let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
                    return;
                };
                let target = key(&reference);
                if self.playlist_controls.pins.contains(&target) {
                    self.playlist_controls.pins.retain(|pin| *pin != target);
                } else if self.playlist_controls.pins.len() < 1024 {
                    self.playlist_controls.pins.insert(0, target);
                }
                if !write_pins(owner, &self.playlist_controls.pins) {
                    self.playlist_notice(self.language.text(
                        "Не удалось сохранить закреплённые плейлисты",
                        "Could not save pinned playlists",
                    ));
                }
                self.sync_playlist_pins();
            }
            playlist_ui::Action::Mix(reference) => {
                if self.search_ready() && !self.demo {
                    self.mix_request = None;
                    self.track_menu_error = None;
                    self.load_mix_from(ctx, true, Some(MixSeed::Playlist(key(&reference))));
                }
            }
            playlist_ui::Action::CopyLink(reference) => {
                if reference.owner_id != 0 && reference.id > 0 {
                    let access = reference
                        .access_key
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(|value| format!("_{value}"))
                        .unwrap_or_default();
                    let mut url = reqwest::Url::parse("https://vk.ru/music/playlist/")
                        .expect("Static VK playlist URL");
                    url.path_segments_mut()
                        .expect("VK URL has path segments")
                        .pop_if_empty()
                        .push(&format!("{}_{}{access}", reference.owner_id, reference.id));
                    ctx.copy_text(url.to_string());
                    self.playlist_notice(self.language.text("Ссылка скопирована", "Link copied"));
                }
            }
        }
        ctx.request_repaint();
    }

    fn reorder_sidebar_playlist(
        &mut self,
        ctx: &egui::Context,
        playlist: &AlbumRef,
        before: Option<&AlbumRef>,
    ) {
        self.sync_playlist_pins();
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        let target = key(playlist);
        let Some(item) = self
            .playlist_items
            .iter()
            .find(|item| key(&item.reference) == target)
        else {
            return;
        };
        let pinned = item.pinned;
        let before_key = before.map(key);
        if before_key == Some(target) {
            return;
        }
        if let Some(before) = before_key
            && !self
                .playlist_items
                .iter()
                .any(|item| key(&item.reference) == before && item.pinned == pinned)
        {
            return;
        }
        let state = &mut self.playlist_controls;
        state.manual_order = true;
        state.base_order.retain(|key| *key != target);
        let position = before_key
            .and_then(|before| state.base_order.iter().position(|key| *key == before))
            .unwrap_or(state.base_order.len());
        state.base_order.insert(position, target);
        let mut saved = write_order(owner, &state.base_order);
        if pinned {
            state.pins.retain(|key| *key != target);
            let position = before_key
                .and_then(|before| state.pins.iter().position(|key| *key == before))
                .unwrap_or(state.pins.len());
            state.pins.insert(position, target);
            saved &= write_pins(owner, &state.pins);
        }
        self.sync_playlist_pins();
        if !saved {
            self.playlist_notice(self.language.text(
                "Не удалось сохранить порядок плейлистов",
                "Could not save playlist order",
            ));
        }
        ctx.request_repaint();
    }

    fn play_sidebar_playlist(&mut self, ctx: &egui::Context, reference: AlbumRef, shuffle: bool) {
        self.cancel_delayed_advance();
        let page_key = self
            .sidebar_collection_page(reference.clone())
            .detail_key()
            .unwrap();
        if let Some(data) = self.details.get(&page_key) {
            let tracks = data.tracks().to_vec();
            self.start_sidebar_playlist(ctx, reference, tracks, shuffle);
            return;
        }
        if self.demo {
            return;
        }
        let Some((owner, client)) = self.session.as_ref().and_then(|session| {
            session
                .music
                .clone()
                .map(|client| (session.profile.id, client))
        }) else {
            return;
        };
        let target = reference.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let repaint = self.repaint_target.with_context(ctx);
        let (sender, receiver) = mpsc::sync_channel(1);
        match thread::Builder::new()
            .name("vk-playlist-play".into())
            .spawn(move || {
                let result = client
                    .album_page(owner, &target, &worker_cancel)
                    .map(|page| page.tracks);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_controls.autoplay = Some(PlayRequest {
                    owner,
                    reference,
                    shuffle,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.track_menu_error = Some(WebAudioError::Network),
        }
    }

    fn start_sidebar_playlist(
        &mut self,
        ctx: &egui::Context,
        reference: AlbumRef,
        tracks: Vec<Track>,
        shuffle: bool,
    ) {
        if tracks.is_empty() {
            self.playlist_notice(self.language.text(
                "В плейлисте пока нет треков",
                "This playlist has no tracks yet",
            ));
            return;
        }
        self.leave_mix();
        let keys: Vec<_> = tracks.iter().map(Track::key).collect();
        self.play_order.set_shuffle(shuffle, &keys, None);
        self.save_playback_modes();
        self.start_queue_from(
            ctx,
            tracks,
            self.sidebar_collection_page(reference)
                .detail_key()
                .unwrap(),
        );
    }

    pub(super) fn open_playlist_editor(
        &mut self,
        ctx: &egui::Context,
        reference: AlbumRef,
        delete: bool,
    ) {
        if self
            .playlist_controls
            .request
            .as_ref()
            .is_some_and(|request| request.job != Job::Load)
        {
            return;
        }
        self.playlist_controls.request = None;
        self.playlist_controls.picker = None;
        self.playlist_controls.form = Some(Form {
            name: reference.title.clone(),
            reference,
            delete,
            description: String::new(),
            discoverable: None,
            can_edit: false,
            can_delete: false,
            remove_saved: false,
            loading: true,
            focus_name: false,
            error: None,
            cover: None,
            remove_cover: false,
            preview: None,
        });
        self.start_playlist_manage(ctx, Job::Load);
    }

    fn start_playlist_manage(&mut self, ctx: &egui::Context, job: Job) {
        if self.playlist_controls.request.is_some() {
            return;
        }
        let Some(form) = &mut self.playlist_controls.form else {
            return;
        };
        if (job == Job::Save && !form.can_edit) || (job == Job::Delete && !form.can_delete) {
            return;
        }
        if job == Job::Save
            && (form.name.trim().is_empty()
                || form.name.trim().chars().count() > crate::web_audio::PLAYLIST_TITLE_MAX_CHARS
                || form.name.chars().any(char::is_control)
                || form.description.chars().count()
                    > crate::web_audio::PLAYLIST_DESCRIPTION_MAX_CHARS)
        {
            return;
        }
        let Some((owner, client)) = self.session.as_ref().and_then(|session| {
            session
                .music
                .clone()
                .map(|client| (session.profile.id, client))
        }) else {
            form.loading = false;
            form.error = Some(WebAudioError::NoSession.localized(self.language));
            return;
        };
        let reference = form.reference.clone();
        let target = key(&reference);
        let update = PlaylistEditUpdate {
            title: form.name.trim().into(),
            description: form.description.clone(),
            discoverable: form.discoverable,
            cover: if form.remove_cover {
                PlaylistCoverChange::Remove
            } else {
                form.cover
                    .as_ref()
                    .map_or(PlaylistCoverChange::Keep, |cover| {
                        PlaylistCoverChange::Replace(cover.jpeg.clone())
                    })
            },
        };
        form.error = None;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let repaint = self.repaint_target.with_context(ctx);
        let (sender, receiver) = mpsc::sync_channel(1);
        match thread::Builder::new()
            .name("vk-playlist-manage".into())
            .spawn(move || {
                let result = match job {
                    Job::Load => client
                        .playlist_edit_details(owner, &reference, &worker_cancel)
                        .map(Managed::Loaded),
                    Job::Save => client
                        .edit_playlist(owner, &reference, &update, &worker_cancel)
                        .map(|()| {
                            Managed::Saved(
                                client
                                    .playlist_edit_details(owner, &reference, &worker_cancel)
                                    .ok(),
                            )
                        }),
                    Job::Delete => client
                        .delete_playlist(owner, &reference, &worker_cancel)
                        .map(|()| Managed::Deleted),
                };
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_controls.request = Some(ManageRequest {
                    owner,
                    target,
                    job,
                    canceled,
                    receiver,
                });
            }
            Err(_) => {
                let form = self.playlist_controls.form.as_mut().unwrap();
                form.loading = false;
                form.error = Some(WebAudioError::Network.localized(self.language));
            }
        }
    }

    fn update_playlist_reference(&mut self, reference: &AlbumRef, can_edit: bool) {
        for item in &mut self.playlist_items {
            if key(&item.reference) == key(reference) {
                item.reference = reference.clone();
                item.can_edit = can_edit;
            }
        }
        self.editable_playlists
            .retain(|item| key(item) != key(reference));
        if can_edit {
            self.editable_playlists.push(reference.clone());
        }
    }

    pub(super) fn playlist_manage_window(&mut self, ctx: &egui::Context) {
        let busy = self
            .playlist_controls
            .request
            .as_ref()
            .is_some_and(|request| request.job != Job::Load);
        let Some(form) = &mut self.playlist_controls.form else {
            return;
        };
        let action = if form.delete {
            match playlist_edit_ui::show_delete(
                ctx,
                playlist_edit_ui::DeleteView {
                    language: self.language,
                    title: &form.name,
                    remove_saved: form.remove_saved,
                    removing: busy,
                    loading: form.loading,
                    can_delete: form.can_delete,
                    error: form.error.as_deref(),
                },
            ) {
                Some(playlist_edit_ui::DeleteAction::Cancel) => {
                    Some(playlist_edit_ui::Action::Cancel)
                }
                Some(playlist_edit_ui::DeleteAction::Confirm) => {
                    Some(playlist_edit_ui::Action::Save)
                }
                None => None,
            }
        } else {
            playlist_edit_ui::show(
                ctx,
                &mut self.covers,
                playlist_edit_ui::View {
                    language: self.language,
                    name: &mut form.name,
                    description: &mut form.description,
                    cover_url: form.reference.cover_url.as_deref(),
                    cover_preview: form.preview.as_ref(),
                    discoverable: form.discoverable.as_mut(),
                    can_change_cover: form.can_edit,
                    can_edit: form.can_edit,
                    loading: form.loading,
                    busy: busy || self.playlist_controls.picker.is_some(),
                    picking_cover: self.playlist_controls.picker.is_some(),
                    error: form.error.as_deref(),
                    focus_name: form.focus_name,
                },
            )
        };
        form.focus_name = false;
        let delete = form.delete;
        match action {
            Some(playlist_edit_ui::Action::Cancel) if !busy => {
                self.playlist_controls.request = None;
                self.playlist_controls.picker = None;
                self.playlist_controls.form = None;
            }
            Some(playlist_edit_ui::Action::Save) => {
                self.start_playlist_manage(ctx, if delete { Job::Delete } else { Job::Save })
            }
            Some(playlist_edit_ui::Action::ChangeCover) => self.choose_playlist_cover(ctx),
            Some(playlist_edit_ui::Action::RemoveCover) => {
                if let Some(form) = &mut self.playlist_controls.form {
                    form.cover = None;
                    form.preview = None;
                    form.reference.cover_url = None;
                    form.remove_cover = true;
                }
            }
            _ => {}
        }
    }

    fn choose_playlist_cover(&mut self, ctx: &egui::Context) {
        if self.playlist_controls.picker.is_some() {
            return;
        }
        let language = self.language;
        let repaint = self.repaint_target.with_context(ctx);
        let (sender, receiver) = mpsc::sync_channel(1);
        // The native chooser has its own lifetime; closing VK Fast must not
        // wait for a file dialog. A dismissed form drops its reply channel.
        if thread::Builder::new()
            .name("vk-playlist-cover-picker".into())
            .spawn(move || {
                let result = playlist_cover::choose(language);
                let _ = sender.send(result);
                repaint.request_repaint();
            })
            .is_ok()
        {
            self.playlist_controls.picker = Some(receiver);
        }
    }

    pub(super) fn poll_playlist_actions(&mut self, ctx: &egui::Context) {
        self.poll_playlist_follow(ctx);
        self.poll_playlist_manage(ctx);
        if let Some(receiver) = &self.playlist_controls.picker {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Ok(None)),
            };
            if let Some(result) = result {
                self.playlist_controls.picker = None;
                if let Some(form) = &mut self.playlist_controls.form {
                    match result {
                        Ok(Some(cover)) => {
                            form.preview = Some(ctx.load_texture(
                                "playlist-cover-preview",
                                cover.image.clone(),
                                egui::TextureOptions::LINEAR,
                            ));
                            form.cover = Some(cover);
                            form.remove_cover = false;
                            form.error = None;
                        }
                        Ok(None) => {}
                        Err(error) => form.error = Some(error.localize(self.language).into()),
                    }
                }
            }
        }
        let Some(request) = &self.playlist_controls.autoplay else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self.playlist_controls.autoplay.take().unwrap();
        if request.canceled.load(Ordering::Acquire)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != request.owner)
        {
            return;
        }
        match result {
            Ok(tracks) => {
                self.start_sidebar_playlist(ctx, request.reference.clone(), tracks, request.shuffle)
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => self.track_menu_error = Some(error),
        }
    }

    fn poll_playlist_manage(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.playlist_controls.request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self.playlist_controls.request.take().unwrap();
        if request.canceled.load(Ordering::Acquire)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != request.owner)
            || self
                .playlist_controls
                .form
                .as_ref()
                .is_none_or(|form| key(&form.reference) != request.target)
        {
            return;
        }
        match result {
            Ok(Managed::Loaded(details)) => {
                self.update_playlist_reference(&details.playlist, details.can_edit);
                let form = self.playlist_controls.form.as_mut().unwrap();
                form.reference = details.playlist;
                form.name = form.reference.title.clone();
                form.description = details.description;
                form.discoverable = details.discoverable;
                form.can_edit = details.can_edit;
                form.can_delete = details.can_delete;
                form.remove_saved = details.is_following;
                form.loading = false;
                form.focus_name = !form.delete;
                if (!form.delete && !form.can_edit) || (form.delete && !form.can_delete) {
                    form.error = Some(
                        self.language
                            .text(
                                "VK не разрешает изменить этот плейлист.",
                                "VK does not allow this playlist to be changed.",
                            )
                            .into(),
                    );
                }
            }
            Ok(Managed::Saved(details)) => {
                let form = self.playlist_controls.form.take().unwrap();
                let can_edit = details
                    .as_ref()
                    .map_or(form.can_edit, |details| details.can_edit);
                let mut reference = form.reference;
                reference.title = form.name.trim().into();
                let description = details
                    .as_ref()
                    .map_or(form.description, |details| details.description.clone());
                if let Some(details) = details {
                    reference = details.playlist;
                }
                self.update_playlist_reference(&reference, can_edit);
                let active = self.page.detail_key();
                let aliases = [
                    MusicPage::Playlist(reference.clone()).detail_key(),
                    MusicPage::Album(reference.clone()).detail_key(),
                ];
                for route in [
                    MusicPage::Playlist(reference.clone()),
                    MusicPage::Album(reference.clone()),
                ] {
                    if route.detail_key() != active {
                        if let Some(key) = route.detail_key() {
                            self.details.remove(&key);
                        }
                        continue;
                    }
                    if let Some(page) = route
                        .detail_key()
                        .and_then(|key| self.details.get_mut(&key))
                        && let DetailPage::Playlist(page) | DetailPage::Album(page) =
                            Arc::make_mut(page)
                    {
                        page.album = reference.clone();
                        page.description = (!description.is_empty()).then(|| description.clone());
                    }
                }
                self.playlist_request = None;
                self.load_playlists(ctx, true);
                if aliases.contains(&active) {
                    self.ensure_detail(ctx, true);
                }
                self.playlist_notice(self.language.text("Изменения сохранены", "Changes saved"));
            }
            Ok(Managed::Deleted) => {
                let form = self.playlist_controls.form.take().unwrap();
                let target = key(&form.reference);
                self.playlist_items
                    .retain(|item| key(&item.reference) != target);
                self.editable_playlists.retain(|item| key(item) != target);
                self.playlist_controls.pins.retain(|pin| *pin != target);
                let _ = write_pins(request.owner, &self.playlist_controls.pins);
                let routes = [
                    MusicPage::Playlist(form.reference.clone()).detail_key(),
                    MusicPage::Album(form.reference).detail_key(),
                ];
                self.details
                    .retain(|key, _| !routes.iter().any(|route| route.as_deref() == Some(key)));
                if routes.contains(&self.page.detail_key()) {
                    self.navigate_music(ctx, MusicPage::Library);
                }
                self.back_history
                    .retain(|location| !routes.contains(&location.page.detail_key()));
                self.forward_history
                    .retain(|location| !routes.contains(&location.page.detail_key()));
                self.playlist_request = None;
                self.load_playlists(ctx, true);
                self.playlist_notice(self.language.text(
                    "Плейлист убран из библиотеки",
                    "Playlist removed from your library",
                ));
            }
            Err(WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(WebAudioError::WrongAccount));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => {
                if let Some(form) = &mut self.playlist_controls.form {
                    form.loading = false;
                    form.error = Some(if error == WebAudioError::Rejected {
                        self.language
                            .text(
                                "VK не разрешил выполнить это действие.",
                                "VK did not allow this action.",
                            )
                            .into()
                    } else {
                        error.localized(self.language)
                    });
                }
            }
        }
        ctx.request_repaint();
    }
}

fn timed_notice(
    ctx: &egui::Context,
    language: Language,
    deadline: &mut Option<Instant>,
    message: &str,
    id: &str,
) -> bool {
    let Some(until) = *deadline else { return false };
    if Instant::now() >= until {
        *deadline = None;
        return false;
    }
    let mut cancel = false;
    egui::Area::new(egui::Id::new(id))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -112.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            Frame::popup(ui.style()).inner_margin(12).show(ui, |ui| {
                ui.set_max_width(360.0);
                ui.horizontal(|ui| {
                    ui.label(message);
                    let response = ui.button(language.text("Отмена", "Cancel"));
                    cancel = response.clicked();
                    release_pointer_focus(&response);
                });
            });
        });
    if cancel {
        *deadline = None;
    } else {
        ctx.request_repaint_after(until.saturating_duration_since(Instant::now()));
    }
    true
}
