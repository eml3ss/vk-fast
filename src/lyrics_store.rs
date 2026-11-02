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
pub enum Mode {
    #[default]
    Anonymous,
    ApiKey,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub api_key: String,
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
    pub api_retry_after: u64,
    #[serde(default)]
    pub token_rejected: bool,
}
impl Drop for Config {
    fn drop(&mut self) {
        self.api_key.zeroize();
        self.user_token.zeroize();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub mode: Mode,
    pub key_ready: bool,
    pub token_ready: bool,
    pub token_rejected: bool,
    pub token_retry_after: u64,
    pub request_retry_after: u64,
}
impl Config {
    pub fn status(&self) -> Status {
        Status {
            mode: self.mode,
            key_ready: !self.api_key.is_empty(),
            token_ready: self.has_current_token() && !self.token_rejected,
            token_rejected: self.token_rejected,
            token_retry_after: self.token_retry_after.max(self.anonymous_retry_after),
            request_retry_after: match self.mode {
                Mode::Anonymous => self.anonymous_retry_after,
                Mode::ApiKey => self.api_retry_after,
            },
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
    let config: Config = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
    if (!config.api_key.is_empty() && !valid_secret(&config.api_key))
        || (!config.user_token.is_empty() && !valid_secret(&config.user_token))
    {
        return Err(Error::Storage);
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
            api_key: "synthetic-key".into(),
            user_token: "synthetic-token".into(),
            token_app_id: super::super::APP_ID.into(),
            token_retry_after: 123,
            mode: Mode::ApiKey,
            ..Default::default()
        };
        save_at(&path, &config).unwrap();
        let restored = load_at(&path).unwrap();
        assert_eq!(restored.api_key, "synthetic-key");
        assert_eq!(restored.user_token, "synthetic-token");
        assert!(restored.has_current_token());
        assert_eq!(restored.status().mode, Mode::ApiKey);
        fs::write(&path, b"invalid synthetic record").unwrap();
        assert!(load_at(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"invalid synthetic record");
        fs::remove_dir_all(root).unwrap();
    }
}
