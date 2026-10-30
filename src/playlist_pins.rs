//! Account-bound sidebar pins and manual order. Only playlist identities are stored here.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u8 = 1;
const MAX_KEYS: usize = 1024;
const MAX_BYTES: u64 = 64 * 1024;
const ORDER_MAX_KEYS: usize = 10_000;
const ORDER_MAX_BYTES: u64 = 512 * 1024;
static WRITES: Mutex<()> = Mutex::new(());

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredPins {
    format_version: u8,
    account_id: i64,
    keys: Vec<(i64, i64)>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid VK Fast playlist pins")
}

fn valid_account(account: i64) -> io::Result<()> {
    if account <= 0 { Err(invalid()) } else { Ok(()) }
}

fn normalized(keys: &[(i64, i64)], max_keys: usize) -> io::Result<Vec<(i64, i64)>> {
    if keys.len() > max_keys {
        return Err(invalid());
    }
    let mut seen = HashSet::with_capacity(keys.len());
    let mut result = Vec::with_capacity(keys.len());
    for &(owner, id) in keys {
        if owner == 0 || owner.checked_abs().is_none() || id <= 0 {
            return Err(invalid());
        }
        if seen.insert((owner, id)) {
            result.push((owner, id));
        }
    }
    Ok(result)
}

fn path_in(config: &Path, account: i64) -> io::Result<PathBuf> {
    valid_account(account)?;
    Ok(config
        .join("pinned-playlists")
        .join(format!("{account}.json")))
}

fn pins_path(account: i64) -> io::Result<PathBuf> {
    valid_account(account)?;
    #[cfg(windows)]
    {
        path_in(&crate::app_paths::config_dir()?, account)
    }
    #[cfg(not(windows))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "User config directory is unavailable",
                )
            })?;
        path_in(&config.join("vk-fast"), account)
    }
}

fn order_path_in(config: &Path, account: i64) -> io::Result<PathBuf> {
    valid_account(account)?;
    Ok(config
        .join("playlist-order")
        .join(format!("{account}.json")))
}

fn order_path(account: i64) -> io::Result<PathBuf> {
    let pins = pins_path(account)?;
    let config = pins.parent().and_then(Path::parent).ok_or_else(invalid)?;
    order_path_in(config, account)
}

/// Restore a manual sidebar order independently of the pinned prefix.
pub fn load_order(account: i64) -> io::Result<Vec<(i64, i64)>> {
    load_with_limits(
        &order_path(account)?,
        account,
        ORDER_MAX_KEYS,
        ORDER_MAX_BYTES,
    )
}

/// Atomically save one account's manual order without changing its pins.
pub fn save_order(account: i64, keys: &[(i64, i64)]) -> io::Result<()> {
    save_with_limits(
        &order_path(account)?,
        account,
        keys,
        ORDER_MAX_KEYS,
        ORDER_MAX_BYTES,
    )
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
                "Playlist pins directory is not private",
            ));
        }
    }
    Ok(parent)
}

fn checked_file(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Playlist pins file is not private",
            ));
        }
    }
    Ok(metadata)
}

pub fn load(account: i64) -> io::Result<Vec<(i64, i64)>> {
    load_from(&pins_path(account)?, account)
}

fn load_from(path: &Path, account: i64) -> io::Result<Vec<(i64, i64)>> {
    load_with_limits(path, account, MAX_KEYS, MAX_BYTES)
}

