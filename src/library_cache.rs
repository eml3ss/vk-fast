//! Account-bound track metadata for immediate library display. Playable URLs
//! and session credentials are deliberately excluded from this cache format.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::model::{
    AlbumRef, ArtistRef, Track, valid_artist_id, valid_cover_url, valid_release_key,
};

const FORMAT_VERSION: u8 = 1;
// Before this revision generic stream-resolution failures were persisted as
// rights restrictions. Keep their track metadata, but discard that evidence.
const AVAILABILITY_REVISION: u8 = 2;
// Older positive flags could have spread through a shared public track page.
// Refresh those flags, retaining metadata and explicit removals from history.
const MEMBERSHIP_REVISION: u8 = 1;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TRACKS: usize = 50_000;
const MAX_TEXT: usize = 4096;
const MAX_TOTAL_BYTES: u64 = 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTotal {
    format_version: u8,
    account_id: i64,
    total: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibrary {
    format_version: u8,
    account_id: i64,
    #[serde(deserialize_with = "deserialize_tracks")]
    tracks: Vec<StoredTrack>,
}

fn deserialize_tracks<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StoredTrack>, D::Error> {
    struct Tracks;
    impl<'de> serde::de::Visitor<'de> for Tracks {
        type Value = Vec<StoredTrack>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded list of track metadata")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut tracks = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(MAX_TRACKS));
            while let Some(track) = sequence.next_element()? {
                if tracks.len() >= MAX_TRACKS {
                    return Err(serde::de::Error::custom("Too many cached tracks"));
                }
                tracks.push(track);
            }
            Ok(tracks)
        }
    }
    deserializer.deserialize_seq(Tracks)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTrack {
    id: i64,
    owner_id: i64,
    artist: String,
    title: String,
    #[serde(default)]
    subtitle: Option<String>,
    duration: u32,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    artists: Vec<StoredArtist>,
    #[serde(default)]
    album: Option<StoredAlbum>,
    #[serde(default)]
    library_liked: Option<bool>,
    #[serde(default)]
    release_key: Option<(i64, i64)>,
    #[serde(default)]
    library_source_key: Option<(i64, i64)>,
    #[serde(default)]
    membership_revision: Option<u8>,
    #[serde(default)]
    unavailable: Option<bool>,
    #[serde(default)]
    availability_revision: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredArtist {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAlbum {
    id: i64,
    owner_id: i64,
    title: String,
    #[serde(default)]
    cover_url: Option<String>,
}

#[derive(Serialize)]
struct BorrowedLibrary<'a> {
    format_version: u8,
    account_id: i64,
    tracks: Vec<BorrowedTrack<'a>>,
}

#[derive(Serialize)]
struct BorrowedTrack<'a> {
    id: i64,
    owner_id: i64,
    artist: &'a str,
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle: Option<&'a str>,
    duration: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    cover_url: Option<&'a str>,
    #[serde(skip_serializing_if = "<[ArtistRef]>::is_empty")]
    artists: &'a [ArtistRef],
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<BorrowedAlbum<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    library_liked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_key: Option<(i64, i64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    library_source_key: Option<(i64, i64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    membership_revision: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unavailable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    availability_revision: Option<u8>,
}

#[derive(Serialize)]
struct BorrowedAlbum<'a> {
    id: i64,
    owner_id: i64,
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cover_url: Option<&'a str>,
}

fn valid_artist(id: &str, name: &str) -> bool {
    valid_artist_id(id) && !name.trim().is_empty() && valid_text(name)
}

fn valid_album(id: i64, owner_id: i64, title: &str, cover: Option<&str>) -> bool {
    id != 0
        && owner_id != 0
        && valid_text(title)
        && cover.is_none_or(|url| valid_cover_url(url).is_some())
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid VK Fast library cache")
}

fn valid_text(value: &str) -> bool {
    value.len() <= MAX_TEXT && !value.chars().any(char::is_control)
}

fn valid_metadata(id: i64, owner_id: i64, artist: &str, title: &str, cover: Option<&str>) -> bool {
    id > 0
        && owner_id != 0
        && valid_text(artist)
        && valid_text(title)
        && cover.is_none_or(|url| valid_cover_url(url).is_some())
}

fn cache_path(account_id: i64) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        if account_id <= 0 {
            return Err(invalid());
        }
        Ok(crate::app_paths::cache_dir()?
            .join("library")
            .join(format!("{account_id}.json")))
    }
    #[cfg(not(windows))]
    {
        if account_id <= 0 {
            return Err(invalid());
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".cache"))
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "User cache directory unavailable")
            })?;
        Ok(base
            .join("vk-fast/library")
            .join(format!("{account_id}.json")))
    }
}

