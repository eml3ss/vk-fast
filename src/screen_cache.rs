//! The last Home and library sidebar, shown at startup before VK answers and
//! replaced by the fresh pages as soon as they arrive. Only display metadata
//! is stored: no access keys, audio links or tracks.
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
}

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
}
