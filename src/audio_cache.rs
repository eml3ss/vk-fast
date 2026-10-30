//! Account-isolated, bounded PCM cache. Filenames contain only numeric frame
//! offsets and a hash of the track identity; URLs and credentials are not stored.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

pub const DEFAULT_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_LIMIT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
pub const BYTES_PER_FRAME: u64 = 8; // 48 kHz, stereo, little-endian f32.
const MAX_READ_FRAMES: usize = 48_000 * 30;

/// Metadata duration is rounded by VK; a successful but implausibly short EOF
/// must never make a truncated track look permanently complete.
pub(crate) fn completed_duration_plausible(end_frame: u64, duration: u32) -> bool {
    end_frame > 0
        && (duration == 0 || end_frame.saturating_add(2 * 48_000) >= u64::from(duration) * 48_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub account_id: i64,
    pub owner_id: i64,
    pub track_id: i64,
    pub duration: u32,
}

impl CacheKey {
    fn name(self) -> io::Result<String> {
        if self.account_id <= 0 || self.owner_id == 0 || self.track_id <= 0 {
            return Err(invalid());
        }
        let mut hash = Sha256::new();
        hash.update(b"vk-fast-pcm-v1-48000-stereo-f32le");
        hash.update(self.account_id.to_le_bytes());
        hash.update(self.owner_id.to_le_bytes());
        hash.update(self.track_id.to_le_bytes());
        hash.update(self.duration.to_le_bytes());
        Ok(hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub used_bytes: u64,
    pub limit_bytes: u64,
}

struct State {
    limit: u64,
    revision: u64,
    pins: HashMap<String, usize>,
}
struct Inner {
    root: PathBuf,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct AudioCache(Arc<Inner>);

/// Protects this track from ordinary LRU eviction. Explicit clear and reducing
/// the limit still remove disk data when necessary to respect the user's limit.
pub struct CachePin {
    inner: Arc<Inner>,
    name: Option<String>,
}
impl Drop for CachePin {
    fn drop(&mut self) {
        if let Some(name) = &self.name
            && let Ok(mut state) = self.inner.state.lock()
            && let Some(count) = state.pins.get_mut(name)
        {
            *count -= 1;
            if *count == 0 {
                state.pins.remove(name);
            }
        }
    }
}

struct ProcessLock(File);
impl Drop for ProcessLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Clone)]
struct Segment {
    start: u64,
    frames: u64,
    path: PathBuf,
}
impl Segment {
    fn end(&self) -> u64 {
        self.start + self.frames
    }
}
struct TrackEntry {
    name: String,
    path: PathBuf,
    bytes: u64,
    used: SystemTime,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid audio cache data")
}
fn locked() -> io::Error {
    io::Error::other("Audio cache lock unavailable")
}
fn validate_limit(limit: u64) -> io::Result<()> {
    if limit > MAX_LIMIT_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Audio cache limit exceeds 5 GiB",
        ))
    } else {
        Ok(())
    }
}
fn root_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let root = crate::app_paths::cache_dir()?;
        private_dir(&root, true)?;
        Ok(root.join("audio"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join(".cache"))
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "User cache directory unavailable")
            })?;
        fs::create_dir_all(&base)?;
        private_dir(&base.join("vk-fast"), true)?;
        Ok(base.join("vk-fast/audio"))
    }
}
fn private_dir(path: &Path, create: bool) -> io::Result<()> {
    if create {
        match fs::create_dir(path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if create {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        } else if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Audio cache directory is not private",
            ));
        }
    }
    Ok(())
}
fn private_file(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Audio cache file is not private",
                    ));
                }
            }
            Ok(Some(metadata))
        }
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options.mode(0o600);
        options
    };
    options
}
fn suffix() -> io::Result<String> {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes)
        .map_err(|_| io::Error::other("Cache temporary name unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    private_file(path)?;
    let parent = path.parent().ok_or_else(invalid)?;
    let temporary = parent.join(format!(".range-{}.tmp", suffix()?));
    let outcome = (|| {
        let mut file = options().write(true).create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_data()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
}
fn segments(path: &Path) -> io::Result<Vec<Segment>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    private_dir(path, false)?;
    let mut output = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(".range-") && name.ends_with(".tmp") {
            // Called under the process lock; no active writer owns this file.
            if private_file(&entry.path())?.is_some() {
                fs::remove_file(entry.path())?;
            }
            continue;
        }
        let Some(start) = name
            .strip_suffix(".pcm")
            .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        let metadata = private_file(&entry.path())?.ok_or_else(invalid)?;
        // Only a separately committed prefix is visible. If a process dies
        // during append, even complete but uncommitted frames stay hidden.
        let marker = entry.path().with_extension("frames");
        let frames = if private_file(&marker)?.is_some_and(|m| m.len() == 8) {
            let mut bytes = [0_u8; 8];
            File::open(marker)?.read_exact(&mut bytes)?;
            u64::from_le_bytes(bytes).min(metadata.len() / BYTES_PER_FRAME)
        } else {
            0
        };
        start.checked_add(frames).ok_or_else(invalid)?;
        output.push(Segment {
            start,
            frames,
            path: entry.path(),
        });
    }
    output.sort_by_key(|segment| segment.start);
    for pair in output.windows(2) {
        if pair[0].end() > pair[1].start {
            return Err(invalid());
        }
    }
    Ok(output)
}
fn range_end(segments: &[Segment], start: u64) -> u64 {
    let mut end = start;
    for segment in segments {
        if segment.start > end {
            break;
        }
        if segment.end() > end {
            end = segment.end();
        }
    }
    end
}

impl AudioCache {
    #[cfg(test)]
    pub(crate) fn test_at(root: PathBuf, limit: u64) -> io::Result<Self> {
        Self::at(root, limit)
    }

    pub fn new(limit_bytes: u64) -> io::Result<Self> {
        Self::at(root_path()?, limit_bytes)
    }
    fn at(root: PathBuf, limit: u64) -> io::Result<Self> {
        validate_limit(limit)?;
        private_dir(&root, true)?;
        let cache = Self(Arc::new(Inner {
            root,
            state: Mutex::new(State {
                limit,
                revision: 0,
                pins: HashMap::new(),
            }),
        }));
        cache.set_limit(limit)?;
        Ok(cache)
    }
    fn process_lock(&self) -> io::Result<ProcessLock> {
        private_dir(&self.0.root, false)?;
        let path = self.0.root.join(".lock");
        private_file(&path)?;
        let file = options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.lock()?;
        Ok(ProcessLock(file))
    }
    fn tracks(&self) -> io::Result<Vec<TrackEntry>> {
        let mut output = Vec::new();
        for entry in fs::read_dir(&self.0.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str().filter(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            }) else {
                continue;
            };
            private_dir(&entry.path(), false)?;
            let parts = segments(&entry.path())?;
            let mut bytes = 0_u64;
            for segment in &parts {
                bytes =
                    bytes.saturating_add(private_file(&segment.path)?.ok_or_else(invalid)?.len());
            }
            let used = private_file(&entry.path().join(".used"))?
                .and_then(|m| m.modified().ok())
                .unwrap_or(UNIX_EPOCH);
            output.push(TrackEntry {
                name: name.to_owned(),
                path: entry.path(),
                bytes,
                used,
            });
        }
        Ok(output)
    }
    fn prune(
        &self,
        state: &State,
        target: u64,
        protect: Option<&str>,
        force: bool,
    ) -> io::Result<u64> {
        let mut tracks = self.tracks()?;
        let mut used = tracks.iter().map(|track| track.bytes).sum::<u64>();
        tracks.sort_by_key(|track| (state.pins.contains_key(&track.name), track.used));
        for track in tracks {
            if used <= target && !(force && target == 0) {
                break;
            }
            if !force
                && (state.pins.contains_key(&track.name) || protect == Some(track.name.as_str()))
            {
                continue;
            }
            fs::remove_dir_all(track.path)?;
            used = used.saturating_sub(track.bytes);
        }
        Ok(used)
    }
    fn touch(path: &Path) -> io::Result<()> {
        let used_path = path.join(".used");
        if private_file(&used_path)?
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed.as_secs() < 1)
        {
            return Ok(());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        atomic_write(&used_path, &stamp.to_le_bytes())
    }
    pub fn pin(&self, key: &CacheKey) -> CachePin {
        let name = key.name().ok();
        if let Some(name) = &name
            && let Ok(mut state) = self.0.state.lock()
        {
            *state.pins.entry(name.clone()).or_default() += 1;
        }
        CachePin {
            inner: self.0.clone(),
            name,
        }
    }
    pub fn stats(&self) -> CacheStats {
        let Ok(state) = self.0.state.lock() else {
            return CacheStats::default();
        };
        let used = self
            .process_lock()
            .and_then(|_lock| {
                self.tracks()
                    .map(|tracks| tracks.iter().map(|t| t.bytes).sum())
            })
            .unwrap_or(0);
        CacheStats {
            used_bytes: used,
            limit_bytes: state.limit,
        }
    }
    pub fn set_limit(&self, bytes: u64) -> io::Result<()> {
        validate_limit(bytes)?;
        let mut state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        self.prune(&state, bytes, None, true)?;
        state.limit = bytes;
        state.revision = state.revision.wrapping_add(1);
        Ok(())
    }
    pub fn clear(&self) -> io::Result<()> {
        let mut state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        state.revision = state.revision.wrapping_add(1);
        for track in self.tracks()? {
            fs::remove_dir_all(track.path)?;
        }
        Ok(())
    }
    /// Writes only missing frames, appending to adjacent ranges. A full budget
    /// returns a short count (possibly zero), never grows beyond the limit.
    pub fn write_range(&self, key: &CacheKey, start: u64, bytes: &[u8]) -> io::Result<usize> {
        self.write_checked(key, start, bytes, None)
    }
    /// Cheap in-process policy snapshot; unlike stats this never scans disk.
    pub(crate) fn policy(&self) -> io::Result<(u64, u64)> {
        let state = self.0.state.lock().map_err(|_| locked())?;
        Ok((state.limit, state.revision))
    }
    pub(crate) fn write_at_revision(
        &self,
        key: &CacheKey,
        start: u64,
        bytes: &[u8],
        revision: u64,
    ) -> io::Result<usize> {
        self.write_checked(key, start, bytes, Some(revision))
    }
    fn write_checked(
        &self,
        key: &CacheKey,
        start: u64,
        bytes: &[u8],
        revision: Option<u64>,
    ) -> io::Result<usize> {
        let name = key.name()?;
        if !bytes.len().is_multiple_of(BYTES_PER_FRAME as usize) {
            return Err(invalid());
        }
        let frames = (bytes.len() / BYTES_PER_FRAME as usize) as u64;
        let end = start.checked_add(frames).ok_or_else(invalid)?;
        let state = self.0.state.lock().map_err(|_| locked())?;
        if revision.is_some_and(|revision| revision != state.revision) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Audio cache policy changed",
            ));
        }
        let _lock = self.process_lock()?;
        if state.limit == 0 || frames == 0 {
            return Ok(0);
        }
        let path = self.0.root.join(&name);
        private_dir(&path, true)?;
        let mut cursor = start;
        while cursor < end {
            let parts = segments(&path)?;
            if let Some(existing) = parts
                .iter()
                .find(|part| part.start <= cursor && part.end() > cursor)
            {
                cursor = end.min(existing.end());
                continue;
            }
            let next = parts
                .iter()
                .find(|part| part.start > cursor)
                .map_or(end, |part| part.start.min(end));
            let requested = (next - cursor)
                .checked_mul(BYTES_PER_FRAME)
                .ok_or_else(invalid)?;
            let target = state.limit.saturating_sub(requested.min(state.limit));
            let used = self.prune(&state, target, Some(&name), false)?;
            let available = state.limit.saturating_sub(used) / BYTES_PER_FRAME;
            let count = (next - cursor).min(available);
            if count == 0 {
                break;
            }
            let begin = usize::try_from(cursor - start)
                .map_err(|_| invalid())?
                .checked_mul(BYTES_PER_FRAME as usize)
                .ok_or_else(invalid)?;
            let length = usize::try_from(count)
                .map_err(|_| invalid())?
                .checked_mul(BYTES_PER_FRAME as usize)
                .ok_or_else(invalid)?;
            if let Some(previous) = parts.iter().find(|part| part.end() == cursor) {
                private_file(&previous.path)?.ok_or_else(invalid)?;
                let mut file = options().write(true).open(&previous.path)?;
                let committed = previous.frames * BYTES_PER_FRAME;
                file.set_len(committed)?; // Discard an interrupted partial frame.
                file.seek(SeekFrom::Start(committed))?;
                file.write_all(&bytes[begin..begin + length])?;
                file.sync_data()?;
                atomic_write(
                    &previous.path.with_extension("frames"),
                    &(previous.frames + count).to_le_bytes(),
                )?;
            } else {
                let segment = path.join(format!("{cursor:020}.pcm"));
                atomic_write(&segment, &bytes[begin..begin + length])?;
                atomic_write(&segment.with_extension("frames"), &count.to_le_bytes())?;
            }
            cursor += count;
        }
        Self::touch(&path)?;
        Ok((cursor - start) as usize)
    }
    pub fn ranges(&self, key: &CacheKey) -> io::Result<Vec<(u64, u64)>> {
        let name = key.name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for part in segments(&self.0.root.join(name))? {
            if part.frames == 0 {
                continue;
            }
            if let Some(last) = ranges.last_mut()
                && last.1 == part.start
            {
                last.1 = part.end();
            } else {
                ranges.push((part.start, part.end()));
            }
        }
        Ok(ranges)
    }
    pub fn contiguous_end(&self, key: &CacheKey, start: u64) -> io::Result<u64> {
        let name = key.name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        Ok(range_end(&segments(&self.0.root.join(name))?, start))
    }
    pub fn contains_range(&self, key: &CacheKey, start: u64, end: u64) -> io::Result<bool> {
        Ok(end >= start && self.contiguous_end(key, start)? >= end)
    }
    pub fn read_contiguous(
        &self,
        key: &CacheKey,
        start: u64,
        max_frames: usize,
    ) -> io::Result<Vec<u8>> {
        let name = key.name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let path = self.0.root.join(name);
        let parts = segments(&path)?;
        let end = range_end(&parts, start)
            .min(start.saturating_add(max_frames.min(MAX_READ_FRAMES) as u64));
        let mut output = Vec::new();
        let mut cursor = start;
        while cursor < end {
            let part = parts
                .iter()
                .find(|part| part.start <= cursor && part.end() > cursor)
                .ok_or_else(invalid)?;
            let count = (part.end().min(end) - cursor) * BYTES_PER_FRAME;
            let mut file = File::open(&part.path)?;
            file.seek(SeekFrom::Start((cursor - part.start) * BYTES_PER_FRAME))?;
            let previous = output.len();
            output.resize(previous + count as usize, 0);
            file.read_exact(&mut output[previous..])?;
            cursor += count / BYTES_PER_FRAME;
        }
        if !output.is_empty() {
            Self::touch(&path)?;
        }
        Ok(output)
    }
    pub fn mark_complete(&self, key: &CacheKey, end_frame: u64) -> io::Result<()> {
        self.mark_complete_checked(key, end_frame, None)
    }
    pub(crate) fn mark_complete_at_revision(
        &self,
        key: &CacheKey,
        end_frame: u64,
        revision: u64,
    ) -> io::Result<()> {
        self.mark_complete_checked(key, end_frame, Some(revision))
    }
    fn mark_complete_checked(
        &self,
        key: &CacheKey,
        end_frame: u64,
        revision: Option<u64>,
    ) -> io::Result<()> {
        let name = key.name()?;
        let state = self.0.state.lock().map_err(|_| locked())?;
        if revision.is_some_and(|revision| revision != state.revision) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Audio cache policy changed",
            ));
        }
        let _lock = self.process_lock()?;
        if state.limit == 0 || !completed_duration_plausible(end_frame, key.duration) {
            return Err(invalid());
        }
        let path = self.0.root.join(name);
        if range_end(&segments(&path)?, 0) < end_frame {
            return Err(invalid());
        }
        atomic_write(&path.join(".complete"), &end_frame.to_le_bytes())
    }
    pub fn complete_frames(&self, key: &CacheKey) -> io::Result<Option<u64>> {
        let name = key.name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let path = self.0.root.join(name);
        let Some(metadata) = private_file(&path.join(".complete"))? else {
            return Ok(None);
        };
        if metadata.len() != 8 {
            return Ok(None);
        }
        let mut bytes = [0_u8; 8];
        File::open(path.join(".complete"))?.read_exact(&mut bytes)?;
        let end = u64::from_le_bytes(bytes);
        Ok((completed_duration_plausible(end, key.duration)
            && range_end(&segments(&path)?, 0) >= end)
            .then_some(end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("vk-fast-audio-cache-test-{}", suffix().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn cache(&self, limit: u64) -> AudioCache {
            AudioCache::at(self.0.join("audio"), limit).unwrap()
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn key(track: i64) -> CacheKey {
        CacheKey {
            account_id: 42,
            owner_id: -77,
            track_id: track,
            // Raw frame-range tests intentionally use a tiny, unknown-duration stream.
            duration: 0,
        }
    }
    fn bytes(start: u64, count: usize) -> Vec<u8> {
        (start..start + count as u64)
            .flat_map(u64::to_le_bytes)
            .collect()
    }

    #[test]
    fn implausible_eof_is_rejected_even_for_an_old_complete_marker() {
        let directory = TestDirectory::new();
        let cache = directory.cache(1024 * 1024);
        let mut key = key(1);
        key.duration = 24;
        cache.write_range(&key, 0, &bytes(0, 48_000)).unwrap();
        assert!(cache.mark_complete(&key, 48_000).is_err());
        let path = cache.0.root.join(key.name().unwrap()).join(".complete");
        atomic_write(&path, &48_000_u64.to_le_bytes()).unwrap();
        assert!(cache.complete_frames(&key).unwrap().is_none());
        assert_eq!(cache.contiguous_end(&key, 0).unwrap(), 48_000);
    }

    #[test]
    fn reopens_partial_ranges_without_filling_gaps_or_mixing_accounts() {
        let directory = TestDirectory::new();
        let cache = directory.cache(4096);
        assert_eq!(cache.write_range(&key(1), 0, &bytes(0, 4)).unwrap(), 4);
        cache.write_range(&key(1), 8, &bytes(8, 4)).unwrap();
        assert_eq!(cache.read_contiguous(&key(1), 0, 12).unwrap(), bytes(0, 4));
        assert_eq!(
            cache.read_contiguous(&key(1), 4, 12).unwrap(),
            Vec::<u8>::new()
        );
        cache.write_range(&key(1), 4, &bytes(4, 4)).unwrap();
        cache.mark_complete(&key(1), 12).unwrap();
        drop(cache);
        let cache = directory.cache(4096);
        assert_eq!(cache.read_contiguous(&key(1), 1, 11).unwrap(), bytes(1, 11));
        assert_eq!(cache.complete_frames(&key(1)).unwrap(), Some(12));
        let mut other = key(1);
        other.account_id += 1;
        assert!(cache.read_contiguous(&other, 0, 12).unwrap().is_empty());
        other = key(1);
        other.duration += 1;
        assert!(cache.read_contiguous(&other, 0, 12).unwrap().is_empty());
        assert_eq!(cache.ranges(&key(1)).unwrap(), vec![(0, 12)]);
    }
    #[test]
    fn budget_eviction_pins_lowering_and_clear_never_grow_past_limit() {
        let directory = TestDirectory::new();
        let cache = directory.cache(64);
        cache.write_range(&key(1), 0, &bytes(0, 4)).unwrap();
        let pin = cache.pin(&key(1));
        cache.write_range(&key(2), 0, &bytes(0, 4)).unwrap();
        cache.write_range(&key(3), 0, &bytes(0, 4)).unwrap();
        assert!(cache.contains_range(&key(1), 0, 4).unwrap());
        assert!(!cache.contains_range(&key(2), 0, 4).unwrap());
        assert!(cache.stats().used_bytes <= 64);
        assert_eq!(cache.write_range(&key(1), 4, &bytes(4, 20)).unwrap(), 4);
        assert_eq!(cache.stats().used_bytes, 64);
        cache.set_limit(16).unwrap();
        assert!(cache.stats().used_bytes <= 16);
        drop(pin);
        cache.set_limit(0).unwrap();
        assert_eq!(cache.write_range(&key(1), 0, &bytes(0, 2)).unwrap(), 0);
        assert_eq!(cache.stats().used_bytes, 0);
        cache.set_limit(64).unwrap();
        cache.write_range(&key(1), 0, &bytes(0, 2)).unwrap();
        cache.clear().unwrap();
        assert_eq!(cache.stats().used_bytes, 0);
        assert!(cache.set_limit(MAX_LIMIT_BYTES + 1).is_err());
    }
    #[test]
    fn interrupted_partial_frame_is_ignored_and_completed_safely() {
        let directory = TestDirectory::new();
        let cache = directory.cache(1024);
        cache.write_range(&key(1), 0, &bytes(0, 2)).unwrap();
        let path = cache
            .0
            .root
            .join(key(1).name().unwrap())
            .join("00000000000000000000.pcm");
        let mut file = options().append(true).open(&path).unwrap();
        // A crash can leave complete frames plus a partial frame after the
        // committed prefix; neither is readable until its marker is committed.
        file.write_all(&bytes(99, 1)).unwrap();
        file.write_all(&[1, 2, 3]).unwrap();
        drop(file);
        assert_eq!(cache.read_contiguous(&key(1), 0, 10).unwrap(), bytes(0, 2));
        cache.write_range(&key(1), 2, &bytes(2, 2)).unwrap();
        assert_eq!(cache.read_contiguous(&key(1), 0, 10).unwrap(), bytes(0, 4));
        assert!(cache.mark_complete(&key(1), 10).is_err());
        assert!(cache.write_range(&key(1), 0, &[1]).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn private_files_and_symlink_rejection() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = TestDirectory::new();
        let cache = directory.cache(1024);
        cache.write_range(&key(1), 0, &bytes(0, 2)).unwrap();
        let track = cache.0.root.join(key(1).name().unwrap());
        let segment = track.join("00000000000000000000.pcm");
        assert_eq!(
            fs::metadata(&track).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&segment).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let alias = track.join("00000000000000000100.pcm");
        symlink(&segment, &alias).unwrap();
        assert!(cache.read_contiguous(&key(1), 100, 1).is_err());
    }
}
