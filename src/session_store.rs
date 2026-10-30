//! Local saved VK ID session. Credentials are never logged and are stored only
//! in the current user's private state directory (0700 directory, 0600 file).
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::model::{Profile, valid_cover_url};

const MAX_BYTES: u64 = 65_536;

// Do not derive Debug: this value contains long-lived credentials.
#[derive(Serialize, Deserialize)]
pub struct SavedSession {
    pub format_version: u8,
    pub client_id: String,
    pub redirect_uri: String,
    pub access_token: String,
    pub refresh_token: String,
    pub device_id: String,
    pub expires_at: u64,
    pub profile: Profile,
}

impl Drop for SavedSession {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.device_id.zeroize();
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid saved VK Fast session")
}

fn validate(session: &SavedSession) -> io::Result<()> {
    let secret = |value: &str, limit: usize| {
        value.len() <= limit && value.is_ascii() && !value.chars().any(char::is_whitespace)
    };
    if session.format_version != 1
        || session.client_id.is_empty()
        || session.client_id.len() > 32
        || !session.client_id.bytes().all(|byte| byte.is_ascii_digit())
        || session
            .client_id
            .parse::<u64>()
            .ok()
            .is_none_or(|id| id == 0)
        || session.redirect_uri.is_empty()
        || session.redirect_uri.len() > 2048
        || session.access_token.is_empty()
        || !secret(&session.access_token, 8192)
        || !secret(&session.refresh_token, 16384)
        || !secret(&session.device_id, 8192)
        || (!session.refresh_token.is_empty() && session.device_id.is_empty())
        || session.profile.id <= 0
        || session.profile.first_name.len() > 1024
        || session.profile.last_name.len() > 1024
        || session
            .profile
            .avatar_url
            .as_ref()
            .is_some_and(|url| valid_cover_url(url).is_none())
    {
        return Err(invalid());
    }
    Ok(())
}

fn session_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(crate::app_paths::state_dir()?.join("session.json"))
    }
    #[cfg(not(windows))]
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/state"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "User state directory is unavailable",
                )
            })?;
        Ok(state.join("vk-fast/session.json"))
    }
}

fn checked_parent(path: &Path, create: bool) -> io::Result<&Path> {
    let parent = path.parent().ok_or_else(invalid)?;
    if create {
        fs::create_dir_all(parent)?;
    }
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if create {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        } else if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Session directory is not private",
            ));
        }
    }
    Ok(parent)
}

fn check_target(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(metadata))
        }
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn random_suffix() -> io::Result<String> {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes)
        .map_err(|_| io::Error::other("Session temporary file name unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

struct TemporaryFile(Option<PathBuf>);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn sync_parent(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

fn save_at(path: &Path, session: &SavedSession) -> io::Result<()> {
    validate(session)?;
    let parent = checked_parent(path, true)?;
    check_target(path)?;
    let bytes = Zeroizing::new(serde_json::to_vec(session).map_err(|_| invalid())?);
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let (mut file, mut temporary) = loop {
        let temporary_path = parent.join(format!(".session-{}.tmp", random_suffix()?));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary_path) {
            Ok(file) => break (file, TemporaryFile(Some(temporary_path))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary.0.as_ref().ok_or_else(invalid)?, path)?;
    temporary.0 = None;
    sync_parent(parent)
}

fn load_at(path: &Path) -> io::Result<Option<SavedSession>> {
    let Some(metadata) = check_target(path)? else {
        return Ok(None);
    };
    checked_parent(path, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Session file is not private",
            ));
        }
    }
    if metadata.len() > MAX_BYTES {
        return Err(invalid());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let session: SavedSession = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    validate(&session)?;
    Ok(Some(session))
}

fn clear_at(path: &Path) -> io::Result<()> {
    if check_target(path)?.is_none() {
        return Ok(());
    }
    let parent = checked_parent(path, false)?;
    fs::remove_file(path)?;
    sync_parent(parent)
}

pub fn save(session: &SavedSession) -> io::Result<()> {
    save_at(&session_path()?, session)
}

pub fn load() -> io::Result<Option<SavedSession>> {
    load_at(&session_path()?)
}

pub fn clear() -> io::Result<()> {
    clear_at(&session_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("vk-fast-session-test-{}", random_suffix().unwrap()));
            fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> SavedSession {
        SavedSession {
            format_version: 1,
            client_id: "54863150".into(),
            redirect_uri: "http://localhost/callback".into(),
            access_token: "synthetic-access-token".into(),
            refresh_token: "synthetic-refresh-token".into(),
            device_id: "synthetic-device".into(),
            expires_at: 2_000_000_000,
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "User".into(),
                avatar_url: None,
            },
        }
    }

    #[test]
    fn saved_session_survives_atomic_update_and_clears_privately() {
        let directory = TestDirectory::new();
        let path = directory.0.join("state/session.json");
        assert!(load_at(&path).unwrap().is_none());
        let mut session = fixture();
        save_at(&path, &session).unwrap();
        assert_eq!(load_at(&path).unwrap().unwrap().profile.id, 42);
        session.access_token = "synthetic-updated-access".into();
        save_at(&path, &session).unwrap();
        assert_eq!(
            load_at(&path).unwrap().unwrap().access_token,
            "synthetic-updated-access"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        clear_at(&path).unwrap();
        assert!(load_at(&path).unwrap().is_none());
        clear_at(&path).unwrap();
    }

    #[test]
    fn invalid_or_oversized_cache_is_not_accepted() {
        let directory = TestDirectory::new();
        let path = directory.0.join("session.json");
        let mut session = fixture();
        save_at(&path, &session).unwrap();
        session.format_version = 2;
        fs::write(&path, serde_json::to_vec(&session).unwrap()).unwrap();
        assert!(matches!(load_at(&path), Err(error) if error.kind() == io::ErrorKind::InvalidData));
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(matches!(load_at(&path), Err(error) if error.kind() == io::ErrorKind::InvalidData));
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_permissions_and_symlink_cache_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = TestDirectory::new();
        let target = directory.0.join("real.json");
        let alias = directory.0.join("session.json");
        save_at(&target, &fixture()).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            matches!(load_at(&target), Err(error) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &alias).unwrap();
        assert!(load_at(&alias).is_err());
        assert!(save_at(&alias, &fixture()).is_err());
        assert!(clear_at(&alias).is_err());
        assert!(target.exists());
    }
}
