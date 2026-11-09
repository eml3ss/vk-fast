//! Shows the last Home and library sidebar at once on startup, then lets the
//! regular VK requests replace them. Reads and writes stay off the UI thread.
use super::*;
use crate::screen_cache::{self, Card, Friend, SidebarArtist, SidebarItem, Tile};
use crate::web_audio::{CatalogMix, CollectionKind, FollowedArtist, FriendPlaylist, MixTile};

pub(super) type Snapshot = (
    Option<screen_cache::Home>,
    Option<screen_cache::Library>,
    Option<screen_cache::PlaylistDates>,
);

fn playlist_key(reference: &AlbumRef) -> String {
    format!("{}_{}", reference.owner_id, reference.id)
}

fn tile(tile: &MixTile) -> Tile {
    let (options, entity_id) = match &tile.mix {
        CatalogMix::Common { options } => (options.clone(), None),
        CatalogMix::Artist { entity_id } => (Vec::new(), Some(entity_id.clone())),
    };
    Tile {
        id: tile.id.clone(),
        title: tile.title.clone(),
        options,
        entity_id,
    }
}

fn mix_tile(tile: Tile) -> MixTile {
    MixTile {
        id: tile.id,
        title: tile.title,
        mix: match tile.entity_id {
            Some(entity_id) => CatalogMix::Artist { entity_id },
            None => CatalogMix::Common {
                options: tile.options,
            },
        },
    }
}

impl MusicApp {
    /// Read this account's saved Home and sidebar once per sign-in.
    pub(super) fn start_screen_cache(&mut self, ctx: &egui::Context) {
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        if cfg!(test) || self.demo || self.screen_cache_owner == Some(owner) {
            return;
        }
        self.screen_cache_owner = Some(owner);
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        if let Ok(worker) = thread::Builder::new()
            .name("vk-screen-cache".into())
            .spawn(move || {
                let home = screen_cache::load_home(owner).ok().flatten();
                let library = screen_cache::load_library(owner).ok().flatten();
                let dates = screen_cache::load_playlist_dates(owner).ok().flatten();
                let _ = sender.send((home, library, dates));
                ctx.request_repaint();
            })
        {
            self.workers.push(worker);
            self.screen_cache = Some((owner, receiver));
        }
    }

