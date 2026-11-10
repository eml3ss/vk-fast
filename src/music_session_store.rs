//! Private, account-bound profile and cookies of the application's VK session.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    model::{Profile, valid_cover_url},
    web_session::SessionCookie,
};

const MAX_BYTES: u64 = 128 * 1024;
const MAX_COOKIES: usize = 256;
// Serialize metadata updates with login/logout so a delayed refresh cannot
// recreate a deleted session or write over a different account's credentials.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

// No Debug implementation: cookie values grant access to the website session.
#[derive(Deserialize, Serialize)]
struct StoredSession {
    format_version: u8,
    owner_id: i64,
    cookies: Vec<SessionCookie>,
    #[serde(default)]
    profile: Option<Profile>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid saved VK music session")
}

fn allowed_domain(domain: &str) -> bool {
    matches!(
        domain,
        "vk.ru"
            | ".vk.ru"
            | "www.vk.ru"
            | ".www.vk.ru"
            | "vk.com"
            | ".vk.com"
            | "www.vk.com"
            | ".www.vk.com"
            | "login.vk.ru"
            | "login.vk.com"
    )
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_cookie(cookie: &SessionCookie) -> bool {
    allowed_domain(&cookie.domain)
        && valid_name(&cookie.name)
        && cookie.value.len() <= 16 * 1024
        // RFC 6265 cookie-octet: exclude separators and header injection.
        && cookie.value.bytes().all(|byte| {
            matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
        })
        && cookie.path.starts_with('/')
        && cookie.path.len() <= 2048
        && !cookie.path.bytes().any(|byte| byte.is_ascii_control())
        && cookie.expires_unix.is_none_or(|expiry| expiry >= 0)
}

fn validate(owner_id: i64, cookies: &[SessionCookie]) -> io::Result<()> {
    if owner_id <= 0
        || cookies.is_empty()
        || cookies.len() > MAX_COOKIES
        || !cookies.iter().all(valid_cookie)
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_profile(owner_id: i64, profile: &Profile) -> io::Result<()> {
    if profile.id <= 0
        || profile.id != owner_id
        || profile.first_name.len() > 1024
        || profile.last_name.len() > 1024
        || profile
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
        Ok(crate::app_paths::state_dir()?.join("music-session.json"))
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
        Ok(state.join("vk-fast/music-session.json"))
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
                "Music session directory is not private",
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
        .map_err(|_| io::Error::other("Music session temporary file name unavailable"))?;
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

#[cfg(test)]
fn save_at(path: &Path, owner_id: i64, cookies: &[SessionCookie]) -> io::Result<()> {
    save_record_at(path, owner_id, cookies, None)
}

fn save_profile_at(path: &Path, profile: &Profile, cookies: &[SessionCookie]) -> io::Result<()> {
    save_record_at(path, profile.id, cookies, Some(profile))
}

fn save_record_at(
    path: &Path,
    owner_id: i64,
    cookies: &[SessionCookie],
    profile: Option<&Profile>,
) -> io::Result<()> {
    validate(owner_id, cookies)?;
    if let Some(profile) = profile {
        validate_profile(owner_id, profile)?;
    }
    #[derive(Serialize)]
    struct BorrowedSession<'a> {
        format_version: u8,
        owner_id: i64,
        cookies: &'a [SessionCookie],
        #[serde(skip_serializing_if = "Option::is_none")]
        profile: Option<&'a Profile>,
    }
    let bytes = Zeroizing::new(
        serde_json::to_vec(&BorrowedSession {
            format_version: 1,
            owner_id,
            cookies,
            profile,
        })
        .map_err(|_| invalid())?,
    );
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let parent = checked_parent(path, true)?;
    check_target(path)?;
    let (mut file, mut temporary) = loop {
        let temporary_path = parent.join(format!(".music-session-{}.tmp", random_suffix()?));
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

fn load_at(path: &Path, owner_id: i64) -> io::Result<Option<Vec<SessionCookie>>> {
    if owner_id <= 0 {
        return Err(invalid());
    }
    let Some(session) = load_record_at(path)? else {
        return Ok(None);
    };
    if session.owner_id != owner_id {
        return Err(invalid());
    }
    Ok(Some(session.cookies))
}

fn restore_profile_at(path: &Path) -> io::Result<Option<(Profile, Vec<SessionCookie>)>> {
    let Some(session) = load_record_at(path)? else {
        return Ok(None);
    };
    Ok(session.profile.map(|profile| (profile, session.cookies)))
}

fn load_record_at(path: &Path) -> io::Result<Option<StoredSession>> {
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
                "Music session file is not private",
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
    let session: StoredSession = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if session.format_version != 1 {
        return Err(invalid());
    }
    validate(session.owner_id, &session.cookies)?;
    if let Some(profile) = &session.profile {
        validate_profile(session.owner_id, profile)?;
    }
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

pub fn load(owner_id: i64) -> io::Result<Option<Vec<SessionCookie>>> {
    load_at(&session_path()?, owner_id)
}

/// Save a verified website profile alongside its cookies, without an OAuth token.
pub fn save_profile(profile: &Profile, cookies: &[SessionCookie]) -> io::Result<()> {
    let _write = WRITE_LOCK.lock().map_err(|_| invalid())?;
    save_profile_at(&session_path()?, profile, cookies)
}

fn update_profile_at(path: &Path, fresh: &Profile) -> io::Result<()> {
    validate_profile(fresh.id, fresh)?;
    let Some(session) = load_record_at(path)? else {
        return Ok(());
    };
    if session.owner_id != fresh.id {
        return Ok(());
    }
    let missing_profile = session.profile.is_none();
    let mut profile = session.profile.unwrap_or_else(|| fresh.clone());
    if profile.merge_metadata(fresh) || missing_profile {
        save_profile_at(path, &profile, &session.cookies)?;
    }
    Ok(())
}

/// VK renewed the session's cookies: keep them for the same, still signed-in
/// account. A deleted session is not recreated, another account's is not
/// touched, and a set without its sign-in cookie is never written.
fn update_cookies_at(path: &Path, owner_id: i64, cookies: &[SessionCookie]) -> io::Result<()> {
    validate(owner_id, cookies)?;
    if !cookies.iter().any(|cookie| {
        matches!(cookie.name.as_str(), "remixsid" | "remixsid6") && !cookie.value.is_empty()
    }) {
        return Err(invalid());
    }
    let Some(session) = load_record_at(path)? else {
        return Ok(());
    };
    if session.owner_id != owner_id {
        return Ok(());
    }
    save_record_at(path, owner_id, cookies, session.profile.as_ref())
}

/// Called off the UI thread.
pub fn update_cookies(owner_id: i64, cookies: &[SessionCookie]) -> io::Result<()> {
    let _write = WRITE_LOCK.lock().map_err(|_| invalid())?;
    update_cookies_at(&session_path()?, owner_id, cookies)
}

/// Update only a still-existing account. Called off the UI thread.
pub fn update_profile(fresh: &Profile) -> io::Result<()> {
    let _write = WRITE_LOCK.lock().map_err(|_| invalid())?;
    update_profile_at(&session_path()?, fresh)
}

/// Legacy cookie-only records still load through `load(owner_id)`, but cannot
/// restore a standalone website login until a verified profile has been saved.
pub fn restore_profile() -> io::Result<Option<(Profile, Vec<SessionCookie>)>> {
    restore_profile_at(&session_path()?)
}

/// Delete data left by the retired website and VK ID sign-in methods: the
/// private webview profile (it held that browser's VK cookies) and the old
/// OAuth session file. Runs off the UI thread; failures are not fatal.
pub fn remove_retired_logins() {
    let Some(directory) = session_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
    else {
        return;
    };
    remove_retired_at(&directory);
}

fn remove_retired_at(directory: &Path) {
    let webview = directory.join("webview");
    // Never follow a link out of the application's own state directory.
    if fs::symlink_metadata(&webview).is_ok_and(|meta| meta.file_type().is_dir()) {
        let _ = fs::remove_dir_all(&webview);
    }
    let oauth = directory.join("session.json");
    if fs::symlink_metadata(&oauth).is_ok_and(|meta| meta.file_type().is_file()) {
        let _ = fs::remove_file(&oauth);
    }
}

pub fn clear() -> io::Result<()> {
    let _write = WRITE_LOCK.lock().map_err(|_| invalid())?;
    clear_at(&session_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "vk-fast-music-session-test-{}",
                random_suffix().unwrap()
            ));
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

    fn fixture() -> Vec<SessionCookie> {
        vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-cookie-value".into(),
            secure: true,
            expires_unix: Some(2_000_000_000),
        }]
    }

    #[test]
    fn renewed_cookies_update_only_the_same_still_saved_account() {
        let directory = TestDirectory::new();
        let path = directory.0.join("state/music-session.json");
        let profile = Profile {
            id: 42,
            first_name: "Synthetic".into(),
            last_name: "User".into(),
            avatar_url: None,
        };
        let mut renewed = fixture();
        renewed[0].value = "renewed-cookie-value".into();
        renewed.push(SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixnsid".into(),
            value: "synthetic-new".into(),
            secure: true,
            expires_unix: None,
        });
        // Signed out meanwhile: the session is not recreated.
        update_cookies_at(&path, 42, &renewed).unwrap();
        assert!(load_record_at(&path).unwrap().is_none());

        save_profile_at(&path, &profile, &fixture()).unwrap();
        // Another account signed in meanwhile: its session is untouched.
        update_cookies_at(&path, 7, &renewed).unwrap();
        assert_eq!(
            load_at(&path, 42).unwrap().unwrap()[0].value,
            "synthetic-cookie-value"
        );

        update_cookies_at(&path, 42, &renewed).unwrap();
        let (restored, cookies) = restore_profile_at(&path).unwrap().unwrap();
        assert_eq!(restored.first_name, "Synthetic", "the profile is kept");
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].value, "renewed-cookie-value");

        // A set without the sign-in cookie is never written.
        assert!(update_cookies_at(&path, 42, &renewed[1..]).is_err());
        assert_eq!(load_at(&path, 42).unwrap().unwrap().len(), 2);
    }

    #[test]
    fn music_cache_is_private_account_bound_atomic_and_clearable() {
        let directory = TestDirectory::new();
        let path = directory.0.join("state/music-session.json");
        assert!(load_at(&path, 42).unwrap().is_none());
        let mut cookies = fixture();
        save_at(&path, 42, &cookies).unwrap();
        assert!(restore_profile_at(&path).unwrap().is_none());
        assert_eq!(
            load_at(&path, 42).unwrap().unwrap()[0].value,
            cookies[0].value
        );
        assert!(load_at(&path, 43).is_err());
        cookies[0].value = "synthetic-updated-cookie".into();
        save_at(&path, 42, &cookies).unwrap();
        assert_eq!(
            load_at(&path, 42).unwrap().unwrap()[0].value,
            cookies[0].value
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
        clear_at(&path).unwrap();
        assert!(load_at(&path, 42).unwrap().is_none());
    }

    #[test]
    fn verified_website_profile_restores_without_an_oauth_record() {
        let directory = TestDirectory::new();
        let path = directory.0.join("state/music-session.json");
        assert!(restore_profile_at(&path).unwrap().is_none());
        let profile = Profile {
            id: 42,
            first_name: "Test".into(),
            last_name: "Пользователь".into(),
            avatar_url: Some("https://images.example.test/self.jpg".into()),
        };
        save_profile_at(&path, &profile, &fixture()).unwrap();
        let (restored, cookies) = restore_profile_at(&path).unwrap().unwrap();
        assert_eq!(restored.id, profile.id);
        assert_eq!(restored.first_name, profile.first_name);
        assert_eq!(restored.last_name, profile.last_name);
        assert_eq!(restored.avatar_url, profile.avatar_url);
        assert_eq!(cookies[0].value, "synthetic-cookie-value");
        assert_eq!(load_at(&path, profile.id).unwrap().unwrap().len(), 1);
        assert!(load_at(&path, profile.id + 1).is_err());
    }

    #[test]
    fn retired_sign_in_data_is_removed_without_touching_the_music_session() {
        let directory = TestDirectory::new();
        let state = directory.0.join("state");
        fs::create_dir_all(state.join("webview/profile")).unwrap();
        fs::write(state.join("webview/profile/Cookies"), "synthetic").unwrap();
        fs::write(state.join("session.json"), "{}").unwrap();
        let music = state.join("music-session.json");
        save_at(&music, 42, &fixture()).unwrap();
        remove_retired_at(&state);
        assert!(!state.join("webview").exists());
        assert!(!state.join("session.json").exists());
        assert!(load_at(&music, 42).unwrap().is_some());
        remove_retired_at(&state);
    }

    #[test]
    fn metadata_refresh_repairs_legacy_names_without_replacing_cookies_or_reviving_logout() {
        let directory = TestDirectory::new();
        let path = directory.0.join("music-session.json");
        let old = Profile {
            id: 42,
            first_name: "VK".into(),
            last_name: "42".into(),
            avatar_url: Some("https://images.example.test/cached.jpg".into()),
        };
        let fresh = Profile {
            id: 42,
            first_name: "Анна".into(),
            last_name: "Test".into(),
            avatar_url: None,
        };
        save_profile_at(&path, &old, &fixture()).unwrap();
        update_profile_at(&path, &fresh).unwrap();
        let (saved, cookies) = restore_profile_at(&path).unwrap().unwrap();
        assert_eq!(saved.display_name(), "Анна Test");
        assert_eq!(saved.avatar_url, old.avatar_url);
        assert_eq!(cookies[0].value, "synthetic-cookie-value");
        let before = fs::read(&path).unwrap();
        for incoming in [
            old.clone(),
            Profile {
                first_name: String::new(),
                last_name: String::new(),
                ..old.clone()
            },
            Profile {
                id: 43,
                ..fresh.clone()
            },
        ] {
            update_profile_at(&path, &incoming).unwrap();
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        clear_at(&path).unwrap();
        update_profile_at(&path, &fresh).unwrap();
        assert!(
            !path.exists(),
            "A late metadata reply must not recreate a signed-out session"
        );
        let other = Profile {
            id: 43,
            ..fresh.clone()
        };
        save_profile_at(&path, &other, &fixture()).unwrap();
        update_profile_at(&path, &fresh).unwrap();
        assert_eq!(restore_profile_at(&path).unwrap().unwrap().0.id, 43);
    }

    #[test]
    fn foreign_or_oversized_website_profile_cannot_restore() {
        let directory = TestDirectory::new();
        let path = directory.0.join("music-session.json");
        let mut profile = Profile {
            id: 42,
            first_name: "Test".into(),
            last_name: String::new(),
            avatar_url: None,
        };
        save_profile_at(&path, &profile, &fixture()).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["profile"]["id"] = serde_json::json!(43);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(restore_profile_at(&path).is_err());
        assert!(load_at(&path, 42).is_err());
        value["profile"]["id"] = serde_json::json!(42);
        value["profile"]["last_name"] = serde_json::json!("x".repeat(1025));
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(restore_profile_at(&path).is_err());
        profile.first_name = "x".repeat(1025);
        assert!(save_profile_at(&path, &profile, &fixture()).is_err());
        profile.first_name.clear();
        profile.id = 0;
        assert!(save_profile_at(&path, &profile, &fixture()).is_err());
    }

    #[test]
    fn only_vk_root_domains_and_safe_cookie_headers_are_accepted() {
        let mut cookies = fixture();
        for domain in ["id.vk.ru", "vk.ru.example.com", ".example.com", "VK.RU"] {
            cookies[0].domain = domain.into();
            assert!(validate(42, &cookies).is_err());
        }
        cookies[0].domain = ".vk.com".into();
        assert!(validate(42, &cookies).is_ok());
        cookies[0].value = "value\r\nHeader: injected".into();
        assert!(validate(42, &cookies).is_err());
        cookies[0].value = "safe".into();
        cookies[0].name = "name=value".into();
        assert!(validate(42, &cookies).is_err());
        assert!(validate(42, &[]).is_err());
    }

    #[test]
    fn malformed_foreign_and_oversized_music_cache_are_rejected() {
        let directory = TestDirectory::new();
        let path = directory.0.join("music-session.json");
        save_at(&path, 42, &fixture()).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(2);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_at(&path, 42).is_err());
        value["format_version"] = serde_json::json!(1);
        value["cookies"][0]["domain"] = serde_json::json!("example.com");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_at(&path, 42).is_err());
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(load_at(&path, 42).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_permissions_and_symlink_music_caches_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = TestDirectory::new();
        let path = directory.0.join("music-session.json");
        let target = directory.0.join("real.json");
        save_at(&target, 42, &fixture()).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            matches!(load_at(&target, 42), Err(error) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        symlink(&target, &path).unwrap();
        assert!(load_at(&path, 42).is_err());
        assert!(save_at(&path, 42, &fixture()).is_err());
        assert!(clear_at(&path).is_err());
    }
}
