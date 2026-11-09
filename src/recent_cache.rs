//! Small, account-bound Recently Played metadata. Kept outside the audio budget;
//! the library codec strips stream URLs and album access keys before writing.
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use crate::{library_cache, model::Track};

pub const MAX_RECENT: usize = 32;
const MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn state_path(account_id: i64) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        if account_id <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid recent account",
            ));
        }
        Ok(crate::app_paths::state_dir()?
            .join("recent")
            .join(format!("{account_id}.json")))
    }
    #[cfg(not(windows))]
    {
        if account_id <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid recent account",
            ));
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
            .join("vk-fast/recent")
            .join(format!("{account_id}.json")))
    }
}

fn load_at(path: &Path, account_id: i64) -> io::Result<Vec<Track>> {
    library_cache::load_bounded_at(path, account_id, MAX_BYTES, MAX_RECENT)
        .map(Option::unwrap_or_default)
}

fn save_at(path: &Path, account_id: i64, tracks: &[Track]) -> io::Result<()> {
    let mut seen = HashSet::new();
    // Input order is MRU-first; duplicate or older entries must not displace a
    // newer identity when a caller restores and updates the list concurrently.
    let recent: Vec<_> = tracks
        .iter()
        .filter(|track| seen.insert(track.key()))
        .take(MAX_RECENT)
        .cloned()
        .collect();
    library_cache::save_bounded_at(path, account_id, &recent, MAX_BYTES, MAX_RECENT)
}

/// Missing history is an empty list. Call off the UI thread when restoring an account.
pub fn load(account_id: i64) -> io::Result<Vec<Track>> {
    load_at(&state_path(account_id)?, account_id)
}

/// Atomically replace one account's MRU-first history. No media bytes or
/// credentials are persisted; a failed write leaves the previous snapshot.
pub fn save(account_id: i64, tracks: &[Track]) -> io::Result<()> {
    save_at(&state_path(account_id)?, account_id, tracks)
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
            let root = std::env::temp_dir().join(format!("vk-fast-recent-test-{suffix}"));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self, account: i64) -> PathBuf {
            self.0
                .join("vk-fast/recent")
                .join(format!("{account}.json"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn track(id: i64) -> Track {
        Track {
            explicit: false,
            added: None,
            id,
            owner_id: -7,
            artist: "Artist".into(),
            title: format!("Track {id}"),
            subtitle: Some("Slowed".into()),
            duration: 180,
            url: Some("https://audio.example.test/private?token=synthetic-stream-secret".into()),
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
            library_liked: None,
            release_key: None,
            library_source_key: None,
            unavailable: None,
        }
    }

    #[test]
    fn history_survives_reopening_in_mru_order_without_credentials() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        assert!(load_at(&path, 42).unwrap().is_empty());
        assert!(!fixture.0.join("vk-fast").exists());
        save_at(&path, 42, &[track(3), track(1), track(2)]).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("synthetic-stream-secret") && !raw.contains("synthetic-album-secret")
        );
        let rows = load_at(&path, 42).unwrap();
        assert_eq!(
            rows.iter().map(|track| track.id).collect::<Vec<_>>(),
            [3, 1, 2]
        );
        assert!(
            rows.iter()
                .all(|track| track.url.is_none()
                    && track.album.as_ref().unwrap().access_key.is_none())
        );
        assert_eq!(rows[0].subtitle.as_deref(), Some("Slowed"));
        assert_eq!(rows[0].artists[0].id, "canonical-artist");
        assert_eq!(rows[0].album.as_ref().unwrap().title, "Release");
        assert!(load_at(&path, 43).is_err());
        assert!(load_at(&fixture.path(43), 43).unwrap().is_empty());
        save_at(&fixture.path(43), 43, &[track(8)]).unwrap();
        assert_eq!(load_at(&path, 42).unwrap()[0].id, 3);
        assert_eq!(load_at(&fixture.path(43), 43).unwrap()[0].id, 8);
    }

    #[test]
    fn recent_is_bounded_deduplicated_and_does_not_touch_library_or_audio() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        let application = path.parent().unwrap().parent().unwrap();
        fs::create_dir_all(application.join("audio")).unwrap();
        fs::create_dir_all(application.join("library")).unwrap();
        fs::write(application.join("audio/unchanged"), b"audio").unwrap();
        fs::write(application.join("library/unchanged"), b"library").unwrap();
        let input: Vec<_> = std::iter::once(track(40))
            .chain((1..=40).map(track))
            .collect();
        save_at(&path, 42, &input).unwrap();
        let rows = load_at(&path, 42).unwrap();
        assert_eq!(rows.len(), MAX_RECENT);
        assert_eq!(rows[0].id, 40);
        assert_eq!(rows[31].id, 31);
        assert_eq!(
            fs::read(application.join("audio/unchanged")).unwrap(),
            b"audio"
        );
        assert_eq!(
            fs::read(application.join("library/unchanged")).unwrap(),
            b"library"
        );
        save_at(&path, 42, &[]).unwrap();
        assert!(load_at(&path, 42).unwrap().is_empty());
    }

    #[test]
    fn invalid_or_oversized_snapshots_never_replace_good_history() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
        let previous = fs::read(&path).unwrap();
        let mut invalid = track(2);
        invalid.title = "invalid\ntitle".into();
        assert!(save_at(&path, 42, &[invalid]).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        let mut large = track(2);
        large.artists = (0..64)
            .map(|index| ArtistRef {
                id: format!("artist-{index}"),
                name: "a".repeat(4096),
            })
            .collect();
        assert!(save_at(&path, 42, &[large]).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert!(save_at(&path, -42, &[track(1)]).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_or_foreign_metadata_is_rejected_without_deleting_the_file() {
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut foreign = original.clone();
        foreign["account_id"] = 43.into();
        let mut too_many = original.clone();
        too_many["tracks"] = serde_json::json!(vec![original["tracks"][0].clone(); MAX_RECENT + 1]);
        let mut secret = original;
        secret["tracks"][0]["url"] = "https://audio.example.test/secret".into();
        for bytes in [
            b"malformed".to_vec(),
            serde_json::to_vec(&foreign).unwrap(),
            serde_json::to_vec(&too_many).unwrap(),
            serde_json::to_vec(&secret).unwrap(),
        ] {
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
    fn recent_files_are_private_and_symlinks_are_not_followed() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let path = fixture.path(42);
        save_at(&path, 42, &[track(1)]).unwrap();
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
        fs::rename(&path, path.with_extension("saved")).unwrap();
        symlink(path.with_extension("saved"), &path).unwrap();
        assert!(load_at(&path, 42).is_err());
        assert!(save_at(&path, 42, &[track(2)]).is_err());
        assert_eq!(fs::read(path.with_extension("saved")).unwrap(), previous);
    }
}
