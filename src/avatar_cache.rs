//! Account avatars live in the application's private cache. The UI only
//! exchanges bounded jobs with one worker; URLs are never saved to disk.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use image::{ImageFormat, imageops::FilterType};

use crate::{cover_art, repaint_target::RepaintTarget};

const MAX_PENDING: usize = 16;
const MAX_ENTRIES: usize = 32;
const MAX_REFRESHED_ACCOUNTS: usize = 256;
const MAX_FILE: u64 = 1024 * 1024;
const SIZE: usize = 256;

#[derive(Default)]
struct RefreshRegistry {
    accounts: HashSet<i64>,
}

fn process_registry() -> Arc<Mutex<RefreshRegistry>> {
    static REGISTRY: OnceLock<Arc<Mutex<RefreshRegistry>>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(RefreshRegistry::default())))
        .clone()
}

#[derive(Clone, Copy)]
enum Kind {
    Load,
    Refresh,
}

struct Job {
    account_id: i64,
    url: Option<String>,
    context: RepaintTarget,
}

struct Reply {
    account_id: i64,
    image: Option<ColorImage>,
    kind: Kind,
}

#[derive(Default)]
struct Entry {
    texture: Option<TextureHandle>,
    image: Option<Arc<ColorImage>>,
    loaded: bool,
    refresh_requested: bool,
    pending: usize,
    touched: u64,
}

pub struct AvatarCache {
    entries: HashMap<i64, Entry>,
    jobs: Option<SyncSender<Job>>,
    replies: Receiver<Reply>,
    canceled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    registry: Arc<Mutex<RefreshRegistry>>,
    pending: usize,
    clock: u64,
    repaint_target: RepaintTarget,
}

impl Default for AvatarCache {
    fn default() -> Self {
        Self::new()
    }
}

impl AvatarCache {
    pub fn new() -> Self {
        #[cfg(not(test))]
        {
            Self::start(None, process_registry(), None)
        }
        #[cfg(test)]
        {
            // App/UI unit tests may use synthetic positive account IDs. They
            // must never read the real user's cache or contact a CDN.
            let (_, replies) = mpsc::sync_channel(MAX_PENDING);
            Self {
                entries: HashMap::new(),
                jobs: None,
                replies,
                canceled: Arc::new(AtomicBool::new(false)),
                worker: None,
                registry: process_registry(),
                pending: 0,
                clock: 0,
                repaint_target: RepaintTarget::default(),
            }
        }
    }

    fn start(
        root: Option<PathBuf>,
        registry: Arc<Mutex<RefreshRegistry>>,
        fetcher: Option<Fetcher>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(MAX_PENDING);
        let (replies_sender, replies) = mpsc::sync_channel(MAX_PENDING);
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let worker = thread::Builder::new()
            .name("vk-avatar-cache".to_owned())
            .spawn(move || worker(receiver, replies_sender, worker_cancel, root, fetcher))
            .ok();
        Self {
            entries: HashMap::new(),
            jobs: worker.as_ref().map(|_| sender),
            replies,
            canceled,
            worker,
            registry,
            pending: 0,
            clock: 0,
            repaint_target: RepaintTarget::default(),
        }
    }

