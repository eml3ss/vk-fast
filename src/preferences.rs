//! Small local preferences, kept separately from authentication credentials.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::i18n::Language;
#[cfg(test)]
use crate::palette::Accent;
use crate::palette::Theme;
use crate::play_order::RepeatMode;

const MAX_BYTES: u64 = 4096;
pub const MAX_CACHE_MB: u32 = 5120;
pub const DEFAULT_CACHE_MB: u32 = 1024;
const DEFAULT_VOLUME: f32 = 0.7;
const DEFAULT_COMPACT_SEEK_BAR: bool = true;
const DEFAULT_DOUBLE_CLICK_TO_PLAY: bool = true;
const DEFAULT_SYSTEM_TITLEBAR: bool = false;
const DEFAULT_MINIMIZE_TO_TRAY: bool = false;
const DEFAULT_LOGGING_ENABLED: bool = false;
const DEFAULT_DISCORD_RPC_ENABLED: bool = true;
const DEFAULT_AUTO_UPDATES: bool = true;
const DEFAULT_SEARCH_ON_ENTER: bool = true;
const DEFAULT_MIX_AFTER_QUEUE: bool = true;
pub const DEFAULT_CROSSFADE_SECONDS: u8 = 6;
pub const MAX_CROSSFADE_SECONDS: u8 = 12;
const DEFAULT_EXPLICIT_BADGE: bool = true;
const DEFAULT_ANIMATED_BACKDROP: bool = true;
static WRITES: Mutex<()> = Mutex::new(());

#[derive(Deserialize, Serialize)]
#[serde(default)]
struct Preferences {
    #[serde(deserialize_with = "deserialize_shuffle")]
    shuffle: bool,
    #[serde(deserialize_with = "deserialize_repeat")]
    repeat: RepeatMode,
    #[serde(deserialize_with = "deserialize_volume")]
    volume: f32,
    #[serde(deserialize_with = "deserialize_cache_limit")]
    cache_limit_mb: u32,
    #[serde(deserialize_with = "deserialize_language")]
    language: String,
    /// The appearance theme's code; empty for the default dark theme.
    #[serde(
        deserialize_with = "deserialize_theme",
        skip_serializing_if = "String::is_empty"
    )]
    theme: String,
    /// The accent colour's code; empty for the original blue.
    #[serde(
        deserialize_with = "deserialize_accent",
        skip_serializing_if = "String::is_empty"
    )]
    accent: String,
    #[serde(deserialize_with = "deserialize_compact_seek_bar")]
    compact_seek_bar: bool,
    #[serde(deserialize_with = "deserialize_system_titlebar")]
    system_titlebar: bool,
    #[serde(deserialize_with = "deserialize_double_click_to_play")]
    double_click_to_play: bool,
    #[serde(deserialize_with = "deserialize_minimize_to_tray")]
    minimize_to_tray: bool,
    #[serde(deserialize_with = "deserialize_logging_enabled")]
    logging_enabled: bool,
    #[serde(deserialize_with = "deserialize_discord_rpc_enabled")]
    discord_rpc_enabled: bool,
    #[serde(deserialize_with = "deserialize_auto_updates")]
    auto_updates: bool,
    #[serde(deserialize_with = "deserialize_search_on_enter")]
    search_on_enter: bool,
    #[serde(deserialize_with = "deserialize_middle_autoscroll")]
    middle_autoscroll: bool,
    #[serde(deserialize_with = "deserialize_mix_after_queue")]
    mix_after_queue: bool,
    /// Off unless turned on.
    #[serde(deserialize_with = "deserialize_crossfade")]
    crossfade: bool,
    #[serde(deserialize_with = "deserialize_crossfade_seconds")]
    crossfade_seconds: u8,
    #[serde(deserialize_with = "deserialize_explicit_badge")]
    explicit_badge: bool,
    #[serde(deserialize_with = "deserialize_animated_backdrop")]
    animated_backdrop: bool,
    /// Off unless turned on: it takes Ctrl + volume keys from the system.
    #[serde(deserialize_with = "deserialize_volume_keys")]
    volume_keys: bool,
    /// Off unless the user turns it on: it changes the account's page.
    #[serde(deserialize_with = "deserialize_broadcast")]
    broadcast: bool,
    /// The browser that receives non-music links while VK Fast handles web
    /// links: a desktop entry id on Linux, a ProgId on Windows.
    #[serde(
        deserialize_with = "deserialize_link_browser",
        skip_serializing_if = "String::is_empty"
    )]
    link_browser: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            shuffle: false,
            repeat: RepeatMode::Off,
            volume: DEFAULT_VOLUME,
            cache_limit_mb: DEFAULT_CACHE_MB,
            language: String::new(),
            theme: String::new(),
            accent: String::new(),
            compact_seek_bar: DEFAULT_COMPACT_SEEK_BAR,
            system_titlebar: DEFAULT_SYSTEM_TITLEBAR,
            double_click_to_play: DEFAULT_DOUBLE_CLICK_TO_PLAY,
            minimize_to_tray: DEFAULT_MINIMIZE_TO_TRAY,
            logging_enabled: DEFAULT_LOGGING_ENABLED,
            discord_rpc_enabled: DEFAULT_DISCORD_RPC_ENABLED,
            auto_updates: DEFAULT_AUTO_UPDATES,
            search_on_enter: DEFAULT_SEARCH_ON_ENTER,
            middle_autoscroll: false,
            mix_after_queue: DEFAULT_MIX_AFTER_QUEUE,
            crossfade: false,
            crossfade_seconds: DEFAULT_CROSSFADE_SECONDS,
            explicit_badge: DEFAULT_EXPLICIT_BADGE,
            animated_backdrop: DEFAULT_ANIMATED_BACKDROP,
            volume_keys: false,
            broadcast: false,
            link_browser: String::new(),
            extra: serde_json::Map::new(),
        }
    }
}

fn deserialize_theme<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_str()
        .filter(|code| crate::themes::valid_id(code))
        .unwrap_or_default()
        .to_owned())
}

fn deserialize_accent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_str()
        .filter(|code| crate::palette::parse_accent(code).is_some())
        .unwrap_or_default()
        .to_owned())
}

fn deserialize_link_browser<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_str()
        .filter(|id| crate::link_router::valid_handler(id))
        .unwrap_or_default()
        .to_owned())
}

fn deserialize_shuffle<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

fn deserialize_repeat<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<RepeatMode, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn deserialize_volume<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_f64()
        .map(|volume| volume as f32)
        .filter(|volume| valid_volume(*volume))
        .unwrap_or(DEFAULT_VOLUME))
}

fn deserialize_cache_limit<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|limit| u32::try_from(limit).ok())
        .filter(|limit| *limit <= MAX_CACHE_MB)
        .unwrap_or(DEFAULT_CACHE_MB))
}

fn language_code(language: Language) -> &'static str {
    match language {
        Language::Russian => "ru",
        Language::English => "en",
    }
}

fn deserialize_language<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_str()
        .and_then(Language::from_code)
        .map(language_code)
        .unwrap_or_default()
        .to_owned())
}

fn deserialize_compact_seek_bar<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(DEFAULT_COMPACT_SEEK_BAR))
}

fn deserialize_system_titlebar<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(DEFAULT_SYSTEM_TITLEBAR))
}

