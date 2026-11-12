//! Visible artwork uses small, persistent thumbnails and bounded background
//! jobs. Disk reads, image decoding and HTTP never run in the UI thread.

use std::{
    collections::HashMap,
    io::{Cursor, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use eframe::egui::{self, Color32, ColorImage, Response, Sense, TextureHandle, TextureOptions};
use image::{ImageFormat, ImageReader, Limits, imageops::FilterType};
use reqwest::{Url, blocking::Client, redirect::Policy};

use crate::{
    cover_cache::DiskCache,
    icons::{self, Icon},
    repaint_target::RepaintTarget,
    theme,
};

const WORKERS: usize = 3;
const MAX_PENDING: usize = 48;
const MAX_TEXTURES: usize = 128;
const MAX_HIGH_RES_TEXTURES: usize = 2;
const MAX_ENTRIES: usize = 256;
const MAX_BODY: u64 = 3 * 1024 * 1024;
const MAX_HIGH_RES_BODY: u64 = 8 * 1024 * 1024;
const MAX_DECODE: u64 = 32 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const TEXTURE_SIZE: u32 = 256;
const HIGH_RES_TEXTURE_SIZE: u32 = 2048;
const CACHE_SIZES: [u32; 6] = [96, 160, 256, 512, 1024, HIGH_RES_TEXTURE_SIZE];
use crate::cover_cache::KEEP_ASPECT;

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    url: String,
    edge: u32,
}

struct Job {
    key: Key,
    context: RepaintTarget,
}

struct Reply {
    key: Key,
    image: Option<ColorImage>,
    color: Option<Color32>,
}

enum State {
    Pending,
    /// The texture lives on the GPU; its pixels stay in memory only for the
    /// one cover an effect reads (see `ready_image`).
    Ready(TextureHandle, Option<Color32>, Option<Arc<ColorImage>>),
    /// Asked again after `FAILED_RETRY`: a network failure must not keep the
    /// large fullscreen cover a stretched thumbnail until the next start.
    Failed(std::time::Instant),
}

const FAILED_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

struct Entry {
    state: State,
    touched: u64,
}

pub struct CoverCache {
    entries: HashMap<Key, Entry>,
    disk: Arc<DiskCache>,
    jobs: Option<SyncSender<Job>>,
    replies: Receiver<Reply>,
    canceled: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    pending: usize,
    clock: u64,
    repaint_target: RepaintTarget,
    /// The one cover whose pixels are kept in memory for `ready_image`.
    kept: Option<Key>,
}

impl CoverCache {
    /// The artwork files on disk, shared with the download workers.
    pub(crate) fn disk(&self) -> Arc<DiskCache> {
        self.disk.clone()
    }

    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Job>(MAX_PENDING);
        let (reply_sender, replies) = mpsc::sync_channel::<Reply>(MAX_PENDING);
        let receiver = Arc::new(Mutex::new(receiver));
        let canceled = Arc::new(AtomicBool::new(false));
        // Synthetic app/UI tests must never read the user's files or contact
        // arbitrary artwork URLs supplied by fixtures.
        let enabled = !cfg!(test);
        let client = enabled.then(http_client).and_then(Result::ok);
        // Tests never index or clear the user's artwork directory.
        let disk = Arc::new(if enabled {
            DiskCache::new()
        } else {
            DiskCache::at(None, 0)
        });
        let mut workers = Vec::new();
        if enabled {
            for index in 0..WORKERS {
                let receiver = receiver.clone();
                let replies = reply_sender.clone();
                let canceled = canceled.clone();
                let client = client.clone();
                let disk = disk.clone();
                if let Ok(worker) = thread::Builder::new()
                    .name(format!("vk-cover-{index}"))
                    .spawn(move || worker(receiver, replies, canceled, client, disk))
                {
                    workers.push(worker);
                }
            }
        }
        Self {
            entries: HashMap::new(),
            disk,
            jobs: if workers.is_empty() {
                None
            } else {
                Some(sender)
            },
            replies,
            canceled,
            workers,
            pending: 0,
            clock: 0,
            repaint_target: RepaintTarget::default(),
            kept: None,
        }
    }

    fn receive(&mut self, context: &egui::Context) {
        while let Ok(reply) = self.replies.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if !self
                .entries
                .get(&reply.key)
                .is_some_and(|entry| matches!(entry.state, State::Pending))
            {
                continue;
            }
            let state = if let Some(image) = reply.image {
                if reply.key.edge > TEXTURE_SIZE
                    && self
                        .entries
                        .iter()
                        .filter(|(key, entry)| {
                            key.edge > TEXTURE_SIZE && matches!(entry.state, State::Ready(..))
                        })
                        .count()
                        >= MAX_HIGH_RES_TEXTURES
                {
                    self.evict_oldest(true, TEXTURE_SIZE + 1);
                }
                if self
                    .entries
                    .values()
                    .filter(|entry| matches!(entry.state, State::Ready(..)))
                    .count()
                    >= MAX_TEXTURES
                {
                    self.evict_oldest(true, 0);
                }
                let image = Arc::new(image);
                let texture =
                    context.load_texture("vk-album-cover", image.clone(), TextureOptions::LINEAR);
                // Uploaded: the pixels go unless an effect reads this cover.
                let kept = (self.kept.as_ref() == Some(&reply.key)).then_some(image);
                State::Ready(texture, reply.color, kept)
            } else {
                State::Failed(std::time::Instant::now())
            };
            if let Some(entry) = self.entries.get_mut(&reply.key) {
                entry.state = state;
            }
        }
    }

    /// After the previous native renderer has retired, re-upload the covers
    /// whose pixels are kept; the others are dropped and load again from the
    /// disk cache when next shown. Pending requests remain untouched.
    pub fn rebind_context(&mut self, context: &egui::Context) {
        self.repaint_target.set_context(context);
        self.entries.retain(|_, entry| match &mut entry.state {
            State::Ready(texture, _, Some(image)) => {
                *texture =
                    context.load_texture("vk-album-cover", image.clone(), TextureOptions::LINEAR);
                true
            }
            State::Ready(_, _, None) => false,
            State::Pending | State::Failed(_) => true,
        });
        self.receive(context);
    }

    fn evict_oldest(&mut self, textures_only: bool, min_edge: u32) -> bool {
        let oldest = self
            .entries
            .iter()
            .filter(|(key, entry)| {
                key.edge >= min_edge
                    && match entry.state {
                        State::Pending => false,
                        State::Ready(..) => true,
                        State::Failed(_) => !textures_only,
                    }
            })
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            self.entries.remove(&key);
            true
        } else {
            false
        }
    }

    fn request(&mut self, context: &egui::Context, key: Key) {
        if self.entries.get(&key).is_some_and(
            |entry| matches!(entry.state, State::Failed(at) if at.elapsed() >= FAILED_RETRY),
        ) {
            self.entries.remove(&key);
        }
        if self.entries.contains_key(&key) || self.pending >= MAX_PENDING {
            return;
        }
        if self.entries.len() >= MAX_ENTRIES && !self.evict_oldest(false, 0) {
            return;
        }
        let valid =
            key.url.len() <= 16_384 && Url::parse(&key.url).is_ok_and(|url| valid_url(&url));
        let state = if valid {
            match &self.jobs {
                Some(jobs) => match jobs.try_send(Job {
                    key: key.clone(),
                    context: self.repaint_target.with_context(context),
                }) {
                    Ok(()) => {
                        self.pending += 1;
                        State::Pending
                    }
                    Err(TrySendError::Full(_)) => return,
                    Err(TrySendError::Disconnected(_)) => State::Failed(std::time::Instant::now()),
                },
                None => State::Failed(std::time::Instant::now()),
            }
        } else {
            State::Failed(std::time::Instant::now())
        };
        self.entries.insert(
            key,
            Entry {
                state,
                touched: self.clock,
            },
        );
    }

    /// Allocates a hover-only square: the surrounding track row keeps its click
    /// behavior. Missing, pending and failed covers share a quiet placeholder.
    pub fn show(&mut self, ui: &mut egui::Ui, url: Option<&str>, size: f32) -> Response {
        self.show_rounded(ui, url, size, 6)
    }

    pub fn show_rounded(
        &mut self,
        ui: &mut egui::Ui,
        url: Option<&str>,
        size: f32,
        radius: u8,
    ) -> Response {
        self.show_with_resolution(ui, url, size, radius, texture_edge(size), None)
    }

    /// Fullscreen artwork is sized in physical pixels, with its own disk and
    /// memory entries. A resident thumbnail remains visible while it loads.
    pub fn show_high_resolution(
        &mut self,
        ui: &mut egui::Ui,
        url: Option<&str>,
        fallback: Option<&str>,
        size: f32,
        radius: u8,
    ) -> Response {
        let edge = if size * ui.ctx().pixels_per_point() > 512.0 {
            HIGH_RES_TEXTURE_SIZE
        } else {
            1024
        };
        self.show_with_resolution(ui, url, size, radius, edge, fallback)
    }

    fn show_with_resolution(
        &mut self,
        ui: &mut egui::Ui,
        url: Option<&str>,
        size: f32,
        radius: u8,
        edge: u32,
        fallback: Option<&str>,
    ) -> Response {
        self.receive(ui.ctx());
        self.clock = self.clock.wrapping_add(1);
        let size = size.clamp(1.0, 512.0);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        if let Some(url) = url.filter(|url| !url.is_empty()) {
            let key = Key {
                url: url.to_owned(),
                edge,
            };
            self.request(ui.ctx(), key.clone());
            let key = if edge > TEXTURE_SIZE
                && !self
                    .entries
                    .get(&key)
                    .is_some_and(|entry| matches!(entry.state, State::Ready(..)))
            {
                self.entries
                    .iter()
                    .filter(|(candidate, entry)| {
                        (candidate.url == url || Some(candidate.url.as_str()) == fallback)
                            && candidate.edge < edge
                            && matches!(entry.state, State::Ready(..))
                    })
                    .max_by_key(|(candidate, _)| candidate.edge)
                    .map_or(key, |(candidate, _)| candidate.clone())
            } else {
                key
            };
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.touched = self.clock;
                if let State::Ready(texture, _, _) = &entry.state {
                    egui::Image::from_texture(texture)
                        .corner_radius(radius)
                        .show_loading_spinner(false)
                        .paint_at(ui, rect);
                    return response;
                }
            }
        }
        ui.painter().rect_filled(rect, radius, theme::selected());
        icons::paint(
            ui.painter(),
            rect.shrink(size * 0.27),
            Icon::Headphones,
            theme::muted(),
        );
        response
    }

    /// Paint an image in its own proportions so it covers `rect`, cropped
    /// around its center. A smaller resident copy is
    /// shown while the sharp one loads. Returns false until any copy is ready.
    pub fn paint_fill(&mut self, ui: &egui::Ui, url: &str, rect: egui::Rect, radius: u8) -> bool {
        if url.is_empty() || !rect.is_positive() || !ui.is_rect_visible(rect) {
            return false;
        }
        self.receive(ui.ctx());
        self.clock = self.clock.wrapping_add(1);
        let pixels = rect.width().max(rect.height()) * ui.ctx().pixels_per_point();
        let edge = if pixels > 1024.0 {
            HIGH_RES_TEXTURE_SIZE
        } else {
            1024
        };
        let key = Key {
            url: url.to_owned(),
            edge: edge | KEEP_ASPECT,
        };
        self.request(ui.ctx(), key.clone());
        let clock = self.clock;
        let Some(texture) = self
            .entries
            .iter_mut()
            .filter(|(candidate, entry)| {
                candidate.url == url
                    && candidate.edge & KEEP_ASPECT != 0
                    && matches!(entry.state, State::Ready(..))
            })
            .max_by_key(|(candidate, _)| candidate.edge)
            .and_then(|(_, entry)| {
                entry.touched = clock;
                match &entry.state {
                    State::Ready(texture, _, _) => Some(texture.clone()),
                    _ => None,
                }
            })
        else {
            return false;
        };
        let image = texture.size_vec2();
        if image.x <= 0.0 || image.y <= 0.0 {
            return false;
        }
        // The share of the image that keeps the rect's proportions.
        let scale = (rect.width() / image.x).max(rect.height() / image.y);
        let span = egui::vec2(
            (rect.width() / (image.x * scale)).min(1.0),
            (rect.height() / (image.y * scale)).min(1.0),
        );
        let center = egui::vec2(0.5, 0.5);
        let min = egui::pos2(
            (center.x - span.x / 2.0).clamp(0.0, 1.0 - span.x),
            (center.y - span.y / 2.0).clamp(0.0, 1.0 - span.y),
        );
        egui::Image::from_texture(&texture)
            .uv(egui::Rect::from_min_size(min, span))
            .corner_radius(radius)
            .show_loading_spinner(false)
            .paint_at(ui, rect);
        true
    }

    /// Uses the same thumbnail job as artwork rendering. Its representative
    /// color is already computed by the worker, including on a disk-cache hit.
    pub fn header_color(
        &mut self,
        context: &egui::Context,
        url: Option<&str>,
        size: f32,
    ) -> Option<Color32> {
        self.receive(context);
        self.clock = self.clock.wrapping_add(1);
        let url = url.filter(|url| !url.is_empty())?;
        let key = Key {
            url: url.to_owned(),
            edge: texture_edge(size),
        };
        self.request(context, key.clone());
        let entry = self.entries.get_mut(&key)?;
        entry.touched = self.clock;
        match &entry.state {
            State::Ready(_, color, _) => *color,
            _ => None,
        }
    }

    /// The decoded cover behind `header_color`, for effects drawn from it.
    pub fn ready_image(
        &mut self,
        context: &egui::Context,
        url: Option<&str>,
        size: f32,
    ) -> Option<Arc<ColorImage>> {
        self.receive(context);
        self.clock = self.clock.wrapping_add(1);
        let url = url.filter(|url| !url.is_empty())?;
        let key = Key {
            url: url.to_owned(),
            edge: texture_edge(size),
        };
        if self.kept.as_ref() != Some(&key) {
            // Only one cover keeps its pixels: the previous one lets go.
            if let Some(previous) = self.kept.replace(key.clone())
                && let Some(entry) = self.entries.get_mut(&previous)
                && let State::Ready(_, _, image) = &mut entry.state
            {
                *image = None;
            }
            // Already shown without its pixels: load it again to keep them.
            if self
                .entries
                .get(&key)
                .is_some_and(|entry| matches!(entry.state, State::Ready(_, _, None)))
            {
                self.entries.remove(&key);
            }
        }
        self.request(context, key.clone());
        let entry = self.entries.get_mut(&key)?;
        entry.touched = self.clock;
        match &entry.state {
            State::Ready(_, _, image) => image.clone(),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn seed_image_for_test(
        &mut self,
        context: &egui::Context,
        url: &str,
        size: f32,
        image: ColorImage,
    ) {
        let color = representative_color(&image);
        let image = Arc::new(image);
        let texture =
            context.load_texture("synthetic-cover", image.clone(), TextureOptions::LINEAR);
        self.entries.insert(
            Key {
                url: url.to_owned(),
                edge: texture_edge(size),
            },
            Entry {
                state: State::Ready(texture, color, Some(image)),
                touched: self.clock,
            },
        );
    }
}

/// Paints an account button without doing any file or network work. The caller
/// supplies a cached square texture, translated tooltip and click action.
pub(crate) fn avatar_button(
    ui: &mut egui::Ui,
    texture: Option<&TextureHandle>,
    initials: &str,
    size: f32,
) -> Response {
    let size = size.clamp(16.0, 128.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Account")
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let visuals = ui.style().interact(&response);
    let fill = if response.is_pointer_button_down_on() {
        theme::selected()
    } else {
        visuals.bg_fill
    };
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    if let Some(texture) = texture {
        // Decoded images are square; a half-width corner radius produces a
        // circular texture mask rather than an overlay hiding square corners.
        egui::Image::from_texture(texture)
            .corner_radius((size * 0.5).ceil() as u8)
            .show_loading_spinner(false)
            .paint_at(ui, rect);
    } else {
        let initials: String = initials
            .chars()
            .filter(|character| !character.is_control() && !character.is_whitespace())
            .flat_map(char::to_uppercase)
            .take(2)
            .collect();
        if initials.is_empty() {
            paint_account_outline(ui.painter(), rect, visuals.fg_stroke.color);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initials,
                egui::FontId::new(size * 0.36, theme::semibold()),
                visuals.fg_stroke.color,
            );
        }
    }
    let stroke = if response.has_focus() {
        egui::Stroke::new(2.0, theme::accent())
    } else if response.hovered() {
        egui::Stroke::new(1.5, theme::accent())
    } else {
        visuals.bg_stroke
    };
    ui.painter()
        .circle_stroke(rect.center(), (size - stroke.width) * 0.5, stroke);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_account_outline(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    icons::paint(
        painter,
        rect.shrink(rect.width() * 0.16),
        Icon::Account,
        color,
    );
}

impl Drop for CoverCache {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
        self.jobs = None;
        // Active downloads have an eight-second timeout. Detaching avoids
        // blocking window close; cancellation prevents queued work from running.
        self.workers.clear();
    }
}

fn worker(
    jobs: Arc<Mutex<Receiver<Job>>>,
    replies: SyncSender<Reply>,
    canceled: Arc<AtomicBool>,
    client: Option<Client>,
    disk: Arc<DiskCache>,
) {
    loop {
        if canceled.load(Ordering::Acquire) {
            break;
        }
        let job = match jobs.lock() {
            Ok(receiver) => match receiver.recv() {
                Ok(job) => job,
                Err(_) => break,
            },
            Err(_) => break,
        };
        if canceled.load(Ordering::Acquire) {
            break;
        }
        let image = load_cover(&disk, &job.key.url, job.key.edge, || {
            fetch_sized(client.as_ref().ok_or(())?, &job.key.url, job.key.edge)
        })
        .ok();
        let color = image.as_ref().and_then(representative_color);
        if canceled.load(Ordering::Acquire) {
            break;
        }
        if replies
            .send(Reply {
                key: job.key,
                image,
                color,
            })
            .is_err()
        {
            break;
        }
        job.context.request_repaint();
    }
}

/// Samples at most 32×32 pixels. Nearby hues are grouped so photograph shading
/// does not lose to a few identical bright pixels; monochrome art stays neutral.
fn representative_color(image: &ColorImage) -> Option<Color32> {
    #[derive(Clone, Copy, Default)]
    struct Bin {
        weight: u32,
        red: u32,
        green: u32,
        blue: u32,
    }
    let [width, height] = image.size;
    if width == 0 || height == 0 || width.checked_mul(height)? > image.pixels.len() {
        return None;
    }
    let mut bins = [Bin::default(); 12];
    let mut visible = 0_u32;
    let mut colored = 0_u32;
    let mut luminance = 0_u64;
    let columns = width.min(32);
    let rows = height.min(32);
    for row in 0..rows {
        for column in 0..columns {
            let x = ((column * 2 + 1) * width / (columns * 2)).min(width - 1);
            let y = ((row * 2 + 1) * height / (rows * 2)).min(height - 1);
            let [red, green, blue, alpha] = image.pixels[y * width + x].to_srgba_unmultiplied();
            if alpha < 128 {
                continue;
            }
            let light = (u32::from(red) * 54 + u32::from(green) * 183 + u32::from(blue) * 19) / 256;
            visible += 1;
            luminance += u64::from(light);
            let high = red.max(green).max(blue);
            let low = red.min(green).min(blue);
            let chroma = high - low;
            if light < 12 || high < 48 || chroma < 24 {
                continue;
            }
            colored += 1;
            let delta = f32::from(chroma);
            let hue = if high == red {
                (f32::from(green) - f32::from(blue)) / delta
            } else if high == green {
                2.0 + (f32::from(blue) - f32::from(red)) / delta
            } else {
                4.0 + (f32::from(red) - f32::from(green)) / delta
            };
            let index = ((hue * 2.0).rem_euclid(12.0).floor() as usize).min(11);
            let weight = u32::from(chroma) + 32;
            let bin = &mut bins[index];
            bin.weight += weight;
            bin.red += u32::from(red) * weight;
            bin.green += u32::from(green) * weight;
            bin.blue += u32::from(blue) * weight;
        }
    }
    if visible == 0 {
        return None;
    }
    if colored < (visible / 50).max(1) {
        return Some(Color32::from_gray((luminance / u64::from(visible)) as u8));
    }
    let mut winner = Bin::default();
    for index in 0..bins.len() {
        let mut combined = Bin::default();
        for neighboring in [(index + 11) % 12, index, (index + 1) % 12] {
            let bin = bins[neighboring];
            combined.weight += bin.weight;
            combined.red += bin.red;
            combined.green += bin.green;
            combined.blue += bin.blue;
        }
        if combined.weight > winner.weight {
            winner = combined;
        }
    }
    Some(Color32::from_rgb(
        (winner.red / winner.weight) as u8,
        (winner.green / winner.weight) as u8,
        (winner.blue / winner.weight) as u8,
    ))
}

fn valid_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| {
            host.contains('.')
                && host != "localhost"
                && ![".localhost", ".local", ".internal", ".lan"]
                    .iter()
                    .any(|suffix| host.ends_with(suffix))
                && host.parse::<std::net::IpAddr>().is_err()
        })
}