    fn receive(&mut self, context: &egui::Context) {
        while let Ok(reply) = self.replies.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if let Some(entry) = self.entries.get_mut(&reply.account_id) {
                entry.pending = entry.pending.saturating_sub(1);
                if matches!(reply.kind, Kind::Load) {
                    entry.loaded = true;
                }
                if let Some(image) = reply.image {
                    let image = Arc::new(image);
                    entry.texture = Some(context.load_texture(
                        "vk-account-avatar",
                        image.clone(),
                        TextureOptions::LINEAR,
                    ));
                    entry.image = Some(image);
                }
            }
        }
    }

    /// Bind the already-loaded avatars to a replacement native renderer. This
    /// keeps the process-wide refresh registry and pending work intact, so
    /// switching title bars cannot trigger another avatar download.
    pub fn rebind_context(&mut self, context: &egui::Context) {
        self.repaint_target.set_context(context);
        for entry in self.entries.values_mut() {
            if let Some(image) = &entry.image {
                entry.texture = Some(context.load_texture(
                    "vk-account-avatar",
                    image.clone(),
                    TextureOptions::LINEAR,
                ));
            }
        }
        self.receive(context);
    }

    fn ensure_entry(&mut self, account_id: i64) -> bool {
        if self.entries.contains_key(&account_id) {
            return true;
        }
        if self.entries.len() >= MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.pending == 0)
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(id, _)| *id);
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            } else {
                return false;
            }
        }
        self.entries.insert(account_id, Entry::default());
        true
    }

    fn request(&mut self, context: &egui::Context, account_id: i64, url: Option<&str>) {
        if self.pending >= MAX_PENDING {
            return;
        }
        let Some(sender) = &self.jobs else {
            return;
        };
        let Some(entry) = self.entries.get_mut(&account_id) else {
            return;
        };
        // The load is queued before the refresh. A failure therefore cannot
        // replace an older texture or remove the previous file.
        if !entry.loaded && entry.pending == 0 {
            match sender.try_send(Job {
                account_id,
                url: None,
                context: self.repaint_target.with_context(context),
            }) {
                Ok(()) => {
                    self.pending += 1;
                    entry.pending += 1;
                }
                Err(TrySendError::Disconnected(_)) => entry.loaded = true,
                Err(TrySendError::Full(_)) => return,
            }
        }
        if entry.refresh_requested || self.pending >= MAX_PENDING {
            return;
        }
        let Some(url) = url.filter(|url| !url.is_empty()) else {
            return;
        };
        let Ok(mut registry) = self.registry.lock() else {
            entry.refresh_requested = true;
            return;
        };
        if registry.accounts.contains(&account_id)
            || registry.accounts.len() >= MAX_REFRESHED_ACCOUNTS
        {
            entry.refresh_requested = true;
            return;
        }
        // The registry outlives textures/cache instances. Even a new CDN URL
        // or an evicted entry cannot trigger a second refresh this process.
        if url.len() > 16_384 {
            registry.accounts.insert(account_id);
            entry.refresh_requested = true;
            return;
        }
        match sender.try_send(Job {
            account_id,
            url: Some(url.to_owned()),
            context: self.repaint_target.with_context(context),
        }) {
            Ok(()) => {
                registry.accounts.insert(account_id);
                entry.refresh_requested = true;
                entry.pending += 1;
                self.pending += 1;
            }
            Err(TrySendError::Disconnected(_)) => entry.refresh_requested = true,
            Err(TrySendError::Full(_)) => (),
        }
    }

    /// Returns only the requested account's texture. A missing account always
    /// shows the caller's fallback, even while an earlier account is loading.
    pub fn texture(
        &mut self,
        context: &egui::Context,
        account_id: Option<i64>,
        refresh_url: Option<&str>,
    ) -> Option<TextureHandle> {
        self.receive(context);
        let account_id = account_id.filter(|id| *id > 0)?;
        self.clock = self.clock.wrapping_add(1);
        if !self.ensure_entry(account_id) {
            return None;
        }
        if let Some(entry) = self.entries.get_mut(&account_id) {
            entry.touched = self.clock;
        }
        self.request(context, account_id, refresh_url);
        self.entries
            .get(&account_id)
            .and_then(|entry| entry.texture.clone())
    }
}

impl Drop for AvatarCache {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
        self.jobs = None;
        // The HTTP helper has a finite timeout; closing the app never waits
        // for it. The worker checks cancellation before writing/publishing.
        #[cfg(not(test))]
        {
            self.worker = None;
        }
        #[cfg(test)]
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

type Fetcher = Box<dyn FnMut(&str) -> Result<ColorImage, ()> + Send>;

fn worker(
    jobs: Receiver<Job>,
    replies: SyncSender<Reply>,
    canceled: Arc<AtomicBool>,
    root: Option<PathBuf>,
    fetcher: Option<Fetcher>,
) {
    let root = root.map(Ok).unwrap_or_else(root_path);
    // Constructing the HTTP client also stays outside the UI thread.
    let mut fetcher = fetcher.unwrap_or_else(|| {
        let client = cover_art::http_client().ok();
        Box::new(move |url| {
            client
                .as_ref()
                .ok_or(())
                .and_then(|client| cover_art::fetch(client, url))
        })
    });
    while let Ok(job) = jobs.recv() {
        if canceled.load(Ordering::Acquire) {
            break;
        }
        let kind = if job.url.is_some() {
            Kind::Refresh
        } else {
            Kind::Load
        };
        let image = match &root {
            Ok(root) => match job.url {
                Some(url) => refresh(root, job.account_id, &url, &mut fetcher, &canceled),
                None => load(root, job.account_id).ok(),
            },
            Err(_) => None,
        };
        if canceled.load(Ordering::Acquire) {
            break;
        }
        if replies
            .send(Reply {
                account_id: job.account_id,
                image,
                kind,
            })
            .is_err()
        {
            break;
        }
        job.context.request_repaint();
    }
}

fn root_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let root = crate::app_paths::cache_dir()?;
        fs::create_dir_all(&root)?;
        Ok(root.join("avatars"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".cache"))
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Avatar cache unavailable"))?;
        fs::create_dir_all(&base)?;
        Ok(base.join("vk-fast/avatars"))
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid avatar cache data")
}

