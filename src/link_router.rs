//! "Open VK Music links in VK Fast": the application registers itself as the
//! system's link handler. Links to VK Music tracks, releases, playlists and
//! artists open in VK Fast; every other link goes on to the browser the user
//! had before, without opening a window.
use std::io;

use reqwest::Url;

use crate::model::{AlbumRef, ArtistRef};

/// VK Fast's own link scheme: `vkfast://vk.ru/music/album/...` or
/// `vkfast://open?url=<link>`.
pub const SCHEME: &str = "vkfast";
pub const MAX_URL: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A track page; the application opens its album or single.
    Track((i64, i64)),
    Album(AlbumRef),
    Playlist(AlbumRef),
    Artist(ArtistRef),
}

/// Recognise a VK Music link. Anything else, including other VK pages, is
/// `None` and belongs to the browser.
pub fn parse(text: &str) -> Option<Target> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_URL {
        return None;
    }
    let url = Url::parse(text).ok()?;
    let url = match url.scheme() {
        "https" | "http" => url,
        SCHEME if url.host_str() == Some("open") => {
            let inner = url
                .query_pairs()
                .find(|(key, _)| key == "url")?
                .1
                .into_owned();
            let inner = Url::parse(&inner).ok()?;
            if !matches!(inner.scheme(), "https" | "http") {
                return None;
            }
            inner
        }
        SCHEME => Url::parse(&format!("https://{}", text.get(SCHEME.len() + 3..)?)).ok()?,
        _ => return None,
    };
    let host = url.host_str()?.to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "vk.ru" | "vk.com" | "www.vk.ru" | "www.vk.com" | "m.vk.ru" | "m.vk.com"
    ) {
        return None;
    }
    // A release or track opened over another page: `?z=audio_playlist…`.
    for (key, value) in url.query_pairs() {
        if key == "z" || key == "act" {
            if let Some(target) = value
                .strip_prefix("audio_playlist")
                .and_then(|id| release(id, false))
            {
                return Some(target);
            }
            if let Some(key) = value.strip_prefix("audio").and_then(track_key) {
                return Some(Target::Track(key));
            }
        }
    }
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        ["music", "album", id] => release(id, true),
        ["music", "playlist", id] => release(id, false),
        ["artist", id, ..] if crate::model::valid_artist_id(id) => {
            Some(Target::Artist(ArtistRef {
                id: (*id).to_owned(),
                name: (*id).to_owned(),
            }))
        }
        [single] => {
            if let Some(id) = single.strip_prefix("audio_playlist") {
                release(id, false)
            } else {
                single
                    .strip_prefix("audio")
                    .and_then(track_key)
                    .map(Target::Track)
            }
        }
        _ => None,
    }
}

