//! Last selected track and committed position, separate from the audio budget.
//! Metadata uses the same account-bound projection as the library and history;
//! streams, cookies and release access keys never enter the saved snapshot.
use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{library_cache, model::Track};

const FORMAT_VERSION: u8 = 1;
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub track: Track,
    pub position: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredResume {
    format_version: u8,
    position: f64,
    metadata: serde_json::Value,
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
        MAX_BYTES,
        1,
    )?;
    let metadata = serde_json::from_slice(&metadata).map_err(|_| invalid())?;
    let bytes = serde_json::to_vec(&StoredResume {
        format_version: FORMAT_VERSION,
        position,
        metadata,
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
    if stored.format_version != FORMAT_VERSION {
        return Err(invalid());
    }
    let metadata = serde_json::to_vec(&stored.metadata).map_err(|_| invalid())?;
    let mut tracks = library_cache::decode_bounded(&metadata, account_id, MAX_BYTES, 1)?;
    if tracks.len() != 1 {
        return Err(invalid());
    }
    let track = tracks.remove(0);
    let position = position_for(&track, stored.position)?;
    Ok(Snapshot { track, position })
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
            position,
            track: Track {
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
        state["format_version"] = 2.into();
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