fn private_dir(path: &Path) -> io::Result<()> {
    let options = fs::DirBuilder::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::DirBuilderExt;
        let mut options = options;
        options.mode(0o700);
        options
    };
    match options.create(path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn prepare(root: &Path) -> io::Result<()> {
    let parent = root.parent().ok_or_else(invalid)?;
    private_dir(parent)?;
    private_dir(root)
}

fn path_for(root: &Path, account_id: i64) -> io::Result<PathBuf> {
    if account_id <= 0 {
        return Err(invalid());
    }
    Ok(root.join(format!("{account_id}.png")))
}

fn load(root: &Path, account_id: i64) -> io::Result<ColorImage> {
    prepare(root)?;
    let path = path_for(root, account_id)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_FILE {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE
        || image::guess_format(&bytes).map_err(|_| invalid())? != ImageFormat::Png
    {
        return Err(invalid());
    }
    cover_art::decode(&bytes).map_err(|_| invalid())
}

fn png(image: &ColorImage) -> io::Result<(ColorImage, Vec<u8>)> {
    let [width, height] = image.size;
    if width == 0
        || height == 0
        || width > SIZE
        || height > SIZE
        || image.pixels.len() != width * height
    {
        return Err(invalid());
    }
    let pixels = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect();
    let image =
        image::RgbaImage::from_raw(width as u32, height as u32, pixels).ok_or_else(invalid)?;
    let image = image::imageops::resize(&image, SIZE as u32, SIZE as u32, FilterType::Triangle);
    let normalized = ColorImage::from_rgba_unmultiplied([SIZE, SIZE], image.as_raw());
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|_| invalid())?;
    let bytes = bytes.into_inner();
    if bytes.len() as u64 > MAX_FILE {
        return Err(invalid());
    }
    Ok((normalized, bytes))
}

fn save(root: &Path, account_id: i64, bytes: &[u8]) -> io::Result<()> {
    prepare(root)?;
    let target = path_for(root, account_id)?;
    if fs::symlink_metadata(&target)
        .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(invalid());
    }
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| io::Error::other("Avatar cache unavailable"))?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = root.join(format!(".{account_id}.{suffix}.tmp"));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn refresh(
    root: &Path,
    account_id: i64,
    url: &str,
    fetcher: &mut Fetcher,
    canceled: &AtomicBool,
) -> Option<ColorImage> {
    refresh_with_save(root, account_id, url, fetcher, canceled, save)
}

