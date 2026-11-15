//! Private Musixmatch credentials, separate from ordinary UI preferences.
use super::Error;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Musixmatch,
    Vk,
    Genius,
    /// A new install starts here: open, with timed lyrics, no account.
    #[default]
    Lrclib,
    NetEase,
}
impl Source {
    /// Every source, in the order fallbacks are asked (Genius, which never
    /// has timing, last).
    pub const ALL: [Self; 5] = [
        Self::Vk,
        Self::Musixmatch,
        Self::Lrclib,
        Self::NetEase,
        Self::Genius,
    ];

    pub(super) fn index(self) -> usize {
        match self {
            Self::Vk => 0,
            Self::Musixmatch => 1,
            Self::Lrclib => 2,
            Self::NetEase => 3,
            Self::Genius => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Musixmatch => "Musixmatch",
            Self::Vk => "VK",
            Self::Genius => "Genius",
            Self::Lrclib => "LRCLIB",
            Self::NetEase => "NetEase",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    #[serde(default)]
    pub source: Source,
    /// On for a new install; an explicit choice in Settings is kept.
    #[serde(default = "genius_fallback_default")]
    pub fallback_to_genius: bool,
    /// Before each source had its own switch: VK and Musixmatch falling
    /// back to each other. Their own switches start from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback_to_other: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback_to_vk: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fallback_to_musixmatch: Option<bool>,
    /// LRCLIB, an open database with timed lyrics. On by default.
    #[serde(default = "genius_fallback_default")]
    pub fallback_to_lrclib: bool,
    #[serde(default)]
    pub genius_retry_after: u64,
    #[serde(default)]
    pub lrclib_retry_after: u64,
    /// NetEase Cloud Music. On by default.
    #[serde(default = "genius_fallback_default")]
    pub fallback_to_netease: bool,
    #[serde(default)]
    pub netease_retry_after: u64,
    #[serde(default)]
    pub vk_retry_after: u64,
    /// The retired personal API key option: read so older files still load,
    /// never kept in memory or written back.
    #[serde(default, rename = "mode", skip_serializing)]
    retired_mode: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "api_key", skip_serializing)]
    retired_key: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "api_retry_after", skip_serializing)]
    retired_retry: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pub user_token: String,
    #[serde(default)]
    pub token_app_id: String,
    #[serde(default)]
    pub token_created: u64,
    #[serde(default)]
    pub token_retry_after: u64,
    #[serde(default)]
    pub anonymous_retry_after: u64,
    #[serde(default)]
    pub token_rejected: bool,
}
fn genius_fallback_default() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: Source::default(),
            fallback_to_genius: genius_fallback_default(),
            fallback_to_other: None,
            fallback_to_vk: None,
            fallback_to_musixmatch: None,
            fallback_to_lrclib: genius_fallback_default(),
            genius_retry_after: 0,
            lrclib_retry_after: 0,
            fallback_to_netease: genius_fallback_default(),
            netease_retry_after: 0,
            vk_retry_after: 0,
            retired_mode: None,
            retired_key: None,
            retired_retry: None,
            user_token: String::new(),
            token_app_id: String::new(),
            token_created: 0,
            token_retry_after: 0,
            anonymous_retry_after: 0,
            token_rejected: false,
        }
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        self.user_token.zeroize();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub source: Source,
    /// Whether each source is asked when the chosen one has nothing, by
    /// `Source::index`.
    pub fallbacks: [bool; 5],
    pub token_ready: bool,
    pub token_rejected: bool,
    pub token_retry_after: u64,
    pub request_retry_after: u64,
}
/// What a new install has, shown until the saved settings are read.
impl Default for Status {
    fn default() -> Self {
        Config::default().status()
    }
}
impl Status {
    pub fn fallback(&self, source: Source) -> bool {
        self.fallbacks[source.index()]
    }
    pub fn set_fallback(&mut self, source: Source, enabled: bool) {
        self.fallbacks[source.index()] = enabled;
    }
}
impl Config {
    pub fn status(&self) -> Status {
        Status {
            source: self.source,
            fallbacks: Source::ALL.map(|source| self.fallback(source)),
            token_ready: self.has_current_token() && !self.token_rejected,
            token_rejected: self.token_rejected,
            token_retry_after: self.token_retry_after.max(self.anonymous_retry_after),
            request_retry_after: {
                let mut retry = self.source_retry_after();
                for source in Source::ALL {
                    if source != self.source && self.fallback(source) {
                        retry = retry.min(self.retry_after(source));
                    }
                }
                retry
            },
        }
    }

