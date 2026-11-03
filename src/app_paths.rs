//! Windows data roots selected by the installer. A missing marker preserves
//! the portable ZIP layout; the working directory never affects data paths.

#[cfg(any(windows, test))]
use std::{
    fs, io,
    io::Read,
    path::{Path, PathBuf},
};

#[cfg(any(windows, test))]
const STORAGE_FILE: &str = "vk-fast-storage.ini";
#[cfg(any(windows, test))]
const MAX_STORAGE_BYTES: u64 = 1024;

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Location {
    #[default]
    Portable,
    AppData,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Selection {
    config: Location,
    cache: Location,
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct DataPaths {
    config: PathBuf,
    cache: PathBuf,
}

#[cfg(any(windows, test))]
fn invalid_storage() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid VK Fast storage selection",
    )
}

#[cfg(any(windows, test))]
fn parse_selection(bytes: &[u8]) -> io::Result<Selection> {
    if bytes.len() as u64 > MAX_STORAGE_BYTES {
        return Err(invalid_storage());
    }
    // WritePrivateProfileString normally writes ASCII here; also accept an INI
    // edited by Windows tools that preserve a UTF-16LE or UTF-8 BOM.
    let text = if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let (pairs, remainder) = bytes.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(invalid_storage());
        }
        let words: Vec<_> = pairs.iter().copied().map(u16::from_le_bytes).collect();
        String::from_utf16(&words).map_err(|_| invalid_storage())?
    } else {
        String::from_utf8(
            bytes
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(bytes)
                .to_vec(),
        )
        .map_err(|_| invalid_storage())?
    };
    let (mut section, mut version, mut config, mut cache) = (false, None, None, None);
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with([';', '#']))
    {
        if line.eq_ignore_ascii_case("[storage]") && !section {
            section = true;
            continue;
        }
        if !section {
            return Err(invalid_storage());
        }
        let (key, value) = line.split_once('=').ok_or_else(invalid_storage)?;
        let location = |value: &str| match value {
            "portable" => Ok(Location::Portable),
            "appdata" => Ok(Location::AppData),
            _ => Err(invalid_storage()),
        };
        let value = value.trim().to_ascii_lowercase();
        match key.trim().to_ascii_lowercase().as_str() {
            "version" if version.is_none() && value == "1" => version = Some(1),
            "config" if config.is_none() => config = Some(location(&value)?),
            "cache" if cache.is_none() => cache = Some(location(&value)?),
            _ => return Err(invalid_storage()),
        }
    }
    if version != Some(1) {
        return Err(invalid_storage());
    }
    Ok(Selection {
        config: config.ok_or_else(invalid_storage)?,
        cache: cache.ok_or_else(invalid_storage)?,
    })
}

#[cfg(any(windows, test))]
fn read_selection(directory: &Path) -> io::Result<Selection> {
    let path = directory.join(STORAGE_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Selection::default()),
        Err(error) => return Err(error),
        Ok(metadata)
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_STORAGE_BYTES =>
        {
            return Err(invalid_storage());
        }
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_STORAGE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    parse_selection(&bytes)
}

#[cfg(any(windows, test))]
fn resolve(
    directory: &Path,
    selection: Selection,
    local_app_data: Option<&Path>,
) -> io::Result<DataPaths> {
    if !directory.is_absolute() {
        return Err(invalid_storage());
    }
    let base = |location| match location {
        Location::Portable => Ok(directory.to_path_buf()),
        Location::AppData => local_app_data
            .filter(|path| path.is_absolute())
            .map(|path| path.join("VK Fast"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Windows LocalAppData directory unavailable",
                )
            }),
    };
    Ok(DataPaths {
        config: base(selection.config)?.join("config"),
        cache: base(selection.cache)?.join("cache"),
    })
}

#[cfg(windows)]
fn data_paths() -> io::Result<&'static DataPaths> {
    type CachedPaths = Result<DataPaths, (io::ErrorKind, String)>;
    static PATHS: std::sync::OnceLock<CachedPaths> = std::sync::OnceLock::new();
    PATHS
        .get_or_init(|| {
            (|| {
                let directory = executable_dir()?;
                let selection = read_selection(&directory)?;
                let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
                resolve(&directory, selection, local.as_deref())
            })()
            .map_err(|error: io::Error| (error.kind(), error.to_string()))
        })
        .as_ref()
        .map_err(|(kind, message)| io::Error::new(*kind, message.clone()))
}