fn private_dir(path: &Path, create: bool) -> io::Result<()> {
    if create {
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(path) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if create {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        } else if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Library cache is not private",
            ));
        }
    }
    Ok(())
}

fn parent(path: &Path, create: bool) -> io::Result<&Path> {
    let directory = path.parent().ok_or_else(invalid)?;
    let application = directory.parent().ok_or_else(invalid)?;
    if create {
        fs::create_dir_all(application.parent().ok_or_else(invalid)?)?;
    }
    private_dir(application, create)?;
    private_dir(directory, create)?;
    Ok(directory)
}

fn target(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(metadata))
        }
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// A bounded JSON sink prevents serialization from allocating beyond the cache
/// budget even when all 50,000 permitted rows have unusually large metadata.
struct BoundedBytes {
    bytes: Vec<u8>,
    limit: u64,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) as u64 > self.limit {
            return Err(invalid());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn encode_bounded(
    account_id: i64,
    tracks: &[Track],
    max_bytes: u64,
    max_tracks: usize,
) -> io::Result<Vec<u8>> {
    if account_id <= 0
        || tracks.len() > max_tracks
        || max_tracks > MAX_TRACKS
        || max_bytes > MAX_BYTES
    {
        return Err(invalid());
    }
    let mut seen = HashSet::with_capacity(tracks.len());
    let mut stored = Vec::with_capacity(tracks.len());
    for track in tracks {
        if !valid_metadata(
            track.id,
            track.owner_id,
            &track.artist,
            &track.title,
            track.cover_url.as_deref(),
        ) {
            return Err(invalid());
        }
        if track
            .subtitle
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || !valid_text(value))
            || track.artists.len() > 64
            || track
                .artists
                .iter()
                .any(|artist| !valid_artist(&artist.id, &artist.name))
            || track.album.as_ref().is_some_and(|album| {
                !valid_album(
                    album.id,
                    album.owner_id,
                    &album.title,
                    album.cover_url.as_deref(),
                )
            })
            || track.release_key.is_some_and(|key| !valid_release_key(key))
            || track
                .library_source_key
                .is_some_and(|key| !valid_release_key(key))
        {
            return Err(invalid());
        }
        if seen.insert(track.key()) {
            stored.push(BorrowedTrack {
                id: track.id,
                owner_id: track.owner_id,
                artist: &track.artist,
                title: &track.title,
                subtitle: track.subtitle.as_deref(),
                duration: track.duration,
                cover_url: track.cover_url.as_deref(),
                artists: &track.artists,
                album: track.album.as_ref().map(|album| BorrowedAlbum {
                    id: album.id,
                    owner_id: album.owner_id,
                    title: &album.title,
                    cover_url: album.cover_url.as_deref(),
                }),
                library_liked: track.library_liked,
                release_key: track.release_key,
                library_source_key: track.library_source_key,
                membership_revision: (track.library_liked.is_some()
                    || track.library_source_key.is_some())
                .then_some(MEMBERSHIP_REVISION),
                unavailable: track.unavailable,
                availability_revision: track.unavailable.map(|_| AVAILABILITY_REVISION),
            });
        }
    }
    let mut bytes = BoundedBytes {
        bytes: Vec::new(),
        limit: max_bytes,
    };
    serde_json::to_writer(
        &mut bytes,
        &BorrowedLibrary {
            format_version: FORMAT_VERSION,
            account_id,
            tracks: stored,
        },
    )
    .map_err(|_| invalid())?;
    Ok(bytes.bytes)
}

fn load_at(path: &Path, account_id: i64) -> io::Result<Option<Vec<Track>>> {
    load_bounded_at(path, account_id, MAX_BYTES, MAX_TRACKS)
}

