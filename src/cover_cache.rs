//! Private, bounded thumbnail files. The directory is indexed once by an
//! artwork worker; subsequent hits and pruning use that in-memory index.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

const LIMIT: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
pub(super) const MAX_FILE: u64 = 4 * 1024 * 1024;

struct Entry {
    bytes: u64,
    touched: u64,
}

#[derive(Default)]
struct Index {
    initialized: bool,
    entries: HashMap<String, Entry>,
    bytes: u64,
    clock: u64,
}

pub(super) struct DiskCache {
    root: Option<PathBuf>,
    limit: u64,
    index: Mutex<Index>,
}

impl DiskCache {
    pub fn new() -> Self {
        Self::at(root_path(), LIMIT)
    }

    pub(super) fn at(root: Option<PathBuf>, limit: u64) -> Self {
        Self {
            root,
            limit,
            index: Mutex::new(Index::default()),
        }
    }

    /// Where artwork files live, for the storage settings.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Bytes of artwork on disk. Indexes the directory once; call off the UI thread.
    pub fn usage(&self) -> io::Result<u64> {
        let root = self.root.as_ref().ok_or_else(unavailable)?;
        let mut index = self.index.lock().map_err(|_| unavailable())?;
        self.initialize(root, &mut index)?;
        Ok(index.bytes)
    }

    /// Delete every saved artwork file. Images already shown stay in memory
    /// and are downloaded again when needed.
    pub fn clear(&self) -> io::Result<()> {
        let root = self.root.as_ref().ok_or_else(unavailable)?;
        let mut index = self.index.lock().map_err(|_| unavailable())?;
        self.initialize(root, &mut index)?;
        let mut result = Ok(());
        for name in index.entries.keys() {
            match fs::remove_file(root.join(name)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => result = Err(error),
                _ => {}
            }
        }
        index.entries.clear();
        index.bytes = 0;
        result
    }

    pub fn load(&self, url: &str, edge: u32) -> io::Result<Vec<u8>> {
        let root = self.root.as_ref().ok_or_else(unavailable)?;
        let mut index = self.index.lock().map_err(|_| unavailable())?;
        self.initialize(root, &mut index)?;
        let name = name(url, edge);
        if !index.entries.contains_key(&name) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let path = root.join(&name);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_FILE {
            return Err(unavailable());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let file = File::open(path)?;
        (&file).take(MAX_FILE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(unavailable());
        }
        index.clock = index.clock.max(timestamp()).saturating_add(1);
        let clock = index.clock;
        // Only the worker touches files. Persist each disk hit's recency so a
        // regularly viewed old cover also survives pruning after a restart.
        let _ = file
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_nanos(clock)));
        if let Some(entry) = index.entries.get_mut(&name) {
            entry.touched = clock;
        }
        Ok(bytes)
    }

    pub fn remove(&self, url: &str, edge: u32) {
        let Some(root) = &self.root else { return };
        let Ok(mut index) = self.index.lock() else {
            return;
        };
        if self.initialize(root, &mut index).is_err() {
            return;
        }
        let name = name(url, edge);
        if fs::remove_file(root.join(&name)).is_ok()
            && let Some(entry) = index.entries.remove(&name)
        {
            index.bytes = index.bytes.saturating_sub(entry.bytes);
        }
    }

    pub fn save(&self, url: &str, edge: u32, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_FILE || bytes.len() as u64 > self.limit {
            return Err(unavailable());
        }
        let root = self.root.as_ref().ok_or_else(unavailable)?;
        let mut index = self.index.lock().map_err(|_| unavailable())?;
        self.initialize(root, &mut index)?;
        let name = name(url, edge);
        let previous = index.entries.get(&name).map_or(0, |entry| entry.bytes);
        let growth = (bytes.len() as u64).saturating_sub(previous);
        self.prune(
            root,
            &mut index,
            growth,
            usize::from(previous == 0),
            Some(&name),
        )?;
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|_| unavailable())?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let temporary = root.join(format!(".{suffix}.tmp"));
        let clock = index.clock.max(timestamp()).saturating_add(1);
        let result: io::Result<()> = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(bytes)?;
            file.set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_nanos(clock)),
            )?;
            drop(file);
            fs::rename(&temporary, root.join(&name))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        index.clock = clock;
        index.entries.insert(
            name,
            Entry {
                bytes: bytes.len() as u64,
                touched: clock,
            },
        );
        index.bytes = index.bytes.saturating_sub(previous) + bytes.len() as u64;
        Ok(())
    }

    fn initialize(&self, root: &Path, index: &mut Index) -> io::Result<()> {
        if index.initialized {
            return Ok(());
        }
        index.entries.clear();
        index.bytes = 0;
        index.clock = 0;
        private_dir(root.parent().ok_or_else(unavailable)?)?;
        private_dir(root)?;
        for entry in fs::read_dir(root)? {
            let Ok(entry) = entry else { continue };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                continue;
            }
            if name.len() == 29
                && name.starts_with('.')
                && name.ends_with(".tmp")
                && name.as_bytes()[1..25].iter().all(u8::is_ascii_hexdigit)
            {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            if !valid_name(&name) {
                continue;
            }
            if metadata.len() == 0 || metadata.len() > MAX_FILE {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            let touched = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| {
                    duration.as_nanos().min(u128::from(u64::MAX)) as u64
                });
            index.clock = index.clock.max(touched);
            index.bytes = index.bytes.saturating_add(metadata.len());
            index.entries.insert(
                name,
                Entry {
                    bytes: metadata.len(),
                    touched,
                },
            );
        }
        self.prune(root, index, 0, 0, None)?;
        index.initialized = true;
        Ok(())
    }

    fn prune(
        &self,
        root: &Path,
        index: &mut Index,
        growth: u64,
        files: usize,
        protected: Option<&str>,
    ) -> io::Result<()> {
        while index.bytes.saturating_add(growth) > self.limit
            || index.entries.len().saturating_add(files) > MAX_FILES
        {
            let oldest = index
                .entries
                .iter()
                .filter(|(name, _)| protected != Some(name.as_str()))
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(name, _)| name.clone())
                .ok_or_else(unavailable)?;
            match fs::remove_file(root.join(&oldest)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if let Some(entry) = index.entries.remove(&oldest) {
                index.bytes = index.bytes.saturating_sub(entry.bytes);
            }
        }
        Ok(())
    }
}