fn load_with_limits(
    path: &Path,
    account: i64,
    max_keys: usize,
    max_bytes: u64,
) -> io::Result<Vec<(i64, i64)>> {
    valid_account(account)?;
    match checked_parent(path, false) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let metadata = match checked_file(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if metadata.len() > max_bytes {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(invalid());
    }
    let stored: StoredPins = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if stored.format_version != FORMAT_VERSION || stored.account_id != account {
        return Err(invalid());
    }
    normalized(&stored.keys, max_keys)
}

pub fn save(account: i64, keys: &[(i64, i64)]) -> io::Result<()> {
    save_to(&pins_path(account)?, account, keys)
}

struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn save_to(path: &Path, account: i64, keys: &[(i64, i64)]) -> io::Result<()> {
    save_with_limits(path, account, keys, MAX_KEYS, MAX_BYTES)
}

fn save_with_limits(
    path: &Path,
    account: i64,
    keys: &[(i64, i64)],
    max_keys: usize,
    max_bytes: u64,
) -> io::Result<()> {
    valid_account(account)?;
    let stored = StoredPins {
        format_version: FORMAT_VERSION,
        account_id: account,
        keys: normalized(keys, max_keys)?,
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| invalid())?;
    if bytes.len() as u64 > max_bytes {
        return Err(invalid());
    }
    let _write = WRITES
        .lock()
        .map_err(|_| io::Error::other("Playlist pins lock unavailable"))?;
    let parent = checked_parent(path, true)?;
    match checked_file(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random)
        .map_err(|_| io::Error::other("Playlist pins temporary name unavailable"))?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = Temporary(parent.join(format!(".pins-{suffix}.tmp")));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary.0)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
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
            let path = std::env::temp_dir().join(format!("vk-fast-playlist-pins-{suffix}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self, account: i64) -> PathBuf {
            path_in(&self.0, account).unwrap()
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn snapshots_round_trip_order_and_deduplicate_without_sharing_accounts() {
        let directory = Directory::new();
        let first = directory.path(42);
        assert!(load_from(&first, 42).unwrap().is_empty());
        save_to(&first, 42, &[(-9, 3), (42, 2), (-9, 3), (42, 1)]).unwrap();
        assert_eq!(load_from(&first, 42).unwrap(), [(-9, 3), (42, 2), (42, 1)]);
        let second = directory.path(43);
        save_to(&second, 43, &[(43, 8)]).unwrap();
        assert_eq!(load_from(&second, 43).unwrap(), [(43, 8)]);
        assert_eq!(
            load_from(&first, 43).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&first).unwrap()).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert_eq!(json["account_id"], 42);
        assert_eq!(json["format_version"], FORMAT_VERSION);
        save_to(&first, 42, &[]).unwrap();
        assert!(load_from(&first, 42).unwrap().is_empty());
        assert_eq!(fs::read_dir(first.parent().unwrap()).unwrap().count(), 2);
        let limit: Vec<_> = (1..=MAX_KEYS as i64).map(|id| (-9, id)).collect();
        save_to(&first, 42, &limit).unwrap();
        assert_eq!(load_from(&first, 42).unwrap(), limit);
    }

    #[test]
    fn manual_order_and_pins_have_independent_account_bound_snapshots() {
        let directory = Directory::new();
        let pins = directory.path(42);
        let order = order_path_in(&directory.0, 42).unwrap();
        let other_order = order_path_in(&directory.0, 43).unwrap();
        let load_order_at =
            |path: &Path, owner| load_with_limits(path, owner, ORDER_MAX_KEYS, ORDER_MAX_BYTES);
        let save_order_at = |path: &Path, owner, keys: &[(i64, i64)]| {
            save_with_limits(path, owner, keys, ORDER_MAX_KEYS, ORDER_MAX_BYTES)
        };
        assert!(load_order_at(&order, 42).unwrap().is_empty());
        save_to(&pins, 42, &[(42, 3)]).unwrap();
        save_order_at(&order, 42, &[(-9, 7), (42, 3), (-9, 7), (42, 1)]).unwrap();
        assert_eq!(
            load_order_at(&order, 42).unwrap(),
            [(-9, 7), (42, 3), (42, 1)]
        );
        assert_eq!(load_from(&pins, 42).unwrap(), [(42, 3)]);
        save_to(&pins, 42, &[(42, 1), (42, 3)]).unwrap();
        assert_eq!(
            load_order_at(&order, 42).unwrap(),
            [(-9, 7), (42, 3), (42, 1)]
        );
        save_order_at(&other_order, 43, &[(43, 8)]).unwrap();
        assert_eq!(load_order_at(&other_order, 43).unwrap(), [(43, 8)]);
        assert!(load_order_at(&order, 43).is_err());
        assert_ne!(pins.parent(), order.parent());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&order).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(order.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn manual_order_has_its_own_larger_bounds_and_invalid_saves_keep_the_snapshot() {
        let directory = Directory::new();
        let path = order_path_in(&directory.0, 42).unwrap();
        let keys: Vec<_> = (0..ORDER_MAX_KEYS as i64)
            .map(|offset| (i64::MAX, i64::MAX - offset))
            .collect();
        save_with_limits(&path, 42, &keys, ORDER_MAX_KEYS, ORDER_MAX_BYTES).unwrap();
        assert_eq!(
            load_with_limits(&path, 42, ORDER_MAX_KEYS, ORDER_MAX_BYTES).unwrap(),
            keys
        );
        // The same payload still exceeds the unchanged pin limit.
        assert!(load_from(&path, 42).is_err());
        let before = fs::read(&path).unwrap();
        let mut too_many = keys.clone();
        too_many.push((42, 1));
        for (owner, invalid_keys) in [(0, vec![(42, 1)]), (42, vec![(0, 1)]), (42, too_many)] {
            assert!(
                save_with_limits(&path, owner, &invalid_keys, ORDER_MAX_KEYS, ORDER_MAX_BYTES)
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        fs::write(&path, vec![b' '; ORDER_MAX_BYTES as usize + 1]).unwrap();
        assert!(load_with_limits(&path, 42, ORDER_MAX_KEYS, ORDER_MAX_BYTES).is_err());
    }

    #[test]
    fn invalid_writes_keep_the_previous_atomic_snapshot() {
        let directory = Directory::new();
        let path = directory.path(42);
        save_to(&path, 42, &[(-9, 7)]).unwrap();
        let before = fs::read(&path).unwrap();
        for keys in [
            vec![(0, 8)],
            vec![(42, 0)],
            vec![(42, -1)],
            vec![(i64::MIN, 8)],
            (1..=MAX_KEYS as i64 + 1).map(|id| (42, id)).collect(),
        ] {
            assert!(save_to(&path, 42, &keys).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        for account in [0, -42] {
            assert!(save_to(&path, account, &[(42, 8)]).is_err());
            assert!(load_from(&path, account).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        assert_eq!(load_from(&path, 42).unwrap(), [(-9, 7)]);
    }

    #[test]
    fn malformed_wrong_version_oversized_and_invalid_identity_files_are_rejected() {
        let directory = Directory::new();
        let path = directory.path(42);
        save_to(&path, 42, &[]).unwrap();
        for bytes in [
            b"not json".to_vec(),
            br#"{"format_version":2,"account_id":42,"keys":[]}"#.to_vec(),
            br#"{"format_version":1,"account_id":43,"keys":[]}"#.to_vec(),
            br#"{"format_version":1,"account_id":42,"keys":[[0,1]]}"#.to_vec(),
            br#"{"format_version":1,"account_id":42,"keys":[],"credentials":"not allowed"}"#
                .to_vec(),
            serde_json::to_vec(&StoredPins {
                format_version: FORMAT_VERSION,
                account_id: 42,
                keys: vec![(42, 1); MAX_KEYS + 1],
            })
            .unwrap(),
            vec![b' '; MAX_BYTES as usize + 1],
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(load_from(&path, 42).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private_and_symlinks_never_read_or_replaced() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = Directory::new();
        let path = directory.path(42);
        save_to(&path, 42, &[(42, 3)]).unwrap();
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
        let outside = directory.0.join("untouched.json");
        fs::rename(&path, &outside).unwrap();
        let before = fs::read(&outside).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(load_from(&path, 42).is_err());
        assert!(save_to(&path, 42, &[]).is_err());
        assert_eq!(fs::read(outside).unwrap(), before);
    }
}