/// Shared safe metadata codec for other account-bound caches. Limits cannot
/// exceed the library's hard bounds; playback URLs and access keys are omitted.
pub(crate) fn load_bounded_at(
    path: &Path,
    account_id: i64,
    max_bytes: u64,
    max_tracks: usize,
) -> io::Result<Option<Vec<Track>>> {
    if account_id <= 0 || max_tracks > MAX_TRACKS || max_bytes > MAX_BYTES {
        return Err(invalid());
    }
    let Some(bytes) = load_private_bytes(path, max_bytes)? else {
        return Ok(None);
    };
    decode_bounded(&bytes, account_id, max_bytes, max_tracks).map(Some)
}

/// Read one bounded private state file without interpreting its format.
pub(crate) fn load_private_bytes(path: &Path, max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
    if max_bytes > MAX_BYTES {
        return Err(invalid());
    }
    let Some(metadata) = target(path)? else {
        return Ok(None);
    };
    parent(path, false)?;
    if metadata.len() > max_bytes {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    let file = File::open(path)?;
    let opened = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
            return Err(invalid());
        }
    }
    if !opened.is_file() || opened.len() > max_bytes {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(invalid());
    }
    Ok(Some(bytes))
}

/// Decode the same safe metadata projection used by library and recent state.
pub(crate) fn decode_bounded(
    bytes: &[u8],
    account_id: i64,
    max_bytes: u64,
    max_tracks: usize,
) -> io::Result<Vec<Track>> {
    if account_id <= 0
        || max_tracks > MAX_TRACKS
        || max_bytes > MAX_BYTES
        || bytes.len() as u64 > max_bytes
    {
        return Err(invalid());
    }
    let stored: StoredLibrary = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if stored.format_version != FORMAT_VERSION
        || stored.account_id != account_id
        || stored.tracks.len() > max_tracks
    {
        return Err(invalid());
    }
    let mut seen = HashSet::with_capacity(stored.tracks.len());
    let mut tracks = Vec::with_capacity(stored.tracks.len());
    for track in stored.tracks {
        if !valid_metadata(
            track.id,
            track.owner_id,
            &track.artist,
            &track.title,
            track.cover_url.as_deref(),
        ) {
            return Err(invalid());
        }
        if track
            .subtitle
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || !valid_text(value))
            || track.artists.len() > 64
            || track
                .artists
                .iter()
                .any(|artist| !valid_artist(&artist.id, &artist.name))
            || track.album.as_ref().is_some_and(|album| {
                !valid_album(
                    album.id,
                    album.owner_id,
                    &album.title,
                    album.cover_url.as_deref(),
                )
            })
            || track.release_key.is_some_and(|key| !valid_release_key(key))
            || track
                .library_source_key
                .is_some_and(|key| !valid_release_key(key))
        {
            return Err(invalid());
        }
        if seen.insert((track.owner_id, track.id)) {
            tracks.push(Track {
                id: track.id,
                owner_id: track.owner_id,
                artist: track.artist,
                title: track.title,
                subtitle: track.subtitle,
                duration: track.duration,
                url: None,
                cover_url: track.cover_url.as_deref().and_then(valid_cover_url),
                artists: track
                    .artists
                    .into_iter()
                    .map(|artist| ArtistRef {
                        id: artist.id,
                        name: artist.name,
                    })
                    .collect(),
                album: track.album.map(|album| AlbumRef {
                    id: album.id,
                    owner_id: album.owner_id,
                    title: album.title,
                    access_key: None,
                    cover_url: album.cover_url.as_deref().and_then(valid_cover_url),
                }),
                library_liked: track.library_liked.filter(|liked| {
                    !liked || track.membership_revision == Some(MEMBERSHIP_REVISION)
                }),
                release_key: track.release_key,
                library_source_key: track
                    .library_source_key
                    .filter(|_| track.membership_revision == Some(MEMBERSHIP_REVISION)),
                unavailable: track
                    .unavailable
                    .filter(|_| track.availability_revision == Some(AVAILABILITY_REVISION)),
            });
        }
    }
    Ok(tracks)
}

fn save_at(path: &Path, account_id: i64, tracks: &[Track]) -> io::Result<()> {
    save_bounded_at(path, account_id, tracks, MAX_BYTES, MAX_TRACKS)
}

pub(crate) fn save_bounded_at(
    path: &Path,
    account_id: i64,
    tracks: &[Track],
    max_bytes: u64,
    max_tracks: usize,
) -> io::Result<()> {
    let bytes = encode_bounded(account_id, tracks, max_bytes, max_tracks)?;
    save_private_bytes(path, &bytes, max_bytes)
}

