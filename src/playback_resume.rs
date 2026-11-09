//! Last selected track, committed position and the queue it played from,
//! separate from the audio budget.
//! Metadata uses the same account-bound projection as the library and history;
//! streams, cookies and release access keys never enter the saved snapshot.
use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{library_cache, model::Track};

const FORMAT_VERSION: u8 = 2;
/// Snapshots written before the queue was saved.
const TRACK_ONLY_VERSION: u8 = 1;
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const TRACK_BYTES: u64 = 256 * 1024;
/// The saved source keeps this many tracks around the current one.
pub const MAX_QUEUE: usize = 5_000;
pub const MAX_ADDED: usize = 1_000;
const MAX_ORIGIN: usize = 1024;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub track: Track,
    pub position: f64,
    pub queue: Option<Queue>,
}

/// Where playback came from: the source's tracks (none for My music, which
/// is cached on its own) and the tracks added to the queue by hand.
#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub origin: String,
    pub tracks: Vec<Track>,
    pub added: Vec<Track>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredResume {
    format_version: u8,
    position: f64,
    metadata: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue: Option<StoredQueue>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredQueue {
    origin: String,
    tracks: serde_json::Value,
    added: serde_json::Value,
}

fn valid_origin(origin: &str) -> bool {
    !origin.is_empty() && origin.len() <= MAX_ORIGIN && !origin.chars().any(char::is_control)
}

fn encode_tracks(account_id: i64, tracks: &[Track], limit: usize) -> io::Result<serde_json::Value> {
    let bytes = library_cache::encode_bounded(account_id, tracks, MAX_BYTES, limit)?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

fn decode_tracks(
    account_id: i64,
    value: &serde_json::Value,
    limit: usize,
) -> io::Result<Vec<Track>> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    library_cache::decode_bounded(&bytes, account_id, MAX_BYTES, limit)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid VK Fast playback state")
}

fn state_path(account_id: i64) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        if account_id <= 0 {
            return Err(invalid());
        }
        Ok(crate::app_paths::state_dir()?
            .join("resume")
            .join(format!("{account_id}.json")))
    }
    #[cfg(not(windows))]
    {
        if account_id <= 0 {
            return Err(invalid());
        }
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".local/state"))
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "User state directory unavailable")
            })?;
        Ok(base
            .join("vk-fast/resume")
            .join(format!("{account_id}.json")))
    }
}

fn position_for(track: &Track, position: f64) -> io::Result<f64> {
    if !position.is_finite() || !(0.0..=f64::from(u32::MAX)).contains(&position) {
        return Err(invalid());
    }
    Ok(if track.duration == 0 {
        position
    } else {
        position.min(f64::from(track.duration))
    })
}

fn encode(account_id: i64, snapshot: &Snapshot) -> io::Result<Vec<u8>> {
    let position = position_for(&snapshot.track, snapshot.position)?;
    let metadata = library_cache::encode_bounded(
        account_id,
        std::slice::from_ref(&snapshot.track),
        TRACK_BYTES,
        1,
    )?;
    let metadata = serde_json::from_slice(&metadata).map_err(|_| invalid())?;
    // A queue that cannot be stored never costs the track and position.
    let queue = snapshot.queue.as_ref().and_then(|queue| {
        if !valid_origin(&queue.origin)
            || queue.tracks.len() > MAX_QUEUE
            || queue.added.len() > MAX_ADDED
        {
            return None;
        }
        Some(StoredQueue {
            origin: queue.origin.clone(),
            tracks: encode_tracks(account_id, &queue.tracks, MAX_QUEUE).ok()?,
            added: encode_tracks(account_id, &queue.added, MAX_ADDED).ok()?,
        })
    });
    let bytes = serde_json::to_vec(&StoredResume {
        format_version: FORMAT_VERSION,
        position,
        metadata,
        queue,
    })
    .map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}

fn decode(account_id: i64, bytes: &[u8]) -> io::Result<Snapshot> {
    if account_id <= 0 || bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let stored: StoredResume = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if stored.format_version != FORMAT_VERSION
        && !(stored.format_version == TRACK_ONLY_VERSION && stored.queue.is_none())
    {
        return Err(invalid());
    }
    let metadata = serde_json::to_vec(&stored.metadata).map_err(|_| invalid())?;
    let mut tracks = library_cache::decode_bounded(&metadata, account_id, TRACK_BYTES, 1)?;
    if tracks.len() != 1 {
        return Err(invalid());
    }
    let track = tracks.remove(0);
    let position = position_for(&track, stored.position)?;
    // A damaged queue only loses the queue.
    let queue = stored.queue.and_then(|queue| {
        if !valid_origin(&queue.origin) {
            return None;
        }
        Some(Queue {
            origin: queue.origin,
            tracks: decode_tracks(account_id, &queue.tracks, MAX_QUEUE).ok()?,
            added: decode_tracks(account_id, &queue.added, MAX_ADDED).ok()?,
        })
    });
    Ok(Snapshot {
        track,
        position,
        queue,
    })
}