fn deserialize_double_click_to_play<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(DEFAULT_DOUBLE_CLICK_TO_PLAY))
}

fn deserialize_minimize_to_tray<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(DEFAULT_MINIMIZE_TO_TRAY))
}

fn deserialize_logging_enabled<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(DEFAULT_LOGGING_ENABLED))
}

fn deserialize_discord_rpc_enabled<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(DEFAULT_DISCORD_RPC_ENABLED))
}

fn valid_volume(volume: f32) -> bool {
    volume.is_finite() && (0.0..=1.0).contains(&volume)
}

fn deserialize_auto_updates<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(DEFAULT_AUTO_UPDATES))
}

fn deserialize_search_on_enter<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(DEFAULT_SEARCH_ON_ENTER))
}

fn deserialize_mix_after_queue<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(DEFAULT_MIX_AFTER_QUEUE))
}

fn deserialize_crossfade<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

fn deserialize_crossfade_seconds<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u8, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_u64()
        .filter(|seconds| (1..=u64::from(MAX_CROSSFADE_SECONDS)).contains(seconds))
        .map_or(DEFAULT_CROSSFADE_SECONDS, |seconds| seconds as u8))
}

fn deserialize_explicit_badge<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(DEFAULT_EXPLICIT_BADGE))
}

fn deserialize_volume_keys<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

fn deserialize_animated_backdrop<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(DEFAULT_ANIMATED_BACKDROP))
}

fn deserialize_broadcast<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

fn deserialize_middle_autoscroll<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid VK Fast preferences")
}

fn preferences_path() -> io::Result<PathBuf> {
    Ok(crate::app_paths::config_dir()?.join("preferences.json"))
}

fn legacy_preferences_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        preferences_path()
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
        Ok(state.join("vk-fast/preferences.json"))
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
                "Preferences directory is not private",
            ));
        }
    }
    Ok(parent)
}

fn checked_target(path: &Path) -> io::Result<Option<fs::Metadata>> {
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
        .map_err(|_| io::Error::other("Preferences temporary file name unavailable"))?;
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

fn write_at(path: &Path, preferences: &Preferences) -> io::Result<()> {
    write_mode_at(path, preferences, true).map(|_| ())
}

fn write_mode_at(path: &Path, preferences: &Preferences, replace: bool) -> io::Result<bool> {
    let bytes = serde_json::to_vec(preferences).map_err(|_| invalid())?;
    write_bytes_at(path, &bytes, MAX_BYTES, replace)
}

/// Publishes `bytes` as a private file at once: a fsynced temporary file
/// renamed (or, without `replace`, linked) into place.
fn write_bytes_at(path: &Path, bytes: &[u8], limit: u64, replace: bool) -> io::Result<bool> {
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    let parent = checked_parent(path, true)?;
    if checked_target(path)?.is_some() && !replace {
        return Ok(false);
    }
    let (mut file, mut temporary) = loop {
        let temporary_path = parent.join(format!(".preferences-{}.tmp", random_suffix()?));
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
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let temporary_path = temporary.0.as_ref().ok_or_else(invalid)?;
    if replace {
        fs::rename(temporary_path, path)?;
    } else {
        // A same-directory hard link publishes an already-fsynced file without
        // replacing a config file another process created during migration.
        match fs::hard_link(temporary_path, path) {
            Ok(()) => fs::remove_file(temporary_path)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    temporary.0 = None;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(true)
}

fn read_bytes_at(path: &Path) -> io::Result<Option<Vec<u8>>> {
    read_limited_at(path, MAX_BYTES)
}

fn read_limited_at(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let Some(metadata) = checked_target(path)? else {
        return Ok(None);
    };
    checked_parent(path, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Preferences file is not private",
            ));
        }
    }
    if metadata.len() > limit {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(Some(bytes))
}

fn load_json_at(path: &Path) -> io::Result<Option<serde_json::Value>> {
    let Some(bytes) = read_bytes_at(path)? else {
        return Ok(None);
    };
    let preferences: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if !preferences.is_object() {
        return Err(invalid());
    }
    Ok(Some(preferences))
}

fn load_preferences_at(path: &Path) -> io::Result<Option<Preferences>> {
    load_json_at(path)?
        .map(|json| serde_json::from_value(json).map_err(|_| invalid()))
        .transpose()
}

fn load_at(path: &Path) -> io::Result<Option<f32>> {
    let Some(json) = load_json_at(path)? else {
        return Ok(None);
    };
    match json.get("volume") {
        Some(value) => value
            .as_f64()
            .map(|volume| volume as f32)
            .filter(|volume| valid_volume(*volume))
            .map(Some)
            .ok_or_else(invalid),
        None if json.get("cache_limit_mb").is_some()
            || json.get("language").is_some()
            || json.get("compact_seek_bar").is_some()
            || json.get("system_titlebar").is_some()
            || json.get("double_click_to_play").is_some()
            // Volume normalization was removed; old files still carry it.
            || json.get("normalize_volume").is_some()
            || json.get("minimize_to_tray").is_some()
            || json.get("logging_enabled").is_some()
            || json.get("discord_rpc_enabled").is_some()
            || json.get("auto_updates").is_some()
            || json.get("shuffle").is_some()
            || json.get("repeat").is_some() =>
        {
            Ok(Some(DEFAULT_VOLUME))
        }
        None => Err(invalid()),
    }
}

fn save_at(path: &Path, volume: f32) -> io::Result<()> {
    if !valid_volume(volume) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Volume must be finite and between zero and one",
        ));
    }
    update_at(path, |preferences| preferences.volume = volume)
}

fn update_at(path: &Path, update: impl FnOnce(&mut Preferences)) -> io::Result<()> {
    let _guard = WRITES
        .lock()
        .map_err(|_| io::Error::other("Preferences writer unavailable"))?;
    let mut preferences = load_preferences_at(path)?.unwrap_or_default();
    update(&mut preferences);
    write_at(path, &preferences)
}

fn save_cache_at(path: &Path, limit_mb: u32) -> io::Result<()> {
    if limit_mb > MAX_CACHE_MB {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cache limit exceeds the five GiB maximum",
        ));
    }
    update_at(path, |preferences| preferences.cache_limit_mb = limit_mb)
}

fn save_language_at(path: &Path, language: Language) -> io::Result<()> {
    update_at(path, |preferences| {
        preferences.language = language_code(language).into();
    })
}

fn save_compact_seek_at(path: &Path, compact: bool) -> io::Result<()> {
    update_at(path, |preferences| preferences.compact_seek_bar = compact)
}

fn save_system_titlebar_at(path: &Path, system: bool) -> io::Result<()> {
    update_at(path, |preferences| preferences.system_titlebar = system)
}

fn save_double_click_at(path: &Path, double_click: bool) -> io::Result<()> {
    update_at(path, |preferences| {
        preferences.double_click_to_play = double_click
    })
}

fn save_minimize_to_tray_at(path: &Path, enabled: bool) -> io::Result<()> {
    update_at(path, |preferences| preferences.minimize_to_tray = enabled)
}

fn save_logging_at(path: &Path, enabled: bool) -> io::Result<()> {
    update_at(path, |preferences| preferences.logging_enabled = enabled)
}