fn number(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || digits.len() > 19 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn access_key(text: &str) -> Option<String> {
    (!text.is_empty() && text.len() <= 64 && text.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| text.to_owned())
}

/// `<owner>_<id>` with an optional `_<hash>` VK appends to shared tracks.
fn track_key(text: &str) -> Option<(i64, i64)> {
    let mut parts = text.split('_');
    let owner = number(parts.next()?).filter(|owner| *owner != 0)?;
    let id = number(parts.next()?).filter(|id| *id > 0)?;
    match (parts.next(), parts.next()) {
        (None, _) => Some((owner, id)),
        (Some(hash), None) if access_key(hash).is_some() => Some((owner, id)),
        _ => None,
    }
}

/// `<owner>_<id>` with an optional access key after `_` or `/`.
fn release(text: &str, album: bool) -> Option<Target> {
    let text = text.replace('/', "_");
    let mut parts = text.split('_');
    let owner = number(parts.next()?).filter(|owner| *owner != 0)?;
    let id = number(parts.next()?).filter(|id| *id > 0)?;
    let key = match (parts.next(), parts.next()) {
        (None, _) => None,
        (Some(key), None) => Some(access_key(key)?),
        _ => return None,
    };
    let reference = AlbumRef {
        id,
        owner_id: owner,
        title: String::new(),
        access_key: key,
        cover_url: None,
    };
    Some(if album {
        Target::Album(reference)
    } else {
        Target::Playlist(reference)
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouterError {
    /// No other browser is known to receive the remaining links.
    NoBrowser,
    /// The executable's path cannot be written into a link handler entry.
    UnsupportedPath,
    System,
}

impl From<io::Error> for RouterError {
    fn from(_: io::Error) -> Self {
        Self::System
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// VK Fast is registered as the link handler.
    pub enabled: bool,
    /// The system currently sends web links to VK Fast. On Windows the user
    /// confirms this in the system settings, so it can lag behind `enabled`.
    pub active: bool,
    /// The browser that receives every other link.
    pub browser: Option<String>,
}

/// A handler id saved as the previous browser: a desktop entry id on Linux,
/// a ProgId on Windows. Never a path or a command line.
pub fn valid_handler(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
}

#[cfg(target_os = "linux")]
pub use linux::{disable, enable, forward, refresh_registration, status};
#[cfg(windows)]
pub use windows::{disable, enable, forward, refresh_registration, status};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    pub const DESKTOP_ID: &str = "vk-fast-links.desktop";
    const SCHEMES: [&str; 3] = [
        "x-scheme-handler/http",
        "x-scheme-handler/https",
        "x-scheme-handler/vkfast",
    ];

    fn absolute(variable: &str) -> Option<PathBuf> {
        std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    fn data_home() -> io::Result<PathBuf> {
        absolute("XDG_DATA_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".local/share")))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn application_dirs() -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = data_home().into_iter().collect();
        let system = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(
            system
                .split(':')
                .map(PathBuf::from)
                .filter(|path| path.is_absolute()),
        );
        dirs.into_iter()
            .map(|dir| dir.join("applications"))
            .collect()
    }

    /// The launcher for this copy of the application. An AppImage is
    /// started through its image, not the temporary mount.
    fn executable() -> io::Result<PathBuf> {
        absolute("APPIMAGE")
            .filter(|path| path.is_file())
            .map_or_else(std::env::current_exe, Ok)
    }

    /// The desktop entry that receives web links. `Exec` quotes the path;
    /// paths needing desktop-entry escapes are refused rather than escaped.
    pub(super) fn desktop_entry(executable: &Path) -> Result<String, RouterError> {
        let path = executable.to_str().ok_or(RouterError::UnsupportedPath)?;
        if !executable.is_absolute()
            || path
                .chars()
                .any(|c| c.is_control() || matches!(c, '"' | '`' | '$' | '\\' | '%'))
        {
            return Err(RouterError::UnsupportedPath);
        }
        // xdg-mime cannot resolve a quoted program, so quote only when the
        // path needs it.
        let program = if path.contains(char::is_whitespace) {
            format!("\"{path}\"")
        } else {
            path.to_owned()
        };
        Ok(format!(
            "[Desktop Entry]\nType=Application\nName=VK Fast\nComment=Opens VK Music links in VK Fast\nComment[ru]=Открывает ссылки VK Музыки в VK Fast\nExec={program} --open-url %u\nIcon={}\nTerminal=false\nNoDisplay=true\nMimeType={};\n",
            crate::app_icon::APP_ID,
            SCHEMES.join(";"),
        ))
    }

    /// The first `[Default Applications]` entry for `scheme` in the user's
    /// and system `mimeapps.list` files, desktop-specific ones first.
    pub(super) fn listed_default(
        dirs: &[PathBuf],
        desktops: &[String],
        scheme: &str,
    ) -> Option<String> {
        for dir in dirs {
            for prefix in desktops
                .iter()
                .map(|desktop| format!("{}-", desktop.to_lowercase()))
                .chain([String::new()])
            {
                let Ok(text) = fs::read_to_string(dir.join(format!("{prefix}mimeapps.list")))
                else {
                    continue;
                };
                let mut in_defaults = false;
                for line in text.lines() {
                    let line = line.trim();
                    if line.starts_with('[') {
                        in_defaults = line == "[Default Applications]";
                    } else if in_defaults
                        && let Some((key, value)) = line.split_once('=')
                        && key.trim() == scheme
                    {
                        if let Some(id) =
                            value.split(';').map(str::trim).find(|id| valid_handler(id))
                        {
                            return Some(id.to_owned());
                        }
                        break;
                    }
                }
            }
        }
        None
    }

    fn config_dirs() -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = absolute("XDG_CONFIG_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".config")))
            .into_iter()
            .collect();
        let system = std::env::var("XDG_CONFIG_DIRS")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/etc/xdg".into());
        dirs.extend(
            system
                .split(':')
                .map(PathBuf::from)
                .filter(|path| path.is_absolute()),
        );
        dirs
    }

    fn query_default(scheme: &str) -> Option<String> {
        let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter(|desktop| !desktop.is_empty())
            .map(str::to_owned)
            .collect();
        if let Some(id) = listed_default(&config_dirs(), &desktops, scheme) {
            return Some(id);
        }
        let output = Command::new("xdg-mime")
            .args(["query", "default", scheme])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let id = String::from_utf8(output.stdout).ok()?.trim().to_owned();
        (output.status.success() && valid_handler(&id)).then_some(id)
    }

    fn set_default(id: &str, schemes: &[&str]) -> Result<(), RouterError> {
        let status = Command::new("xdg-mime")
            .arg("default")
            .arg(id)
            .args(schemes)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(RouterError::System)
        }
    }

    pub(super) fn find_desktop(dirs: &[PathBuf], id: &str) -> Option<PathBuf> {
        if !valid_handler(id) || !id.ends_with(".desktop") {
            return None;
        }
        dirs.iter().find_map(|dir| {
            let direct = dir.join(id);
            if direct.is_file() {
                return Some(direct);
            }
            // `org-app.desktop` may live at `org/app.desktop`.
            let nested = dir.join(id.replace('-', "/"));
            nested.is_file().then_some(nested)
        })
    }

    /// The `[Desktop Entry]` group's value for `key`, unescaped.
    pub(super) fn entry_value(text: &str, key: &str) -> Option<String> {
        let mut in_entry = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some((name, value)) = line.split_once('=')
                && name.trim() == key
            {
                let mut result = String::new();
                let mut chars = value.trim().chars();
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        match chars.next() {
                            Some('s') => result.push(' '),
                            Some('n') => result.push('\n'),
                            Some('t') => result.push('\t'),
                            Some('r') => result.push('\r'),
                            Some(other) => result.push(other),
                            None => result.push('\\'),
                        }
                    } else {
                        result.push(c);
                    }
                }
                return Some(result);
            }
        }
        None
    }

    /// Split an `Exec` value and put `url` where its field code asks for a
    /// URL or file; append it when the entry has none.
    pub(super) fn exec_command(exec: &str, url: &str) -> Option<Vec<String>> {
        let mut args = Vec::new();
        let mut current = String::new();
        let mut quoted = false;
        let mut started = false;
        let mut chars = exec.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    quoted = !quoted;
                    started = true;
                }
                '\\' if quoted => current.push(chars.next()?),
                c if c.is_whitespace() && !quoted => {
                    if started {
                        args.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            }
        }
        if quoted {
            return None;
        }
        if started {
            args.push(current);
        }
        let mut placed = false;
        let mut result = Vec::with_capacity(args.len() + 1);
        for arg in args {
            match arg.as_str() {
                "%u" | "%U" | "%f" | "%F" => {
                    if !placed {
                        result.push(url.to_owned());
                        placed = true;
                    }
                }
                "%i" | "%c" | "%k" => {}
                _ => {
                    let mut text = String::new();
                    let mut chars = arg.chars();
                    while let Some(c) = chars.next() {
                        if c == '%' {
                            match chars.next() {
                                Some('%') => text.push('%'),
                                Some('u' | 'U' | 'f' | 'F') if !placed => {
                                    text.push_str(url);
                                    placed = true;
                                }
                                _ => {}
                            }
                        } else {
                            text.push(c);
                        }
                    }
                    result.push(text);
                }
            }
        }
        if result.is_empty() {
            return None;
        }
        if !placed {
            result.push(url.to_owned());
        }
        Some(result)
    }

    fn browser_name(id: &str) -> Option<String> {
        let path = find_desktop(&application_dirs(), id)?;
        let text = fs::read_to_string(path).ok()?;
        entry_value(&text, "Name").filter(|name| !name.trim().is_empty())
    }

    fn write_entry(directory: &Path) -> Result<(), RouterError> {
        let entry = desktop_entry(&executable()?)?;
        fs::create_dir_all(directory)?;
        let path = directory.join(DESKTOP_ID);
        if fs::read_to_string(&path).ok().as_deref() == Some(entry.as_str()) {
            return Ok(());
        }
        let temporary = directory.join(format!(".{DESKTOP_ID}.tmp"));
        fs::write(&temporary, entry)?;
        fs::rename(&temporary, &path)?;
        // Optional cache refresh; the default above works without it.
        let _ = Command::new("update-desktop-database")
            .arg(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(())
    }

    pub fn status() -> Status {
        let current = query_default("x-scheme-handler/https");
        let enabled = current.as_deref() == Some(DESKTOP_ID)
            && data_home().is_ok_and(|home| home.join("applications").join(DESKTOP_ID).is_file());
        let browser = if enabled {
            crate::preferences::load_link_browser()
        } else {
            current.filter(|id| id != DESKTOP_ID)
        };
        Status {
            enabled,
            active: enabled,
            browser: browser.map(|id| browser_name(&id).unwrap_or(id)),
        }
    }

    pub fn enable() -> Result<(), RouterError> {
        let current = query_default("x-scheme-handler/https").filter(|id| id != DESKTOP_ID);
        let previous = current
            .or_else(crate::preferences::load_link_browser)
            .filter(|id| find_desktop(&application_dirs(), id).is_some())
            .ok_or(RouterError::NoBrowser)?;
        crate::preferences::save_link_browser(&previous)?;
        write_entry(&data_home()?.join("applications"))?;
        set_default(DESKTOP_ID, &SCHEMES)
    }

    pub fn disable() -> Result<(), RouterError> {
        let previous = crate::preferences::load_link_browser().ok_or(RouterError::NoBrowser)?;
        set_default(&previous, &SCHEMES[..2])?;
        match fs::remove_file(data_home()?.join("applications").join(DESKTOP_ID)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Keep the handler entry pointing at this copy after it moved, e.g. a
    /// new AppImage. Does nothing unless the user enabled the handler.
    pub fn refresh_registration() {
        if let Ok(home) = data_home() {
            let directory = home.join("applications");
            if directory.join(DESKTOP_ID).is_file() {
                let _ = write_entry(&directory);
            }
        }
    }

    pub fn forward(url: &str) -> Result<(), RouterError> {
        let id = crate::preferences::load_link_browser().ok_or(RouterError::NoBrowser)?;
        if id == DESKTOP_ID {
            return Err(RouterError::NoBrowser);
        }
        let path = find_desktop(&application_dirs(), &id).ok_or(RouterError::NoBrowser)?;
        let text = fs::read_to_string(path)?;
        let exec = entry_value(&text, "Exec").ok_or(RouterError::NoBrowser)?;
        let command = exec_command(&exec, url).ok_or(RouterError::NoBrowser)?;
        use std::os::unix::process::CommandExt;
        Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::process::{Command, Stdio};
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    const PROG_ID: &str = "VKFast.Link";
    const APP_NAME: &str = "VK Fast";
    const CAPABILITIES: &str = "Software\\VK Fast\\Capabilities";
    const REGISTERED: &str = "Software\\RegisteredApplications";

    fn user_choice() -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(
                "Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice",
            )
            .ok()?
            .get_value::<String, _>("ProgId")
            .ok()
            .filter(|id| valid_handler(id))
    }

    fn command(executable: &str) -> String {
        format!("\"{executable}\" --open-url \"%1\"")
    }

    fn write_class(path: &str, name: &str, executable: &str, protocol: bool) -> io::Result<()> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let (class, _) = user.create_subkey(path)?;
        class.set_value("", &name.to_owned())?;
        if protocol {
            class.set_value("URL Protocol", &String::new())?;
        }
        let (icon, _) = class.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{executable}\",0"))?;
        let (open, _) = class.create_subkey("shell\\open\\command")?;
        open.set_value("", &command(executable))
    }

    fn register() -> Result<(), RouterError> {
        let executable = std::env::current_exe()?;
        let executable = executable.to_str().ok_or(RouterError::UnsupportedPath)?;
        if executable.contains('"') {
            return Err(RouterError::UnsupportedPath);
        }
        write_class(
            &format!("Software\\Classes\\{PROG_ID}"),
            "VK Fast Link",
            executable,
            false,
        )?;
        write_class(
            &format!("Software\\Classes\\{SCHEME}"),
            "URL:VK Fast",
            executable,
            true,
        )?;
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let (capabilities, _) = user.create_subkey(CAPABILITIES)?;
        capabilities.set_value("ApplicationName", &APP_NAME.to_owned())?;
        capabilities.set_value(
            "ApplicationDescription",
            &"Opens VK Music links in VK Fast".to_owned(),
        )?;
        let (urls, _) = capabilities.create_subkey("URLAssociations")?;
        for scheme in ["http", "https", SCHEME] {
            urls.set_value(scheme, &PROG_ID.to_owned())?;
        }
        let (registered, _) = user.create_subkey(REGISTERED)?;
        registered.set_value(APP_NAME, &CAPABILITIES.to_owned())?;
        Ok(())
    }

    fn registered() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(REGISTERED)
            .and_then(|key| key.get_value::<String, _>(APP_NAME))
            .is_ok()
    }

    fn open_default_apps() {
        let _ = open::that("ms-settings:defaultapps?registeredAppUser=VK%20Fast");
    }

    fn class_command(id: &str) -> Option<String> {
        let path = format!("Software\\Classes\\{id}\\shell\\open\\command");
        [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
            .into_iter()
            .find_map(|root| {
                RegKey::predef(root)
                    .open_subkey(&path)
                    .ok()?
                    .get_value::<String, _>("")
                    .ok()
            })
            .filter(|command| !command.trim().is_empty())
    }

    fn browser_name(id: &str) -> Option<String> {
        let path = format!("Software\\Classes\\{id}\\Application");
        [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
            .into_iter()
            .find_map(|root| {
                RegKey::predef(root)
                    .open_subkey(&path)
                    .ok()?
                    .get_value::<String, _>("ApplicationName")
                    .ok()
            })
            .filter(|name| !name.trim().is_empty() && !name.starts_with('@'))
    }

    pub fn status() -> Status {
        let enabled = registered();
        let current = user_choice();
        let active = enabled && current.as_deref() == Some(PROG_ID);
        let browser = crate::preferences::load_link_browser()
            .filter(|_| enabled)
            .or_else(|| current.filter(|id| id != PROG_ID));
        Status {
            enabled,
            active,
            browser: browser.map(|id| browser_name(&id).unwrap_or(id)),
        }
    }

    pub fn enable() -> Result<(), RouterError> {
        let previous = user_choice()
            .filter(|id| id != PROG_ID)
            .or_else(crate::preferences::load_link_browser)
            .filter(|id| class_command(id).is_some())
            .ok_or(RouterError::NoBrowser)?;
        crate::preferences::save_link_browser(&previous)?;
        register()?;
        // Windows lets only the user change the default link handler.
        open_default_apps();
        Ok(())
    }

    pub fn disable() -> Result<(), RouterError> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = user.open_subkey_with_flags(REGISTERED, winreg::enums::KEY_SET_VALUE) {
            let _ = key.delete_value(APP_NAME);
        }
        for path in [
            CAPABILITIES.to_owned(),
            format!("Software\\Classes\\{PROG_ID}"),
            format!("Software\\Classes\\{SCHEME}"),
        ] {
            match user.delete_subkey_all(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if user_choice().as_deref() == Some(PROG_ID) {
            open_default_apps();
        }
        Ok(())
    }

    /// Keep the registered command pointing at this copy after it moved.
    pub fn refresh_registration() {
        if registered() {
            let _ = register();
        }
    }

    pub fn forward(url: &str) -> Result<(), RouterError> {
        use std::os::windows::process::CommandExt;
        let id = crate::preferences::load_link_browser()
            .filter(|id| id != PROG_ID)
            .ok_or(RouterError::NoBrowser)?;
        let line = class_command(&id).ok_or(RouterError::NoBrowser)?;
        let line = line.trim();
        let (program, rest) = if let Some(quoted) = line.strip_prefix('"') {
            let end = quoted.find('"').ok_or(RouterError::NoBrowser)?;
            (&quoted[..end], &quoted[end + 1..])
        } else {
            line.split_once(' ').unwrap_or((line, ""))
        };
        // A parsed URL never contains a raw quote, so it stays one argument.
        let url = Url::parse(url).map_err(|_| RouterError::System)?;
        let url = url.as_str();
        let arguments = if rest.contains("%1") || rest.contains("%L") || rest.contains("%l") {
            rest.replace("%1", url)
                .replace("%L", url)
                .replace("%l", url)
        } else {
            format!("{rest} \"{url}\"")
        };
        Command::new(program)
            .raw_arg(arguments.trim())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(owner: i64, id: i64, key: Option<&str>) -> AlbumRef {
        AlbumRef {
            id,
            owner_id: owner,
            title: String::new(),
            access_key: key.map(str::to_owned),
            cover_url: None,
        }
    }

    #[test]
    fn vk_music_links_are_recognised_and_everything_else_goes_to_the_browser() {
        for (link, expected) in [
            ("https://vk.ru/audio-2001_123", Target::Track((-2001, 123))),
            ("https://vk.com/audio42_7_0a1b", Target::Track((42, 7))),
            (
                "https://vk.ru/music/album/-2000123_456_abc123",
                Target::Album(album(-2000123, 456, Some("abc123"))),
            ),
            (
                "https://m.vk.com/music/playlist/12_34",
                Target::Playlist(album(12, 34, None)),
            ),
            (
                "https://vk.ru/audios42?z=audio_playlist-2000_77%2Fdeadbeef",
                Target::Playlist(album(-2000, 77, Some("deadbeef"))),
            ),
            (
                "https://vk.ru/audio_playlist5_6_ff",
                Target::Playlist(album(5, 6, Some("ff"))),
            ),
            (
                "https://www.vk.ru/artist/synthetic_artist",
                Target::Artist(ArtistRef {
                    id: "synthetic_artist".into(),
                    name: "synthetic_artist".into(),
                }),
            ),
            (
                "vkfast://vk.ru/music/album/-1_2",
                Target::Album(album(-1, 2, None)),
            ),
            (
                "vkfast://open?url=https%3A%2F%2Fvk.ru%2Faudio1_2",
                Target::Track((1, 2)),
            ),
        ] {
            assert_eq!(parse(link), Some(expected), "{link}");
        }
        for link in [
            "https://vk.ru/",
            "https://vk.ru/id1",
            "https://vk.ru/audios42",
            "https://vk.ru/audio0_5",
            "https://vk.ru/audio1_0",
            "https://vk.ru/audio1_2_bad/key",
            "https://vk.ru/music/album/1_2_a_b",
            "https://vk.ru/artist/..",
            "https://vk.ru.evil.test/audio1_2",
            "https://evil.test/?u=https://vk.ru/audio1_2",
            "ftp://vk.ru/audio1_2",
            "vkfast://open?url=javascript:alert(1)",
            "https://example.com",
        ] {
            assert_eq!(parse(link), None, "{link}");
        }
        assert_eq!(
            parse(&format!("https://vk.ru/audio1_2?{}", "a".repeat(MAX_URL))),
            None
        );
    }

    #[test]
    fn handler_ids_are_plain_names() {
        assert!(valid_handler("helium.desktop"));
        assert!(valid_handler("FirefoxURL-308046B0AF4A39CB"));
        assert!(!valid_handler(""));
        assert!(!valid_handler("../evil.desktop"));
        assert!(!valid_handler("a b"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_entries_quote_the_path_and_browsers_get_the_link_in_place_of_their_field_code() {
        use super::linux::*;
        use std::path::Path;
        let entry = desktop_entry(Path::new("/home/user/Apps/VK Fast.AppImage")).unwrap();
        assert!(entry.contains("Exec=\"/home/user/Apps/VK Fast.AppImage\" --open-url %u\n"));
        let plain = desktop_entry(Path::new("/home/user/bin/vk-fast")).unwrap();
        assert!(plain.contains("Exec=/home/user/bin/vk-fast --open-url %u\n"));
        assert!(entry.contains(
            "MimeType=x-scheme-handler/http;x-scheme-handler/https;x-scheme-handler/vkfast;"
        ));
        assert!(entry.contains("NoDisplay=true"));
        for bad in [
            "relative/vk-fast",
            "/a/$HOME/vk",
            "/a/\"q\"/vk",
            "/a/b%u",
            "/a\nb",
        ] {
            assert_eq!(
                desktop_entry(Path::new(bad)),
                Err(RouterError::UnsupportedPath),
                "{bad}"
            );
        }
        let url = "https://example.com/?a=1&b=2";
        assert_eq!(
            exec_command("/usr/bin/helium %U", url).unwrap(),
            ["/usr/bin/helium", url]
        );
        assert_eq!(
            exec_command(
                "\"/opt/My Browser/browser\" --new-window %u --icon %i %c",
                url
            )
            .unwrap(),
            ["/opt/My Browser/browser", "--new-window", url, "--icon"]
        );
        assert_eq!(
            exec_command("flatpak run org.example.Browser", url).unwrap(),
            ["flatpak", "run", "org.example.Browser", url]
        );
        assert_eq!(
            exec_command("browser --url=%u 100%%", url).unwrap(),
            ["browser", &format!("--url={url}"), "100%"]
        );
        assert!(exec_command("\"unterminated", url).is_none());
        let text = "[Desktop Entry]\nName=Helium\nName[ru]=Гелий\nExec=helium\\s--x %U\n[Desktop Action new]\nExec=other\n";
        assert_eq!(entry_value(text, "Name").as_deref(), Some("Helium"));
        assert_eq!(entry_value(text, "Exec").as_deref(), Some("helium --x %U"));

        let root = tempfile::tempdir().unwrap();
        let dirs = [root.path().join("a"), root.path().join("b")];
        std::fs::create_dir_all(dirs[1].join("org")).unwrap();
        std::fs::write(dirs[1].join("org/example.desktop"), text).unwrap();
        assert_eq!(
            find_desktop(&dirs, "org-example.desktop"),
            Some(dirs[1].join("org/example.desktop"))
        );
        assert_eq!(find_desktop(&dirs, "../org/example.desktop"), None);
        assert_eq!(find_desktop(&dirs, "missing.desktop"), None);

        // The user's desktop-specific list wins, then the plain one; other
        // sections and other schemes are ignored.
        let config = [root.path().join("config"), root.path().join("xdg")];
        std::fs::create_dir_all(&config[0]).unwrap();
        std::fs::create_dir_all(&config[1]).unwrap();
        std::fs::write(
            config[0].join("mimeapps.list"),
            "[Added Associations]\nx-scheme-handler/https=other.desktop;\n[Default Applications]\nx-scheme-handler/http=b.desktop\nx-scheme-handler/https=vk-fast-links.desktop;helium.desktop;\n",
        )
        .unwrap();
        std::fs::write(
            config[1].join("mimeapps.list"),
            "[Default Applications]\nx-scheme-handler/https=system.desktop\n",
        )
        .unwrap();
        let none: [String; 0] = [];
        assert_eq!(
            listed_default(&config, &none, "x-scheme-handler/https").as_deref(),
            Some("vk-fast-links.desktop")
        );
        std::fs::write(
            config[0].join("niri-mimeapps.list"),
            "[Default Applications]\nx-scheme-handler/https=niri.desktop\n",
        )
        .unwrap();
        assert_eq!(
            listed_default(&config, &["niri".to_owned()], "x-scheme-handler/https").as_deref(),
            Some("niri.desktop")
        );
        assert_eq!(
            listed_default(&config, &none, "x-scheme-handler/vkfast"),
            None
        );
    }
}