/// Atomically replace a bounded private state file; a failed write keeps it.
pub(crate) fn save_private_bytes(path: &Path, bytes: &[u8], max_bytes: u64) -> io::Result<()> {
    if max_bytes > MAX_BYTES || bytes.len() as u64 > max_bytes {
        return Err(invalid());
    }
    let directory = parent(path, true)?;
    target(path)?;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| io::Error::other("Library temporary name unavailable"))?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = directory.join(format!(".library-{suffix}.tmp"));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Returns saved metadata only; audio URLs always need to be resolved afresh.
pub fn load(account_id: i64) -> io::Result<Option<Vec<Track>>> {
    load_at(&cache_path(account_id)?, account_id)
}

/// Atomically replaces one account's cached metadata. Call off the UI thread.
pub fn save(account_id: i64, tracks: &[Track]) -> io::Result<()> {
    save_at(&cache_path(account_id)?, account_id, tracks)
}

fn total_path(account_id: i64) -> io::Result<PathBuf> {
    Ok(cache_path(account_id)?.with_extension("count.json"))
}

fn load_total_at(path: &Path, account_id: i64) -> io::Result<Option<usize>> {
    if account_id <= 0 {
        return Err(invalid());
    }
    let Some(bytes) = load_private_bytes(path, MAX_TOTAL_BYTES)? else {
        return Ok(None);
    };
    let stored: StoredTotal = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if stored.format_version != FORMAT_VERSION || stored.account_id != account_id {
        return Err(invalid());
    }
    Ok(Some(stored.total))
}

fn save_total_at(path: &Path, account_id: i64, total: usize) -> io::Result<()> {
    if account_id <= 0 {
        return Err(invalid());
    }
    let stored = StoredTotal {
        format_version: FORMAT_VERSION,
        account_id,
        total,
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| invalid())?;
    save_private_bytes(path, &bytes, MAX_TOTAL_BYTES)
}

/// Last server-reported library size, independent of the cached track slice.
pub fn load_total(account_id: i64) -> io::Result<Option<usize>> {
    load_total_at(&total_path(account_id)?, account_id)
}