fn save_discord_rpc_at(path: &Path, enabled: bool) -> io::Result<()> {
    update_at(path, |preferences| {
        preferences.discord_rpc_enabled = enabled
    })
}

fn migrate_at(legacy: &Path, destination: &Path) -> io::Result<()> {
    let _guard = WRITES
        .lock()
        .map_err(|_| io::Error::other("Preferences writer unavailable"))?;
    if checked_target(destination)?.is_some() {
        return Ok(());
    }
    let Some(original) = read_bytes_at(legacy)? else {
        return Ok(());
    };
    let json: serde_json::Value = serde_json::from_slice(&original).map_err(|_| invalid())?;
    if !json.is_object() {
        return Err(invalid());
    }
    let preferences: Preferences = serde_json::from_value(json).map_err(|_| invalid())?;
    if !write_mode_at(destination, &preferences, false)? {
        return Ok(());
    }
    // Do not delete changes saved by an older running version after our read.
    // Such a file can remain as a legacy copy; the new config always takes priority.
    remove_legacy_if_unchanged(legacy, &original)
}

fn remove_legacy_if_unchanged(legacy: &Path, original: &[u8]) -> io::Result<()> {
    if read_bytes_at(legacy)?.as_deref() == Some(original) {
        fs::remove_file(legacy)?;
        #[cfg(unix)]
        File::open(legacy.parent().ok_or_else(invalid)?)?.sync_all()?;
    }
    Ok(())
}

/// The equalizer's file: its curve and the user's presets, beside the
/// preferences but on its own, since presets outgrow their size limit.
const EQUALIZER_BYTES: u64 = 16 * 1024;

fn equalizer_path() -> io::Result<PathBuf> {
    let preferences = preferences_path()?;
    Ok(preferences
        .parent()
        .ok_or_else(invalid)?
        .join("equalizer.json"))
}

fn load_equalizer_at(path: &Path) -> io::Result<crate::equalizer::Settings> {
    let Some(bytes) = read_limited_at(path, EQUALIZER_BYTES)? else {
        return Ok(crate::equalizer::Settings::default());
    };
    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if !json.is_object() {
        return Err(invalid());
    }
    serde_json::from_value::<crate::equalizer::Settings>(json)
        .map(crate::equalizer::Settings::cleaned)
        .map_err(|_| invalid())
}

fn save_equalizer_at(path: &Path, settings: &crate::equalizer::Settings) -> io::Result<()> {
    let bytes = serde_json::to_vec(&settings.clone().cleaned()).map_err(|_| invalid())?;
    let _guard = WRITES
        .lock()
        .map_err(|_| io::Error::other("Preferences writer unavailable"))?;
    write_bytes_at(path, &bytes, EQUALIZER_BYTES, true).map(|_| ())
}

/// Off and flat when there is no file or it cannot be read.
pub fn load_equalizer() -> crate::equalizer::Settings {
    equalizer_path()
        .and_then(|path| load_equalizer_at(&path))
        .unwrap_or_default()
}

pub fn save_equalizer(settings: &crate::equalizer::Settings) -> io::Result<()> {
    save_equalizer_at(&equalizer_path()?, settings)
}

pub fn load_volume() -> Option<f32> {
    load_at(&preferences_path().ok()?).ok().flatten()
}

pub fn load_playback_modes() -> (bool, RepeatMode) {
    if cfg!(test) {
        return (false, RepeatMode::Off);
    }
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map(|prefs| (prefs.shuffle, prefs.repeat))
        .unwrap_or_default()
}

pub fn save_playback_modes(shuffle: bool, repeat: RepeatMode) -> io::Result<()> {
    save_playback_modes_at(&preferences_path()?, shuffle, repeat)
}

fn save_playback_modes_at(path: &Path, shuffle: bool, repeat: RepeatMode) -> io::Result<()> {
    update_at(path, |prefs| {
        prefs.shuffle = shuffle;
        prefs.repeat = repeat;
    })
}

pub fn save_volume(volume: f32) -> io::Result<()> {
    save_at(&preferences_path()?, volume)
}

pub fn load_cache_limit_mb() -> u32 {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_CACHE_MB, |preferences| preferences.cache_limit_mb)
}

pub fn save_cache_limit_mb(limit_mb: u32) -> io::Result<()> {
    save_cache_at(&preferences_path()?, limit_mb)
}

pub fn load_language() -> Language {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .and_then(|preferences| Language::from_code(&preferences.language))
        .unwrap_or_else(Language::system)
}

pub fn save_language(language: Language) -> io::Result<()> {
    save_language_at(&preferences_path()?, language)
}

/// The appearance theme; the original dark one unless another was chosen.
pub fn load_theme() -> Theme {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .and_then(|preferences| Theme::from_code(&preferences.theme))
        .unwrap_or_default()
}

pub fn save_theme(theme: Theme) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.theme = theme.code().to_owned()
    })
}

/// The accent colour; the theme's own unless one was chosen.
pub fn load_accent() -> eframe::egui::Color32 {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .and_then(|preferences| crate::palette::parse_accent(&preferences.accent))
        .unwrap_or(crate::palette::THEME_ACCENT)
}

pub fn save_accent(accent: eframe::egui::Color32) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.accent = crate::palette::accent_code(accent)
    })
}

pub fn load_compact_seek_bar() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_COMPACT_SEEK_BAR, |preferences| {
            preferences.compact_seek_bar
        })
}

pub fn save_compact_seek_bar(compact: bool) -> io::Result<()> {
    save_compact_seek_at(&preferences_path()?, compact)
}

pub fn load_system_titlebar() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_SYSTEM_TITLEBAR, |preferences| {
            preferences.system_titlebar
        })
}

pub fn save_system_titlebar(system: bool) -> io::Result<()> {
    save_system_titlebar_at(&preferences_path()?, system)
}

pub fn load_double_click_to_play() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_DOUBLE_CLICK_TO_PLAY, |preferences| {
            preferences.double_click_to_play
        })
}

pub fn save_double_click_to_play(double_click: bool) -> io::Result<()> {
    save_double_click_at(&preferences_path()?, double_click)
}

pub fn load_minimize_to_tray() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_MINIMIZE_TO_TRAY, |preferences| {
            preferences.minimize_to_tray
        })
}

pub fn save_minimize_to_tray(enabled: bool) -> io::Result<()> {
    save_minimize_to_tray_at(&preferences_path()?, enabled)
}

pub fn load_logging_enabled() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_LOGGING_ENABLED, |preferences| {
            preferences.logging_enabled
        })
}

pub fn save_logging_enabled(enabled: bool) -> io::Result<()> {
    save_logging_at(&preferences_path()?, enabled)
}

pub fn load_discord_rpc_enabled() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_DISCORD_RPC_ENABLED, |preferences| {
            preferences.discord_rpc_enabled
        })
}

pub fn save_discord_rpc_enabled(enabled: bool) -> io::Result<()> {
    save_discord_rpc_at(&preferences_path()?, enabled)
}

pub fn load_mix_after_queue() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_MIX_AFTER_QUEUE, |preferences| {
            preferences.mix_after_queue
        })
}

/// Whether tracks crossfade, and over how many seconds.
pub fn load_crossfade() -> (bool, u8) {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or((false, DEFAULT_CROSSFADE_SECONDS), |preferences| {
            (preferences.crossfade, preferences.crossfade_seconds)
        })
}

