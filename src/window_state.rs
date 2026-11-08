//! Global restored window geometry, independent of account and audio caches.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 1024;
static WRITES: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RightPanel {
    pub lyrics_open: bool,
    pub queue_open: bool,
    /// The queue's share of the height when both sections are visible.
    pub split: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedWindow {
    pub size: [f32; 2],
    pub maximized: bool,
    /// Outer position in physical desktop pixels; negative monitor origins are valid.
    pub position_pixels: Option<[i32; 2]>,
    /// Outer widths of the left navigation and the right lyrics/queue panel.
    pub panels: Option<[f32; 2]>,
    pub right_panel: Option<RightPanel>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredWindow {
    format_version: u8,
    size: [f32; 2],
    maximized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position_pixels: Option<[i32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    panels: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    right_panel: Option<RightPanel>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid VK Fast window state")
}

fn valid(window: SavedWindow) -> bool {
    window.size.iter().all(|value| value.is_finite())
        && (320.0..=16384.0).contains(&window.size[0])
        && (240.0..=16384.0).contains(&window.size[1])
        && window.position_pixels.is_none_or(|position| {
            position
                .iter()
                .all(|value| (-1_000_000..=1_000_000).contains(value))
        })
        && window.panels.is_none_or(|panels| {
            panels
                .iter()
                .all(|value| value.is_finite() && (100.0..=4096.0).contains(value))
        })
        && window
            .right_panel
            .is_none_or(|panel| panel.split.is_finite() && panel.split > 0.0 && panel.split < 1.0)
}

#[cfg_attr(test, allow(dead_code))] // Unit tests use isolated paths, not the user's config.
fn file_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(crate::app_paths::config_dir()?.join("window.json"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".config"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Configuration directory unavailable",
                )
            })?;
        Ok(base.join("vk-fast/window.json"))
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
                "Window state directory is not private",
            ));
        }
    }
    Ok(parent)
}

fn target(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(metadata))
        }
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn load_from(path: &Path) -> io::Result<Option<SavedWindow>> {
    let Some(metadata) = target(path)? else {
        return Ok(None);
    };
    checked_parent(path, false)?;
    if metadata.len() > MAX_BYTES {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Window state file is not private",
            ));
        }
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let stored: StoredWindow = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let window = SavedWindow {
        size: stored.size,
        maximized: stored.maximized,
        position_pixels: stored.position_pixels,
        panels: stored.panels,
        right_panel: stored.right_panel,
    };
    if stored.format_version != 1 || !valid(window) {
        return Err(invalid());
    }
    Ok(Some(window))
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn save_to(path: &Path, window: SavedWindow) -> io::Result<()> {
    if !valid(window) {
        return Err(invalid());
    }
    let _write = WRITES
        .lock()
        .map_err(|_| io::Error::other("Window state writer unavailable"))?;
    let parent = checked_parent(path, true)?;
    target(path)?;
    let bytes = serde_json::to_vec(&StoredWindow {
        format_version: 1,
        size: window.size,
        maximized: window.maximized,
        position_pixels: window.position_pixels,
        panels: window.panels,
        right_panel: window.right_panel,
    })
    .map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random)
        .map_err(|_| io::Error::other("Window state temporary name unavailable"))?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = Temporary(parent.join(format!(".window-{suffix}.tmp")));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary.0)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