#[cfg(any(windows, test))]
fn directory_of(executable: &Path) -> io::Result<PathBuf> {
    if !executable.is_absolute() || executable.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Application directory unavailable",
        ));
    }
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Application directory unavailable"))
}

#[cfg(windows)]
pub fn executable_dir() -> io::Result<PathBuf> {
    directory_of(&std::env::current_exe()?)
}

#[cfg(windows)]
pub fn config_dir() -> io::Result<PathBuf> {
    Ok(data_paths()?.config.clone())
}

#[cfg(windows)]
pub fn state_dir() -> io::Result<PathBuf> {
    config_dir()
}

#[cfg(windows)]
pub fn cache_dir() -> io::Result<PathBuf> {
    Ok(data_paths()?.cache.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation() -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\Music players\Музыка VK")
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/Music players/Музыка VK")
        }
    }

    #[test]
    fn portable_directories_stay_next_to_the_executable_with_spaces_and_unicode() {
        let installation = installation();
        let directory = directory_of(&installation.join("vk-fast.exe")).unwrap();
        assert_eq!(directory.join("config"), installation.join("config"));
        assert_eq!(directory.join("cache"), installation.join("cache"));
        assert_ne!(directory.join("config"), directory.join("cache"));
    }

    #[test]
    fn relative_or_missing_executable_paths_never_fall_back_to_the_working_directory() {
        for path in [
            Path::new("vk-fast.exe"),
            Path::new("./vk-fast.exe"),
            Path::new(""),
        ] {
            assert!(directory_of(path).is_err());
        }
        #[cfg(windows)]
        assert!(directory_of(Path::new(r"C:\")).is_err());
        #[cfg(not(windows))]
        assert!(directory_of(Path::new("/")).is_err());
    }

    #[test]
    fn installer_choices_are_independent_and_old_portable_layout_still_works() {
        let install = installation();
        let local = install.join("Test AppData");
        for config in [Location::Portable, Location::AppData] {
            for cache in [Location::Portable, Location::AppData] {
                let paths = resolve(&install, Selection { config, cache }, Some(&local)).unwrap();
                assert_eq!(
                    paths.config,
                    if config == Location::Portable {
                        install.join("config")
                    } else {
                        local.join("VK Fast/config")
                    }
                );
                assert_eq!(
                    paths.cache,
                    if cache == Location::Portable {
                        install.join("cache")
                    } else {
                        local.join("VK Fast/cache")
                    }
                );
            }
        }
        assert!(resolve(&install, Selection::default(), None).is_ok());
        assert!(
            resolve(
                &install,
                Selection {
                    config: Location::AppData,
                    cache: Location::Portable
                },
                None
            )
            .is_err()
        );
        assert!(
            resolve(
                &install,
                Selection {
                    config: Location::Portable,
                    cache: Location::AppData
                },
                Some(Path::new("relative"))
            )
            .is_err()
        );
    }

    #[test]
    fn storage_marker_accepts_windows_ini_encodings_and_rejects_ambiguous_or_arbitrary_paths() {
        let ini = "[Storage]\r\nVersion=1\r\nConfig=appdata\r\nCache=portable\r\n";
        let expected = Selection {
            config: Location::AppData,
            cache: Location::Portable,
        };
        assert_eq!(parse_selection(ini.as_bytes()).unwrap(), expected);
        let mut unicode = vec![0xff, 0xfe];
        unicode.extend(ini.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(parse_selection(&unicode).unwrap(), expected);
        for bad in [
            "",
            "[storage]\nversion=2\nconfig=portable\ncache=portable",
            "[storage]\nversion=1\nconfig=appdata",
            "[storage]\nversion=1\nconfig=C:\\Other\ncache=portable",
            "[storage]\nversion=1\nconfig=appdata\nconfig=portable\ncache=appdata",
        ] {
            assert!(parse_selection(bad.as_bytes()).is_err());
        }
    }

    #[test]
    fn missing_marker_is_portable_but_an_invalid_existing_marker_is_not_silently_ignored() {
        let mut random = [0; 12];
        getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!("vk-fast-storage-{:x?}", random));
        fs::create_dir(&dir).unwrap();
        assert_eq!(read_selection(&dir).unwrap(), Selection::default());
        fs::write(dir.join(STORAGE_FILE), b"broken").unwrap();
        assert!(read_selection(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