fn load_at(path: &Path, account_id: i64) -> io::Result<Option<Snapshot>> {
    if account_id <= 0 {
        return Err(invalid());
    }
    library_cache::load_private_bytes(path, MAX_BYTES)?
        .map(|bytes| decode(account_id, &bytes))
        .transpose()
}

fn save_at(path: &Path, account_id: i64, snapshot: &Snapshot) -> io::Result<()> {
    let bytes = encode(account_id, snapshot)?;
    library_cache::save_private_bytes(path, &bytes, MAX_BYTES)
}

/// Read one account's saved track without starting playback or resolving URLs.
/// Call off the UI thread together with the account's cached library.
pub fn load(account_id: i64) -> io::Result<Option<Snapshot>> {
    load_at(&state_path(account_id)?, account_id)
}

/// Atomically save committed playback state for one account. An invalid update
/// leaves the previous snapshot; callers with no selected track should skip it.
pub fn save(account_id: i64, snapshot: &Snapshot) -> io::Result<()> {
    save_at(&state_path(account_id)?, account_id, snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AlbumRef, ArtistRef};
    use std::fs;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let root = std::env::temp_dir().join(format!("vk-fast-resume-test-{suffix}"));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn path(&self, account: i64) -> PathBuf {
            self.0
                .join("vk-fast/resume")
                .join(format!("{account}.json"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot(id: i64, position: f64) -> Snapshot {
        Snapshot {
            queue: None,
            position,
            track: Track {
                added: None,
                id,
                owner_id: -7,
                artist: "Artist".into(),
                title: format!("Track {id}"),
                subtitle: Some("Slowed".into()),
                duration: 180,
                url: Some("https://audio.example.test/play?token=synthetic-stream-secret".into()),
                cover_url: Some("https://images.example.test/cover.jpg".into()),
                artists: vec![ArtistRef {
                    id: "canonical-artist".into(),
                    name: "Artist".into(),
                }],
                album: Some(AlbumRef {
                    id: 9,
                    owner_id: -7,
                    title: "Release".into(),
                    access_key: Some("synthetic-album-secret".into()),
                    cover_url: None,
                }),
                library_liked: Some(false),
                release_key: Some((-7, 8)),
                library_source_key: None,
                unavailable: None,
            },
        }
    }

    #[test]
    fn track_and_fractional_position_survive_reopening_without_credentials() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        assert!(load_at(&path, 42).unwrap().is_none());
        assert!(!fixture.0.join("vk-fast").exists());
        save_at(&path, 42, &snapshot(8, 71.375)).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        for forbidden in [
            "synthetic-stream-secret",
            "synthetic-album-secret",
            "\"url\"",
            "access_key",
        ] {
            assert!(!raw.contains(forbidden));
        }
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(restored.track.key(), (-7, 8));
        assert_eq!(restored.position, 71.375);
        assert_eq!(restored.track.subtitle.as_deref(), Some("Slowed"));
        assert_eq!(restored.track.artists[0].id, "canonical-artist");
        assert_eq!(restored.track.library_liked, Some(false));
        assert_eq!(restored.track.release_key, Some((-7, 8)));
        assert!(restored.track.url.is_none());
        assert!(restored.track.album.unwrap().access_key.is_none());
    }

    #[test]
    fn the_queue_survives_reopening_without_credentials_and_old_files_still_load() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let mut saved = snapshot(8, 12.5);
        saved.queue = Some(Queue {
            origin: "artist:canonical-artist".into(),
            tracks: vec![
                snapshot(7, 0.0).track,
                snapshot(8, 0.0).track,
                snapshot(9, 0.0).track,
            ],
            added: vec![snapshot(30, 0.0).track],
        });
        save_at(&path, 42, &saved).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        for forbidden in [
            "synthetic-stream-secret",
            "synthetic-album-secret",
            "access_key",
        ] {
            assert!(!raw.contains(forbidden));
        }
        let restored = load_at(&path, 42).unwrap().unwrap();
        let queue = restored.queue.unwrap();
        assert_eq!(queue.origin, "artist:canonical-artist");
        let keys: Vec<_> = queue.tracks.iter().map(Track::key).collect();
        assert_eq!(keys, [(-7, 7), (-7, 8), (-7, 9)]);
        assert_eq!(queue.added[0].key(), (-7, 30));
        assert!(queue.tracks.iter().all(|track| track.url.is_none()));
        // A file from before queues were saved still restores its track.
        let mut old: serde_json::Value = serde_json::from_str(&raw).unwrap();
        old["format_version"] = 1.into();
        old.as_object_mut().unwrap().remove("queue");
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(restored.track.key(), (-7, 8));
        assert!(restored.queue.is_none());
        // A damaged queue only loses the queue.
        let mut damaged: serde_json::Value = serde_json::from_str(&raw).unwrap();
        damaged["queue"]["tracks"] = serde_json::json!({"tracks": "broken"});
        fs::write(&path, serde_json::to_vec(&damaged).unwrap()).unwrap();
        let restored = load_at(&path, 42).unwrap().unwrap();
        assert_eq!(restored.position, 12.5);
        assert!(restored.queue.is_none());
        // An origin with control characters is not stored.
        saved.queue.as_mut().unwrap().origin = "bad\norigin".into();
        save_at(&path, 42, &saved).unwrap();
        assert!(load_at(&path, 42).unwrap().unwrap().queue.is_none());
    }

    #[test]
    fn saved_state_is_account_bound_and_does_not_replace_other_accounts() {
        let fixture = Fixture::new();
        let first = fixture.path(42);
        let second = fixture.path(43);
        save_at(&first, 42, &snapshot(8, 17.0)).unwrap();
        let original = fs::read(&first).unwrap();
        assert!(load_at(&first, 43).is_err());
        assert!(load_at(&second, 43).unwrap().is_none());
        save_at(&second, 43, &snapshot(9, 93.5)).unwrap();
        assert_eq!(fs::read(&first).unwrap(), original);
        assert_eq!(load_at(&first, 42).unwrap().unwrap().position, 17.0);
        assert_eq!(load_at(&second, 43).unwrap().unwrap().track.id, 9);
        assert!(save_at(&first, 0, &snapshot(1, 2.0)).is_err());
        assert_eq!(fs::read(&first).unwrap(), original);
    }

    #[test]
    fn invalid_updates_preserve_previous_state_and_positions_use_known_duration() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &snapshot(8, 17.0)).unwrap();
        let previous = fs::read(&path).unwrap();
        for position in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, f64::MAX] {
            assert!(save_at(&path, 42, &snapshot(9, position)).is_err());
            assert_eq!(fs::read(&path).unwrap(), previous);
        }
        let mut invalid = snapshot(9, 1.0);
        invalid.track.title = "invalid\ntitle".into();
        assert!(save_at(&path, 42, &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        let mut large = snapshot(9, 1.0);
        large.track.artists = (0..64)
            .map(|index| ArtistRef {
                id: format!("artist-{index}"),
                name: "a".repeat(4096),
            })
            .collect();
        assert!(save_at(&path, 42, &large).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        save_at(&path, 42, &snapshot(9, 180.5)).unwrap();
        assert_eq!(load_at(&path, 42).unwrap().unwrap().position, 180.0);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_unknown_and_credential_metadata_are_rejected_without_deleting() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &snapshot(8, 17.0)).unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut invalid_states = Vec::new();
        let mut state = original.clone();
        state["format_version"] = 3.into();
        invalid_states.push(state);
        let mut state = original.clone();
        state["position"] = (-1).into();
        invalid_states.push(state);
        let mut state = original.clone();
        state["position"] = serde_json::Value::Null;
        invalid_states.push(state);
        let mut state = original.clone();
        state["metadata"]["account_id"] = 43.into();
        invalid_states.push(state);
        let mut state = original.clone();
        state["metadata"]["tracks"] = serde_json::json!([]);
        invalid_states.push(state);
        let mut state = original.clone();
        state["metadata"]["tracks"] =
            serde_json::json!(vec![original["metadata"]["tracks"][0].clone(); 2]);
        invalid_states.push(state);
        let mut state = original.clone();
        state["metadata"]["tracks"][0]["url"] = "https://audio.example.test/private".into();
        invalid_states.push(state);
        let mut state = original.clone();
        state["metadata"]["tracks"][0]["album"]["access_key"] = "forbidden".into();
        invalid_states.push(state);
        let mut state = original;
        state["cookies"] = "forbidden".into();
        invalid_states.push(state);
        for bytes in std::iter::once(b"not JSON".to_vec()).chain(
            invalid_states
                .iter()
                .map(|state| serde_json::to_vec(state).unwrap()),
        ) {
            fs::write(&path, &bytes).unwrap();
            assert!(load_at(&path, 42).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(load_at(&path, 42).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_files_are_private_and_symlink_targets_are_not_read_or_replaced() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &snapshot(8, 17.0)).unwrap();
        let previous = fs::read(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for directory in [
            path.parent().unwrap(),
            path.parent().unwrap().parent().unwrap(),
        ] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_at(&path, 42).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&path, path.with_extension("saved")).unwrap();
        symlink(path.with_extension("saved"), &path).unwrap();
        assert!(load_at(&path, 42).is_err());
        assert!(save_at(&path, 42, &snapshot(9, 1.0)).is_err());
        assert_eq!(fs::read(path.with_extension("saved")).unwrap(), previous);
    }
}