    /// Whether `source` is asked when the chosen one has nothing.
    pub fn fallback(&self, source: Source) -> bool {
        match source {
            Source::Vk => self
                .fallback_to_vk
                .or(self.fallback_to_other)
                .unwrap_or(true),
            Source::Musixmatch => self
                .fallback_to_musixmatch
                .or(self.fallback_to_other)
                .unwrap_or(true),
            Source::Lrclib => self.fallback_to_lrclib,
            Source::NetEase => self.fallback_to_netease,
            Source::Genius => self.fallback_to_genius,
        }
    }

    pub fn set_fallback(&mut self, source: Source, enabled: bool) {
        match source {
            Source::Vk => self.fallback_to_vk = Some(enabled),
            Source::Musixmatch => self.fallback_to_musixmatch = Some(enabled),
            Source::Lrclib => self.fallback_to_lrclib = enabled,
            Source::NetEase => self.fallback_to_netease = enabled,
            Source::Genius => self.fallback_to_genius = enabled,
        }
    }

    pub fn source_retry_after(&self) -> u64 {
        self.retry_after(self.source)
    }

    fn retry_after(&self, source: Source) -> u64 {
        match source {
            Source::Vk => self.vk_retry_after,
            Source::Genius => self.genius_retry_after,
            Source::Lrclib => self.lrclib_retry_after,
            Source::NetEase => self.netease_retry_after,
            Source::Musixmatch => self.anonymous_retry_after,
        }
    }

    pub fn has_current_token(&self) -> bool {
        self.token_app_id == super::APP_ID && valid_user_token(&self.user_token)
    }
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(super) fn valid_secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && value.bytes().all(|byte| (33..=126).contains(&byte))
}
pub(super) fn valid_user_token(value: &str) -> bool {
    valid_secret(value)
        && !value.bytes().all(|byte| byte == b'0')
        && !value.starts_with("UpgradeOnly")
}

fn path() -> Result<PathBuf, Error> {
    #[cfg(windows)]
    {
        Ok(crate::app_paths::config_dir()
            .map_err(|_| Error::Storage)?
            .join("musixmatch.json"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join(".config"))
            })
            .ok_or(Error::Storage)?;
        Ok(base.join("vk-fast/musixmatch.json"))
    }
}

fn safe_file(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16384 {
                return Err(Error::Storage);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(Error::Storage);
                }
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::Storage),
    }
}

fn load_at(path: &Path) -> Result<Config, Error> {
    if !safe_file(path)? {
        return Ok(Config::default());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)
        .map_err(|_| Error::Storage)?
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > 16384 {
        return Err(Error::Storage);
    }
    let mut config: Config = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
    if !config.user_token.is_empty() && !valid_secret(&config.user_token) {
        return Err(Error::Storage);
    }
    // A personal API key saved by an older version is erased from the file.
    if config.retired_mode.is_some()
        || config.retired_key.is_some()
        || config.retired_retry.is_some()
    {
        config.retired_mode = None;
        config.retired_key = None;
        config.retired_retry = None;
        save_at(path, &config)?;
    }
    Ok(config)
}

