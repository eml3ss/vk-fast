//! The last Home and library sidebar, shown at startup before VK answers and
//! replaced by the fresh pages as soon as they arrive. Only display metadata
//! is stored: no access keys or audio links, and of tracks only the few a
//! friend's card shows, as their title, artists, length and cover.
use std::io;

use serde::{Deserialize, Serialize};

use crate::library_cache::{cache_path, load_private_bytes, save_private_bytes};

const FORMAT_VERSION: u32 = 1;
const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub owner_id: i64,
    pub playlist_id: i64,
    pub title: String,
    pub subtitle: String,
    pub cover_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub id: String,
    pub title: String,
    /// `common` options, or empty with `entity_id` for an artist mix.
    pub options: Vec<(String, String)>,
    pub entity_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistMix {
    pub title: String,
    pub description: String,
    pub photo_url: Option<String>,
    pub entity_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Friend {
    pub owner_id: i64,
    pub playlist_id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub owner_name: String,
    pub owner_photo: Option<String>,
    pub match_percent: Option<u8>,
    pub color: Option<[u8; 3]>,
    /// The tracks on the card; an older snapshot has none.
    #[serde(default)]
    pub tracks: Vec<FriendTrack>,
}

/// A track as a friend's card shows it: no link, no access key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendTrack {
    pub owner_id: i64,
    pub id: i64,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    pub artist: String,
    #[serde(default)]
    pub artists: Vec<(String, String)>,
    pub duration: u32,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub explicit: bool,
}
/// The most tracks a friend's card shows.
pub const FRIEND_TRACKS: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Home {
    pub cards: Vec<Card>,
    pub genres: Vec<Tile>,
    pub moods: Vec<Tile>,
    pub artist_mixes: Vec<ArtistMix>,
    pub friends: Vec<Friend>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarItem {
    pub owner_id: i64,
    pub id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub album: bool,
    pub subtitle: String,
    pub can_edit: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarArtist {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub playlists: Vec<SidebarItem>,
    pub artists: Vec<SidebarArtist>,
}

/// "Date added" of tracks in the account's own playlists, by playlist:
/// `"owner_id"_"playlist_id"` -> (track owner, track id, Unix seconds).
pub type PlaylistDates = std::collections::HashMap<String, Vec<(i64, i64, i64)>>;

#[derive(Serialize, Deserialize)]
struct Stored<T> {
    format_version: u32,
    account_id: i64,
    data: T,
}

fn path(account_id: i64, name: &str) -> io::Result<std::path::PathBuf> {
    Ok(cache_path(account_id)?.with_extension(format!("{name}.json")))
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid screen cache")
}

fn load<T: for<'de> Deserialize<'de>>(account_id: i64, name: &str) -> io::Result<Option<T>> {
    let Some(bytes) = load_private_bytes(&path(account_id, name)?, MAX_BYTES)? else {
        return Ok(None);
    };
    let stored: Stored<T> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if stored.format_version != FORMAT_VERSION || stored.account_id != account_id {
        return Err(invalid());
    }
    Ok(Some(stored.data))
}

fn save<T: Serialize>(account_id: i64, name: &str, data: &T) -> io::Result<()> {
    let stored = Stored {
        format_version: FORMAT_VERSION,
        account_id,
        data,
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| invalid())?;
    save_private_bytes(&path(account_id, name)?, &bytes, MAX_BYTES)
}

/// Read off the UI thread.
pub fn load_home(account_id: i64) -> io::Result<Option<Home>> {
    load(account_id, "home")
}

/// Write off the UI thread.
pub fn save_home(account_id: i64, home: &Home) -> io::Result<()> {
    save(account_id, "home", home)
}

/// Album and release names by album: (owner_id, album_id, title). Only
/// display names: no access keys.
pub type AlbumNames = Vec<(i64, i64, String)>;

/// The most album names kept, within the file size limit.
pub const MAX_ALBUM_NAMES: usize = 20_000;

/// Read off the UI thread; malformed names are dropped one by one.
pub fn load_album_names(account_id: i64) -> io::Result<Option<AlbumNames>> {
    Ok(load::<AlbumNames>(account_id, "albums")?.map(|mut names| {
        names.retain(|(owner, id, title)| valid_album_name(*owner, *id, title));
        names.truncate(MAX_ALBUM_NAMES);
        names
    }))
}

/// Write off the UI thread.
pub fn save_album_names(account_id: i64, names: &AlbumNames) -> io::Result<()> {
    if names.len() > MAX_ALBUM_NAMES
        || !names
            .iter()
            .all(|(owner, id, title)| valid_album_name(*owner, *id, title))
    {
        return Err(invalid());
    }
    save(account_id, "albums", names)
}

fn valid_album_name(owner: i64, id: i64, title: &str) -> bool {
    owner != 0
        && id > 0
        && !title.trim().is_empty()
        && title.len() <= 512
        && !title.chars().any(char::is_control)
}

pub fn load_playlist_dates(account_id: i64) -> io::Result<Option<PlaylistDates>> {
    load(account_id, "dates")
}

pub fn save_playlist_dates(account_id: i64, dates: &PlaylistDates) -> io::Result<()> {
    save(account_id, "dates", dates)
}

pub fn load_library(account_id: i64) -> io::Result<Option<Library>> {
    load(account_id, "sidebar")
}

pub fn save_library(account_id: i64, library: &Library) -> io::Result<()> {
    save(account_id, "sidebar", library)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_roundtrip_and_hold_no_access_keys() {
        let home = Home {
            cards: vec![Card {
                id: "for-you".into(),
                owner_id: -1,
                playlist_id: 7,
                title: "Synthetic".into(),
                ..Default::default()
            }],
            genres: vec![Tile {
                id: "rock".into(),
                title: "Rock".into(),
                options: vec![("additionals".into(), "rock".into())],
                entity_id: None,
            }],
            ..Default::default()
        };
        let bytes = serde_json::to_vec(&Stored {
            format_version: FORMAT_VERSION,
            account_id: 42,
            data: &home,
        })
        .unwrap();
        let restored: Stored<Home> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.data, home);
        assert_eq!(restored.account_id, 42);
        assert!(!String::from_utf8(bytes).unwrap().contains("access"));
    }

    #[test]
    fn friends_keep_their_card_tracks_without_links_and_old_snapshots_still_load() {
        let home = Home {
            friends: vec![Friend {
                owner_id: 7,
                playlist_id: 3,
                title: "Synthetic friend list".into(),
                tracks: vec![FriendTrack {
                    owner_id: 7,
                    id: 11,
                    title: "Synthetic track".into(),
                    artist: "Synthetic artist".into(),
                    artists: vec![("1".into(), "Synthetic artist".into())],
                    duration: 120,
                    explicit: true,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = serde_json::to_vec(&Stored {
            format_version: FORMAT_VERSION,
            account_id: 42,
            data: &home,
        })
        .unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("url\":\"http") && !text.contains("access"));
        let restored: Stored<Home> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.data, home);
        // A snapshot from before card tracks were kept.
        let old = r#"{"format_version":1,"account_id":42,"data":{"cards":[],"genres":[],
            "moods":[],"artist_mixes":[],"friends":[{"owner_id":7,"playlist_id":3,
            "title":"T","cover_url":null,"owner_name":"N","owner_photo":null,
            "match_percent":90,"color":null}]}}"#;
        let restored: Stored<Home> = serde_json::from_str(old).unwrap();
        assert!(restored.data.friends[0].tracks.is_empty());
    }

    #[test]
    fn album_names_keep_only_plain_titles() {
        assert!(valid_album_name(
            -2000928961,
            28928961,
            "BANANATEKK (Versions)"
        ));
        for (owner, id, title) in [
            (0, 1, "Album"),
            (-1, 0, "Album"),
            (-1, 1, "  "),
            (-1, 1, "Line\nbreak"),
        ] {
            assert!(!valid_album_name(owner, id, title));
        }
        assert!(!valid_album_name(-1, 1, &"x".repeat(513)));
    }
}