pub fn save_crossfade(enabled: bool, seconds: u8) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.crossfade = enabled;
        preferences.crossfade_seconds = seconds.clamp(1, MAX_CROSSFADE_SECONDS);
    })
}

pub fn save_mix_after_queue(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.mix_after_queue = enabled
    })
}

/// The "E" badge by explicit tracks; on unless turned off in Settings.
pub fn load_explicit_badge() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_EXPLICIT_BADGE, |preferences| {
            preferences.explicit_badge
        })
}

pub fn save_explicit_badge(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.explicit_badge = enabled
    })
}

/// Ctrl + the volume keys change VK Fast's volume; off by default.
pub fn load_volume_keys() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .is_some_and(|preferences| preferences.volume_keys)
}

pub fn save_volume_keys(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.volume_keys = enabled
    })
}

/// The animated blurred backdrop of the fullscreen player; on by default.
pub fn load_animated_backdrop() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_ANIMATED_BACKDROP, |preferences| {
            preferences.animated_backdrop
        })
}

pub fn save_animated_backdrop(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.animated_backdrop = enabled
    })
}

pub fn load_broadcast() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .is_some_and(|preferences| preferences.broadcast)
}

pub fn save_broadcast(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.broadcast = enabled
    })
}

pub fn load_middle_autoscroll() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .is_some_and(|preferences| preferences.middle_autoscroll)
}

pub fn save_middle_autoscroll(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.middle_autoscroll = enabled
    })
}

pub fn load_search_on_enter() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_SEARCH_ON_ENTER, |preferences| {
            preferences.search_on_enter
        })
}

pub fn save_search_on_enter(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.search_on_enter = enabled
    })
}

#[cfg(any(windows, test))]
pub fn load_auto_updates() -> bool {
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map_or(DEFAULT_AUTO_UPDATES, |preferences| preferences.auto_updates)
}

#[cfg(any(windows, test))]
pub fn save_auto_updates(enabled: bool) -> io::Result<()> {
    update_at(&preferences_path()?, |preferences| {
        preferences.auto_updates = enabled
    })
}

pub fn load_link_browser() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    preferences_path()
        .ok()
        .and_then(|path| load_preferences_at(&path).ok().flatten())
        .map(|preferences| preferences.link_browser)
        .filter(|id| crate::link_router::valid_handler(id))
}

pub fn save_link_browser(id: &str) -> io::Result<()> {
    if !crate::link_router::valid_handler(id) {
        return Err(invalid());
    }
    update_at(&preferences_path()?, |preferences| {
        preferences.link_browser = id.to_owned()
    })
}