fn name(url: &str, edge: u32) -> String {
    format!("{:x}-{edge}.jpg", Sha256::digest(url.as_bytes()))
}

fn valid_name(name: &str) -> bool {
    let Some((digest, suffix)) = name.split_once('-') else {
        return false;
    };
    digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        && matches!(
            suffix,
            "96.jpg" | "160.jpg" | "256.jpg" | "512.jpg" | "1024.jpg" | "2048.jpg"
        )
}

fn root_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        Some(crate::app_paths::cache_dir().ok()?.join("covers"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(base.join("vk-fast").join("covers"))
    }
}

fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn unavailable() -> io::Error {
    io::Error::other("Artwork cache unavailable")
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_nanos().min(u128::from(u64::MAX)) as u64
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct Fixture(pub PathBuf);

    impl Fixture {
        pub fn new() -> Self {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("vk-fast-cover-{suffix}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn thumbnails_persist_separately_and_paths_never_contain_the_url() {
        let fixture = Fixture::new();
        let root = fixture.0.join("covers");
        let url = "https://images.example.test/private/art.jpg?signature=synthetic";
        DiskCache::at(Some(root.clone()), LIMIT)
            .save(url, 96, b"synthetic-thumbnail")
            .unwrap();
        let next = DiskCache::at(Some(root.clone()), LIMIT);
        assert_eq!(next.load(url, 96).unwrap(), b"synthetic-thumbnail");
        let names = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 1);
        assert!(valid_name(&names[0]));
        assert!(!names[0].contains("signature"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(root.join(&names[0]))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn cache_evicts_oldest_for_its_own_budget_and_keeps_recent_hits() {
        let fixture = Fixture::new();
        let cache = DiskCache::at(Some(fixture.0.join("covers")), 12);
        cache
            .save("https://images.example.test/a", 96, b"aaaa")
            .unwrap();
        cache
            .save("https://images.example.test/b", 96, b"bbbb")
            .unwrap();
        cache
            .save("https://images.example.test/c", 96, b"cccc")
            .unwrap();
        cache.load("https://images.example.test/a", 96).unwrap();
        cache
            .save("https://images.example.test/d", 96, b"dddd")
            .unwrap();
        assert!(cache.load("https://images.example.test/b", 96).is_err());
        assert_eq!(
            cache.load("https://images.example.test/a", 96).unwrap(),
            b"aaaa"
        );
        assert_eq!(cache.index.lock().unwrap().bytes, 12);
        let restarted = DiskCache::at(Some(fixture.0.join("covers")), 8);
        assert!(restarted.load("https://images.example.test/d", 96).is_ok());
        assert!(restarted.index.lock().unwrap().bytes <= 8);
    }

    #[test]
    fn directory_is_indexed_once_and_unrelated_files_are_untouched() {
        let fixture = Fixture::new();
        let root = fixture.0.join("covers");
        let cache = DiskCache::at(Some(root.clone()), LIMIT);
        cache
            .save("https://images.example.test/a", 96, b"aaaa")
            .unwrap();
        fs::write(root.join("leave-me.txt"), b"unrelated").unwrap();
        fs::write(
            root.join(name("https://images.example.test/outside-index", 96)),
            b"bbbb",
        )
        .unwrap();
        assert!(
            cache
                .load("https://images.example.test/outside-index", 96)
                .is_err()
        );
        assert_eq!(fs::read(root.join("leave-me.txt")).unwrap(), b"unrelated");
        assert!(
            cache
                .save(
                    "https://images.example.test/a",
                    96,
                    &vec![0; MAX_FILE as usize + 1]
                )
                .is_err()
        );
        assert_eq!(
            cache.load("https://images.example.test/a", 96).unwrap(),
            b"aaaa"
        );
    }
}