fn refresh_with_save(
    root: &Path,
    account_id: i64,
    url: &str,
    fetcher: &mut Fetcher,
    canceled: &AtomicBool,
    mut persist: impl FnMut(&Path, i64, &[u8]) -> io::Result<()>,
) -> Option<ColorImage> {
    let image = fetcher(url).ok()?;
    let (image, bytes) = png(&image).ok()?;
    if canceled.load(Ordering::Acquire) {
        return None;
    }
    persist(root, account_id, &bytes).ok()?;
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::AtomicUsize,
        time::{Duration, Instant},
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random).unwrap();
            let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let base = std::env::temp_dir().join(format!("vk-fast-avatar-test-{name}"));
            fs::create_dir(&base).unwrap();
            Self(base)
        }

        fn root(&self) -> PathBuf {
            self.0.join("vk-fast/avatars")
        }

        fn seed(&self, account_id: i64, color: egui::Color32) -> Vec<u8> {
            let (_, bytes) = png(&solid(color)).unwrap();
            save(&self.root(), account_id, &bytes).unwrap();
            bytes
        }

        fn cache(&self, registry: Arc<Mutex<RefreshRegistry>>, fetch: Fetcher) -> AvatarCache {
            AvatarCache::start(Some(self.root()), registry, Some(fetch))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn registry() -> Arc<Mutex<RefreshRegistry>> {
        Arc::new(Mutex::new(RefreshRegistry::default()))
    }

    fn solid(color: egui::Color32) -> ColorImage {
        ColorImage::filled([4, 4], color)
    }

    fn settle(
        cache: &mut AvatarCache,
        context: &egui::Context,
        id: i64,
        url: Option<&str>,
    ) -> Option<TextureHandle> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let texture = cache.texture(context, Some(id), url);
            if cache.pending == 0 {
                return texture;
            }
            assert!(
                Instant::now() < deadline,
                "synthetic avatar worker did not complete"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn cached_avatar_survives_a_new_cache_without_a_refresh_url() {
        let fixture = Fixture::new();
        let original = fixture.seed(11, egui::Color32::RED);
        let context = egui::Context::default();
        for _ in 0..2 {
            let mut cache = fixture.cache(registry(), Box::new(|_| panic!("no fetch expected")));
            assert!(settle(&mut cache, &context, 11, None).is_some());
            assert_eq!(fs::read(fixture.root().join("11.png")).unwrap(), original);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(fixture.root()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(fixture.root().join("11.png"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn refresh_is_once_per_account_across_new_urls_and_cache_instances() {
        let fixture = Fixture::new();
        let context = egui::Context::default();
        let registry = registry();
        let calls = Arc::new(AtomicUsize::new(0));
        for attempt in 0..2 {
            let counted = calls.clone();
            let mut cache = fixture.cache(
                registry.clone(),
                Box::new(move |_| {
                    counted.fetch_add(1, Ordering::Relaxed);
                    Ok(solid(egui::Color32::GREEN))
                }),
            );
            let url = if attempt == 0 {
                "https://synthetic.invalid/one"
            } else {
                "https://synthetic.invalid/changed"
            };
            assert!(settle(&mut cache, &context, 22, Some(url)).is_some());
            for _ in 0..8 {
                assert!(
                    cache
                        .texture(
                            &context,
                            Some(22),
                            Some("https://synthetic.invalid/another")
                        )
                        .is_some()
                );
            }
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            load(&fixture.root(), 22).unwrap().pixels[0],
            egui::Color32::GREEN
        );
    }

    #[test]
    fn rebind_preserves_ready_avatar_and_once_per_process_refresh_without_a_new_job() {
        let fixture = Fixture::new();
        fixture.seed(22, egui::Color32::BLUE);
        let old_context = egui::Context::default();
        let new_context = egui::Context::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let registry = registry();
        let mut cache = fixture.cache(
            registry.clone(),
            Box::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                Ok(solid(egui::Color32::GREEN))
            }),
        );
        let old_texture = settle(
            &mut cache,
            &old_context,
            22,
            Some("https://synthetic.invalid/first"),
        )
        .unwrap();
        let old_id = old_texture.id();
        drop(old_texture);
        let bytes = fs::read(fixture.root().join("22.png")).unwrap();
        let worker = cache.worker.as_ref().unwrap().thread().id();
        let pixels = Arc::downgrade(cache.entries[&22].image.as_ref().unwrap());
        let late_job = cache.repaint_target.with_context(&old_context);
        old_context.request_repaint();
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = notifications.clone();
        new_context.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        cache.rebind_context(&new_context);
        late_job.request_repaint();
        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert!(old_context.tex_manager().read().meta(old_id).is_none());
        let texture = cache
            .texture(
                &new_context,
                Some(22),
                Some("https://synthetic.invalid/changed"),
            )
            .unwrap();
        assert!(
            new_context
                .tex_manager()
                .read()
                .meta(texture.id())
                .is_some()
        );
        assert!(Arc::ptr_eq(
            &pixels.upgrade().unwrap(),
            cache.entries[&22].image.as_ref().unwrap()
        ));
        assert_eq!(cache.worker.as_ref().unwrap().thread().id(), worker);
        assert!(Arc::ptr_eq(&cache.registry, &registry));
        assert!(registry.lock().unwrap().accounts.contains(&22));
        assert!(cache.entries[&22].loaded && cache.entries[&22].refresh_requested);
        assert_eq!(cache.pending, 0);
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(fs::read(fixture.root().join("22.png")).unwrap(), bytes);
        assert!(!cache.canceled.load(Ordering::Acquire));
        drop(texture);
        old_context.tex_manager().write().take_delta().clear();
        drop(cache);
        new_context.tex_manager().write().take_delta().clear();
    }

    #[test]
    fn failed_refresh_keeps_the_previous_file_and_texture() {
        let fixture = Fixture::new();
        let original = fixture.seed(33, egui::Color32::BLUE);
        let context = egui::Context::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut cache = fixture.cache(
            registry(),
            Box::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                Err(())
            }),
        );
        let old = settle(&mut cache, &context, 33, None).unwrap();
        let after = settle(
            &mut cache,
            &context,
            33,
            Some("https://synthetic.invalid/fail"),
        )
        .unwrap();
        assert_eq!(old.id(), after.id());
        assert_eq!(fs::read(fixture.root().join("33.png")).unwrap(), original);
        settle(
            &mut cache,
            &context,
            33,
            Some("https://synthetic.invalid/retry"),
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn failed_encoding_or_atomic_write_preserves_the_previous_png() {
        let fixture = Fixture::new();
        let original = fixture.seed(44, egui::Color32::YELLOW);
        let canceled = AtomicBool::new(false);
        let mut invalid_image: Fetcher =
            Box::new(|_| Ok(ColorImage::filled([257, 1], egui::Color32::RED)));
        assert!(
            refresh(
                &fixture.root(),
                44,
                "synthetic",
                &mut invalid_image,
                &canceled
            )
            .is_none()
        );
        let mut valid_image: Fetcher = Box::new(|_| Ok(solid(egui::Color32::RED)));
        assert!(
            refresh_with_save(
                &fixture.root(),
                44,
                "synthetic",
                &mut valid_image,
                &canceled,
                |_, _, _| Err(io::Error::other("synthetic write failure"))
            )
            .is_none()
        );
        assert_eq!(fs::read(fixture.root().join("44.png")).unwrap(), original);
    }

    #[test]
    fn accounts_and_logout_are_isolated_and_corrupt_files_are_ignored() {
        let fixture = Fixture::new();
        fixture.seed(55, egui::Color32::RED);
        fixture.seed(66, egui::Color32::BLUE);
        fs::write(fixture.root().join("77.png"), b"invalid synthetic PNG").unwrap();
        let context = egui::Context::default();
        let mut cache = fixture.cache(registry(), Box::new(|_| panic!("no fetch expected")));
        let first = settle(&mut cache, &context, 55, None).unwrap();
        let second = settle(&mut cache, &context, 66, None).unwrap();
        assert_ne!(first.id(), second.id());
        assert!(cache.texture(&context, None, None).is_none());
        assert!(cache.texture(&context, Some(0), None).is_none());
        assert!(cache.texture(&context, Some(-55), None).is_none());
        assert!(settle(&mut cache, &context, 77, None).is_none());
        assert_eq!(
            settle(&mut cache, &context, 55, None).unwrap().id(),
            first.id()
        );
        assert_eq!(
            load(&fixture.root(), 55).unwrap().pixels[0],
            egui::Color32::RED
        );
        assert_eq!(
            load(&fixture.root(), 66).unwrap().pixels[0],
            egui::Color32::BLUE
        );
        assert_eq!(
            fs::read(fixture.root().join("77.png")).unwrap(),
            b"invalid synthetic PNG"
        );
    }

    #[test]
    fn ui_test_constructor_is_inactive_and_entries_are_bounded() {
        let mut cache = AvatarCache::new();
        let context = egui::Context::default();
        for id in 1..100 {
            assert!(
                cache
                    .texture(&context, Some(id), Some("https://synthetic.invalid/no-io"))
                    .is_none()
            );
        }
        assert!(cache.worker.is_none());
        assert!(cache.jobs.is_none());
        assert_eq!(cache.pending, 0);
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
    }
}