/// Explicit startup migration. Loaders never create, move, or remove files.
pub fn migrate_legacy() -> io::Result<()> {
    let destination = preferences_path()?;
    if checked_target(&destination)?.is_some() {
        return Ok(());
    }
    migrate_at(&legacy_preferences_path()?, &destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "vk-fast-preferences-test-{}",
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

    #[test]
    fn automatic_updates_default_on_and_explicit_opt_out_survives_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        for json in [
            r#"{}"#,
            r#"{"volume":0.2}"#,
            r#"{"auto_updates":"invalid"}"#,
        ] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert!(preferences.auto_updates);
        }
        update_at(&path, |preferences| preferences.auto_updates = false).unwrap();
        save_at(&path, 0.375).unwrap();
        save_playback_modes_at(&path, true, RepeatMode::All).unwrap();
        let restored = load_preferences_at(&path).unwrap().unwrap();
        assert!(!restored.auto_updates);
        assert_eq!(restored.volume, 0.375);
        assert!(restored.shuffle);
    }

    #[test]
    fn crossfade_is_off_at_six_seconds_by_default_and_keeps_a_valid_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vk-fast/preferences.json");
        for json in [
            r#"{}"#,
            r#"{"crossfade":"yes","crossfade_seconds":0}"#,
            r#"{"crossfade_seconds":99}"#,
            r#"{"crossfade_seconds":-3}"#,
        ] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert!(!preferences.crossfade);
            assert_eq!(preferences.crossfade_seconds, DEFAULT_CROSSFADE_SECONDS);
        }
        update_at(&path, |preferences| {
            preferences.crossfade = true;
            preferences.crossfade_seconds = 9;
        })
        .unwrap();
        let restored = load_preferences_at(&path).unwrap().unwrap();
        assert!(restored.crossfade);
        assert_eq!(restored.crossfade_seconds, 9);
    }

    #[test]
    fn equalizer_settings_round_trip_beside_the_preferences_and_bad_files_read_as_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vk-fast/equalizer.json");
        assert_eq!(
            load_equalizer_at(&path).unwrap(),
            crate::equalizer::Settings::default()
        );
        let mut settings = crate::equalizer::Settings {
            enabled: true,
            gains: crate::equalizer::Builtin::Rock.gains(),
            presets: Vec::new(),
        };
        for index in 0..crate::equalizer::MAX_PRESETS {
            assert!(
                settings.save_preset(
                    &"Длинное имя пресета "
                        .repeat(3)
                        .replace("пресета", &index.to_string())
                )
            );
        }
        save_equalizer_at(&path, &settings).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        }
        assert_eq!(load_equalizer_at(&path).unwrap(), settings);
        for bad in [&b"[]"[..], b"{\"gains\":[1,2]}", b"not json"] {
            fs::write(&path, bad).unwrap();
            assert!(load_equalizer_at(&path).is_err());
        }
        fs::write(
            &path,
            br#"{"enabled":true,"gains":[99,0,0,0,0,0,0,0,0,0],"future":1}"#,
        )
        .unwrap();
        let loaded = load_equalizer_at(&path).unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.gains[0], crate::equalizer::MAX_DB);
    }

    #[test]
    fn volume_keys_default_off_and_turning_them_on_survives_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        for json in [r#"{}"#, r#"{"volume_keys":"invalid"}"#] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert!(!preferences.volume_keys);
        }
        update_at(&path, |preferences| preferences.volume_keys = true).unwrap();
        save_at(&path, 0.375).unwrap();
        let restored = load_preferences_at(&path).unwrap().unwrap();
        assert!(restored.volume_keys);
        assert_eq!(restored.volume, 0.375);
    }

    #[test]
    fn animated_backdrop_defaults_on_and_turning_it_off_survives_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        for json in [r#"{}"#, r#"{"animated_backdrop":"invalid"}"#] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert!(preferences.animated_backdrop);
        }
        update_at(&path, |preferences| preferences.animated_backdrop = false).unwrap();
        save_at(&path, 0.375).unwrap();
        let restored = load_preferences_at(&path).unwrap().unwrap();
        assert!(!restored.animated_backdrop);
        assert_eq!(restored.volume, 0.375);
    }

    #[test]
    fn theme_defaults_to_dark_and_a_chosen_theme_survives_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        for json in [
            r#"{}"#,
            r#"{"theme":"white"}"#,
            r#"{"theme":7}"#,
            r#"{"theme":null}"#,
        ] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(Theme::from_code(&preferences.theme), None);
        }
        // The default theme writes nothing new to an old file.
        assert!(
            !serde_json::to_string(&Preferences::default())
                .unwrap()
                .contains("theme")
        );
        for code in Theme::all().map(Theme::code).chain(["night-drive"]) {
            update_at(&path, |preferences| preferences.theme = code.to_owned()).unwrap();
            save_at(&path, 0.375).unwrap();
            let restored = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(restored.theme, code);
            assert_eq!(restored.volume, 0.375);
        }
        let restored: Preferences = serde_json::from_str(r#"{"theme":"oled"}"#).unwrap();
        assert_eq!(restored.theme, "oled");
    }

    #[test]
    fn accent_defaults_to_blue_and_a_chosen_accent_survives_theme_and_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        for json in [
            r#"{}"#,
            r#"{"accent":"gold"}"#,
            r#"{"accent":3}"#,
            r##"{"accent":"#12345"}"##,
        ] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert_eq!(crate::palette::parse_accent(&preferences.accent), None);
        }
        let custom: Preferences = serde_json::from_str(r##"{"accent":"#e01f7a"}"##).unwrap();
        assert_eq!(
            crate::palette::parse_accent(&custom.accent),
            Some(eframe::egui::Color32::from_rgb(0xe0, 0x1f, 0x7a))
        );
        assert!(
            !serde_json::to_string(&Preferences::default())
                .unwrap()
                .contains("accent")
        );
        update_at(&path, |preferences| preferences.theme = "oled".to_owned()).unwrap();
        for accent in Accent::ALL {
            update_at(&path, |preferences| {
                preferences.accent = accent.code().to_owned()
            })
            .unwrap();
            save_at(&path, 0.375).unwrap();
            let restored = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(Accent::from_code(&restored.accent), Some(accent));
            assert_eq!(restored.theme, "oled");
            assert_eq!(restored.volume, 0.375);
        }
    }

    #[test]
    fn explicit_badge_defaults_on_and_turning_it_off_survives_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        for json in [r#"{}"#, r#"{"explicit_badge":"invalid"}"#] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert!(preferences.explicit_badge);
        }
        update_at(&path, |preferences| preferences.explicit_badge = false).unwrap();
        save_at(&path, 0.375).unwrap();
        let restored = load_preferences_at(&path).unwrap().unwrap();
        assert!(!restored.explicit_badge);
        assert_eq!(restored.volume, 0.375);
    }

    #[test]
    fn playback_modes_roundtrip_without_overwriting_other_preferences() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.375).unwrap();
        for (shuffle, repeat) in [
            (true, RepeatMode::All),
            (false, RepeatMode::One),
            (true, RepeatMode::Off),
        ] {
            save_playback_modes_at(&path, shuffle, repeat).unwrap();
            let restored = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!((restored.shuffle, restored.repeat), (shuffle, repeat));
            assert_eq!(restored.volume, 0.375);
            let order =
                crate::play_order::PlaybackOrder::with_modes(restored.shuffle, restored.repeat);
            assert_eq!(order.shuffle(), shuffle);
            assert_eq!(order.repeat(), repeat);
        }
        let legacy: Preferences = serde_json::from_str(r#"{"volume":0.25}"#).unwrap();
        assert_eq!((legacy.shuffle, legacy.repeat), (false, RepeatMode::Off));
        let invalid: Preferences =
            serde_json::from_str(r#"{"volume":0.25,"shuffle":"invalid","repeat":"invalid"}"#)
                .unwrap();
        assert_eq!((invalid.shuffle, invalid.repeat), (false, RepeatMode::Off));
        assert_eq!(invalid.volume, 0.25);
    }

    #[test]
    fn logging_is_opt_in_and_its_atomic_updates_preserve_existing_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        assert!(!Preferences::default().logging_enabled);
        save_at(&path, 0.375).unwrap();
        let mut value = serde_json::json!({"volume":0.375,"volume_keys":true,"compact_seek_bar":false,"future_setting":{"keep":true}});
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(!load_preferences_at(&path).unwrap().unwrap().logging_enabled);
        for enabled in [true, false] {
            save_logging_at(&path, enabled).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.logging_enabled, enabled);
            assert_eq!(preferences.volume, 0.375);
            assert!(preferences.volume_keys && !preferences.compact_seek_bar);
            assert_eq!(preferences.extra["future_setting"]["keep"], true);
        }
        for malformed in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(42),
        ] {
            value["logging_enabled"] = malformed;
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(!load_preferences_at(&path).unwrap().unwrap().logging_enabled);
        }
        fs::write(&path, br#"{"logging_enabled":true}"#).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
    }

    #[test]
    fn discord_rpc_defaults_to_enabled_and_preserves_disabled_choices_during_updates_and_migration()
    {
        let directory = TestDirectory::new();
        let legacy = directory.0.join("state/preferences.json");
        let destination = directory.0.join("config/preferences.json");
        assert!(Preferences::default().discord_rpc_enabled);
        assert!(load_preferences_at(&legacy).unwrap().is_none());
        assert!(
            !legacy.exists(),
            "Loading defaults must not create settings"
        );
        save_at(&legacy, 0.375).unwrap();
        fs::write(
            &legacy,
            serde_json::to_vec(&serde_json::json!({
                "volume":0.375, "cache_limit_mb":384, "language":"en",
                "compact_seek_bar":false, "double_click_to_play":false,
                "system_titlebar":true, "volume_keys":true,
                "minimize_to_tray":true, "logging_enabled":true,
                "future_setting":{"keep":true}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            load_preferences_at(&legacy)
                .unwrap()
                .unwrap()
                .discord_rpc_enabled
        );
        for enabled in [false, true, false] {
            save_discord_rpc_at(&legacy, enabled).unwrap();
            let preferences = load_preferences_at(&legacy).unwrap().unwrap();
            assert_eq!(preferences.discord_rpc_enabled, enabled);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert!(!preferences.compact_seek_bar && !preferences.double_click_to_play);
            assert!(preferences.system_titlebar && preferences.volume_keys);
            assert!(preferences.minimize_to_tray && preferences.logging_enabled);
            assert_eq!(preferences.extra["future_setting"]["keep"], true);
        }
        save_at(&legacy, 0.625).unwrap();
        save_language_at(&legacy, Language::Russian).unwrap();
        save_cache_at(&legacy, 512).unwrap();
        migrate_at(&legacy, &destination).unwrap();
        let preferences = load_preferences_at(&destination).unwrap().unwrap();
        assert!(!preferences.discord_rpc_enabled && preferences.logging_enabled);
        assert_eq!(preferences.volume, 0.625);
        assert_eq!(preferences.cache_limit_mb, 512);
        assert_eq!(preferences.language, "ru");
        assert_eq!(preferences.extra["future_setting"]["keep"], true);
        assert!(!legacy.exists());
    }

    #[test]
    fn malformed_discord_rpc_values_recover_independently_and_single_flag_keeps_volume() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.375).unwrap();
        for malformed in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(0),
            serde_json::json!(1),
            serde_json::json!(1.5),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "volume":0.375, "language":"en", "logging_enabled":true,
                    "volume_keys":true, "minimize_to_tray":true,
                    "discord_rpc_enabled":malformed, "future_setting":"kept"
                }))
                .unwrap(),
            )
            .unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.discord_rpc_enabled);
            assert!(
                preferences.logging_enabled
                    && preferences.volume_keys
                    && preferences.minimize_to_tray
            );
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.language, "en");
            save_discord_rpc_at(&path, false).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(!preferences.discord_rpc_enabled && preferences.logging_enabled);
            assert_eq!(preferences.extra["future_setting"], "kept");
        }
        for enabled in [false, true] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({"discord_rpc_enabled":enabled})).unwrap(),
            )
            .unwrap();
            assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
            assert_eq!(
                load_preferences_at(&path)
                    .unwrap()
                    .unwrap()
                    .discord_rpc_enabled,
                enabled
            );
        }
    }

    #[test]
    fn fresh_profiles_enable_compact_seek_and_double_click_with_custom_titlebar() {
        let directory = TestDirectory::new();
        let missing = directory.0.join("config/preferences.json");
        assert!(load_preferences_at(&missing).unwrap().is_none());
        for preferences in [
            Preferences::default(),
            serde_json::from_value(serde_json::json!({})).unwrap(),
        ] {
            assert!(preferences.compact_seek_bar && preferences.double_click_to_play);
            assert!(!preferences.system_titlebar);
            assert!(!preferences.volume_keys && !preferences.minimize_to_tray);
            assert!(preferences.discord_rpc_enabled);
            assert_eq!(preferences.volume, DEFAULT_VOLUME);
        }
        assert!(
            !missing.exists(),
            "Reading defaults must not create settings"
        );
    }

    #[test]
    fn new_feature_toggles_preserve_explicit_old_choices_and_migrate_together() {
        let directory = TestDirectory::new();
        let legacy = directory.0.join("state/preferences.json");
        let destination = directory.0.join("config/preferences.json");
        save_at(&legacy, 0.375).unwrap();
        fs::write(
            &legacy,
            serde_json::to_vec(&serde_json::json!({
                "volume":0.375, "cache_limit_mb":384, "language":"en",
                "compact_seek_bar":false, "double_click_to_play":false,
                "system_titlebar":true, "future_setting":{"enabled":true}
            }))
            .unwrap(),
        )
        .unwrap();
        for (keys, tray) in [(true, false), (false, true), (true, true)] {
            update_at(&legacy, |preferences| preferences.volume_keys = keys).unwrap();
            save_minimize_to_tray_at(&legacy, tray).unwrap();
            let preferences = load_preferences_at(&legacy).unwrap().unwrap();
            assert_eq!(preferences.volume_keys, keys);
            assert_eq!(preferences.minimize_to_tray, tray);
            assert!(!preferences.compact_seek_bar && !preferences.double_click_to_play);
            assert!(preferences.system_titlebar);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        }
        save_at(&legacy, 0.625).unwrap();
        save_cache_at(&legacy, 512).unwrap();
        migrate_at(&legacy, &destination).unwrap();
        let preferences = load_preferences_at(&destination).unwrap().unwrap();
        assert!(preferences.volume_keys && preferences.minimize_to_tray);
        assert!(!preferences.compact_seek_bar && !preferences.double_click_to_play);
        assert!(preferences.system_titlebar);
        assert_eq!(preferences.volume, 0.625);
        assert_eq!(preferences.cache_limit_mb, 512);
        assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        assert!(!legacy.exists());
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn malformed_feature_toggles_recover_independently_and_single_flag_files_keep_volume() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.375).unwrap();
        for bad in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "volume":0.375, "cache_limit_mb":384, "language":"en",
                    "compact_seek_bar":"invalid", "double_click_to_play":[],
                    "system_titlebar":true, "volume_keys":bad,
                    "minimize_to_tray":bad, "future_setting":"kept"
                }))
                .unwrap(),
            )
            .unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.compact_seek_bar && preferences.double_click_to_play);
            assert!(preferences.system_titlebar);
            assert!(!preferences.volume_keys && !preferences.minimize_to_tray);
            assert_eq!(preferences.volume, 0.375);
            update_at(&path, |preferences| preferences.volume_keys = true).unwrap();
            save_minimize_to_tray_at(&path, true).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.volume_keys && preferences.minimize_to_tray);
            assert_eq!(preferences.extra["future_setting"], "kept");
        }
        // A file from before volume normalization was removed still counts.
        for key in ["normalize_volume", "minimize_to_tray"] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({key:true})).unwrap(),
            )
            .unwrap();
            assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
        }
    }

    #[test]
    fn volume_survives_reopening_and_atomic_replacement_privately() {
        let directory = TestDirectory::new();
        let path = directory.0.join("state/preferences.json");
        assert!(load_at(&path).unwrap().is_none());
        save_at(&path, 0.375).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(0.375));
        save_at(&path, 0.0).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(0.0));
        save_at(&path, 1.0).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(1.0));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
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
    }

    #[test]
    fn malformed_oversized_and_out_of_range_preferences_are_rejected() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.5).unwrap();
        for volume in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            assert!(save_at(&path, volume).is_err());
            assert_eq!(load_at(&path).unwrap(), Some(0.5));
        }
        for text in [
            "not JSON",
            "{}",
            r#"{"volume":null}"#,
            r#"{"volume":"0.5"}"#,
            r#"{"volume":-0.1}"#,
            r#"{"volume":1.1}"#,
            r#"{"volume":1e1000}"#,
        ] {
            fs::write(&path, text).unwrap();
            assert!(load_at(&path).is_err());
        }
        fs::write(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(load_at(&path).is_err());
    }

    #[test]
    fn cache_limit_migrates_old_volume_and_both_saves_preserve_other_field() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.25).unwrap();
        fs::write(&path, r#"{"volume":0.375}"#).unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert_eq!(preferences.volume, 0.375);
        assert_eq!(preferences.cache_limit_mb, DEFAULT_CACHE_MB);
        save_cache_at(&path, 256).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(0.375));
        save_at(&path, 0.625).unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert_eq!(preferences.volume, 0.625);
        assert_eq!(preferences.cache_limit_mb, 256);
        for limit in [0, MAX_CACHE_MB] {
            save_cache_at(&path, limit).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.cache_limit_mb, limit);
            assert_eq!(preferences.volume, 0.625);
        }
        assert!(save_cache_at(&path, MAX_CACHE_MB + 1).is_err());
        assert_eq!(
            load_preferences_at(&path).unwrap().unwrap().cache_limit_mb,
            MAX_CACHE_MB
        );
        fs::write(&path, r#"{"cache_limit_mb":128}"#).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
    }

    #[test]
    fn malformed_optional_fields_recover_independently() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.25).unwrap();
        for limit in [
            serde_json::json!(null),
            serde_json::json!("256"),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(5121),
            serde_json::json!(true),
            serde_json::json!({}),
        ] {
            let value = serde_json::json!({"volume":0.375,"cache_limit_mb":limit});
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, DEFAULT_CACHE_MB);
            assert_eq!(load_at(&path).unwrap(), Some(0.375));
            save_at(&path, 0.625).unwrap();
            assert_eq!(
                load_preferences_at(&path).unwrap().unwrap().cache_limit_mb,
                DEFAULT_CACHE_MB
            );
        }
        fs::write(&path, r#"{"volume":"invalid","cache_limit_mb":384}"#).unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert_eq!(preferences.volume, DEFAULT_VOLUME);
        assert_eq!(preferences.cache_limit_mb, 384);
        save_cache_at(&path, 512).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
        assert_eq!(
            load_preferences_at(&path).unwrap().unwrap().cache_limit_mb,
            512
        );
    }

    #[test]
    fn language_is_optional_recovers_independently_and_all_saves_preserve_fields() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.25).unwrap();
        for language in [
            serde_json::json!(null),
            serde_json::json!(12),
            serde_json::json!("fr"),
            serde_json::json!("EN"),
            serde_json::json!({}),
        ] {
            let value = serde_json::json!({
                "volume":0.375,"cache_limit_mb":384,"language":language,
                "future_setting":{"enabled":true}
            });
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert!(preferences.language.is_empty());
            save_language_at(&path, Language::English).unwrap();
            save_at(&path, 0.625).unwrap();
            save_cache_at(&path, 512).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.volume, 0.625);
            assert_eq!(preferences.cache_limit_mb, 512);
            assert_eq!(preferences.language, "en");
            assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        }
        fs::write(&path, r#"{"volume":0.5,"cache_limit_mb":256}"#).unwrap();
        assert!(
            load_preferences_at(&path)
                .unwrap()
                .unwrap()
                .language
                .is_empty()
        );
    }

    #[test]
    fn compact_seek_bar_is_optional_and_each_layout_survives_reopening() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        assert!(load_preferences_at(&path).unwrap().is_none());
        save_at(&path, 0.375).unwrap();
        fs::write(
            &path,
            r#"{"volume":0.375,"cache_limit_mb":384,"language":"en","future_setting":{"enabled":true}}"#,
        )
        .unwrap();
        assert!(
            load_preferences_at(&path)
                .unwrap()
                .unwrap()
                .compact_seek_bar
        );
        for compact in [true, false, true] {
            save_compact_seek_at(&path, compact).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.compact_seek_bar, compact);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        }
        save_at(&path, 0.625).unwrap();
        save_cache_at(&path, 512).unwrap();
        save_language_at(&path, Language::Russian).unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert!(preferences.compact_seek_bar);
        assert_eq!(preferences.volume, 0.625);
        assert_eq!(preferences.cache_limit_mb, 512);
        assert_eq!(preferences.language, "ru");
        assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_compact_seek_bar_recovers_without_discarding_valid_preferences() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.5).unwrap();
        for compact in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!(-1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "volume":0.375,"cache_limit_mb":384,"language":"en",
                    "compact_seek_bar":compact,"future_setting":"preserved"
                }))
                .unwrap(),
            )
            .unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.compact_seek_bar);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            save_compact_seek_at(&path, true).unwrap();
            assert!(
                load_preferences_at(&path)
                    .unwrap()
                    .unwrap()
                    .compact_seek_bar
            );
            assert_eq!(
                load_preferences_at(&path).unwrap().unwrap().extra["future_setting"],
                "preserved"
            );
        }
        fs::write(&path, r#"{"compact_seek_bar":true}"#).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
    }

    #[test]
    fn titlebar_choice_defaults_to_custom_and_survives_other_preference_updates() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        assert!(load_preferences_at(&path).unwrap().is_none());
        save_at(&path, 0.375).unwrap();
        fs::write(
            &path,
            r#"{"volume":0.375,"cache_limit_mb":384,"language":"en","compact_seek_bar":true,"future_setting":{"enabled":true}}"#,
        )
        .unwrap();
        assert!(!load_preferences_at(&path).unwrap().unwrap().system_titlebar);
        for system in [true, false, true] {
            save_system_titlebar_at(&path, system).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.system_titlebar, system);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert!(preferences.compact_seek_bar);
            assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        }
        save_at(&path, 0.625).unwrap();
        save_cache_at(&path, 512).unwrap();
        save_language_at(&path, Language::Russian).unwrap();
        save_compact_seek_at(&path, false).unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert!(preferences.system_titlebar);
        assert_eq!(preferences.volume, 0.625);
        assert_eq!(preferences.cache_limit_mb, 512);
        assert_eq!(preferences.language, "ru");
        assert!(!preferences.compact_seek_bar);
        assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_titlebar_choice_recovers_without_discarding_other_preferences() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.5).unwrap();
        for system in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!(-1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "volume":0.375,"cache_limit_mb":384,"language":"en",
                    "compact_seek_bar":true,"system_titlebar":system,
                    "future_setting":"preserved"
                }))
                .unwrap(),
            )
            .unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(!preferences.system_titlebar);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert!(preferences.compact_seek_bar);
            save_system_titlebar_at(&path, true).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.system_titlebar);
            assert_eq!(preferences.extra["future_setting"], "preserved");
        }
        fs::write(&path, r#"{"system_titlebar":true}"#).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
        assert!(load_preferences_at(&path).unwrap().unwrap().system_titlebar);
    }

    #[test]
    fn track_click_mode_defaults_to_double_and_persists_without_losing_other_settings() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        assert!(Preferences::default().double_click_to_play);
        save_at(&path, 0.375).unwrap();
        fs::write(
            &path,
            r#"{"volume":0.375,"cache_limit_mb":384,"language":"en","compact_seek_bar":true,"system_titlebar":true,"future_setting":{"enabled":true}}"#,
        )
        .unwrap();
        assert!(
            load_preferences_at(&path)
                .unwrap()
                .unwrap()
                .double_click_to_play
        );
        for double_click in [true, false, true] {
            save_double_click_at(&path, double_click).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert_eq!(preferences.double_click_to_play, double_click);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert!(preferences.compact_seek_bar && preferences.system_titlebar);
            assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        }
        save_at(&path, 0.625).unwrap();
        save_cache_at(&path, 512).unwrap();
        save_language_at(&path, Language::Russian).unwrap();
        save_compact_seek_at(&path, false).unwrap();
        save_system_titlebar_at(&path, false).unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert!(preferences.double_click_to_play);
        assert_eq!(preferences.volume, 0.625);
        assert_eq!(preferences.cache_limit_mb, 512);
        assert_eq!(preferences.language, "ru");
        assert!(!preferences.compact_seek_bar && !preferences.system_titlebar);
        assert_eq!(preferences.extra["future_setting"]["enabled"], true);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_track_click_mode_recovers_without_discarding_other_preferences() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.5).unwrap();
        for mode in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!(-1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "volume":0.375,"cache_limit_mb":384,"language":"en",
                    "compact_seek_bar":true,"system_titlebar":true,
                    "double_click_to_play":mode,"future_setting":"preserved"
                }))
                .unwrap(),
            )
            .unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.double_click_to_play);
            assert_eq!(preferences.volume, 0.375);
            assert_eq!(preferences.cache_limit_mb, 384);
            assert_eq!(preferences.language, "en");
            assert!(preferences.compact_seek_bar && preferences.system_titlebar);
            save_double_click_at(&path, true).unwrap();
            let preferences = load_preferences_at(&path).unwrap().unwrap();
            assert!(preferences.double_click_to_play);
            assert_eq!(preferences.extra["future_setting"], "preserved");
        }
        fs::write(&path, r#"{"double_click_to_play":true}"#).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(DEFAULT_VOLUME));
        assert!(
            load_preferences_at(&path)
                .unwrap()
                .unwrap()
                .double_click_to_play
        );
    }

    #[test]
    fn explicit_migration_moves_only_preferences_and_loaders_do_not_create_config() {
        let directory = TestDirectory::new();
        let legacy = directory.0.join("state/vk-fast/preferences.json");
        let config = directory.0.join("config/vk-fast/preferences.json");
        save_at(&legacy, 0.375).unwrap();
        save_cache_at(&legacy, 256).unwrap();
        save_language_at(&legacy, Language::English).unwrap();
        save_compact_seek_at(&legacy, true).unwrap();
        save_system_titlebar_at(&legacy, true).unwrap();
        save_double_click_at(&legacy, true).unwrap();
        update_at(&legacy, |preferences| preferences.volume_keys = true).unwrap();
        save_minimize_to_tray_at(&legacy, true).unwrap();
        let unrelated = legacy.parent().unwrap().join("unrelated.json");
        fs::write(&unrelated, b"synthetic unrelated state").unwrap();
        assert!(load_preferences_at(&config).unwrap().is_none());
        assert!(!config.exists());
        assert!(legacy.exists());
        migrate_at(&legacy, &config).unwrap();
        assert!(!legacy.exists());
        let preferences = load_preferences_at(&config).unwrap().unwrap();
        assert_eq!(preferences.volume, 0.375);
        assert_eq!(preferences.cache_limit_mb, 256);
        assert_eq!(preferences.language, "en");
        assert!(preferences.compact_seek_bar);
        assert!(preferences.system_titlebar);
        assert!(preferences.double_click_to_play);
        assert!(preferences.volume_keys && preferences.minimize_to_tray);
        assert_eq!(fs::read(&unrelated).unwrap(), b"synthetic unrelated state");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&config).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(config.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn migration_never_replaces_existing_config_or_removes_changed_legacy_data() {
        let directory = TestDirectory::new();
        let legacy = directory.0.join("state/preferences.json");
        let config = directory.0.join("config/preferences.json");
        save_at(&legacy, 0.25).unwrap();
        save_language_at(&legacy, Language::English).unwrap();
        save_at(&config, 0.75).unwrap();
        save_cache_at(&config, 512).unwrap();
        let original_legacy = fs::read(&legacy).unwrap();
        let original_config = fs::read(&config).unwrap();
        migrate_at(&legacy, &config).unwrap();
        assert_eq!(fs::read(&legacy).unwrap(), original_legacy);
        assert_eq!(fs::read(&config).unwrap(), original_config);
        assert!(!write_mode_at(&config, &Preferences::default(), false).unwrap());
        assert_eq!(fs::read(&config).unwrap(), original_config);
        save_at(&legacy, 0.875).unwrap();
        remove_legacy_if_unchanged(&legacy, &original_legacy).unwrap();
        assert_eq!(load_at(&legacy).unwrap(), Some(0.875));
    }

    #[test]
    fn invalid_legacy_file_is_preserved_without_creating_new_preferences() {
        let directory = TestDirectory::new();
        let legacy = directory.0.join("state/preferences.json");
        let config = directory.0.join("config/preferences.json");
        save_at(&legacy, 0.5).unwrap();
        fs::write(&legacy, b"malformed preferences").unwrap();
        assert!(migrate_at(&legacy, &config).is_err());
        assert!(!config.exists());
        assert_eq!(fs::read(&legacy).unwrap(), b"malformed preferences");
        // The old format had no language field; malformed optional language
        // must not discard otherwise valid volume and cache settings either.
        fs::write(
            &legacy,
            r#"{"volume":0.25,"cache_limit_mb":384,"language":"unsupported"}"#,
        )
        .unwrap();
        migrate_at(&legacy, &config).unwrap();
        let preferences = load_preferences_at(&config).unwrap().unwrap();
        assert_eq!(preferences.volume, 0.25);
        assert_eq!(preferences.cache_limit_mb, 384);
        assert!(preferences.language.is_empty());
        assert!(!legacy.exists());
    }

    #[test]
    fn concurrent_volume_cache_and_language_updates_preserve_each_other() {
        let directory = TestDirectory::new();
        let path = directory.0.join("preferences.json");
        save_at(&path, 0.5).unwrap();
        let volume_path = path.clone();
        let volume = std::thread::spawn(move || {
            for value in [0.1, 0.2, 0.375] {
                save_at(&volume_path, value).unwrap();
            }
        });
        let cache_path = path.clone();
        let cache = std::thread::spawn(move || {
            for limit in [128, 256, 384] {
                save_cache_at(&cache_path, limit).unwrap();
            }
        });
        let language_path = path.clone();
        let language = std::thread::spawn(move || {
            for language in [Language::English, Language::Russian, Language::English] {
                save_language_at(&language_path, language).unwrap();
            }
        });
        let compact_path = path.clone();
        let compact = std::thread::spawn(move || {
            for compact in [true, false, true] {
                save_compact_seek_at(&compact_path, compact).unwrap();
            }
        });
        let titlebar_path = path.clone();
        let titlebar = std::thread::spawn(move || {
            for system in [true, false, true] {
                save_system_titlebar_at(&titlebar_path, system).unwrap();
            }
        });
        let click_path = path.clone();
        let click = std::thread::spawn(move || {
            for double_click in [true, false, true] {
                save_double_click_at(&click_path, double_click).unwrap();
            }
        });
        let keys_path = path.clone();
        let keys = std::thread::spawn(move || {
            for enabled in [true, false, true] {
                update_at(&keys_path, |preferences| preferences.volume_keys = enabled).unwrap();
            }
        });
        let tray_path = path.clone();
        let tray = std::thread::spawn(move || {
            for enabled in [true, false, true] {
                save_minimize_to_tray_at(&tray_path, enabled).unwrap();
            }
        });
        let discord_path = path.clone();
        let discord = std::thread::spawn(move || {
            for enabled in [false, true, false] {
                save_discord_rpc_at(&discord_path, enabled).unwrap();
            }
        });
        volume.join().unwrap();
        cache.join().unwrap();
        language.join().unwrap();
        compact.join().unwrap();
        titlebar.join().unwrap();
        click.join().unwrap();
        keys.join().unwrap();
        tray.join().unwrap();
        discord.join().unwrap();
        let preferences = load_preferences_at(&path).unwrap().unwrap();
        assert_eq!(preferences.volume, 0.375);
        assert_eq!(preferences.cache_limit_mb, 384);
        assert_eq!(preferences.language, "en");
        assert!(preferences.compact_seek_bar);
        assert!(preferences.system_titlebar);
        assert!(preferences.double_click_to_play);
        assert!(preferences.volume_keys && preferences.minimize_to_tray);
        assert!(!preferences.discord_rpc_enabled);
    }

    #[cfg(unix)]
    #[test]
    fn preference_symlinks_and_unsafe_permissions_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = TestDirectory::new();
        let target = directory.0.join("real.json");
        let alias = directory.0.join("preferences.json");
        save_at(&target, 0.5).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_at(&target).is_err());
        symlink(&target, &alias).unwrap();
        assert!(load_at(&alias).is_err());
        assert!(save_at(&alias, 0.5).is_err());
        let real_parent = directory.0.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let linked_parent = directory.0.join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        assert!(save_at(&linked_parent.join("preferences.json"), 0.5).is_err());
    }
}