/// Call only with an authoritative VK count, off the UI thread. Track metadata
/// updates never overwrite this file with the number of loaded rows.
pub fn save_total(account_id: i64, total: usize) -> io::Result<()> {
    save_total_at(&total_path(account_id)?, account_id, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let root = std::env::temp_dir().join(format!("vk-fast-library-test-{suffix}"));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self, account_id: i64) -> PathBuf {
            self.0
                .join("vk-fast/library")
                .join(format!("{account_id}.json"))
        }
        fn total_path(&self, account_id: i64) -> PathBuf {
            self.path(account_id).with_extension("count.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn track(id: i64) -> Track {
        Track {
            id,
            owner_id: -7,
            artist: "Исполнитель".into(),
            title: format!("Track {id}"),
            subtitle: None,
            duration: 180,
            url: Some("https://audio.example.test/play?access_token=synthetic-secret".into()),
            cover_url: Some("https://images.example.test/cover.jpg".into()),
            artists: Vec::new(),
            album: None,
            library_liked: None,
            release_key: None,
            library_source_key: None,
            unavailable: None,
        }
    }

    #[test]
    fn restriction_state_roundtrips_without_streams_and_old_caches_remain_unknown() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let mut blocked = track(1);
        blocked.unavailable = Some(true);
        let mut cleared = track(2);
        cleared.unavailable = Some(false);
        save_at(&path, 42, &[blocked, cleared, track(3)]).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let rows = value["tracks"].as_array().unwrap();
        assert_eq!(rows[0]["unavailable"], true);
        assert_eq!(rows[0]["availability_revision"], AVAILABILITY_REVISION);
        assert_eq!(rows[1]["unavailable"], false);
        assert!(rows[2].get("unavailable").is_none());
        assert!(rows.iter().all(|row| row.get("url").is_none()));
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(
            restored
                .iter()
                .map(|track| track.unavailable)
                .collect::<Vec<_>>(),
            [Some(true), Some(false), None]
        );
        assert!(restored.iter().all(|track| track.url.is_none()));

        // Old releases persisted generic resolver failures as restrictions.
        // Recover the metadata without keeping that unverified grey status.
        let mut legacy = value.clone();
        for row in legacy["tracks"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("availability_revision");
        }
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let recovered = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(
            recovered.iter().map(Track::key).collect::<Vec<_>>(),
            [(-7, 1), (-7, 2), (-7, 3)]
        );
        assert!(recovered.iter().all(|track| track.unavailable.is_none()));
        assert_eq!(recovered[0].artist, "Исполнитель");
        assert_eq!(recovered[0].title, "Track 1");

        for row in value["tracks"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("unavailable");
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            load_at(&path, 42)
                .unwrap()
                .unwrap()
                .iter()
                .all(|track| track.unavailable.is_none())
        );
        value["tracks"][0]["unavailable"] = serde_json::json!("not-a-boolean");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_at(&path, 42).is_err());
    }

    #[test]
    fn authoritative_total_survives_reopening_and_partial_track_cache_updates() {
        let fixture = Fixture::new();
        let total = fixture.total_path(42);
        assert_eq!(load_total_at(&total, 42).unwrap(), None);
        assert!(!fixture.0.join("vk-fast").exists());
        save_at(&fixture.path(42), 42, &[track(1), track(2)]).unwrap();
        let tracks_before = fs::read(fixture.path(42)).unwrap();
        save_total_at(&total, 42, 638).unwrap();
        assert_eq!(load_total_at(&total, 42).unwrap(), Some(638));
        assert_eq!(fs::read(fixture.path(42)).unwrap(), tracks_before);
        save_at(&fixture.path(42), 42, &[track(1), track(2), track(3)]).unwrap();
        assert_eq!(load_total_at(&total, 42).unwrap(), Some(638));
        assert_eq!(load_at(&fixture.path(42), 42).unwrap().unwrap().len(), 3);
        save_total_at(&total, 42, 0).unwrap();
        assert_eq!(load_total_at(&total, 42).unwrap(), Some(0));
        // The server can report more tracks than the bounded metadata cache.
        save_total_at(&total, 42, MAX_TRACKS + 100).unwrap();
        assert_eq!(load_total_at(&total, 42).unwrap(), Some(MAX_TRACKS + 100));
    }

    #[test]
    fn cached_totals_are_account_bound_and_invalid_writes_preserve_the_previous_value() {
        let fixture = Fixture::new();
        let first = fixture.total_path(42);
        let second = fixture.total_path(77);
        save_total_at(&first, 42, 638).unwrap();
        save_total_at(&second, 77, 123).unwrap();
        assert_eq!(load_total_at(&first, 42).unwrap(), Some(638));
        assert_eq!(load_total_at(&second, 77).unwrap(), Some(123));
        let original = fs::read(&first).unwrap();
        assert!(load_total_at(&first, 77).is_err());
        for invalid_owner in [0, -1] {
            assert!(save_total_at(&first, invalid_owner, 10).is_err());
            assert!(load_total_at(&first, invalid_owner).is_err());
            assert!(load_total(invalid_owner).is_err());
            assert!(save_total(invalid_owner, 10).is_err());
        }
        assert_eq!(fs::read(&first).unwrap(), original);
        assert_eq!(load_total_at(&first, 42).unwrap(), Some(638));
    }

    #[test]
    fn malformed_or_oversized_total_metadata_is_rejected_without_deleting_it() {
        let fixture = Fixture::new();
        let path = fixture.total_path(42);
        save_total_at(&path, 42, 638).unwrap();
        for bad in [
            r#"{"format_version":2,"account_id":42,"total":638}"#.to_owned(),
            r#"{"format_version":1,"account_id":77,"total":638}"#.to_owned(),
            r#"{"format_version":1,"account_id":42,"total":-1}"#.to_owned(),
            r#"{"format_version":1,"account_id":42,"total":1.5}"#.to_owned(),
            r#"{"format_version":1,"account_id":42,"total":"638"}"#.to_owned(),
            r#"{"format_version":1,"account_id":42,"total":null}"#.to_owned(),
            r#"{"format_version":1,"account_id":42,"total":18446744073709551616}"#.to_owned(),
            r#"{"format_version":1,"account_id":42,"total":638,"access_token":"synthetic"}"#
                .to_owned(),
            r#"{"format_version":1,"account_id":42}"#.to_owned(),
            "{".to_owned(),
            " ".repeat(MAX_TOTAL_BYTES as usize + 1),
        ] {
            fs::write(&path, &bad).unwrap();
            assert!(
                load_total_at(&path, 42).is_err(),
                "Accepted bad metadata: {bad}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        }
        save_total_at(&path, 42, 700).unwrap();
        assert_eq!(load_total_at(&path, 42).unwrap(), Some(700));
    }

    #[test]
    fn version_subtitle_survives_restart_and_old_cache_without_it_still_loads() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let mut slowed = track(1);
        slowed.subtitle = Some("Slowed".into());
        save_at(&path, 42, &[slowed, track(2)]).unwrap();
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(restored[0].subtitle.as_deref(), Some("Slowed"));
        assert_eq!(restored[0].display_title(), "Track 1 Slowed");
        assert!(restored[1].subtitle.is_none());
        let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for track in old["tracks"].as_array_mut().unwrap() {
            track.as_object_mut().unwrap().remove("subtitle");
        }
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert!(restored.iter().all(|track| track.subtitle.is_none()));
    }

    #[test]
    fn legacy_positive_flags_are_rechecked_without_losing_metadata_or_explicit_removals() {
        let mut removed = track(901);
        removed.owner_id = 42;
        removed.library_liked = Some(false);
        removed.library_source_key = Some((-7, 81));
        let mut old_positive = track(8);
        old_positive.library_liked = Some(true);
        old_positive.release_key = Some((-7, 80));
        let bytes = encode_bounded(42, &[removed, old_positive], MAX_BYTES, MAX_TRACKS).unwrap();
        let mut stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for row in stored["tracks"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("membership_revision");
        }
        let bytes = serde_json::to_vec(&stored).unwrap();
        let restored = decode_bounded(&bytes, 42, MAX_BYTES, MAX_TRACKS).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].library_liked, Some(false));
        assert_eq!(restored[0].library_source_key, None);
        assert_eq!(restored[1].library_liked, None);
        assert_eq!(restored[1].release_key, Some((-7, 80)));
        assert_eq!(restored[1].title, "Track 8");
    }

    #[test]
    fn explicit_likes_and_release_identity_survive_restart_without_credentials() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let mut unliked_own_copy = track(901);
        unliked_own_copy.owner_id = 42;
        unliked_own_copy.library_liked = Some(false);
        unliked_own_copy.release_key = Some((-7, 8));
        unliked_own_copy.library_source_key = Some((-7, 80));
        unliked_own_copy.album = Some(AlbumRef {
            id: 9,
            owner_id: -7,
            title: "Release".into(),
            access_key: Some("synthetic-library-secret".into()),
            cover_url: None,
        });
        let mut liked_source = track(8);
        liked_source.library_liked = Some(true);
        liked_source.release_key = Some((-7, 8));
        save_at(&path, 42, &[unliked_own_copy, liked_source, track(3)]).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("synthetic-secret") && !raw.contains("synthetic-library-secret"));
        assert!(!raw.contains("access_key") && !raw.contains("\"url\""));
        let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(stored["tracks"][0]["library_liked"], false);
        assert_eq!(
            stored["tracks"][0]["release_key"],
            serde_json::json!([-7, 8])
        );
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(restored[0].key(), (42, 901));
        assert_eq!(restored[0].library_liked, Some(false));
        assert_eq!(restored[0].library_source_key, Some((-7, 80)));
        assert_eq!(restored[1].library_liked, Some(true));
        assert_eq!(restored[0].release_key, restored[1].release_key);
        assert_eq!(restored[2].library_liked, None);
        assert_eq!(restored[2].release_key, None);
        assert!(restored.iter().all(|row| row.url.is_none()));

        // Recently played uses this same bounded metadata codec. Explicit
        // false must survive even when its old copy is owned by the account.
        let recent_path = fixture.0.join("vk-fast/recent/42.json");
        save_bounded_at(&recent_path, 42, &restored, 256 * 1024, 32).unwrap();
        let recent = load_bounded_at(&recent_path, 42, 256 * 1024, 32)
            .unwrap()
            .unwrap();
        assert_eq!(recent[0].library_liked, Some(false));
        assert_eq!(recent[0].release_key, Some((-7, 8)));
        assert_eq!(recent[0].library_source_key, Some((-7, 80)));

        let mut old = stored;
        for row in old["tracks"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("library_liked");
            row.as_object_mut().unwrap().remove("release_key");
            row.as_object_mut().unwrap().remove("library_source_key");
        }
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let old_rows = load_at(&path, 42).unwrap().unwrap();
        assert!(
            old_rows
                .iter()
                .all(|row| row.library_liked.is_none() && row.release_key.is_none())
        );
    }

    #[test]
    fn invalid_membership_and_release_identity_are_rejected_without_replacing_cache() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
        let original = fs::read(&path).unwrap();
        for invalid_key in [(0, 7), (42, 0), (42, -7)] {
            let mut invalid_track = track(2);
            invalid_track.release_key = Some(invalid_key);
            assert!(save_at(&path, 42, &[invalid_track]).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
        }
        for (field, value) in [
            ("release_key", serde_json::json!([0, 7])),
            ("release_key", serde_json::json!([42, 0])),
            ("release_key", serde_json::json!([42, -7])),
            ("release_key", serde_json::json!([42, 7, 8])),
            ("release_key", serde_json::json!(["42", "7"])),
            ("release_key", serde_json::json!("42_7_secret")),
            ("library_liked", serde_json::json!(1)),
            ("library_liked", serde_json::json!("false")),
            ("track_code", serde_json::json!("forbidden")),
            ("access_key", serde_json::json!("forbidden")),
        ] {
            let mut injected: serde_json::Value = serde_json::from_slice(&original).unwrap();
            injected["tracks"][0][field] = value;
            fs::write(&path, serde_json::to_vec(&injected).unwrap()).unwrap();
            assert!(load_at(&path, 42).is_err(), "invalid {field}");
        }
    }

    #[test]
    fn metadata_survives_reopening_without_playable_urls_and_keeps_accounts_separate() {
        let fixture = Fixture::new();
        let first = fixture.path(42);
        assert!(load_at(&first, 42).unwrap().is_none());
        assert!(!fixture.0.join("vk-fast").exists());
        save_at(&first, 42, &[track(1), track(2)]).unwrap();
        let bytes = fs::read(&first).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["account_id"], 42);
        for item in json["tracks"].as_array().unwrap() {
            assert!(item.get("url").is_none());
        }
        assert!(
            !String::from_utf8(bytes)
                .unwrap()
                .contains("synthetic-secret")
        );
        let restored = load_at(&first, 42).unwrap().unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].artist, "Исполнитель");
        assert_eq!(restored[0].title, "Track 1");
        assert_eq!(
            restored[0].cover_url.as_deref(),
            Some("https://images.example.test/cover.jpg")
        );
        assert!(restored.iter().all(|item| item.url.is_none()));
        assert!(load_at(&first, 43).is_err());
        assert!(load_at(&fixture.path(43), 43).unwrap().is_none());
        save_at(&fixture.path(43), 43, &[track(3)]).unwrap();
        assert_eq!(load_at(&first, 42).unwrap().unwrap()[0].id, 1);
        assert_eq!(load_at(&fixture.path(43), 43).unwrap().unwrap()[0].id, 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&first).unwrap().permissions().mode() & 0o777,
                0o600
            );
            for path in [
                first.parent().unwrap(),
                first.parent().unwrap().parent().unwrap(),
            ] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
    }

    #[test]
    fn canonical_artist_and_album_metadata_survives_without_access_keys() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let mut item = track(8);
        item.artists = vec![
            ArtistRef {
                id: "opaque-a".into(),
                name: "Earth, Wind & Fire".into(),
            },
            ArtistRef {
                id: "opaque-b".into(),
                name: "Guest".into(),
            },
        ];
        item.album = Some(AlbumRef {
            id: 19,
            owner_id: -42,
            title: "Album".into(),
            access_key: Some("synthetic-album-secret".into()),
            cover_url: Some("https://images.example.test/album.jpg".into()),
        });
        save_at(&path, 42, std::slice::from_ref(&item)).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-album-secret"));
        let restored = load_at(&path, 42).unwrap().unwrap().remove(0);
        assert_eq!(restored.artists, item.artists);
        let restored_album = restored.album.unwrap();
        assert_eq!(restored_album.id, 19);
        assert_eq!(restored_album.owner_id, -42);
        assert_eq!(restored_album.title, "Album");
        assert_eq!(restored_album.cover_url, item.album.unwrap().cover_url);
        assert!(restored_album.access_key.is_none());

        let mut injected: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        injected["tracks"][0]["album"]["access_key"] = "forbidden".into();
        fs::write(&path, serde_json::to_vec(&injected).unwrap()).unwrap();
        assert!(load_at(&path, 42).is_err());
    }

    #[test]
    fn old_metadata_cache_loads_without_canonical_identities() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(8)]).unwrap();
        let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        old["tracks"][0].as_object_mut().unwrap().remove("artists");
        old["tracks"][0].as_object_mut().unwrap().remove("album");
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let restored = load_at(&path, 42).unwrap().unwrap().remove(0);
        assert_eq!(restored.artist, "Исполнитель");
        assert!(restored.artists.is_empty());
        assert!(restored.album.is_none());
    }

    #[test]
    fn atomic_updates_deduplicate_keys_and_invalid_updates_preserve_old_metadata() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let mut duplicate = track(1);
        duplicate.title = "Duplicate is not a second row".into();
        let mut other_owner = track(1);
        other_owner.owner_id = 9;
        save_at(&path, 42, &[track(1), duplicate, other_owner]).unwrap();
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].title, "Track 1");
        assert_eq!(restored[1].owner_id, 9);
        save_at(&path, 42, &[track(8)]).unwrap();
        let original = fs::read(&path).unwrap();
        for invalid_track in [
            Track { id: 0, ..track(9) },
            Track {
                owner_id: 0,
                ..track(9)
            },
            Track {
                artist: "bad\nartist".into(),
                ..track(9)
            },
            Track {
                title: "a".repeat(MAX_TEXT + 1),
                ..track(9)
            },
            Track {
                cover_url: Some("http://images.example.test/cover.jpg".into()),
                ..track(9)
            },
        ] {
            assert!(save_at(&path, 42, &[invalid_track]).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
        }
        assert!(save_at(&path, -42, &[track(9)]).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        save_at(&path, 42, &[]).unwrap();
        assert!(load_at(&path, 42).unwrap().unwrap().is_empty());
    }

    #[test]
    fn malformed_wrong_version_oversized_and_credential_fields_are_not_loaded() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut bad_version = original.clone();
        bad_version["format_version"] = 2.into();
        let mut bad_identity = original.clone();
        bad_identity["tracks"][0]["owner_id"] = 0.into();
        let mut credential = original.clone();
        credential["tracks"][0]["url"] = "https://audio.example.test/secret".into();
        let mut bad_cover = original;
        bad_cover["tracks"][0]["cover_url"] = "file:///tmp/private".into();
        for bytes in [
            b"not JSON".to_vec(),
            serde_json::to_vec(&bad_version).unwrap(),
            serde_json::to_vec(&bad_identity).unwrap(),
            serde_json::to_vec(&credential).unwrap(),
            serde_json::to_vec(&bad_cover).unwrap(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(load_at(&path, 42).is_err());
        }
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(load_at(&path, 42).is_err());
    }

    #[test]
    fn serialization_budget_and_row_limit_leave_existing_cache_untouched() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
        let original = fs::read(&path).unwrap();
        let oversized: Vec<_> = (1..=4200)
            .map(|id| Track {
                artist: "a".repeat(MAX_TEXT),
                ..track(id)
            })
            .collect();
        assert!(save_at(&path, 42, &oversized).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let many = vec![track(1); MAX_TRACKS + 1];
        assert!(save_at(&path, 42, &many).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_non_private_cache_files_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_at(&path, 42).is_err());
        save_at(&path, 42, &[track(1)]).unwrap();
        fs::rename(&path, path.with_extension("saved")).unwrap();
        symlink(path.with_extension("saved"), &path).unwrap();
        assert!(load_at(&path, 42).is_err());
        assert!(save_at(&path, 42, &[track(2)]).is_err());
        fs::remove_file(&path).unwrap();
        fs::rename(path.with_extension("saved"), &path).unwrap();
        let directory = path.parent().unwrap();
        let moved = directory.with_extension("saved");
        fs::rename(directory, &moved).unwrap();
        symlink(&moved, directory).unwrap();
        assert!(load_at(&path, 42).is_err());
        assert!(save_at(&path, 42, &[track(2)]).is_err());
    }
}