pub(crate) fn http_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(4))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 || !valid_url(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .user_agent(concat!("vk-fast/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Headless diagnostic reports only success; response bytes and URLs are never
/// printed. Uses the exact same bounded download and decoder as the UI.
pub(crate) fn check(value: &str) -> bool {
    http_client().is_ok_and(|client| fetch(&client, value).is_ok())
}

pub(crate) fn fetch(client: &Client, value: &str) -> Result<ColorImage, ()> {
    fetch_sized(client, value, TEXTURE_SIZE)
}

fn fetch_sized(client: &Client, value: &str, edge: u32) -> Result<ColorImage, ()> {
    let limit = if edge > TEXTURE_SIZE {
        MAX_HIGH_RES_BODY
    } else {
        MAX_BODY
    };
    if value.len() > 16_384 {
        return Err(());
    }
    let url = Url::parse(value).map_err(|_| ())?;
    if !valid_url(&url) {
        return Err(());
    }
    let response = client.get(url).send().map_err(|_| ())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > limit)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > limit {
        return Err(());
    }
    decode_sized(&bytes, edge)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<ColorImage, ()> {
    decode_sized(bytes, TEXTURE_SIZE)
}

fn decode_sized(bytes: &[u8], edge: u32) -> Result<ColorImage, ()> {
    let limit = if edge > TEXTURE_SIZE {
        MAX_HIGH_RES_BODY
    } else {
        MAX_BODY
    };
    if bytes.len() as u64 > limit {
        return Err(());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ())?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP)
    ) {
        return Err(());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| ())?;
    if image.width() == 0 || image.height() == 0 {
        return Err(());
    }
    let keep_aspect = edge & KEEP_ASPECT != 0;
    let edge = (edge & !KEEP_ASPECT).clamp(1, HIGH_RES_TEXTURE_SIZE);
    let filter = if edge > TEXTURE_SIZE {
        FilterType::Lanczos3
    } else {
        FilterType::Triangle
    };
    if keep_aspect {
        // Fit inside edge × edge, never enlarged.
        let image = if image.width().max(image.height()) > edge {
            image.resize(edge, edge, filter)
        } else {
            image
        }
        .into_rgba8();
        return Ok(ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ));
    }
    let size = image.width().min(image.height()).min(edge);
    let image = image.resize_to_fill(size, size, filter).into_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [size as usize, size as usize],
        image.as_raw(),
    ))
}