fn save_at(path: &Path, config: &Config) -> Result<(), Error> {
    safe_file(path)?;
    let parent = path.parent().ok_or(Error::Storage)?;
    fs::create_dir_all(parent).map_err(|_| Error::Storage)?;
    let metadata = fs::symlink_metadata(parent).map_err(|_| Error::Storage)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Storage)?;
    }
    let bytes = Zeroizing::new(serde_json::to_vec(config).map_err(|_| Error::Storage)?);
    if bytes.len() > 16384 {
        return Err(Error::Storage);
    }
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| Error::Storage)?;
    let temporary = parent.join(format!(
        ".musixmatch-{:016x}.tmp",
        u64::from_ne_bytes(random)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|_| Error::Storage)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Storage)?;
        drop(file);
        safe_file(path)?;
        fs::rename(&temporary, path).map_err(|_| Error::Storage)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(super) fn load() -> Result<Config, Error> {
    if cfg!(test) {
        return Ok(Config::default());
    }
    load_at(&path()?)
}
pub(super) fn save(config: &Config) -> Result<(), Error> {
    if cfg!(test) {
        return Ok(());
    }
    save_at(&path()?, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_and_placeholder_tokens_are_not_reported_as_ready() {
        let mut config: Config =
            serde_json::from_str(r#"{"user_token":"synthetic-legacy-token"}"#).unwrap();
        assert!(!config.status().token_ready);
        config.token_app_id = super::super::APP_ID.into();
        assert!(config.status().token_ready);
        for invalid in [
            "".to_owned(),
            "0".repeat(56),
            "UpgradeOnly-synthetic".into(),
        ] {
            config.user_token = invalid;
            assert!(!config.status().token_ready);
            assert!(!valid_user_token(&config.user_token));
        }
        config.user_token = "synthetic-current-token".into();
        config.token_rejected = true;
        assert!(!config.status().token_ready);
    }
    #[test]
    fn credentials_roundtrip_privately_and_corruption_is_not_overwritten_by_load() {
        let root = std::env::temp_dir().join(format!(
            "vk-fast-lyrics-store-{}-{}",
            std::process::id(),
            now()
        ));
        let path = root.join("musixmatch.json");
        let config = Config {
            source: Source::Vk,
            fallback_to_genius: true,
            user_token: "synthetic-token".into(),
            token_app_id: super::super::APP_ID.into(),
            token_retry_after: 123,
            ..Default::default()
        };
        save_at(&path, &config).unwrap();
        let restored = load_at(&path).unwrap();
        assert_eq!(restored.user_token, "synthetic-token");
        assert!(restored.has_current_token());
        assert_eq!(restored.source, Source::Vk);
        assert!(restored.fallback_to_genius);
        // A file from before per-source switches: its VK/Musixmatch choice
        // carries over to both, and a new switch for one keeps the other.
        let mut older: Config = serde_json::from_str(
            r#"{"source":"vk","fallback_to_other":false,"fallback_to_genius":true}"#,
        )
        .unwrap();
        assert!(!older.fallback(Source::Vk) && !older.fallback(Source::Musixmatch));
        assert!(older.fallback(Source::Lrclib), "LRCLIB on by default");
        older.set_fallback(Source::Musixmatch, true);
        let migrated = root.join("migrated.json");
        save_at(&migrated, &older).unwrap();
        let restored = load_at(&migrated).unwrap();
        assert!(restored.fallback(Source::Musixmatch) && !restored.fallback(Source::Vk));
        assert_eq!(Config::default().source, Source::Lrclib, "a new install");
        // An older file with a personal API key loads, and the key is erased.
        let saved = fs::read_to_string(&path).unwrap();
        let mut older: serde_json::Value = serde_json::from_str(&saved).unwrap();
        older["mode"] = "api_key".into();
        older["api_key"] = "synthetic-retired-key".into();
        older["api_retry_after"] = 9.into();
        fs::write(&path, older.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let restored = load_at(&path).unwrap();
        assert_eq!(restored.user_token, "synthetic-token");
        assert_eq!(restored.status().request_retry_after, 0);
        let rewritten = fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains("synthetic-retired-key") && !rewritten.contains("api_key"));
        assert!(!rewritten.contains("\"mode\""));
        fs::write(&path, b"invalid synthetic record").unwrap();
        assert!(load_at(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"invalid synthetic record");
        fs::remove_dir_all(root).unwrap();
    }
}