    /// Cached content fills only what VK has not answered yet.
    pub(super) fn poll_screen_cache(&mut self) {
        let Some((owner, receiver)) = &self.screen_cache else {
            return;
        };
        let owner = *owner;
        let (home, library, dates) = match receiver.try_recv() {
            Ok(snapshot) => snapshot,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => (None, None, None),
        };
        self.screen_cache = None;
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.profile.id != owner)
        {
            return;
        }
        if let Some(dates) = dates {
            for (playlist, rows) in dates {
                let known = self.playlist_dates.entry(playlist).or_default();
                for (owner, id, added) in rows {
                    known.entry((owner, id)).or_insert(added);
                }
            }
            self.apply_playlist_dates();
        }
        if let Some(home) = home {
            if self.home_sources.is_empty() {
                self.home_sources = home
                    .cards
                    .into_iter()
                    .map(|card| crate::web_audio::HomeCollection {
                        id: card.id,
                        owner_id: card.owner_id,
                        playlist_id: card.playlist_id,
                        title: card.title,
                        subtitle: card.subtitle,
                        cover_url: card.cover_url,
                        access_key: None,
                        tracks: Vec::new(),
                    })
                    .collect();
            }
            let catalog = &mut self.home_catalog;
            if catalog.genres.is_empty()
                && catalog.moods.is_empty()
                && catalog.friends.is_empty()
                && catalog.artist_mixes.is_empty()
            {
                catalog.genres = home.genres.into_iter().map(mix_tile).collect();
                catalog.moods = home.moods.into_iter().map(mix_tile).collect();
                catalog.artist_mixes = home
                    .artist_mixes
                    .into_iter()
                    .map(|mix| crate::web_audio::ArtistMix {
                        title: mix.title,
                        description: mix.description,
                        photo_url: mix.photo_url,
                        mix: CatalogMix::Artist {
                            entity_id: mix.entity_id,
                        },
                    })
                    .collect();
                catalog.friends = home
                    .friends
                    .into_iter()
                    .map(|friend| FriendPlaylist {
                        playlist: AlbumRef {
                            id: friend.playlist_id,
                            owner_id: friend.owner_id,
                            title: friend.title,
                            access_key: None,
                            cover_url: friend.cover_url,
                        },
                        owner_name: friend.owner_name,
                        owner_photo: friend.owner_photo,
                        match_percent: friend.match_percent,
                        color: friend.color,
                        tracks: Vec::new(),
                    })
                    .collect();
            }
            self.rebuild_home_collections();
        }
        if let Some(library) = library {
            if self.playlist_items.is_empty() {
                self.editable_playlists = library
                    .playlists
                    .iter()
                    .filter(|item| item.can_edit)
                    .map(|item| AlbumRef {
                        id: item.id,
                        owner_id: item.owner_id,
                        title: item.title.clone(),
                        access_key: None,
                        cover_url: item.cover_url.clone(),
                    })
                    .collect();
                self.playlist_items = library
                    .playlists
                    .into_iter()
                    .map(|item| playlist_ui::PlaylistItem {
                        reference: AlbumRef {
                            id: item.id,
                            owner_id: item.owner_id,
                            title: item.title,
                            access_key: None,
                            cover_url: item.cover_url,
                        },
                        kind: if item.album {
                            CollectionKind::Album
                        } else {
                            CollectionKind::Playlist
                        },
                        subtitle: item.subtitle,
                        can_edit: item.can_edit,
                        pinned: false,
                    })
                    .collect();
                self.sync_playlist_pins();
            }
            if self.library_browser.artists.is_empty() {
                self.library_browser.artists = library
                    .artists
                    .into_iter()
                    .map(|artist| FollowedArtist {
                        artist: ArtistRef {
                            id: artist.id,
                            name: artist.name,
                        },
                        image_url: artist.image_url,
                    })
                    .collect();
            }
        }
    }

    /// The account's own playlist open now, if any.
    pub(super) fn own_playlist(&self) -> Option<AlbumRef> {
        let owner = self.session.as_ref()?.profile.id;
        match &self.page {
            MusicPage::Playlist(reference) if reference.owner_id == owner => {
                Some(reference.clone())
            }
            _ => None,
        }
    }

    /// Fill missing dates of own playlists from the saved ones, and save new
    /// dates VK or the background fill brought.
    pub(super) fn apply_playlist_dates(&mut self) {
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        let mut changed = false;
        let keys: Vec<String> = self.details.keys().cloned().collect();
        for detail_key in keys {
            let Some(page) = self.details.get_mut(&detail_key) else {
                continue;
            };
            let DetailPage::Playlist(data) = Arc::make_mut(page) else {
                continue;
            };
            if data.album.owner_id != owner {
                continue;
            }
            let known = self
                .playlist_dates
                .entry(playlist_key(&data.album))
                .or_default();
            for track in &mut data.tracks {
                match track.added {
                    Some(added) => {
                        if known.insert(track.key(), added) != Some(added) {
                            changed = true;
                        }
                    }
                    None => track.added = known.get(&track.key()).copied(),
                }
            }
        }
        if changed && !cfg!(test) && !self.demo {
            let snapshot: screen_cache::PlaylistDates = self
                .playlist_dates
                .iter()
                .map(|(playlist, rows)| {
                    (
                        playlist.clone(),
                        rows.iter()
                            .map(|((o, i), added)| (*o, *i, *added))
                            .collect(),
                    )
                })
                .collect();
            if let Ok(worker) = thread::Builder::new()
                .name("vk-screen-cache-save".into())
                .spawn(move || {
                    let _ = screen_cache::save_playlist_dates(owner, &snapshot);
                })
            {
                self.workers.push(worker);
            }
        }
        self.track_view_cache.invalidate();
    }

    /// Save what Home shows now, after a fresh VK answer.
    pub(super) fn save_home_cache(&mut self) {
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        if cfg!(test) || self.demo || self.home_sources.is_empty() {
            return;
        }
        let catalog = &self.home_catalog;
        let home = screen_cache::Home {
            cards: self
                .home_sources
                .iter()
                .map(|source| Card {
                    id: source.id.clone(),
                    owner_id: source.owner_id,
                    playlist_id: source.playlist_id,
                    title: source.title.clone(),
                    subtitle: source.subtitle.clone(),
                    cover_url: source.cover_url.clone(),
                })
                .collect(),
            genres: catalog.genres.iter().map(tile).collect(),
            moods: catalog.moods.iter().map(tile).collect(),
            artist_mixes: catalog
                .artist_mixes
                .iter()
                .filter_map(|mix| match &mix.mix {
                    CatalogMix::Artist { entity_id } => Some(screen_cache::ArtistMix {
                        title: mix.title.clone(),
                        description: mix.description.clone(),
                        photo_url: mix.photo_url.clone(),
                        entity_id: entity_id.clone(),
                    }),
                    CatalogMix::Common { .. } => None,
                })
                .collect(),
            friends: catalog
                .friends
                .iter()
                .map(|friend| Friend {
                    owner_id: friend.playlist.owner_id,
                    playlist_id: friend.playlist.id,
                    title: friend.playlist.title.clone(),
                    cover_url: friend.playlist.cover_url.clone(),
                    owner_name: friend.owner_name.clone(),
                    owner_photo: friend.owner_photo.clone(),
                    match_percent: friend.match_percent,
                    color: friend.color,
                })
                .collect(),
        };
        if let Ok(worker) = thread::Builder::new()
            .name("vk-screen-cache-save".into())
            .spawn(move || {
                let _ = screen_cache::save_home(owner, &home);
            })
        {
            self.workers.push(worker);
        }
    }

    /// Save the library sidebar after a fresh VK answer.
    pub(super) fn save_library_cache(&mut self) {
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        if cfg!(test) || self.demo {
            return;
        }
        let library = screen_cache::Library {
            playlists: self
                .playlist_items
                .iter()
                .map(|item| SidebarItem {
                    owner_id: item.reference.owner_id,
                    id: item.reference.id,
                    title: item.reference.title.clone(),
                    cover_url: item.reference.cover_url.clone(),
                    album: item.kind == CollectionKind::Album,
                    subtitle: item.subtitle.clone(),
                    can_edit: item.can_edit,
                })
                .collect(),
            artists: self
                .library_browser
                .artists
                .iter()
                .map(|item| SidebarArtist {
                    id: item.artist.id.clone(),
                    name: item.artist.name.clone(),
                    image_url: item.image_url.clone(),
                })
                .collect(),
        };
        if let Ok(worker) = thread::Builder::new()
            .name("vk-screen-cache-save".into())
            .spawn(move || {
                let _ = screen_cache::save_library(owner, &library);
            })
        {
            self.workers.push(worker);
        }
    }
}