pub fn load() -> io::Result<Option<SavedWindow>> {
    load_from(&file_path()?)
}
#[cfg_attr(test, allow(dead_code))]
pub fn save(window: SavedWindow) -> io::Result<()> {
    save_to(&file_path()?, window)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            Self(std::env::temp_dir().join(format!("vk-fast-window-state-{suffix}")))
        }
        fn path(&self) -> PathBuf {
            self.0.join("config/window.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saved_window_roundtrips_restored_size_and_maximized_state_independently() {
        let directory = Directory::new();
        let path = directory.path();
        assert!(load_from(&path).unwrap().is_none());
        for maximized in [false, true] {
            let window = SavedWindow {
                size: [1184.5, 742.25],
                maximized,
                position_pixels: Some([-1680, 120]),
                panels: Some([300.5, 420.0]),
                right_panel: Some(RightPanel {
                    lyrics_open: true,
                    queue_open: maximized,
                    split: 0.35,
                }),
            };
            save_to(&path, window).unwrap();
            assert_eq!(load_from(&path).unwrap(), Some(window));
        }
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["format_version"], 1);
        assert_eq!(value.as_object().unwrap().len(), 6);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn older_size_only_files_load_and_invalid_positions_do_not_replace_them() {
        let directory = Directory::new();
        let path = directory.path();
        let old = SavedWindow {
            size: [1040.0, 720.0],
            maximized: true,
            position_pixels: None,
            panels: None,
            right_panel: None,
        };
        save_to(&path, old).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&before).contains("position_pixels"));
        assert!(!String::from_utf8_lossy(&before).contains("panels"));
        assert!(!String::from_utf8_lossy(&before).contains("right_panel"));
        assert_eq!(load_from(&path).unwrap(), Some(old));
        for position in [[i32::MIN, 0], [0, i32::MAX], [1_000_001, 0]] {
            assert!(
                save_to(
                    &path,
                    SavedWindow {
                        position_pixels: Some(position),
                        ..old
                    }
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        for panels in [[f32::NAN, 300.0], [252.0, 0.0], [252.0, 5000.0]] {
            assert!(
                save_to(
                    &path,
                    SavedWindow {
                        panels: Some(panels),
                        ..old
                    }
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        for split in [f32::NAN, f32::INFINITY, -0.1, 0.0, 1.0, 1.1] {
            assert!(
                save_to(
                    &path,
                    SavedWindow {
                        right_panel: Some(RightPanel {
                            lyrics_open: true,
                            queue_open: true,
                            split,
                        }),
                        ..old
                    }
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn invalid_writes_preserve_the_last_valid_snapshot() {
        let directory = Directory::new();
        let path = directory.path();
        let saved = SavedWindow {
            size: [1040.0, 720.0],
            maximized: false,
            position_pixels: None,
            panels: None,
            right_panel: None,
        };
        save_to(&path, saved).unwrap();
        let before = fs::read(&path).unwrap();
        for size in [
            [f32::NAN, 720.0],
            [1040.0, f32::INFINITY],
            [0.0, 720.0],
            [319.0, 720.0],
            [1040.0, 239.0],
            [16385.0, 720.0],
        ] {
            assert!(
                save_to(
                    &path,
                    SavedWindow {
                        size,
                        maximized: true,
                        position_pixels: None,
                        panels: None,
                        right_panel: None,
                    }
                )
                .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        assert_eq!(load_from(&path).unwrap(), Some(saved));
    }

    #[test]
    fn corrupt_oversized_and_unsupported_files_are_rejected_without_repairing_them() {
        let directory = Directory::new();
        let path = directory.path();
        save_to(
            &path,
            SavedWindow {
                size: [800.0, 540.0],
                maximized: true,
                position_pixels: None,
                panels: None,
                right_panel: None,
            },
        )
        .unwrap();
        for bytes in [
            b"{broken".to_vec(),
            br#"{"format_version":2,"size":[1040,720],"maximized":false}"#.to_vec(),
            br#"{"format_version":1,"size":[1040,0],"maximized":false}"#.to_vec(),
            br#"{"format_version":1,"size":[1040,720],"maximized":false,"cookie":"no"}"#.to_vec(),
            vec![b' '; MAX_BYTES as usize + 1],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert!(load_from(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn window_state_has_private_permissions_and_never_follows_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = Directory::new();
        let path = directory.path();
        let saved = SavedWindow {
            size: [900.0, 600.0],
            maximized: false,
            position_pixels: None,
            panels: None,
            right_panel: None,
        };
        save_to(&path, saved).unwrap();
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
        let original = directory.0.join("original.json");
        fs::rename(&path, &original).unwrap();
        let before = fs::read(&original).unwrap();
        symlink(&original, &path).unwrap();
        assert!(load_from(&path).is_err());
        assert!(save_to(&path, saved).is_err());
        assert_eq!(fs::read(original).unwrap(), before);
    }
}