fn texture_edge(size: f32) -> u32 {
    if size <= 64.0 {
        96
    } else if size <= 132.0 {
        160
    } else {
        256
    }
}

fn load_cover(
    disk: &DiskCache,
    url: &str,
    edge: u32,
    fetcher: impl FnOnce() -> Result<ColorImage, ()>,
) -> Result<ColorImage, ()> {
    // A larger existing thumbnail satisfies a smaller view without HTTP.
    // Square thumbnails never stand in for a proportional image.
    let keep_aspect = edge & KEEP_ASPECT != 0;
    if keep_aspect && let Ok(bytes) = disk.load(url, edge) {
        if let Ok(image) = decode_sized(&bytes, edge) {
            return Ok(image);
        }
        disk.remove(url, edge);
    }
    for candidate in CACHE_SIZES
        .into_iter()
        .filter(|candidate| !keep_aspect && *candidate >= edge)
    {
        if let Ok(bytes) = disk.load(url, candidate) {
            if let Ok(image) = decode_sized(&bytes, edge) {
                return Ok(image);
            }
            disk.remove(url, candidate);
        }
    }
    match fetcher() {
        Ok(image) => {
            if let Ok(bytes) = jpeg(&image) {
                let _ = disk.save(url, edge, &bytes);
            }
            Ok(image)
        }
        Err(()) => {
            // A cached small cover remains useful when a bigger view is opened
            // offline or a CDN temporarily rejects its signed URL.
            for candidate in CACHE_SIZES
                .into_iter()
                .rev()
                .filter(|candidate| !keep_aspect && *candidate < edge)
            {
                if let Ok(bytes) = disk.load(url, candidate)
                    && let Ok(image) = decode_sized(&bytes, edge)
                {
                    return Ok(image);
                }
            }
            Err(())
        }
    }
}

