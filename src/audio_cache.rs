//! Account-isolated, bounded audio cache. Each track has a folder named by a
//! hash of its identity: either the original compressed file from VK, in
//! numbered chunks (`media_*`, used for plain files such as MP3), or decoded
//! PCM ranges by frame offset (HLS). URLs and credentials are not stored.
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
/// The original file is kept in chunks of this size: the same ranges the
/// player asks VK for, so each answer is stored as it arrives.
pub const MEDIA_CHUNK: u64 = 256 * 1024;
/// Larger files are not media from VK.
pub const MAX_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
/// Set once the decoded PCM of earlier versions was removed: compressed files
/// take about a tenth of its room, and those tracks simply load again.
const MEDIA_FORMAT_MARKER: &str = ".media-v1";

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
        self.hashed(b"vk-fast-pcm-v1-48000-stereo-f32le")
    }
    /// The folder of the track's original file, apart from its PCM.
    fn media_name(self) -> io::Result<String> {
        self.hashed(b"vk-fast-media-v1-chunks-256k")
    }
    fn hashed(self, domain: &[u8]) -> io::Result<String> {
        if self.account_id <= 0 || self.owner_id == 0 || self.track_id <= 0 {
            return Err(invalid());
        }
        let mut hash = Sha256::new();
        hash.update(domain);
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
pub(crate) fn root_path() -> io::Result<PathBuf> {
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

/// The length of chunk `index` of a `total`-byte file, if it has one.
fn chunk_len(total: u64, index: u64) -> Option<u64> {
    let start = index.checked_mul(MEDIA_CHUNK)?;
    (start < total).then(|| (total - start).min(MEDIA_CHUNK))
}
fn chunk_path(folder: &Path, index: u64) -> PathBuf {
    folder.join(format!("{index:08}.chunk"))
}
fn read_total(folder: &Path) -> io::Result<Option<u64>> {
    let path = folder.join(".total");
    if !private_file(&path)?.is_some_and(|metadata| metadata.len() == 8) {
        return Ok(None);
    }
    let mut bytes = [0_u8; 8];
    File::open(path)?.read_exact(&mut bytes)?;
    let total = u64::from_le_bytes(bytes);
    Ok((1..=MAX_MEDIA_BYTES).contains(&total).then_some(total))
}
/// Whether chunk `index` is stored whole.
fn has_chunk(folder: &Path, total: u64, index: u64) -> io::Result<bool> {
    let Some(len) = chunk_len(total, index) else {
        return Ok(false);
    };
    Ok(private_file(&chunk_path(folder, index))?.is_some_and(|metadata| metadata.len() == len))
}
/// The bytes of a folder's chunks, which count toward the limit like PCM.
fn chunk_bytes(folder: &Path) -> io::Result<u64> {
    let mut bytes = 0_u64;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".chunk"))
            && let Some(metadata) = private_file(&entry.path())?
        {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

/// One track's original file in the cache, as the media decoder reads and
/// fills it. Writes from a background download carry the cache's revision,
/// so a clear or a new limit stops them.
#[derive(Clone)]
pub struct MediaStore {
    cache: AudioCache,
    key: CacheKey,
    revision: Option<u64>,
}
impl MediaStore {
    pub fn new(cache: AudioCache, key: CacheKey) -> Self {
        Self {
            cache,
            key,
            revision: None,
        }
    }
    /// The same store for a background download: it stops at a cache change.
    pub fn at_current_revision(&self) -> io::Result<Self> {
        let (_, revision) = self.cache.policy()?;
        Ok(Self {
            revision: Some(revision),
            ..self.clone()
        })
    }
    /// Whether the cache has room for a whole file, by its policy.
    pub fn fits(&self, total: u64) -> bool {
        self.cache
            .policy()
            .is_ok_and(|(limit, _)| limit >= total.saturating_add(MEDIA_CHUNK))
    }
    pub fn pin(&self) -> CachePin {
        self.cache.pin_media(&self.key)
    }
    /// The file's size once every chunk is stored.
    pub fn complete(&self) -> Option<u64> {
        self.cache.media_complete(&self.key).ok().flatten()
    }
    pub fn total(&self) -> Option<u64> {
        self.cache.media_total(&self.key).ok().flatten()
    }
    pub fn read(&self, total: u64, index: u64) -> Option<Vec<u8>> {
        self.cache
            .read_media_chunk(&self.key, total, index)
            .ok()
            .flatten()
    }
    /// Store one chunk; false when it was not stored (no room, a changed
    /// cache, a disk error). The player goes on either way.
    pub fn write(&self, total: u64, index: u64, bytes: &[u8]) -> bool {
        self.cache
            .write_media_chunk(&self.key, total, index, bytes, self.revision)
            .unwrap_or(false)
    }
    /// How far the stored chunks run on, without a gap, from `byte`.
    pub fn contiguous_end(&self, byte: u64) -> u64 {
        self.cache
            .media_contiguous_end(&self.key, byte)
            .unwrap_or(byte)
    }
    /// Whether the chunk holding `byte` is stored.
    pub fn has_byte(&self, byte: u64) -> bool {
        self.contiguous_end(byte) > byte
    }
    pub fn revision_current(&self) -> bool {
        self.revision.is_none_or(|revision| {
            self.cache
                .policy()
                .is_ok_and(|(_, current)| current == revision)
        })
    }
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
        cache.forget_pcm_once()?;
        cache.set_limit(limit)?;
        Ok(cache)
    }
    /// Remove the decoded PCM of earlier versions, once: it filled the limit
    /// ten times faster than the original files that replace it.
    fn forget_pcm_once(&self) -> io::Result<()> {
        let marker = self.0.root.join(MEDIA_FORMAT_MARKER);
        if private_file(&marker)?.is_some() {
            return Ok(());
        }
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let mut removed = 0_u64;
        for track in self.tracks()? {
            if !segments(&track.path)?.is_empty() {
                removed = removed.saturating_add(track.bytes);
                fs::remove_dir_all(&track.path)?;
            }
        }
        crate::diagnostics::event(
            "cache",
            &format!("old_pcm_removed mib={:.0}", removed as f64 / 1_048_576.0),
        );
        atomic_write(&marker, &[1])
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
            let mut bytes = chunk_bytes(&entry.path())?;
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
    pub fn pin_media(&self, key: &CacheKey) -> CachePin {
        let name = key.media_name().ok();
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
    pub fn media_total(&self, key: &CacheKey) -> io::Result<Option<u64>> {
        let name = key.media_name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let path = self.0.root.join(name);
        if !path.exists() {
            return Ok(None);
        }
        private_dir(&path, false)?;
        read_total(&path)
    }
    /// The file's size when every one of its chunks is stored.
    pub fn media_complete(&self, key: &CacheKey) -> io::Result<Option<u64>> {
        let name = key.media_name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let path = self.0.root.join(name);
        if !path.exists() {
            return Ok(None);
        }
        private_dir(&path, false)?;
        let Some(total) = read_total(&path)? else {
            return Ok(None);
        };
        for index in 0..total.div_ceil(MEDIA_CHUNK) {
            if !has_chunk(&path, total, index)? {
                return Ok(None);
            }
        }
        Ok(Some(total))
    }
    pub fn media_contiguous_end(&self, key: &CacheKey, byte: u64) -> io::Result<u64> {
        let name = key.media_name()?;
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let path = self.0.root.join(name);
        if !path.exists() {
            return Ok(byte);
        }
        private_dir(&path, false)?;
        let Some(total) = read_total(&path)? else {
            return Ok(byte);
        };
        let mut index = byte / MEDIA_CHUNK;
        while has_chunk(&path, total, index)? {
            index += 1;
        }
        Ok((index * MEDIA_CHUNK).min(total).max(byte))
    }
    pub fn read_media_chunk(
        &self,
        key: &CacheKey,
        total: u64,
        index: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        let name = key.media_name()?;
        let Some(len) = chunk_len(total, index) else {
            return Err(invalid());
        };
        let _state = self.0.state.lock().map_err(|_| locked())?;
        let _lock = self.process_lock()?;
        let path = self.0.root.join(name);
        if !path.exists() {
            return Ok(None);
        }
        private_dir(&path, false)?;
        if read_total(&path)? != Some(total) || !has_chunk(&path, total, index)? {
            return Ok(None);
        }
        let mut bytes = vec![0; len as usize];
        File::open(chunk_path(&path, index))?.read_exact(&mut bytes)?;
        Self::touch(&path)?;
        Ok(Some(bytes))
    }
    /// Store chunk `index` of a `total`-byte file. A file of another size
    /// replaces what was kept (VK changed it); a full budget evicts older
    /// tracks, and when that is not enough the chunk is not stored.
    pub fn write_media_chunk(
        &self,
        key: &CacheKey,
        total: u64,
        index: u64,
        bytes: &[u8],
        revision: Option<u64>,
    ) -> io::Result<bool> {
        let name = key.media_name()?;
        if !(1..=MAX_MEDIA_BYTES).contains(&total)
            || chunk_len(total, index) != Some(bytes.len() as u64)
        {
            return Err(invalid());
        }
        let state = self.0.state.lock().map_err(|_| locked())?;
        if revision.is_some_and(|revision| revision != state.revision) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Audio cache policy changed",
            ));
        }
        let _lock = self.process_lock()?;
        if state.limit == 0 {
            return Ok(false);
        }
        let path = self.0.root.join(&name);
        private_dir(&path, true)?;
        match read_total(&path)? {
            Some(known) if known == total => {}
            known => {
                if known.is_some() {
                    for entry in fs::read_dir(&path)? {
                        let entry = entry?;
                        if entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with(".chunk"))
                        {
                            fs::remove_file(entry.path())?;
                        }
                    }
                }
                atomic_write(&path.join(".total"), &total.to_le_bytes())?;
            }
        }
        if has_chunk(&path, total, index)? {
            Self::touch(&path)?;
            return Ok(true);
        }
        let length = bytes.len() as u64;
        let target = state.limit.saturating_sub(length.min(state.limit));
        let used = self.prune(&state, target, Some(&name), false)?;
        if state.limit.saturating_sub(used) < length {
            return Ok(false);
        }
        atomic_write(&chunk_path(&path, index), bytes)?;
        Self::touch(&path)?;
        Ok(true)
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
    fn chunk(index: u64, total: u64) -> Vec<u8> {
        let len = chunk_len(total, index).unwrap();
        (0..len).map(|byte| (byte + index) as u8).collect()
    }

    #[test]
    fn media_chunks_store_a_whole_file_and_a_new_size_replaces_it() {
        let directory = TestDirectory::new();
        let cache = directory.cache(4 * MEDIA_CHUNK);
        let total = 2 * MEDIA_CHUNK + 100;
        let store = MediaStore::new(cache.clone(), key(1));
        assert!(store.total().is_none() && store.complete().is_none());
        assert!(store.write(total, 0, &chunk(0, total)));
        assert!(store.write(total, 2, &chunk(2, total)));
        assert_eq!(store.total(), Some(total));
        assert!(store.complete().is_none());
        assert_eq!(store.contiguous_end(0), MEDIA_CHUNK);
        assert!(store.has_byte(2 * MEDIA_CHUNK + 50) && !store.has_byte(MEDIA_CHUNK));
        assert!(store.read(total, 1).is_none());
        assert!(store.write(total, 1, &chunk(1, total)));
        assert_eq!(store.complete(), Some(total));
        assert_eq!(store.read(total, 2), Some(chunk(2, total)));
        assert_eq!(store.contiguous_end(10), total);
        // A wrong length, or a chunk past the end, is refused.
        assert!(!store.write(total, 1, &chunk(0, total)[..10]));
        assert!(!store.write(total, 3, &[1]));
        // The same file under another account or length is another track.
        let mut other = key(1);
        other.account_id += 1;
        assert!(MediaStore::new(cache.clone(), other).total().is_none());
        // VK sends a file of another size: what was kept goes.
        assert!(store.write(MEDIA_CHUNK, 0, &chunk(0, MEDIA_CHUNK)));
        assert_eq!(store.complete(), Some(MEDIA_CHUNK));
        assert!(store.read(total, 2).is_none());
        // The PCM of the same track lives apart and is untouched.
        cache.write_range(&key(1), 0, &bytes(0, 4)).unwrap();
        assert_eq!(store.complete(), Some(MEDIA_CHUNK));
        assert_eq!(cache.ranges(&key(1)).unwrap(), vec![(0, 4)]);
    }

    #[test]
    fn media_chunks_share_the_limit_eviction_and_pins_with_pcm() {
        let directory = TestDirectory::new();
        let cache = directory.cache(2 * MEDIA_CHUNK);
        let total = MEDIA_CHUNK;
        let first = MediaStore::new(cache.clone(), key(1));
        let second = MediaStore::new(cache.clone(), key(2));
        let third = MediaStore::new(cache.clone(), key(3));
        assert!(first.write(total, 0, &chunk(0, total)));
        let pin = first.pin();
        assert!(second.write(total, 0, &chunk(0, total)));
        // Full: the oldest unpinned track makes room, the pinned one stays.
        assert!(third.write(total, 0, &chunk(0, total)));
        assert!(first.complete().is_some() && second.complete().is_none());
        assert!(third.complete().is_some());
        assert!(cache.stats().used_bytes <= 2 * MEDIA_CHUNK);
        drop(pin);
        cache.set_limit(MEDIA_CHUNK / 2).unwrap();
        assert!(cache.stats().used_bytes <= MEDIA_CHUNK / 2);
        // A chunk larger than the whole limit is not stored at all.
        assert!(!first.write(total, 0, &chunk(0, total)));
        cache.set_limit(2 * MEDIA_CHUNK).unwrap();
        assert!(first.write(total, 0, &chunk(0, total)));
        // A clear stops a download that started before it.
        let download = first.at_current_revision().unwrap();
        cache.clear().unwrap();
        assert_eq!(cache.stats().used_bytes, 0);
        assert!(!download.write(total, 0, &chunk(0, total)));
        assert!(first.write(total, 0, &chunk(0, total)));
    }

    #[test]
    fn old_decoded_pcm_is_removed_once_and_original_files_stay() {
        let directory = TestDirectory::new();
        let root = directory.0.join("audio");
        let cache = directory.cache(4 * MEDIA_CHUNK);
        let store = MediaStore::new(cache.clone(), key(2));
        assert!(store.write(MEDIA_CHUNK, 0, &chunk(0, MEDIA_CHUNK)));
        cache.write_range(&key(1), 0, &bytes(0, 4)).unwrap();
        // As an older version left it: PCM, and no marker yet.
        fs::remove_file(root.join(MEDIA_FORMAT_MARKER)).unwrap();
        drop(cache);
        let cache = directory.cache(4 * MEDIA_CHUNK);
        assert!(cache.ranges(&key(1)).unwrap().is_empty());
        assert_eq!(store.complete(), Some(MEDIA_CHUNK));
        // Later PCM (an HLS track) is kept on the next start.
        cache.write_range(&key(1), 0, &bytes(0, 4)).unwrap();
        drop(cache);
        let cache = directory.cache(4 * MEDIA_CHUNK);
        assert_eq!(cache.ranges(&key(1)).unwrap(), vec![(0, 4)]);
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