fn jpeg(image: &ColorImage) -> Result<Vec<u8>, ()> {
    let [width, height] = image.size;
    if width == 0
        || height == 0
        || width > HIGH_RES_TEXTURE_SIZE as usize
        || height > HIGH_RES_TEXTURE_SIZE as usize
        || image.pixels.len() != width * height
    {
        return Err(());
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for pixel in &image.pixels {
        let [r, g, b, alpha] = pixel.to_srgba_unmultiplied();
        let alpha = u32::from(alpha);
        for (channel, background) in [(r, 23_u32), (g, 27_u32), (b, 33_u32)] {
            rgb.push(((u32::from(channel) * alpha + background * (255 - alpha)) / 255) as u8);
        }
    }
    let mut bytes = Vec::new();
    let quality = if width > TEXTURE_SIZE as usize || height > TEXTURE_SIZE as usize {
        90
    } else {
        68
    };
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode(
            &rgb,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|_| ())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_palette_recognizes_dominant_hues_and_ignores_bright_outliers() {
        for color in [
            Color32::from_rgb(220, 30, 30),
            Color32::from_rgb(30, 220, 30),
            Color32::from_rgb(0, 0, 255),
            Color32::from_rgb(255, 255, 0),
        ] {
            let mut pixels = vec![color; 32 * 32];
            for pixel in pixels.iter_mut().take(80) {
                *pixel = Color32::WHITE;
            }
            for pixel in pixels.iter_mut().skip(80).take(8) {
                *pixel = Color32::from_rgb(220, 30, 220);
            }
            let actual = representative_color(&ColorImage::new([32, 32], pixels)).unwrap();
            assert!(
                actual.r().abs_diff(color.r()) <= 3
                    && actual.g().abs_diff(color.g()) <= 3
                    && actual.b().abs_diff(color.b()) <= 3
            );
        }
    }

    #[test]
    fn monochrome_art_stays_neutral_and_transparent_images_have_no_color() {
        let mut image = ColorImage::new([32, 32], vec![Color32::from_gray(150); 32 * 32]);
        image.pixels[0] = Color32::from_rgb(240, 20, 20);
        let gray = representative_color(&image).unwrap();
        assert_eq!(gray.r(), gray.g());
        assert_eq!(gray.g(), gray.b());
        assert!((148..=150).contains(&gray.r()));
        assert_eq!(
            representative_color(&ColorImage::new([1, 1], vec![Color32::TRANSPARENT])),
            None
        );
        assert_eq!(
            representative_color(&ColorImage::new(
                [1, 1],
                vec![Color32::from_rgb(10, 90, 180)]
            )),
            Some(Color32::from_rgb(10, 90, 180))
        );
    }

    #[test]
    fn a_failed_cover_is_asked_for_again_only_after_a_while() {
        let context = egui::Context::default();
        let mut cache = CoverCache::new();
        // Not a valid cover address: no job, no network, just a failure.
        let key = Key {
            url: "ftp://images.example.test/cover.jpg".into(),
            edge: HIGH_RES_TEXTURE_SIZE,
        };
        cache.request(&context, key.clone());
        let failed_at = |cache: &CoverCache| match cache.entries[&key].state {
            State::Failed(at) => at,
            _ => panic!("failed"),
        };
        let first = failed_at(&cache);
        cache.request(&context, key.clone());
        assert_eq!(failed_at(&cache), first, "not again at once");
        let long_ago = std::time::Instant::now() - FAILED_RETRY - Duration::from_secs(1);
        cache.entries.get_mut(&key).unwrap().state = State::Failed(long_ago);
        cache.request(&context, key.clone());
        assert!(failed_at(&cache) > long_ago, "asked again");
    }

    #[test]
    fn header_color_reuses_the_same_ready_thumbnail_without_new_jobs() {
        let context = egui::Context::default();
        let mut cache = CoverCache::new();
        let url = "https://images.example.test/synthetic.jpg";
        let color = Color32::from_rgb(180, 50, 35);
        cache.seed_image_for_test(
            &context,
            url,
            180.0,
            ColorImage::new([32, 32], vec![color; 32 * 32]),
        );
        assert_eq!(cache.header_color(&context, Some(url), 180.0), Some(color));
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            cache.show(ui, Some(url), 180.0);
        });
        output.textures_delta.clear();
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.pending, 0);
    }

    #[test]
    fn rebind_reuses_pixels_and_palette_and_receives_pending_covers_in_the_new_context() {
        let old_context = egui::Context::default();
        let new_context = egui::Context::default();
        let mut cache = CoverCache::new();
        let late_job = cache.repaint_target.with_context(&old_context);
        old_context.request_repaint();
        let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = notifications.clone();
        new_context.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        let url = "https://images.example.test/resident.jpg";
        let color = Color32::from_rgb(120, 60, 35);
        cache.seed_image_for_test(
            &old_context,
            url,
            180.0,
            ColorImage::filled([32, 32], color),
        );
        let resident = Key {
            url: url.into(),
            edge: texture_edge(180.0),
        };
        let (old_id, pixels) = match &cache.entries[&resident].state {
            State::Ready(texture, _, Some(pixels)) => (texture.id(), Arc::downgrade(pixels)),
            _ => unreachable!(),
        };
        // Mimic a reply completing while the old native renderer is retiring.
        let pending = Key {
            url: "https://images.example.test/pending.jpg".into(),
            edge: 96,
        };
        cache.entries.insert(
            pending.clone(),
            Entry {
                state: State::Pending,
                touched: 0,
            },
        );
        cache.pending = 1;
        let (sender, replies) = mpsc::sync_channel(1);
        cache.replies = replies;
        sender
            .send(Reply {
                key: pending.clone(),
                image: Some(ColorImage::filled([96, 96], Color32::BLUE)),
                color: Some(Color32::BLUE),
            })
            .unwrap();
        cache.rebind_context(&new_context);
        late_job.request_repaint();
        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert!(old_context.tex_manager().read().meta(old_id).is_none());
        let State::Ready(texture, palette, Some(rebound_pixels)) = &cache.entries[&resident].state
        else {
            panic!("Resident cover disappeared")
        };
        assert_eq!(*palette, Some(color));
        assert!(Arc::ptr_eq(&pixels.upgrade().unwrap(), rebound_pixels));
        assert!(
            new_context
                .tex_manager()
                .read()
                .meta(texture.id())
                .is_some()
        );
        // Arrived after the switch: uploaded, its pixels not kept.
        assert!(matches!(
            cache.entries[&pending].state,
            State::Ready(_, _, None)
        ));
        assert_eq!(cache.pending, 0);
        assert!(!cache.canceled.load(Ordering::Acquire));
        assert!(
            cache.jobs.is_none() && cache.workers.is_empty(),
            "No fetch/disk worker is needed to rebind ready pixels"
        );
        let mut uploaded = new_context.tex_manager().write().take_delta();
        assert!(uploaded.set.get(&texture.id()).is_some_and(|images| matches!(&images[0].image,egui::ImageData::Color(image) if Arc::ptr_eq(image,rebound_pixels))));
        uploaded.clear();
        old_context.tex_manager().write().take_delta().clear();
        drop(cache);
        new_context.tex_manager().write().take_delta().clear();
    }

    #[test]
    fn only_the_cover_an_effect_reads_keeps_its_pixels_in_memory() {
        let context = egui::Context::default();
        let mut cache = CoverCache::new();
        let (sender, replies) = mpsc::sync_channel(4);
        cache.replies = replies;
        let deliver = |cache: &mut CoverCache, url: &str, edge: u32| {
            let key = Key {
                url: url.into(),
                edge,
            };
            cache.entries.insert(
                key.clone(),
                Entry {
                    state: State::Pending,
                    touched: 0,
                },
            );
            cache.pending += 1;
            sender
                .send(Reply {
                    key: key.clone(),
                    image: Some(ColorImage::filled([8, 8], Color32::RED)),
                    color: Some(Color32::RED),
                })
                .unwrap();
            cache.receive(&context);
            key
        };
        // A list cover is only a texture once uploaded.
        let listed = deliver(&mut cache, "https://images.example.test/list.jpg", 96);
        assert!(matches!(
            cache.entries[&listed].state,
            State::Ready(_, _, None)
        ));

        // The backdrop's cover keeps its pixels, and it alone.
        let first = "https://images.example.test/first.jpg";
        let edge = texture_edge(512.0);
        assert!(cache.ready_image(&context, Some(first), 512.0).is_none());
        cache.entries.remove(&Key {
            url: first.into(),
            edge,
        });
        cache.pending = 0;
        let first_key = deliver(&mut cache, first, edge);
        assert!(cache.ready_image(&context, Some(first), 512.0).is_some());
        let second = "https://images.example.test/second.jpg";
        let _ = cache.ready_image(&context, Some(second), 512.0);
        assert!(
            matches!(cache.entries[&first_key].state, State::Ready(_, _, None)),
            "the previous cover lets its pixels go"
        );

        // A new window drops covers without pixels; they load again later.
        let new_context = egui::Context::default();
        cache.rebind_context(&new_context);
        assert!(!cache.entries.contains_key(&listed));
        assert!(!cache.entries.contains_key(&first_key));
        context.tex_manager().write().take_delta().clear();
        new_context.tex_manager().write().take_delta().clear();
    }

    #[test]
    fn avatar_masks_the_texture_and_keeps_button_clicks() {
        fn circular_texture(shape: &egui::Shape, texture: egui::TextureId) -> bool {
            match shape {
                egui::Shape::Rect(rect) => {
                    rect.brush
                        .as_ref()
                        .is_some_and(|brush| brush.fill_texture_id == texture)
                        && rect.corner_radius == egui::CornerRadius::same(19)
                        && rect.rect.size() == egui::vec2(38.0, 38.0)
                }
                egui::Shape::Vec(shapes) => {
                    shapes.iter().any(|shape| circular_texture(shape, texture))
                }
                _ => false,
            }
        }
        let context = egui::Context::default();
        theme::install(&context);
        let texture = context.load_texture(
            "synthetic-avatar",
            ColorImage::filled([2, 2], egui::Color32::WHITE),
            TextureOptions::LINEAR,
        );
        let texture_id = texture.id();
        let mut target = egui::Pos2::ZERO;
        let mut clicked = false;
        for frame in 0..4 {
            let mut events = Vec::new();
            if frame >= 2 {
                events.push(egui::Event::PointerMoved(target));
                events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: frame == 2,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(200.0, 100.0),
                    )),
                    time: Some(f64::from(frame) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let response = avatar_button(ui, Some(&texture), "AB", 38.0);
                    target = response.rect.center();
                    clicked |= response.clicked();
                },
            );
            output.textures_delta.clear();
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| circular_texture(&shape.shape, texture_id))
            );
        }
        assert!(clicked);
    }

    #[test]
    fn cover_requests_allow_only_public_https_without_credentials() {
        for value in [
            "http://example.com/a.jpg",
            "file:///tmp/a.jpg",
            "https://localhost/a.jpg",
            "https://127.0.0.1/a.jpg",
            "https://[::1]/a.jpg",
            "https://album.local/a.jpg",
            "https://user:secret@example.com/a.jpg",
            "https://example.com:8443/a.jpg",
        ] {
            assert!(
                !Url::parse(value).is_ok_and(|url| valid_url(&url)),
                "{value}"
            );
        }
        assert!(valid_url(
            &Url::parse("https://sun9-1.userapi.com/a.jpg?signature=synthetic").unwrap()
        ));
    }

    #[test]
    fn decodes_small_art_but_rejects_invalid_and_oversized_input() {
        let image = image::RgbaImage::from_pixel(2, 1, image::Rgba([20, 40, 80, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        let decoded = decode(bytes.get_ref()).unwrap();
        assert_eq!(decoded.size, [1, 1]);
        assert_eq!(decoded.pixels[0], egui::Color32::from_rgb(20, 40, 80));
        assert!(decode(b"<html>not an image</html>").is_err());
        assert!(decode(&vec![0; MAX_BODY as usize + 1]).is_err());
        let oversized = image::RgbaImage::new(MAX_DIMENSION + 1, 1);
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(oversized)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        assert!(decode(bytes.get_ref()).is_err());
    }

    #[test]
    fn wide_banners_keep_their_proportions_and_fit_the_edge() {
        let banner = image::RgbaImage::from_pixel(3000, 800, image::Rgba([90, 90, 90, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(banner)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        let wide = decode_sized(bytes.get_ref(), 1024 | KEEP_ASPECT).unwrap();
        assert_eq!(wide.size, [1024, 273]);
        let square = decode_sized(bytes.get_ref(), 1024).unwrap();
        assert_eq!(square.size, [800, 800], "covers stay square crops");
    }

    struct CacheFixture(std::path::PathBuf);

    impl CacheFixture {
        fn new() -> Self {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("vk-fast-cover-loader-{suffix}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn disk(&self) -> DiskCache {
            DiskCache::at(Some(self.0.join("covers")), 128 * 1024 * 1024)
        }
    }

    impl Drop for CacheFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn small_track_covers_use_96_pixels_and_compact_jpeg_files() {
        let source = image::RgbImage::from_fn(900, 700, |x, y| {
            image::Rgb([
                (x * 3 % 256) as u8,
                (y * 5 % 256) as u8,
                ((x + y) % 256) as u8,
            ])
        });
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(source)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        for (size, expected) in [(40.0, 96), (64.0, 96), (112.0, 160), (180.0, 256)] {
            let edge = texture_edge(size);
            assert_eq!(edge, expected);
            let image = decode_sized(bytes.get_ref(), edge).unwrap();
            assert_eq!(image.size, [edge as usize; 2]);
            let compressed = jpeg(&image).unwrap();
            assert_eq!(image::guess_format(&compressed).unwrap(), ImageFormat::Jpeg);
            assert!(compressed.len() < 32 * 1024);
            if edge == 96 {
                assert!(compressed.len() < 8 * 1024);
            }
        }
    }

    #[test]
    fn fullscreen_cover_fetches_original_instead_of_enlarging_a_cached_thumbnail() {
        let fixture = CacheFixture::new();
        let disk = fixture.disk();
        let url = "https://images.example.test/fullscreen.jpg";
        disk.save(
            url,
            256,
            &jpeg(&ColorImage::filled([256, 256], Color32::RED)).unwrap(),
        )
        .unwrap();
        let source = image::RgbImage::from_fn(2400, 2400, |x, y| {
            image::Rgb([(x / 6 % 256) as u8, (y / 6 % 256) as u8, 140])
        });
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(source)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        let mut fetched = false;
        let image = load_cover(&disk, url, 2048, || {
            fetched = true;
            decode_sized(bytes.get_ref(), 2048)
        })
        .unwrap();
        assert!(fetched);
        assert_eq!(image.size, [2048, 2048]);
        let restarted = fixture.disk();
        assert_eq!(
            load_cover(&restarted, url, 2048, || panic!(
                "high-resolution disk hit must avoid HTTP"
            ))
            .unwrap()
            .size,
            [2048, 2048]
        );
        assert_eq!(
            load_cover(&restarted, url, 512, || panic!(
                "larger cached art must satisfy this view"
            ))
            .unwrap()
            .size,
            [512, 512]
        );
        let offline = "https://images.example.test/offline-fullscreen.jpg";
        restarted
            .save(
                offline,
                512,
                &jpeg(&ColorImage::filled([512, 512], Color32::BLUE)).unwrap(),
            )
            .unwrap();
        assert_eq!(
            load_cover(&restarted, offline, 2048, || Err(()))
                .unwrap()
                .size,
            [512, 512]
        );
        assert!(restarted.load(offline, 2048).is_err());
    }

    #[test]
    fn full_resolution_art_keeps_a_resident_track_thumbnail_while_its_original_loads() {
        let context = egui::Context::default();
        let mut cache = CoverCache::new();
        let thumbnail = "https://images.example.test/track-small.jpg";
        cache.seed_image_for_test(
            &context,
            thumbnail,
            40.0,
            ColorImage::filled([96, 96], Color32::BLUE),
        );
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                cache.show_high_resolution(
                    ui,
                    Some("https://images.example.test/track-original.jpg"),
                    Some(thumbnail),
                    400.0,
                    14,
                );
            },
        );
        output.textures_delta.clear();
        assert!(
            output.shapes.iter().any(
                |shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.brush.is_some())
            )
        );
        assert!(cache.entries.contains_key(&Key {
            url: "https://images.example.test/track-original.jpg".into(),
            edge: 1024
        }));
    }

    #[test]
    fn fullscreen_textures_have_a_separate_memory_limit_and_preserve_small_covers() {
        let ctx = egui::Context::default();
        let mut cache = CoverCache::new();
        let small = "https://images.example.test/small-resident.jpg";
        cache.seed_image_for_test(&ctx, small, 40.0, ColorImage::filled([2, 2], Color32::BLUE));
        let (sender, replies) = mpsc::sync_channel(MAX_PENDING);
        cache.replies = replies;
        let keys: Vec<_> = (0..MAX_HIGH_RES_TEXTURES + 2)
            .map(|index| Key {
                url: format!("https://images.example.test/large-{index}.jpg"),
                edge: HIGH_RES_TEXTURE_SIZE,
            })
            .collect();
        for (index, key) in keys.iter().enumerate() {
            cache.entries.insert(
                key.clone(),
                Entry {
                    state: State::Pending,
                    touched: index as u64,
                },
            );
            sender
                .send(Reply {
                    key: key.clone(),
                    image: Some(ColorImage::filled([2, 2], Color32::RED)),
                    color: None,
                })
                .unwrap();
            cache.receive(&ctx);
            assert!(
                cache
                    .entries
                    .iter()
                    .filter(|(key, entry)| key.edge > TEXTURE_SIZE
                        && matches!(entry.state, State::Ready(..)))
                    .count()
                    <= MAX_HIGH_RES_TEXTURES
            );
        }
        assert!(!cache.entries.contains_key(&keys[0]));
        assert!(!cache.entries.contains_key(&keys[1]));
        assert!(keys[2..].iter().all(|key| cache.entries.contains_key(key)));
        assert!(cache.entries.contains_key(&Key {
            url: small.into(),
            edge: 96
        }));
    }

    #[test]
    fn restarting_art_cache_loads_the_thumbnail_without_any_network_request() {
        let fixture = CacheFixture::new();
        let url = "https://images.example.test/cover.jpg?signature=synthetic";
        let image = ColorImage::filled([96, 96], egui::Color32::from_rgb(32, 60, 90));
        let first = load_cover(&fixture.disk(), url, 96, || Ok(image)).unwrap();
        assert_eq!(first.size, [96, 96]);
        let second = load_cover(&fixture.disk(), url, 96, || {
            panic!("disk hit must not access HTTP")
        })
        .unwrap();
        assert_eq!(second.size, [96, 96]);
        let [r, g, b, _] = second.pixels[0].to_array();
        assert!((i16::from(r) - 32).abs() <= 4);
        assert!((i16::from(g) - 60).abs() <= 4);
        assert!((i16::from(b) - 90).abs() <= 4);
    }

    #[test]
    fn bigger_cached_art_satisfies_small_views_and_small_art_survives_offline() {
        let fixture = CacheFixture::new();
        let disk = fixture.disk();
        let large_url = "https://images.example.test/large.jpg";
        let small_url = "https://images.example.test/small.jpg";
        disk.save(
            large_url,
            256,
            &jpeg(&ColorImage::filled([256, 256], egui::Color32::RED)).unwrap(),
        )
        .unwrap();
        disk.save(
            small_url,
            96,
            &jpeg(&ColorImage::filled([96, 96], egui::Color32::BLUE)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_cover(&disk, large_url, 96, || panic!(
                "larger cached art must avoid HTTP"
            ))
            .unwrap()
            .size,
            [96, 96]
        );
        assert_eq!(
            load_cover(&disk, small_url, 256, || Err(())).unwrap().size,
            [96, 96]
        );
        assert!(disk.load(small_url, 96).is_ok());
    }

    #[test]
    fn corrupted_thumbnail_is_replaced_after_a_successful_download() {
        let fixture = CacheFixture::new();
        let disk = fixture.disk();
        let url = "https://images.example.test/corrupted.jpg";
        disk.save(url, 96, b"not an image").unwrap();
        let image = load_cover(&disk, url, 96, || {
            Ok(ColorImage::filled([96, 96], egui::Color32::GREEN))
        })
        .unwrap();
        assert_eq!(image.size, [96, 96]);
        assert!(decode_sized(&disk.load(url, 96).unwrap(), 96).is_ok());
    }
}
