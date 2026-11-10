use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, Align, Color32, Frame, Layout, RichText, Sense};
use zeroize::Zeroize;

#[path = "album_titles.rs"]
mod album_titles;
#[path = "audio_upload_app.rs"]
mod audio_upload_app;

#[cfg(test)]
#[path = "current_settings_tests.rs"]
mod current_settings_tests;
#[path = "discord_rpc_app.rs"]
mod discord_rpc_app;
#[path = "library_app.rs"]
mod library_app;
#[cfg(test)]
#[path = "library_membership_tests.rs"]
mod library_membership_tests;
#[path = "lyrics_app.rs"]
mod lyrics_app;
#[path = "notice_fade.rs"]
mod notice_fade;
#[cfg(test)]
#[path = "owner_page_tests.rs"]
mod owner_page_tests;
#[path = "playlist_actions.rs"]
mod playlist_actions;
#[path = "playlist_removal.rs"]
mod playlist_removal;
#[path = "queue_app.rs"]
mod queue_app;
#[path = "row_drag.rs"]
mod row_drag;
#[path = "screen_cache_app.rs"]
mod screen_cache_app;
#[path = "system_media_app.rs"]
mod system_media_app;
#[path = "track_reorder.rs"]
mod track_reorder;
#[path = "track_sort.rs"]
mod track_sort;
#[path = "track_view_cache.rs"]
mod track_view_cache;
#[path = "tray_app.rs"]
mod tray_app;
#[cfg(test)]
#[path = "unavailable_library_tests.rs"]
mod unavailable_library_tests;
#[path = "unavailable_playback.rs"]
mod unavailable_playback;
#[cfg(any(windows, test))]
#[path = "update_app.rs"]
mod update_app;

use crate::{
    audio_cache::{AudioCache, CacheKey, CacheStats},
    avatar_cache::AvatarCache,
    cover_art::{self, CoverCache},
    detail_ui, home_ui,
    i18n::Language,
    icons::{self, Icon},
    keybinds, library_cache,
    model::{AlbumRef, ArtistRef, AudioPage, Profile, Track, demo_tracks},
    music_session_store,
    play_order::{PlaybackOrder, RepeatMode},
    playback::{PlaybackError, PlaybackState, Player},
    playback_resume, playlist_create_ui, playlist_ui, preferences, qr_auth, recent_cache,
    release_info,
    repaint_target::RepaintTarget,
    scrolling::Scrolling,
    search_swipe, search_ui, seek_bar,
    settings_search::{self, Query as SettingsQuery, Setting},
    theme::{self, ACCENT, BACKGROUND, MUTED, PRIMARY, SURFACE},
    titlebar, track_menu,
    web_audio::{WebAudioClient, WebAudioError},
    web_session::SessionCookie,
};

const PLAYER_PADDING_Y: i8 = 8;
/// Default outer widths of the left navigation and the right lyrics/queue panel.
pub(crate) const DEFAULT_PANEL_WIDTHS: [f32; 2] = [252.0, 340.0];
/// Allowed outer widths; the central page always keeps `MIN_CONTENT_WIDTH`.
/// The minimum keeps the Russian "Библиотека" heading clear of its buttons.
pub(crate) const LEFT_PANEL_WIDTHS: (f32, f32) = (238.0, 420.0);
pub(crate) const RIGHT_PANEL_WIDTHS: (f32, f32) = (260.0, 560.0);
const MIN_CONTENT_WIDTH: f32 = 380.0;
const LEFT_PANEL_ID: &str = "account";
pub(crate) const RIGHT_PANEL_ID: &str = "playback-queue-panel";
const PLAYER_TIME_GAP: f32 = 2.0;
const PLAYER_COLUMN_GAP: f32 = 16.0;
const PLAYER_SIDE_MIN_WIDTH: f32 = 224.0;
const PLAYER_TRANSPORT_WIDTH: f32 = 38.0 * 5.0 + 10.0 * 4.0;

fn write_profile_metadata(profile: &Profile) -> bool {
    #[cfg(not(test))]
    {
        music_session_store::update_profile(profile).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (profile, music_session_store::update_profile);
        true
    }
}

fn player_seek_rect(body: egui::Rect, compact: bool, time_width: f32) -> egui::Rect {
    let width = if compact {
        let minimum = PLAYER_TRANSPORT_WIDTH.max(time_width * 2.0 + PLAYER_TIME_GAP * 2.0 + 80.0);
        (body.width() - 2.0 * (PLAYER_SIDE_MIN_WIDTH + PLAYER_COLUMN_GAP))
            .max(minimum)
            .min(520.0)
            .min(body.width())
    } else {
        body.width()
    };
    egui::Rect::from_center_size(
        egui::pos2(
            body.center().x,
            body.bottom() - seek_bar::CONTROL_HEIGHT * 0.5,
        ),
        egui::vec2(width, seek_bar::CONTROL_HEIGHT),
    )
}

fn save_seek_layout(compact: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_compact_seek_bar(compact).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (compact, preferences::save_compact_seek_bar);
        true
    }
}

fn save_volume_value(volume: f32) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_volume(volume).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (volume, preferences::save_volume);
        true
    }
}

fn save_titlebar_choice(system: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_system_titlebar(system).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (system, preferences::save_system_titlebar);
        true
    }
}

fn save_click_behavior(double_click: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_double_click_to_play(double_click).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (double_click, preferences::save_double_click_to_play);
        true
    }
}

fn save_normalization(enabled: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_normalize_volume(enabled).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (enabled, preferences::save_normalize_volume);
        true
    }
}

fn save_tray_preference(enabled: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_minimize_to_tray(enabled).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (enabled, preferences::save_minimize_to_tray);
        true
    }
}

fn save_logging_preference(enabled: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_logging_enabled(enabled).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (enabled, preferences::save_logging_enabled);
        true
    }
}

fn configure_logging(enabled: bool) -> bool {
    #[cfg(not(test))]
    {
        crate::diagnostics::set_enabled(enabled).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (enabled, crate::diagnostics::set_enabled);
        true
    }
}

fn save_discord_preference(enabled: bool) -> bool {
    #[cfg(not(test))]
    {
        preferences::save_discord_rpc_enabled(enabled).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (enabled, preferences::save_discord_rpc_enabled);
        true
    }
}

fn track_double_click(ui: &egui::Ui, response: &egui::Response, eligible: bool) -> bool {
    if !ui.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary)) {
        return false;
    }
    let state_id = egui::Id::new("track-row-click-pair");
    let now = ui.input(|input| input.time);
    let limits = ui.ctx().options(|options| {
        (
            options.input_options.max_double_click_delay,
            options.input_options.max_click_dist,
        )
    });
    ui.ctx().data_mut(|data| {
        let previous = data.get_temp::<(egui::Id, egui::Pos2, f64)>(state_id);
        if !eligible {
            if previous.is_some_and(|(id, _, _)| id == response.id) {
                data.remove::<(egui::Id, egui::Pos2, f64)>(state_id);
            }
            return false;
        }
        let Some(position) = response.interact_pointer_pos() else {
            return false;
        };
        let paired = previous.is_some_and(|(id, point, at)| {
            id == response.id
                && now >= at
                && now - at < limits.0
                && point.distance(position) < limits.1
        });
        if paired {
            data.remove::<(egui::Id, egui::Pos2, f64)>(state_id);
        } else {
            data.insert_temp(state_id, (response.id, position, now));
        }
        paired
    })
}

fn release_pointer_focus(response: &egui::Response) {
    if response
        .ctx
        .input(|input| input.pointer.button_released(egui::PointerButton::Primary))
        && (response.clicked_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
    {
        response
            .ctx
            .memory_mut(|memory| memory.surrender_focus(response.id));
        let frame = response.ctx.cumulative_frame_nr();
        response.ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new("player-pointer-release-frame"), frame)
        });
    }
}

#[derive(Clone, Copy)]
struct PlayerTimeSlots {
    key: Option<(i64, i64)>,
    duration: u32,
    width: f32,
}

fn write_library_snapshot(owner: i64, tracks: &[Track]) -> bool {
    #[cfg(not(test))]
    {
        library_cache::save(owner, tracks).is_ok()
    }
    #[cfg(test)]
    {
        // UI tests use synthetic accounts; filesystem behavior is covered by
        // library_cache's isolated temporary-directory tests.
        let _ = (owner, tracks, library_cache::save);
        true
    }
}

fn read_recent_snapshot(owner: i64) -> Vec<Track> {
    #[cfg(not(test))]
    {
        recent_cache::load(owner).unwrap_or_default()
    }
    #[cfg(test)]
    {
        let _ = (owner, recent_cache::load);
        Vec::new()
    }
}

fn write_recent_snapshot(owner: i64, tracks: &[Track]) -> bool {
    #[cfg(not(test))]
    {
        recent_cache::save(owner, tracks).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (owner, tracks, recent_cache::save);
        true
    }
}

fn read_playback_resume(owner: i64) -> Option<playback_resume::Snapshot> {
    #[cfg(not(test))]
    {
        playback_resume::load(owner).ok().flatten()
    }
    #[cfg(test)]
    {
        let _ = (owner, playback_resume::load);
        None
    }
}

fn write_playback_resume(owner: i64, snapshot: &playback_resume::Snapshot) -> bool {
    #[cfg(not(test))]
    {
        playback_resume::save(owner, snapshot).is_ok()
    }
    #[cfg(test)]
    {
        let _ = (owner, snapshot, playback_resume::save);
        true
    }
}

struct Session {
    profile: Profile,
    cache_warning: bool,
    music: Option<WebAudioClient>,
}

struct RefreshedProfile {
    profile: Profile,
    cache_warning: bool,
}

enum Reply {
    Connected(
        Box<Session>,
        Option<UiError>,
        bool,
        Vec<Track>,
        Vec<Track>,
        Option<Box<playback_resume::Snapshot>>,
    ),
    NoSavedSession,
    Audio(AudioPage, bool, Option<RefreshedProfile>),
}

/// Sizes and places of what VK Fast keeps on this computer, for Settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StorageInfo {
    covers: Option<u64>,
    covers_path: Option<std::path::PathBuf>,
    audio_path: Option<std::path::PathBuf>,
    history: Option<u64>,
    history_path: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageAction {
    Info,
    ClearCovers,
}

enum CacheAction {
    Info,
    Apply(u64),
    Clear,
}

#[derive(Clone, Default, PartialEq, Eq)]
enum MusicPage {
    Home,
    Search(String),
    #[default]
    Library,
    Recent,
    Collection(String),
    Artist(ArtistRef),
    Album(AlbumRef),
    Playlist(AlbumRef),
    Owner(crate::web_audio::PlaylistCreator),
    TrackAlbum((i64, i64), String),
}

impl MusicPage {
    fn detail_key(&self) -> Option<String> {
        match self {
            Self::Artist(artist) => Some(if artist.id.is_empty() {
                format!("artist-name:{}", artist.name.to_lowercase())
            } else {
                format!("artist:{}", artist.id)
            }),
            Self::Album(album) => Some(format!("album:{}_{}", album.owner_id, album.id)),
            Self::Playlist(playlist) => {
                Some(format!("playlist:{}_{}", playlist.owner_id, playlist.id))
            }
            Self::TrackAlbum(key, _) => Some(format!("track-album:{}_{}", key.0, key.1)),
            Self::Owner(creator) => Some(format!("owner:{}", creator.owner_id)),
            _ => None,
        }
    }
}

#[derive(Clone)]
enum DetailPage {
    Artist(crate::web_audio::ArtistPage),
    Album(crate::web_audio::AlbumPage),
    Playlist(crate::web_audio::AlbumPage),
    Owner(crate::web_audio::OwnerPage),
}

impl DetailPage {
    fn tracks(&self) -> &[Track] {
        match self {
            Self::Artist(page) => &page.tracks,
            Self::Album(page) | Self::Playlist(page) => &page.tracks,
            Self::Owner(page) => &page.tracks,
        }
    }
}

type LibraryTailReceiver = Receiver<Result<(usize, Vec<Track>), WebAudioError>>;

struct DetailRequest {
    owner: i64,
    key: String,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<DetailPage, WebAudioError>>,
}

struct MetadataRequest {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Vec<Track>, WebAudioError>>,
}

struct SearchRequest<T> {
    owner: i64,
    query: String,
    offset: usize,
    generation: u64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<T, WebAudioError>>,
}

impl<T> Drop for SearchRequest<T> {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

#[derive(Default)]
struct SearchState {
    input: String,
    popular: Vec<String>,
    popular_requested: bool,
    popular_error: Option<WebAudioError>,
    popular_request: Option<SearchRequest<Vec<String>>>,
    hints: Vec<String>,
    hints_for: String,
    hints_error: Option<WebAudioError>,
    hints_request: Option<SearchRequest<Vec<String>>>,
    hints_due: Option<Instant>,
    selected_hint: Option<usize>,
    generation: u64,
    results: Option<Arc<crate::web_audio::SearchResults>>,
    results_query: String,
    results_error: Option<WebAudioError>,
    failed_offset: usize,
    category: SearchCategory,
    swipe: search_swipe::Swipe,
    results_request: Option<SearchRequest<crate::web_audio::SearchResults>>,
    cache: HashMap<String, Arc<crate::web_audio::SearchResults>>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SearchCategory {
    #[default]
    All,
    Tracks,
    Artists,
    Albums,
    Playlists,
}

impl SearchState {
    fn edit(&mut self) {
        self.swipe.clear();
        self.generation = self.generation.wrapping_add(1);
        self.hints_request = None;
        self.results_request = None;
        self.hints.clear();
        self.hints_for.clear();
        self.hints_error = None;
        self.hints_due = None;
        self.selected_hint = None;
        self.results_error = None;
        self.failed_offset = 0;
    }

    fn cancel_requests(&mut self) {
        self.swipe.clear();
        self.generation = self.generation.wrapping_add(1);
        self.popular_request = None;
        self.hints_request = None;
        self.results_request = None;
        self.hints_due = None;
        if self.popular.is_empty() && self.popular_error.is_none() {
            self.popular_requested = false;
        }
        self.hints_for.clear();
    }
}

fn search_reply<T>(
    request: &mut Option<SearchRequest<T>>,
) -> Option<(SearchRequest<T>, Result<T, WebAudioError>)> {
    let pending = request.as_ref()?;
    let result = match pending.receiver.try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return None,
        Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
    };
    Some((request.take()?, result))
}

impl Drop for MetadataRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

impl Drop for DetailRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

/// The page a VK Music link opens: a track opens its album or single.
fn link_page(target: crate::link_router::Target) -> MusicPage {
    match target {
        crate::link_router::Target::Track(key) => MusicPage::TrackAlbum(key, String::new()),
        crate::link_router::Target::Album(album) => MusicPage::Album(album),
        crate::link_router::Target::Playlist(playlist) => MusicPage::Playlist(playlist),
        crate::link_router::Target::Artist(artist) => MusicPage::Artist(artist),
    }
}

struct LinkTrackRequest {
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Track, WebAudioError>>,
}

type LinkHandlerReply = (
    Option<Result<(), crate::link_router::RouterError>>,
    crate::link_router::Status,
);

enum TrackAction {
    Menu(Track, track_menu::Action),
    /// A row of the open list dropped at a new place.
    Reorder(row_drag::Move),
    ToggleLike(Track),
    Play((i64, i64)),
    Artist(ArtistRef),
    Album(AlbumRef),
    TrackAlbum((i64, i64), String),
}

enum LibraryChange {
    AlreadyAdded,
    Added((i64, i64)),
    Removed(Vec<(i64, i64)>),
}

/// A track just removed from My music, kept to restore it in place.
struct LibraryUndo {
    owner: i64,
    row: Track,
    index: Option<usize>,
    until: Instant,
}

struct LikeRequest {
    owner: i64,
    source: Track,
    /// Where an undone removal returns in the visible library.
    restore_index: Option<usize>,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<LibraryChange, WebAudioError>>,
}

struct DislikeRequest {
    owner: i64,
    key: (i64, i64),
    dislike: bool,
    receiver: Receiver<Result<(), WebAudioError>>,
}

struct PlaylistAddRequest {
    owner: i64,
    source: Track,
    target: AlbumRef,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<(i64, i64), WebAudioError>>,
}

struct PlaylistCreateForm {
    name: String,
    focus_name: bool,
    error: Option<WebAudioError>,
}

struct PlaylistCreateRequest {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<crate::web_audio::UserPlaylist, WebAudioError>>,
}

impl Drop for PlaylistCreateRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

impl Drop for PlaylistAddRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

impl Drop for LikeRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

enum HomeReply {
    Catalog(Vec<crate::web_audio::HomeCollection>),
    Tracks(String, Vec<Track>),
}

struct HomeRequest {
    owner: i64,
    collection: Option<String>,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<HomeReply, WebAudioError>>,
}

/// VK's catalog Home blocks load beside the legacy Home, so neither waits
/// for the other.
struct CatalogRequest {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<crate::web_audio::HomeCatalog, WebAudioError>>,
}

impl Drop for CatalogRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

struct PlaylistRequest {
    owner: i64,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<Vec<crate::web_audio::UserPlaylist>, WebAudioError>>,
}

impl Drop for PlaylistRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

impl Drop for HomeRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

#[derive(Clone, PartialEq, Eq)]
struct NavigationLocation {
    page: MusicPage,
    settings: bool,
    section: SettingsSection,
    filter: String,
}

struct MixRequest {
    owner: i64,
    start: bool,
    seed: Option<MixSeed>,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<crate::web_audio::MixPage, WebAudioError>>,
}

impl Drop for MixRequest {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

type MixSettingsReceiver = Receiver<Result<crate::web_audio::MixSettings, WebAudioError>>;

/// The "Configure VK Mix" dialog: VK's options load in the background.
#[derive(Default)]
struct MixSettingsDialog {
    open: bool,
    request: Option<(Arc<AtomicBool>, MixSettingsReceiver)>,
    settings: Option<crate::web_audio::MixSettings>,
    selection: Vec<(String, String)>,
    error: Option<WebAudioError>,
}

impl MixSettingsDialog {
    fn close(&mut self) {
        if let Some((canceled, _)) = self.request.take() {
            canceled.store(true, Ordering::Release);
        }
        *self = Self::default();
    }
}

/// Disk sizes in binary units, like the cache limit.
fn format_size(bytes: u64, language: Language) -> String {
    let units = [
        language.text("Б", "B"),
        language.text("КБ", "KB"),
        language.text("МБ", "MB"),
        language.text("ГБ", "GB"),
    ];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", units[0])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// VK's player picks a random 13-digit id for a mix session (and for a set
/// of VK Mix choices) and reuses it for every batch.
fn random_mix_session() -> u64 {
    let random = getrandom::u64().unwrap_or_else(|_| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64)
    });
    1_000_000_000_000 + random % 9_000_000_000_000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MixSeed {
    Track((i64, i64)),
    Playlist((i64, i64)),
    /// A Home genre, mood or artist mix; the definition is `catalog_mix`.
    Catalog(u64),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SettingsSection {
    #[default]
    General,
    Interface,
    Playback,
    Cache,
    About,
}

impl SettingsSection {
    fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Settings,
            Self::Interface => Icon::Interface,
            Self::Playback => Icon::Play,
            Self::Cache => Icon::Storage,
            Self::About => Icon::Info,
        }
    }
    fn title(self, language: Language) -> &'static str {
        match self {
            Self::General => language.text("Общие", "General"),
            Self::Interface => language.text("Интерфейс", "Interface"),
            Self::Playback => language.text("Воспроизведение", "Playback"),
            Self::Cache => language.text("Кэш", "Cache"),
            Self::About => language.text("О приложении", "About"),
        }
    }
}

enum UiError {
    Music(WebAudioError),
    MusicStorage,
    Qr(qr_auth::Error),
    QrCode(qr_auth::Error),
    QrImage,
    Playback(PlaybackError),
    NoStream,
    WorkerStart,
    Interrupted,
}

impl UiError {
    fn message(&self, language: Language) -> String {
        match self {
            Self::Music(error) => error.localized(language),
            Self::Playback(error) => error.localized(language),
            Self::Qr(error) => error.localized(language),
            Self::QrCode(qr_auth::Error::IncorrectCode) => {
                qr_auth::Error::IncorrectCode.localized(language)
            }
            Self::QrCode(error) => {
                let text = language.text("Не удалось проверить код. Повторите ввод — текущий QR-код остаётся действующим.", "Could not verify the code. Try entering it again; the current QR code remains active.");
                match error {
                    qr_auth::Error::Api(code) => format!("{text} (VK {code})"),
                    qr_auth::Error::Http(code) => format!("{text} (HTTP {code})"),
                    _ => text.into(),
                }
            }
            Self::QrImage => language
                .text(
                    "Не удалось отобразить QR-код. Обновите его.",
                    "Could not display the QR code. Refresh it.",
                )
                .into(),
            Self::NoStream => language
                .text(
                    "Для этого трека нет доступной аудиоссылки.",
                    "No playable audio is available for this track.",
                )
                .into(),
            Self::MusicStorage => language
                .text(
                    "Не удалось прочитать сохранённый вход. Войдите по QR-коду заново.",
                    "Could not read the saved session. Sign in with the QR code again.",
                )
                .into(),
            Self::WorkerStart => language
                .text(
                    "Не удалось запустить сетевой запрос.",
                    "Could not start the network request.",
                )
                .into(),
            Self::Interrupted => language
                .text(
                    "Сетевой запрос был прерван. Повторите попытку.",
                    "The network request was interrupted. Try again.",
                )
                .into(),
        }
    }
}

struct ResolvedTrack {
    track: Track,
    url: Option<String>,
    cache_key: CacheKey,
}

struct QrConnected {
    profile: Profile,
    cookies: Vec<SessionCookie>,
    music: WebAudioClient,
    page: AudioPage,
    cached: Vec<Track>,
    recent: Vec<Track>,
    resume: Option<playback_resume::Snapshot>,
}

type QrWindowLayout = (
    egui::Rect,
    egui::Rect,
    Option<[egui::Rect; 2]>,
    [egui::Rect; 2],
);

fn time_label(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn bounded_track_position(position: f64, duration: u32) -> f64 {
    if !position.is_finite() {
        return 0.0;
    }
    position.clamp(
        0.0,
        f64::from(if duration == 0 { u32::MAX } else { duration }),
    )
}

fn time_slot_width(ui: &egui::Ui, duration: u32) -> f32 {
    let font = egui::TextStyle::Small.resolve(ui.style());
    ui.fonts_mut(|fonts| {
        let widest_digit = ('0'..='9')
            .max_by(|left, right| {
                fonts
                    .glyph_width(&font, *left)
                    .total_cmp(&fonts.glyph_width(&font, *right))
            })
            .unwrap_or('8');
        let template = time_label(f64::from(duration))
            .chars()
            .map(|character| {
                if character.is_ascii_digit() {
                    widest_digit
                } else {
                    character
                }
            })
            .collect();
        fonts.layout_no_wrap(template, font, MUTED).size().x.ceil() + 1.0
    })
}

fn track_index_width(ui: &egui::Ui, count: usize) -> f32 {
    let id = egui::Id::new("track-index-width");
    if let Some((cached_count, width)) = ui.ctx().data(|data| data.get_temp::<(usize, f32)>(id))
        && cached_count == count
    {
        return width;
    }
    let font = egui::TextStyle::Small.resolve(ui.style());
    let width = ui.fonts_mut(|fonts| {
        let widest = ('0'..='9')
            .map(|digit| fonts.glyph_width(&font, digit))
            .fold(0.0, f32::max);
        (widest * count.max(1).to_string().len() as f32 + 3.0)
            .ceil()
            .max(24.0)
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(id, (count, width)));
    width
}

fn paint_time(ui: &egui::Ui, rect: egui::Rect, text: String, align: egui::Align2, id: &str) {
    let anchor = if align == egui::Align2::RIGHT_CENTER {
        rect.right_center()
    } else {
        rect.left_center()
    };
    ui.painter().text(
        anchor,
        align,
        &text,
        egui::TextStyle::Small.resolve(ui.style()),
        MUTED,
    );
    ui.interact(rect, ui.id().with(id), Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), &text));
}

/// Pointer changes are only previews; keyboard and accessibility changes can
/// commit immediately. An unpaired release outside the control still finishes
/// a captured gesture, while losing focus/capture cancels it.
fn seek_commit(
    preview: &mut Option<f64>,
    position: f64,
    duration: f64,
    response: &egui::Response,
) -> Option<f64> {
    let (focused, released, primary_down) = response.ctx.input(|input| {
        (
            input.focused,
            input.pointer.button_released(egui::PointerButton::Primary),
            input.pointer.primary_down(),
        )
    });
    if !response.enabled() || !focused {
        *preview = None;
        return None;
    }
    if released {
        let captured = preview.take().is_some();
        return (captured || response.clicked())
            .then(|| seek_bar::position_at_pointer(response, duration).unwrap_or(position));
    }
    if (response.is_pointer_button_down_on() && primary_down)
        || response.dragged_by(egui::PointerButton::Primary)
    {
        *preview = Some(position);
        return None;
    }
    if preview.is_some() {
        if !primary_down {
            *preview = None;
        }
        return None;
    }
    response.changed().then_some(position)
}

fn qr_image(ctx: &egui::Context, url: &str) -> Option<egui::TextureHandle> {
    let qr = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let scale = 4;
    let side = (qr.width() + 8) * scale;
    let mut pixels = vec![Color32::WHITE; side * side];
    for y in 0..qr.width() {
        for x in 0..qr.width() {
            if qr[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        pixels[((y + 4) * scale + dy) * side + (x + 4) * scale + dx] =
                            Color32::from_rgb(20, 31, 44);
                    }
                }
            }
        }
    }
    Some(ctx.load_texture(
        "vk-fast-qr",
        egui::ColorImage::new([side, side], pixels),
        egui::TextureOptions::NEAREST,
    ))
}

#[derive(Default)]
struct Library {
    tracks: Vec<Track>,
    total: Option<usize>,
    offset: usize,
    fresh_prefix: usize,
    exhausted: bool,
    /// The signed-in account. Rows its heart would show as not saved are
    /// kept out: VK's music page can list recently played tracks there.
    owner: Option<i64>,
}

impl Library {
    fn from_cached(tracks: Vec<Track>) -> Self {
        Self {
            tracks,
            ..Self::default()
        }
    }

    /// The same rule as the heart: VK's saved mark, else the account's own copy.
    fn saved(owner: i64, track: &Track) -> bool {
        track.library_liked.unwrap_or(track.owner_id == owner)
    }

    fn for_owner(mut self, owner: i64) -> Self {
        self.owner = Some(owner);
        self.tracks.retain(|track| Self::saved(owner, track));
        self
    }

    fn refresh(&mut self, page: AudioPage) {
        if page.count == Some(0) && page.items.is_empty() {
            self.tracks.clear();
        }
        self.offset = 0;
        self.fresh_prefix = 0;
        self.exhausted = false;
        self.append(page);
    }

    fn append(&mut self, mut page: AudioPage) {
        // VK's offsets count every row it sent, shown or not.
        let received = page.items.len();
        if let Some(owner) = self.owner {
            page.items.retain(|track| Self::saved(owner, track));
            if received != page.items.len() {
                crate::diagnostics::event("library", "unsaved_rows_hidden");
            }
        }
        self.total = page.count.or(self.total);
        self.offset = self.offset.saturating_add(received);
        let known_metadata: HashMap<_, _> = self
            .tracks
            .iter()
            .map(|track| {
                (
                    track.key(),
                    (
                        track.unavailable,
                        track.library_source_key,
                        track.added,
                        track.album.clone(),
                    ),
                )
            })
            .collect();
        let mut old = std::mem::take(&mut self.tracks);
        let tail = old.split_off(self.fresh_prefix.min(old.len()));
        let mut known: HashMap<_, _> = old
            .iter()
            .enumerate()
            .map(|(index, track)| (track.key(), index))
            .collect();
        for mut track in page.items {
            if let Some((unavailable, source, added, album)) = known_metadata.get(&track.key()) {
                track.unavailable = track.unavailable.or(*unavailable);
                track.library_source_key = track.library_source_key.or(*source);
                // Website rows carry no dates: keep the ones already loaded.
                track.added = track.added.or(*added);
                // Nor album names: keep the ones already learned.
                album_titles::keep_known_album(&mut track, album.as_ref());
            }
            if let Some(index) = known.get(&track.key()).copied() {
                old[index] = track;
            } else {
                known.insert(track.key(), old.len());
                old.push(track);
            }
        }
        self.fresh_prefix = old.len();
        old.extend(
            tail.into_iter()
                .filter(|track| !known.contains_key(&track.key())),
        );
        self.tracks = old;
        if self.total == Some(0) && !self.tracks.is_empty() {
            self.total = None;
        }
        self.exhausted = page.exhausted.unwrap_or_else(|| {
            received == 0 || self.total.is_some_and(|total| self.offset >= total)
        });
    }
}

pub struct MusicApp {
    session: Option<Session>,
    pending: Option<Receiver<Result<Reply, UiError>>>,
    /// A saved session is being restored at startup.
    restoring: bool,
    workers: Vec<thread::JoinHandle<()>>,
    library: Library,
    track_view_cache: track_view_cache::TrackViewCache,
    page: MusicPage,
    scrolling: Scrolling,
    back_history: Vec<NavigationLocation>,
    forward_history: Vec<NavigationLocation>,
    last_history_frame: Option<u64>,
    recent: Vec<Track>,
    recent_save_pending: Option<Receiver<bool>>,
    recent_save_current: Option<(i64, Arc<Vec<Track>>)>,
    recent_save_queue: HashMap<i64, Arc<Vec<Track>>>,
    recent_latest: HashMap<i64, Arc<Vec<Track>>>,
    play_queue: Option<Vec<Track>>,
    play_order: PlaybackOrder,
    playback_modes_writer: crate::playback_modes::Writer,
    queued_tracks: HashMap<(i64, i64), Track>,
    queue_ui: queue_app::PanelState,
    lyrics: lyrics_app::State,
    queue_origin: Option<String>,
    home_collections: Vec<home_ui::Collection>,
    home_sources: Vec<crate::web_audio::HomeCollection>,
    home_catalog: crate::web_audio::HomeCatalog,
    catalog_request: Option<CatalogRequest>,
    catalog_mix: Option<(u64, crate::web_audio::CatalogMix)>,
    /// The user's VK Mix settings, sent with every VK Mix batch.
    mix_tuning: Option<crate::web_audio::MixTuning>,
    mix_settings: MixSettingsDialog,
    home_request: Option<HomeRequest>,
    home_requested: bool,
    home_error: Option<WebAudioError>,
    home_autoplay: Option<String>,
    playlist_items: Vec<playlist_ui::PlaylistItem>,
    editable_playlists: Vec<AlbumRef>,
    playlist_add_request: Option<PlaylistAddRequest>,
    playlist_create_form: Option<PlaylistCreateForm>,
    playlist_create_request: Option<PlaylistCreateRequest>,
    playlist_controls: playlist_actions::State,
    library_browser: library_app::State,
    audio_upload: audio_upload_app::State,
    track_menu_error: Option<WebAudioError>,
    track_menu_notice: Option<(String, Instant)>,
    /// Undo for the notice whose deadline it shares.
    library_undo: Option<LibraryUndo>,
    /// When the shown error notice hides, and which errors it shows: a
    /// new error starts its own time.
    error_closing: Option<Instant>,
    error_shown: Option<(Option<WebAudioError>, Option<WebAudioError>)>,
    playlist_removal: playlist_removal::State,
    /// Set while a reorderable list draws its rows: rows may then be dragged.
    row_drag_enabled: bool,
    /// The rect of the row whose drag began this frame.
    row_drag_started: Option<egui::Rect>,
    reorder_request: Option<track_reorder::Request>,
    album_titles: album_titles::State,
    playlist_request: Option<PlaylistRequest>,
    playlists_requested: bool,
    playlist_error: Option<WebAudioError>,
    details: HashMap<String, Arc<DetailPage>>,
    detail_request: Option<DetailRequest>,
    detail_error: Option<WebAudioError>,
    metadata_request: Option<MetadataRequest>,
    metadata_attempted: HashSet<(i64, i64)>,
    like_request: Option<LikeRequest>,
    dislike_request: Option<DislikeRequest>,
    /// Tracks marked "Не нравится" during this session.
    disliked: HashSet<(i64, i64)>,
    like_aliases: HashMap<(i64, i64), (i64, i64)>,
    removed_from_library: HashSet<(i64, i64)>,
    like_error: Option<WebAudioError>,
    library_refresh_due: bool,
    search: SearchState,
    mix_request: Option<MixRequest>,
    mix_active: bool,
    mix_seed: Option<MixSeed>,
    mix_has_more: bool,
    mix_advance: bool,
    mix_error: Option<WebAudioError>,
    library_save_pending: Option<Receiver<bool>>,
    library_save_current: Option<(i64, Arc<Vec<Track>>)>,
    library_save_queue: HashMap<i64, Arc<Vec<Track>>>,
    filter: String,
    selected: Option<(i64, i64)>,
    auth_error: Option<UiError>,
    audio_error: Option<UiError>,
    demo: bool,
    language: Language,
    player: Player,
    unavailable_playback: unavailable_playback::State,
    system_media: system_media_app::State,
    tray: tray_app::State,
    minimize_to_tray: bool,
    tray_save_failed: bool,
    normalization_save_failed: bool,
    logging_enabled: bool,
    logging_save_failed: bool,
    logging_runtime_failed: bool,
    discord_rpc: discord_rpc_app::State,
    discord_rpc_enabled: bool,
    discord_rpc_save_failed: bool,
    /// The system link handler as last read; None until Settings load it.
    link_handler: Option<crate::link_router::Status>,
    link_handler_pending: Option<Receiver<LinkHandlerReply>>,
    link_handler_error: Option<crate::link_router::RouterError>,
    /// A VK Music link from the system, opened once music is connected.
    pending_link: Option<crate::link_router::Target>,
    /// A linked track being looked up before it starts playing.
    link_track: Option<LinkTrackRequest>,
    #[cfg(any(windows, test))]
    updater: update_app::State,
    playing: Option<Track>,
    playback_pending: Option<Receiver<Result<ResolvedTrack, UiError>>>,
    playback_cancel: Option<Arc<AtomicBool>>,
    playback_error: Option<UiError>,
    logged_playback_failure: Option<PlaybackError>,
    seek_position: Option<f64>,
    /// Committed position while a restored track waits for Play or resolution.
    resume_position: Option<f64>,
    time_slots: Option<PlayerTimeSlots>,
    finished_handled: bool,
    advance_on_load: bool,
    covers: CoverCache,
    avatars: AvatarCache,
    avatar_refresh_ready: bool,
    avatar_refresh_url: Option<String>,
    volume_save_at: Option<Instant>,
    volume_save_failed: bool,
    compact_seek_bar: bool,
    seek_layout_save_failed: bool,
    system_titlebar: bool,
    titlebar_save_failed: bool,
    double_click_to_play: bool,
    click_behavior_save_failed: bool,
    search_on_enter: bool,
    search_on_enter_save_failed: bool,
    middle_autoscroll: bool,
    middle_autoscroll_save_failed: bool,
    /// The "E" badge by explicit tracks' titles.
    explicit_badge: bool,
    explicit_badge_save_failed: bool,
    /// The fullscreen player's animated blurred backdrop.
    animated_backdrop: bool,
    animated_backdrop_save_failed: bool,
    /// Ctrl + the keyboard's volume keys change VK Fast's own volume.
    volume_keys_enabled: bool,
    volume_keys_save_failed: bool,
    volume_keys: crate::volume_keys::VolumeKeys,
    /// The seek bar's width in physical pixels when last drawn.
    seek_pixels: f32,
    autoscroll: crate::autoscroll::Autoscroll,
    /// The page area of the last frame, where a middle click autoscrolls.
    page_rect: egui::Rect,
    mix_after_queue: bool,
    mix_after_queue_save_failed: bool,
    screen_cache: Option<(i64, Receiver<screen_cache_app::Snapshot>)>,
    screen_cache_owner: Option<i64>,
    /// Known "date added" of own playlist tracks, by playlist.
    playlist_dates: HashMap<String, HashMap<(i64, i64), i64>>,
    /// Column sort per track list, by its queue origin.
    track_sorts: HashMap<String, track_sort::Sort>,
    /// The last sorted row order and what it was built from.
    sort_cache: Option<track_sort::Cached>,
    /// My music read from its end for a reversed list: tracks in VK's order
    /// starting at `library_tail_start`.
    library_tail: Vec<Track>,
    library_tail_start: Option<usize>,
    library_tail_request: Option<(i64, LibraryTailReceiver)>,
    /// Added to a row's index for its "#": the list's first real position.
    row_number_offset: usize,
    broadcast: bool,
    broadcast_save_failed: bool,
    window_reopen: bool,
    /// Outer widths of the navigation and the lyrics/queue panels, as the
    /// user last dragged them. Saved with the window geometry.
    panel_widths: [f32; 2],
    ui_context: egui::Context,
    repaint_target: RepaintTarget,
    instance_activation: Option<crate::single_instance::Activation>,
    audio_cache: Option<AudioCache>,
    cache_limit_mb: u32,
    cache_stats: Option<CacheStats>,
    cache_pending: Option<Receiver<Result<CacheStats, ()>>>,
    storage: StorageInfo,
    storage_pending: Option<Receiver<StorageInfo>>,
    cache_save_at: Option<Instant>,
    cache_error: bool,
    cache_save_failed: bool,
    settings_open: bool,
    settings_section: SettingsSection,
    settings_query: String,
    language_save_failed: bool,
    qr_control: Option<qr_auth::Control>,
    qr_events: Option<Receiver<qr_auth::Event>>,
    qr_pending: Option<Receiver<Result<QrConnected, UiError>>>,
    qr_texture: Option<egui::TextureHandle>,
    qr_url: Option<String>,
    qr_expires: Option<Instant>,
    qr_stage: qr_auth::Stage,
    qr_error: Option<UiError>,
    qr_open: bool,
    qr_code_input: String,
    #[cfg(feature = "screenshot")]
    screenshot_path: Option<std::path::PathBuf>,
    #[cfg(feature = "screenshot")]
    screenshot_requested: bool,
    #[cfg(feature = "screenshot")]
    screenshot_ready_at: Option<Instant>,
}

impl MusicApp {
    fn location(&self) -> NavigationLocation {
        NavigationLocation {
            page: self.page.clone(),
            settings: self.settings_open,
            section: self.settings_section,
            filter: self.filter.clone(),
        }
    }

    fn apply_location(
        &mut self,
        ctx: &egui::Context,
        location: NavigationLocation,
        restore_pending: bool,
    ) {
        let opening_settings = location.settings && !self.settings_open;
        if self.page.detail_key() != location.page.detail_key() {
            self.detail_request = None;
            self.detail_error = None;
        }
        if !matches!(location.page, MusicPage::Search(_)) || location.settings {
            self.search.cancel_requests();
        }
        self.scrolling.stop();
        self.page = location.page;
        self.settings_open = location.settings;
        self.settings_section = location.section;
        self.filter = location.filter;
        if opening_settings {
            self.start_cache_job(ctx, CacheAction::Info);
            self.start_storage_job(ctx, StorageAction::Info);
            self.start_link_handler_job(ctx, None);
        }
        if restore_pending
            && !self.settings_open
            && let MusicPage::Collection(id) = self.page.clone()
        {
            let empty = self
                .home_collections
                .iter()
                .find(|collection| collection.id == id)
                .is_none_or(|collection| collection.tracks.is_empty());
            let loading_this = self
                .home_request
                .as_ref()
                .is_some_and(|request| request.collection.as_deref() == Some(id.as_str()));
            if empty && !loading_this {
                self.open_collection(ctx, id, false);
            }
        }
        if !self.settings_open {
            self.ensure_detail(ctx, false);
        }
        if !self.settings_open
            && let MusicPage::Search(query) = self.page.clone()
        {
            self.restore_search(ctx, &query);
        }
        if !self.settings_open
            && self.page == MusicPage::Home
            && !self.home_requested
            && (self.library.offset > 0 || self.library.exhausted)
        {
            self.load_home(ctx);
        }
        ctx.request_repaint();
    }

    fn navigate(&mut self, ctx: &egui::Context, location: NavigationLocation) {
        let current = self.location();
        if current == location {
            return;
        }
        self.back_history.push(current);
        if self.back_history.len() > 128 {
            self.back_history.remove(0);
        }
        self.forward_history.clear();
        self.apply_location(ctx, location, false);
    }

    fn navigate_music(&mut self, ctx: &egui::Context, page: MusicPage) {
        if let MusicPage::Artist(artist) = &page
            && !crate::model::valid_artist_id(&artist.id)
            && !artist.name.trim().is_empty()
        {
            // A display name is not a canonical VK artist identity. Search the
            // whole name, including its spaces and punctuation, instead of
            // resolving a possibly unrelated artist with the same name.
            let query = artist.name.trim().chars().take(512).collect::<String>();
            self.search.input = query.clone();
            self.search.edit();
            self.search.category = SearchCategory::All;
            self.navigate_music(ctx, MusicPage::Search(query.clone()));
            self.restore_search(ctx, &query);
            return;
        }
        if !self.settings_open && self.page == page {
            return;
        }
        self.navigate(
            ctx,
            NavigationLocation {
                page,
                settings: false,
                section: self.settings_section,
                filter: String::new(),
            },
        );
    }

    fn go_back(&mut self, ctx: &egui::Context) {
        let frame = ctx.cumulative_frame_nr();
        if self.last_history_frame == Some(frame) {
            return;
        }
        if let Some(location) = self.back_history.pop() {
            self.last_history_frame = Some(frame);
            self.forward_history.push(self.location());
            self.apply_location(ctx, location, true);
        } else if self.settings_open {
            self.last_history_frame = Some(frame);
            self.forward_history.push(self.location());
            self.settings_open = false;
            self.scrolling.stop();
        }
    }

    fn go_forward(&mut self, ctx: &egui::Context) {
        let frame = ctx.cumulative_frame_nr();
        if self.last_history_frame == Some(frame) {
            return;
        }
        if let Some(location) = self.forward_history.pop() {
            self.last_history_frame = Some(frame);
            self.back_history.push(self.location());
            self.apply_location(ctx, location, true);
        }
    }

    pub fn open_settings(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            self.navigate(
                ctx,
                NavigationLocation {
                    page: self.page.clone(),
                    settings: true,
                    section: self.settings_section,
                    filter: self.filter.clone(),
                },
            );
        }
    }

    pub fn new(ctx: &egui::Context, demo: bool, language: Language) -> Self {
        theme::install(ctx);
        let cache_limit_mb = preferences::load_cache_limit_mb();
        #[cfg(not(test))]
        let audio_cache = AudioCache::new(u64::from(cache_limit_mb) * 1024 * 1024).ok();
        // UI tests inject a temporary cache and must never prune the user's cache.
        #[cfg(test)]
        let audio_cache: Option<AudioCache> = None;
        let player = audio_cache
            .clone()
            .map(Player::with_cache)
            .unwrap_or_else(Player::new);
        if let Some(volume) = preferences::load_volume() {
            player.set_volume(volume);
        }
        player.set_normalize_volume(preferences::load_normalize_volume());
        let (shuffle, repeat) = preferences::load_playback_modes();
        let mut app = Self {
            session: None,
            pending: None,
            restoring: false,
            workers: Vec::new(),
            library: Library::default(),
            track_view_cache: track_view_cache::TrackViewCache::default(),
            page: MusicPage::default(),
            scrolling: Scrolling::default(),
            back_history: Vec::new(),
            forward_history: Vec::new(),
            last_history_frame: None,
            recent: Vec::new(),
            recent_save_pending: None,
            recent_save_current: None,
            recent_save_queue: HashMap::new(),
            recent_latest: HashMap::new(),
            play_queue: None,
            play_order: PlaybackOrder::with_modes(shuffle, repeat),
            playback_modes_writer: crate::playback_modes::Writer::default(),
            queued_tracks: HashMap::new(),
            queue_ui: queue_app::PanelState::default(),
            lyrics: lyrics_app::State::default(),
            queue_origin: None,
            home_collections: Vec::new(),
            home_sources: Vec::new(),
            home_catalog: Default::default(),
            catalog_request: None,
            catalog_mix: None,
            mix_tuning: None,
            mix_settings: MixSettingsDialog::default(),
            home_request: None,
            home_requested: false,
            home_error: None,
            home_autoplay: None,
            playlist_items: Vec::new(),
            editable_playlists: Vec::new(),
            playlist_add_request: None,
            playlist_create_form: None,
            playlist_create_request: None,
            playlist_controls: playlist_actions::State::default(),
            library_browser: library_app::State::default(),
            audio_upload: audio_upload_app::State::default(),
            track_menu_error: None,
            track_menu_notice: None,
            library_undo: None,
            error_closing: None,
            error_shown: None,
            playlist_removal: playlist_removal::State::default(),
            row_drag_enabled: false,
            row_drag_started: None,
            reorder_request: None,
            album_titles: album_titles::State::default(),
            playlist_request: None,
            playlists_requested: false,
            playlist_error: None,
            details: HashMap::new(),
            detail_request: None,
            detail_error: None,
            metadata_request: None,
            metadata_attempted: HashSet::new(),
            like_request: None,
            dislike_request: None,
            disliked: HashSet::new(),
            like_aliases: HashMap::new(),
            removed_from_library: HashSet::new(),
            like_error: None,
            library_refresh_due: false,
            search: SearchState::default(),
            mix_request: None,
            mix_active: false,
            mix_seed: None,
            mix_has_more: false,
            mix_advance: false,
            mix_error: None,
            library_save_pending: None,
            library_save_current: None,
            library_save_queue: HashMap::new(),
            filter: String::new(),
            selected: None,
            auth_error: None,
            audio_error: None,
            demo: false,
            language,
            player,
            unavailable_playback: unavailable_playback::State::default(),
            system_media: system_media_app::State::default(),
            tray: tray_app::State::default(),
            minimize_to_tray: preferences::load_minimize_to_tray(),
            tray_save_failed: false,
            normalization_save_failed: false,
            logging_enabled: preferences::load_logging_enabled(),
            logging_save_failed: false,
            logging_runtime_failed: preferences::load_logging_enabled()
                && !crate::diagnostics::enabled(),
            discord_rpc: discord_rpc_app::State::default(),
            discord_rpc_enabled: preferences::load_discord_rpc_enabled(),
            discord_rpc_save_failed: false,
            link_handler: None,
            link_handler_pending: None,
            link_handler_error: None,
            pending_link: None,
            link_track: None,
            #[cfg(any(windows, test))]
            updater: if cfg!(test) {
                update_app::State::default()
            } else {
                update_app::State::new(preferences::load_auto_updates())
            },
            playing: None,
            playback_pending: None,
            playback_cancel: None,
            playback_error: None,
            logged_playback_failure: None,
            seek_position: None,
            resume_position: None,
            time_slots: None,
            finished_handled: false,
            advance_on_load: false,
            covers: CoverCache::new(),
            avatars: AvatarCache::new(),
            avatar_refresh_ready: false,
            avatar_refresh_url: None,
            volume_save_at: None,
            volume_save_failed: false,
            compact_seek_bar: preferences::load_compact_seek_bar(),
            seek_layout_save_failed: false,
            system_titlebar: preferences::load_system_titlebar(),
            titlebar_save_failed: false,
            double_click_to_play: preferences::load_double_click_to_play(),
            click_behavior_save_failed: false,
            search_on_enter: preferences::load_search_on_enter(),
            search_on_enter_save_failed: false,
            middle_autoscroll: preferences::load_middle_autoscroll(),
            middle_autoscroll_save_failed: false,
            explicit_badge: preferences::load_explicit_badge(),
            explicit_badge_save_failed: false,
            animated_backdrop: preferences::load_animated_backdrop(),
            animated_backdrop_save_failed: false,
            volume_keys_enabled: preferences::load_volume_keys(),
            volume_keys_save_failed: false,
            volume_keys: crate::volume_keys::VolumeKeys::default(),
            seek_pixels: 0.0,
            autoscroll: Default::default(),
            page_rect: egui::Rect::NOTHING,
            mix_after_queue: preferences::load_mix_after_queue(),
            mix_after_queue_save_failed: false,
            screen_cache: None,
            screen_cache_owner: None,
            playlist_dates: HashMap::new(),
            track_sorts: HashMap::new(),
            sort_cache: None,
            library_tail: Vec::new(),
            library_tail_start: None,
            library_tail_request: None,
            row_number_offset: 0,
            broadcast: preferences::load_broadcast(),
            broadcast_save_failed: false,
            window_reopen: false,
            panel_widths: DEFAULT_PANEL_WIDTHS,
            ui_context: ctx.clone(),
            repaint_target: RepaintTarget::new(ctx),
            instance_activation: None,
            audio_cache,
            cache_limit_mb,
            cache_stats: None,
            cache_pending: None,
            storage: StorageInfo::default(),
            storage_pending: None,
            cache_save_at: None,
            cache_error: false,
            cache_save_failed: false,
            settings_open: false,
            settings_section: SettingsSection::default(),
            settings_query: String::new(),
            language_save_failed: false,
            qr_control: None,
            qr_events: None,
            qr_pending: None,
            qr_texture: None,
            qr_url: None,
            qr_expires: None,
            qr_stage: qr_auth::Stage::Creating,
            qr_error: None,
            qr_open: false,
            qr_code_input: String::new(),
            #[cfg(feature = "screenshot")]
            screenshot_path: None,
            #[cfg(feature = "screenshot")]
            screenshot_requested: false,
            #[cfg(feature = "screenshot")]
            screenshot_ready_at: None,
        };
        if demo {
            app.enter_demo();
        }
        app
    }

    pub fn uses_system_titlebar(&self) -> bool {
        self.system_titlebar
    }

    pub fn window_reopen_requested(&self) -> bool {
        self.window_reopen
    }

    pub(crate) fn background_context(&self) -> egui::Context {
        self.ui_context.clone()
    }

    pub(crate) fn attach_instance(&mut self, activation: crate::single_instance::Activation) {
        activation.set_repaint_target(self.repaint_target.clone());
        self.instance_activation = Some(activation);
    }

    fn poll_instance_activation(&mut self, ctx: &egui::Context) {
        // Do not show a bootstrap-hidden window before its first UI frame has
        // reached the renderer. Keep early activation queued for the next tick.
        if ctx.cumulative_frame_nr() > 0
            && self
                .instance_activation
                .as_ref()
                .is_some_and(crate::single_instance::Activation::take_pending)
            && !self.tray_quit_requested()
        {
            self.show_application(ctx);
        }
    }

    pub fn prepare_reopened_window(&mut self, ctx: &egui::Context) {
        self.system_media.window_recreated();
        if self.ui_context != *ctx {
            let memory = self.ui_context.memory(Clone::clone);
            ctx.memory_mut(|current| *current = memory);
            ctx.data_mut(|data| {
                // Frame numbers restart with the replacement context.
                data.remove::<u64>(egui::Id::new("vk-fast-window-command-frame"));
                data.remove::<u64>(egui::Id::new("player-pointer-release-frame"));
                data.remove::<(usize, f32)>(egui::Id::new("track-index-width"));
            });
            theme::install(ctx);
            crate::app_icon::rebind(&self.ui_context);
            crate::app_icon::rebind(ctx);
            self.covers.rebind_context(ctx);
            self.avatars.rebind_context(ctx);
            self.playlist_controls.rebind(ctx);
            self.lyrics.rebind(ctx);
            self.qr_texture = self.qr_url.as_deref().and_then(|url| qr_image(ctx, url));
            // Existing worker clones share this target and wake the new window.
            self.repaint_target.set_context(ctx);
            // The former renderer has retired. Its queued texture frees must
            // not survive until the old worker context is eventually dropped.
            self.ui_context.tex_manager().write().take_delta().clear();
            self.ui_context = ctx.clone();
        }
        self.window_reopen = false;
        self.last_history_frame = None;
        self.search.swipe.clear();
        self.scrolling.stop();
        self.seek_position = None;
        self.time_slots = None;
        ctx.request_repaint();
    }

    #[cfg(feature = "screenshot")]
    pub fn with_screenshot(mut self, path: Option<std::path::PathBuf>) -> Self {
        self.screenshot_path = path;
        self
    }

    #[cfg(feature = "screenshot")]
    fn capture(&mut self, ui: &mut egui::Ui) {
        if self.cache_pending.is_some() {
            return;
        }
        let Some(path) = &self.screenshot_path else {
            return;
        };
        if matches!(self.page, MusicPage::Search(_)) {
            if self.pending.is_some()
                || self.search.popular_request.is_some()
                || self.search.hints_request.is_some()
                || self.search.results_request.is_some()
                || self.search.hints_due.is_some()
            {
                self.screenshot_ready_at = None;
                return;
            }
            let ready = self
                .screenshot_ready_at
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(1500));
            if Instant::now() < *ready {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
                return;
            }
        }
        if self.page == MusicPage::Home {
            if self.pending.is_some()
                || self.home_request.is_some()
                || self.catalog_request.is_some()
            {
                self.screenshot_ready_at = None;
                return;
            }
            let ready = self
                .screenshot_ready_at
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(1500));
            if Instant::now() < *ready {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
                return;
            }
        }
        let image = ui.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });
        if let Some(image) = image {
            let bytes: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|color| color.to_array())
                .collect();
            if let Err(error) = image::save_buffer_with_format(
                path,
                &bytes,
                image.width() as u32,
                image.height() as u32,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            ) {
                eprintln!("Could not save screenshot: {error}");
            }
            self.screenshot_path = None;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else if !self.screenshot_requested {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.screenshot_requested = true;
        }
    }

    fn reset(&mut self) {
        if let Some(request) = self.link_track.take() {
            request.canceled.store(true, Ordering::Release);
        }
        self.pending_link = None;
        self.screen_cache = None;
        self.screen_cache_owner = None;
        self.playlist_dates.clear();
        self.library_tail.clear();
        self.library_tail_start = None;
        self.library_tail_request = None;
        self.audio_upload = audio_upload_app::State::default();
        self.cancel_qr();
        self.stop_playback();
        self.play_order =
            PlaybackOrder::with_modes(self.play_order.shuffle(), self.play_order.repeat());
        self.restoring = false;
        // VK Mix settings and dislikes belong to the account that chose them.
        self.mix_tuning = None;
        self.dislike_request = None;
        self.disliked.clear();
        self.mix_settings.close();
        self.pending = None; // Discard any late response from the previous session.
        self.session = None;
        self.avatar_refresh_ready = false;
        self.avatar_refresh_url = None;
        self.library = Library::default();
        self.track_view_cache = track_view_cache::TrackViewCache::default();
        self.recent.clear();
        self.scrolling.stop();
        self.back_history.clear();
        self.forward_history.clear();
        self.leave_mix();
        self.home_request = None;
        self.home_requested = false;
        self.home_collections.clear();
        self.home_sources.clear();
        self.home_catalog = Default::default();
        self.catalog_request = None;
        self.catalog_mix = None;
        self.playlist_items.clear();
        self.editable_playlists.clear();
        self.playlist_add_request = None;
        self.playlist_create_form = None;
        self.playlist_create_request = None;
        self.playlist_controls = playlist_actions::State::default();
        self.library_browser = library_app::State::default();
        self.track_menu_error = None;
        self.track_menu_notice = None;
        self.library_undo = None;
        self.error_closing = None;
        self.error_shown = None;
        self.playlist_removal.reset();
        self.reorder_request = None;
        self.album_titles.reset();
        self.playlist_request = None;
        self.playlists_requested = false;
        self.playlist_error = None;
        self.details.clear();
        self.detail_request = None;
        self.detail_error = None;
        self.metadata_request = None;
        self.metadata_attempted.clear();
        self.like_request = None;
        self.like_aliases.clear();
        self.removed_from_library.clear();
        self.like_error = None;
        self.library_refresh_due = false;
        self.search = SearchState::default();
        self.home_error = None;
        self.home_autoplay = None;
        if matches!(self.page, MusicPage::Search(_)) {
            self.page = MusicPage::Search(String::new());
        } else if !matches!(self.page, MusicPage::Home | MusicPage::Library) {
            self.page = MusicPage::Home;
        }
        self.filter.clear();
        self.selected = None;
        self.auth_error = None;
        self.audio_error = None;
        self.demo = false;
    }

    fn cancel_qr(&mut self) {
        if let Some(control) = self.qr_control.take() {
            control.cancel();
        }
        self.qr_pending = None;
        self.qr_events = None;
        self.qr_texture = None;
        if let Some(mut url) = self.qr_url.take() {
            url.zeroize();
        }
        self.qr_expires = None;
        self.qr_open = false;
        self.qr_code_input.zeroize();
    }

    pub fn connect_qr(&mut self, ctx: &egui::Context) {
        self.cancel_qr();
        self.restoring = false;
        self.pending = None;
        self.qr_open = true;
        self.qr_error = None;
        self.qr_stage = qr_auth::Stage::Creating;
        let control = qr_auth::Control::default();
        self.qr_control = Some(control.clone());
        let (events_sender, events) = mpsc::sync_channel(8);
        let (sender, pending) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-qr-login".into())
            .spawn(move || {
                let result = (|| {
                    let signed_in = qr_auth::start(control, events_sender).map_err(UiError::Qr)?;
                    let music =
                        WebAudioClient::new(signed_in.cookies.clone()).map_err(UiError::Music)?;
                    music
                        .verify_owner(signed_in.profile.id)
                        .map_err(UiError::Music)?;
                    let page = music
                        .page(signed_in.profile.id, 0)
                        .map_err(UiError::Music)?;
                    let cached = library_cache::load(signed_in.profile.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let recent = read_recent_snapshot(signed_in.profile.id);
                    let resume = read_playback_resume(signed_in.profile.id);
                    Ok(QrConnected {
                        resume,
                        recent,
                        cached,
                        profile: signed_in.profile,
                        cookies: signed_in.cookies,
                        music,
                        page,
                    })
                })();
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.qr_events = Some(events);
                self.qr_pending = Some(pending);
            }
            Err(_) => self.qr_error = Some(UiError::WorkerStart),
        }
    }

    fn poll_qr(&mut self, ctx: &egui::Context) {
        if let Some(events) = &self.qr_events {
            while let Ok(event) = events.try_recv() {
                match event {
                    qr_auth::Event::Ready { url, expires_in } => {
                        self.qr_texture = qr_image(ctx, &url);
                        self.qr_url = Some(url);
                        if self.qr_texture.is_none() {
                            self.qr_error = Some(UiError::QrImage);
                        }
                        self.qr_expires =
                            Some(Instant::now() + Duration::from_secs(expires_in.min(300)));
                    }
                    qr_auth::Event::Status(stage) => self.qr_stage = stage,
                    qr_auth::Event::CodeResult(result) => {
                        self.qr_error = result.err().map(UiError::QrCode);
                    }
                }
            }
        }
        if let Some(receiver) = &self.qr_pending {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(UiError::Interrupted)),
            };
            if let Some(result) = result {
                self.qr_pending = None;
                match result {
                    Ok(signed_in) => {
                        if music_session_store::save_profile(&signed_in.profile, &signed_in.cookies)
                            .is_err()
                        {
                            self.qr_error = Some(UiError::MusicStorage);
                            return;
                        }
                        self.reset();
                        self.session = Some(Session {
                            profile: signed_in.profile,
                            cache_warning: false,
                            music: Some(signed_in.music),
                        });
                        self.avatar_refresh_ready = true;
                        self.avatar_refresh_url = self
                            .session
                            .as_ref()
                            .and_then(|session| session.profile.avatar_url.clone());
                        self.recent = self.restored_recent(
                            self.session.as_ref().unwrap().profile.id,
                            signed_in.recent,
                        );
                        self.library = self.restored_library(
                            self.session.as_ref().unwrap().profile.id,
                            signed_in.cached,
                        );
                        self.library.refresh(signed_in.page);
                        self.restore_playback_resume(
                            self.session.as_ref().unwrap().profile.id,
                            signed_in.resume,
                        );
                        self.save_library(ctx);
                    }
                    Err(error) => self.qr_error = Some(error),
                }
            }
        }
    }

    fn stop_playback(&mut self) {
        let ctx = self.ui_context.clone();
        self.set_lyrics_fullscreen(&ctx, false);
        self.lyrics.clear_track();
        self.lyrics.open = false;
        self.discord_rpc.clear();
        self.unavailable_playback.clear();
        self.playlist_controls.cancel_play();
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.playback_pending = None;
        self.player.stop();
        self.playing = None;
        self.play_queue = None;
        self.play_order.clear_queued();
        self.queued_tracks.clear();
        self.queue_ui = queue_app::PanelState::default();
        self.play_order.reset_source(&[], None);
        self.queue_origin = None;
        self.playback_error = None;
        self.seek_position = None;
        self.resume_position = None;
        self.time_slots = None;
        self.finished_handled = false;
        self.advance_on_load = false;
    }

    fn restore_playback_resume(&mut self, owner: i64, saved: Option<playback_resume::Snapshot>) {
        let Some(mut saved) = saved else { return };
        if self.demo
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != owner)
            || saved.track.id <= 0
            || saved.track.owner_id == 0
            || !saved.position.is_finite()
            || saved.position < 0.0
        {
            return;
        }
        let right_panel = self.right_panel_state();
        self.stop_playback();
        self.restore_right_panel_state(right_panel);
        // Credentials and stream URLs are always resolved afresh after Play.
        saved.track.url = None;
        if let Some(album) = &mut saved.track.album {
            album.access_key = None;
        }
        let position = bounded_track_position(saved.position, saved.track.duration);
        let key = saved.track.key();
        self.selected = Some(key);
        self.playing = Some(saved.track.clone());
        let queue = saved.queue.take().unwrap_or_default();
        let in_library = self.library.tracks.iter().any(|track| track.key() == key);
        if queue.tracks.iter().any(|track| track.key() == key) {
            // The source it played from, such as an artist's page.
            self.play_queue = Some(queue.tracks);
            self.queue_origin = Some(queue.origin);
        } else if queue.tracks.is_empty() && in_library {
            self.queue_origin = Some(if queue.origin.is_empty() {
                "library".into()
            } else {
                queue.origin
            });
        } else if !in_library {
            self.play_queue = Some(vec![saved.track]);
            self.queue_origin = Some("resume".into());
        }
        self.resume_position = Some(position);
        self.synchronize_order();
        for mut track in queue.added {
            track.url = None;
            if self.play_order.enqueue(track.key()) {
                self.queued_tracks.insert(track.key(), track);
            }
        }
    }

    fn playback_snapshot(&self) -> crate::playback::Snapshot {
        let mut snapshot = self.player.snapshot();
        if let Some(position) = self.resume_position.filter(|_| self.playing.is_some()) {
            snapshot.position = position;
            snapshot.buffered_until = position;
            snapshot.state = if self.playback_pending.is_some() {
                PlaybackState::Loading
            } else {
                PlaybackState::Paused
            };
        }
        snapshot
    }

    fn commit_playback_seek(&mut self, position: f64) {
        let Some(track) = &self.playing else { return };
        let position = bounded_track_position(position, track.duration);
        self.system_media.position_changed();
        if self.resume_position.is_some() {
            self.resume_position = Some(position);
        } else {
            self.player.seek(position);
        }
    }

    fn playback_resume_snapshot(&self) -> Option<(i64, playback_resume::Snapshot)> {
        if self.demo {
            return None;
        }
        let owner = self.session.as_ref()?.profile.id;
        let track = self.playing.clone()?;
        if owner <= 0 || track.id <= 0 || track.owner_id == 0 {
            return None;
        }
        let position = self
            .resume_position
            .unwrap_or_else(|| self.player.snapshot().position);
        if !position.is_finite() || position < 0.0 {
            return None;
        }
        Some((
            owner,
            playback_resume::Snapshot {
                position: bounded_track_position(position, track.duration),
                queue: Some(self.saved_queue(track.key())),
                track,
            },
        ))
    }

    /// The queue to reopen with: its source (My music is cached apart, so
    /// it saves no tracks) around the current track, and the added tracks.
    fn saved_queue(&self, current: (i64, i64)) -> playback_resume::Queue {
        let tracks = self.play_queue.as_deref().map_or_else(Vec::new, |tracks| {
            let at = tracks
                .iter()
                .position(|track| track.key() == current)
                .unwrap_or(0);
            // Keep some history before the current track and the rest ahead.
            let start = at
                .saturating_sub(playback_resume::MAX_QUEUE / 10)
                .min(tracks.len().saturating_sub(playback_resume::MAX_QUEUE));
            tracks
                .iter()
                .skip(start)
                .take(playback_resume::MAX_QUEUE)
                .cloned()
                .collect()
        });
        let added = self
            .play_order
            .queued()
            .iter()
            .filter_map(|entry| self.queued_tracks.get(&entry.key).cloned())
            .take(playback_resume::MAX_ADDED)
            .collect();
        playback_resume::Queue {
            origin: self
                .queue_origin
                .clone()
                .unwrap_or_else(|| "library".into()),
            tracks,
            added,
        }
    }

    fn play_track(&mut self, ctx: &egui::Context, key: (i64, i64)) {
        self.play_track_at(ctx, key, 0.0);
    }

    fn play_track_at(&mut self, ctx: &egui::Context, key: (i64, i64), position: f64) {
        self.playlist_controls.cancel_play();
        self.home_autoplay = None;
        self.mix_advance = false;
        if self.demo {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let Some(track) = self
            .play_queue
            .iter()
            .flat_map(|tracks| tracks.iter())
            .chain(self.library.tracks.iter())
            .chain(self.recent.iter())
            .chain(self.queued_tracks.get(&key))
            .chain(self.playing.iter())
            .chain(
                self.home_collections
                    .iter()
                    .flat_map(|collection| collection.tracks.iter()),
            )
            .find(|track| track.key() == key)
            .cloned()
        else {
            return;
        };
        if track.unavailable == Some(true) {
            crate::diagnostics::event("playback", "play_request: explicit_provider_restriction");
            self.reject_unavailable_track(ctx, key, false);
            return;
        }
        let music = session.music.clone();
        crate::diagnostics::event("playback", "play_request: resolve_start");
        let keys = self.playback_keys();
        self.play_order.sync(&keys, Some(key));
        self.play_order.record_played(key);
        let cache_key = CacheKey {
            account_id: session.profile.id,
            owner_id: key.0,
            track_id: key.1,
            duration: track.duration,
        };
        let cache = self.audio_cache.clone();
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.playback_cancel = Some(cancel.clone());
        self.player.stop();
        self.playback_pending = None;
        self.playback_error = None;
        self.selected = Some(key);
        self.playing = Some(track.clone());
        self.seek_position = None;
        self.resume_position = Some(bounded_track_position(position, track.duration));
        self.time_slots = None;
        self.finished_handled = false;
        self.advance_on_load = false;
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-resolve-audio".into())
            .spawn(move || {
                let result = (|| {
                    let complete = cache.as_ref().is_some_and(|cache| {
                        cache
                            .complete_frames(&cache_key)
                            .ok()
                            .flatten()
                            .is_some_and(|frames| {
                                crate::playback::completed_duration_plausible(
                                    frames,
                                    cache_key.duration,
                                ) && cache.contains_range(&cache_key, 0, frames).unwrap_or(false)
                            })
                    });
                    if cancel.load(Ordering::Acquire) {
                        return Err(UiError::Interrupted);
                    }
                    let url = if complete {
                        crate::diagnostics::event("playback", "resolve: complete_pcm_cache");
                        None
                    } else if let Some(music) = music {
                        crate::diagnostics::event("playback", "resolve: vk_session");
                        Some(music.resolve_cached(key, &cancel).map_err(UiError::Music)?)
                    } else {
                        if !track.has_stream() {
                            return Err(UiError::NoStream);
                        }
                        Some(
                            track
                                .url
                                .clone()
                                .filter(|url| url.starts_with("https://"))
                                .ok_or(UiError::NoStream)?,
                        )
                    };
                    Ok(ResolvedTrack {
                        track,
                        url,
                        cache_key,
                    })
                })();
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playback_pending = Some(receiver);
            }
            Err(_) => self.playback_error = Some(UiError::WorkerStart),
        }
    }

    fn playback_keys(&self) -> Vec<(i64, i64)> {
        self.play_queue
            .as_deref()
            .unwrap_or(&self.library.tracks)
            .iter()
            .filter(|track| track.unavailable != Some(true))
            .map(Track::key)
            .collect()
    }

    fn synchronize_order(&mut self) {
        let current = self.playing.as_ref().map(Track::key).or(self.selected);
        let keys: Vec<_> = self
            .play_queue
            .as_deref()
            .unwrap_or(&self.library.tracks)
            .iter()
            .filter(|track| track.unavailable != Some(true) || Some(track.key()) == current)
            .map(Track::key)
            .collect();
        self.play_order.sync(&keys, current);
    }

    fn view_queue_origin(&self) -> String {
        match &self.page {
            MusicPage::Recent => "recent".into(),
            MusicPage::Search(query) => format!("search:{query}"),
            MusicPage::Collection(id) => format!("collection:{id}"),
            MusicPage::Artist(_)
            | MusicPage::Album(_)
            | MusicPage::Playlist(_)
            | MusicPage::Owner(_)
            | MusicPage::TrackAlbum(_, _) => self.page.detail_key().unwrap_or_default(),
            MusicPage::Home | MusicPage::Library => "library".into(),
        }
    }

    fn replace_play_queue_from(&mut self, queue: Option<Vec<Track>>, origin: String) {
        let same_source = self.queue_origin.as_ref() == Some(&origin);
        self.play_queue = queue;
        self.queue_origin = Some(origin);
        let keys = self.playback_keys();
        if same_source {
            self.play_order.sync(
                &keys,
                self.playing.as_ref().map(Track::key).or(self.selected),
            );
        } else {
            self.play_order.reset_source(&keys, None);
        }
    }

    fn replace_play_queue(&mut self, queue: Option<Vec<Track>>) {
        let origin = if queue.is_none() {
            "library".into()
        } else {
            self.view_queue_origin()
        };
        self.replace_play_queue_from(queue, origin);
    }

    fn allow_queue_wrap(&self) -> bool {
        if self.mix_active {
            !self.mix_has_more
        } else {
            !self.library_queue_active() || self.library.exhausted
        }
    }

    fn library_queue_active(&self) -> bool {
        self.play_queue.is_none() || self.queue_origin.as_deref() == Some("library")
    }

    fn start_queue_from(&mut self, ctx: &egui::Context, tracks: Vec<Track>, origin: String) {
        self.unavailable_playback.clear();
        if tracks.is_empty() {
            return;
        }
        self.replace_play_queue_from(Some(tracks), origin);
        if let Some(first) = self.first_playback_key() {
            self.play_order.select_source(first);
            self.play_track(ctx, first);
        }
    }

    fn start_queue(&mut self, ctx: &egui::Context, tracks: Vec<Track>) {
        self.start_queue_from(ctx, tracks, self.view_queue_origin());
    }

    fn first_playback_key(&self) -> Option<(i64, i64)> {
        self.play_order.first().or_else(|| {
            self.play_queue
                .as_deref()
                .unwrap_or(&self.library.tracks)
                .first()
                .map(Track::key)
        })
    }

    fn toggle_shuffle(&mut self) {
        let keys = self.playback_keys();
        self.play_order.set_shuffle(
            !self.play_order.shuffle(),
            &keys,
            self.playing.as_ref().map(Track::key).or(self.selected),
        );
        self.save_playback_modes();
    }

    fn save_playback_modes(&mut self) {
        self.playback_modes_writer
            .save(self.play_order.shuffle(), self.play_order.repeat());
    }

    fn cancel_delayed_advance(&mut self) {
        self.unavailable_playback.clear();
        self.playlist_controls.cancel_play();
        self.cancel_mix_start();
        self.mix_advance = false;
        self.home_autoplay = None;
        self.advance_on_load = false;
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.qr_open
            || self.mix_settings.open
            || self.playlist_create_form.is_some()
            || self.playlist_controls.has_modal()
            || egui::Popup::is_any_open(ctx)
            || ctx.data(|data| data.get_temp::<u64>(egui::Id::new("player-pointer-release-frame")))
                == Some(ctx.cumulative_frame_nr())
        {
            return;
        }
        let commands = keybinds::read(ctx);
        for command in commands {
            match command {
                keybinds::Command::Volume(delta) => self.nudge_volume(delta),
                keybinds::Command::Seek(seconds) => self.seek_by(seconds),
                keybinds::Command::TogglePause if !self.demo && self.session.is_some() => {
                    self.toggle_playback(ctx)
                }
                keybinds::Command::Previous if !self.demo && self.session.is_some() => {
                    self.previous_track(ctx, false)
                }
                keybinds::Command::Next if !self.demo && self.session.is_some() => {
                    self.next_track(ctx)
                }
                keybinds::Command::Reload if !self.demo && self.session.is_some() => {
                    self.reload_page(ctx)
                }
                _ => {}
            }
            ctx.request_repaint();
        }
    }

    /// Ctrl+R: the same refresh as the open page's own Refresh button.
    fn reload_page(&mut self, ctx: &egui::Context) {
        if self.settings_open {
            return;
        }
        match self.page.clone() {
            MusicPage::Home => {
                if self.home_request.is_none() && self.catalog_request.is_none() {
                    self.load_home(ctx);
                    if self.pending.is_none() {
                        self.load_page(ctx, true);
                    }
                }
            }
            MusicPage::Library => {
                if self.pending.is_none() && self.home_request.is_none() {
                    self.load_page(ctx, true);
                }
            }
            MusicPage::Collection(id) => {
                if self.pending.is_none() && self.home_request.is_none() {
                    self.open_collection(ctx, id, true);
                }
            }
            MusicPage::Search(query) if !query.is_empty() => {
                if self.search.results_request.is_none() {
                    self.search.cache.remove(&query);
                    self.search.results = None;
                    self.search.results_error = None;
                    self.start_search_results(ctx, 0);
                }
            }
            MusicPage::Artist(_)
            | MusicPage::Album(_)
            | MusicPage::Playlist(_)
            | MusicPage::Owner(_)
            | MusicPage::TrackAlbum(..) => {
                if self.detail_request.is_none() {
                    self.ensure_detail(ctx, true);
                }
            }
            MusicPage::Search(_) | MusicPage::Recent => {}
        }
    }

    fn seek_by(&mut self, seconds: f64) {
        if self.playback_pending.is_some() {
            return;
        }
        let Some(track) = &self.playing else { return };
        if track.duration == 0 {
            return;
        }
        let snapshot = self.playback_snapshot();
        if !matches!(
            snapshot.state,
            PlaybackState::Playing
                | PlaybackState::Paused
                | PlaybackState::Loading
                | PlaybackState::Ended
        ) {
            return;
        }
        let target = (snapshot.position + seconds).clamp(0.0, f64::from(track.duration));
        self.cancel_delayed_advance();
        self.seek_position = None;
        self.finished_handled = false;
        self.commit_playback_seek(target);
    }

    fn previous_track(&mut self, ctx: &egui::Context, restart_after_three_seconds: bool) {
        if restart_after_three_seconds && self.restart_track_after_three_seconds() {
            return;
        }
        self.unavailable_playback.clear();
        self.cancel_delayed_advance();
        self.synchronize_order();
        let Some(current) = self.playing.as_ref().map(Track::key).or(self.selected) else {
            return;
        };
        if let Some(key) = self
            .play_order
            .advance_previous(current, self.allow_queue_wrap())
        {
            self.play_track(ctx, key);
        }
    }

    fn restart_track_after_three_seconds(&mut self) -> bool {
        if self.playing.is_some() && self.playback_snapshot().position > 3.0 {
            crate::diagnostics::event("playback", "previous_restart_current");
            self.unavailable_playback.clear();
            self.cancel_delayed_advance();
            self.seek_position = None;
            self.commit_playback_seek(0.0);
            self.finished_handled = false;
            true
        } else {
            false
        }
    }

    fn toggle_playback(&mut self, ctx: &egui::Context) {
        self.unavailable_playback.clear();
        self.cancel_mix_start();
        self.mix_advance = false;
        self.home_autoplay = None;
        self.advance_on_load = false;
        if self.playback_pending.is_some() {
            return;
        }
        if self.playing.is_none() && !self.play_order.queued().is_empty() {
            self.synchronize_order();
            if let Some(key) = self.play_order.start_queued(None) {
                self.play_track(ctx, key);
            }
            return;
        }
        if let Some(track) = &self.playing
            && track.unavailable == Some(true)
            && !matches!(
                self.player.snapshot().state,
                PlaybackState::Playing | PlaybackState::Loading
            )
        {
            let key = track.key();
            self.reject_unavailable_track(ctx, key, false);
            return;
        }
        if let Some(position) = self.resume_position
            && let Some(key) = self.playing.as_ref().map(Track::key)
        {
            self.play_track_at(ctx, key, position);
            return;
        }
        match self.player.snapshot().state {
            PlaybackState::Playing | PlaybackState::Loading => self.player.pause(),
            PlaybackState::Paused => self.player.resume(),
            _ => {
                if self.playing.is_none() && self.play_order.shuffle() {
                    self.prepare_view_queue();
                    if let Some(key) = self.first_playback_key() {
                        self.play_track(ctx, key);
                    }
                    return;
                }
                if let Some(key) = self
                    .playing
                    .as_ref()
                    .map(Track::key)
                    .or(self.selected)
                    .or_else(|| self.view_tracks().first().map(Track::key))
                {
                    if self.playing.is_none() {
                        self.play_from_view(ctx, key);
                    } else {
                        self.play_track(ctx, key);
                    }
                }
            }
        }
    }

    fn adjacent_track(&self, next: bool) -> Option<(i64, i64)> {
        if next && self.playing.is_none() && self.selected.is_none() {
            return self.play_order.queued().front().map(|entry| entry.key);
        }
        let key = self.playing.as_ref().map(Track::key).or(self.selected)?;
        let ordered = if next {
            self.play_order.next(key, self.allow_queue_wrap())
        } else {
            self.play_order.previous(key, self.allow_queue_wrap())
        };
        if ordered.is_some() || self.play_order.shuffle() {
            return ordered;
        }
        let tracks = self.play_queue.as_deref().unwrap_or(&self.library.tracks);
        let index = tracks.iter().position(|track| track.key() == key)?;
        if next {
            tracks[index + 1..]
                .iter()
                .find(|track| track.unavailable != Some(true))
                .map(Track::key)
        } else {
            tracks[..index]
                .iter()
                .rev()
                .find(|track| track.unavailable != Some(true))
                .map(Track::key)
        }
    }

    fn next_track(&mut self, ctx: &egui::Context) {
        self.unavailable_playback.clear();
        if self.restart_repeated_track(ctx) {
            return;
        }
        self.advance_track(ctx, false);
    }

    /// Set or clear the account's "now playing" status. Fire and forget: a
    /// failure leaves the status as it was and never stops playback.
    fn send_broadcast(&mut self, key: Option<(i64, i64)>) {
        if self.demo || cfg!(test) {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        if let Ok(worker) = thread::Builder::new()
            .name("vk-broadcast".into())
            .spawn(move || {
                let result = client.set_broadcast(owner, key, &AtomicBool::new(false));
                crate::diagnostics::event(
                    "broadcast",
                    match (&result, key.is_some()) {
                        (Ok(()), true) => "set",
                        (Ok(()), false) => "cleared",
                        (Err(_), _) => "failed",
                    },
                );
            })
        {
            self.workers.push(worker);
        }
    }

    /// With repeat one, Next plays the current track again from the start.
    fn restart_repeated_track(&mut self, ctx: &egui::Context) -> bool {
        self.synchronize_order();
        let Some(key) = self
            .play_order
            .repeats(self.playing.as_ref().map(Track::key))
        else {
            return false;
        };
        self.cancel_delayed_advance();
        self.play_track(ctx, key);
        true
    }

    fn advance_track(&mut self, ctx: &egui::Context, natural_end: bool) {
        if self.unavailable_playback.halted() {
            return;
        }
        if self.unavailable_playback.active() {
            self.continue_unavailable_recovery(ctx);
            return;
        }
        self.cancel_mix_start();
        self.mix_advance = false;
        self.home_autoplay = None;
        if self.playing.is_none() && !self.play_order.queued().is_empty() {
            self.synchronize_order();
            if let Some(key) = self.play_order.start_queued(None) {
                self.play_track(ctx, key);
            }
            return;
        }
        if self.playing.is_none() && self.selected.is_none() {
            self.prepare_view_queue();
            if let Some(key) = self.first_playback_key() {
                self.play_track(ctx, key);
            }
            return;
        }
        self.synchronize_order();
        let key = self
            .playing
            .as_ref()
            .map(Track::key)
            .or(self.selected)
            .and_then(|key| {
                self.play_order
                    .advance_next(key, self.allow_queue_wrap(), natural_end)
            });
        if let Some(key) = key {
            self.play_track(ctx, key);
        } else if self.mix_active && self.mix_has_more {
            if !self.unavailable_playback.allow_continuation() {
                self.stop_unavailable_recovery();
                return;
            }
            self.mix_advance = true;
            self.load_mix(ctx, false);
        } else if self.library_queue_active() && !self.library.exhausted {
            if !self.unavailable_playback.allow_continuation() {
                self.stop_unavailable_recovery();
                return;
            }
            self.advance_on_load = true;
            if self.pending.is_none() {
                self.load_page(ctx, false);
            }
        } else if self.mix_after_queue
            && !self.mix_active
            && let Some(last) = self.playing.as_ref().map(Track::key)
        {
            // The queue has ended: carry on with a mix of the last track.
            if !self.unavailable_playback.allow_continuation() {
                self.stop_unavailable_recovery();
                return;
            }
            self.play_track_mix(ctx, last);
        }
    }

    fn poll_playback(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.playback_pending {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(UiError::Interrupted)),
            };
            if let Some(result) = result {
                self.playback_pending = None;
                self.playback_cancel = None;
                match result {
                    Ok(mut resolved) => {
                        crate::diagnostics::event(
                            "playback",
                            "resolve: success; decoder_requested",
                        );
                        self.unavailable_playback.clear();
                        if self.session.is_some() {
                            resolved.track.library_liked =
                                Some(self.track_is_liked(&resolved.track));
                        }
                        self.recent
                            .retain(|track| track.key() != resolved.track.key());
                        self.track_view_cache.invalidate();
                        self.recent.insert(0, resolved.track.clone());
                        self.recent.truncate(recent_cache::MAX_RECENT);
                        self.save_recent(ctx);
                        if self.broadcast {
                            self.send_broadcast(Some(resolved.track.key()));
                        }
                        let position = self.resume_position.take().unwrap_or(0.0);
                        self.player
                            .play_cached(resolved.cache_key, resolved.url, position);
                        self.playing = Some(resolved.track);
                    }
                    Err(UiError::NoStream | UiError::Music(WebAudioError::NoStream)) => {
                        crate::diagnostics::event(
                            "playback",
                            "resolve_failed: no_stream; availability_unchanged",
                        );
                        self.playback_error = Some(UiError::Music(WebAudioError::NoStream));
                    }
                    Err(error) => {
                        crate::diagnostics::event(
                            "playback",
                            "resolve_failed: other; availability_unchanged",
                        );
                        self.playback_error = Some(error);
                    }
                }
            }
        }
        if self.playback_pending.is_none()
            && self.playing.is_some()
            && matches!(self.player.snapshot().state, PlaybackState::Ended)
            && !self.finished_handled
            && self.home_autoplay.is_none()
            && !self.playlist_controls.loading_play()
            && !self
                .mix_request
                .as_ref()
                .is_some_and(|request| request.start)
        {
            self.finished_handled = true;
            self.advance_track(ctx, true);
        }
    }

    fn enter_demo(&mut self) {
        self.reset();
        self.demo = true;
        self.library.tracks = demo_tracks();
        self.library.total = Some(self.library.tracks.len());
        self.library.offset = self.library.tracks.len();
        self.library.exhausted = true;
    }

    fn start_job(
        &mut self,
        ctx: &egui::Context,
        job: impl FnOnce() -> Result<Reply, UiError> + Send + 'static,
    ) {
        self.workers.retain(|worker| !worker.is_finished());
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        match thread::Builder::new().name("vk-api".into()).spawn(move || {
            let _ = sender.send(job());
            ctx.request_repaint();
        }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.pending = Some(receiver);
            }
            Err(_) => {
                let message = UiError::WorkerStart;
                if self.session.is_some() {
                    self.audio_error = Some(message);
                } else {
                    self.auth_error = Some(message);
                }
            }
        }
    }

    pub fn restore_session(&mut self, ctx: &egui::Context) {
        self.restoring = true;
        self.start_job(ctx, move || {
            music_session_store::remove_retired_logins();
            match music_session_store::restore_profile().map_err(|_| UiError::MusicStorage)? {
                Some((profile, cookies)) => {
                    let cached = library_cache::load(profile.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let recent = read_recent_snapshot(profile.id);
                    let resume = read_playback_resume(profile.id);
                    Ok(Reply::Connected(
                        Box::new(Session {
                            profile,
                            cache_warning: false,
                            music: Some(WebAudioClient::new(cookies).map_err(UiError::Music)?),
                        }),
                        None,
                        false,
                        cached,
                        recent,
                        resume.map(Box::new),
                    ))
                }
                None => Ok(Reply::NoSavedSession),
            }
        });
    }

    /// Changes VK Fast's own volume by `delta` and saves it shortly after.
    fn nudge_volume(&mut self, delta: f32) {
        let previous = self.player.snapshot().volume;
        let value = (previous + delta).clamp(0.0, 1.0);
        if value != previous {
            self.player.set_volume(value);
            self.volume_save_at = Some(Instant::now() + Duration::from_millis(500));
        }
    }

    /// Ctrl + volume keys, pressed anywhere: held only while enabled, and
    /// applied without the window, like other media commands.
    fn poll_volume_keys(&mut self) {
        self.volume_keys
            .sync(self.volume_keys_enabled, &self.repaint_target);
        let change = self.volume_keys.take_change();
        if change != 0.0 {
            self.nudge_volume(change);
        }
    }

    /// VK renews its website session cookies as the app talks to it; the
    /// saved session keeps the new ones, or it would stop working later.
    fn save_session_cookies(&mut self) {
        if cfg!(test) || self.demo {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(cookies) = session
            .music
            .as_ref()
            .and_then(WebAudioClient::take_cookie_update)
        else {
            return;
        };
        let owner = session.profile.id;
        self.workers.retain(|worker| !worker.is_finished());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-session-save".into())
            .spawn(move || {
                let saved = music_session_store::update_cookies(owner, &cookies).is_ok();
                crate::diagnostics::event("session", &format!("cookies_saved ok={saved}"));
            })
        {
            self.workers.push(worker);
        }
    }

    fn logout(&mut self) {
        self.reset();
        if music_session_store::clear().is_err() {
            self.auth_error = Some(UiError::MusicStorage);
        }
    }

    fn save_recent(&mut self, ctx: &egui::Context) {
        if let Some(session) = &self.session {
            let snapshot = Arc::new(self.recent.clone());
            self.recent_latest
                .insert(session.profile.id, snapshot.clone());
            self.recent_save_queue.insert(session.profile.id, snapshot);
            self.flush_recent_save(ctx);
        }
    }

    fn restored_recent(&self, owner: i64, fallback: Vec<Track>) -> Vec<Track> {
        let latest = self
            .recent_latest
            .get(&owner)
            .or_else(|| self.recent_save_queue.get(&owner))
            .or_else(|| {
                self.recent_save_current
                    .as_ref()
                    .filter(|(account, _)| *account == owner)
                    .map(|(_, tracks)| tracks)
            });
        latest.map_or(fallback, |tracks| tracks.as_ref().clone())
    }

    fn flush_recent_save(&mut self, ctx: &egui::Context) {
        if self.recent_save_pending.is_some() {
            return;
        }
        let Some((&owner, tracks)) = self.recent_save_queue.iter().next() else {
            return;
        };
        let tracks = tracks.clone();
        let snapshot = tracks.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-recent-cache".into())
            .spawn(move || {
                let _ = sender.send(write_recent_snapshot(owner, &snapshot));
                ctx.request_repaint();
            })
        {
            self.workers.push(worker);
            self.recent_save_queue.remove(&owner);
            self.recent_save_current = Some((owner, tracks));
            self.recent_save_pending = Some(receiver);
        }
    }

    fn poll_recent_save(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.recent_save_pending else {
            return;
        };
        let saved = match receiver.try_recv() {
            Ok(saved) => saved,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => false,
        };
        self.recent_save_pending = None;
        if !saved {
            if let Some((owner, tracks)) = self.recent_save_current.take() {
                self.recent_save_queue.entry(owner).or_insert(tracks);
            }
            return;
        }
        self.recent_save_current = None;
        self.flush_recent_save(ctx);
    }

    fn save_library(&mut self, ctx: &egui::Context) {
        if let Some(session) = &self.session {
            self.library_save_queue
                .insert(session.profile.id, Arc::new(self.library.tracks.clone()));
            self.flush_library_save(ctx);
        }
    }

    fn restored_library(&self, owner: i64, fallback: Vec<Track>) -> Library {
        let latest = self.library_save_queue.get(&owner).or_else(|| {
            self.library_save_current
                .as_ref()
                .filter(|(account, _)| *account == owner)
                .map(|(_, tracks)| tracks)
        });
        #[cfg(not(test))]
        let total = library_cache::load_total(owner).ok().flatten();
        #[cfg(test)]
        let total = {
            let _ = library_cache::load_total;
            None
        };
        let library = Library {
            total,
            ..Library::from_cached(latest.map_or(fallback, |tracks| tracks.as_ref().clone()))
        };
        // Synthetic app tests use rows of other owners as library tracks.
        if cfg!(test) {
            library
        } else {
            library.for_owner(owner)
        }
    }

    fn flush_library_save(&mut self, ctx: &egui::Context) {
        if self.library_save_pending.is_some() {
            return;
        }
        let Some((&owner, tracks)) = self.library_save_queue.iter().next() else {
            return;
        };
        let tracks = tracks.clone();
        let writer_tracks = tracks.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-library-cache".into())
            .spawn(move || {
                let saved = write_library_snapshot(owner, &writer_tracks);
                let _ = sender.send(saved);
                ctx.request_repaint();
            })
        {
            self.workers.push(worker);
            self.library_save_queue.remove(&owner);
            self.library_save_current = Some((owner, tracks));
            self.library_save_pending = Some(receiver);
        }
    }

    fn poll_library_save(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.library_save_pending else {
            return;
        };
        let saved = match receiver.try_recv() {
            Ok(saved) => saved,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => false,
        };
        self.library_save_pending = None;
        if !saved {
            if let Some((owner, tracks)) = self.library_save_current.take() {
                self.library_save_queue.entry(owner).or_insert(tracks);
            }
            return;
        }
        self.library_save_current = None;
        self.flush_library_save(ctx);
    }

    fn load_page(&mut self, ctx: &egui::Context, refresh: bool) {
        if self.pending.is_some() {
            return;
        }
        let Some(session) = &self.session else { return };
        let music = session.music.clone();
        let owner_id = session.profile.id;
        // Preserve the current library until a refresh actually succeeds.
        let offset = if refresh { 0 } else { self.library.offset };
        self.audio_error = None;
        self.start_job(ctx, move || {
            let mut refreshed = None;
            let page = if let Some(music) = music {
                let mut load = || {
                    if refresh {
                        let profile = music.refresh_profile(owner_id)?;
                        if profile.id != owner_id {
                            return Err(WebAudioError::WrongAccount);
                        }
                        let cache_warning = !write_profile_metadata(&profile);
                        refreshed = Some(RefreshedProfile {
                            profile,
                            cache_warning,
                        });
                    }
                    music.page_batch(owner_id, offset, 100)
                };
                load().map_err(|error| UiError::Music(music.confirm_expired(owner_id, error)))
            } else {
                Err(UiError::NoStream)
            };
            page.map(|page| Reply::Audio(page, refresh, refreshed))
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.restoring = false;
                self.avatar_refresh_ready = true;
                let message = UiError::Interrupted;
                if self.session.is_some() {
                    self.audio_error = Some(message);
                } else {
                    self.auth_error = Some(message);
                }
                return;
            }
        };
        self.pending = None;
        self.restoring = false;
        match result {
            Ok(Reply::NoSavedSession) => {}
            Ok(Reply::Connected(session, music_error, explicit_login, cached, recent, resume)) => {
                self.track_view_cache.invalidate();
                if explicit_login {
                    self.avatar_refresh_ready = true;
                    self.avatar_refresh_url = session.profile.avatar_url.clone();
                }
                self.recent = self.restored_recent(session.profile.id, recent);
                self.library = self.restored_library(session.profile.id, cached);
                self.session = Some(*session);
                self.restore_playback_resume(
                    self.session.as_ref().unwrap().profile.id,
                    resume.map(|resume| *resume),
                );
                self.auth_error = None;
                if let Some(error) = music_error {
                    self.audio_error = Some(error);
                } else {
                    self.load_page(ctx, true);
                }
            }
            Ok(Reply::Audio(page, refresh, refreshed)) => {
                if let Some(refreshed) = &refreshed
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|session| session.profile.id != refreshed.profile.id)
                {
                    self.audio_error = Some(UiError::Music(WebAudioError::WrongAccount));
                    return;
                }
                if refresh && !self.avatar_refresh_ready {
                    self.avatar_refresh_ready = true;
                    self.avatar_refresh_url = refreshed
                        .as_ref()
                        .and_then(|update| update.profile.avatar_url.clone());
                }
                if let Some(refreshed) = refreshed
                    && let Some(session) = &mut self.session
                {
                    session.profile.merge_metadata(&refreshed.profile);
                    session.cache_warning |= refreshed.cache_warning;
                }
                if refresh {
                    self.library.refresh(page);
                } else {
                    self.library.append(page);
                }
                self.reconcile_library_removals();
                let availability: HashMap<_, _> = self
                    .library
                    .tracks
                    .iter()
                    .filter_map(|track| track.unavailable.map(|status| (track.key(), status)))
                    .collect();
                let mut availability_changed = false;
                self.update_known_tracks(|track| {
                    if let Some(status) = availability.get(&track.key()).copied()
                        && track.unavailable != Some(status)
                    {
                        track.unavailable = Some(status);
                        availability_changed = true;
                    }
                });
                if availability_changed {
                    self.save_recent(ctx);
                }
                if self.queue_origin.as_deref() == Some("library")
                    && let Some(queue) = &mut self.play_queue
                {
                    let mut known: HashSet<_> = queue.iter().map(Track::key).collect();
                    queue.extend(
                        self.library
                            .tracks
                            .iter()
                            .filter(|track| known.insert(track.key()))
                            .cloned(),
                    );
                }
                self.save_library(ctx);
                self.audio_error = None;
                if self.library_queue_active() {
                    self.synchronize_order();
                }
                if self.advance_on_load {
                    self.advance_on_load = false;
                    self.advance_track(ctx, false);
                }
            }
            Err(error) => {
                self.advance_on_load = false;
                // A failed startup check keeps the cached avatar for this run.
                // A later library refresh must not consume a stale stored URL.
                self.avatar_refresh_ready = true;
                if let UiError::Music(
                    music_error @ (WebAudioError::Expired
                    | WebAudioError::NoSession
                    | WebAudioError::WrongAccount),
                ) = &error
                {
                    crate::diagnostics::event(
                        "session",
                        &format!("account_disconnected stage=library reason={music_error:?}"),
                    );
                    self.reset();
                    self.auth_error = Some(error);
                } else if self.session.is_some() {
                    self.audio_error = Some(error);
                } else {
                    self.auth_error = Some(error);
                }
            }
        }
    }

    fn login_controls(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let ctx = ui.ctx().clone();
        let can_login = self.pending.is_none();
        let button_size = egui::vec2(ui.available_width(), 36.0);
        if ui
            .add_enabled(
                can_login,
                egui::Button::new(t("Войти по QR-коду", "Sign in with QR code"))
                    .fill(PRIMARY)
                    .min_size(button_size),
            )
            .clicked()
        {
            self.connect_qr(&ctx);
            egui::Popup::close_id(&ctx, egui::Id::new("account-popup"));
        }
        if self.restoring {
            ui.label(t(
                "Восстанавливаем сохранённый вход…",
                "Restoring your saved sign-in…",
            ));
            if ui.button(t("Отменить", "Cancel")).clicked() {
                self.reset();
            }
        }
        if let Some(error) = &self.auth_error {
            ui.colored_label(Color32::from_rgb(255, 158, 153), error.message(language));
        }
    }

    fn account_menu(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        if let Some(session) = &self.session {
            ui.label(RichText::new(session.profile.display_name()).family(theme::semibold()));
            ui.label(
                RichText::new(format!("VK ID {}", session.profile.id))
                    .small()
                    .color(MUTED),
            );
            if session.cache_warning {
                ui.colored_label(
                    Color32::from_rgb(255, 178, 120),
                    t(
                        "Вход выполнен, но сохранить сессию не удалось.",
                        "Signed in, but the session could not be saved.",
                    ),
                );
            }
            ui.add_space(8.0);
            ui.separator();
            if ui
                .add_sized(
                    [ui.available_width(), 36.0],
                    egui::Button::new(t("Выйти из аккаунта", "Sign out")),
                )
                .clicked()
            {
                self.logout();
                ui.close();
            }
        } else {
            ui.label(
                RichText::new(t("Подключение к VK", "Connect to VK")).family(theme::semibold()),
            );
            ui.label(
                RichText::new(t(
                    "Вход на официальном сайте VK",
                    "Sign in on the official VK website",
                ))
                .small()
                .color(MUTED),
            );
            ui.add_space(8.0);
            ui.push_id("account-menu-login", |ui| self.login_controls(ui));
        }
    }

    fn account_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let profile = self.session.as_ref().map(|session| &session.profile);
        let initials: String = profile
            .into_iter()
            .flat_map(|profile| [&profile.first_name, &profile.last_name])
            .filter_map(|name| name.trim().chars().next())
            .flat_map(char::to_uppercase)
            .take(2)
            .collect();
        let texture = self.avatars.texture(
            ui.ctx(),
            profile.map(|profile| profile.id),
            self.avatar_refresh_url.as_deref(),
        );
        let response = cover_art::avatar_button(ui, texture.as_ref(), &initials, 38.0)
            .on_hover_text(self.language.text("Аккаунт", "Account"));
        egui::Popup::from_toggle_button_response(&response)
            .id(egui::Id::new("account-popup"))
            .align(egui::RectAlign::BOTTOM_END)
            .gap(10.0)
            .width(300.0)
            .layout(Layout::top_down(Align::Min))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .frame(
                Frame::popup(ui.style())
                    .fill(SURFACE)
                    .inner_margin(18)
                    .corner_radius(12),
            )
            .show(|ui| self.account_menu(ui));
        response
    }

    fn cancel_mix_start(&mut self) {
        if self
            .mix_request
            .as_ref()
            .is_some_and(|request| request.start)
        {
            self.mix_request = None;
            self.mix_error = None;
        }
    }

    fn leave_mix(&mut self) {
        self.playlist_controls.cancel_play();
        self.mix_request = None;
        self.mix_active = false;
        self.mix_seed = None;
        self.mix_has_more = false;
        self.mix_advance = false;
        self.mix_error = None;
    }

    fn play_mix(&mut self, ctx: &egui::Context) {
        if self.mix_active && self.mix_seed.is_none() && self.playing.is_some() {
            if matches!(self.player.snapshot().state, PlaybackState::Ended)
                && self.adjacent_track(true).is_none()
            {
                if self.mix_has_more {
                    self.mix_advance = true;
                    self.load_mix(ctx, false);
                } else {
                    self.leave_mix();
                    self.load_mix(ctx, true);
                }
            } else {
                self.toggle_playback(ctx);
            }
        } else {
            self.mix_request = None;
            self.load_mix_from(ctx, true, None);
        }
    }

    /// Open "Configure VK Mix" and load VK's current options for it.
    fn open_mix_settings(&mut self, ctx: &egui::Context) {
        if self.demo {
            return;
        }
        let Some((owner, client)) = self
            .session
            .as_ref()
            .and_then(|session| Some((session.profile.id, session.music.clone()?)))
        else {
            return;
        };
        self.mix_settings.close();
        self.mix_settings.open = true;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-mix-settings".into())
            .spawn(move || {
                let _ = sender.send(client.vk_mix_settings(owner, &worker_cancel));
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.mix_settings.request = Some((canceled, receiver));
            }
            Err(_) => self.mix_settings.error = Some(WebAudioError::Network),
        }
    }

    fn poll_mix_settings(&mut self) {
        let Some((_, receiver)) = &self.mix_settings.request else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        self.mix_settings.request = None;
        match result {
            Ok(settings) => {
                // The current choices: this run's own, else what VK applies.
                self.mix_settings.selection = match &self.mix_tuning {
                    Some(tuning) => tuning.choices.clone(),
                    None => settings
                        .categories
                        .iter()
                        .filter_map(|category| {
                            let option = category.options.iter().find(|option| option.selected)?;
                            Some((category.id.clone(), option.id.clone()))
                        })
                        .collect(),
                };
                self.mix_settings.selection.retain(|(category, option)| {
                    settings.categories.iter().any(|known| {
                        known.id == *category
                            && known.options.iter().any(|known| known.id == *option)
                    })
                });
                self.mix_settings.settings = Some(settings);
            }
            Err(error) => self.mix_settings.error = Some(error),
        }
    }

    fn mix_settings_window(&mut self, ctx: &egui::Context) {
        if !self.mix_settings.open {
            return;
        }
        let error = self
            .mix_settings
            .error
            .as_ref()
            .map(|error| error.localized(self.language));
        let output = crate::mix_settings_ui::show(
            ctx,
            crate::mix_settings_ui::View {
                language: self.language,
                settings: self.mix_settings.settings.as_ref(),
                loading: self.mix_settings.request.is_some(),
                error: error.as_deref(),
            },
            &mut self.mix_settings.selection,
        );
        match output {
            crate::mix_settings_ui::Output::None => {}
            crate::mix_settings_ui::Output::Close => self.mix_settings.close(),
            crate::mix_settings_ui::Output::Apply => {
                let choices = std::mem::take(&mut self.mix_settings.selection);
                let mix_id = self
                    .mix_settings
                    .settings
                    .as_ref()
                    .map(|settings| settings.mix_id.clone())
                    .unwrap_or_else(|| "common".into());
                self.mix_settings.close();
                self.mix_tuning = (!choices.is_empty()).then(|| crate::web_audio::MixTuning {
                    mix_id,
                    id: random_mix_session(),
                    choices,
                });
                // Like VK, applying starts VK Mix again with the new choices.
                self.leave_mix();
                self.load_mix_from(ctx, true, None);
            }
        }
    }

    /// Start a Home genre, mood or artist mix, or toggle the one already playing.
    fn play_catalog_mix(&mut self, ctx: &egui::Context, mix: crate::web_audio::CatalogMix) {
        if self.demo || !self.search_ready() {
            return;
        }
        if self.mix_active
            && self.playing.is_some()
            && let (Some(MixSeed::Catalog(session)), Some((current, active))) =
                (self.mix_seed, &self.catalog_mix)
            && session == *current
            && *active == mix
        {
            self.toggle_playback(ctx);
            return;
        }
        let session = random_mix_session();
        self.mix_request = None;
        self.track_menu_error = None;
        self.catalog_mix = Some((session, mix));
        self.load_mix_from(ctx, true, Some(MixSeed::Catalog(session)));
    }

    fn load_mix(&mut self, ctx: &egui::Context, start: bool) {
        self.load_mix_from(ctx, start, self.mix_seed);
    }

    fn play_track_mix(&mut self, ctx: &egui::Context, key: (i64, i64)) {
        if self.demo || !self.search_ready() || key.0 == 0 || key.1 <= 0 {
            return;
        }
        self.mix_request = None;
        self.track_menu_error = None;
        self.load_mix_from(ctx, true, Some(MixSeed::Track(key)));
    }

    fn load_mix_from(&mut self, ctx: &egui::Context, start: bool, seed: Option<MixSeed>) {
        if start {
            self.playlist_controls.cancel_play();
        }
        if self.mix_request.is_some() {
            return;
        }
        if start && matches!(seed, Some(MixSeed::Playlist(_))) {
            self.clear_playlist_mix_notice();
        }
        let Some(session) = &self.session else {
            return;
        };
        let Some(client) = session.music.clone() else {
            if matches!(seed, Some(MixSeed::Playlist(_))) {
                self.mix_load_failed(ctx, start, seed, WebAudioError::NoSession);
            } else {
                self.mix_error = Some(WebAudioError::NoSession);
            }
            return;
        };
        let owner = session.profile.id;
        self.home_autoplay = None;
        self.advance_on_load = false;
        self.mix_error = None;
        let catalog = match seed {
            Some(MixSeed::Catalog(session)) => {
                let Some((_, mix)) = self
                    .catalog_mix
                    .as_ref()
                    .filter(|(current, _)| *current == session)
                else {
                    self.mix_load_failed(ctx, start, seed, WebAudioError::NoStream);
                    return;
                };
                Some(mix.clone())
            }
            _ => None,
        };
        let tuning = self.mix_tuning.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new().name("vk-mix".into()).spawn(move || {
            let result = match (seed, catalog) {
                (Some(MixSeed::Catalog(session)), Some(mix)) => {
                    client.catalog_mix(owner, &mix, session, &worker_cancel)
                }
                (Some(MixSeed::Catalog(_)), None) => Err(WebAudioError::NoStream),
                (Some(MixSeed::Track(key)), _) => client.track_mix(owner, key, &worker_cancel),
                (Some(MixSeed::Playlist((owner_id, id))), _) => client.playlist_mix(
                    owner,
                    &AlbumRef {
                        owner_id,
                        id,
                        title: String::new(),
                        access_key: None,
                        cover_url: None,
                    },
                    &worker_cancel,
                ),
                (None, _) => match &tuning {
                    Some(tuning) => client.vk_mix_tuned(owner, tuning, &worker_cancel),
                    None => client.vk_mix(owner, &worker_cancel),
                },
            };
            let _ = sender.send(result);
            repaint.request_repaint();
        }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.mix_request = Some(MixRequest {
                    owner,
                    start,
                    seed,
                    canceled,
                    receiver,
                });
            }
            Err(_) => {
                if matches!(seed, Some(MixSeed::Playlist(_))) {
                    self.mix_load_failed(ctx, start, seed, WebAudioError::Network);
                } else {
                    self.mix_error = Some(WebAudioError::Network);
                }
            }
        }
    }

    fn mix_load_failed(
        &mut self,
        ctx: &egui::Context,
        start: bool,
        seed: Option<MixSeed>,
        error: WebAudioError,
    ) {
        if matches!(seed, Some(MixSeed::Playlist(_))) {
            self.playlist_mix_unavailable(ctx);
            if !start {
                self.mix_error = Some(error);
            }
        } else {
            if start || seed.is_some() {
                self.track_menu_error = Some(error.clone());
            }
            if !start || !self.mix_active {
                self.mix_error = Some(error);
            }
        }
    }

    fn poll_mix(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.mix_request else {
            return;
        };
        let (owner, start, seed) = (request.owner, request.start, request.seed);
        let canceled = request.canceled.load(Ordering::Acquire);
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        self.mix_request = None;
        if canceled
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != owner)
        {
            return;
        }
        if !start && self.mix_seed != seed {
            return;
        }
        match result {
            Ok(page) => {
                if start {
                    if !page.tracks.is_empty() {
                        if matches!(seed, Some(MixSeed::Playlist(_))) {
                            self.clear_playlist_mix_notice();
                        }
                        self.mix_has_more = page.has_more;
                        self.mix_active = true;
                        self.mix_seed = seed;
                        self.mix_advance = false;
                        let origin = seed.map_or_else(
                            || "mix".into(),
                            |seed| match seed {
                                MixSeed::Track(key) => format!("track-mix:{}_{}", key.0, key.1),
                                MixSeed::Playlist(key) => {
                                    format!("playlist-mix:{}_{}", key.0, key.1)
                                }
                                MixSeed::Catalog(session) => format!("catalog-mix:{session}"),
                            },
                        );
                        self.start_queue_from(ctx, page.tracks, origin);
                    } else {
                        if matches!(seed, Some(MixSeed::Playlist(_))) {
                            self.mix_load_failed(ctx, true, seed, WebAudioError::NoStream);
                        } else {
                            if seed.is_some() {
                                self.track_menu_error = Some(WebAudioError::NoStream);
                            }
                            if !self.mix_active {
                                self.mix_error = Some(WebAudioError::NoStream);
                            }
                        }
                    }
                } else if self.mix_active && self.mix_seed == seed {
                    self.mix_has_more = page.has_more;
                    let queue = self.play_queue.get_or_insert_with(Vec::new);
                    let mut known: HashSet<_> = queue.iter().map(Track::key).collect();
                    let before = queue.len();
                    queue.extend(
                        page.tracks
                            .into_iter()
                            .filter(|track| known.insert(track.key())),
                    );
                    if queue.len() == before && self.mix_has_more {
                        self.mix_load_failed(ctx, false, seed, WebAudioError::Pagination);
                        return;
                    }
                    if let Some(position) = self.playing.as_ref().and_then(|playing| {
                        queue.iter().position(|track| track.key() == playing.key())
                    }) && queue.len() > 200
                        && position > 20
                        && !self.play_order.shuffle()
                    {
                        queue.drain(..position - 20);
                    }
                    self.synchronize_order();
                    if self.mix_advance {
                        self.mix_advance = false;
                        self.advance_track(ctx, false);
                    }
                }
            }
            Err(error @ WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => self.mix_load_failed(ctx, start, seed, error),
        }
    }

    fn view_tracks(&self) -> &[Track] {
        match &self.page {
            MusicPage::Recent => &self.recent,
            MusicPage::Search(query) => self
                .search
                .results
                .as_ref()
                .filter(|_| {
                    !query.is_empty()
                        && query == self.search.input.trim()
                        && query == &self.search.results_query
                })
                .map_or(&[], |results| results.tracks.as_slice()),
            MusicPage::Collection(id) => self
                .home_collections
                .iter()
                .find(|collection| &collection.id == id)
                .map_or(&[], |collection| collection.tracks.as_slice()),
            MusicPage::Artist(_)
            | MusicPage::Album(_)
            | MusicPage::Playlist(_)
            | MusicPage::Owner(_)
            | MusicPage::TrackAlbum(_, _) => self
                .page
                .detail_key()
                .and_then(|key| self.details.get(&key))
                .map_or(&[], |page| page.tracks()),
            MusicPage::Library if self.library_tail_mode() => &self.library_tail,
            MusicPage::Home | MusicPage::Library => &self.library.tracks,
        }
    }

    fn play_from_view(&mut self, ctx: &egui::Context, key: (i64, i64)) {
        self.unavailable_playback.clear();
        self.leave_mix();
        self.prepare_view_queue();
        self.play_order.select_source(key);
        self.play_track(ctx, key);
    }

    fn prepare_view_queue(&mut self) {
        let queue = if matches!(
            self.page,
            MusicPage::Recent
                | MusicPage::Search(_)
                | MusicPage::Collection(_)
                | MusicPage::Artist(_)
                | MusicPage::Album(_)
                | MusicPage::Playlist(_)
                | MusicPage::Owner(_)
                | MusicPage::TrackAlbum(_, _)
        ) {
            Some(self.view_tracks().to_vec())
        } else {
            None
        };
        // A sorted list plays in the order it is shown.
        let sort = self.current_sort();
        let queue = if sort.active() {
            let tracks = queue.as_deref().unwrap_or(if self.library_tail_mode() {
                &self.library_tail
            } else {
                &self.library.tracks
            });
            Some(
                track_sort::order(tracks, 0..tracks.len(), sort)
                    .into_iter()
                    .map(|index| tracks[index].clone())
                    .collect(),
            )
        } else {
            queue
        };
        self.replace_play_queue(queue);
    }

    fn current_sort(&self) -> track_sort::Sort {
        self.track_sorts
            .get(&self.view_queue_origin())
            .copied()
            .unwrap_or_default()
    }

    fn set_sort(&mut self, column: track_sort::Column) {
        let was_tail = self.library_tail_mode();
        let next = self.current_sort().clicked(column);
        let origin = self.view_queue_origin();
        if next.active() {
            self.track_sorts.insert(origin, next);
        } else {
            self.track_sorts.remove(&origin);
        }
        self.sort_cache = None;
        if was_tail != self.library_tail_mode() {
            self.track_view_cache.invalidate();
        }
    }

    /// A reversed My music that is not fully loaded reads from VK's end.
    fn library_tail_mode(&self) -> bool {
        self.page == MusicPage::Library
            && !self.library.exhausted
            && !self.demo
            && self
                .track_sorts
                .get("library")
                .is_some_and(|sort| sort.reversed())
    }

    /// Load the next 100 tracks from the end of My music.
    fn load_library_tail(&mut self, ctx: &egui::Context) {
        if self.library_tail_request.is_some() || self.library_tail_start == Some(0) {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let before = self.library_tail_start;
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        if let Ok(worker) = thread::Builder::new()
            .name("vk-library-tail".into())
            .spawn(move || {
                let _ = sender.send(client.library_tail(owner, before, &AtomicBool::new(false)));
                ctx.request_repaint();
            })
        {
            self.workers.push(worker);
            self.library_tail_request = Some((owner, receiver));
        }
    }

    fn poll_library_tail(&mut self) {
        let Some((owner, receiver)) = &self.library_tail_request else {
            return;
        };
        let owner = *owner;
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        self.library_tail_request = None;
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.profile.id != owner)
        {
            return;
        }
        match result {
            Ok((start, mut tracks)) => {
                let known: HashSet<_> = self.library_tail.iter().map(Track::key).collect();
                tracks.retain(|track| !known.contains(&track.key()));
                tracks.append(&mut self.library_tail);
                self.library_tail = tracks;
                self.library_tail_start = Some(start);
                self.track_view_cache.invalidate();
                self.sort_cache = None;
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => self.audio_error = Some(UiError::Music(error)),
        }
    }

    /// "Date added" only for the account's own lists, and only when VK sent
    /// the dates.
    fn show_date_column(&self) -> bool {
        let owner = self.session.as_ref().map(|session| session.profile.id);
        let own = match &self.page {
            MusicPage::Library => true,
            MusicPage::Playlist(reference) => owner == Some(reference.owner_id),
            _ => false,
        };
        own && self.view_tracks().iter().any(|track| track.added.is_some())
    }

    /// The list's visible rows in the chosen order, reused until the list,
    /// its filter or the sort changes.
    fn sorted_rows(
        &mut self,
        visible: track_view_cache::VisibleRows,
    ) -> track_view_cache::VisibleRows {
        let sort = self.current_sort();
        if !sort.active() {
            return visible;
        }
        let origin = self.view_queue_origin();
        let revision = self.track_view_cache.revision();
        let length = self.view_tracks().len();
        if let Some((cached_origin, cached_sort, cached_revision, cached_length, filter, rows)) =
            &self.sort_cache
            && *cached_origin == origin
            && *cached_sort == sort
            && *cached_revision == revision
            && *cached_length == length
            && *filter == self.filter
        {
            return track_view_cache::VisibleRows::Matching(rows.clone());
        }
        let rows: Arc<[usize]> = track_sort::order(
            self.view_tracks(),
            (0..visible.len()).map(|row| visible.source_index(row)),
            sort,
        )
        .into();
        self.sort_cache = Some((
            origin,
            sort,
            revision,
            length,
            self.filter.clone(),
            rows.clone(),
        ));
        track_view_cache::VisibleRows::Matching(rows)
    }

    fn search_ready(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.music.is_some())
            && (self.library.offset > 0 || self.library.exhausted)
    }

    fn spawn_search<T: Send + 'static>(
        &mut self,
        ctx: &egui::Context,
        query: String,
        offset: usize,
        job: impl FnOnce(WebAudioClient, i64, &AtomicBool) -> Result<T, WebAudioError> + Send + 'static,
    ) -> Result<SearchRequest<T>, WebAudioError> {
        let session = self.session.as_ref().ok_or(WebAudioError::NoSession)?;
        let client = session.music.clone().ok_or(WebAudioError::NoSession)?;
        let owner = session.profile.id;
        let generation = self.search.generation;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        let worker = thread::Builder::new()
            .name("vk-search".into())
            .spawn(move || {
                let result = job(client, owner, &worker_cancel);
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .map_err(|_| WebAudioError::Network)?;
        self.workers.push(worker);
        Ok(SearchRequest {
            owner,
            query,
            offset,
            generation,
            canceled,
            receiver,
        })
    }

    fn restore_search(&mut self, ctx: &egui::Context, query: &str) {
        if !query.is_empty() && self.search.input != query {
            self.search.input = query.to_owned();
            self.search.edit();
        }
        if query.is_empty() {
            return;
        }
        if self.search.results_query != query || self.search.results.is_none() {
            self.search.results = self.search.cache.get(query).cloned();
            self.search.results_query = query.to_owned();
        }
        if self.search.results.is_none()
            && self.search.results_request.is_none()
            && self.search.results_error.is_none()
            && self.search_ready()
        {
            self.start_search_results(ctx, 0);
        }
    }

    fn submit_search(&mut self, ctx: &egui::Context, query: String) {
        let query = query.trim().chars().take(512).collect::<String>();
        self.search.input = query.clone();
        self.search.edit();
        self.page = MusicPage::Search(query.clone());
        self.scrolling.stop();
        if self.search.results_query != query {
            self.search.results = self.search.cache.get(&query).cloned();
        }
        self.search.results_query = query;
        self.search.category = SearchCategory::All;
        if !self.search.input.is_empty() && self.search_ready() {
            self.start_search_results(ctx, 0);
        }
        ctx.request_repaint();
    }

    fn start_search_results(&mut self, ctx: &egui::Context, offset: usize) {
        if self.search.results_request.is_some() || !self.search_ready() {
            return;
        }
        let query = self.search.input.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.search.results_error = None;
        let worker_query = query.clone();
        match self.spawn_search(ctx, query, offset, move |client, owner, cancel| {
            client.search(owner, &worker_query, offset, cancel)
        }) {
            Ok(request) => self.search.results_request = Some(request),
            Err(error) => {
                self.search.failed_offset = offset;
                self.search.results_error = Some(error);
            }
        }
    }

    fn drive_search(&mut self, ctx: &egui::Context) {
        if self.settings_open || !matches!(self.page, MusicPage::Search(_)) || !self.search_ready()
        {
            return;
        }
        let query = self.search.input.trim().to_owned();
        if query.is_empty() {
            if !self.search.popular_requested {
                self.search.popular_requested = true;
                match self.spawn_search(ctx, String::new(), 0, |client, owner, cancel| {
                    client.search_popular(owner, cancel)
                }) {
                    Ok(request) => self.search.popular_request = Some(request),
                    Err(error) => self.search.popular_error = Some(error),
                }
            }
            return;
        }
        let submitted = matches!(&self.page, MusicPage::Search(submitted) if submitted == &query);
        if submitted {
            if self.search.results.is_none()
                && self.search.results_request.is_none()
                && self.search.results_error.is_none()
            {
                self.start_search_results(ctx, 0);
            }
            return;
        }
        if self.search.hints_request.is_none()
            && self.search.hints_for != query
            && self.search.hints_due.is_none()
        {
            self.search.hints_due = Some(
                Instant::now()
                    + Duration::from_millis(if self.search_on_enter { 250 } else { 400 }),
            );
        }
        if let Some(deadline) = self.search.hints_due {
            if Instant::now() < deadline {
                ctx.request_repaint_after(deadline.saturating_duration_since(Instant::now()));
                return;
            }
            self.search.hints_due = None;
            if !self.search_on_enter {
                // Without Enter, a pause in typing starts the search itself.
                self.page = MusicPage::Search(query.clone());
                if self.search.results_query != query {
                    self.search.results = self.search.cache.get(&query).cloned();
                }
                self.search.results_query = query;
                self.search.category = SearchCategory::All;
                self.start_search_results(ctx, 0);
                return;
            }
            self.search.hints_for = query.clone();
            let worker_query = query.clone();
            match self.spawn_search(ctx, query, 0, move |client, owner, cancel| {
                client.search_hints(owner, &worker_query, cancel)
            }) {
                Ok(request) => self.search.hints_request = Some(request),
                Err(error) => self.search.hints_error = Some(error),
            }
        }
    }

    fn poll_search(&mut self, _ctx: &egui::Context) {
        if let Some((request, result)) = search_reply(&mut self.search.popular_request)
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.profile.id == request.owner)
        {
            match result {
                Ok(queries) => {
                    self.search.popular = queries;
                    self.search.popular_error = None;
                }
                Err(error @ WebAudioError::WrongAccount) => {
                    self.reset();
                    self.auth_error = Some(UiError::Music(error));
                    return;
                }
                Err(WebAudioError::Canceled) => {}
                Err(error) => self.search.popular_error = Some(error),
            }
        }
        if let Some((request, result)) = search_reply(&mut self.search.hints_request)
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.profile.id == request.owner)
            && request.generation == self.search.generation
            && request.query == self.search.input.trim()
        {
            match result {
                Ok(queries) => {
                    self.search.hints = queries;
                    self.search.hints_error = None;
                }
                Err(error @ WebAudioError::WrongAccount) => {
                    self.reset();
                    self.auth_error = Some(UiError::Music(error));
                    return;
                }
                Err(WebAudioError::Canceled) => {}
                Err(error) => self.search.hints_error = Some(error),
            }
        }
        if let Some((request, result)) = search_reply(&mut self.search.results_request)
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.profile.id == request.owner)
            && request.generation == self.search.generation
            && request.query == self.search.input.trim()
        {
            match result {
                Ok(mut next) => {
                    let page = if request.offset == 0 {
                        Arc::new(next)
                    } else {
                        self.search.cache.remove(&request.query);
                        let mut page = self.search.results.take().unwrap_or_else(|| {
                            Arc::new(crate::web_audio::SearchResults {
                                tracks: Vec::new(),
                                artists: Vec::new(),
                                artist_images: HashMap::new(),
                                albums: Vec::new(),
                                playlists: Vec::new(),
                                has_more: false,
                                next_offset: 0,
                            })
                        });
                        let current = Arc::make_mut(&mut page);
                        let before = current.tracks.len();
                        let mut seen: HashSet<_> = current.tracks.iter().map(Track::key).collect();
                        current.tracks.extend(
                            next.tracks
                                .drain(..)
                                .filter(|track| seen.insert(track.key())),
                        );
                        current.has_more = next.has_more
                            && next.next_offset > request.offset
                            && current.tracks.len() > before;
                        current.next_offset = next.next_offset;
                        page
                    };
                    if self.search.cache.len() >= 8
                        && !self.search.cache.contains_key(&request.query)
                        && let Some(key) = self
                            .search
                            .cache
                            .keys()
                            .find(|key| *key != &request.query)
                            .cloned()
                    {
                        self.search.cache.remove(&key);
                    }
                    self.search
                        .cache
                        .insert(request.query.clone(), page.clone());
                    self.search.results = Some(page);
                    self.search.results_query = request.query.clone();
                    self.search.results_error = None;
                    self.search.failed_offset = 0;
                }
                Err(error @ WebAudioError::WrongAccount) => {
                    self.reset();
                    self.auth_error = Some(UiError::Music(error));
                }
                Err(WebAudioError::Canceled) => {}
                Err(error) => {
                    self.search.failed_offset = request.offset;
                    self.search.results_error = Some(error);
                }
            }
        }
    }

    fn handle_global_search(&mut self, ui: &mut egui::Ui, input: search_ui::SearchInput) {
        let ctx = ui.ctx().clone();
        let on_search = !self.settings_open && matches!(self.page, MusicPage::Search(_));
        if input.changed {
            // A pasted VK Music link opens the track or page, not a search.
            if let Some(target) = crate::link_router::parse(&self.search.input) {
                self.search.input.clear();
                self.search.edit();
                self.pending_link = Some(target);
                ctx.request_repaint();
                return;
            }
            let value = self.search.input.clone();
            if !value.trim().is_empty() && !on_search {
                self.navigate_music(&ctx, MusicPage::Search(String::new()));
                self.search.input = value;
            }
            self.search.edit();
            if self.search.input.trim().is_empty() {
                self.search.results_query.clear();
                self.search.results = None;
                if matches!(self.page, MusicPage::Search(_)) {
                    self.page = MusicPage::Search(String::new());
                }
            }
            self.scrolling.stop();
            ctx.request_repaint();
        }
        let query = self.search.input.trim().to_owned();
        let on_search = !self.settings_open && matches!(self.page, MusicPage::Search(_));
        let submitted = !query.is_empty()
            && matches!(&self.page, MusicPage::Search(submitted) if submitted == &query);
        if input.focused
            && on_search
            && !submitted
            && !self.search.hints.is_empty()
            && self.search.hints_for == query
        {
            let count = self.search.hints.len().min(search_ui::MAX_HINTS);
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
            {
                self.search.selected_hint = Some(
                    self.search
                        .selected_hint
                        .map_or(0, |selected| (selected + 1) % count),
                );
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                self.search.selected_hint = Some(
                    self.search
                        .selected_hint
                        .map_or(count - 1, |selected| (selected + count - 1) % count),
                );
            }
        }
        if input.submitted {
            let selected = on_search
                .then(|| {
                    self.search
                        .selected_hint
                        .and_then(|index| self.search.hints.get(index))
                        .cloned()
                })
                .flatten();
            if !on_search {
                let value = self.search.input.clone();
                self.navigate_music(&ctx, MusicPage::Search(String::new()));
                self.search.input = value;
            }
            self.submit_search(&ctx, selected.unwrap_or(query));
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        ui.horizontal(|ui| {
            let caption_width = if self.system_titlebar {
                0.0
            } else {
                titlebar::controls_width(ui.ctx())
            };
            let caption_room = caption_width
                + if caption_width > 0.0 {
                    ui.spacing().item_spacing.x
                } else {
                    0.0
                };
            #[cfg(any(windows, test))]
            let update_width = self.updater.notification_width();
            #[cfg(not(any(windows, test)))]
            let update_width = 0.0;
            let available = (ui.available_width() - 96.0 - caption_room - update_width).max(80.0);
            let search_width = if self.settings_open {
                available
            } else {
                let drag_width = if self.system_titlebar { 0.0 } else { 32.0 };
                search_ui::preferred_input_width(ui).min((available - drag_width).max(80.0))
            };
            let input = ui
                .allocate_ui_with_layout(
                    egui::vec2(search_width, 38.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if self.settings_open {
                            None
                        } else {
                            Some(search_ui::input(ui, &mut self.search.input, language))
                        }
                    },
                )
                .inner;
            if let Some(input) = input {
                self.handle_global_search(ui, input);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.system_titlebar {
                    titlebar::controls(ui, language);
                }
                self.account_button(ui);
                if icons::compact_button(
                    ui,
                    Icon::Settings,
                    language.text("Настройки", "Settings"),
                    true,
                    self.settings_open,
                )
                .clicked()
                {
                    if self.settings_open {
                        self.go_back(ui.ctx());
                    } else {
                        self.open_settings(ui.ctx());
                    }
                }
                #[cfg(any(windows, test))]
                self.updater.notification_button(ui, language);
            });
        });
    }

    fn search_content(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let language = self.language;
        let query = self.search.input.trim().to_owned();
        let submitted = !query.is_empty()
            && matches!(&self.page, MusicPage::Search(submitted) if submitted == &query);
        if self.session.is_none() {
            ui.label(language.text(
                "Войдите в VK для поиска музыки.",
                "Sign in with VK to search music.",
            ));
            self.login_controls(ui);
            return;
        }
        let mut action = None;
        let mut track_action = None;
        let results = self
            .search
            .results
            .clone()
            .filter(|_| submitted && self.search.results_query == query);
        let scroll = theme::scroll_area(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("search-content", &query))
                .auto_shrink([false, false]),
            |ui| {
                if query.is_empty() {
                    let error = self
                        .search
                        .popular_error
                        .as_ref()
                        .map(|error| error.localized(language));
                    action = search_ui::queries(
                        ui,
                        language,
                        search_ui::QueryKind::Popular,
                        &self.search.popular,
                        self.search.popular_request.is_some() || !self.search.popular_requested,
                        error.as_deref(),
                        None,
                    );
                } else if !submitted {
                    let error = self
                        .search
                        .hints_error
                        .as_ref()
                        .map(|error| error.localized(language));
                    action = search_ui::queries(
                        ui,
                        language,
                        search_ui::QueryKind::Suggestions,
                        &self.search.hints,
                        self.search.hints_request.is_some()
                            || self.search.hints_due.is_some()
                            || self.search.hints_for != query,
                        error.as_deref(),
                        self.search.selected_hint,
                    );
                } else {
                    if let Some(error) = &self.search.results_error {
                        theme::inset_heading(ui, |ui| {
                            ui.colored_label(
                                Color32::from_rgb(255, 158, 153),
                                error.localized(language),
                            )
                        });
                    }
                    if let Some(results) = &results {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.horizontal(|ui| {
                            for (category, title) in [
                                (SearchCategory::All, language.text("Всё", "All")),
                                (SearchCategory::Tracks, language.text("Треки", "Tracks")),
                                (
                                    SearchCategory::Artists,
                                    language.text("Исполнители", "Artists"),
                                ),
                                (SearchCategory::Albums, language.text("Альбомы", "Albums")),
                                (
                                    SearchCategory::Playlists,
                                    language.text("Плейлисты", "Playlists"),
                                ),
                            ] {
                                let previous = self.search.category;
                                if ui
                                    .selectable_value(&mut self.search.category, category, title)
                                    .clicked()
                                    && previous != category
                                {
                                    self.search.swipe.start(
                                        previous as usize,
                                        category as usize,
                                        ui.input(|input| input.time),
                                    );
                                    self.scrolling.stop();
                                }
                            }
                        });
                        ui.add_space(10.0);
                        (action, track_action) = self.render_search_categories(ui, results);
                    }
                    if self.search.results_request.is_some() {
                        theme::inset_heading(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(language.text("Ищем музыку…", "Searching music…"));
                                ui.spinner();
                            })
                        });
                    }
                }
                let error = if query.is_empty() {
                    self.search.popular_error.is_some()
                } else if submitted {
                    self.search.results_error.is_some()
                } else {
                    self.search.hints_error.is_some()
                };
                if error
                    && theme::inset_heading(ui, |ui| ui.button(language.text("Повторить", "Retry")))
                        .clicked()
                {
                    if query.is_empty() {
                        self.search.popular_error = None;
                        self.search.popular_requested = false;
                    } else if submitted {
                        self.start_search_results(&ctx, self.search.failed_offset);
                    } else {
                        self.search.hints_for.clear();
                        self.search.hints_error = None;
                    }
                }
            },
        );
        match action {
            Some(search_ui::Action::Search(query)) => self.submit_search(&ctx, query),
            Some(search_ui::Action::OpenArtist(artist)) => {
                self.navigate_music(&ctx, MusicPage::Artist(artist))
            }
            Some(search_ui::Action::OpenAlbum(album)) => {
                self.navigate_music(&ctx, MusicPage::Album(album))
            }
            Some(search_ui::Action::OpenPlaylist(playlist)) => {
                self.navigate_music(&ctx, MusicPage::Playlist(playlist))
            }
            None => {}
        }
        if let Some(action) = track_action {
            self.handle_track_action(&ctx, action);
        }
        if let Some(results) = results
            && submitted
            && matches!(
                self.search.category,
                SearchCategory::All | SearchCategory::Tracks
            )
            && self.search.results_request.is_none()
            && self.search.results_error.is_none()
            && results.has_more
            && matches!(&self.page, MusicPage::Search(current) if current == &query)
            && scroll.state.offset.y + scroll.inner_rect.height() + 116.0 >= scroll.content_size.y
        {
            self.start_search_results(&ctx, results.next_offset);
        }
    }

    fn render_search_categories(
        &mut self,
        ui: &mut egui::Ui,
        results: &crate::web_audio::SearchResults,
    ) -> (Option<search_ui::Action>, Option<TrackAction>) {
        let available = ui.available_rect_before_wrap();
        let width = available.width().max(1.0);
        let bounds = egui::Rect::from_min_size(
            available.min,
            egui::vec2(width, available.height().max(1.0)),
        );
        let clip = ui.clip_rect().intersect(egui::Rect::from_min_max(
            egui::pos2(bounds.left(), ui.clip_rect().top()),
            egui::pos2(bounds.right(), ui.clip_rect().bottom()),
        ));
        let animation = self.search.swipe.frame(ui.input(|input| input.time), width);
        if let Some(frame) = animation {
            let from = match frame.outgoing {
                0 => SearchCategory::All,
                1 => SearchCategory::Tracks,
                2 => SearchCategory::Artists,
                3 => SearchCategory::Albums,
                _ => SearchCategory::Playlists,
            };
            let mut outgoing = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("search-category-outgoing", frame.outgoing))
                    .max_rect(bounds.translate(egui::vec2(frame.outgoing_x, 0.0)))
                    .layout(Layout::top_down(Align::Min)),
            );
            outgoing.set_clip_rect(clip);
            outgoing.disable();
            outgoing.set_opacity(1.0);
            let _ = self.render_search_category(&mut outgoing, results, from);
        }
        let offset = animation.map_or(0.0, |frame| frame.incoming_x);
        let index = animation.map_or(self.search.category as usize, |frame| frame.incoming);
        let mut incoming = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("search-category", index))
                .max_rect(bounds.translate(egui::vec2(offset, 0.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        incoming.set_clip_rect(clip);
        if animation.is_some() {
            incoming.disable();
            incoming.set_opacity(1.0);
        }
        let action = self.render_search_category(&mut incoming, results, self.search.category);
        let height = incoming.min_rect().height().max(0.0);
        ui.advance_cursor_after_rect(egui::Rect::from_min_size(
            bounds.min,
            egui::vec2(width, height),
        ));
        if animation.is_some() {
            ui.ctx().request_repaint();
            (None, None)
        } else {
            action
        }
    }

    fn render_search_category(
        &mut self,
        ui: &mut egui::Ui,
        results: &crate::web_audio::SearchResults,
        category: SearchCategory,
    ) -> (Option<search_ui::Action>, Option<TrackAction>) {
        let language = self.language;
        let mut action = None;
        let mut track_action = None;
        match category {
            SearchCategory::All => {
                action = search_ui::results_metadata(
                    ui,
                    &mut self.covers,
                    language,
                    &results.artists,
                    &results.albums,
                    &results.artist_images,
                );
                action = search_ui::playlist_results(
                    ui,
                    &mut self.covers,
                    language,
                    &results.playlists,
                    true,
                    Some(6),
                )
                .or(action);
                if !results.tracks.is_empty() {
                    if !results.artists.is_empty()
                        || !results.albums.is_empty()
                        || !results.playlists.is_empty()
                    {
                        ui.add_space(4.0);
                    }
                    theme::inset_heading(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(language.text("Треки", "Tracks"))
                                    .size(20.0)
                                    .family(theme::semibold()),
                            )
                            .selectable(false),
                        )
                    });
                    ui.add_space(4.0);
                    track_action = self.search_track_rows(ui, &results.tracks);
                } else if results.artists.is_empty()
                    && results.albums.is_empty()
                    && results.playlists.is_empty()
                {
                    theme::inset_heading(ui, |ui| {
                        ui.label(language.text("Ничего не найдено.", "No results found."))
                    });
                }
            }
            SearchCategory::Tracks => {
                if results.tracks.is_empty() {
                    theme::inset_heading(ui, |ui| {
                        ui.label(language.text("Ничего не найдено.", "No results found."))
                    });
                } else {
                    track_action = self.search_track_rows(ui, &results.tracks);
                }
            }
            SearchCategory::Artists | SearchCategory::Albums => {
                let artists = if category == SearchCategory::Artists {
                    results.artists.as_slice()
                } else {
                    &[]
                };
                let albums = if category == SearchCategory::Albums {
                    results.albums.as_slice()
                } else {
                    &[]
                };
                action = search_ui::results_metadata_sections(
                    ui,
                    &mut self.covers,
                    language,
                    artists,
                    albums,
                    false,
                    &results.artist_images,
                );
                if artists.is_empty() && albums.is_empty() {
                    theme::inset_heading(ui, |ui| {
                        ui.label(language.text("Ничего не найдено.", "No results found."))
                    });
                }
            }
            SearchCategory::Playlists => {
                action = search_ui::playlist_results(
                    ui,
                    &mut self.covers,
                    language,
                    &results.playlists,
                    false,
                    None,
                );
                if results.playlists.is_empty() {
                    theme::inset_heading(ui, |ui| {
                        ui.label(language.text("Ничего не найдено.", "No results found."))
                    });
                }
            }
        }
        (action, track_action)
    }

    fn search_track_rows(&mut self, ui: &mut egui::Ui, tracks: &[Track]) -> Option<TrackAction> {
        let active = matches!(
            self.player.snapshot().state,
            PlaybackState::Playing | PlaybackState::Loading
        );
        self.clipped_track_rows(
            ui,
            tracks,
            1,
            active,
            "search-track-rows",
            track_sort::Extra::default(),
            false,
        )
    }

    // Reserve the complete list height, but lay out only rows in the viewport.
    // Detail pages share this with search so large playlists do not create
    // widgets and text layouts for every off-screen track.
    /// Track rows that can be dragged into a new order, sliding like the
    /// sidebar. Rows are looked up in the open list by display row.
    #[allow(clippy::too_many_arguments)]
    fn lane_track_rows(
        &mut self,
        ui: &mut egui::Ui,
        id: &'static str,
        origin_y: f32,
        range: std::ops::Range<usize>,
        count: usize,
        source: &dyn Fn(usize) -> usize,
        active: bool,
        show_date: track_sort::Extra,
        visit: &mut dyn FnMut(&Track),
    ) -> Option<TrackAction> {
        let lane_id = egui::Id::new(("track-row-drag", id));
        // The open page is the list: a track on two pages starts in place.
        let scope = row_drag::scope_key(&self.view_queue_origin());
        let mut lane = row_drag::Lane::begin(ui, lane_id, origin_y, 58.0, 54.0, scope, |key| {
            let tracks = self.view_tracks();
            (0..count).position(|row| {
                tracks
                    .get(source(row))
                    .is_some_and(|track| row_drag::track_key(track.key()) == key)
            })
        });
        let lifted = lane.dragging() || lane.suppressing();
        let mut action = None;
        // One extra row at each edge covers rows sliding into view.
        for row in range.start.saturating_sub(1)..(range.end + 1).min(count) {
            let index = source(row);
            let Some(track) = self.view_tracks().get(index).cloned() else {
                continue;
            };
            if range.contains(&row) {
                visit(&track);
            }
            let key = row_drag::track_key(track.key());
            if lane.hides(key) {
                continue;
            }
            let rect = lane.rect(ui.ctx(), row, key);
            self.row_drag_enabled = !lane.dragging();
            let intent = ui
                .scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .id_salt(("lane-row", track.key())),
                    |ui| self.track_row(ui, &track, index, active, show_date),
                )
                .inner;
            self.row_drag_enabled = false;
            if let Some(started) = self.row_drag_started.take() {
                lane.start(ui, row, key, started, 0..=count.saturating_sub(1));
            }
            if !lifted && !lane.dragging() && intent.is_some() {
                action = intent;
            }
        }
        if let Some(moved) = lane.moved {
            action = Some(TrackAction::Reorder(moved));
        }
        if let Some((from, rect)) = lane.finish(ui)
            && let Some(track) = self.view_tracks().get(source(from)).cloned()
        {
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(rect).id_salt("lane-ghost"),
                |ui| {
                    self.track_row(ui, &track, source(from), active, show_date);
                },
            );
        }
        action
    }

    #[allow(clippy::too_many_arguments)]
    fn clipped_track_rows(
        &mut self,
        ui: &mut egui::Ui,
        tracks: &[Track],
        columns: usize,
        active: bool,
        id: &'static str,
        show_date: track_sort::Extra,
        reorder: bool,
    ) -> Option<TrackAction> {
        if tracks.is_empty() {
            return None;
        }
        let stride = 58.0;
        let rows = tracks.len().div_ceil(columns);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), rows as f32 * stride - 4.0),
            Sense::hover(),
        );
        let first = ((ui.clip_rect().top() - rect.top()) / stride)
            .floor()
            .max(0.0) as usize;
        let last = (((ui.clip_rect().bottom() - rect.top()) / stride)
            .ceil()
            .max(0.0) as usize
            + 1)
        .min(rows);
        if first >= last {
            return None;
        }
        let mut action = None;
        if reorder && columns == 1 {
            // Rows of the open list itself, so drags find them again.
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt(id)
                    .max_rect(rect)
                    .layout(Layout::top_down(Align::Min)),
                |ui| {
                    action = self.lane_track_rows(
                        ui,
                        id,
                        rect.top(),
                        first..last,
                        rows,
                        &|row| row,
                        active,
                        show_date,
                        &mut |_| {},
                    );
                },
            );
            ui.advance_cursor_after_rect(rect);
            return action;
        }
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(id)
                .max_rect(egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() + first as f32 * stride),
                    rect.max,
                ))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for row in first..last {
                    let row_id = ui.make_persistent_id(("clipped-track-row", row));
                    ui.scope_builder(egui::UiBuilder::new().id(row_id), |ui| {
                        if columns == 1 {
                            if let Some(intent) =
                                self.track_row(ui, &tracks[row], row, active, show_date)
                            {
                                action = Some(intent);
                            }
                        } else {
                            ui.columns(columns, |uis| {
                                for (column, column_ui) in uis.iter_mut().enumerate() {
                                    let index = row * columns + column;
                                    if let Some(track) = tracks.get(index)
                                        && let Some(intent) = self.track_row(
                                            column_ui,
                                            track,
                                            index,
                                            active,
                                            track_sort::Extra::default(),
                                        )
                                    {
                                        action = Some(intent);
                                    }
                                }
                            });
                        }
                    });
                }
            },
        );
        ui.advance_cursor_after_rect(rect);
        action
    }

    pub fn open_search(&mut self, ctx: &egui::Context, query: Option<String>) {
        let query = query.unwrap_or_else(|| self.search.results_query.clone());
        self.navigate_music(ctx, MusicPage::Search(query));
    }

    pub fn open_home(&mut self, ctx: &egui::Context) {
        self.navigate_music(ctx, MusicPage::Home);
        if !self.home_requested && (self.library.offset > 0 || self.library.exhausted) {
            self.load_home(ctx);
        }
    }

    fn playlist_ready(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.music.is_some())
            && (self.library.offset > 0 || self.library.exhausted)
    }

    fn load_playlists(&mut self, ctx: &egui::Context, refresh: bool) {
        if self.playlist_request.is_some()
            || (!refresh && self.playlists_requested)
            || !self.playlist_ready()
        {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        self.playlists_requested = true;
        self.playlist_error = None;
        self.start_screen_cache(ctx);
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-playlists".into())
            .spawn(move || {
                let result = client
                    .playlists(owner, &worker_cancel)
                    .map_err(|error| client.confirm_expired(owner, error));
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_request = Some(PlaylistRequest {
                    owner,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.playlist_error = Some(WebAudioError::Network),
        }
    }

    fn poll_playlists(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.playlist_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self
            .playlist_request
            .take()
            .expect("Active playlist request");
        if request.canceled.load(Ordering::Acquire)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != request.owner)
        {
            return;
        }
        match result {
            Ok(playlists) => {
                self.editable_playlists = playlists
                    .iter()
                    .filter(|playlist| playlist.can_add_tracks)
                    .map(|playlist| playlist.album.clone())
                    .collect();
                self.playlist_items = playlists
                    .into_iter()
                    .map(|playlist| playlist_ui::PlaylistItem {
                        kind: playlist.kind,
                        reference: playlist.album,
                        subtitle: playlist.byline,
                        can_edit: playlist.can_add_tracks,
                        pinned: false,
                    })
                    .collect();
                self.remember_playlist_order();
                self.sync_playlist_pins();
                self.playlist_error = None;
                self.save_library_cache();
            }
            Err(
                error @ (WebAudioError::Expired
                | WebAudioError::NoSession
                | WebAudioError::WrongAccount),
            ) => {
                crate::diagnostics::event(
                    "session",
                    &format!("account_disconnected stage=playlists reason={error:?}"),
                );
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => self.playlist_error = Some(error),
        }
        ctx.request_repaint();
    }

    fn playlist_create_window(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.playlist_create_form else {
            return;
        };
        let error = form.error.as_ref().map(|error| {
            if *error == WebAudioError::Rejected {
                self.language
                    .text(
                        "VK не разрешил создать плейлист.",
                        "VK did not allow this playlist to be created.",
                    )
                    .into()
            } else {
                error.localized(self.language)
            }
        });
        let action = playlist_create_ui::show(
            ctx,
            playlist_create_ui::View {
                language: self.language,
                name: &mut form.name,
                creating: self.playlist_create_request.is_some(),
                error: error.as_deref(),
                focus_name: form.focus_name,
            },
        );
        form.focus_name = false;
        match action {
            Some(playlist_create_ui::Action::Cancel) => self.playlist_create_form = None,
            Some(playlist_create_ui::Action::Submit) => self.create_playlist(ctx),
            None => {}
        }
    }

    fn create_playlist(&mut self, ctx: &egui::Context) {
        if self.playlist_create_request.is_some() {
            return;
        }
        let Some(form) = &mut self.playlist_create_form else {
            return;
        };
        let title = form.name.trim().to_owned();
        if title.is_empty()
            || title.chars().count() > crate::web_audio::PLAYLIST_TITLE_MAX_CHARS
            || title.chars().any(char::is_control)
        {
            return;
        }
        let Some((owner, client)) = self
            .session
            .as_ref()
            .and_then(|session| {
                session
                    .music
                    .clone()
                    .map(|client| (session.profile.id, client))
            })
            .filter(|_| !self.demo)
        else {
            form.error = Some(WebAudioError::NoSession);
            return;
        };
        form.error = None;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-playlist-create".into())
            .spawn(move || {
                let result = client.create_playlist(owner, &title, &worker_cancel);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_create_request = Some(PlaylistCreateRequest {
                    owner,
                    canceled,
                    receiver,
                });
            }
            Err(_) => form.error = Some(WebAudioError::Network),
        }
    }

    fn poll_playlist_create(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.playlist_create_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self
            .playlist_create_request
            .take()
            .expect("Active playlist creation");
        if request.canceled.load(Ordering::Acquire)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != request.owner)
        {
            return;
        }
        match result {
            Ok(playlist) => {
                let reference = playlist.album;
                self.playlist_create_form = None;
                // A read begun before creation must not replace the new entry.
                self.playlist_request = None;
                self.playlist_items.retain(|item| {
                    item.reference.owner_id != reference.owner_id
                        || item.reference.id != reference.id
                });
                if playlist.can_add_tracks {
                    self.editable_playlists.retain(|item| {
                        item.owner_id != reference.owner_id || item.id != reference.id
                    });
                    self.editable_playlists.insert(0, reference.clone());
                }
                self.playlist_items.insert(
                    0,
                    playlist_ui::PlaylistItem {
                        kind: playlist.kind,
                        reference: reference.clone(),
                        subtitle: playlist.byline,
                        can_edit: playlist.can_add_tracks,
                        pinned: false,
                    },
                );
                self.sync_playlist_pins();
                self.playlists_requested = true;
                self.playlist_error = None;
                self.load_playlists(ctx, true);
                self.navigate_music(ctx, MusicPage::Playlist(reference));
                self.track_menu_notice = Some((
                    self.language
                        .text("Плейлист создан", "Playlist created")
                        .into(),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error @ WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(error) => {
                if let Some(form) = &mut self.playlist_create_form {
                    form.error = Some(error);
                }
            }
        }
        ctx.request_repaint();
    }

    fn load_home(&mut self, ctx: &egui::Context) {
        if self.home_request.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        self.home_requested = true;
        self.home_error = None;
        self.start_screen_cache(ctx);
        self.load_home_catalog(ctx, client.clone(), owner);
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-home".into())
            .spawn(move || {
                let result = client
                    .home(owner, &worker_cancel)
                    .map(HomeReply::Catalog)
                    .map_err(|error| client.confirm_expired(owner, error));
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.home_request = Some(HomeRequest {
                    owner,
                    collection: None,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.home_error = Some(WebAudioError::Network),
        }
    }

    fn open_collection(&mut self, ctx: &egui::Context, id: String, reload: bool) {
        self.home_autoplay = None;
        if !self.home_sources.iter().any(|source| source.id == id) {
            return;
        }
        self.navigate_music(ctx, MusicPage::Collection(id.clone()));
        self.home_error = None;
        if !reload
            && self
                .home_collections
                .iter()
                .any(|collection| collection.id == id && !collection.tracks.is_empty())
        {
            return;
        }
        self.load_collection_tracks(ctx, id);
    }

    /// Which album or playlist is current or loading, for cover play buttons.
    fn cover_playback(&self) -> crate::cover_play::Playback {
        let active = self
            .queue_origin
            .as_deref()
            .filter(|_| !self.mix_active && self.playing.is_some())
            .and_then(|origin| {
                origin
                    .strip_prefix("album:")
                    .or_else(|| origin.strip_prefix("playlist:"))
            })
            .and_then(|key| key.split_once('_'))
            .and_then(|(owner, id)| Some((owner.parse().ok()?, id.parse().ok()?)));
        crate::cover_play::Playback {
            pending: self.playlist_controls.pending_play(),
            active,
            audible: matches!(
                self.player.snapshot().state,
                PlaybackState::Playing | PlaybackState::Loading
            ),
        }
    }

    /// A cover's play button: toggle the current collection, or start it in
    /// the background without leaving the page.
    fn play_cover(&mut self, ctx: &egui::Context, reference: AlbumRef, page: MusicPage) {
        let key = (reference.owner_id, reference.id);
        if self.cover_playback().active == Some(key) {
            self.toggle_playback(ctx);
        } else {
            self.play_collection(ctx, reference, page, false);
        }
    }

    /// Home's play button: start the collection, or toggle it when it is the
    /// current queue. Missing tracks load in place; it never opens the page.
    fn play_home_collection(&mut self, ctx: &egui::Context, id: String) {
        let origin = format!("collection:{id}");
        if !self.mix_active
            && self.playing.is_some()
            && self.queue_origin.as_deref() == Some(origin.as_str())
        {
            self.toggle_playback(ctx);
            return;
        }
        self.leave_mix();
        let tracks = self
            .home_collections
            .iter()
            .find(|collection| collection.id == id)
            .map(|collection| collection.tracks.clone())
            .unwrap_or_default();
        if !tracks.is_empty() {
            self.start_queue_from(ctx, tracks, origin);
        } else {
            self.home_error = None;
            self.load_collection_tracks(ctx, id.clone());
            if self.home_request.is_some() {
                self.home_autoplay = Some(id);
                self.advance_on_load = false;
            }
        }
    }

    /// Fetch a Home collection's tracks in the background, wherever the user is.
    fn load_collection_tracks(&mut self, ctx: &egui::Context, id: String) {
        let Some(source) = self
            .home_sources
            .iter()
            .find(|source| source.id == id)
            .cloned()
        else {
            return;
        };
        self.home_request = None;
        let Some(session) = &self.session else {
            return;
        };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        let request_id = id.clone();
        match thread::Builder::new()
            .name("vk-home-playlist".into())
            .spawn(move || {
                let result = client
                    .playlist_tracks(owner, &source, &worker_cancel)
                    .map(|tracks| HomeReply::Tracks(id, tracks))
                    .map_err(|error| client.confirm_expired(owner, error));
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.home_request = Some(HomeRequest {
                    owner,
                    collection: Some(request_id),
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.home_error = Some(WebAudioError::Network),
        }
    }

    fn load_home_catalog(
        &mut self,
        ctx: &egui::Context,
        client: crate::web_audio::WebAudioClient,
        owner: i64,
    ) {
        if self.catalog_request.is_some() {
            return;
        }
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        if let Ok(worker) = thread::Builder::new()
            .name("vk-home-catalog".into())
            .spawn(move || {
                let _ = sender.send(client.home_catalog(owner, &worker_cancel));
                ctx.request_repaint();
            })
        {
            self.workers.push(worker);
            self.catalog_request = Some(CatalogRequest {
                owner,
                canceled,
                receiver,
            });
        }
    }

    /// Optional blocks: a failed catalog keeps the legacy Home as it is.
    fn poll_home_catalog(&mut self) {
        let Some(request) = &self.catalog_request else {
            return;
        };
        let owner = request.owner;
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        self.catalog_request = None;
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.profile.id != owner)
        {
            return;
        }
        if let Ok(catalog) = result {
            self.home_catalog = catalog;
            self.rebuild_home_collections();
            self.save_home_cache();
        }
    }

    /// The legacy Home repeats VK's genre and mood tiles as large playlist
    /// cards; Home shows those tiles as compact chips instead. Tracks already
    /// loaded into a card are kept.
    fn rebuild_home_collections(&mut self) {
        let chips: HashSet<&str> = self
            .home_catalog
            .genres
            .iter()
            .chain(&self.home_catalog.moods)
            .map(|tile| tile.title.as_str())
            .collect();
        let previous = std::mem::take(&mut self.home_collections);
        self.home_collections = self
            .home_sources
            .iter()
            .filter(|source| !chips.contains(source.title.as_str()))
            .map(|source| home_ui::Collection {
                id: source.id.clone(),
                title: source.title.clone(),
                subtitle: source.subtitle.clone(),
                cover_url: source.cover_url.clone(),
                tracks: previous
                    .iter()
                    .find(|collection| collection.id == source.id && !collection.tracks.is_empty())
                    .map_or_else(
                        || source.tracks.clone(),
                        |collection| collection.tracks.clone(),
                    ),
            })
            .collect();
    }

    fn poll_home(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.home_request else {
            return;
        };
        let owner = request.owner;
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        self.home_request = None;
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.profile.id != owner)
        {
            return;
        }
        if result.is_ok() {
            self.track_view_cache.invalidate();
        }
        match result {
            Ok(HomeReply::Catalog(sources)) => {
                self.home_sources = sources;
                self.rebuild_home_collections();
                self.home_error = None;
                self.save_home_cache();
            }
            Ok(HomeReply::Tracks(id, tracks)) => {
                let autoplay = self
                    .home_autoplay
                    .take()
                    .is_some_and(|pending| pending == id);
                if let Some(collection) = self
                    .home_collections
                    .iter_mut()
                    .find(|collection| collection.id == id)
                {
                    collection.tracks = tracks.clone();
                }
                self.home_error = None;
                if autoplay {
                    self.start_queue_from(ctx, tracks, format!("collection:{id}"));
                }
            }
            Err(
                error @ (WebAudioError::Expired
                | WebAudioError::NoSession
                | WebAudioError::WrongAccount),
            ) => {
                crate::diagnostics::event(
                    "session",
                    &format!("account_disconnected stage=home reason={error:?}"),
                );
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => {
                self.home_autoplay = None;
                self.home_error = Some(error);
            }
        }
        ctx.request_repaint();
    }

    fn owned_library_key(&self, track: &Track) -> Option<(i64, i64)> {
        let owner = self.session.as_ref()?.profile.id;
        if self.removed_from_library.contains(&track.key())
            || track
                .library_source_key
                .is_some_and(|key| self.removed_from_library.contains(&key))
        {
            return None;
        }
        self.like_aliases
            .get(&track.key())
            .copied()
            .or_else(|| {
                self.track_view_cache
                    .library_key(&self.library.tracks, track, owner)
            })
            .filter(|key| key.0 == owner)
    }

    fn track_is_liked(&self, track: &Track) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        if self.removed_from_library.contains(&track.key())
            || track
                .library_source_key
                .is_some_and(|key| self.removed_from_library.contains(&key))
        {
            return false;
        }
        track
            .library_liked
            .unwrap_or(track.owner_id == session.profile.id)
            || self.owned_library_key(track).is_some()
    }

    fn like_button(&self, ui: &mut egui::Ui, track: &Track) -> egui::Response {
        let liked = self.track_is_liked(track);
        let pending = self.like_request.as_ref().is_some_and(|request| {
            request.source.key() == track.key()
                || request.source.library_source_key == Some(track.key())
                || track.library_source_key == Some(request.source.key())
        });
        let response = icons::heart_button(
            ui,
            self.language.text(
                if liked {
                    "Убрать из моей музыки"
                } else {
                    "Добавить в мою музыку"
                },
                if liked {
                    "Remove from My music"
                } else {
                    "Add to My music"
                },
            ),
            !self.demo
                && self.search_ready()
                && self.like_request.is_none()
                && self.playlist_add_request.is_none(),
            liked,
            pending,
        );
        release_pointer_focus(&response);
        response
    }

    fn dislike_button(&self, ui: &mut egui::Ui, track: &Track) -> egui::Response {
        let disliked = self.disliked.contains(&track.key());
        let response = icons::track_button(
            ui,
            Icon::Dislike,
            if disliked {
                self.language
                    .text("Убрать из «Не нравится»", "Remove dislike")
            } else {
                self.language.text("Не нравится", "Dislike")
            },
            !self.demo
                && self.search_ready()
                && self.dislike_request.is_none()
                && track.unavailable != Some(true),
            disliked,
            self.dislike_request
                .as_ref()
                .is_some_and(|request| request.key == track.key()),
        );
        release_pointer_focus(&response);
        response
    }

    fn track_mix_button(&self, ui: &mut egui::Ui) -> egui::Response {
        let response = icons::track_button(
            ui,
            Icon::Radio,
            self.language.text("Микс по треку", "Go to track mix"),
            !self.demo && self.search_ready(),
            false,
            false,
        );
        release_pointer_focus(&response);
        response
    }

    /// Mark or unmark VK's "Не нравится" for a track. Like VK's player, a
    /// disliked track that is playing is skipped.
    fn toggle_dislike(&mut self, ctx: &egui::Context, track: &Track) {
        if self.demo || self.dislike_request.is_some() || !self.search_ready() {
            return;
        }
        let Some((owner, client)) = self
            .session
            .as_ref()
            .and_then(|session| Some((session.profile.id, session.music.clone()?)))
        else {
            return;
        };
        let key = track.key();
        let dislike = !self.disliked.contains(&key);
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-dislike".into())
            .spawn(move || {
                let canceled = AtomicBool::new(false);
                let _ = sender.send(client.set_dislike(owner, key, dislike, &canceled));
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.dislike_request = Some(DislikeRequest {
                    owner,
                    key,
                    dislike,
                    receiver,
                });
            }
            Err(_) => self.track_menu_error = Some(WebAudioError::Network),
        }
    }

    fn poll_dislike(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.dislike_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let DislikeRequest {
            owner,
            key,
            dislike,
            ..
        } = self.dislike_request.take().unwrap();
        if self.session.as_ref().map(|session| session.profile.id) != Some(owner) {
            return;
        }
        match result {
            Ok(()) => {
                let text = if dislike {
                    self.disliked.insert(key);
                    self.language
                        .text("Отмечено «Не нравится»", "Marked as disliked")
                } else {
                    self.disliked.remove(&key);
                    self.language
                        .text("Убрано из «Не нравится»", "Dislike removed")
                };
                self.track_menu_notice =
                    Some((text.into(), Instant::now() + Duration::from_secs(3)));
                if dislike
                    && self
                        .playing
                        .as_ref()
                        .is_some_and(|track| track.key() == key)
                {
                    self.advance_track(ctx, false);
                }
            }
            Err(error) => self.track_menu_error = Some(error),
        }
    }

    fn toggle_like(&mut self, ctx: &egui::Context, source: Track) {
        if self.demo
            || self.like_request.is_some()
            || self.playlist_add_request.is_some()
            || !self.search_ready()
        {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let liked = self.track_is_liked(&source);
        let owned = self
            .owned_library_key(&source)
            .or_else(|| (source.owner_id == owner).then_some(source.key()));
        let source_key = source.key();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_ctx = self.repaint_target.with_context(ctx);
        self.like_error = None;
        match thread::Builder::new()
            .name("vk-library-action".into())
            .spawn(move || {
                let result = if liked {
                    if let Some(owned) = owned {
                        client.remove_from_library(owner, owned, &worker_cancel)
                    } else {
                        client.remove_source_from_library(owner, source_key, &worker_cancel)
                    }
                    .map(LibraryChange::Removed)
                } else if source_key.0 == owner {
                    client
                        .restore_to_library(owner, source_key, &worker_cancel)
                        .map(LibraryChange::Added)
                } else {
                    // Foreign search results can omit membership. Confirm it
                    // before adding, so a stale outline cannot create a copy.
                    client
                        .track_metadata(owner, source_key, &worker_cancel)
                        .and_then(|fresh| {
                            if fresh.library_liked == Some(true) {
                                Ok(LibraryChange::AlreadyAdded)
                            } else {
                                client
                                    .add_to_library(owner, source_key, &worker_cancel)
                                    .map(LibraryChange::Added)
                            }
                        })
                };
                let _ = sender.send(result);
                worker_ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.like_request = Some(LikeRequest {
                    owner,
                    source,
                    restore_index: None,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.like_error = Some(WebAudioError::Network),
        }
    }

    fn update_known_tracks(&mut self, mut update: impl FnMut(&mut Track)) {
        self.track_view_cache.invalidate();
        for track in self
            .library
            .tracks
            .iter_mut()
            .chain(self.recent.iter_mut())
            .chain(self.play_queue.iter_mut().flatten())
            .chain(self.queued_tracks.values_mut())
            .chain(self.playing.iter_mut())
            .chain(
                self.home_collections
                    .iter_mut()
                    .flat_map(|collection| &mut collection.tracks),
            )
        {
            update(track);
        }
        for page in self.details.values_mut() {
            let tracks = match Arc::make_mut(page) {
                DetailPage::Artist(page) => &mut page.tracks,
                DetailPage::Album(page) | DetailPage::Playlist(page) => &mut page.tracks,
                DetailPage::Owner(_) => continue,
            };
            for track in tracks {
                update(track);
            }
        }
        for result in self
            .search
            .results
            .iter_mut()
            .chain(self.search.cache.values_mut())
        {
            for track in &mut Arc::make_mut(result).tracks {
                update(track);
            }
        }
    }

    fn reconcile_library_removals(&mut self) {
        self.track_view_cache.invalidate();
        // Offset counts VK rows, including removed rows returned by an older
        // request. Only the fresh prefix is adjusted when hiding those rows.
        let mut index = 0;
        let mut removed_prefix = 0;
        self.library.tracks.retain(|track| {
            let remove = self.removed_from_library.contains(&track.key())
                || track
                    .library_source_key
                    .is_some_and(|key| self.removed_from_library.contains(&key));
            if remove && index < self.library.fresh_prefix {
                removed_prefix += 1;
            }
            index += 1;
            !remove
        });
        self.library.fresh_prefix = self.library.fresh_prefix.saturating_sub(removed_prefix);
    }

    fn apply_library_change(&mut self, ctx: &egui::Context, source: &Track, change: LibraryChange) {
        let mut identities = HashSet::from([source.key()]);
        identities.extend(source.library_source_key);
        match change {
            LibraryChange::AlreadyAdded => {
                self.update_known_tracks(|track| {
                    if identities.contains(&track.key())
                        || track
                            .library_source_key
                            .is_some_and(|key| identities.contains(&key))
                    {
                        track.library_liked = Some(true);
                    }
                });
            }
            LibraryChange::Added(owned) => {
                identities.insert(owned);
                for key in &identities {
                    self.like_aliases.insert(*key, owned);
                    self.removed_from_library.remove(key);
                }
                let confirmed_source = source
                    .library_source_key
                    .or_else(|| (source.key() != owned).then_some(source.key()));
                if let Some(saved) = self
                    .library
                    .tracks
                    .iter_mut()
                    .find(|track| track.key() == owned)
                {
                    saved.library_source_key = confirmed_source;
                } else {
                    let mut saved = source.clone();
                    saved.owner_id = owned.0;
                    saved.id = owned.1;
                    saved.url = None;
                    // The source's date is its release or when it joined
                    // another list; this copy was added to My music now.
                    saved.added = Some(chrono::Utc::now().timestamp());
                    saved.library_liked = Some(true);
                    saved.library_source_key = confirmed_source;
                    saved.release_key = source
                        .release_key
                        .or_else(|| (source.key() != owned).then_some(source.key()));
                    self.library.tracks.insert(0, saved);
                    self.library.fresh_prefix += 1;
                }
                self.update_known_tracks(|track| {
                    if identities.contains(&track.key())
                        || track
                            .library_source_key
                            .is_some_and(|key| identities.contains(&key))
                    {
                        track.library_liked = Some(true);
                    }
                });
            }
            LibraryChange::Removed(owned) => {
                identities.extend(owned);
                if let Some(key) = self.owned_library_key(source) {
                    identities.insert(key);
                }
                for track in &self.library.tracks {
                    if identities.contains(&track.key()) {
                        identities.extend(track.library_source_key);
                    }
                }
                for (alias, saved) in &self.like_aliases {
                    if identities.contains(saved) {
                        identities.insert(*alias);
                    }
                }
                // Keep the active source independent of membership edits. In
                // particular, removing the playing track must not lose its
                // position or prevent Next from reaching the following track.
                if self.play_queue.is_none() && self.playing.is_some() {
                    self.play_queue = Some(self.library.tracks.clone());
                    self.queue_origin = Some("library".into());
                }
                self.removed_from_library.extend(identities.iter().copied());
                self.like_aliases.retain(|alias, owned| {
                    !identities.contains(alias) && !identities.contains(owned)
                });
                self.update_known_tracks(|track| {
                    if identities.contains(&track.key())
                        || track
                            .library_source_key
                            .is_some_and(|key| identities.contains(&key))
                    {
                        track.library_liked = Some(false);
                    }
                });
                self.reconcile_library_removals();
            }
        }
        self.save_library(ctx);
        self.save_recent(ctx);
        self.synchronize_order();
        // Membership edits can invalidate VK's opaque catalog continuation.
        // Rebase it after an older page finishes, keeping the visible cache.
        self.library_refresh_due = true;
    }

    fn poll_like(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.like_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self.like_request.take().expect("Active library action");
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.profile.id != request.owner)
        {
            return;
        }
        match result {
            Ok(change) => {
                let undo = match &change {
                    LibraryChange::Removed(owned) => {
                        self.removal_undo(request.owner, &request.source, owned)
                    }
                    LibraryChange::Added(owned) => {
                        if let Some(index) = request.restore_index {
                            self.reinsert_library_row(&request.source, *owned, index);
                        }
                        None
                    }
                    LibraryChange::AlreadyAdded => None,
                };
                self.apply_library_change(ctx, &request.source, change);
                if let Some(undo) = undo {
                    self.track_menu_notice = Some((
                        self.language
                            .text("Трек удалён из моей музыки", "Removed from My music")
                            .into(),
                        undo.until,
                    ));
                    self.library_undo = Some(undo);
                }
            }
            Err(WebAudioError::Canceled) => {}
            Err(error @ WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(error) => self.like_error = Some(error),
        }
        ctx.request_repaint();
    }

    /// The owned row a removal deleted, so its notice can restore it. VK
    /// restores only the account's own copy.
    fn removal_undo(
        &self,
        owner: i64,
        source: &Track,
        owned: &[(i64, i64)],
    ) -> Option<LibraryUndo> {
        let mut identities: HashSet<_> = owned.iter().copied().collect();
        identities.insert(source.key());
        identities.extend(source.library_source_key);
        identities.extend(self.owned_library_key(source));
        let (index, row) = match self
            .library
            .tracks
            .iter()
            .position(|track| track.owner_id == owner && identities.contains(&track.key()))
        {
            Some(index) => (Some(index), self.library.tracks[index].clone()),
            None => {
                let key = owned
                    .iter()
                    .copied()
                    .chain(self.owned_library_key(source))
                    .chain(std::iter::once(source.key()))
                    .find(|key| key.0 == owner)?;
                let mut row = source.clone();
                if row.key() != key {
                    row.library_source_key = row.library_source_key.or(Some(source.key()));
                }
                (row.owner_id, row.id) = key;
                row.url = None;
                (None, row)
            }
        };
        Some(LibraryUndo {
            owner,
            row,
            index,
            until: Instant::now() + Duration::from_secs(5),
        })
    }

    /// An undone removal returns to its old place instead of the top.
    fn reinsert_library_row(&mut self, row: &Track, owned: (i64, i64), index: usize) {
        if self.library.tracks.iter().any(|track| track.key() == owned) {
            return;
        }
        let mut row = row.clone();
        (row.owner_id, row.id) = owned;
        row.library_liked = Some(true);
        let index = index.min(self.library.tracks.len());
        if index <= self.library.fresh_prefix {
            self.library.fresh_prefix += 1;
        }
        self.library.tracks.insert(index, row);
        self.track_view_cache.invalidate();
    }

    /// "Undo" in the removal notice: VK restores the deleted copy.
    fn undo_library_removal(&mut self, ctx: &egui::Context) {
        let Some(undo) = self.library_undo.take() else {
            return;
        };
        if self.demo
            || self.like_request.is_some()
            || self.playlist_add_request.is_some()
            || !self.search_ready()
        {
            return;
        }
        let Some(session) = &self.session else { return };
        if session.profile.id != undo.owner {
            return;
        }
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = undo.owner;
        let key = undo.row.key();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_ctx = self.repaint_target.with_context(ctx);
        self.like_error = None;
        match thread::Builder::new()
            .name("vk-library-restore".into())
            .spawn(move || {
                let result = client
                    .restore_to_library(owner, key, &worker_cancel)
                    .map(LibraryChange::Added);
                let _ = sender.send(result);
                worker_ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.like_request = Some(LikeRequest {
                    owner,
                    source: undo.row,
                    restore_index: Some(undo.index.unwrap_or(0)),
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.like_error = Some(WebAudioError::Network),
        }
    }

    fn hydrate_metadata(&mut self, ctx: &egui::Context, keys: Vec<(i64, i64)>) {
        if self.demo || self.metadata_request.is_some() {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let mut unique = HashSet::new();
        let keys: Vec<_> = keys
            .into_iter()
            .filter(|key| !self.metadata_attempted.contains(key) && unique.insert(*key))
            .take(100)
            .collect();
        if keys.is_empty() {
            return;
        }
        let owner = session.profile.id;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.metadata_attempted.extend(keys.iter().copied());
        if let Ok(worker) = thread::Builder::new()
            .name("vk-track-metadata".into())
            .spawn(move || {
                let result = client.tracks_metadata(owner, &keys, &worker_cancel);
                let _ = sender.send(result);
                ctx.request_repaint();
            })
        {
            self.workers.push(worker);
            self.metadata_request = Some(MetadataRequest {
                owner,
                canceled,
                receiver,
            });
        }
    }

    /// My music comes from VK's website without dates. While it is open,
    /// fill them 100 tracks at a time from the API, which reports when the
    /// account's own copy was added; this enables "Date added".
    fn hydrate_library_dates(&mut self, ctx: &egui::Context) {
        if self.demo || self.metadata_request.is_some() || self.settings_open {
            return;
        }
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        // My music, or an own playlist: its own copies' dates come the same way.
        let tracks: &[Track] = if self.page == MusicPage::Library {
            &self.library.tracks
        } else if self.own_playlist().is_some() {
            self.view_tracks()
        } else {
            return;
        };
        let keys: Vec<_> = tracks
            .iter()
            .filter(|track| {
                track.added.is_none()
                    && track.owner_id == owner
                    && !self.metadata_attempted.contains(&track.key())
            })
            .map(Track::key)
            .take(100)
            .collect();
        if !keys.is_empty() {
            self.hydrate_metadata(ctx, keys);
        }
    }

    fn poll_metadata(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.metadata_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self
            .metadata_request
            .take()
            .expect("Active metadata request");
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.profile.id != request.owner)
        {
            return;
        }
        let fresh = match result {
            Ok(tracks) => tracks,
            Err(error @ WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(error));
                return;
            }
            Err(_) => return,
        };
        let fresh: HashMap<_, _> = fresh
            .into_iter()
            .map(|track| (track.key(), track))
            .collect();
        self.track_view_cache.invalidate();
        let removed = &self.removed_from_library;
        let added = &self.like_aliases;
        // `library`: a My music row, whose own copy's VK date outranks any
        // date it brought from elsewhere. A playlist row keeps its own date.
        let enrich = |track: &mut Track, library: bool| {
            let Some(metadata) = fresh.get(&track.key()) else {
                return false;
            };
            let mut changed = false;
            if !metadata.artists.is_empty() && metadata.artists != track.artists {
                track.artists = metadata.artists.clone();
                changed = true;
            }
            if metadata.subtitle.is_some() && metadata.subtitle != track.subtitle {
                track.subtitle = metadata.subtitle.clone();
                changed = true;
            }
            if metadata.unavailable.is_some() && metadata.unavailable != track.unavailable {
                track.unavailable = metadata.unavailable;
                changed = true;
            }
            if metadata.album.is_some() && metadata.album != track.album {
                track.album = metadata.album.clone();
                changed = true;
            }
            if track.release_key.is_none() && metadata.release_key.is_some() {
                track.release_key = metadata.release_key;
                changed = true;
            }
            if let Some(native_liked) = metadata.library_liked {
                let liked = if removed.contains(&track.key())
                    || track
                        .library_source_key
                        .is_some_and(|key| removed.contains(&key))
                {
                    false
                } else if added.contains_key(&track.key())
                    || track
                        .library_source_key
                        .is_some_and(|key| added.contains_key(&key))
                {
                    true
                } else {
                    native_liked
                };
                if track.library_liked != Some(liked) {
                    track.library_liked = Some(liked);
                    changed = true;
                }
            }
            if track.cover_url.is_none() && metadata.cover_url.is_some() {
                track.cover_url = metadata.cover_url.clone();
                changed = true;
            }
            // An own copy's VK date is when it was added to My music.
            if metadata.added.is_some()
                && metadata.added != track.added
                && (library || track.added.is_none())
                && track.owner_id == request.owner
            {
                track.added = metadata.added;
                changed = true;
            }
            changed
        };
        let mut library_changed = false;
        for track in &mut self.library.tracks {
            library_changed |= enrich(track, true);
        }
        for track in self
            .recent
            .iter_mut()
            .chain(self.play_queue.iter_mut().flatten())
            .chain(self.queued_tracks.values_mut())
            .chain(self.playing.iter_mut())
        {
            enrich(track, false);
        }
        for track in self
            .home_collections
            .iter_mut()
            .flat_map(|collection| &mut collection.tracks)
        {
            enrich(track, false);
        }
        for page in self.details.values_mut() {
            let tracks = match Arc::make_mut(page) {
                DetailPage::Artist(page) => &mut page.tracks,
                DetailPage::Album(page) | DetailPage::Playlist(page) => &mut page.tracks,
                DetailPage::Owner(_) => continue,
            };
            for track in tracks {
                enrich(track, false);
            }
        }
        for result in self
            .search
            .results
            .iter_mut()
            .chain(self.search.cache.values_mut())
        {
            for track in &mut Arc::make_mut(result).tracks {
                enrich(track, false);
            }
        }
        if library_changed {
            self.save_library(ctx);
        }
        self.apply_playlist_dates();
        ctx.request_repaint();
    }

    fn ensure_detail(&mut self, ctx: &egui::Context, reload: bool) {
        let Some(key) = self.page.detail_key() else {
            self.detail_request = None;
            self.detail_error = None;
            return;
        };
        if !reload
            && (self.details.contains_key(&key)
                || self
                    .detail_request
                    .as_ref()
                    .is_some_and(|request| request.key == key))
        {
            return;
        }
        self.detail_request = None;
        self.detail_error = None;
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let page = self.page.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        // Two slots: an artist page arrives first without its full discography.
        let (sender, receiver) = mpsc::sync_channel(2);
        let ctx = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("vk-artist-album".into())
            .spawn(move || {
                let result = match page {
                    MusicPage::Artist(artist) => client
                        .artist_page_progressive(owner, &artist, &worker_cancel, |first| {
                            let _ = sender.try_send(Ok(DetailPage::Artist(first)));
                            ctx.request_repaint();
                        })
                        .map(DetailPage::Artist),
                    MusicPage::Album(album) => client
                        .album_page(owner, &album, &worker_cancel)
                        .map(DetailPage::Album),
                    MusicPage::Playlist(playlist) => client
                        .collection_page(owner, &playlist, &worker_cancel)
                        .map(|(page, kind)| match kind {
                            crate::web_audio::CollectionKind::Album => DetailPage::Album(page),
                            crate::web_audio::CollectionKind::Playlist => {
                                DetailPage::Playlist(page)
                            }
                        }),
                    MusicPage::Owner(creator) => client
                        .owner_page(owner, &creator, 0, &worker_cancel)
                        .map(DetailPage::Owner),
                    MusicPage::TrackAlbum(key, _) => client
                        .track_metadata(owner, key, &worker_cancel)
                        .and_then(|track| track.album.ok_or(WebAudioError::NoAlbum))
                        .and_then(|album| client.album_page(owner, &album, &worker_cancel))
                        .map(DetailPage::Album),
                    _ => Err(WebAudioError::Canceled),
                };
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.detail_request = Some(DetailRequest {
                    owner,
                    key,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.detail_error = Some(WebAudioError::Network),
        }
    }

    fn poll_detail(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.detail_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        // A partial artist page keeps the request open for its discography.
        let partial = matches!(&result, Ok(DetailPage::Artist(page)) if page.releases_pending);
        let (owner, mut key, canceled) = (
            request.owner,
            request.key.clone(),
            request.canceled.load(Ordering::Acquire),
        );
        if !partial {
            self.detail_request = None;
        }
        if canceled
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != owner)
        {
            return;
        }
        if self.page.detail_key().as_deref() != Some(key.as_str()) {
            return;
        }
        // A `/music/playlist/` link can lead to an album or single: show it
        // as VK describes it.
        if let (MusicPage::Playlist(reference), Ok(DetailPage::Album(_))) = (&self.page, &result) {
            self.page = MusicPage::Album(reference.clone());
            key = self.page.detail_key().unwrap_or(key);
        }
        match result {
            Ok(mut page) => {
                if let DetailPage::Owner(batch) = &mut page {
                    if batch.offset > 0 {
                        if let Some(cached) = self.details.get_mut(&key)
                            && let DetailPage::Owner(cached) = Arc::make_mut(cached)
                        {
                            cached.append(batch.clone());
                        }
                        ctx.request_repaint();
                        return;
                    }
                    if batch.playlists_error.is_some()
                        && let Some(cached) = self.details.get(&key)
                        && let DetailPage::Owner(cached) = cached.as_ref()
                    {
                        batch.playlists = cached.playlists.clone();
                        batch.total = cached.total;
                        batch.next_offset = cached.next_offset;
                    }
                }
                if let DetailPage::Artist(fresh) = &mut page
                    && fresh.releases_error.is_some()
                    && let Some(old) = self.details.get(&key)
                    && let DetailPage::Artist(old) = old.as_ref()
                {
                    let present: HashSet<_> = fresh
                        .discography
                        .iter()
                        .map(|release| (release.album.owner_id, release.album.id))
                        .collect();
                    fresh.discography.extend(
                        old.discography
                            .iter()
                            .filter(|release| {
                                !present.contains(&(release.album.owner_id, release.album.id))
                            })
                            .cloned(),
                    );
                }
                if self.details.len() >= 32 && !self.details.contains_key(&key) {
                    let retained: HashSet<_> = self
                        .back_history
                        .iter()
                        .rev()
                        .take(8)
                        .filter_map(|location| location.page.detail_key())
                        .collect();
                    if let Some(key) = self
                        .details
                        .keys()
                        .find(|key| !retained.contains(*key))
                        .cloned()
                    {
                        self.details.remove(&key);
                    }
                }
                self.details.insert(key.clone(), Arc::new(page));
                self.detail_error = None;
                self.apply_playlist_dates();
            }
            Err(error @ WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => {
                let query = if self.settings_open {
                    None
                } else {
                    match (&self.page, &error) {
                        (
                            MusicPage::Artist(artist),
                            WebAudioError::Format
                            | WebAudioError::Rejected
                            | WebAudioError::NoStream,
                        ) => Some(artist.name.clone()),
                        (
                            MusicPage::Album(album),
                            WebAudioError::Format
                            | WebAudioError::Rejected
                            | WebAudioError::NoAlbum
                            | WebAudioError::NoStream,
                        ) => Some(if album.title.trim().is_empty() {
                            self.playing
                                .iter()
                                .chain(self.library.tracks.iter())
                                .chain(self.recent.iter())
                                .find(|track| {
                                    track.album.as_ref().is_some_and(|release| {
                                        release.owner_id == album.owner_id && release.id == album.id
                                    })
                                })
                                .map(|track| track.display_title().into_owned())
                                .unwrap_or_default()
                        } else {
                            album.title.clone()
                        }),
                        (
                            MusicPage::TrackAlbum(_, title),
                            WebAudioError::Format
                            | WebAudioError::Rejected
                            | WebAudioError::NoAlbum
                            | WebAudioError::NoStream,
                        ) => Some(title.clone()),
                        _ => None,
                    }
                }
                .filter(|query| {
                    !query.trim().is_empty()
                        || matches!(self.page, MusicPage::Album(_) | MusicPage::TrackAlbum(_, _))
                });
                if let Some(query) = query {
                    self.detail_error = None;
                    // Replace the missing detail page so Back returns to its
                    // source rather than reopening the failed artist/release.
                    self.submit_search(ctx, query);
                } else {
                    self.detail_error = Some(error);
                }
            }
        }
        ctx.request_repaint();
    }

    fn detail_content(&mut self, ui: &mut egui::Ui) {
        let cover_playback = self.cover_playback();
        let ctx = ui.ctx().clone();
        let page = self.page.clone();
        let language = self.language;
        ui.horizontal(|ui| {
            if icons::compact_button(
                ui,
                Icon::ArrowLeft,
                language.text("Назад", "Back"),
                !self.back_history.is_empty(),
                false,
            )
            .clicked()
            {
                self.go_back(&ctx);
            }
            ui.label(
                RichText::new(language.text("Вернуться назад", "Go back"))
                    .small()
                    .color(MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::compact_button(
                    ui,
                    Icon::Refresh,
                    language.text("Обновить", "Refresh"),
                    self.detail_request.is_none(),
                    false,
                )
                .clicked()
                {
                    self.ensure_detail(&ctx, true);
                }
            });
        });
        if self.page != page {
            return;
        }
        ui.add_space(12.0);
        let loading = self.detail_request.is_some();
        if let Some(error) = &self.detail_error {
            theme::inset_heading(ui, |ui| {
                ui.colored_label(Color32::from_rgb(255, 158, 153), error.localized(language))
            });
        }
        if let MusicPage::Owner(creator) = &page {
            self.owner_content(ui, creator);
            return;
        }
        let stored = page
            .detail_key()
            .and_then(|key| self.details.get(&key))
            .cloned();
        let data = stored.as_deref();
        // A partial artist page is shown; only its discography is loading.
        let releases_pending =
            matches!(data, Some(DetailPage::Artist(artist)) if artist.releases_pending);
        let loading = loading && !releases_pending;
        let can_edit_playlist =
            matches!(&page, MusicPage::Playlist(reference) if self.can_edit_playlist(reference));
        let tracks = data.map_or(&[][..], DetailPage::tracks);
        let active = matches!(
            self.player.snapshot().state,
            PlaybackState::Playing | PlaybackState::Loading
        );
        let playing = active
            && self
                .playing
                .as_ref()
                .is_some_and(|playing| tracks.iter().any(|track| track.key() == playing.key()));
        let (artist, artist_image, albums, album, album_artists, year) = match (&page, data) {
            (_, Some(DetailPage::Artist(data))) => (
                Some(data.artist.clone()),
                data.image_url.clone(),
                data.latest_releases.clone(),
                None,
                Vec::new(),
                None,
            ),
            (_, Some(DetailPage::Album(data) | DetailPage::Playlist(data))) => (
                None,
                None,
                Vec::new(),
                Some(data.album.clone()),
                data.artists.clone(),
                data.year,
            ),
            (MusicPage::Artist(artist), _) => (
                Some(artist.clone()),
                None,
                Vec::new(),
                None,
                Vec::new(),
                None,
            ),
            (MusicPage::Album(album) | MusicPage::Playlist(album), _) => (
                None,
                None,
                Vec::new(),
                Some(album.clone()),
                Vec::new(),
                None,
            ),
            (MusicPage::TrackAlbum(_, title), _) => (
                None,
                None,
                Vec::new(),
                Some(AlbumRef {
                    id: 0,
                    owner_id: 0,
                    title: title.clone(),
                    access_key: None,
                    cover_url: None,
                }),
                Vec::new(),
                None,
            ),
            _ => return,
        };
        let follow = album
            .as_ref()
            .and_then(|album| self.playlist_follow_view(album));
        let artist_follow = artist
            .as_ref()
            .and_then(|artist| self.artist_follow_view(artist));
        let mut action = None;
        let mut track_action = None;
        theme::scroll_area(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("music-detail", page.detail_key()))
                .auto_shrink([false, false]),
            |ui| {
                if let Some(artist) = &artist {
                    action = detail_ui::artist_header(
                        ui,
                        &mut self.covers,
                        detail_ui::ArtistView {
                            language,
                            name: &artist.name,
                            image_url: artist_image.as_deref(),
                            track_count: tracks.len(),
                            loading,
                            playing,
                            follow: artist_follow,
                        },
                    );
                } else if let Some(album) = &album {
                    let subtitle = year.map_or_else(String::new, |year| year.to_string());
                    let header = if matches!(page, MusicPage::Playlist(_)) {
                        detail_ui::playlist_header
                    } else {
                        detail_ui::album_header
                    };
                    action = header(
                        ui,
                        &mut self.covers,
                        detail_ui::AlbumView {
                            language,
                            title: if album.title.is_empty() {
                                if matches!(page, MusicPage::Playlist(_)) {
                                    language.text("Плейлист", "Playlist")
                                } else {
                                    language.text("Альбом", "Album")
                                }
                            } else {
                                &album.title
                            },
                            artists: &album_artists,
                            creator: match data {
                                Some(DetailPage::Playlist(playlist)) => playlist.creator.as_ref(),
                                _ => None,
                            },
                            image_url: album.cover_url.as_deref(),
                            subtitle: &subtitle,
                            track_count: tracks.len(),
                            loading,
                            playing,
                            can_edit_playlist,
                            follow,
                        },
                    );
                }
                if let Some(DetailPage::Playlist(playlist)) = data
                    && let Some(description) = playlist.description.as_deref()
                {
                    detail_ui::playlist_description(ui, description);
                }
                ui.add_space(20.0);
                theme::inset_heading(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(if artist.is_some() {
                                language.text("Популярное", "Popular")
                            } else {
                                language.text("Треки", "Tracks")
                            })
                            .size(20.0)
                            .family(theme::semibold()),
                        )
                        .selectable(false),
                    )
                });
                ui.add_space(8.0);
                if loading && tracks.is_empty() {
                    theme::inset_heading(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(language.text("Загружаем…", "Loading…"));
                            ui.spinner();
                        })
                    });
                } else if tracks.is_empty() && self.detail_error.is_none() {
                    let empty = matches!(data,Some(DetailPage::Album(album)|DetailPage::Playlist(album)) if album.confirmed_empty());
                    let text = match (&page, empty) {
                        (MusicPage::Playlist(_), true) => language.text(
                            "В этом плейлисте пока нет треков.",
                            "This playlist is empty.",
                        ),
                        (MusicPage::Album(_) | MusicPage::TrackAlbum(..), true) => {
                            language.text("В этом альбоме пока нет треков.", "This album is empty.")
                        }
                        (MusicPage::Artist(_), _) => language.text(
                            "VK не вернул популярные треки.",
                            "VK did not return popular tracks.",
                        ),
                        (MusicPage::Playlist(_), _) => language.text(
                            "VK не вернул треки плейлиста.",
                            "VK did not return the playlist tracks.",
                        ),
                        _ => language.text(
                            "VK не вернул треки альбома.",
                            "VK did not return the album tracks.",
                        ),
                    };
                    theme::inset_heading(ui, |ui| ui.label(text));
                }
                ui.spacing_mut().item_spacing.y = 4.0;
                let expanded_id = ui.make_persistent_id(("artist-all-tracks", page.detail_key()));
                let expanded = ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(expanded_id))
                    .unwrap_or(false);
                let count = if artist.is_some() && !expanded {
                    tracks.len().min(6)
                } else {
                    tracks.len()
                };
                let columns = if artist.is_some() && ui.available_width() >= 560.0 {
                    2
                } else {
                    1
                };
                // Albums and playlists sort from their column header; an
                // artist's top tracks keep VK's order.
                let show_date = track_sort::Extra {
                    // An album page would repeat its own title on every row.
                    album: artist.is_none() && matches!(self.page, MusicPage::Playlist(_)),
                    date: artist.is_none() && self.show_date_column(),
                };
                let sorted = if artist.is_none() && !tracks.is_empty() {
                    let sort = self.current_sort();
                    let index_width = track_index_width(ui, tracks.len());
                    if let Some(column) =
                        track_sort::header(ui, language, sort, show_date, index_width, false)
                    {
                        self.set_sort(column);
                        ui.ctx().request_repaint();
                    }
                    let sort = self.current_sort();
                    sort.active().then(|| {
                        track_sort::order(tracks, 0..tracks.len(), sort)
                            .into_iter()
                            .map(|index| tracks[index].clone())
                            .collect::<Vec<_>>()
                    })
                } else {
                    None
                };
                let reorder = artist.is_none() && sorted.is_none() && self.playlist_reorderable();
                track_action = self.clipped_track_rows(
                    ui,
                    &sorted.as_deref().unwrap_or(tracks)[..count],
                    columns,
                    active,
                    "detail-track-rows",
                    show_date,
                    reorder,
                );
                if let Some(DetailPage::Album(release) | DetailPage::Playlist(release)) = data {
                    release_info::show(ui, language, tracks, &release.stats);
                }
                // The same blue "Show all" link as under the discography.
                if artist.is_some()
                    && tracks.len() > 6
                    && detail_ui::expand_button(ui, language, expanded)
                {
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(expanded_id, !expanded));
                    ui.ctx()
                        .request_discard("popular tracks expanded or hidden");
                }
                if let Some(DetailPage::Artist(artist_page)) = data
                    && let Some(intent) = detail_ui::artist_releases(
                        ui,
                        &mut self.covers,
                        detail_ui::ReleasesView {
                            language,
                            artist_id: &artist_page.artist.id,
                            latest: &albums,
                            discography: &artist_page.discography,
                            incomplete: artist_page.releases_error.is_some(),
                            loading,
                            pending: releases_pending,
                            playback: cover_playback,
                            related: &artist_page.related,
                        },
                    )
                {
                    action = Some(intent);
                }
            },
        );
        if let Some(action) = track_action {
            self.handle_track_action(&ctx, action);
            return;
        }
        match action {
            Some(detail_ui::Action::ReloadReleases) => self.ensure_detail(&ctx, true),
            Some(detail_ui::Action::ToggleLibraryLike) => {
                if let Some(artist) = &artist {
                    self.toggle_artist_follow(&ctx, artist.clone());
                } else if let Some(album) = &album {
                    self.toggle_playlist_follow(&ctx, album.clone());
                }
            }
            Some(detail_ui::Action::CopyLink) => {
                let link = if let Some(artist) = &artist {
                    crate::model::valid_artist_id(&artist.id)
                        .then(|| format!("https://vk.ru/artist/{}", artist.id))
                } else {
                    album.as_ref().and_then(|album| {
                        playlist_actions::collection_link(
                            album,
                            !matches!(page, MusicPage::Playlist(_)),
                        )
                    })
                };
                if let Some(link) = link {
                    ctx.copy_text(link);
                    self.playlist_notice(self.language.text("Ссылка скопирована", "Link copied"));
                }
            }
            Some(detail_ui::Action::EditPlaylist) => {
                if let MusicPage::Playlist(reference) = page {
                    self.open_playlist_editor(&ctx, reference, false);
                }
            }
            Some(detail_ui::Action::OpenArtist(artist)) => {
                self.navigate_music(&ctx, MusicPage::Artist(artist))
            }
            Some(detail_ui::Action::OpenAlbum(album)) => {
                self.navigate_music(&ctx, MusicPage::Album(album))
            }
            Some(detail_ui::Action::PlayAlbum(album)) => {
                self.play_cover(&ctx, album.clone(), MusicPage::Album(album));
            }
            Some(detail_ui::Action::OpenCreator(creator)) if creator.owner_id != 0 => {
                self.navigate_music(&ctx, MusicPage::Owner(creator));
            }
            Some(detail_ui::Action::Play) if playing => self.toggle_playback(&ctx),
            Some(detail_ui::Action::Play | detail_ui::Action::Shuffle) if !tracks.is_empty() => {
                self.leave_mix();
                if matches!(action, Some(detail_ui::Action::Shuffle)) {
                    let keys: Vec<_> = tracks.iter().map(Track::key).collect();
                    self.play_order.set_shuffle(true, &keys, None);
                    self.save_playback_modes();
                }
                self.start_queue(&ctx, tracks.to_vec());
            }
            _ => {}
        }
    }

    fn owner_content(&mut self, ui: &mut egui::Ui, creator: &crate::web_audio::PlaylistCreator) {
        let stored = self
            .page
            .detail_key()
            .and_then(|key| self.details.get(&key))
            .cloned();
        let page = stored.as_deref().and_then(|page| match page {
            DetailPage::Owner(page) => Some(page),
            _ => None,
        });
        let playback = self.cover_playback();
        let language = self.language;
        let loading = self.detail_request.is_some();
        let tab_id = egui::Id::new(("owner-tab", creator.owner_id));
        let mut tab: crate::owner_ui::Tab =
            ui.data(|data| data.get_temp(tab_id)).unwrap_or_default();
        let mut action = None;
        let mut track_action = None;
        theme::scroll_area(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("playlist-owner", creator.owner_id))
                .auto_shrink([false, false]),
            |ui| {
                action = crate::owner_ui::show(
                    ui,
                    &mut self.covers,
                    language,
                    creator,
                    page,
                    loading,
                    playback,
                    &mut tab,
                );
                if tab == crate::owner_ui::Tab::Playlists {
                    return;
                }
                let Some(page) = page.filter(|page| page.closed.is_none()) else {
                    return;
                };
                ui.add_space(if tab == crate::owner_ui::Tab::All {
                    24.0
                } else {
                    16.0
                });
                theme::inset_heading(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(language.text("Треки", "Tracks"))
                                .size(20.0)
                                .family(theme::semibold()),
                        )
                        .selectable(false),
                    )
                });
                ui.add_space(10.0);
                if page.tracks.is_empty() {
                    let message = if page.tracks_error == Some(WebAudioError::Rejected) {
                        language
                            .text(
                                "VK не предоставил доступ к трекам.",
                                "VK did not grant access to these tracks.",
                            )
                            .into()
                    } else if let Some(error) = &page.tracks_error {
                        error.localized(language)
                    } else {
                        language
                            .text("Нет доступных треков.", "No available tracks.")
                            .into()
                    };
                    theme::inset_heading(ui, |ui| ui.label(RichText::new(message).color(MUTED)));
                    return;
                }
                let tracks = &page.tracks;
                let sort = self.current_sort();
                let index_width = track_index_width(ui, tracks.len());
                if let Some(column) = track_sort::header(
                    ui,
                    language,
                    sort,
                    track_sort::Extra {
                        album: true,
                        date: false,
                    },
                    index_width,
                    false,
                ) {
                    self.set_sort(column);
                    ui.ctx().request_repaint();
                }
                let sort = self.current_sort();
                let sorted = sort.active().then(|| {
                    track_sort::order(tracks, 0..tracks.len(), sort)
                        .into_iter()
                        .map(|index| tracks[index].clone())
                        .collect::<Vec<_>>()
                });
                let active = matches!(
                    self.player.snapshot().state,
                    PlaybackState::Playing | PlaybackState::Loading
                );
                ui.spacing_mut().item_spacing.y = 4.0;
                track_action = self.clipped_track_rows(
                    ui,
                    sorted.as_deref().unwrap_or(tracks),
                    1,
                    active,
                    "owner-track-rows",
                    track_sort::Extra {
                        album: true,
                        date: false,
                    },
                    false,
                );
                ui.add_space(22.0);
            },
        );
        ui.data_mut(|data| data.insert_temp(tab_id, tab));
        let ctx = ui.ctx().clone();
        if let Some(track_action) = track_action {
            self.handle_track_action(&ctx, track_action);
        }
        match action {
            Some(crate::owner_ui::Action::OpenPlaylist(playlist)) => {
                self.navigate_music(&ctx, MusicPage::Playlist(playlist))
            }
            Some(crate::owner_ui::Action::PlayPlaylist(playlist)) => {
                self.play_cover(&ctx, playlist.clone(), MusicPage::Playlist(playlist));
            }
            Some(crate::owner_ui::Action::More) if self.detail_error.is_none() => {
                self.load_more_owner(&ctx)
            }
            Some(crate::owner_ui::Action::Retry) => {
                if page.is_some_and(|page| {
                    page.retry_offset
                        .is_some_and(|offset| offset > 0 && page.next_offset == Some(offset))
                }) {
                    self.load_more_owner(&ctx);
                } else {
                    self.ensure_detail(&ctx, true);
                }
            }
            Some(crate::owner_ui::Action::More) | None => {}
        }
    }

    fn load_more_owner(&mut self, ctx: &egui::Context) {
        if self.detail_request.is_some() || self.settings_open {
            return;
        }
        let MusicPage::Owner(creator) = self.page.clone() else {
            return;
        };
        let key = self.page.detail_key().expect("Owner page identity");
        let Some(DetailPage::Owner(page)) = self.details.get(&key).map(Arc::as_ref) else {
            return;
        };
        let Some(offset) = page.next_offset else {
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.detail_error = None;
        match thread::Builder::new()
            .name("vk-owner-playlists".into())
            .spawn(move || {
                let result = client
                    .owner_page(owner, &creator, offset, &worker_cancel)
                    .map(DetailPage::Owner);
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.detail_request = Some(DetailRequest {
                    owner,
                    key,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.detail_error = Some(WebAudioError::Network),
        }
    }

    fn home_content(&mut self, ui: &mut egui::Ui) {
        // The same title row as My music: refresh sits at the top right.
        let refresh_enabled = (self.session.is_some() || self.demo)
            && self.home_request.is_none()
            && self.catalog_request.is_none()
            && !(self.pending.is_some() && !self.home_requested);
        let mut refresh = false;
        ui.horizontal(|ui| {
            theme::inset_heading(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(self.language.text("Главная", "Home"))
                            .text_style(egui::TextStyle::Heading)
                            .size(26.0),
                    )
                    .selectable(false),
                )
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                refresh = icons::compact_button(
                    ui,
                    Icon::Refresh,
                    self.language.text("Обновить", "Refresh"),
                    refresh_enabled,
                    false,
                )
                .clicked();
            });
        });
        if refresh {
            let ctx = ui.ctx().clone();
            self.load_home(&ctx);
            // Home shows the My music track count: refresh it too.
            if self.pending.is_none() {
                self.load_page(&ctx, true);
            }
        }
        ui.add_space(8.0);
        #[cfg(feature = "screenshot")]
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                egui::Id::new("home-screenshot"),
                self.screenshot_path.is_some(),
            )
        });
        let error = self
            .mix_error
            .as_ref()
            .filter(|_| !matches!(self.mix_seed, Some(MixSeed::Playlist(_))))
            .or(self.home_error.as_ref())
            .map(|error| error.localized(self.language))
            .or_else(|| {
                self.auth_error
                    .as_ref()
                    .map(|error| error.message(self.language))
            });
        let catalog_mix = self.catalog_mix.as_ref();
        let action = home_ui::show(
            ui,
            &mut self.covers,
            home_ui::HomeView {
                language: self.language,
                library: &self.library.tracks,
                library_total: self.library.total,
                recent: &self.recent,
                playing: self.playing.as_ref(),
                connected: self.session.is_some() || self.demo,
                mix_loading: self
                    .mix_request
                    .as_ref()
                    .is_some_and(|request| request.start && request.seed.is_none()),
                mix_playing: self.mix_active
                    && self.mix_seed.is_none()
                    && matches!(
                        self.player.snapshot().state,
                        PlaybackState::Playing | PlaybackState::Loading
                    ),
                loading: self.home_request.is_some()
                    || (!self.home_requested && self.pending.is_some() && self.session.is_some()),
                collections: &self.home_collections,
                active_collection: self
                    .queue_origin
                    .as_deref()
                    .filter(|_| !self.mix_active && self.playing.is_some())
                    .and_then(|origin| origin.strip_prefix("collection:")),
                pending_collection: self
                    .home_autoplay
                    .as_deref()
                    .filter(|_| self.home_request.is_some()),
                catalog: &self.home_catalog,
                active_mix: catalog_mix
                    .filter(|_| self.mix_active)
                    .filter(|(session, _)| self.mix_seed == Some(MixSeed::Catalog(*session)))
                    .map(|(_, mix)| mix),
                active_mix_playing: matches!(
                    self.player.snapshot().state,
                    PlaybackState::Playing | PlaybackState::Loading
                ),
                pending_mix: catalog_mix
                    .filter(|(session, _)| {
                        self.mix_request.as_ref().is_some_and(|request| {
                            request.start && request.seed == Some(MixSeed::Catalog(*session))
                        })
                    })
                    .map(|(_, mix)| mix),
                error: error.as_deref(),
            },
        );
        // Like My music: a small spinner in the corner while any Home block
        // (VK's picks, the catalog rows or the library tiles) is loading.
        if self.home_request.is_some() || self.catalog_request.is_some() || self.pending.is_some() {
            let area = ui.clip_rect();
            egui::Spinner::new().size(16.0).paint_at(
                ui,
                egui::Rect::from_center_size(
                    area.right_bottom() - egui::vec2(14.0, 14.0),
                    egui::vec2(16.0, 16.0),
                ),
            );
        }
        let ctx = ui.ctx().clone();
        match action {
            Some(home_ui::Action::OpenLibrary) => self.navigate_music(&ctx, MusicPage::Library),
            Some(home_ui::Action::OpenRecent) => {
                self.navigate_music(&ctx, MusicPage::Recent);
            }
            Some(home_ui::Action::PlayMix) => self.play_mix(&ctx),
            Some(home_ui::Action::ConfigureMix) => self.open_mix_settings(&ctx),
            Some(home_ui::Action::PlayRecent) => {
                self.leave_mix();
                if !self.recent.is_empty() {
                    self.start_queue_from(&ctx, self.recent.clone(), "recent".into());
                }
            }
            Some(home_ui::Action::PlayTrack(key)) => {
                if self
                    .playing
                    .as_ref()
                    .is_some_and(|track| track.key() == key)
                {
                    self.toggle_playback(&ctx);
                } else {
                    self.leave_mix();
                    self.replace_play_queue_from(Some(self.recent.clone()), "recent".into());
                    self.play_track(&ctx, key);
                }
            }
            Some(home_ui::Action::OpenCollection(id)) => self.open_collection(&ctx, id, false),
            Some(home_ui::Action::PlayCollection(id)) => self.play_home_collection(&ctx, id),
            Some(home_ui::Action::PlayCatalogMix(mix)) => self.play_catalog_mix(&ctx, mix),
            Some(home_ui::Action::OpenPlaylist(reference)) => {
                self.navigate_music(&ctx, MusicPage::Playlist(reference));
            }
            Some(home_ui::Action::PlayFriendTrack(playlist, key)) => {
                let tracks = self
                    .home_catalog
                    .friends
                    .iter()
                    .find(|friend| (friend.playlist.owner_id, friend.playlist.id) == playlist)
                    .map(|friend| friend.tracks.clone())
                    .unwrap_or_default();
                if tracks.iter().any(|track| track.key() == key) {
                    self.leave_mix();
                    self.replace_play_queue_from(
                        Some(tracks),
                        format!("friend-playlist:{}_{}", playlist.0, playlist.1),
                    );
                    self.play_track(&ctx, key);
                }
            }
            Some(home_ui::Action::SignIn) => self.connect_qr(&ctx),
            None => {}
        }
    }

    fn nav_item(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
        Self::navigation_button(ui, icon, label, selected, 42.0)
    }

    fn navigation_button(
        ui: &mut egui::Ui,
        icon: Icon,
        label: &str,
        selected: bool,
        height: f32,
    ) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
        if selected || response.hovered() {
            ui.painter().rect_filled(
                rect,
                9.0,
                if selected {
                    theme::SELECTED
                } else {
                    theme::BACKGROUND
                },
            );
        }
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 22.0, rect.center().y),
                egui::vec2(19.0, 19.0),
            ),
            icon,
            if selected { ACCENT } else { MUTED },
        );
        ui.painter().text(
            egui::pos2(rect.left() + 43.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        if Self::nav_item(
            ui,
            Icon::Home,
            language.text("Главная", "Home"),
            !matches!(
                self.page,
                MusicPage::Library
                    | MusicPage::Search(_)
                    | MusicPage::Playlist(_)
                    | MusicPage::Album(_)
                    | MusicPage::Artist(_)
            ),
        )
        .clicked()
        {
            self.open_home(ui.ctx());
        }
        if Self::nav_item(
            ui,
            Icon::Search,
            language.text("Поиск", "Search"),
            matches!(self.page, MusicPage::Search(_)),
        )
        .clicked()
        {
            self.open_search(ui.ctx(), None);
        }
        if Self::nav_item(
            ui,
            Icon::Heart,
            language.text("Моя музыка", "My music"),
            self.page == MusicPage::Library,
        )
        .clicked()
        {
            self.navigate_music(ui.ctx(), MusicPage::Library);
        }
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.music.is_none())
        {
            return;
        }
        ui.add_space(12.0);
        let (divider, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().line_segment(
            [
                egui::pos2(divider.left() + 6.0, divider.center().y),
                egui::pos2(divider.right() - 6.0, divider.center().y),
            ],
            egui::Stroke::new(1.0, Color32::from_rgb(57, 65, 77)),
        );
        ui.add_space(0.0);
        let selected =
            if let MusicPage::Playlist(playlist) | MusicPage::Album(playlist) = &self.page {
                Some((playlist.owner_id, playlist.id))
            } else {
                None
            };
        let error = self
            .playlist_error
            .as_ref()
            .or(self.library_browser.error.as_ref())
            .map(|error| error.localized(language));
        let library_loading = self.playlist_request.is_some()
            || !self.playlists_requested
            || self.library_browser.loading();
        if let Some(action) = playlist_ui::show_sidebar(
            ui,
            &mut self.covers,
            &mut self.library_browser.sidebar,
            playlist_ui::SidebarView {
                language,
                items: &self.playlist_items,
                artists: &self.library_browser.artists,
                selected,
                selected_artist: match &self.page {
                    MusicPage::Artist(artist) => Some(&artist.id),
                    _ => None,
                },
                loading: library_loading,
                error: error.as_deref(),
            },
        ) {
            self.playlist_sidebar_action(ui.ctx(), action);
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let ctx = ui.ctx().clone();
        let pagination_id = ui.make_persistent_id("track-autopage");
        let is_library = self.page == MusicPage::Library;
        let rendered_page = self.page.clone();
        let heading = match &self.page {
            MusicPage::Recent => t("Недавнее", "Recent").to_owned(),
            MusicPage::Collection(id) => self
                .home_collections
                .iter()
                .find(|collection| &collection.id == id)
                .map_or_else(
                    || t("Подборка", "Playlist").to_owned(),
                    |collection| collection.title.clone(),
                ),
            _ => t("Моя музыка", "My music").to_owned(),
        };
        ui.horizontal(|ui| {
            if matches!(self.page, MusicPage::Recent | MusicPage::Collection(_))
                && !self.back_history.is_empty()
                && icons::compact_button(ui, Icon::ArrowLeft, t("Назад", "Back"), true, false)
                    .clicked()
            {
                self.go_back(&ctx);
            }
            let title_width =
                (ui.available_width() - if is_library { 92.0 } else { 44.0 }).max(80.0);
            ui.allocate_ui_with_layout(
                egui::vec2(title_width, 36.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    theme::inset_heading(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let count = is_library.then_some(self.library.total).flatten();
                        let number = count.map(|total| {
                            ui.fonts_mut(|fonts| {
                                fonts.layout_no_wrap(
                                    total.to_string(),
                                    egui::FontId::proportional(16.0),
                                    MUTED,
                                )
                            })
                        });
                        let count_width =
                            number.as_ref().map_or(0.0, |galley| galley.size().x + 8.0);
                        let heading_width = (ui.available_width() - count_width).max(0.0);
                        let heading_galley = ui.fonts_mut(|fonts| {
                            fonts.layout_no_wrap(
                                heading.clone(),
                                egui::FontId::new(26.0, theme::semibold()),
                                ui.visuals().text_color(),
                            )
                        });
                        let response = ui.add_sized(
                            egui::vec2(heading_galley.size().x.min(heading_width), 36.0),
                            egui::Label::new(
                                RichText::new(&heading).size(26.0).family(theme::semibold()),
                            )
                            .truncate()
                            .selectable(false),
                        );
                        if let Some(number) = number {
                            let center_y = response.rect.center().y
                                + heading_galley.mesh_bounds.center().y
                                - heading_galley.size().y * 0.5;
                            let (rect, label) = ui.allocate_exact_size(
                                egui::vec2(number.size().x, 36.0),
                                Sense::hover(),
                            );
                            label.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Label,
                                    ui.is_enabled(),
                                    number.text(),
                                )
                            });
                            let pos =
                                egui::pos2(rect.left(), center_y - number.mesh_bounds.center().y);
                            ui.painter().galley(pos, number, MUTED);
                        }
                        response
                    })
                    .on_hover_text(&heading);
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::compact_button(
                    ui,
                    Icon::Refresh,
                    t("Обновить", "Refresh"),
                    self.session.is_some()
                        && self.pending.is_none()
                        && self.home_request.is_none()
                        && self.page != MusicPage::Recent,
                    false,
                )
                .clicked()
                {
                    ctx.data_mut(|data| data.remove::<(i64, usize, usize)>(pagination_id));
                    if let MusicPage::Collection(id) = self.page.clone() {
                        self.open_collection(&ctx, id, true);
                    } else {
                        self.load_page(&ctx, true);
                    }
                }
                if is_library
                    && icons::compact_button(
                        ui,
                        Icon::Plus,
                        t("Загрузить свой трек", "Upload your track"),
                        self.session
                            .as_ref()
                            .is_some_and(|session| session.music.is_some())
                            && !self.demo,
                        false,
                    )
                    .clicked()
                {
                    self.open_audio_upload();
                }
            });
        });
        if self.page != rendered_page || self.settings_open {
            return;
        }
        if self.demo {
            ui.label(
                RichText::new(t(
                    "Демо · вымышленные треки, без подключения к VK",
                    "Demo · sample tracks, no VK connection",
                ))
                .color(MUTED),
            );
        } else if self.session.is_none() {
            theme::inset_heading(ui, |ui| {
                ui.label(
                    RichText::new(t(
                        "Ваша библиотека начнётся с подключения аккаунта",
                        "Connect your account to see your library",
                    ))
                    .color(MUTED),
                )
            });
        }
        ui.add_space(8.0);
        if self.session.is_some() || !self.view_tracks().is_empty() {
            Frame::new()
                .fill(SURFACE)
                .corner_radius(8)
                .inner_margin(6)
                .show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 22.0;
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.horizontal(|ui| {
                        icons::show(ui, Icon::Search, MUTED, 16.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.filter)
                                .frame(Frame::NONE)
                                .desired_width(ui.available_width())
                                .hint_text(t("Поиск в загруженных треках", "Search loaded tracks")),
                        );
                    });
                });
            ui.add_space(8.0);
        }
        if let Some(error) = self.audio_error.as_ref().filter(|_| is_library) {
            Frame::new()
                .fill(Color32::from_rgb(58, 40, 41))
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 12,
                    top: 12,
                    bottom: 12,
                })
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(t("Не удалось загрузить музыку", "Could not load music"))
                            .family(theme::semibold()),
                    );
                    ui.label(error.message(language));
                    ui.label(
                        RichText::new(t(
                            "Профиль подключён, но VK не дал доступ к музыке.",
                            "Your profile is connected, but VK did not grant music access.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                });
            ui.add_space(6.0);
        }
        if !is_library && let Some(error) = &self.home_error {
            theme::inset_heading(ui, |ui| {
                ui.colored_label(Color32::from_rgb(255, 158, 153), error.localized(language))
            });
        }
        if self.view_tracks().is_empty() && !is_library {
            ui.add_space(28.0);
            if self.home_request.is_some() {
                theme::inset_heading(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(t("Загружаем треки…", "Loading tracks…"));
                        ui.spinner();
                    })
                });
            } else if self.home_error.is_none() {
                theme::inset_heading(ui, |ui| {
                    ui.label(if self.page == MusicPage::Recent {
                        t(
                            "Здесь появятся треки, которые вы слушаете в VK Fast.",
                            "Tracks you listen to in VK Fast will appear here.",
                        )
                    } else {
                        t(
                            "VK не вернул треки подборки.",
                            "VK did not return the collection tracks.",
                        )
                    })
                });
            }
            return;
        }
        if self.view_tracks().is_empty() {
            ctx.data_mut(|data| data.remove::<(i64, usize, usize)>(pagination_id));
            if self.pending.is_some() && self.session.is_some() {
                theme::inset_heading(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(t("Загружаем аудиозаписи…", "Loading tracks…"));
                        ui.spinner();
                    })
                });
            } else if self.audio_error.is_none() && self.session.is_some() {
                theme::inset_heading(ui, |ui| {
                    ui.label(if self.library.total == Some(0) {
                        t(
                            "В вашей музыке пока нет треков.",
                            "Your music library is empty.",
                        )
                    } else {
                        t(
                            "VK не вернул треки библиотеки.",
                            "VK did not return the library tracks.",
                        )
                    })
                });
            } else if self.audio_error.is_none() {
                let hero_height = 180.0;
                let top_gap = ((ui.available_height() - hero_height) * 0.25).clamp(0.0, 60.0);
                ui.add_space(top_gap);
                ui.vertical_centered(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("Музыка рядом", "Your music, closer")).size(24.0),
                        )
                        .selectable(false),
                    );
                    ui.label(t(
                        "Войдите в VK, чтобы слушать свою музыку.",
                        "Sign in with VK to listen to your music.",
                    ));
                    ui.add_space(18.0);
                    let width = ui.available_width().min(300.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        Layout::top_down(Align::Center),
                        |ui| {
                            ui.push_id("empty-library-login", |ui| self.login_controls(ui));
                        },
                    );
                });
            }
            return;
        }
        let filtering = !self.filter.trim().is_empty();
        let visible = self
            .track_view_cache
            .visible(self.view_tracks(), &self.filter);
        let show_date = track_sort::Extra {
            album: true,
            date: self.show_date_column(),
        };
        if !visible.is_empty() {
            let index_width = track_index_width(ui, self.view_tracks().len());
            let sort = self.current_sort();
            if let Some(column) =
                track_sort::header(ui, self.language, sort, show_date, index_width, true)
            {
                self.set_sort(column);
                ui.ctx().request_repaint();
            }
        }
        let visible = self.sorted_rows(visible);
        let tail_mode = self.library_tail_mode();
        if tail_mode && self.library_tail_start.is_none() {
            self.load_library_tail(ui.ctx());
        }
        self.row_number_offset = if tail_mode {
            self.library_tail_start.unwrap_or(0)
        } else {
            0
        };
        if visible.is_empty() {
            theme::inset_heading(ui, |ui| {
                ui.label(t(
                    "Среди загруженных треков ничего не найдено.",
                    "No matching tracks in the loaded library.",
                ))
            });
        }
        let row_height = 54.0;
        let active = matches!(
            self.player.snapshot().state,
            PlaybackState::Playing | PlaybackState::Loading
        );
        let mut track_action = None;
        let mut metadata_keys = Vec::new();
        let reorder = is_library && self.library_reorderable();
        let scroll = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.style_mut().interaction.selectable_labels = false;
                theme::scroll_rows(
                    ui,
                    egui::ScrollArea::vertical()
                        .id_salt((
                            "track-list",
                            match &self.page {
                                MusicPage::Recent => "recent",
                                MusicPage::Collection(id) => id,
                                _ => "library",
                            },
                        ))
                        .max_height(ui.available_height().max(row_height))
                        .auto_shrink([false, false]),
                    row_height,
                    visible.len(),
                    |ui, range| {
                        let mut visit = |track: &Track| {
                            if track.artists.is_empty()
                                || track.subtitle.is_none()
                                || track.library_liked.is_none()
                            {
                                metadata_keys.push(track.key());
                            }
                        };
                        if reorder {
                            let stride = row_height + ui.spacing().item_spacing.y;
                            let origin_y = ui.max_rect().top() - range.start as f32 * stride;
                            track_action = self.lane_track_rows(
                                ui,
                                "library",
                                origin_y,
                                range,
                                visible.len(),
                                &|row| visible.source_index(row),
                                active,
                                show_date,
                                &mut visit,
                            );
                            return;
                        }
                        for row in range {
                            let i = visible.source_index(row);
                            let Some(track) = self.view_tracks().get(i).cloned() else {
                                continue;
                            };
                            visit(&track);
                            if let Some(action) = self.track_row(ui, &track, i, active, show_date) {
                                track_action = Some(action);
                            }
                        }
                    },
                )
            })
            .inner;
        self.hydrate_metadata(&ctx, metadata_keys);
        if let Some(action) = track_action {
            self.handle_track_action(&ctx, action);
            if self.page != rendered_page {
                return;
            }
        }
        let near_bottom = scroll.state.offset.y + scroll.inner_rect.height() + row_height * 2.0
            >= scroll.content_size.y;
        if !near_bottom || filtering {
            ctx.data_mut(|data| data.remove::<(i64, usize, usize)>(pagination_id));
        }
        self.row_number_offset = 0;
        if tail_mode {
            // Reversed: scrolling down reads further back from VK's end.
            if near_bottom && !filtering {
                self.load_library_tail(&ctx);
            }
        } else if is_library
            && near_bottom
            && !filtering
            && self.pending.is_none()
            && self.audio_error.is_none()
            && !self.library.exhausted
            && let Some(session) = &self.session
        {
            let current = (
                session.profile.id,
                self.library.offset,
                self.library.tracks.len(),
            );
            let previous = ctx.data(|data| data.get_temp::<(i64, usize, usize)>(pagination_id));
            // Cached rows can repeat in a fresh pagination sequence. Continue
            // while its logical offset advances; empty/repeated pages stop.
            let progressed =
                previous.is_none_or(|previous| previous.0 != current.0 || previous.1 != current.1);
            if progressed {
                ctx.data_mut(|data| data.insert_temp(pagination_id, current));
                self.load_page(&ctx, false);
            }
        }
        if self.pending.is_some() {
            ui.put(
                egui::Rect::from_center_size(
                    scroll.inner_rect.right_bottom() - egui::vec2(14.0, 14.0),
                    egui::vec2(16.0, 16.0),
                ),
                egui::Spinner::new().size(16.0),
            );
        }
    }

    fn track_popup(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        track: &Track,
    ) -> Option<TrackAction> {
        self.track_popup_in(ui, response, &[rect], track)
    }

    fn track_popup_in(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        areas: &[egui::Rect],
        track: &Track,
    ) -> Option<TrackAction> {
        // Child artist/heart controls own their primary clicks. A secondary
        // click anywhere in the visible track region opens the same menu.
        let requested = ui
            .input(|input| {
                input
                    .pointer
                    .button_clicked(egui::PointerButton::Secondary)
                    .then(|| input.pointer.interact_pos())
                    .flatten()
            })
            .is_some_and(|point| {
                ui.is_enabled()
                    && areas
                        .iter()
                        .any(|rect| rect.intersect(ui.clip_rect()).contains(point))
                    && ui.ctx().layer_id_at(point) == Some(ui.layer_id())
            });
        let popup = egui::Popup::context_menu(response)
            .id(response.id.with(("track-context-menu", track.key())));
        let popup = if requested {
            popup.open_memory(egui::SetOpenCommand::Bool(true))
        } else {
            popup
        };
        popup
            .show(|menu| self.track_context_menu(menu, track))
            .and_then(|inner| inner.inner)
    }

    fn track_context_menu(&mut self, ui: &mut egui::Ui, track: &Track) -> Option<TrackAction> {
        let connected = !self.demo && self.search_ready();
        let removable = self.removable_playlist(track);
        let action = track_menu::show(
            ui,
            track_menu::View {
                language: self.language,
                track,
                liked: self.track_is_liked(track),
                like_enabled: connected
                    && self.like_request.is_none()
                    && self.playlist_add_request.is_none(),
                like_pending: self
                    .like_request
                    .as_ref()
                    .is_some_and(|request| request.source.key() == track.key()),
                playlists: &self.editable_playlists,
                playlists_loading: self.playlist_request.is_some() || !self.playlists_requested,
                playlist_busy: self.playlist_add_request.is_some() || self.like_request.is_some(),
                connected,
                disliked: self.disliked.contains(&track.key()),
                dislike_pending: self.dislike_request.is_some(),
                removable_from: removable.as_ref(),
            },
        )?;
        Some(TrackAction::Menu(track.clone(), action))
    }

    fn handle_track_menu_action(
        &mut self,
        ctx: &egui::Context,
        track: &Track,
        action: track_menu::Action,
    ) {
        self.track_menu_error = None;
        self.track_menu_notice = None;
        self.like_error = None;
        match action {
            track_menu::Action::AddToQueue => self.add_to_queue(ctx, track.clone()),
            track_menu::Action::ToggleLike => self.toggle_like(ctx, track.clone()),
            track_menu::Action::ToggleDislike => self.toggle_dislike(ctx, track),
            track_menu::Action::AddToPlaylist(playlist) => {
                self.add_track_to_playlist(ctx, track.clone(), playlist)
            }
            track_menu::Action::RemoveFromPlaylist(playlist) => {
                self.remove_from_playlist(ctx, track, playlist)
            }
            track_menu::Action::TrackMix => self.play_track_mix(
                ctx,
                track
                    .release_key
                    .filter(|key| key.0 != 0 && key.1 > 0)
                    .unwrap_or(track.key()),
            ),
            track_menu::Action::Artist(artist) => {
                self.set_lyrics_fullscreen(ctx, false);
                self.navigate_music(ctx, MusicPage::Artist(artist))
            }
            track_menu::Action::Album => {
                let action = track.album.as_ref().map_or_else(
                    || TrackAction::TrackAlbum(track.key(), track.display_title().into_owned()),
                    |album| TrackAction::Album(album.clone()),
                );
                self.handle_track_action(ctx, action);
            }
            track_menu::Action::CopyLink => {
                let key = track
                    .release_key
                    .filter(|key| key.0 != 0 && key.1 > 0)
                    .unwrap_or(track.key());
                if key.0 != 0 && key.1 > 0 {
                    ctx.copy_text(format!("https://vk.ru/audio{}_{}", key.0, key.1));
                    self.track_menu_notice = Some((
                        self.language
                            .text("Ссылка скопирована", "Link copied")
                            .into(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                }
            }
            track_menu::Action::RefreshPlaylists => self.load_playlists(ctx, true),
            track_menu::Action::CopyDetails(detail) => {
                let text = detail.text(track);
                if !text.is_empty() {
                    ctx.copy_text(text);
                    self.track_menu_notice = Some((
                        self.language.text("Скопировано", "Copied").into(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                }
            }
        }
    }

    fn add_track_to_playlist(&mut self, ctx: &egui::Context, source: Track, target: AlbumRef) {
        if self.demo
            || !self.search_ready()
            || self.playlist_add_request.is_some()
            || self.like_request.is_some()
        {
            return;
        }
        if !self
            .editable_playlists
            .iter()
            .any(|playlist| playlist.owner_id == target.owner_id && playlist.id == target.id)
        {
            self.track_menu_error = Some(WebAudioError::Rejected);
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let worker_target = target.clone();
        let source_key = source.key();
        let repaint = self.repaint_target.with_context(ctx);
        let (sender, receiver) = mpsc::sync_channel(1);
        self.like_error = None;
        match thread::Builder::new()
            .name("vk-playlist-add".into())
            .spawn(move || {
                let result =
                    client.add_to_playlist(owner, &worker_target, source_key, &worker_cancel);
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.playlist_add_request = Some(PlaylistAddRequest {
                    owner,
                    source,
                    target,
                    canceled,
                    receiver,
                });
            }
            Err(_) => self.track_menu_error = Some(WebAudioError::Network),
        }
    }

    fn poll_playlist_add(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.playlist_add_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self
            .playlist_add_request
            .take()
            .expect("Active playlist addition");
        if request.canceled.load(Ordering::Acquire)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.profile.id != request.owner)
        {
            return;
        }
        match result {
            Ok(owned) => {
                if owned.0 == request.owner {
                    self.apply_library_change(ctx, &request.source, LibraryChange::Added(owned));
                }
                let playlist_key = MusicPage::Playlist(request.target.clone())
                    .detail_key()
                    .unwrap();
                let album_key = MusicPage::Album(request.target.clone())
                    .detail_key()
                    .unwrap();
                let active = self.page.detail_key();
                for key in [&playlist_key, &album_key] {
                    if active.as_deref() != Some(key) {
                        self.details.remove(key);
                    }
                }
                if active.as_deref() == Some(&playlist_key) || active.as_deref() == Some(&album_key)
                {
                    self.ensure_detail(ctx, true);
                }
                // A confirmed mutation is a reason to update the sidebar once.
                self.load_playlists(ctx, true);
                self.track_menu_notice = Some((
                    self.language
                        .text("Трек добавлен в плейлист", "Track added to playlist")
                        .into(),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            Err(WebAudioError::Canceled) => {}
            Err(error @ WebAudioError::WrongAccount) => {
                self.reset();
                self.auth_error = Some(UiError::Music(error));
            }
            Err(error) => self.track_menu_error = Some(error),
        }
        ctx.request_repaint();
    }

    fn track_row(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        index: usize,
        active: bool,
        show_date: track_sort::Extra,
    ) -> Option<TrackAction> {
        let highlighted = self.selected == Some(track.key())
            || self
                .playing
                .as_ref()
                .is_some_and(|current| current.key() == track.key());
        let playing = self
            .playing
            .as_ref()
            .is_some_and(|current| current.key() == track.key());
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), Sense::hover());
        let response = ui.interact(
            rect,
            ui.id().with(("track-row", track.key(), index)),
            if self.row_drag_enabled {
                Sense::click_and_drag()
            } else {
                Sense::click()
            },
        );
        if self.row_drag_enabled && response.drag_started_by(egui::PointerButton::Primary) {
            self.row_drag_started = Some(rect);
        }
        let row_hovered = ui.is_enabled()
            && response.contains_pointer()
            && ui.input(|input| {
                input
                    .pointer
                    .hover_pos()
                    .is_some_and(|point| rect.contains(point))
            });
        ui.painter()
            .rect_filled(rect, 5.0, if row_hovered { SURFACE } else { BACKGROUND });
        let index_width = track_index_width(
            ui,
            self.view_tracks().len().max(index + 1) + self.row_number_offset,
        );
        let unavailable = track.unavailable == Some(true);
        let mut action = None;
        let mut controls_contain_pointer = false;
        let mut index_contains_pointer = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(("track", track.key(), index))
                .max_rect(rect.shrink2(egui::vec2(4.0, 6.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if unavailable {
                    ui.set_opacity(ui.opacity() * 0.65);
                    ui.visuals_mut().override_text_color = Some(MUTED);
                }
                let (index_rect, index_response) =
                    ui.allocate_exact_size(egui::vec2(index_width, 40.0), Sense::click());
                index_contains_pointer = index_response.contains_pointer();
                if index_response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    action = Some(TrackAction::Play(track.key()));
                }
                if playing || row_hovered {
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(
                            index_rect.right_center() - egui::vec2(11.0, 0.0),
                            egui::vec2(16.0, 16.0),
                        ),
                        if playing && active {
                            Icon::Pause
                        } else {
                            Icon::Play
                        },
                        if unavailable {
                            MUTED
                        } else if playing {
                            ACCENT
                        } else {
                            Color32::WHITE
                        },
                    );
                } else {
                    ui.painter().text(
                        index_rect.right_center() - egui::vec2(3.0, 0.0),
                        egui::Align2::RIGHT_CENTER,
                        (index + 1 + self.row_number_offset).to_string(),
                        egui::TextStyle::Small.resolve(ui.style()),
                        if !unavailable && highlighted {
                            ACCENT
                        } else {
                            MUTED
                        },
                    );
                }
                self.covers.show(ui, track.cover_url.as_deref(), 40.0);
                let columns = show_date.fit(ui.available_width() - track_sort::CONTROLS_WIDTH);
                let width = columns.title;
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 40.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_min_width(width);
                        ui.spacing_mut().item_spacing.y = 3.0;
                        // Title and artists centred on the 40 px cover, not
                        // pinned to its top.
                        let title_font = egui::FontId::new(14.0, theme::semibold());
                        let small = egui::TextStyle::Small.resolve(ui.style());
                        let (title_height, artist_height) = ui.fonts_mut(|fonts| {
                            (
                                fonts.row_height(&title_font),
                                fonts.row_height(&small).max(16.0),
                            )
                        });
                        ui.add_space(((40.0 - title_height - 3.0 - artist_height) * 0.5).max(0.0));
                        ui.add(
                            egui::Label::new(theme::track_title(
                                track,
                                if unavailable {
                                    MUTED
                                } else if highlighted {
                                    ACCENT
                                } else {
                                    ui.visuals().text_color()
                                },
                                egui::FontId::new(14.0, theme::semibold()),
                            ))
                            .truncate()
                            .selectable(false),
                        );
                        let badge = theme::explicit_badge(
                            ui.ctx(),
                            Some(track),
                            egui::TextStyle::Small.resolve(ui.style()).size,
                        );
                        if let Some(artist) = theme::badged_line(ui, badge, |ui| {
                            detail_ui::artist_links(ui, &track.artists, &track.artist)
                        }) {
                            action = Some(TrackAction::Artist(artist));
                        }
                    },
                );
                if let Some(album_width) = columns.album {
                    ui.allocate_ui_with_layout(
                        egui::vec2(album_width, 40.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(album_width);
                            if let Some(label) = self.album_titles.label(track)
                                && let Some(album) = self.album_titles.named(track)
                            {
                                let link = ui
                                    .add(
                                        egui::Label::new(RichText::new(label).color(MUTED))
                                            .truncate()
                                            .selectable(false)
                                            .sense(Sense::click()),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                if link.hovered() {
                                    ui.painter().hline(
                                        link.rect.x_range(),
                                        link.rect.bottom() - 1.0,
                                        egui::Stroke::new(1.0, MUTED),
                                    );
                                }
                                controls_contain_pointer |= link.contains_pointer();
                                if link.clicked() {
                                    action = Some(TrackAction::Album(album));
                                }
                            }
                        },
                    );
                }
                if columns.date {
                    let label = track
                        .added
                        .map(|added| track_sort::date_label(added, self.language))
                        .unwrap_or_default();
                    ui.allocate_ui_with_layout(
                        egui::vec2(track_sort::DATE_WIDTH, 40.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(track_sort::DATE_WIDTH);
                            ui.add(
                                egui::Label::new(RichText::new(label).color(MUTED))
                                    .truncate()
                                    .selectable(false),
                            );
                        },
                    );
                }
                let mix = self.track_mix_button(ui);
                controls_contain_pointer |= mix.contains_pointer();
                if mix.clicked() {
                    action = Some(TrackAction::Menu(
                        track.clone(),
                        track_menu::Action::TrackMix,
                    ));
                }
                let dislike = self.dislike_button(ui, track);
                controls_contain_pointer |= dislike.contains_pointer();
                if dislike.clicked() {
                    action = Some(TrackAction::Menu(
                        track.clone(),
                        track_menu::Action::ToggleDislike,
                    ));
                }
                let heart = self.like_button(ui, track);
                controls_contain_pointer |= heart.contains_pointer();
                if heart.clicked() {
                    action = Some(TrackAction::ToggleLike(track.clone()));
                }
                ui.add(
                    egui::Label::new(RichText::new(track.duration_label()).color(MUTED))
                        .selectable(false),
                );
            },
        );
        if let Some(intent) = self.track_popup(ui, &response, rect, track) {
            action = Some(intent);
        }
        release_pointer_focus(&response);
        ui.advance_cursor_after_rect(rect);
        let row_clicked = action.is_none()
            && !controls_contain_pointer
            && !index_contains_pointer
            && response.clicked_by(egui::PointerButton::Primary);
        let play = if self.double_click_to_play {
            track_double_click(ui, &response, row_clicked)
        } else {
            row_clicked
        };
        if play {
            action = Some(TrackAction::Play(track.key()));
        }
        action
    }

    fn handle_track_action(&mut self, ctx: &egui::Context, action: TrackAction) {
        if self.lyrics.fullscreen
            && matches!(
                &action,
                TrackAction::Artist(_) | TrackAction::Album(_) | TrackAction::TrackAlbum(..)
            )
        {
            self.set_lyrics_fullscreen(ctx, false);
        }
        match action {
            TrackAction::Reorder(moved) => self.reorder_view(ctx, moved),
            TrackAction::Menu(track, action) => self.handle_track_menu_action(ctx, &track, action),
            TrackAction::ToggleLike(track) => self.toggle_like(ctx, track),
            TrackAction::Artist(artist) => self.navigate_music(ctx, MusicPage::Artist(artist)),
            TrackAction::Album(album) => self.navigate_music(ctx, MusicPage::Album(album)),
            TrackAction::TrackAlbum(key, title) => {
                self.navigate_music(ctx, MusicPage::TrackAlbum(key, title))
            }
            TrackAction::Play(key) => {
                if self.demo {
                    self.selected = Some(key);
                } else if self
                    .playing
                    .as_ref()
                    .is_some_and(|track| track.key() == key)
                {
                    self.toggle_playback(ctx);
                } else {
                    self.play_from_view(ctx, key);
                }
            }
        }
    }

    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let ctx = ui.ctx().clone();
        ui.spacing_mut().item_spacing.y = 4.0;
        let snapshot = self.playback_snapshot();
        let resolving = self.playback_pending.is_some();
        let active = matches!(
            &snapshot.state,
            PlaybackState::Playing | PlaybackState::Loading
        );
        let ready = !self.demo
            && self.session.is_some()
            && (self.playing.is_some()
                || !self.view_tracks().is_empty()
                || !self.play_order.queued().is_empty());
        let track = self.playing.as_ref().or_else(|| {
            self.library
                .tracks
                .iter()
                .find(|track| Some(track.key()) == self.selected)
        });
        let duration = track.map_or(0, |track| track.duration);
        let time_key = track.map(Track::key);
        let time_slots = match self.time_slots {
            Some(slots) if slots.key == time_key && slots.duration == duration => slots,
            _ => {
                let slots = PlayerTimeSlots {
                    key: time_key,
                    duration,
                    width: time_slot_width(
                        ui,
                        if duration == 0 {
                            snapshot.position.ceil() as u32
                        } else {
                            duration
                        },
                    ),
                };
                self.time_slots = Some(slots);
                slots
            }
        };
        let title_track = track.cloned();
        let has_track = track.is_some();
        let track_key = track.map(Track::key);
        let artists = track.map_or_else(Vec::new, |track| track.artists.clone());
        let album = track.and_then(|track| track.album.clone());
        let mut metadata_action = None;
        let cover_url = track.and_then(|track| track.cover_url.clone());
        let title = track.map_or_else(
            || t("Выберите трек", "Choose a track").to_owned(),
            |track| track.display_title().into_owned(),
        );
        let artist = track
            .map_or(
                t(
                    "Нажмите на трек в библиотеке",
                    "Click a track in your library",
                ),
                |track| track.artist.as_str(),
            )
            .to_owned();
        let (body, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 80.0), Sense::hover());
        let seek_row = player_seek_rect(body, self.compact_seek_bar, time_slots.width);
        let panel_top = body.top() - f32::from(PLAYER_PADDING_Y);
        let row = egui::Rect::from_center_size(
            egui::pos2(body.center().x, (panel_top + seek_row.center().y) * 0.5),
            egui::vec2(body.width(), 44.0),
        );
        let transport = egui::Rect::from_center_size(
            row.center(),
            egui::vec2(PLAYER_TRANSPORT_WIDTH, row.height()),
        );
        let side_row = if self.compact_seek_bar {
            egui::Rect::from_center_size(body.center(), egui::vec2(body.width(), 64.0))
        } else {
            row
        };
        let artwork_size = if self.compact_seek_bar { 64.0 } else { 40.0 };
        let title_font = egui::FontId::new(14.0, theme::semibold());
        let artist_font = egui::TextStyle::Small.resolve(ui.style());
        let (title_line_height, artist_line_height) = ui.fonts_mut(|fonts| {
            (
                fonts.row_height(&title_font).max(22.0),
                fonts.row_height(&artist_font).max(16.0),
            )
        });
        let metadata_height = title_line_height + artist_line_height + 2.0;
        let metadata_right = if self.compact_seek_bar {
            seek_row.left().min(transport.left()) - PLAYER_COLUMN_GAP
        } else {
            transport.left() - PLAYER_COLUMN_GAP
        };
        let metadata = egui::Rect::from_min_max(
            side_row.min,
            egui::pos2(metadata_right.max(side_row.left()), side_row.bottom()),
        );
        let mut menu_areas = Vec::with_capacity(2);
        let metadata_response = ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("player-metadata")
                .max_rect(metadata)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(metadata));
                let artwork = self.covers.show(ui, cover_url.as_deref(), artwork_size);
                if let Some(album) = &album {
                    let response = ui
                        .interact(artwork.rect, ui.id().with("player-album"), Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(language.text("Открыть альбом", "Open album"));
                    if response.clicked() {
                        metadata_action = Some(TrackAction::Album(album.clone()));
                    }
                }
                let cover_gap = ui.spacing().item_spacing.x;
                let heart_gap = 10.0;
                let action_gap = 6.0;
                let actions_width = 3.0 * 22.0 + 2.0 * action_gap;
                let max_text_width = (metadata.width()
                    - artwork_size
                    - cover_gap
                    - if has_track {
                        actions_width + heart_gap
                    } else {
                        0.0
                    })
                .max(0.0);
                let title_job = title_track.as_ref().map_or_else(
                    || {
                        egui::text::LayoutJob::simple_singleline(
                            title.clone(),
                            title_font.clone(),
                            ui.visuals().text_color(),
                        )
                    },
                    |track| {
                        theme::track_title(track, ui.visuals().text_color(), title_font.clone())
                    },
                );
                let title_width = ui.painter().layout_job(title_job.clone()).size().x;
                let artist_width = ui.fonts_mut(|fonts| {
                    let mut width = |text: &str| {
                        fonts
                            .layout_no_wrap(text.to_owned(), artist_font.clone(), MUTED)
                            .size()
                            .x
                    };
                    if artists.is_empty() {
                        width(&artist)
                    } else {
                        let separator = width(", ");
                        artists
                            .iter()
                            .map(|artist| width(&artist.name))
                            .sum::<f32>()
                            + separator * artists.len().saturating_sub(1) as f32
                    }
                });
                let artist_badge =
                    theme::explicit_badge(ui.ctx(), title_track.as_ref(), artist_font.size);
                let artist_width =
                    artist_width + artist_badge.map_or(0.0, |size| size + theme::EXPLICIT_GAP);
                let text_width = title_width.max(artist_width).ceil().min(max_text_width);
                ui.spacing_mut().item_spacing.x = heart_gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, metadata_height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), title_line_height),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let width = title_width.min(ui.available_width().max(0.0));
                                let title_response = ui.add_sized(
                                    egui::vec2(width, title_line_height),
                                    egui::Label::new(title_job.clone())
                                        .truncate()
                                        .selectable(false)
                                        .sense(if has_track {
                                            Sense::click()
                                        } else {
                                            Sense::hover()
                                        }),
                                );
                                menu_areas.push(title_response.rect);
                                if let Some(key) = track_key
                                    && title_response
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(t(
                                            "Открыть альбом / сингл",
                                            "Open album / single",
                                        ))
                                        .clicked()
                                {
                                    metadata_action = Some(album.as_ref().map_or_else(
                                        || TrackAction::TrackAlbum(key, title.clone()),
                                        |album| TrackAction::Album(album.clone()),
                                    ));
                                }
                            },
                        );
                        menu_areas.push(egui::Rect::from_min_size(
                            ui.cursor().min,
                            egui::vec2(
                                artist_width.min(ui.available_width().max(0.0)),
                                artist_line_height,
                            ),
                        ));
                        if has_track {
                            if let Some(artist) = theme::badged_line(ui, artist_badge, |ui| {
                                detail_ui::artist_links_with_font(
                                    ui,
                                    &artists,
                                    &artist,
                                    artist_font.clone(),
                                )
                            }) {
                                metadata_action = Some(TrackAction::Artist(artist));
                            }
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&artist)
                                        .font(artist_font.clone())
                                        .color(MUTED),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        }
                    },
                );
                if let Some(track) = &title_track {
                    let (slot, _) = ui.allocate_exact_size(
                        egui::vec2(actions_width, metadata_height),
                        Sense::hover(),
                    );
                    let middle = (title_line_height * 0.5
                        + title_line_height
                        + 2.0
                        + artist_line_height * 0.5)
                        * 0.5;
                    let actions = egui::Rect::from_center_size(
                        egui::pos2(slot.center().x, slot.top() + middle),
                        egui::vec2(actions_width, 22.0),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("player-track-actions")
                            .max_rect(actions)
                            .layout(Layout::left_to_right(Align::Center)),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = action_gap;
                            if self.like_button(ui, track).clicked() {
                                metadata_action = Some(TrackAction::ToggleLike(track.clone()));
                            }
                            if self.dislike_button(ui, track).clicked() {
                                metadata_action = Some(TrackAction::Menu(
                                    track.clone(),
                                    track_menu::Action::ToggleDislike,
                                ));
                            }
                            if self.track_mix_button(ui).clicked() {
                                metadata_action = Some(TrackAction::Menu(
                                    track.clone(),
                                    track_menu::Action::TrackMix,
                                ));
                            }
                        },
                    );
                }
            },
        );
        if let Some(track) = &title_track
            && let Some(action) =
                self.track_popup_in(ui, &metadata_response.response, &menu_areas, track)
        {
            metadata_action = Some(action);
        }
        if let Some(action) = metadata_action {
            self.handle_track_action(&ctx, action);
        }
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("player-transport")
                .max_rect(transport)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let shuffle = icons::toggle_button(
                    ui,
                    Icon::Shuffle,
                    if self.play_order.shuffle() {
                        t("Перемешивание: вкл.", "Shuffle: on")
                    } else {
                        t("Перемешивание: выкл.", "Shuffle: off")
                    },
                    ready,
                    self.play_order.shuffle(),
                );
                release_pointer_focus(&shuffle);
                if shuffle.clicked() {
                    self.toggle_shuffle();
                }
                let previous = icons::compact_button(
                    ui,
                    Icon::Previous,
                    t("Предыдущий трек", "Previous track"),
                    ready && (self.playing.is_some() || self.adjacent_track(false).is_some()),
                    false,
                );
                release_pointer_focus(&previous);
                if previous.clicked() {
                    self.previous_track(&ctx, true);
                }
                let play = icons::compact_button(
                    ui,
                    if active { Icon::Pause } else { Icon::Play },
                    if active {
                        t("Пауза", "Pause")
                    } else {
                        t("Воспроизвести", "Play")
                    },
                    ready && !resolving,
                    true,
                );
                release_pointer_focus(&play);
                if play.clicked() {
                    self.toggle_playback(&ctx);
                }
                let next = icons::compact_button(
                    ui,
                    Icon::Next,
                    t("Следующий трек", "Next track"),
                    ready
                        && (self.adjacent_track(true).is_some()
                            || (self.library_queue_active() && !self.library.exhausted)
                            || (self.mix_active && self.mix_has_more)),
                    false,
                );
                release_pointer_focus(&next);
                if next.clicked() {
                    self.next_track(&ctx);
                }
                let repeat = self.play_order.repeat();
                let repeat_label = match repeat {
                    RepeatMode::Off => t("Повтор: выключен", "Loop: off"),
                    RepeatMode::All => t("Повтор очереди", "Loop: queue"),
                    RepeatMode::One => t("Повтор трека", "Loop: track"),
                };
                let repeat_button = icons::toggle_button(
                    ui,
                    if repeat == RepeatMode::One {
                        Icon::LoopOne
                    } else {
                        Icon::Loop
                    },
                    repeat_label,
                    ready,
                    repeat != RepeatMode::Off,
                );
                release_pointer_focus(&repeat_button);
                if repeat_button.clicked() {
                    self.play_order.cycle_repeat();
                    self.save_playback_modes();
                }
            },
        );
        let volume_left = if self.compact_seek_bar {
            (side_row.right() - 252.0).max(seek_row.right() + PLAYER_COLUMN_GAP)
        } else {
            side_row.right() - 252.0
        };
        let volume_rect = egui::Rect::from_min_max(
            egui::pos2(volume_left.min(side_row.right()), side_row.top()),
            side_row.max,
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("player-volume")
                .max_rect(volume_rect)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(volume_rect));
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut volume = snapshot.volume;
                let volume_width = (volume_rect.width() - 124.0).clamp(17.0, 120.0);
                let response =
                    seek_bar::volume(ui, &mut volume, volume_width, t("Громкость", "Volume"));
                if response.changed() {
                    self.player.set_volume(volume);
                    self.volume_save_at = Some(Instant::now() + Duration::from_millis(500));
                }
                release_pointer_focus(&response);
                response.on_hover_text(if self.volume_save_failed {
                    t(
                        "Не удалось сохранить громкость",
                        "Could not save the volume",
                    )
                    .into()
                } else {
                    format!("{}: {:.0}%", t("Громкость", "Volume"), volume * 100.0)
                });
                let mute = icons::toggle_button_sized(
                    ui,
                    Icon::volume(volume),
                    if volume > 0.0 {
                        t("Выключить звук", "Mute")
                    } else {
                        t("Включить звук", "Unmute")
                    },
                    true,
                    false,
                    32.0,
                );
                release_pointer_focus(&mute);
                if mute.clicked() {
                    self.player.toggle_mute();
                    self.volume_save_at = Some(Instant::now() + Duration::from_millis(500));
                }
                let queue = icons::toggle_button_sized(
                    ui,
                    Icon::Queue,
                    t("Очередь", "Queue"),
                    true,
                    self.queue_ui.open,
                    32.0,
                );
                release_pointer_focus(&queue);
                if queue.clicked() {
                    self.toggle_queue(&ctx);
                }
                let lyrics = icons::toggle_button_sized(
                    ui,
                    Icon::Lyrics,
                    t("Текст", "Lyrics"),
                    true,
                    self.lyrics.active && self.lyrics.open,
                    32.0,
                );
                release_pointer_focus(&lyrics);
                if lyrics.clicked() {
                    self.toggle_lyrics(&ctx);
                }
            },
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("player-position")
                .max_rect(seek_row)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                let mut position = bounded_track_position(
                    if duration == 0 {
                        snapshot.position
                    } else {
                        self.seek_position.unwrap_or(snapshot.position)
                    },
                    duration,
                );
                // An unknown total has no meaningful progress fraction. Keep
                // its committed time visible without filling the disabled bar.
                let mut progress = if duration == 0 { 0.0 } else { position };
                let can_seek = self.playing.is_some()
                    && !resolving
                    && duration > 0
                    && matches!(
                        &snapshot.state,
                        PlaybackState::Playing
                            | PlaybackState::Paused
                            | PlaybackState::Loading
                            | PlaybackState::Ended
                    );
                let gap = PLAYER_TIME_GAP;
                let elapsed_rect = egui::Rect::from_min_size(
                    seek_row.min,
                    egui::vec2(time_slots.width, seek_row.height()),
                );
                let duration_rect = egui::Rect::from_min_max(
                    egui::pos2(seek_row.right() - time_slots.width, seek_row.top()),
                    seek_row.max,
                );
                let control_rect = egui::Rect::from_min_max(
                    egui::pos2(elapsed_rect.right() + gap, seek_row.top()),
                    egui::pos2(duration_rect.left() - gap, seek_row.bottom()),
                );
                self.seek_pixels = control_rect.width() * ui.ctx().pixels_per_point();
                let response = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .id_salt(("seek-control", self.playing.as_ref().map(Track::key)))
                            .max_rect(control_rect)
                            .layout(Layout::left_to_right(Align::Center)),
                        |ui| {
                            ui.add_enabled_ui(can_seek, |ui| {
                                seek_bar::show(
                                    ui,
                                    &mut progress,
                                    seek_bar::Progress {
                                        duration: f64::from(duration.max(1)),
                                        width: control_rect.width(),
                                        label: t("Позиция воспроизведения", "Playback position"),
                                    },
                                )
                            })
                            .inner
                        },
                    )
                    .inner;
                if duration > 0 {
                    position = progress;
                }
                if let Some(target) = seek_commit(
                    &mut self.seek_position,
                    position,
                    f64::from(duration.max(1)),
                    &response,
                ) {
                    position = target;
                    self.cancel_mix_start();
                    self.mix_advance = false;
                    self.home_autoplay = None;
                    self.advance_on_load = false;
                    self.finished_handled = false;
                    self.commit_playback_seek(target);
                }
                release_pointer_focus(&response);
                paint_time(
                    ui,
                    elapsed_rect,
                    time_label(position),
                    egui::Align2::RIGHT_CENTER,
                    "elapsed-time",
                );
                paint_time(
                    ui,
                    duration_rect,
                    time_label(f64::from(duration)),
                    egui::Align2::LEFT_CENTER,
                    "track-duration",
                );
            },
        );
        if let Some(error) = &self.playback_error {
            ui.colored_label(Color32::from_rgb(255, 158, 153), error.message(language));
        } else if let PlaybackState::Failed(error) = &snapshot.state {
            ui.colored_label(
                Color32::from_rgb(255, 158, 153),
                UiError::Playback(error.clone()).message(language),
            );
        } else if self.demo {
            ui.label(
                RichText::new(t(
                    "Демо: вымышленные треки без аудио",
                    "Demo: sample tracks without audio",
                ))
                .small()
                .color(MUTED),
            );
        }
    }

    fn qr_accepts_code(&self) -> bool {
        self.qr_texture.is_some()
            && self.qr_pending.is_some()
            && self
                .qr_expires
                .is_some_and(|deadline| deadline > Instant::now())
            && matches!(
                self.qr_stage,
                qr_auth::Stage::Scanned | qr_auth::Stage::Confirming
            )
    }

    fn qr_window(&mut self, ctx: &egui::Context) -> Option<QrWindowLayout> {
        if !self.qr_open {
            return None;
        }
        let code_entry = self.qr_accepts_code();
        let error = self
            .qr_error
            .as_ref()
            .map(|error| error.message(self.language));
        let output = crate::qr_login_ui::show(
            ctx,
            crate::qr_login_ui::View {
                language: self.language,
                texture: self.qr_texture.as_ref(),
                stage: self.qr_stage,
                pending: self.qr_pending.is_some(),
                remaining: self
                    .qr_expires
                    .map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs()),
                code_entry,
                code: &mut self.qr_code_input,
                error: error.as_deref(),
            },
        );
        match output.action {
            Some(crate::qr_login_ui::Action::Cancel) => {
                self.cancel_qr();
                if self.session.is_none() && !self.demo && self.pending.is_none() {
                    self.restore_session(ctx);
                }
            }
            Some(crate::qr_login_ui::Action::Refresh) => self.connect_qr(ctx),
            Some(crate::qr_login_ui::Action::Submit) => {
                if let Some(control) = &self.qr_control {
                    self.qr_error = None;
                    control.submit_code(std::mem::take(&mut self.qr_code_input));
                }
            }
            None => {}
        }
        Some(output.layout)
    }

    fn start_cache_job(&mut self, ctx: &egui::Context, action: CacheAction) {
        if self.cache_pending.is_some() {
            return;
        }
        let Some(cache) = self.audio_cache.clone() else {
            self.cache_error = true;
            return;
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-audio-cache".into())
            .spawn(move || {
                let result = match action {
                    CacheAction::Info => Ok(()),
                    CacheAction::Apply(limit) => cache.set_limit(limit),
                    CacheAction::Clear => cache.clear(),
                }
                .map(|()| cache.stats())
                .map_err(|_| ());
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.cache_pending = Some(receiver);
            }
            Err(_) => self.cache_error = true,
        }
    }

    /// Measure artwork and history, or clear artwork, off the UI thread.
    /// Runs when Settings open and after a change, never on a timer.
    /// Read the system link handler, or turn it on or off first. Runs when
    /// Settings open and on the switch, off the UI thread.
    fn start_link_handler_job(&mut self, ctx: &egui::Context, enable: Option<bool>) {
        // Tests never read or change the system's link handlers.
        if cfg!(test) || self.link_handler_pending.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-link-handler".into())
            .spawn(move || {
                let result = enable.map(|enable| {
                    if enable {
                        crate::link_router::enable()
                    } else {
                        crate::link_router::disable()
                    }
                });
                let _ = sender.send((result, crate::link_router::status()));
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.link_handler_pending = Some(receiver);
            }
            Err(_) => self.link_handler_error = Some(crate::link_router::RouterError::System),
        }
    }

    fn poll_link_handler(&mut self) {
        let Some(receiver) = &self.link_handler_pending else {
            return;
        };
        match receiver.try_recv() {
            Ok((result, status)) => {
                self.link_handler_pending = None;
                if let Some(result) = result {
                    self.link_handler_error = result.err();
                }
                self.link_handler = Some(status);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.link_handler_pending = None;
                self.link_handler_error = Some(crate::link_router::RouterError::System);
            }
        }
    }

    /// Take links sent by later launches and open the newest one as soon as
    /// the music session can load it.
    fn poll_links(&mut self, ctx: &egui::Context) {
        if let Some(activation) = &self.instance_activation {
            for link in activation.take_links() {
                if let Some(target) = crate::link_router::parse(&link) {
                    self.pending_link = Some(target);
                }
            }
        }
        if self.demo
            || !self
                .session
                .as_ref()
                .is_some_and(|session| session.music.is_some())
        {
            return;
        }
        let Some(target) = self.pending_link.take() else {
            return;
        };
        if let crate::link_router::Target::Track(key) = target {
            self.play_linked_track(ctx, key);
            return;
        }
        if self.lyrics.fullscreen {
            self.set_lyrics_fullscreen(ctx, false);
        }
        self.navigate_music(ctx, link_page(target));
    }

    /// A track link plays the track itself, as its own one-track queue.
    fn play_linked_track(&mut self, ctx: &egui::Context, key: (i64, i64)) {
        if let Some(request) = self.link_track.take() {
            request.canceled.store(true, Ordering::Release);
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-link-track".into())
            .spawn(move || {
                let result = client
                    .tracks_metadata(owner, &[key], &worker_cancel)
                    .and_then(|tracks| tracks.into_iter().next().ok_or(WebAudioError::NoStream));
                let _ = sender.send(result);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.link_track = Some(LinkTrackRequest { canceled, receiver });
            }
            Err(_) => self.track_menu_error = Some(WebAudioError::Network),
        }
    }

    fn poll_link_track(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.link_track else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        self.link_track = None;
        match result {
            Ok(track) => {
                let origin = format!("link:{}_{}", track.owner_id, track.id);
                self.leave_mix();
                self.start_queue_from(ctx, vec![track], origin);
            }
            Err(WebAudioError::Canceled) => {}
            Err(error) => self.track_menu_error = Some(error),
        }
    }

    fn link_handler_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        use crate::link_router::RouterError;
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let busy = self.link_handler_pending.is_some();
        let status = self.link_handler.clone();
        let mut enabled = status.as_ref().is_some_and(|status| status.enabled);
        ui.horizontal(|ui| {
            let changed = ui
                .add_enabled_ui(status.is_some() && !busy, |ui| {
                    theme::switch(
                        ui,
                        &mut enabled,
                        t(
                            "Открывать ссылки VK Музыки в VK Fast",
                            "Open VK Music links in VK Fast",
                        ),
                    )
                    .changed()
                })
                .inner;
            if changed {
                self.link_handler_error = None;
                self.start_link_handler_job(ctx, Some(enabled));
            }
            if busy {
                ui.add(egui::Spinner::new().size(14.0));
            }
        });
        let browser = status
            .as_ref()
            .and_then(|status| status.browser.clone())
            .map_or_else(
                || t("в браузере", "in your browser").to_owned(),
                |browser| format!("{} {browser}", t("в", "in")),
            );
        ui.label(
            RichText::new(format!(
                "{} {browser}.",
                t(
                    "Треки, альбомы, плейлисты и исполнители по ссылкам из других приложений откроются здесь, остальные ссылки —",
                    "Tracks, albums, playlists and artists linked from other apps open here; other links open",
                )
            ))
            .small()
            .color(MUTED),
        );
        if status
            .as_ref()
            .is_some_and(|status| status.enabled && !status.active)
        {
            ui.label(
                RichText::new(t(
                    "Осталось выбрать VK Fast для ссылок HTTP и HTTPS в параметрах Windows.",
                    "Now choose VK Fast for HTTP and HTTPS links in Windows Settings.",
                ))
                .small()
                .color(MUTED),
            );
            if ui
                .button(t("Открыть параметры Windows", "Open Windows Settings"))
                .clicked()
            {
                let _ = open::that("ms-settings:defaultapps?registeredAppUser=VK%20Fast");
            }
        }
        if let Some(error) = &self.link_handler_error {
            ui.colored_label(
                Color32::from_rgb(255, 158, 153),
                match error {
                    RouterError::NoBrowser => t(
                        "Не найден браузер по умолчанию, которому VK Fast передавал бы остальные ссылки.",
                        "No default browser was found to receive the other links.",
                    ),
                    RouterError::UnsupportedPath => t(
                        "Путь к VK Fast содержит символы, которые нельзя записать в настройки системы.",
                        "The path to VK Fast contains characters the system settings cannot store.",
                    ),
                    RouterError::System => t(
                        "Не удалось изменить обработчик ссылок в системе.",
                        "Could not change the system's link handler.",
                    ),
                },
            );
        }
    }

    fn start_storage_job(&mut self, ctx: &egui::Context, action: StorageAction) {
        if self.storage_pending.is_some() {
            return;
        }
        let disk = self.covers.disk();
        let owner = self.session.as_ref().map(|session| session.profile.id);
        let (sender, receiver) = mpsc::sync_channel(1);
        let ctx = self.repaint_target.with_context(ctx);
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-storage".into())
            .spawn(move || {
                if action == StorageAction::ClearCovers {
                    let _ = disk.clear();
                }
                let history_path = owner.and_then(|owner| recent_cache::state_path(owner).ok());
                let info = StorageInfo {
                    covers: disk.usage().ok(),
                    covers_path: disk.root().map(std::path::Path::to_owned),
                    audio_path: if cfg!(test) {
                        None
                    } else {
                        crate::audio_cache::root_path().ok()
                    },
                    history: history_path
                        .as_ref()
                        .filter(|_| !cfg!(test))
                        .map(|path| std::fs::metadata(path).map_or(0, |metadata| metadata.len())),
                    history_path,
                };
                let _ = sender.send(info);
                ctx.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                self.storage_pending = Some(receiver);
            }
            Err(_) => self.storage = StorageInfo::default(),
        }
    }

    /// Settings → Cache: what VK Fast keeps on this computer, where, and how
    /// to clear each part.
    fn storage_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let size = |bytes: Option<u64>| {
            bytes.map_or_else(|| "—".to_owned(), |bytes| format_size(bytes, language))
        };
        let home = (!cfg!(windows))
            .then(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .flatten()
            .filter(|home| home.is_absolute());
        let place = |path: &Option<std::path::PathBuf>| {
            path.as_ref().map_or_else(
                || "—".to_owned(),
                |path| match home.as_ref().and_then(|home| path.strip_prefix(home).ok()) {
                    // Paths inside the home folder read like the shell's ~/…
                    Some(relative) => format!("~/{}", relative.display()),
                    None => path.display().to_string(),
                },
            )
        };
        ui.add(
            egui::Label::new(
                RichText::new(t("Хранилище", "Storage"))
                    .size(18.0)
                    .family(theme::semibold()),
            )
            .selectable(false),
        );
        ui.add_space(8.0);
        let audio = self.cache_stats.as_ref().map_or_else(
            || "—".to_owned(),
            |stats| {
                format!(
                    "{} {} {}",
                    format_size(stats.used_bytes, language),
                    t("из", "of"),
                    format_size(stats.limit_bytes, language)
                )
            },
        );
        let rows = [
            (
                format!(
                    "{} · {}",
                    t("Обложки", "Artwork cache"),
                    size(self.storage.covers)
                ),
                format!(
                    "{} {}",
                    t("Хранятся в", "Stored in"),
                    place(&self.storage.covers_path)
                ),
                t("Очистить", "Clear"),
                self.storage_pending.is_none() && self.storage.covers_path.is_some(),
            ),
            (
                format!("{} · {audio}", t("Аудиокэш", "Audio cache")),
                format!(
                    "{} {}",
                    t("Хранится в", "Stored in"),
                    place(&self.storage.audio_path)
                ),
                t("Очистить", "Clear"),
                self.audio_cache.is_some() && self.cache_pending.is_none(),
            ),
            (
                format!(
                    "{} · {}",
                    t("История прослушиваний", "Play history"),
                    size(self.storage.history)
                ),
                format!(
                    "{} {}",
                    t("Хранится в", "Stored in"),
                    place(&self.storage.history_path)
                ),
                t("Очистить", "Clear"),
                self.session.is_some() && !self.recent.is_empty(),
            ),
        ];
        // The text column ends before the widest button, plus a gap, so long
        // paths wrap instead of running under the buttons.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let button_column = rows
            .iter()
            .map(|(_, _, action, _)| {
                ui.painter()
                    .layout_no_wrap((*action).to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
                    + 54.0
            })
            .fold(0.0_f32, f32::max)
            + 16.0;
        let mut clicked = None;
        egui::Frame::new()
            .fill(theme::SURFACE)
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(44, 53, 67)))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                for (index, (title, description, action, enabled)) in rows.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width((ui.available_width() - button_column).max(120.0));
                            ui.label(RichText::new(title.as_str()).size(15.0));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(description.as_str()).small().color(MUTED),
                                )
                                .wrap(),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let widgets = &mut ui.style_mut().visuals.widgets;
                            widgets.inactive.bg_fill = Color32::from_rgb(35, 41, 50);
                            widgets.inactive.weak_bg_fill = Color32::from_rgb(35, 41, 50);
                            widgets.hovered.bg_fill = Color32::from_rgb(45, 53, 65);
                            widgets.hovered.bg_stroke = egui::Stroke::NONE;
                            if icons::button(ui, Icon::Trash, action, *enabled).clicked() {
                                clicked = Some(index);
                            }
                        });
                    });
                    if index + 1 < rows.len() {
                        ui.add_space(10.0);
                    }
                }
            });
        match clicked {
            Some(0) => self.start_storage_job(ctx, StorageAction::ClearCovers),
            Some(1) => self.start_cache_job(ctx, CacheAction::Clear),
            Some(2) => self.clear_history(ctx),
            _ => {}
        }
    }

    fn clear_history(&mut self, ctx: &egui::Context) {
        self.recent.clear();
        self.save_recent(ctx);
        self.storage.history = Some(0);
    }

    fn poll_cache(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.storage_pending {
            match receiver.try_recv() {
                Ok(info) => {
                    self.storage = info;
                    self.storage_pending = None;
                }
                Err(TryRecvError::Disconnected) => self.storage_pending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(receiver) = &self.cache_pending {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(())),
            };
            if let Some(result) = result {
                self.cache_pending = None;
                match result {
                    Ok(stats) => {
                        self.cache_stats = Some(stats);
                        self.cache_error = false;
                    }
                    Err(()) => self.cache_error = true,
                }
            }
        }
        if let Some(deadline) = self.cache_save_at {
            if Instant::now() >= deadline && self.cache_pending.is_none() {
                self.cache_save_at = None;
                self.cache_save_failed =
                    preferences::save_cache_limit_mb(self.cache_limit_mb).is_err();
                self.start_cache_job(
                    ctx,
                    CacheAction::Apply(u64::from(self.cache_limit_mb) * 1024 * 1024),
                );
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn settings_sidebar(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        if Self::navigation_button(
            ui,
            Icon::ArrowLeft,
            language.text("К музыке", "Back to music"),
            false,
            36.0,
        )
        .clicked()
        {
            self.go_back(ui.ctx());
        }
        ui.add_space(16.0);
        ui.label(
            RichText::new(language.text("НАСТРОЙКИ", "SETTINGS"))
                .small()
                .color(MUTED),
        );
        ui.add_space(6.0);
        for section in [
            SettingsSection::General,
            SettingsSection::Interface,
            SettingsSection::Playback,
            SettingsSection::Cache,
            SettingsSection::About,
        ] {
            ui.push_id(section as u8, |ui| {
                if Self::navigation_button(
                    ui,
                    section.icon(),
                    section.title(language),
                    self.settings_query.trim().is_empty() && self.settings_section == section,
                    36.0,
                )
                .clicked()
                {
                    self.settings_section = section;
                    self.settings_query.clear();
                }
            });
        }
    }

    fn settings_content(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 36.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                theme::inset_heading(ui, |ui| {
                    let title = language.text("Настройки", "Settings");
                    let width = ui
                        .painter()
                        .layout_no_wrap(
                            title.into(),
                            egui::FontId::new(26.0, theme::semibold()),
                            ui.visuals().text_color(),
                        )
                        .size()
                        .x;
                    ui.add_sized(
                        egui::vec2(width.min(ui.available_width()), 36.0),
                        egui::Label::new(RichText::new(title).size(26.0).family(theme::semibold()))
                            .truncate()
                            .selectable(false),
                    )
                });
            },
        );
        ui.add_space(10.0);
        let field_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_max_width(ui.available_width().min(640.0));
        settings_search::input(ui, &mut self.settings_query, language);
        let query = SettingsQuery::new(&self.settings_query);
        let searching = !query.is_empty();
        let scroll_key = self.settings_query.trim().to_lowercase();
        theme::scroll_area(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("settings-content", scroll_key))
                .auto_shrink([false, false]),
            |ui| {
                ui.set_max_width(ui.available_width().min(640.0));
                let mut found = false;
                for section in [
                    SettingsSection::General,
                    SettingsSection::Interface,
                    SettingsSection::Playback,
                    SettingsSection::Cache,
                    SettingsSection::About,
                ] {
                    if (!searching && section != self.settings_section)
                        || (searching && !Self::settings_section_matches(section, &query))
                    {
                        continue;
                    }
                    found = true;
                    ui.add_space(18.0);
                    let title = section.title(language);
                    let color = ui.visuals().text_color();
                    let galley = ui.painter().layout_no_wrap(
                        title.into(),
                        egui::FontId::new(20.0, theme::semibold()),
                        color,
                    );
                    let ink = galley.mesh_bounds;
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), ink.height().max(1.0)),
                        Sense::hover(),
                    );
                    ui.painter().galley(
                        rect.min + egui::vec2(8.0, 0.0) - ink.min.to_vec2(),
                        galley,
                        color,
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, title)
                    });
                    ui.add_space(18.0);
                    ui.push_id(("settings-section", section as u8), |ui| {
                        Frame::new()
                            .fill(SURFACE)
                            .corner_radius(12)
                            .inner_margin(20)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = field_spacing;
                                ui.set_min_width(ui.available_width());
                                self.settings_fields(ui, section, &query);
                            });
                    });
                }
                if !found {
                    ui.add_space(18.0);
                    theme::inset_heading(ui, |ui| {
                        ui.label(
                            RichText::new(
                                language.text("Настройки не найдены.", "No settings found."),
                            )
                            .color(MUTED),
                        );
                    });
                }
            },
        );
    }

    fn settings_section_matches(section: SettingsSection, query: &SettingsQuery) -> bool {
        let fields: &[Setting] = match section {
            SettingsSection::General => &[
                Setting::Language,
                Setting::MinimizeToTray,
                Setting::LinkHandler,
                Setting::DiscordRpc,
                Setting::Logging,
            ],
            SettingsSection::Interface => &[
                Setting::SystemTitlebar,
                Setting::CompactSeek,
                Setting::DoubleClickPlay,
                Setting::SearchOnEnter,
                Setting::MiddleAutoscroll,
                Setting::ExplicitBadge,
                Setting::AnimatedBackdrop,
            ],
            SettingsSection::Playback => &[
                Setting::NormalizeVolume,
                Setting::VolumeKeys,
                Setting::MixAfterQueue,
                Setting::Broadcast,
                Setting::Lyrics,
            ],
            SettingsSection::Cache => &[
                Setting::CacheLimit,
                Setting::CacheClear,
                Setting::CacheStatus,
            ],
            SettingsSection::About => &[
                Setting::About,
                #[cfg(any(windows, test))]
                Setting::Updates,
            ],
        };
        fields.iter().any(|field| query.matches(*field))
    }

    fn settings_fields(
        &mut self,
        ui: &mut egui::Ui,
        section: SettingsSection,
        query: &SettingsQuery,
    ) {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let ctx = ui.ctx().clone();
        match section {
            SettingsSection::General => {
                if query.matches(Setting::Language) {
                    ui.label(t("Язык интерфейса", "Interface language"));
                    let previous = self.language;
                    egui::ComboBox::from_id_salt("settings-language")
                        .width(200.0)
                        .selected_text(match self.language {
                            Language::Russian => "Русский",
                            Language::English => "English",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.language, Language::Russian, "Русский");
                            ui.selectable_value(&mut self.language, Language::English, "English");
                        });
                    if self.language != previous {
                        self.language_save_failed =
                            preferences::save_language(self.language).is_err();
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(t(
                            "Изменения сохраняются автоматически.",
                            "Changes are saved automatically.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.language_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t("Не удалось сохранить язык.", "Could not save the language."),
                        );
                    }
                    if query.matches(Setting::MinimizeToTray)
                        || query.matches(Setting::LinkHandler)
                        || query.matches(Setting::DiscordRpc)
                        || query.matches(Setting::Logging)
                    {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::MinimizeToTray) {
                    let mut enabled = self.minimize_to_tray;
                    if theme::switch(
                        ui,
                        &mut enabled,
                        t(
                            "Сворачивать в системный трей при закрытии",
                            "Minimize to system tray when closing",
                        ),
                    )
                    .changed()
                    {
                        self.set_tray_enabled(&ctx, enabled);
                    }
                    ui.label(RichText::new(t("Музыка продолжит играть. Открыть окно или выйти можно через значок в трее.", "Music keeps playing. Use the tray icon to show the window or exit.")).small().color(MUTED));
                    if self.minimize_to_tray && self.tray.unavailable() {
                        ui.label(
                            RichText::new(t(
                                "Системный трей недоступен — закрытие завершит работу приложения.",
                                "System tray is unavailable; closing will exit the application.",
                            ))
                            .small()
                            .color(MUTED),
                        );
                    }
                    if self.tray_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку трея.",
                                "Could not save the tray preference.",
                            ),
                        );
                    }
                    if query.matches(Setting::LinkHandler)
                        || query.matches(Setting::DiscordRpc)
                        || query.matches(Setting::Logging)
                    {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::LinkHandler) {
                    self.link_handler_settings(ui, &ctx);
                    if query.matches(Setting::DiscordRpc) || query.matches(Setting::Logging) {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::DiscordRpc) {
                    let mut enabled = self.discord_rpc_enabled;
                    if theme::switch(ui, &mut enabled, "Discord Rich Presence").changed() {
                        self.set_discord_rpc_enabled(enabled);
                    }
                    ui.label(
                        RichText::new(t(
                            "Показывать текущий трек в Discord.",
                            "Show the current track in Discord.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.discord_rpc_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку Discord.",
                                "Could not save the Discord preference.",
                            ),
                        );
                    }
                    if query.matches(Setting::Logging) {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::Logging) {
                    if theme::switch(
                        ui,
                        &mut self.logging_enabled,
                        t("Включить логи", "Enable logging"),
                    )
                    .changed()
                    {
                        self.logging_runtime_failed = !configure_logging(self.logging_enabled);
                        self.logging_save_failed = !save_logging_preference(self.logging_enabled);
                    }
                    ui.label(
                        RichText::new(t(
                            "Диагностика ошибок воспроизведения и работы приложения.",
                            "Diagnostics for playback and application issues.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if ui
                        .add_enabled(
                            self.logging_enabled && !self.logging_runtime_failed,
                            egui::Button::new(t("Открыть папку логов", "Open logs folder")),
                        )
                        .clicked()
                        && let Ok(file) = crate::diagnostics::file_path()
                        && let Some(folder) = file.parent()
                    {
                        let _ = open::that(folder);
                    }
                    if self.logging_runtime_failed || self.logging_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось включить логи или сохранить настройку.",
                                "Could not enable logging or save the preference.",
                            ),
                        );
                    }
                }
            }
            SettingsSection::Interface => {
                if query.matches(Setting::SystemTitlebar) {
                    let label = t("Системный заголовок окна", "System title bar");
                    if theme::switch(ui, &mut self.system_titlebar, label).changed() {
                        self.titlebar_save_failed = !save_titlebar_choice(self.system_titlebar);
                        self.window_reopen = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    ui.label(
                        RichText::new(t(
                            "Выключено — кнопки окна встроены в верхнюю панель VK Fast.",
                            "Off: window controls are integrated into the VK Fast toolbar.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.titlebar_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить оформление окна.",
                                "Could not save the window decoration preference.",
                            ),
                        );
                    }
                    if query.matches(Setting::CompactSeek)
                        || query.matches(Setting::DoubleClickPlay)
                        || query.matches(Setting::SearchOnEnter)
                        || query.matches(Setting::MiddleAutoscroll)
                        || query.matches(Setting::ExplicitBadge)
                        || query.matches(Setting::AnimatedBackdrop)
                    {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::CompactSeek) {
                    let label = t("Короткая полоска по центру", "Centered compact seek bar");
                    if theme::switch(ui, &mut self.compact_seek_bar, label).changed() {
                        self.seek_layout_save_failed = !save_seek_layout(self.compact_seek_bar);
                    }
                    ui.label(
                        RichText::new(t(
                            "Выключено — полоска занимает всю ширину плеера.",
                            "Off: the seek bar spans the full player width.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.seek_layout_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить вид полоски.",
                                "Could not save the seek bar layout.",
                            ),
                        );
                    }
                    if query.matches(Setting::DoubleClickPlay)
                        || query.matches(Setting::SearchOnEnter)
                        || query.matches(Setting::MiddleAutoscroll)
                        || query.matches(Setting::ExplicitBadge)
                        || query.matches(Setting::AnimatedBackdrop)
                    {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::DoubleClickPlay) {
                    let label = t(
                        "Запускать трек двойным щелчком",
                        "Play tracks with a double click",
                    );
                    if theme::switch(ui, &mut self.double_click_to_play, label).changed() {
                        self.click_behavior_save_failed =
                            !save_click_behavior(self.double_click_to_play);
                    }
                    ui.label(RichText::new(t(
                        "Выключено — один клик по строке. Кнопка Play вместо номера всегда запускает трек одним кликом.",
                        "Off: click a row once. The Play button in place of its number always works with one click.",
                    )).small().color(MUTED));
                    if self.click_behavior_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить способ запуска треков.",
                                "Could not save the track click preference.",
                            ),
                        );
                    }
                    if query.matches(Setting::SearchOnEnter) {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::SearchOnEnter) {
                    let label = t("Поиск при нажатии Enter", "Search on Enter");
                    if theme::switch(ui, &mut self.search_on_enter, label).changed() {
                        self.search_on_enter_save_failed = !cfg!(test)
                            && preferences::save_search_on_enter(self.search_on_enter).is_err();
                    }
                    ui.label(
                        RichText::new(t(
                            "Выключено — поиск начинается сразу при вводе текста.",
                            "Off: search starts as you type.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.search_on_enter_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку поиска.",
                                "Could not save the search preference.",
                            ),
                        );
                    }
                    if query.matches(Setting::MiddleAutoscroll)
                        || query.matches(Setting::ExplicitBadge)
                        || query.matches(Setting::AnimatedBackdrop)
                    {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::MiddleAutoscroll) {
                    let label = t(
                        "Автопрокрутка по щелчку колёсиком",
                        "Autoscroll on middle click",
                    );
                    if theme::switch(ui, &mut self.middle_autoscroll, label).changed() {
                        self.middle_autoscroll_save_failed = !cfg!(test)
                            && preferences::save_middle_autoscroll(self.middle_autoscroll).is_err();
                    }
                    ui.label(
                        RichText::new(t(
                            "Щёлкните колёсиком на странице и ведите мышь вверх или вниз. Любой щелчок останавливает прокрутку.",
                            "Middle-click a page and move the mouse up or down. Any click stops scrolling.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.middle_autoscroll_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку.",
                                "Could not save the setting.",
                            ),
                        );
                    }
                    if query.matches(Setting::ExplicitBadge) {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::ExplicitBadge) {
                    let label = t("Значок Explicit у треков", "Explicit badge on tracks");
                    if theme::switch(ui, &mut self.explicit_badge, label).changed() {
                        self.explicit_badge_save_failed = !cfg!(test)
                            && preferences::save_explicit_badge(self.explicit_badge).is_err();
                    }
                    ui.label(
                        RichText::new(t(
                            "Буква E в квадрате рядом с названием трека, который VK отметил как содержащий ненормативную лексику.",
                            "A boxed E next to the title of tracks VK marks as explicit.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.explicit_badge_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку.",
                                "Could not save the setting.",
                            ),
                        );
                    }
                    if query.matches(Setting::AnimatedBackdrop) {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::AnimatedBackdrop) {
                    let label = t(
                        "Анимированный фон в полноэкранном режиме",
                        "Animated fullscreen background",
                    );
                    if theme::switch(ui, &mut self.animated_backdrop, label).changed() {
                        self.animated_backdrop_save_failed = !cfg!(test)
                            && preferences::save_animated_backdrop(self.animated_backdrop).is_err();
                    }
                    ui.label(
                        RichText::new(t(
                            "Размытая обложка трека медленно переливается за текстом. Выключено — спокойный градиент в её цвете.",
                            "The track's blurred cover slowly drifts behind the lyrics. Off: a still gradient in its colour.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.animated_backdrop_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку.",
                                "Could not save the setting.",
                            ),
                        );
                    }
                }
            }
            SettingsSection::Playback => {
                if query.matches(Setting::NormalizeVolume) {
                    let mut normalize = self.player.normalize_volume();
                    if theme::switch(
                        ui,
                        &mut normalize,
                        t("Нормализация громкости", "Normalize volume"),
                    )
                    .changed()
                    {
                        self.player.set_normalize_volume(normalize);
                        self.normalization_save_failed = !save_normalization(normalize);
                    }
                    ui.label(
                        RichText::new(t(
                            "Выравнивает воспринимаемую громкость разных треков.",
                            "Keeps different tracks at a consistent perceived volume.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.normalization_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку нормализации.",
                                "Could not save the normalization setting.",
                            ),
                        );
                    }
                }
                if query.matches(Setting::VolumeKeys) {
                    if query.matches(Setting::NormalizeVolume) {
                        ui.add_space(16.0);
                    }
                    let support = crate::volume_keys::support();
                    let available = support == crate::volume_keys::Support::Available;
                    let label = t(
                        "Изменение громкости с помощью Ctrl + мультимедийных клавиш",
                        "Adjust the volume with Ctrl + media keys",
                    );
                    let mut enabled = self.volume_keys_enabled && available;
                    if ui
                        .add_enabled_ui(available, |ui| theme::switch(ui, &mut enabled, label))
                        .inner
                        .changed()
                    {
                        self.volume_keys_enabled = enabled;
                        self.volume_keys_save_failed =
                            !cfg!(test) && preferences::save_volume_keys(enabled).is_err();
                    }
                    let hint = match support {
                        crate::volume_keys::Support::Available => t(
                            "Зажатие Ctrl при использовании мультимедийных клавиш будет менять громкость треков в VK Fast.",
                            "Holding down Ctrl while using media keys will change the volume of tracks in VK Fast.",
                        ),
                        crate::volume_keys::Support::Wayland => t(
                            "Недоступно в сеансе Wayland: он не позволяет приложениям перехватывать клавиши глобально.",
                            "Unavailable in a Wayland session: it does not let applications take keys globally.",
                        ),
                        crate::volume_keys::Support::Unsupported => {
                            t("Недоступно в этой системе.", "Unavailable on this system.")
                        }
                    };
                    ui.label(RichText::new(hint).small().color(MUTED));
                    if self.volume_keys_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку.",
                                "Could not save the setting.",
                            ),
                        );
                    } else if let Some(error) = self.volume_keys.error().filter(|_| enabled) {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            match error {
                                crate::volume_keys::Error::Taken => t(
                                    "Эти сочетания уже заняты другой программой.",
                                    "Another program already uses these combinations.",
                                ),
                                crate::volume_keys::Error::Failed => t(
                                    "Не удалось занять клавиши громкости.",
                                    "Could not take the volume keys.",
                                ),
                            },
                        );
                    }
                }
                if query.matches(Setting::MixAfterQueue) {
                    if query.matches(Setting::NormalizeVolume) || query.matches(Setting::VolumeKeys)
                    {
                        ui.add_space(16.0);
                    }
                    let label = t(
                        "Включать микс, когда очередь закончится",
                        "Start a mix when the queue ends",
                    );
                    if theme::switch(ui, &mut self.mix_after_queue, label).changed() {
                        self.mix_after_queue_save_failed = !cfg!(test)
                            && preferences::save_mix_after_queue(self.mix_after_queue).is_err();
                    }
                    ui.label(
                        RichText::new(t(
                            "Музыка продолжится миксом, похожим на последний трек.",
                            "Music continues with a mix based on the last track.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.mix_after_queue_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку.",
                                "Could not save the setting.",
                            ),
                        );
                    }
                }
                if query.matches(Setting::Broadcast) {
                    if query.matches(Setting::NormalizeVolume)
                        || query.matches(Setting::VolumeKeys)
                        || query.matches(Setting::MixAfterQueue)
                    {
                        ui.add_space(16.0);
                    }
                    let label = t(
                        "Транслировать аудиозаписи на мою страницу",
                        "Show what I listen to on my page",
                    );
                    if theme::switch(ui, &mut self.broadcast, label).changed() {
                        self.broadcast_save_failed =
                            !cfg!(test) && preferences::save_broadcast(self.broadcast).is_err();
                        let key = self
                            .broadcast
                            .then(|| self.playing.as_ref().map(Track::key))
                            .flatten();
                        // On: show the playing track now. Off: clear the status.
                        if !self.broadcast || key.is_some() {
                            self.send_broadcast(key);
                        }
                    }
                    ui.label(
                        RichText::new(t(
                            "Играющий трек показывается в статусе вашей страницы VK.",
                            "The playing track is shown in your VK page status.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                    if self.broadcast_save_failed {
                        ui.colored_label(
                            Color32::from_rgb(255, 158, 153),
                            t(
                                "Не удалось сохранить настройку.",
                                "Could not save the setting.",
                            ),
                        );
                    }
                }
                if query.matches(Setting::Lyrics) {
                    if query.matches(Setting::NormalizeVolume)
                        || query.matches(Setting::VolumeKeys)
                        || query.matches(Setting::MixAfterQueue)
                        || query.matches(Setting::Broadcast)
                    {
                        ui.add_space(18.0);
                    }
                    self.lyrics_settings(ui);
                }
            }
            SettingsSection::Cache => {
                if query.matches(Setting::CacheStatus) {
                    ui.label(t("Загруженное аудио сохраняется на диске для перемотки и повторного воспроизведения.",
                        "Loaded audio is kept on disk for seeking and playing again."));
                    ui.add_space(16.0);
                }
                if query.matches(Setting::CacheLimit) {
                    ui.label(t("Лимит аудиокэша", "Audio cache limit"));
                    let mut gb = self.cache_limit_mb as f32 / 1024.0;
                    ui.horizontal(|ui| {
                        let response = ui.add_enabled(
                            self.audio_cache.is_some(),
                            egui::Slider::new(
                                &mut gb,
                                0.0..=preferences::MAX_CACHE_MB as f32 / 1024.0,
                            )
                            .step_by(0.25)
                            .show_value(false),
                        );
                        ui.label(format!("{gb:.2} {}", t("ГБ", "GB")));
                        if response.changed() {
                            self.cache_limit_mb = (gb * 1024.0).round() as u32;
                            self.cache_save_at = Some(Instant::now() + Duration::from_millis(500));
                        }
                    });
                    ui.label(RichText::new(if self.cache_limit_mb==0 { t("Кэш выключен","Cache is off") }
                        else { t("При достижении лимита старые записи удаляются автоматически.","Older entries are removed automatically when the limit is reached.") }).small().color(MUTED));
                    if query.matches(Setting::CacheStatus) || query.matches(Setting::CacheClear) {
                        ui.add_space(16.0);
                    }
                }
                if query.matches(Setting::CacheStatus) || query.matches(Setting::CacheClear) {
                    self.storage_panel(ui, &ctx);
                    ui.add_space(8.0);
                }
                if self.audio_cache.is_none() || self.cache_error || self.cache_save_failed {
                    ui.colored_label(
                        Color32::from_rgb(255, 158, 153),
                        t(
                            "Не удалось обновить кэш или сохранить настройки.",
                            "Could not update the cache or save settings.",
                        ),
                    );
                }
            }
            SettingsSection::About => {
                if query.matches(Setting::About) {
                    ui.horizontal(|ui| {
                        crate::app_icon::show(ui, 56.0);
                        ui.label(
                            RichText::new("VK")
                                .size(28.0)
                                .family(theme::semibold())
                                .color(ACCENT),
                        );
                        ui.label(RichText::new("FAST").size(26.0).family(theme::semibold()));
                    });
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!(
                            "{} {}",
                            t("Версия", "Version"),
                            env!("CARGO_PKG_VERSION")
                        ))
                        .color(MUTED),
                    );
                    ui.label(t("Музыкальный плеер для VK", "Music player for VK"));
                    ui.add_space(12.0);
                    ui.label(RichText::new("GPL-3.0-only").small().color(MUTED));
                }
                #[cfg(any(windows, test))]
                if query.matches(Setting::Updates) {
                    self.updater.settings(ui, language, &self.repaint_target);
                }
            }
        }
    }

    pub fn panel_widths(&self) -> [f32; 2] {
        self.panel_widths
    }

    pub fn restore_panel_widths(&mut self, widths: [f32; 2]) {
        self.panel_widths = [
            widths[0].clamp(LEFT_PANEL_WIDTHS.0, LEFT_PANEL_WIDTHS.1),
            widths[1].clamp(RIGHT_PANEL_WIDTHS.0, RIGHT_PANEL_WIDTHS.1),
        ];
    }

    /// Start each frame from the chosen width, unless the user is dragging
    /// the edge right now: egui then owns the size until the drag ends.
    pub(super) fn seed_panel_width(&self, ctx: &egui::Context, index: usize, id: &str) {
        let id = egui::Id::new(id);
        if ctx
            .read_response(id.with("__resize"))
            .is_some_and(|response| response.dragged())
        {
            return;
        }
        let state = egui::containers::panel::PanelState {
            outer_rect: egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.panel_widths[index], 1.0),
            ),
        };
        ctx.data_mut(|data| data.insert_persisted(id, state));
    }

    /// Keep a width only from the user's own drag: a window that is briefly
    /// too narrow squeezes the panel without forgetting the chosen width.
    pub(super) fn remember_panel_width(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        id: &str,
        width: f32,
    ) {
        let dragged = ctx
            .read_response(egui::Id::new(id).with("__resize"))
            .is_some_and(|response| response.dragged() || response.drag_stopped());
        if dragged && width.is_finite() {
            let (min, max) = [LEFT_PANEL_WIDTHS, RIGHT_PANEL_WIDTHS][index];
            self.panel_widths[index] = width.clamp(min, max);
        }
    }

    /// The right lyrics/queue panel: resizable from its left edge, keeping
    /// the central page at least `MIN_CONTENT_WIDTH` wide.
    pub(super) fn right_side_panel(&self, ui: &egui::Ui) -> egui::Panel {
        self.seed_panel_width(ui.ctx(), 1, RIGHT_PANEL_ID);
        let max = (ui.available_width() - MIN_CONTENT_WIDTH)
            .clamp(RIGHT_PANEL_WIDTHS.0, RIGHT_PANEL_WIDTHS.1);
        egui::Panel::right(RIGHT_PANEL_ID)
            .resizable(true)
            // Neither a drag nor a double click on the edge closes the panel;
            // its own close button does.
            .drag_to_open(false)
            .default_size(self.panel_widths[1])
            .size_range(RIGHT_PANEL_WIDTHS.0..=max)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(12)
                    .outer_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 6,
                        bottom: 6,
                    })
                    .corner_radius(14),
            )
    }

    fn render(&mut self, ui: &mut egui::Ui) {
        theme::set_explicit_badges(ui.ctx(), self.explicit_badge);
        self.render_frame(ui);
        // Rows drawn this frame that still miss their album's name.
        self.request_album_titles(ui.ctx());
    }

    fn render_frame(&mut self, ui: &mut egui::Ui) {
        self.scrolling.apply(ui.ctx());
        if ui.ctx().current_pass_index() == 0 {
            let enabled = self.middle_autoscroll && !self.lyrics.fullscreen;
            self.autoscroll.apply(ui.ctx(), enabled, self.page_rect);
        }
        self.poll_lyrics(ui.ctx());
        if self.lyrics.fullscreen
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.set_lyrics_fullscreen(ui.ctx(), false);
        }
        if !self.system_titlebar && !self.lyrics.fullscreen {
            let window = ui.ctx().viewport_rect();
            let caption = egui::Rect::from_min_max(
                window.min,
                egui::pos2(window.right(), (window.top() + 50.0).min(window.bottom())),
            );
            // Register the background first: search, navigation, account and
            // caption buttons keep their own pointer interactions above it.
            titlebar::drag_region(ui, caption, ui.id().with("window-caption-drag"));
        }
        if ui.ctx().current_pass_index() == 0
            && self.playlist_create_form.is_none()
            && !self.playlist_controls.has_modal()
            && !self.audio_upload.has_modal()
        {
            if ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Extra1)) {
                if self.lyrics.fullscreen {
                    self.set_lyrics_fullscreen(ui.ctx(), false);
                } else {
                    self.go_back(ui.ctx());
                }
            } else if !self.lyrics.fullscreen
                && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Extra2))
            {
                self.go_forward(ui.ctx());
            }
        }
        egui::Panel::bottom("player")
            .show_separator_line(true)
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(egui::Margin::symmetric(16, PLAYER_PADDING_Y)),
            )
            .show(ui, |ui| self.player_bar(ui));
        if self.lyrics.fullscreen {
            egui::CentralPanel::default()
                .frame(Frame::NONE)
                .show(ui, |ui| self.lyrics_fullscreen(ui));
            self.render_overlays_and_shortcuts(ui);
            return;
        }
        let left_max = (ui.available_width() * 0.4).clamp(LEFT_PANEL_WIDTHS.0, LEFT_PANEL_WIDTHS.1);
        self.seed_panel_width(ui.ctx(), 0, LEFT_PANEL_ID);
        let left = egui::Panel::left(LEFT_PANEL_ID)
            .resizable(true)
            .default_size(self.panel_widths[0])
            .size_range(LEFT_PANEL_WIDTHS.0..=left_max)
            .show_separator_line(false)
            .frame(Frame::new().fill(SURFACE).inner_margin(egui::Margin {
                left: 18,
                right: 18,
                top: 18,
                bottom: if self.settings_open { 18 } else { 9 },
            }))
            .show(ui, |ui| {
                if self.settings_open {
                    self.settings_sidebar(ui);
                } else {
                    self.sidebar(ui);
                }
            });
        self.remember_panel_width(ui.ctx(), 0, LEFT_PANEL_ID, left.response.rect.width());
        egui::Panel::top("header")
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| self.toolbar(ui));
        self.right_panel(ui);
        let page = egui::CentralPanel::default()
            .frame(Frame::new().fill(BACKGROUND).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: if !self.settings_open && matches!(self.page, MusicPage::Search(_)) {
                    8
                } else {
                    16
                },
                bottom: if self.settings_open { 24 } else { 0 },
            }))
            .show(ui, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                if self.settings_open {
                    self.settings_content(ui);
                } else if self.page == MusicPage::Home {
                    self.home_content(ui);
                } else if matches!(self.page, MusicPage::Search(_)) {
                    self.search_content(ui);
                } else if self.page.detail_key().is_some() {
                    self.detail_content(ui);
                } else {
                    self.content(ui);
                }
            });
        self.page_rect = page.response.rect;
        let _ = self.qr_window(ui.ctx());
        self.mix_settings_window(ui.ctx());
        self.playlist_create_window(ui.ctx());
        self.playlist_manage_window(ui.ctx());
        self.audio_upload_window(ui.ctx());
        if !self.system_titlebar
            && !self.qr_open
            && !self.mix_settings.open
            && !self.audio_upload.has_modal()
            && self.playlist_create_form.is_none()
            && !self.playlist_controls.has_modal()
            && !egui::Popup::is_any_open(ui.ctx())
        {
            titlebar::resize(ui.ctx());
        }
        self.render_overlays_and_shortcuts(ui);
    }

    fn render_overlays_and_shortcuts(&mut self, ui: &mut egui::Ui) {
        if self.audio_upload.has_modal() {
            return;
        }
        #[cfg(any(windows, test))]
        {
            self.updater
                .dialog(ui.ctx(), self.language, &self.repaint_target);
            if self.updater.open {
                return;
            }
        }
        if !self.render_track_unavailable_notice(ui.ctx()) && !self.playlist_mix_notice(ui.ctx()) {
            let menu_error = self.track_menu_error.is_some();
            if let Some(error) = self.track_menu_error.as_ref().or(self.like_error.as_ref()) {
                let message = if menu_error && *error == WebAudioError::Rejected {
                    self.language
                        .text(
                            "VK не разрешил выполнить это действие.",
                            "VK did not allow this action.",
                        )
                        .into()
                } else {
                    error.localized(self.language)
                };
                let mut dismiss = false;
                // Errors hide by themselves too, given longer to read than
                // the short notices.
                let shown = (self.track_menu_error.clone(), self.like_error.clone());
                if self.error_shown.as_ref() != Some(&shown) {
                    self.error_shown = Some(shown);
                    self.error_closing = Some(Instant::now() + Duration::from_secs(6));
                }
                let opacity = self
                    .error_closing
                    .map_or(1.0, |until| notice_fade::opacity(ui.ctx(), until));
                egui::Area::new(egui::Id::new("library-action-error"))
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -112.0))
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        ui.multiply_opacity(opacity);
                        Frame::popup(ui.style()).inner_margin(12).show(ui, |ui| {
                            ui.set_max_width(340.0);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(if menu_error {
                                        self.language.text(
                                            "Не удалось выполнить действие",
                                            "Could not complete the action",
                                        )
                                    } else {
                                        self.language.text(
                                            "Не удалось обновить мою музыку",
                                            "Could not update My music",
                                        )
                                    })
                                    .color(Color32::WHITE),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::Vec2::splat(22.0),
                                        Sense::hover(),
                                    );
                                    dismiss = icons::panel_button(
                                        ui,
                                        rect,
                                        Icon::Close,
                                        self.language.text("Закрыть", "Close"),
                                    )
                                    .clicked();
                                });
                            });
                            ui.label(RichText::new(message).color(MUTED));
                        });
                    });
                if dismiss && let Some(until) = &mut self.error_closing {
                    notice_fade::dismiss(ui.ctx(), until);
                }
                if self
                    .error_closing
                    .is_some_and(|until| Instant::now() >= until)
                {
                    self.like_error = None;
                    self.track_menu_error = None;
                    self.error_closing = None;
                    self.error_shown = None;
                }
            } else if let Some((message, until)) = &self.track_menu_notice {
                self.error_closing = None;
                self.error_shown = None;
                let until = *until;
                if Instant::now() >= until {
                    self.track_menu_notice = None;
                    self.library_undo = None;
                } else {
                    // The undo belongs to this notice only, not to a newer one.
                    let playlist_undo = self
                        .playlist_removal
                        .pending
                        .as_ref()
                        .is_some_and(|removal| removal.until == until);
                    let undo = if playlist_undo {
                        Some(true)
                    } else {
                        self.library_undo
                            .as_ref()
                            .filter(|undo| undo.until == until)
                            .map(|_| {
                                self.like_request.is_none() && self.playlist_add_request.is_none()
                            })
                    };
                    let opacity = notice_fade::opacity(ui.ctx(), until);
                    let mut undone = false;
                    egui::Area::new(egui::Id::new("track-menu-notice"))
                        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -112.0))
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            ui.multiply_opacity(opacity);
                            Frame::popup(ui.style()).inner_margin(12).show(ui, |ui| {
                                ui.set_max_width(360.0);
                                let Some(enabled) = undo else {
                                    // Text alone: no row sized for a button.
                                    ui.label(message);
                                    return;
                                };
                                ui.horizontal(|ui| {
                                    ui.label(message);
                                    {
                                        let response = ui.add_enabled(
                                            enabled,
                                            egui::Button::new(self.language.text("Отмена", "Undo")),
                                        );
                                        undone = response.clicked();
                                        release_pointer_focus(&response);
                                    }
                                });
                            });
                        });
                    if undone {
                        if playlist_undo {
                            self.undo_playlist_removal();
                        } else {
                            self.undo_library_removal(ui.ctx());
                        }
                        if let Some((_, until)) = &mut self.track_menu_notice {
                            notice_fade::dismiss(ui.ctx(), until);
                        }
                    }
                }
            } else {
                self.error_closing = None;
                self.error_shown = None;
            }
        }
        if ui.ctx().current_pass_index() == 0
            && (!self.settings_open || self.lyrics.fullscreen)
            && !self.qr_open
            && !self.mix_settings.open
            && self.playlist_create_form.is_none()
            && !self.playlist_controls.has_modal()
            && !egui::Popup::is_any_open(ui.ctx())
            && !ui.ctx().text_edit_focused()
            && ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary))
            && let Some(id) = ui.ctx().memory(|memory| memory.focused())
        {
            ui.ctx().memory_mut(|memory| memory.surrender_focus(id));
            let frame = ui.ctx().cumulative_frame_nr();
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("player-pointer-release-frame"), frame)
            });
        }
        self.handle_shortcuts(ui.ctx());
    }
}

impl eframe::App for MusicApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BACKGROUND.to_normalized_gamma_f32()
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.background_tick(ctx, Some(frame));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
        if ui.ctx().cumulative_frame_nr() == 0 {
            // An activation queued during startup is handled after presenting
            // this first frame, preserving the native no-flash reveal order.
            ui.ctx().request_repaint();
        }
        #[cfg(feature = "screenshot")]
        self.capture(ui);
    }
}

impl MusicApp {
    pub(super) fn background_tick(&mut self, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        #[cfg(any(windows, test))]
        {
            if self.updater.tick(&self.repaint_target) {
                self.request_quit(ctx);
            }
            if let Some(delay) = self.updater.next_check_in() {
                ctx.request_repaint_after(delay);
            }
        }
        self.save_session_cookies();
        crate::ui_fonts::refresh(ctx);
        self.poll_volume_keys();
        self.poll_system_tray(ctx, frame);
        self.poll_instance_activation(ctx);
        self.poll_links(ctx);
        self.poll_link_track(ctx);
        self.poll_link_handler();
        self.poll_system_media(ctx, frame);
        self.poll_lyrics(ctx);
        self.poll(ctx);
        // The saved Home and sidebar are read right after sign-in, before
        // VK answers anything.
        if self.session.is_some() {
            self.start_screen_cache(ctx);
        }
        self.poll_screen_cache();
        self.poll_library_tail();
        self.poll_home(ctx);
        self.poll_home_catalog();
        self.poll_playlists(ctx);
        self.poll_library_artists(ctx);
        if !self.playlists_requested && self.playlist_ready() {
            self.load_playlists(ctx, false);
        }
        self.poll_search(ctx);
        self.drive_search(ctx);
        self.poll_detail(ctx);
        self.poll_metadata(ctx);
        self.hydrate_library_dates(ctx);
        self.poll_like(ctx);
        self.poll_dislike(ctx);
        self.poll_playlist_add(ctx);
        self.poll_playlist_removal(ctx);
        self.poll_reorder(ctx);
        self.poll_album_titles(ctx);
        self.request_release_titles(ctx);
        self.poll_playlist_create(ctx);
        self.poll_audio_upload(ctx);
        self.poll_playlist_actions(ctx);
        if self.library_refresh_due && self.pending.is_none() {
            self.library_refresh_due = false;
            self.load_page(ctx, true);
        }
        if (self.resume_position.is_none() || self.lyrics.fullscreen)
            && let Some(track) = self.playing.as_ref().filter(|track| {
                track.artists.is_empty()
                    || track.subtitle.is_none()
                    || track.library_liked.is_none()
                    || self.lyrics.fullscreen
            })
        {
            self.hydrate_metadata(ctx, vec![track.key()]);
        }
        self.poll_mix(ctx);
        if self.mix_active
            && self.mix_has_more
            && !self.unavailable_playback.halted()
            && !self.unavailable_playback.active()
            && self.mix_request.is_none()
            && self.mix_error.is_none()
        {
            let remaining = self
                .playing
                .as_ref()
                .map(|playing| self.play_order.remaining_after(playing.key()));
            if remaining.is_some_and(|remaining| remaining <= 3) {
                self.load_mix(ctx, false);
            }
        }
        if self.page == MusicPage::Home
            && !self.home_requested
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.music.is_some())
            && (self.library.offset > 0 || self.library.exhausted)
        {
            self.load_home(ctx);
        }
        self.poll_library_save(ctx);
        self.poll_recent_save(ctx);
        self.poll_playback(ctx);
        let failure = match self.player.snapshot().state {
            PlaybackState::Failed(error) => Some(error),
            _ => None,
        };
        if failure != self.logged_playback_failure {
            if let Some(error) = &failure {
                crate::diagnostics::event("playback", &format!("engine_failed: {error:?}"));
            }
            self.logged_playback_failure = failure;
        }
        self.poll_qr(ctx);
        self.poll_mix_settings();
        self.poll_cache(ctx);
        self.publish_system_media();
        self.publish_discord_rpc();
        if self.qr_open {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(deadline) = self.volume_save_at {
            if Instant::now() >= deadline {
                self.volume_save_at = None;
                self.volume_save_failed = !save_volume_value(self.player.snapshot().volume);
            } else {
                ctx.request_repaint_after(deadline.saturating_duration_since(Instant::now()));
            }
        }
        if self.restoring {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let snapshot = self.player.snapshot();
        if self.playback_pending.is_some() || snapshot.state == PlaybackState::Loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if snapshot.state == PlaybackState::Playing {
            // Only when something visibly changes or the track ends: paused
            // music needs no wakes at all.
            let duration = self
                .playing
                .as_ref()
                .map_or(0.0, |track| f64::from(track.duration));
            ctx.request_repaint_after(playing_wake(snapshot.position, duration, self.seek_pixels));
        }
    }
}

/// The next wake while music plays: when the elapsed time ticks to the next
/// second, the seek bar moves by a physical pixel (at most twice a second),
/// or the track ends (its end is noticed by polling). Between 50 ms and 1 s.
fn playing_wake(position: f64, duration: f64, seek_pixels: f32) -> std::time::Duration {
    let to_second = 1.0 - position.rem_euclid(1.0) + 0.005;
    let mut wait = to_second;
    if duration > 0.0 {
        if seek_pixels >= 1.0 {
            wait = wait.min((duration / f64::from(seek_pixels)).max(0.5));
        }
        wait = wait.min((duration - position).max(0.0) + 0.02);
    } else {
        // Without a known length, the end is checked often.
        wait = wait.min(0.25);
    }
    std::time::Duration::from_secs_f64(wait.clamp(0.05, 1.0))
}

/// Background threads that only read from VK or the compositor, safe to
/// leave behind when the application exits.
fn read_only_worker(name: Option<&str>) -> bool {
    matches!(
        name,
        Some(
            "vk-api"
                | "vk-home"
                | "vk-home-catalog"
                | "vk-home-playlist"
                | "vk-search"
                | "vk-artist-album"
                | "vk-track-metadata"
                | "vk-track-prefetch"
                | "vk-resolve-audio"
                | "vk-lyrics"
                | "vk-mix"
                | "vk-mix-settings"
                | "vk-playlists"
                | "vk-owner-playlists"
                | "vk-library-artists"
                | "vk-link-track"
                | "vk-discord-links"
                | "vk-playlist-play"
                | "vk-qr-login"
                | "vk-window-focus"
                | "vk-broadcast"
                | "vk-screen-cache"
                | "vk-library-tail"
                | "vk-album-titles"
                | "vk-album-names"
        )
    )
}

impl Drop for MusicApp {
    fn drop(&mut self) {
        self.tray.detach();
        self.system_media.detach();
        self.discord_rpc.detach();
        // Capture committed media time before Stop resets the engine snapshot.
        let resume = self.playback_resume_snapshot();
        self.cancel_qr();
        self.pending = None;
        self.home_request = None;
        self.detail_request = None;
        self.playlist_request = None;
        self.metadata_request = None;
        self.like_request = None;
        self.playlist_add_request = None;
        self.playlist_create_form = None;
        self.playlist_create_request = None;
        self.playlist_controls = playlist_actions::State::default();
        self.library_browser = library_app::State::default();
        self.audio_upload = audio_upload_app::State::default();
        self.search.cancel_requests();
        self.mix_request = None;
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.playback_pending = None;
        self.player.stop();
        // A removal whose Undo was still offered is sent; the worker below
        // is awaited like other account changes.
        self.send_playlist_removal();
        self.save_album_names(true);
        self.save_session_cookies();
        if self.volume_save_at.is_some() {
            let _ = save_volume_value(self.player.snapshot().volume);
        }
        if self.cache_save_at.is_some() {
            let _ = preferences::save_cache_limit_mb(self.cache_limit_mb);
        }
        for worker in self.workers.drain(..) {
            // Canceled reads may still wait for VK's answer (up to the HTTP
            // timeout); the process exits without them. Saves and account
            // changes are still awaited.
            if worker.is_finished() || !read_only_worker(worker.thread().name()) {
                let _ = worker.join();
            }
        }
        let recent_failed = self
            .recent_save_pending
            .as_ref()
            .is_some_and(|receiver| receiver.try_recv().is_ok_and(|saved| !saved));
        if recent_failed && let Some((owner, tracks)) = self.recent_save_current.take() {
            self.recent_save_queue.entry(owner).or_insert(tracks);
        }
        for (owner, tracks) in self.recent_save_queue.drain() {
            let _ = write_recent_snapshot(owner, &tracks);
        }
        let failed = self
            .library_save_pending
            .as_ref()
            .is_some_and(|receiver| receiver.try_recv().is_ok_and(|saved| !saved));
        if failed && let Some((owner, tracks)) = self.library_save_current.take() {
            self.library_save_queue.entry(owner).or_insert(tracks);
        }
        for (owner, tracks) in self.library_save_queue.drain() {
            let _ = write_library_snapshot(owner, &tracks);
        }
        if let Some((owner, snapshot)) = resume {
            let _ = write_playback_resume(owner, &snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_wakes_only_for_visible_changes_and_the_track_end() {
        let secs = |wait: std::time::Duration| wait.as_secs_f64();
        // A 3-minute track on a wide bar: the next second, at most twice a
        // second for the bar, never the old ten times a second.
        let wait = secs(playing_wake(10.2, 180.0, 750.0));
        assert!((0.5..=0.81).contains(&wait), "{wait}");
        assert!(secs(playing_wake(10.0, 180.0, 750.0)) >= 0.5);
        // A long track on a narrow bar still updates the elapsed seconds.
        assert!(secs(playing_wake(10.0, 3600.0, 100.0)) <= 1.01);
        // The end is caught right after it comes.
        let wait = secs(playing_wake(179.9, 180.0, 750.0));
        assert!((0.05..=0.13).contains(&wait), "{wait}");
        assert!((secs(playing_wake(181.0, 180.0, 750.0)) - 0.05).abs() < 1e-9);
        // An unknown length is checked often.
        assert!(secs(playing_wake(5.5, 0.0, 750.0)) <= 0.25);
    }
    use crate::web_audio::MixPage;

    struct SeekIntentHarness {
        context: egui::Context,
        preview: Option<f64>,
        position: f64,
        displayed: f64,
        commits: Vec<f64>,
        tick: u32,
        focused: bool,
        enabled: bool,
    }

    impl SeekIntentHarness {
        fn new() -> Self {
            Self {
                context: egui::Context::default(),
                preview: None,
                position: 10.0,
                displayed: 10.0,
                commits: Vec::new(),
                tick: 0,
                focused: true,
                enabled: true,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::Response {
            self.tick += 1;
            self.displayed = self.preview.unwrap_or(self.position);
            let context = self.context.clone();
            let mut response = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(320.0, 100.0),
                    )),
                    time: Some(f64::from(self.tick) * 0.1),
                    focused: self.focused,
                    events,
                    ..Default::default()
                },
                |ui| {
                    let result = ui
                        .add_enabled_ui(self.enabled, |ui| {
                            seek_bar::show(
                                ui,
                                &mut self.displayed,
                                seek_bar::Progress {
                                    duration: 100.0,
                                    width: 240.0,
                                    label: "Synthetic seek",
                                },
                            )
                        })
                        .inner;
                    if let Some(target) =
                        seek_commit(&mut self.preview, self.displayed, 100.0, &result)
                    {
                        self.position = target;
                        self.commits.push(target);
                    }
                    response = Some(result);
                },
            );
            output.textures_delta.clear();
            response.unwrap()
        }

        fn warm(&mut self) -> egui::Response {
            self.frame(Vec::new());
            self.frame(Vec::new())
        }
    }

    fn seek_pointer(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn seeking_previews_press_hold_and_drag_but_commits_only_once_on_release() {
        let mut gesture = SeekIntentHarness::new();
        let rect = gesture.warm().rect;
        let point = |fraction| {
            egui::pos2(
                rect.left() + 8.0 + (rect.width() - 16.0) * fraction,
                rect.center().y,
            )
        };
        let pressed = gesture.frame(seek_pointer(point(0.25), true));
        assert!(pressed.is_pointer_button_down_on());
        assert!(
            !pressed.dragged(),
            "Press must cover the pre-drag decision frame"
        );
        assert!((gesture.displayed - 25.0).abs() < 1e-5);
        assert!(gesture.preview.is_some());
        assert!(gesture.commits.is_empty());
        gesture.frame(Vec::new());
        assert!(gesture.commits.is_empty());
        for fraction in [0.4, 0.6, 0.8] {
            let moved = gesture.frame(vec![egui::Event::PointerMoved(point(fraction))]);
            assert!(moved.dragged());
            assert!((gesture.displayed - f64::from(fraction) * 100.0).abs() < 1e-5);
            assert!(gesture.commits.is_empty());
        }
        let released = gesture.frame(seek_pointer(point(0.8), false));
        assert!(released.drag_stopped());
        assert!(
            !released.changed(),
            "An unchanged release must still commit its preview"
        );
        assert_eq!(gesture.commits.len(), 1);
        assert!((gesture.commits[0] - 80.0).abs() < 1e-5);
        assert!(gesture.preview.is_none());
        gesture.frame(Vec::new());
        assert_eq!(gesture.commits.len(), 1);
    }

    #[test]
    fn seek_click_and_outside_release_commit_the_final_pointer_once() {
        for outside in [false, true] {
            for had_drag_frame in [false, true] {
                let mut gesture = SeekIntentHarness::new();
                let rect = gesture.warm().rect;
                let start = egui::pos2(rect.center().x, rect.center().y);
                gesture.frame(seek_pointer(start, true));
                assert!(gesture.commits.is_empty());
                if had_drag_frame {
                    gesture.frame(vec![egui::Event::PointerMoved(
                        start + egui::vec2(40.0, 0.0),
                    )]);
                    assert!(gesture.commits.is_empty());
                }
                let end = if outside {
                    egui::pos2(rect.right() + 30.0, rect.center().y)
                } else {
                    start
                };
                gesture.frame(seek_pointer(end, false));
                assert_eq!(gesture.commits.len(), 1);
                let expected = if outside { 100.0 } else { 50.0 };
                assert!((gesture.commits[0] - expected).abs() < 1e-5);
                assert!(gesture.preview.is_none());
                gesture.frame(Vec::new());
                assert_eq!(gesture.commits.len(), 1);
            }
        }
    }

    #[test]
    fn keyboard_seek_commits_immediately_and_lost_focus_or_disable_cancels_preview() {
        let mut gesture = SeekIntentHarness::new();
        let response = gesture.warm();
        gesture
            .context
            .memory_mut(|memory| memory.request_focus(response.id));
        gesture.frame(vec![egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: Some(egui::Key::ArrowRight),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(gesture.commits, [15.0]);
        assert!(gesture.preview.is_none());

        for disable in [false, true] {
            let mut gesture = SeekIntentHarness::new();
            let response = gesture.warm();
            gesture.frame(seek_pointer(response.rect.center(), true));
            gesture.frame(vec![egui::Event::PointerMoved(
                response.rect.center() + egui::vec2(30.0, 0.0),
            )]);
            assert!(gesture.preview.is_some());
            assert!(gesture.commits.is_empty());
            gesture.focused = disable;
            gesture.enabled = !disable;
            gesture.frame(vec![egui::Event::PointerGone]);
            assert!(gesture.preview.is_none());
            assert!(gesture.commits.is_empty());
            gesture.focused = true;
            gesture.enabled = true;
            let release = gesture.frame(seek_pointer(response.rect.center(), false));
            assert!(
                gesture.commits.is_empty(),
                "Canceled gesture committed on release (disable={disable}, clicked={}, drag_stopped={}, changed={}): {:?}",
                release.clicked(),
                release.drag_stopped(),
                release.changed(),
                gesture.commits,
            );
        }
    }

    #[test]
    fn seek_track_reserves_compact_time_slots_once_for_each_track() {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            let context = egui::Context::default();
            let mut app = account_ui_app(&context, Language::English);
            let mut frame = 0;
            for duration in [0, 180, 600, 3661] {
                let mut first_track = None;
                app.playing = Some(Track {
                    explicit: false,
                    added: None,
                    unavailable: None,
                    id: 77,
                    owner_id: 42,
                    artist: "Synthetic artist".into(),
                    title: "Synthetic track".into(),
                    duration,
                    url: None,
                    cover_url: None,
                    subtitle: None,
                    artists: Vec::new(),
                    album: None,
                    library_liked: None,
                    release_key: None,
                    library_source_key: None,
                });
                for position in [0.0_f64, 599.0, 600.0] {
                    // The player is stopped and has no device/source. This
                    // paints a preview label without issuing Player::seek.
                    let (output, track) = (0..4)
                        .find_map(|_| {
                            app.seek_position = Some(position);
                            let output =
                                account_ui_frame(&context, &mut app, size, &mut frame, Vec::new());
                            let track =
                                account_ui_shapes(&output).into_iter().find_map(
                                    |shape| match shape {
                                        egui::Shape::Rect(rect)
                                            if (rect.rect.height() - 7.0).abs() <= 0.5
                                                && rect.rect.width() > size.x * 0.5 =>
                                        {
                                            Some(rect.rect)
                                        }
                                        _ => None,
                                    },
                                );
                            // The initial invisible panel-sizing pass paints no
                            // controls. Compare from the first visible frame.
                            track.map(|track| (output, track))
                        })
                        .expect("Missing seek track");
                    if let Some(first) = first_track {
                        assert_eq!(
                            track, first,
                            "Seek bar moved with duration={duration}, position={position} at {size:?}"
                        );
                    } else {
                        first_track = Some(track);
                    }
                    let elapsed = time_label(position.min(f64::from(duration)));
                    let total = time_label(f64::from(duration));
                    let labels: Vec<_> = account_ui_shapes(&output)
                        .into_iter()
                        .filter_map(|shape| match shape {
                            egui::Shape::Text(text)
                                if text.galley.text() == elapsed || text.galley.text() == total =>
                            {
                                Some(text.visual_bounding_rect())
                            }
                            _ => None,
                        })
                        .collect();
                    let elapsed_rect = labels
                        .iter()
                        .find(|label| label.right() < track.left())
                        .unwrap();
                    let total_rect = labels
                        .iter()
                        .find(|label| label.left() > track.right())
                        .unwrap();
                    assert!(
                        track.left() - elapsed_rect.right() < 14.0,
                        "Excess elapsed-time gap"
                    );
                    assert!(
                        total_rect.left() - track.right() < 14.0,
                        "Excess duration gap"
                    );
                    assert!(
                        app.time_slots.unwrap().width < 48.0,
                        "Oversized time reservation"
                    );
                    assert!(app.seek_position.is_none());
                }
            }
        }
    }

    // These UI fixtures never load saved preferences, caches or account data.
    pub(super) fn account_ui_app(ctx: &egui::Context, language: Language) -> MusicApp {
        theme::install(ctx);
        MusicApp {
            session: None,
            pending: None,
            restoring: false,
            workers: Vec::new(),
            library: Library::default(),
            track_view_cache: track_view_cache::TrackViewCache::default(),
            page: MusicPage::default(),
            scrolling: Scrolling::default(),
            back_history: Vec::new(),
            forward_history: Vec::new(),
            last_history_frame: None,
            recent: Vec::new(),
            recent_save_pending: None,
            recent_save_current: None,
            recent_save_queue: HashMap::new(),
            recent_latest: HashMap::new(),
            play_queue: None,
            play_order: PlaybackOrder::default(),
            playback_modes_writer: crate::playback_modes::Writer::default(),
            queued_tracks: HashMap::new(),
            queue_ui: queue_app::PanelState::default(),
            lyrics: lyrics_app::State::default(),
            queue_origin: None,
            home_collections: Vec::new(),
            home_sources: Vec::new(),
            home_catalog: Default::default(),
            catalog_request: None,
            catalog_mix: None,
            mix_tuning: None,
            mix_settings: MixSettingsDialog::default(),
            home_request: None,
            home_requested: false,
            home_error: None,
            home_autoplay: None,
            details: HashMap::new(),
            playlist_items: Vec::new(),
            editable_playlists: Vec::new(),
            playlist_add_request: None,
            playlist_create_form: None,
            playlist_create_request: None,
            playlist_controls: playlist_actions::State::default(),
            library_browser: library_app::State::fixture(),
            audio_upload: audio_upload_app::State::default(),
            track_menu_error: None,
            track_menu_notice: None,
            library_undo: None,
            error_closing: None,
            error_shown: None,
            playlist_removal: playlist_removal::State::default(),
            row_drag_enabled: false,
            row_drag_started: None,
            reorder_request: None,
            album_titles: album_titles::State::default(),
            playlist_request: None,
            playlists_requested: false,
            playlist_error: None,
            detail_request: None,
            detail_error: None,
            metadata_request: None,
            metadata_attempted: HashSet::new(),
            like_request: None,
            dislike_request: None,
            disliked: HashSet::new(),
            like_aliases: HashMap::new(),
            removed_from_library: HashSet::new(),
            like_error: None,
            library_refresh_due: false,
            search: SearchState::default(),
            mix_request: None,
            mix_active: false,
            mix_seed: None,
            mix_has_more: false,
            mix_advance: false,
            mix_error: None,
            library_save_pending: None,
            library_save_current: None,
            library_save_queue: HashMap::new(),
            filter: String::new(),
            selected: None,
            auth_error: None,
            audio_error: None,
            demo: false,
            language,
            player: Player::new(),
            unavailable_playback: unavailable_playback::State::default(),
            system_media: system_media_app::State::default(),
            tray: tray_app::State::default(),
            minimize_to_tray: false,
            tray_save_failed: false,
            normalization_save_failed: false,
            logging_enabled: false,
            logging_save_failed: false,
            logging_runtime_failed: false,
            discord_rpc: discord_rpc_app::State::default(),
            discord_rpc_enabled: false,
            discord_rpc_save_failed: false,
            link_handler: None,
            link_handler_pending: None,
            link_handler_error: None,
            pending_link: None,
            link_track: None,
            #[cfg(any(windows, test))]
            updater: update_app::State::default(),
            playing: None,
            playback_pending: None,
            playback_cancel: None,
            playback_error: None,
            logged_playback_failure: None,
            seek_position: None,
            time_slots: None,
            resume_position: None,
            finished_handled: false,
            advance_on_load: false,
            covers: CoverCache::new(),
            avatars: AvatarCache::new(),
            avatar_refresh_ready: false,
            avatar_refresh_url: None,
            volume_save_at: None,
            volume_save_failed: false,
            compact_seek_bar: false,
            seek_layout_save_failed: false,
            // Existing layout fixtures exercise the system-decorated variant.
            // Custom chrome and live switching are covered separately below.
            system_titlebar: true,
            titlebar_save_failed: false,
            double_click_to_play: false,
            click_behavior_save_failed: false,
            search_on_enter: true,
            search_on_enter_save_failed: false,
            middle_autoscroll: false,
            middle_autoscroll_save_failed: false,
            explicit_badge: true,
            explicit_badge_save_failed: false,
            animated_backdrop: true,
            animated_backdrop_save_failed: false,
            volume_keys_enabled: false,
            volume_keys_save_failed: false,
            volume_keys: crate::volume_keys::VolumeKeys::default(),
            seek_pixels: 0.0,
            autoscroll: Default::default(),
            page_rect: egui::Rect::NOTHING,
            mix_after_queue: false,
            mix_after_queue_save_failed: false,
            screen_cache: None,
            screen_cache_owner: None,
            playlist_dates: HashMap::new(),
            track_sorts: HashMap::new(),
            sort_cache: None,
            library_tail: Vec::new(),
            library_tail_start: None,
            library_tail_request: None,
            row_number_offset: 0,
            broadcast: false,
            broadcast_save_failed: false,
            window_reopen: false,
            panel_widths: DEFAULT_PANEL_WIDTHS,
            ui_context: ctx.clone(),
            repaint_target: RepaintTarget::new(ctx),
            instance_activation: None,
            audio_cache: None,
            cache_limit_mb: preferences::DEFAULT_CACHE_MB,
            cache_stats: None,
            cache_pending: None,
            storage: StorageInfo::default(),
            storage_pending: None,
            cache_save_at: None,
            cache_error: false,
            cache_save_failed: false,
            settings_open: false,
            settings_section: SettingsSection::default(),
            settings_query: String::new(),
            language_save_failed: false,
            qr_control: None,
            qr_events: None,
            qr_pending: None,
            qr_texture: None,
            qr_url: None,
            qr_expires: None,
            qr_stage: qr_auth::Stage::Creating,
            qr_error: None,
            qr_open: false,
            qr_code_input: String::new(),
            #[cfg(feature = "screenshot")]
            screenshot_path: None,
            #[cfg(feature = "screenshot")]
            screenshot_requested: false,
            #[cfg(feature = "screenshot")]
            screenshot_ready_at: None,
        }
    }

    pub(super) fn account_ui_shapes(output: &egui::FullOutput) -> Vec<&egui::Shape> {
        fn flatten<'a>(shape: &'a egui::Shape, output: &mut Vec<&'a egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        flatten(shape, output);
                    }
                }
                _ => output.push(shape),
            }
        }
        let mut shapes = Vec::new();
        for shape in &output.shapes {
            flatten(&shape.shape, &mut shapes);
        }
        shapes
    }

    pub(super) fn account_ui_text_rect(
        output: &egui::FullOutput,
        label: &str,
    ) -> Option<egui::Rect> {
        account_ui_shapes(output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
    }

    fn account_ui_button_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        let text = account_ui_text_rect(output, label)
            .unwrap_or_else(|| panic!("Missing button text: {label}"));
        account_ui_shapes(output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if (34.0..=40.0).contains(&rect.rect.height())
                        && rect.rect.expand(1.0).contains_rect(text) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .unwrap_or_else(|| panic!("Missing button background: {label} at {text:?}"))
    }

    pub(super) fn account_ui_frame(
        ctx: &egui::Context,
        app: &mut MusicApp,
        size: egui::Vec2,
        frame: &mut u32,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        *frame += 1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(f64::from(*frame) * 0.1),
                events,
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        output.textures_delta.clear();
        let volume_save = app.volume_save_at.take();
        let cache_save = app.cache_save_at.take();
        assert!(volume_save.is_none(), "UI armed an unintended volume write");
        assert!(
            cache_save.is_none(),
            "UI armed an unintended cache preference write"
        );
        assert!(
            app.workers.is_empty(),
            "UI started a worker without a login click"
        );
        assert!(app.qr_pending.is_none());
        assert!(app.qr_control.is_none());
        output
    }

    pub(super) fn account_ui_click(
        ctx: &egui::Context,
        app: &mut MusicApp,
        size: egui::Vec2,
        frame: &mut u32,
        position: egui::Pos2,
    ) -> egui::FullOutput {
        let events = |pressed| {
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        account_ui_frame(ctx, app, size, frame, events(true));
        account_ui_frame(ctx, app, size, frame, events(false))
    }

    #[test]
    fn compact_library_rows_keep_uniform_fill_and_virtualized_stride_without_load_controls() {
        fn covers(output: &egui::FullOutput) -> Vec<egui::Rect> {
            account_ui_shapes(output)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == theme::SELECTED
                            && (rect.rect.width() - 40.0).abs() <= 0.5
                            && (rect.rect.height() - 40.0).abs() <= 0.5
                            && rect.rect.left() > 200.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect()
        }
        fn same_cover_column(output: &egui::FullOutput, left: f32) {
            let images = covers(output);
            assert!(!images.is_empty(), "No 40 px row artwork rendered");
            for image in images {
                assert!(
                    (image.left() - left).abs() <= 0.5,
                    "Digits or the playing icon shifted artwork: {image:?}, expected x={left}"
                );
            }
        }
        fn scroll_rows(
            ctx: &egui::Context,
            app: &mut MusicApp,
            size: egui::Vec2,
            frame: &mut u32,
            position: egui::Pos2,
            delta: f32,
        ) -> egui::FullOutput {
            let _ = account_ui_frame(
                ctx,
                app,
                size,
                frame,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, delta / 120.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            for _ in 0..6 {
                let _ = account_ui_frame(ctx, app, size, frame, Vec::new());
            }
            // Only scrolling is simulated. Clearing hover makes the uniform
            // row/background assertions independent of pointer feedback.
            account_ui_frame(ctx, app, size, frame, vec![egui::Event::PointerGone])
        }
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            for language in [Language::Russian, Language::English] {
                for connected in [false, true] {
                    let ctx = egui::Context::default();
                    let mut app = account_ui_app(&ctx, language);
                    let (sender, receiver) = mpsc::sync_channel(1);
                    if connected {
                        app.session = Some(Session {
                            profile: Profile {
                                id: 42,
                                first_name: "Test".into(),
                                last_name: "User".into(),
                                avatar_url: None,
                            },
                            cache_warning: false,
                            music: None,
                        });
                        // A short connected page is deliberately underfilled.
                        // Its existing request must never be replaced by paging.
                        app.pending = Some(receiver);
                    } else {
                        app.demo = true;
                        drop(receiver);
                    }
                    let count = if connected { 4 } else { 100 };
                    app.library.tracks = (1..=count)
                        .map(|id| Track {
                            explicit: false,
                            added: None,
                            unavailable: None,
                            id,
                            owner_id: 42,
                            title: format!(
                                "ROW {id:03} — {}",
                                "Длинное название / Long title ".repeat(4)
                            ),
                            artist: format!(
                                "ARTIST {id:03} — {}",
                                "Длинный исполнитель / Long artist ".repeat(4)
                            ),
                            duration: 180,
                            url: None,
                            cover_url: None,
                            subtitle: None,
                            artists: Vec::new(),
                            album: None,
                            library_liked: None,
                            release_key: None,
                            library_source_key: None,
                        })
                        .collect();
                    app.library.total = Some(0); // The old counter showed N of 0.
                    app.library.offset = count as usize;
                    app.library.exhausted = !connected;
                    app.selected = Some((42, 1));
                    let mut frame = 0;
                    let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                    for _ in 0..3 {
                        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                    }
                    let shapes = account_ui_shapes(&output);
                    let mut rows: Vec<_> = shapes
                        .iter()
                        .filter_map(|shape| match shape {
                            egui::Shape::Rect(rect)
                                if (rect.rect.height() - 54.0).abs() <= 0.5
                                    && rect.rect.width() > 200.0
                                    && rect.rect.left() > 200.0 =>
                            {
                                Some((rect.rect, rect.fill))
                            }
                            _ => None,
                        })
                        .collect();
                    rows.sort_by(|left, right| left.0.top().total_cmp(&right.0.top()));
                    assert!(rows.len() >= 3, "Too few rows rendered at {size:?}");
                    for (rect, fill) in &rows {
                        assert_eq!(
                            *fill, BACKGROUND,
                            "Unexpected zebra/selection fill at {rect:?}"
                        );
                    }
                    let artwork = covers(&output);
                    let cover_left = artwork.first().expect("Missing row artwork").left();
                    same_cover_column(&output, cover_left);
                    for (row, _) in rows.iter().take(2) {
                        let label = |prefix: &str| {
                            shapes
                                .iter()
                                .find_map(|shape| match shape {
                                    egui::Shape::Text(text)
                                        if text.galley.text().starts_with(prefix)
                                            && row
                                                .contains(text.visual_bounding_rect().center()) =>
                                    {
                                        Some(text.visual_bounding_rect())
                                    }
                                    _ => None,
                                })
                                .unwrap_or_else(|| panic!("Missing {prefix} inside row {row:?}"))
                        };
                        let title = label("ROW ");
                        let artist = label("ARTIST ");
                        assert!(
                            row.expand(0.5).contains_rect(title),
                            "Clipped title: {title:?} in {row:?}"
                        );
                        assert!(
                            row.expand(0.5).contains_rect(artist),
                            "Clipped artist: {artist:?} in {row:?}"
                        );
                        assert!(
                            title.bottom() + 0.5 <= artist.top(),
                            "Title/artist overlap in {row:?}"
                        );
                        assert!(
                            artwork.iter().any(|image| row.contains_rect(*image)),
                            "40 px artwork does not fit its own row {row:?}"
                        );
                    }
                    for pair in rows.windows(2) {
                        assert!(
                            (pair[1].0.top() - pair[0].0.top() - 58.0).abs() <= 0.5,
                            "Row cursor/virtualization drift at {size:?}: {:?} then {:?}",
                            pair[0].0,
                            pair[1].0
                        );
                        assert!(pair[1].0.top() >= pair[0].0.bottom() + 3.5);
                    }
                    let search = shapes
                        .iter()
                        .find_map(|shape| match shape {
                            egui::Shape::Rect(rect)
                                if rect.fill == SURFACE
                                    && rect.rect.width() >= rows[0].0.width() * 0.9
                                    && rect.rect.left() >= rows[0].0.left() - 1.0
                                    && rect.rect.bottom() <= rows[0].0.top()
                                    && rect.rect.height() < 50.0 =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .expect("Missing compact search field");
                    assert!(
                        search.height() <= 36.5,
                        "Search field is too tall: {search:?}"
                    );
                    for shape in &shapes {
                        if let egui::Shape::Text(text) = shape {
                            let label = text.galley.text();
                            assert!(
                                !label.starts_with("Загружено ") && !label.starts_with("Loaded ")
                            );
                            assert!(
                                !label.starts_with("Показано:") && !label.starts_with("Shown:")
                            );
                            assert_ne!(label, "Загрузить ещё");
                            assert_ne!(label, "Load more");
                        }
                    }
                    drop(shapes);
                    if !connected {
                        let pointer = egui::pos2(rows[0].0.left() + 80.0, rows[1].0.center().y);
                        output =
                            scroll_rows(&ctx, &mut app, size, &mut frame, pointer, -6.0 * 58.0);
                        same_cover_column(&output, cover_left);
                        assert!(
                            account_ui_shapes(&output).iter().any(|shape| match shape {
                                egui::Shape::Text(text) =>
                                    text.galley.text().starts_with("ROW 007"),
                                _ => false,
                            }),
                            "Scroll fixture did not expose row 7"
                        );
                        app.playing = Some(app.library.tracks[6].clone());
                        app.selected = app.playing.as_ref().map(Track::key);
                        for _ in 0..3 {
                            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                        }
                        same_cover_column(&output, cover_left);
                        output =
                            scroll_rows(&ctx, &mut app, size, &mut frame, pointer, -3.0 * 58.0);
                        same_cover_column(&output, cover_left);
                        assert!(
                            account_ui_shapes(&output).iter().any(|shape| match shape {
                                egui::Shape::Text(text) =>
                                    text.galley.text().starts_with("ROW 010"),
                                _ => false,
                            }),
                            "Scroll fixture did not expose a two-digit index"
                        );
                    }
                    if connected {
                        assert!(
                            sender
                                .send(Ok(Reply::Audio(page(0, &[]), false, None)))
                                .is_ok(),
                            "Rendering replaced the existing pagination receiver"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn home_navigation_sits_above_music_and_keeps_the_current_queue() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.demo = true;
            app.library.tracks = page(3, &[1, 2, 3]).items;
            app.library.exhausted = true;
            app.recent = page(1, &[9]).items;
            app.play_queue = Some(page(2, &[9, 10]).items);
            app.playing = app.recent.first().cloned();
            let size = egui::vec2(1040.0, 720.0);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let home = account_ui_text_rect(&output, language.text("Главная", "Home")).unwrap();
            let music =
                account_ui_text_rect(&output, language.text("Моя музыка", "My music")).unwrap();
            assert!(home.bottom() < music.top());
            account_ui_click(&ctx, &mut app, size, &mut frame, home.center());
            assert!(app.page == MusicPage::Home);
            assert_eq!(app.playing.as_ref().map(Track::key), Some((1, 9)));
            assert_eq!(app.adjacent_track(true), Some((1, 10)));
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let recent =
                account_ui_text_rect(&output, language.text("Недавнее", "Recent")).unwrap();
            account_ui_click(&ctx, &mut app, size, &mut frame, recent.center());
            assert!(app.page == MusicPage::Recent);
            assert_eq!(
                app.view_tracks()
                    .iter()
                    .map(|track| track.id)
                    .collect::<Vec<_>>(),
                [9]
            );
            assert_eq!(app.adjacent_track(true), Some((1, 10)));
            for _ in 0..2 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let music =
                account_ui_text_rect(&output, language.text("Моя музыка", "My music")).unwrap();
            account_ui_click(&ctx, &mut app, size, &mut frame, music.center());
            assert!(app.page == MusicPage::Library);
            assert_eq!(app.library.tracks.len(), 3);
            assert_eq!(app.adjacent_track(true), Some((1, 10)));
        }
    }

    #[test]
    fn a_late_catalog_hides_duplicate_genre_cards_and_keeps_loaded_tracks() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "User".into(),
                avatar_url: None,
            },
            music: None,
            cache_warning: false,
        });
        let source = |id: &str, title: &str| crate::web_audio::HomeCollection {
            id: id.into(),
            owner_id: 42,
            playlist_id: 5,
            title: title.into(),
            subtitle: String::new(),
            cover_url: None,
            access_key: None,
            tracks: Vec::new(),
        };
        app.home_sources = vec![source("for-you", "Для вас"), source("pop", "Поп")];
        app.rebuild_home_collections();
        assert_eq!(app.home_collections.len(), 2, "legacy Home shows first");
        app.home_collections[0].tracks = page(2, &[7, 8]).items;

        let mut catalog = crate::web_audio::HomeCatalog::default();
        catalog.genres.push(crate::web_audio::MixTile {
            id: "pop_genre".into(),
            title: "Поп".into(),
            mix: crate::web_audio::CatalogMix::Common {
                options: vec![("additionals".into(), "pop".into())],
            },
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        app.catalog_request = Some(CatalogRequest {
            owner: 42,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        sender.send(Ok(catalog)).unwrap();
        app.poll_home_catalog();
        assert!(app.catalog_request.is_none());
        assert_eq!(
            app.home_collections
                .iter()
                .map(|collection| collection.id.as_str())
                .collect::<Vec<_>>(),
            ["for-you"]
        );
        assert_eq!(app.home_collections[0].tracks.len(), 2);
        assert_eq!(app.home_catalog.genres.len(), 1);
    }

    #[test]
    fn home_play_button_starts_a_collection_without_opening_its_page() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.page = MusicPage::Home;
        app.home_sources.push(crate::web_audio::HomeCollection {
            id: "unloaded".into(),
            owner_id: 42,
            playlist_id: 5,
            title: "Unloaded playlist".into(),
            subtitle: String::new(),
            cover_url: None,
            access_key: None,
            tracks: Vec::new(),
        });
        app.home_collections.push(home_ui::Collection {
            id: "unloaded".into(),
            title: "Unloaded playlist".into(),
            subtitle: String::new(),
            cover_url: None,
            tracks: Vec::new(),
        });
        app.play_home_collection(&ctx, "unloaded".into());
        assert!(
            app.page == MusicPage::Home,
            "play must not open the playlist"
        );

        app.home_collections[0].tracks = page(2, &[7, 8]).items;
        app.play_home_collection(&ctx, "unloaded".into());
        assert!(app.page == MusicPage::Home);
        assert_eq!(app.queue_origin.as_deref(), Some("collection:unloaded"));
        assert_eq!(app.play_queue.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn manual_playback_toggle_cancels_a_delayed_collection_start() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.demo = true;
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "User".into(),
                avatar_url: None,
            },
            music: None,
            cache_warning: false,
        });
        app.home_collections.push(home_ui::Collection {
            id: "pending".into(),
            title: "Pending playlist".into(),
            subtitle: String::new(),
            cover_url: None,
            tracks: Vec::new(),
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        app.home_request = Some(HomeRequest {
            owner: 42,
            collection: None,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.home_autoplay = Some("pending".into());
        app.advance_on_load = true;
        app.toggle_playback(&ctx);
        assert!(app.home_autoplay.is_none());
        assert!(!app.advance_on_load);
        assert!(
            sender
                .send(Ok(HomeReply::Tracks("pending".into(), page(1, &[7]).items)))
                .is_ok()
        );
        app.poll_home(&ctx);
        assert_eq!(app.home_collections[0].tracks.len(), 1);
        assert!(app.playing.is_none());
        assert!(app.playback_pending.is_none());
    }

    #[test]
    fn only_child_page_headers_have_a_back_arrow() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::Russian);
        app.demo = true;
        app.library.tracks = page(2, &[1, 2]).items;
        app.library.exhausted = true;
        app.recent = page(1, &[9]).items;
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        for (route, title, child) in [
            (MusicPage::Home, "Главная", false),
            (MusicPage::Library, "Моя музыка", false),
            (MusicPage::Recent, "Недавнее", true),
        ] {
            app.navigate_music(&ctx, route);
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let heading = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text)
                        if text.galley.text() == title && text.pos.x > 252.0 =>
                    {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                heading.left() > 300.0,
                child,
                "Unexpected Back affordance at {title}: {heading:?}"
            );
        }
    }

    #[test]
    fn mouse_side_buttons_restore_page_history_and_search_without_changing_volume() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.demo = true;
        app.library.tracks = page(2, &[1, 2]).items;
        app.library.exhausted = true;
        app.recent = page(1, &[9]).items;
        app.filter = "library query".into();
        app.open_home(&ctx);
        app.navigate_music(&ctx, MusicPage::Recent);
        app.filter = "recent query".into();
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let volume = player_volume_rail(&output, size).center();
        let initial_volume = app.player.snapshot().volume;
        let mut click_side = |app: &mut MusicApp, button| {
            for pressed in [true, false] {
                account_ui_frame(
                    &ctx,
                    app,
                    size,
                    &mut frame,
                    vec![
                        egui::Event::PointerMoved(volume),
                        egui::Event::PointerButton {
                            pos: volume,
                            button,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        click_side(&mut app, egui::PointerButton::Extra1);
        assert!(app.page == MusicPage::Home);
        click_side(&mut app, egui::PointerButton::Extra1);
        assert!(app.page == MusicPage::Library);
        assert_eq!(app.filter, "library query");
        click_side(&mut app, egui::PointerButton::Extra2);
        assert!(app.page == MusicPage::Home);
        click_side(&mut app, egui::PointerButton::Extra2);
        assert!(app.page == MusicPage::Recent);
        assert_eq!(app.filter, "recent query");
        assert_eq!(app.player.snapshot().volume, initial_volume);
        click_side(&mut app, egui::PointerButton::Extra1);
        app.navigate_music(&ctx, MusicPage::Library);
        assert!(app.forward_history.is_empty());
    }

    #[test]
    fn pausing_mix_cancels_delayed_advance_but_keeps_the_next_batch() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.demo = true;
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "User".into(),
                avatar_url: None,
            },
            music: None,
            cache_warning: false,
        });
        app.mix_active = true;
        app.mix_has_more = true;
        app.mix_advance = true;
        app.play_queue = Some(page(2, &[1, 2]).items);
        app.playing = app.play_queue.as_ref().unwrap().first().cloned();
        let (sender, receiver) = mpsc::sync_channel(1);
        app.mix_request = Some(MixRequest {
            owner: 42,
            start: false,
            seed: None,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.toggle_playback(&ctx);
        assert!(!app.mix_advance);
        assert!(
            sender
                .send(Ok(crate::web_audio::MixPage {
                    tracks: page(1, &[3]).items,
                    has_more: true
                }))
                .is_ok()
        );
        app.poll_mix(&ctx);
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 3);
        assert_eq!(app.playing.as_ref().unwrap().id, 1);
        assert!(app.playback_pending.is_none());
    }

    #[test]
    fn reset_cancels_home_requests_and_discards_late_account_data() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let canceled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(1);
        app.home_request = Some(HomeRequest {
            owner: 42,
            collection: None,
            canceled: canceled.clone(),
            receiver,
        });
        app.home_autoplay = Some("old-playlist".into());
        app.recent = page(1, &[7]).items;
        app.play_queue = Some(page(1, &[7]).items);
        app.page = MusicPage::Collection("old-playlist".into());
        app.reset();
        assert!(canceled.load(Ordering::Acquire));
        assert!(sender.send(Ok(HomeReply::Catalog(Vec::new()))).is_err());
        assert!(app.recent.is_empty());
        assert!(app.play_queue.is_none());
        assert!(app.home_autoplay.is_none());
        assert!(app.page == MusicPage::Home);
    }

    fn player_volume_rail(output: &egui::FullOutput, size: egui::Vec2) -> egui::Rect {
        account_ui_shapes(output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == Color32::from_rgb(43, 49, 59)
                        && (rect.rect.height() - 7.0).abs() < 0.5
                        && rect.rect.width() > 40.0
                        && rect.rect.width() < 150.0
                        && rect.rect.left() > size.x * 0.7
                        && rect.rect.top() > size.y - 110.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("Player volume rail")
    }

    fn seek_track_in_player(output: &egui::FullOutput, screen_height: f32) -> egui::Rect {
        account_ui_shapes(output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.height() - 7.0).abs() < 0.5
                        && rect.rect.width() > 32.0
                        && rect.rect.bottom() > screen_height - 40.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("Playback position track")
    }

    fn shortcut_test_frame(
        ctx: &egui::Context,
        app: &mut MusicApp,
        frame: &mut u32,
        events: Vec<egui::Event>,
    ) {
        *frame += 1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1040.0, 720.0),
                )),
                time: Some(f64::from(*frame) * 0.1),
                events,
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        output.textures_delta.clear();
        app.volume_save_at.take();
    }

    #[test]
    fn player_shortcuts_seek_and_adjust_volume_once_and_leave_search_editing_alone() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        app.library.exhausted = true;
        let mut track = page(1, &[1]).items.remove(0);
        track.duration = 90;
        app.playing = Some(track);
        let (player, _commands) = Player::test_paused(90, 20.0);
        app.player = player;
        let key = |key, modifiers, repeat| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat,
            modifiers,
        };
        let mut frame = 0;
        shortcut_test_frame(&ctx, &mut app, &mut frame, Vec::new());
        let original = app.player.snapshot().volume;
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![
                key(egui::Key::ArrowRight, egui::Modifiers::NONE, false),
                key(egui::Key::ArrowUp, egui::Modifiers::NONE, false),
            ],
        );
        assert_eq!(app.player.snapshot().position, 25.0);
        assert!((app.player.snapshot().volume - original - 0.05).abs() < 1e-6);
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![
                key(egui::Key::ArrowLeft, egui::Modifiers::NONE, true),
                key(egui::Key::ArrowDown, egui::Modifiers::NONE, false),
            ],
        );
        assert_eq!(app.player.snapshot().position, 20.0);
        assert!((app.player.snapshot().volume - original).abs() < 1e-6);
        // Clicking the global field keeps arrows for caret/suggestion navigation.
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![
                egui::Event::PointerMoved(egui::pos2(350.0, 25.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(350.0, 25.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(350.0, 25.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(ctx.text_edit_focused());
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![
                key(egui::Key::ArrowRight, egui::Modifiers::NONE, false),
                key(egui::Key::ArrowUp, egui::Modifiers::NONE, false),
            ],
        );
        assert_eq!(app.player.snapshot().position, 20.0);
        assert!((app.player.snapshot().volume - original).abs() < 1e-6);
        ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            [egui::Key::ArrowLeft, egui::Key::ArrowRight]
                .into_iter()
                .map(|key| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
        );
        app.library.tracks = page(3, &[1, 2, 3]).items;
        app.playing = Some(app.library.tracks[1].clone());
        app.prepare_view_queue();
        app.synchronize_order();
        let (player, _more_commands) = Player::test_paused(90, 20.0);
        app.player = player;
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![key(egui::Key::ArrowLeft, egui::Modifiers::CTRL, false)],
        );
        assert_eq!(
            app.playing.as_ref().unwrap().id,
            1,
            "Ctrl+Left must select the previous track even after 3 seconds"
        );
        shortcut_test_frame(
            &ctx,
            &mut app,
            &mut frame,
            vec![key(egui::Key::ArrowRight, egui::Modifiers::CTRL, false)],
        );
        assert_eq!(app.playing.as_ref().unwrap().id, 2);
    }

    #[test]
    fn shuffle_and_loop_buttons_change_modes_without_interrupting_the_current_track() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        app.library.tracks = page(4, &[1, 2, 3, 4]).items;
        app.library.exhausted = true;
        app.playing = Some(app.library.tracks[0].clone());
        app.prepare_view_queue();
        app.synchronize_order();
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let play = account_ui_shapes(&output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == PRIMARY
                        && rect.rect.width() == 38.0
                        && rect.rect.bottom() > size.y - 100.0 =>
                {
                    Some(rect.rect.center())
                }
                _ => None,
            })
            .unwrap();
        account_ui_click(
            &ctx,
            &mut app,
            size,
            &mut frame,
            play - egui::vec2(96.0, 0.0),
        );
        assert!(app.play_order.shuffle());
        assert_eq!(app.playing.as_ref().unwrap().id, 1);
        let next = app.adjacent_track(true).unwrap();
        assert_ne!(next, (1, 1));
        assert_eq!(app.adjacent_track(true), Some(next));
        for mode in [RepeatMode::All, RepeatMode::One, RepeatMode::Off] {
            account_ui_click(
                &ctx,
                &mut app,
                size,
                &mut frame,
                play + egui::vec2(96.0, 0.0),
            );
            assert_eq!(app.play_order.repeat(), mode);
        }
        assert_eq!(
            app.library
                .tracks
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert!(app.playback_pending.is_none());
        // Selecting within the same source must preserve the shuffled back-history.
        app.play_order.record_played(next);
        app.playing = app
            .library
            .tracks
            .iter()
            .find(|track| track.key() == next)
            .cloned();
        app.prepare_view_queue();
        assert_eq!(app.adjacent_track(false), Some((1, 1)));
        let old_keys = app.playback_keys();
        app.start_queue(&ctx, Vec::new());
        assert_eq!(app.playback_keys(), old_keys);
    }

    #[test]
    fn recent_restore_keeps_newest_history_even_after_a_finished_write_or_account_reset() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.recent = page(2, &[8, 7]).items;
        app.save_recent(&ctx);
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.recent_save_pending.is_some() {
            app.poll_recent_save(&ctx);
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(app.recent_save_current.is_none());
        assert_eq!(
            app.restored_recent(42, page(1, &[1]).items)
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [8, 7]
        );
        app.reset();
        assert!(app.recent.is_empty());
        assert_eq!(
            app.restored_recent(42, Vec::new())
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [8, 7]
        );
        assert!(app.restored_recent(43, Vec::new()).is_empty());
    }

    #[test]
    fn compact_player_enlarges_metadata_and_centers_side_controls_on_the_full_panel() {
        for size in [
            egui::vec2(800.0, 540.0),
            egui::vec2(1040.0, 720.0),
            egui::vec2(1600.0, 900.0),
        ] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.demo = false;
            let mut track = page(1, &[1]).items.remove(0);
            track.title = "Large title".into();
            track.artist = "First, Second".into();
            track.artists = vec![
                ArtistRef {
                    id: "first".into(),
                    name: "First".into(),
                },
                ArtistRef {
                    id: "second".into(),
                    name: "Second".into(),
                },
            ];
            app.playing = Some(track);
            let mut frame = 0;
            let mut last = None;
            for compact in [false, true] {
                app.compact_seek_bar = compact;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let shapes = account_ui_shapes(&output);
                let panel = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == SURFACE
                                && (rect.rect.width() - size.x).abs() < 0.5
                                && (rect.rect.bottom() - size.y).abs() < 0.5 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .unwrap();
                let art_size = if compact { 64.0 } else { 40.0 };
                let art = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if (rect.rect.width() - art_size).abs() < 0.5
                                && (rect.rect.height() - art_size).abs() < 0.5
                                && rect.rect.left() < 30.0
                                && panel.contains_rect(rect.rect) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Player artwork");
                let volume = player_volume_rail(&output, size).center();
                assert!((volume.y - art.center().y).abs() < 0.5);
                for (caption, expected_font) in [("Large title", 14.0), ("Second", 12.0)] {
                    let font = shapes
                        .iter()
                        .find_map(|shape| match shape {
                            egui::Shape::Text(text)
                                if text.galley.text() == caption
                                    && panel.contains_rect(text.visual_bounding_rect()) =>
                            {
                                Some(text.galley.job.sections[0].format.font_id.size)
                            }
                            _ => None,
                        })
                        .unwrap();
                    assert_eq!(font, expected_font);
                }
                let title = account_ui_text_rect(&output, "Large title").unwrap();
                let artist = account_ui_text_rect(&output, "Second").unwrap();
                let text_block = title.union(artist);
                assert!((text_block.center().y - art.center().y).abs() < 3.0);
                if compact {
                    assert!((art.center().y - panel.center().y).abs() < 0.5);
                    assert!(text_block.bottom() < seek_track_in_player(&output, size.y).top());
                }
                last = Some(output);
            }
            let output = last.unwrap();
            let second = account_ui_text_rect(&output, "Second").unwrap().center();
            account_ui_click(&ctx, &mut app, size, &mut frame, second);
            assert!(matches!(&app.page,MusicPage::Artist(artist) if artist.id=="second"));
            assert!(app.playing.is_some());
        }
    }

    #[test]
    fn compact_player_keeps_long_metadata_outside_the_timeline_while_resizing() {
        for duration in [263, u32::MAX] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.demo = false;
            app.compact_seek_bar = true;
            let mut track = page(1, &[1]).items.remove(0);
            track.duration = duration;
            track.title = "LONG_TITLE Very long title / Очень длинное название ".repeat(8);
            track.subtitle = Some("Slowed".into());
            track.artist = "LONG_ARTIST Очень длинный исполнитель ".repeat(8);
            track.artists = vec![ArtistRef {
                id: "long".into(),
                name: track.artist.clone(),
            }];
            app.playing = Some(track);
            let mut frame = 0;
            let mut widths = Vec::new();
            for size in [
                egui::vec2(640.0, 540.0),
                egui::vec2(800.0, 540.0),
                egui::vec2(1040.0, 720.0),
                egui::vec2(1600.0, 900.0),
                egui::vec2(800.0, 540.0),
            ] {
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let body = egui::Rect::from_min_size(
                    egui::pos2(16.0, size.y - 88.0),
                    egui::vec2(size.x - 32.0, 80.0),
                );
                let seek = player_seek_rect(body, true, app.time_slots.unwrap().width);
                let track = seek_track_in_player(&output, size.y);
                widths.push(track.width());
                assert!((seek.center().x - size.x * 0.5).abs() < 0.5);
                let elapsed = account_ui_text_rect(&output, "0:00").unwrap();
                let total =
                    account_ui_text_rect(&output, &time_label(f64::from(duration))).unwrap();
                assert!(elapsed.left() >= seek.left() - 1.0 && total.right() <= seek.right() + 1.0);
                for prefix in ["LONG_TITLE", "LONG_ARTIST"] {
                    let visible = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text)
                                if text.galley.text().starts_with(prefix)
                                    && text.pos.y > body.top() =>
                            {
                                Some(text.visual_bounding_rect().intersect(shape.clip_rect))
                            }
                            _ => None,
                        })
                        .unwrap();
                    assert!(
                        visible.right() <= seek.left() - PLAYER_COLUMN_GAP + 1.0,
                        "Metadata overlaps time at {size:?}: {visible:?} vs {seek:?}"
                    );
                    assert!(!visible.intersects(elapsed) && !visible.intersects(total));
                }
                let volume_left = (body.right() - 160.0).max(seek.right() + PLAYER_COLUMN_GAP);
                assert!(total.right() < volume_left);
            }
            assert!(widths[0] < widths[1] && widths[1] < widths[2]);
            assert_eq!(
                widths[1], widths[4],
                "Returning to a window size must restore the same layout"
            );
        }
    }

    #[test]
    fn compact_seek_bar_is_centered_and_changes_only_the_timeline_width() {
        for size in [
            egui::vec2(800.0, 540.0),
            egui::vec2(1040.0, 720.0),
            egui::vec2(1600.0, 900.0),
        ] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.demo = false;
            let mut track = page(1, &[1]).items.remove(0);
            track.duration = 263;
            app.playing = Some(track);
            let mut frame = 0;
            let mut wide = None;
            let mut transport_center = None;
            for compact in [false, true] {
                app.compact_seek_bar = compact;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let seek = seek_track_in_player(&output, size.y);
                assert!((seek.center().x - size.x * 0.5).abs() < 1.0);
                let play = account_ui_shapes(&output)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == PRIMARY
                                && (rect.rect.width() - 38.0).abs() < 0.5
                                && rect.rect.bottom() > size.y - 100.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .unwrap();
                if let Some(center) = transport_center {
                    assert_eq!(play.center(), center);
                } else {
                    transport_center = Some(play.center());
                }
                if compact {
                    assert!(seek.width() <= 520.0 && seek.width() < wide.unwrap() * 0.7);
                } else {
                    wide = Some(seek.width());
                }
                assert!(account_ui_text_rect(&output, "4:23").is_some());
            }
        }
    }

    #[test]
    fn about_settings_contains_brand_and_package_version_in_both_languages() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.settings_open = true;
            app.settings_section = SettingsSection::General;
            let size = egui::vec2(1040.0, 720.0);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let version = format!(
                "{} {}",
                language.text("Версия", "Version"),
                env!("CARGO_PKG_VERSION")
            );
            assert!(account_ui_text_rect(&output, &version).is_none());
            let tab = account_ui_text_rect(&output, language.text("О приложении", "About"))
                .unwrap()
                .center();
            account_ui_click(&ctx, &mut app, size, &mut frame, tab);
            assert!(app.settings_section == SettingsSection::About);
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let version_rect = account_ui_text_rect(&output, &version).unwrap();
            assert!(
                version_rect.left() > 240.0,
                "Version belongs in the settings content"
            );
            assert!(account_ui_shapes(&output).iter().any(|shape| matches!(shape, egui::Shape::Text(text) if text.galley.text() == "VK" && text.pos.x > 240.0)));
            assert!(account_ui_shapes(&output).iter().any(|shape| matches!(shape, egui::Shape::Text(text) if text.galley.text() == "FAST" && text.pos.x > 240.0)));
            assert!(account_ui_text_rect(&output, "GPL-3.0-only").is_some());
            let icon = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.brush.is_some()
                            && (rect.rect.width() - 56.0).abs() < 0.5
                            && (rect.rect.height() - 56.0).abs() < 0.5
                            && rect.rect.left() > 240.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("About should display the embedded application icon");
            let brand_center = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text() == "VK" && text.pos.x > 240.0 => {
                        Some(text.pos.y + text.galley.size().y * 0.5)
                    }
                    _ => None,
                })
                .unwrap();
            assert!((icon.center().y - brand_center).abs() < 1.0);
        }
    }

    #[test]
    fn interface_settings_switch_applies_both_layouts_without_touching_user_preferences() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.settings_open = true;
            app.settings_section = SettingsSection::General;
            let size = egui::vec2(1040.0, 720.0);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let label = language.text("Короткая полоска по центру", "Centered compact seek bar");
            assert!(account_ui_text_rect(&output, label).is_none());
            app.settings_section = SettingsSection::Playback;
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            assert!(account_ui_text_rect(&output, label).is_none());
            assert!(
                account_ui_text_rect(
                    &output,
                    language.text("Нормализация громкости", "Normalize volume")
                )
                .is_some()
            );
            assert!(account_ui_text_rect(&output, language.text("Громкость", "Volume")).is_none());
            let interface = account_ui_text_rect(&output, language.text("Интерфейс", "Interface"))
                .unwrap()
                .center();
            account_ui_click(&ctx, &mut app, size, &mut frame, interface);
            assert!(app.settings_section == SettingsSection::Interface);
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            let wide = seek_track_in_player(&output, size.y);
            let switch = account_ui_text_rect(&output, label).unwrap().center();
            account_ui_click(&ctx, &mut app, size, &mut frame, switch);
            assert!(app.compact_seek_bar);
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            assert!(seek_track_in_player(&output, size.y).width() < wide.width() * 0.7);
            account_ui_frame(
                &ctx,
                &mut app,
                size,
                &mut frame,
                vec![egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: Some(egui::Key::Space),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(
                !app.compact_seek_bar,
                "Focused toggle must also support keyboard activation"
            );
            assert!(!app.seek_layout_save_failed);
        }
    }

    #[test]
    fn player_transport_stays_centered_with_solid_glyphs_and_separate_metadata_and_volume() {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            for language in [Language::Russian, Language::English] {
                let ctx = egui::Context::default();
                let mut app = account_ui_app(&ctx, language);
                app.session = Some(Session {
                    profile: Profile {
                        id: 42,
                        first_name: "Test".into(),
                        last_name: "User".into(),
                        avatar_url: None,
                    },
                    cache_warning: false,
                    music: None,
                });
                let track = Track {
                    explicit: false,
                    added: None,
                    unavailable: None,
                    id: 2,
                    owner_id: 42,
                    title: "TITLE Очень длинное название / A deliberately long track title "
                        .repeat(4),
                    artist: "ARTIST Очень длинный исполнитель / A deliberately long artist name "
                        .repeat(4),
                    duration: 240,
                    url: None,
                    cover_url: None,
                    subtitle: None,
                    artists: Vec::new(),
                    album: None,
                    library_liked: None,
                    release_key: None,
                    library_source_key: None,
                };
                app.library.tracks = [1, 2, 3]
                    .into_iter()
                    .map(|id| Track {
                        unavailable: None,
                        id,
                        ..track.clone()
                    })
                    .collect();
                app.library.total = Some(3);
                app.library.offset = 3;
                app.library.exhausted = true;
                app.selected = Some(track.key());
                app.playing = Some(track);
                let mut frame = 0;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let shapes = account_ui_shapes(&output);
                let panel = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == SURFACE
                                && (rect.rect.width() - size.x).abs() <= 1.0
                                && (rect.rect.bottom() - size.y).abs() <= 1.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Missing full-width player panel");
                assert!(
                    panel.height() <= 100.0,
                    "Player scopes inflated the footer: {panel:?}"
                );
                let play = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == PRIMARY
                                && (rect.rect.width() - 38.0).abs() <= 0.5
                                && (rect.rect.height() - 38.0).abs() <= 0.5
                                && panel.contains_rect(rect.rect) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Missing 38 px primary play/pause button");
                assert!(
                    (play.center().x - panel.center().x).abs() <= 1.0,
                    "Transport moved off-center at {size:?}: {play:?} in {panel:?}"
                );
                let seek_track = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if (rect.rect.height() - 7.0).abs() <= 0.5
                                && rect.rect.width() > size.x * 0.5
                                && panel.contains_rect(rect.rect) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Missing playback position track");
                let expected_y = (panel.top() + seek_track.center().y) * 0.5;
                assert!(
                    (play.center().y - expected_y).abs() <= 1.0,
                    "Transport is not halfway between panel and seek bar: {play:?}, {seek_track:?}, {panel:?}"
                );
                let mut triangles: Vec<_> = shapes
                    .iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Path(path)
                            if path.closed
                                && path.points.len() >= 6
                                && path.fill != Color32::TRANSPARENT
                                && path.stroke.width == 0.0 =>
                        {
                            let bounds = egui::Rect::from_points(&path.points);
                            ((bounds.center().y - play.center().y).abs() <= 1.0
                                && (bounds.center().x - play.center().x).abs() < 80.0)
                                .then_some(bounds)
                        }
                        _ => None,
                    })
                    .collect();
                triangles.sort_by(|left, right| left.center().x.total_cmp(&right.center().x));
                assert_eq!(
                    triangles.len(),
                    3,
                    "Previous/play/next must have filled rounded glyphs"
                );
                assert!(play.contains_rect(triangles[1]));
                assert!(triangles[0].right() < play.left());
                assert!(triangles[2].left() > play.right());
                for triangle in [triangles[0], triangles[2]] {
                    assert!(
                        shapes.iter().any(|shape| match shape {
                            egui::Shape::Rect(rect) =>
                                rect.fill != Color32::TRANSPARENT
                                    && (1.0..=3.0).contains(&rect.rect.width())
                                    && (8.0..=15.0).contains(&rect.rect.height())
                                    && (rect.rect.center().y - play.center().y).abs() <= 1.0
                                    && rect.rect.center().distance(triangle.center()) < 16.0,
                            _ => false,
                        }),
                        "Previous/next is missing its solid vertical bar"
                    );
                }
                let idle_backgrounds = shapes
                    .iter()
                    .filter(|shape| match shape {
                        egui::Shape::Rect(rect) => {
                            (rect.rect.width() - 38.0).abs() <= 0.5
                                && (rect.rect.height() - 38.0).abs() <= 0.5
                                && (rect.rect.center().y - play.center().y).abs() <= 1.0
                                && (rect.rect.center().x - play.center().x).abs() < 80.0
                        }
                        _ => false,
                    })
                    .count();
                assert_eq!(
                    idle_backgrounds, 1,
                    "Idle previous/next acquired circular backgrounds"
                );
                for prefix in ["TITLE ", "ARTIST "] {
                    let text = shapes
                        .iter()
                        .find_map(|shape| match shape {
                            egui::Shape::Text(text)
                                if text.galley.text().starts_with(prefix)
                                    && panel.contains(text.pos) =>
                            {
                                Some(text.visual_bounding_rect())
                            }
                            _ => None,
                        })
                        .expect("Missing player metadata");
                    assert!(
                        text.right() < triangles[0].left() - 5.0,
                        "Long metadata overlaps transport: {text:?} and {:?}",
                        triangles[0]
                    );
                    assert!(panel.contains_rect(text));
                }
                let volume = player_volume_rail(&output, size);
                assert!(volume.left() >= panel.right() - 160.0);
                assert!(volume.right() <= panel.right() - 8.0);
                assert!(volume.left() > triangles[2].right() + 16.0);
                assert!(panel.contains_rect(volume));
            }
        }
    }

    #[test]
    fn profile_refresh_replaces_numeric_fallback_and_keeps_known_metadata_on_partial_responses() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let placeholder = Profile {
            id: 42,
            first_name: "VK".into(),
            last_name: "42".into(),
            avatar_url: Some("https://images.example.test/cached.jpg".into()),
        };
        app.session = Some(Session {
            profile: placeholder.clone(),
            music: None,
            cache_warning: false,
        });
        let fresh = Profile {
            id: 42,
            first_name: "Анна".into(),
            last_name: "Test".into(),
            avatar_url: None,
        };
        for profile in [
            fresh.clone(),
            Profile {
                avatar_url: None,
                ..placeholder.clone()
            },
            Profile { id: 99, ..fresh },
        ] {
            let foreign = profile.id != 42;
            let (sender, receiver) = mpsc::sync_channel(1);
            app.pending = Some(receiver);
            sender
                .send(Ok(Reply::Audio(
                    page(0, &[]),
                    true,
                    Some(RefreshedProfile {
                        profile,
                        cache_warning: false,
                    }),
                )))
                .ok()
                .unwrap();
            app.poll(&ctx);
            let session = app.session.as_ref().unwrap();
            assert_eq!(session.profile.display_name(), "Анна Test");
            assert_eq!(session.profile.avatar_url, placeholder.avatar_url);
            if foreign {
                assert!(matches!(
                    app.audio_error,
                    Some(UiError::Music(WebAudioError::WrongAccount))
                ));
            }
        }
    }

    #[test]
    fn avatar_refresh_uses_only_the_first_viewer_check_of_the_run() {
        for first in [
            None,
            Some("https://images.example.test/first.jpg".to_owned()),
        ] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            for avatar in [
                first.clone(),
                Some("https://images.example.test/changed.jpg".to_owned()),
            ] {
                let (sender, receiver) = mpsc::sync_channel(1);
                app.pending = Some(receiver);
                assert!(
                    sender
                        .send(Ok(Reply::Audio(
                            page(0, &[]),
                            true,
                            Some(RefreshedProfile {
                                profile: Profile {
                                    id: 42,
                                    first_name: "Test".into(),
                                    last_name: "User".into(),
                                    avatar_url: avatar,
                                },
                                cache_warning: false,
                            })
                        )))
                        .is_ok()
                );
                app.poll(&ctx);
                assert!(app.avatar_refresh_ready);
                assert_eq!(app.avatar_refresh_url, first);
            }
        }
    }

    #[test]
    fn account_popup_opens_at_avatar_and_closes_without_starting_auth() {
        let id = egui::Id::new("account-popup");
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            for language in [Language::Russian, Language::English] {
                let ctx = egui::Context::default();
                let mut app = account_ui_app(&ctx, language);
                let mut frame = 0;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                assert!(!egui::Popup::is_id_open(&ctx, id));
                let shapes = account_ui_shapes(&output);
                let avatar = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Circle(circle)
                            if circle.radius >= 18.0 && circle.center.y < 90.0 =>
                        {
                            Some(egui::Rect::from_center_size(
                                circle.center,
                                egui::vec2(38.0, 38.0),
                            ))
                        }
                        _ => None,
                    })
                    .expect("Missing account avatar in header");
                let settings = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.rect.center().y < 90.0
                                && rect.rect.size() == egui::vec2(38.0, 38.0) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Missing settings button in header");
                assert!(avatar.left() > settings.right());
                assert!((avatar.center().y - settings.center().y).abs() <= 1.0);

                account_ui_click(&ctx, &mut app, size, &mut frame, avatar.center());
                assert!(egui::Popup::is_id_open(&ctx, id));
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let popup = ctx.memory(|memory| memory.area_rect(id)).unwrap();
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                assert!(
                    screen.contains_rect(popup),
                    "Popup clipped at {size:?}: {popup:?}"
                );
                assert!((popup.right() - avatar.right()).abs() <= 2.0);
                assert!(popup.top() >= avatar.bottom());
                assert!(
                    account_ui_text_rect(
                        &output,
                        language.text("Подключение к VK", "Connect to VK")
                    )
                    .is_some()
                );

                account_ui_click(&ctx, &mut app, size, &mut frame, egui::pos2(4.0, 100.0));
                assert!(!egui::Popup::is_id_open(&ctx, id));
                account_ui_click(&ctx, &mut app, size, &mut frame, avatar.center());
                assert!(egui::Popup::is_id_open(&ctx, id));
                account_ui_frame(
                    &ctx,
                    &mut app,
                    size,
                    &mut frame,
                    vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: Some(egui::Key::Escape),
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                assert!(!egui::Popup::is_id_open(&ctx, id));
            }
        }
    }

    #[test]
    fn signed_out_library_centers_login_actions_and_hides_legacy_connection_ui() {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            for language in [Language::Russian, Language::English] {
                let ctx = egui::Context::default();
                let mut app = account_ui_app(&ctx, language);
                let mut frame = 0;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let hero = account_ui_text_rect(
                    &output,
                    language.text("Музыка рядом", "Your music, closer"),
                )
                .unwrap();
                let qr = account_ui_button_rect(
                    &output,
                    language.text("Войти по QR-коду", "Sign in with QR code"),
                );
                assert!(qr.top() > hero.bottom());
                assert!((qr.center().x - hero.center().x).abs() <= 2.0);
                let player = account_ui_shapes(&output)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if (rect.rect.bottom() - size.y).abs() <= 1.0
                                && (rect.rect.width() - size.x).abs() <= 1.0
                                && rect.fill == SURFACE =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Missing player panel");
                assert!(
                    qr.bottom() <= player.top(),
                    "Login actions overlap player at {size:?}: QR {qr:?}, player {player:?}"
                );
                let texts: Vec<_> = account_ui_shapes(&output)
                    .into_iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                for removed in [
                    "Войти через VK",
                    "Sign in with VK",
                    "Вход по токену",
                    "Token sign-in",
                    "Посмотреть демо",
                    "Explore demo",
                    "О подключении",
                    "About connecting",
                    "Подключение к VK",
                    "Connect to VK",
                ] {
                    assert!(
                        !texts.contains(&removed),
                        "Legacy connection UI is visible: {removed}"
                    );
                }

                let (_sender, receiver) = mpsc::sync_channel(1);
                app.pending = Some(receiver);
                app.restoring = true;
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                assert!(
                    account_ui_text_rect(
                        &output,
                        language.text(
                            "Восстанавливаем сохранённый вход…",
                            "Restoring your saved sign-in…"
                        ),
                    )
                    .is_some()
                );

                app.restoring = false;
                app.session = Some(Session {
                    profile: Profile {
                        id: 42,
                        first_name: "Synthetic".into(),
                        last_name: "Listener".into(),
                        avatar_url: None,
                    },
                    cache_warning: false,
                    music: None,
                });
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                assert!(
                    account_ui_text_rect(
                        &output,
                        language.text("Загружаем аудиозаписи…", "Loading tracks…"),
                    )
                    .is_some()
                );
                for label in [
                    language.text("Войти через VK", "Sign in with VK"),
                    language.text("Войти по QR-коду", "Sign in with QR code"),
                    language.text("Отменить", "Cancel"),
                ] {
                    assert!(account_ui_text_rect(&output, label).is_none());
                }
            }
        }
    }

    struct CacheScheduleDirectory(std::path::PathBuf);

    impl CacheScheduleDirectory {
        fn new() -> Self {
            let mut bytes = [0_u8; 12];
            getrandom::fill(&mut bytes).unwrap();
            let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("vk-fast-cache-schedule-{suffix}"));
            std::fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
    }

    impl Drop for CacheScheduleDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cache_schedule_app() -> (CacheScheduleDirectory, egui::Context, MusicApp, AudioCache) {
        let directory = CacheScheduleDirectory::new();
        let cache = AudioCache::test_at(directory.0.join("audio"), 4096).unwrap();
        let ctx = egui::Context::default();
        let mut app = MusicApp::new(&ctx, false, Language::Russian);
        app.audio_cache = Some(cache.clone());
        app.cache_limit_mb = preferences::DEFAULT_CACHE_MB;
        (directory, ctx, app, cache)
    }

    fn cache_schedule_key(track_id: i64) -> CacheKey {
        CacheKey {
            account_id: 42,
            owner_id: 42,
            track_id,
            duration: 60,
        }
    }

    fn finish_cache_schedule_job(app: &mut MusicApp, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.cache_pending.is_some() {
            app.poll_cache(ctx);
            assert!(Instant::now() < deadline, "Cache job did not complete");
            if app.cache_pending.is_some() {
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(app.cache_save_at.is_none());
        assert!(app.volume_save_at.is_none());
    }

    /// Settings also start a storage measurement; these tests follow only
    /// the audio cache's own worker.
    fn cache_workers(app: &MusicApp) -> Vec<std::thread::ThreadId> {
        app.workers
            .iter()
            .filter(|worker| worker.thread().name() == Some("vk-audio-cache"))
            .map(|worker| worker.thread().id())
            .collect()
    }

    #[test]
    fn cache_settings_snapshot_is_requested_once_per_open_and_refreshed_on_reopen() {
        let (_directory, ctx, mut app, cache) = cache_schedule_app();
        cache
            .write_range(&cache_schedule_key(1), 0, &[0; 64])
            .unwrap();
        let initial = cache.stats();
        assert!(initial.used_bytes > 0);
        app.open_settings(&ctx);
        assert!(app.settings_open);
        assert!(app.cache_pending.is_some());
        assert_eq!(cache_workers(&app).len(), 1);
        let first_worker = cache_workers(&app)[0];
        app.open_settings(&ctx);
        assert_eq!(cache_workers(&app).len(), 1);
        assert_eq!(cache_workers(&app)[0], first_worker);
        finish_cache_schedule_job(&mut app, &ctx);
        assert_eq!(
            app.cache_stats.as_ref().unwrap().used_bytes,
            initial.used_bytes
        );

        cache
            .write_range(&cache_schedule_key(2), 0, &[0; 64])
            .unwrap();
        let updated = cache.stats();
        assert!(updated.used_bytes > initial.used_bytes);
        for (frame, section) in [
            SettingsSection::Cache,
            SettingsSection::General,
            SettingsSection::Playback,
        ]
        .into_iter()
        .enumerate()
        {
            app.settings_section = section;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(frame as f64 + 10.0),
                    ..Default::default()
                },
                |ui| app.settings_content(ui),
            );
            output.textures_delta.clear();
            // Empty input must never arm a real preference write, even if a
            // future UI regression causes this test to fail during teardown.
            let cache_save = app.cache_save_at.take();
            let volume_save = app.volume_save_at.take();
            assert!(cache_save.is_none());
            assert!(volume_save.is_none());
            app.open_settings(&ctx);
            app.poll_cache(&ctx);
            assert!(app.cache_pending.is_none());
        }
        // The old implementation polled cache size every second while the
        // settings window was open. Cross that interval without changing state.
        thread::sleep(Duration::from_millis(1100));
        for _ in 0..3 {
            app.poll_cache(&ctx);
        }
        assert!(app.cache_pending.is_none());
        assert_eq!(cache_workers(&app).len(), 1);
        assert_eq!(cache_workers(&app)[0], first_worker);
        assert_eq!(
            app.cache_stats.as_ref().unwrap().used_bytes,
            initial.used_bytes
        );

        app.settings_open = false;
        app.open_settings(&ctx);
        assert!(app.cache_pending.is_some());
        assert_ne!(app.workers.last().unwrap().thread().id(), first_worker);
        finish_cache_schedule_job(&mut app, &ctx);
        let snapshot = app.cache_stats.as_ref().unwrap();
        assert_eq!(snapshot.used_bytes, updated.used_bytes);
        assert_eq!(snapshot.limit_bytes, updated.limit_bytes);
        drop(app);
    }

    #[test]
    fn cache_settings_open_reuses_pending_apply_or_clear_result() {
        for (action, expected_limit, clears) in [
            (CacheAction::Apply(2048), 2048, false),
            (CacheAction::Clear, 4096, true),
        ] {
            let (_directory, ctx, mut app, cache) = cache_schedule_app();
            cache
                .write_range(&cache_schedule_key(1), 0, &[0; 64])
                .unwrap();
            app.cache_error = true;
            app.start_cache_job(&ctx, action);
            let operation_worker = cache_workers(&app)[0];
            app.open_settings(&ctx);
            assert!(app.settings_open);
            assert_eq!(cache_workers(&app).len(), 1);
            assert_eq!(cache_workers(&app)[0], operation_worker);
            finish_cache_schedule_job(&mut app, &ctx);
            assert!(!app.cache_error);
            let snapshot = app.cache_stats.as_ref().unwrap();
            assert_eq!(snapshot.limit_bytes, expected_limit);
            if clears {
                assert_eq!(snapshot.used_bytes, 0);
            } else {
                assert!(snapshot.used_bytes > 0);
            }
            app.poll_cache(&ctx);
            app.open_settings(&ctx);
            assert!(app.cache_pending.is_none());
            assert_eq!(cache_workers(&app)[0], operation_worker);
            drop(app);
        }
    }

    #[test]
    fn cache_settings_failed_or_disconnected_job_waits_for_explicit_reopen() {
        for disconnected in [false, true] {
            let (_directory, ctx, mut app, _cache) = cache_schedule_app();
            let (sender, receiver) = mpsc::sync_channel(1);
            app.cache_pending = Some(receiver);
            app.open_settings(&ctx);
            assert!(cache_workers(&app).is_empty());
            if !disconnected {
                sender.send(Err(())).unwrap();
            }
            drop(sender);
            app.poll_cache(&ctx);
            assert!(app.cache_error);
            assert!(app.cache_pending.is_none());
            for section in [SettingsSection::Cache, SettingsSection::General] {
                app.settings_section = section;
                app.open_settings(&ctx);
                app.poll_cache(&ctx);
                assert!(app.cache_pending.is_none());
            }
            assert!(cache_workers(&app).is_empty());
            app.settings_open = false;
            app.open_settings(&ctx);
            assert!(app.cache_pending.is_some());
            finish_cache_schedule_job(&mut app, &ctx);
            assert!(!app.cache_error);
            assert_eq!(app.cache_stats.as_ref().unwrap().used_bytes, 0);
            drop(app);
        }
    }

    fn resume_test_app(ctx: &egui::Context) -> MusicApp {
        let mut app = account_ui_app(ctx, Language::English);
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "User".into(),
                avatar_url: None,
            },
            music: None,
            cache_warning: false,
        });
        app.library.exhausted = true;
        app
    }

    fn resume_test_saved(position: f64) -> playback_resume::Snapshot {
        let mut track = page(1, &[7]).items.remove(0);
        track.title = "Saved current track".into();
        track.artist = "Saved artist".into();
        track.duration = 200;
        track.url = Some("https://audio.example.test/stale?access_token=synthetic".into());
        track.album = Some(AlbumRef {
            id: 8,
            owner_id: -7,
            title: "Saved album".into(),
            access_key: Some("synthetic-stale-album-key".into()),
            cover_url: None,
        });
        track.library_liked = Some(false);
        track.release_key = Some((-7, 8));
        playback_resume::Snapshot {
            track,
            position,
            queue: None,
        }
    }

    #[test]
    fn playback_resume_is_applied_only_after_accepting_the_saved_account_reply() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let session = resume_test_app(&ctx).session.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        app.pending = Some(receiver);
        assert!(app.playing.is_none());
        assert!(
            sender
                .send(Ok(Reply::Connected(
                    Box::new(session),
                    Some(UiError::Music(WebAudioError::Network)),
                    false,
                    Vec::new(),
                    Vec::new(),
                    Some(Box::new(resume_test_saved(42.75))),
                )))
                .is_ok()
        );
        app.poll(&ctx);
        assert_eq!(app.session.as_ref().unwrap().profile.id, 42);
        assert_eq!(app.playback_snapshot().state, PlaybackState::Paused);
        assert_eq!(app.playback_snapshot().position, 42.75);
        assert!(matches!(
            app.audio_error,
            Some(UiError::Music(WebAudioError::Network))
        ));
        assert!(app.workers.is_empty() && app.playback_pending.is_none());
    }

    #[test]
    fn the_queue_an_artist_page_started_comes_back_after_a_restart() {
        let ctx = egui::Context::default();
        let mut app = resume_test_app(&ctx);
        let mut source = page(3, &[6, 7, 8]).items;
        for track in &mut source {
            track.title = format!("Artist song {}", track.id);
        }
        let mut added = page(1, &[90]).items.remove(0);
        added.owner_id = -5;
        app.play_queue = Some(source.clone());
        app.queue_origin = Some("artist:canonical".into());
        app.playing = Some(source[1].clone());
        app.selected = Some(source[1].key());
        app.synchronize_order();
        app.play_order.enqueue(added.key());
        app.queued_tracks.insert(added.key(), added.clone());
        let (owner, saved) = app.playback_resume_snapshot().unwrap();
        let queue = saved.queue.as_ref().unwrap();
        assert_eq!(queue.origin, "artist:canonical");
        assert_eq!(queue.tracks.len(), 3);
        assert_eq!(queue.added[0].key(), added.key());
        // A fresh start of the app with that snapshot.
        let mut restarted = resume_test_app(&ctx);
        restarted.restore_playback_resume(owner, Some(saved));
        assert_eq!(restarted.queue_origin.as_deref(), Some("artist:canonical"));
        let keys: Vec<_> = restarted
            .play_queue
            .as_ref()
            .unwrap()
            .iter()
            .map(Track::key)
            .collect();
        assert_eq!(keys, source.iter().map(Track::key).collect::<Vec<_>>());
        assert_eq!(restarted.playing.as_ref().unwrap().key(), source[1].key());
        let upcoming: Vec<_> = restarted
            .play_order
            .upcoming(Some(source[1].key()), false)
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        assert_eq!(upcoming, [added.key(), source[2].key()]);
        assert_eq!(restarted.player.snapshot().state, PlaybackState::Stopped);
        assert!(restarted.workers.is_empty());
    }

    #[test]
    fn playback_resume_restores_a_visible_paused_track_without_audio_or_resolution() {
        let ctx = egui::Context::default();
        let mut app = resume_test_app(&ctx);
        app.restore_playback_resume(43, Some(resume_test_saved(42.75)));
        assert!(
            app.playing.is_none(),
            "A different account must not restore its track"
        );
        app.restore_playback_resume(42, Some(resume_test_saved(42.75)));
        assert_eq!(app.player.snapshot().state, PlaybackState::Stopped);
        let shown = app.playback_snapshot();
        assert_eq!(shown.state, PlaybackState::Paused);
        assert_eq!(shown.position, 42.75);
        assert_eq!(app.playing.as_ref().unwrap().key(), (1, 7));
        assert!(app.playing.as_ref().unwrap().url.is_none());
        assert!(
            app.playing
                .as_ref()
                .unwrap()
                .album
                .as_ref()
                .unwrap()
                .access_key
                .is_none()
        );
        assert_eq!(app.playing.as_ref().unwrap().library_liked, Some(false));
        assert!(app.playback_pending.is_none() && app.metadata_request.is_none());
        assert!(app.workers.is_empty());
        assert_eq!(app.player.decoder_starts(), 0);
        let mut frame = 0;
        let size = egui::vec2(1040.0, 720.0);
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        assert!(account_ui_text_rect(&output, "Saved current track").is_some());
        assert!(account_ui_text_rect(&output, "0:42").is_some());
        assert!(account_ui_text_rect(&output, "3:20").is_some());
        assert!(app.playback_pending.is_none() && app.workers.is_empty());
        let (owner, saved) = app.playback_resume_snapshot().unwrap();
        assert_eq!(owner, 42);
        assert_eq!(saved.position, 42.75);
        assert_eq!(saved.track.release_key, Some((-7, 8)));
        app.reset();
        assert!(
            app.playback_resume_snapshot().is_none(),
            "Logging out must not overwrite a saved file with empty state"
        );
    }

    #[test]
    fn playback_resume_play_keeps_the_offset_through_transient_resolution_failure_and_success() {
        let ctx = egui::Context::default();
        let mut app = resume_test_app(&ctx);
        app.restore_playback_resume(42, Some(resume_test_saved(76.25)));
        // Play finds the saved track without opening audio. A transient network
        // failure keeps its resume offset; a permanent missing stream is skipped.
        app.toggle_playback(&ctx);
        assert!(
            app.playback_pending.is_some(),
            "The restored track can be found outside the library"
        );
        assert_eq!(app.resume_position, Some(76.25));
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 76.25);
        for worker in app.workers.drain(..) {
            worker.join().unwrap();
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Err(UiError::Music(WebAudioError::Network)))
            .ok()
            .unwrap();
        app.playback_pending = Some(receiver);
        app.poll_playback(&ctx);
        assert!(matches!(
            app.playback_error,
            Some(UiError::Music(WebAudioError::Network))
        ));
        assert_eq!(app.resume_position, Some(76.25));
        assert_eq!(app.playback_snapshot().state, PlaybackState::Paused);

        let (player, _commands) = Player::test_paused(200, 0.0);
        app.player = player;
        app.player.stop();
        let (sender, receiver) = mpsc::sync_channel(1);
        app.playback_pending = Some(receiver);
        assert!(
            sender
                .send(Ok(ResolvedTrack {
                    track: app.playing.clone().unwrap(),
                    url: None,
                    cache_key: CacheKey {
                        account_id: 42,
                        owner_id: 1,
                        track_id: 7,
                        duration: 200
                    },
                }))
                .is_ok()
        );
        app.poll_playback(&ctx);
        assert!(app.resume_position.is_none());
        assert_eq!(app.player.snapshot().state, PlaybackState::Loading);
        assert_eq!(app.player.snapshot().position, 76.25);
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 76.25);
        assert_eq!(
            app.player.decoder_starts(),
            0,
            "The synthetic player never opens audio"
        );
    }

    #[test]
    fn playback_resume_seek_saves_only_the_released_position_and_supports_keys() {
        let ctx = egui::Context::default();
        let mut app = resume_test_app(&ctx);
        app.restore_playback_resume(42, Some(resume_test_saved(40.0)));
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let rail = seek_track_in_player(&output, size.y);
        let point = egui::pos2(egui::lerp(rail.x_range(), 0.75), rail.center().y);
        account_ui_frame(&ctx, &mut app, size, &mut frame, seek_pointer(point, true));
        assert!(app.seek_position.is_some());
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 40.0);
        assert_eq!(app.resume_position, Some(40.0));
        account_ui_frame(&ctx, &mut app, size, &mut frame, seek_pointer(point, false));
        let released = app.resume_position.unwrap();
        assert!((released - 150.0).abs() < 1.0);
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, released);
        assert!(app.seek_position.is_none());
        app.seek_by(5.0);
        assert!((app.resume_position.unwrap() - released - 5.0).abs() < 1e-6);
        app.previous_track(&ctx, true);
        assert_eq!(app.resume_position, Some(0.0));
        assert_eq!(app.player.snapshot().state, PlaybackState::Stopped);
        assert_eq!(app.player.decoder_starts(), 0);
        assert!(app.workers.is_empty());
    }

    #[test]
    fn playback_resume_preserves_unknown_duration_and_clamps_known_duration() {
        let ctx = egui::Context::default();
        let mut app = resume_test_app(&ctx);
        let mut saved = resume_test_saved(71.25);
        saved.track.duration = 0;
        app.restore_playback_resume(42, Some(saved));
        assert_eq!(app.resume_position, Some(71.25));
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 71.25);
        let mut frame = 0;
        let size = egui::vec2(1040.0, 720.0);
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            app.seek_position = Some(599.0); // Disabled previews cannot replace committed time.
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        assert!(account_ui_text_rect(&output, "1:11").is_some());
        assert!(account_ui_text_rect(&output, "0:00").is_some());
        assert!(account_ui_text_rect(&output, "0:01").is_none());
        assert_eq!(app.resume_position, Some(71.25));
        assert!(app.seek_position.is_none());
        assert!(app.time_slots.unwrap().width < 48.0);
        app.restore_playback_resume(42, Some(resume_test_saved(251.25)));
        assert_eq!(app.resume_position, Some(200.0));
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 200.0);
    }

    #[test]
    fn playback_resume_snapshot_captures_live_committed_time_while_loading() {
        let ctx = egui::Context::default();
        let mut app = resume_test_app(&ctx);
        let saved = resume_test_saved(40.0);
        app.playing = Some(saved.track);
        let (player, _commands) = Player::test_paused(200, 51.25);
        app.player = player;
        app.seek_position = Some(123.0);
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 51.25);
        app.player.play_cached(
            CacheKey {
                account_id: 42,
                owner_id: 1,
                track_id: 7,
                duration: 200,
            },
            None,
            65.5,
        );
        assert_eq!(app.player.snapshot().state, PlaybackState::Loading);
        assert_eq!(app.playback_resume_snapshot().unwrap().1.position, 65.5);
    }

    pub(super) fn page(count: usize, ids: &[i64]) -> AudioPage {
        AudioPage {
            count: Some(count),
            exhausted: None,
            items: ids
                .iter()
                .map(|id| Track {
                    explicit: false,
                    added: None,
                    unavailable: None,
                    id: *id,
                    owner_id: 1,
                    artist: "A".into(),
                    title: "T".into(),
                    duration: 1,
                    url: None,
                    cover_url: None,
                    subtitle: None,
                    artists: Vec::new(),
                    album: None,
                    library_liked: None,
                    release_key: None,
                    library_source_key: None,
                })
                .collect(),
        }
    }

    pub(super) fn search_test_app(ctx: &egui::Context) -> MusicApp {
        let mut app = account_ui_app(ctx, Language::English);
        app.demo = true;
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "User".into(),
                avatar_url: None,
            },
            music: None,
            cache_warning: false,
        });
        app
    }

    #[test]
    fn links_from_later_launches_wait_for_music_and_open_the_newest_release() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        let activation = crate::single_instance::Activation::default();
        app.attach_instance(activation.clone());
        activation.open_link("https://vk.ru/audio1_2");
        activation.open_link("https://example.com/not-music");
        activation.open_link("https://vk.ru/music/album/-2000_5_ab");
        app.poll_links(&ctx);
        // No music session yet: the newest music link waits for it.
        let album = AlbumRef {
            id: 5,
            owner_id: -2000,
            title: String::new(),
            access_key: Some("ab".into()),
            cover_url: None,
        };
        assert_eq!(
            app.pending_link,
            Some(crate::link_router::Target::Album(album.clone()))
        );
        assert!(activation.take_links().is_empty());
        assert!(app.page != MusicPage::Album(album.clone()));
        assert!(app.workers.is_empty());
        assert!(
            link_page(crate::link_router::Target::Album(album.clone())) == MusicPage::Album(album)
        );
        assert!(
            link_page(crate::link_router::Target::Track((1, 2)))
                == MusicPage::TrackAlbum((1, 2), String::new())
        );
        let artist = ArtistRef {
            id: "synthetic".into(),
            name: "synthetic".into(),
        };
        assert!(
            link_page(crate::link_router::Target::Artist(artist.clone()))
                == MusicPage::Artist(artist)
        );
    }

    #[test]
    fn a_linked_track_plays_as_its_own_queue_instead_of_opening_its_release() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        app.library.tracks = page(2, &[1, 2]).items;
        let mut linked = page(1, &[7]).items.remove(0);
        linked.owner_id = -2001;
        linked.title = "Linked track".into();
        let page_before = app.page.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        app.link_track = Some(LinkTrackRequest {
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.poll_link_track(&ctx);
        assert!(app.link_track.is_some(), "waits for the lookup");
        sender.send(Ok(linked.clone())).unwrap();
        app.poll_link_track(&ctx);
        assert!(app.link_track.is_none());
        assert_eq!(app.playing.as_ref().map(Track::key), Some(linked.key()));
        assert_eq!(
            app.play_queue
                .as_deref()
                .map(|queue| queue.iter().map(Track::key).collect::<Vec<_>>()),
            Some(vec![linked.key()])
        );
        assert_eq!(app.queue_origin.as_deref(), Some("link:-2001_7"));
        assert!(app.page == page_before, "the page stays where it was");

        let (sender, receiver) = mpsc::sync_channel(1);
        app.link_track = Some(LinkTrackRequest {
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        sender.send(Err(WebAudioError::NoStream)).unwrap();
        app.poll_link_track(&ctx);
        assert_eq!(app.track_menu_error, Some(WebAudioError::NoStream));
        assert_eq!(app.playing.as_ref().map(Track::key), Some(linked.key()));
    }

    #[test]
    fn exit_leaves_pending_reads_behind_but_waits_for_saves() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let saved = Arc::new(AtomicBool::new(false));
        let done = saved.clone();
        app.workers.push(
            thread::Builder::new()
                .name("vk-search".into())
                .spawn(|| thread::sleep(Duration::from_secs(5)))
                .unwrap(),
        );
        app.workers.push(
            thread::Builder::new()
                .name("vk-recent-cache".into())
                .spawn(move || {
                    thread::sleep(Duration::from_millis(100));
                    done.store(true, Ordering::Release);
                })
                .unwrap(),
        );
        let started = Instant::now();
        drop(app);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(saved.load(Ordering::Acquire), "a save is not cut off");
        assert!(read_only_worker(Some("vk-home")));
        assert!(!read_only_worker(Some("vk-playlist-add")));
        assert!(!read_only_worker(None));
    }

    #[test]
    fn link_handler_switch_describes_the_browser_and_waits_for_windows_confirmation() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.language = Language::Russian;
        app.settings_open = true;
        app.settings_section = SettingsSection::General;
        app.link_handler = Some(crate::link_router::Status {
            enabled: true,
            active: false,
            browser: Some("Helium".into()),
        });
        let mut frame = 0;
        let mut output = account_ui_frame(
            &ctx,
            &mut app,
            egui::vec2(1040.0, 720.0),
            &mut frame,
            Vec::new(),
        );
        for _ in 0..2 {
            output = account_ui_frame(
                &ctx,
                &mut app,
                egui::vec2(1040.0, 720.0),
                &mut frame,
                Vec::new(),
            );
        }
        assert!(account_ui_text_rect(&output, "Открывать ссылки VK Музыки в VK Fast").is_some());
        let texts: Vec<String> = account_ui_shapes(&output)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text.ends_with("остальные ссылки — в Helium.")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("Осталось выбрать VK Fast"))
        );
        app.link_handler.as_mut().unwrap().active = true;
        for _ in 0..2 {
            output = account_ui_frame(
                &ctx,
                &mut app,
                egui::vec2(1040.0, 720.0),
                &mut frame,
                Vec::new(),
            );
        }
        assert!(account_ui_text_rect(&output, "Открыть параметры Windows").is_none());
        assert!(SettingsQuery::new("ссылки браузер").matches(Setting::LinkHandler));
        assert!(SettingsQuery::new("open links").matches(Setting::LinkHandler));
    }

    #[test]
    fn expired_music_key_keeps_profile_and_cached_tracks_across_library_home_and_playlists() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.tracks = page(2, &[1, 2]).items;
        app.recent = page(1, &[3]).items;
        let (sender, receiver) = mpsc::sync_channel(1);
        app.pending = Some(receiver);
        sender
            .send(Err(UiError::Music(WebAudioError::TokenExpired)))
            .unwrap();
        app.poll(&ctx);
        assert!(matches!(
            app.audio_error,
            Some(UiError::Music(WebAudioError::TokenExpired))
        ));

        let (sender, receiver) = mpsc::sync_channel(1);
        app.home_request = Some(HomeRequest {
            owner: 42,
            collection: None,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        sender.send(Err(WebAudioError::TokenExpired)).unwrap();
        app.poll_home(&ctx);
        assert_eq!(app.home_error, Some(WebAudioError::TokenExpired));

        let (sender, receiver) = mpsc::sync_channel(1);
        app.playlist_request = Some(PlaylistRequest {
            owner: 42,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        sender.send(Err(WebAudioError::TokenExpired)).unwrap();
        app.poll_playlists(&ctx);
        assert_eq!(app.playlist_error, Some(WebAudioError::TokenExpired));
        assert_eq!(app.session.as_ref().unwrap().profile.id, 42);
        assert_eq!(app.library.tracks.len(), 2);
        assert_eq!(app.recent.len(), 1);
        assert!(app.auth_error.is_none());
    }

    fn like_test_page(count: usize, ids: &[i64]) -> AudioPage {
        let mut result = page(count, ids);
        for track in &mut result.items {
            track.owner_id = 42;
        }
        result
    }

    fn like_reply(
        owner: i64,
        source: Track,
        result: Result<LibraryChange, WebAudioError>,
    ) -> LikeRequest {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        LikeRequest {
            owner,
            source,
            restore_index: None,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    #[test]
    fn library_hearts_wait_for_vk_and_map_the_new_owned_copy_without_changing_playback() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library = Library::from_cached(like_test_page(2, &[1, 2]).items);
        app.library.offset = 100;
        let mut source = like_test_page(1, &[9]).items.remove(0);
        source.owner_id = -10;
        source.library_liked = Some(false);
        app.play_queue = Some(vec![source.clone()]);
        app.queue_origin = Some("search:query".into());
        app.playing = Some(source.clone());
        app.recent = vec![source.clone()];
        assert!(!app.track_is_liked(&source));
        let (sender, receiver) = mpsc::sync_channel(1);
        app.like_request = Some(LikeRequest {
            owner: 42,
            source: source.clone(),
            restore_index: None,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.poll_like(&ctx);
        assert_eq!(app.library.tracks.len(), 2);
        assert!(!app.track_is_liked(&source));
        sender
            .send(Ok(LibraryChange::Added((42, 999))))
            .ok()
            .unwrap();
        app.poll_like(&ctx);
        assert!(app.like_request.is_none());
        assert_eq!(app.library.tracks[0].key(), (42, 999));
        assert_eq!(app.library.tracks[0].release_key, Some(source.key()));
        assert!(app.library.tracks[0].url.is_none());
        assert_eq!(app.library.offset, 100);
        assert!(app.track_is_liked(&source));
        assert_eq!(app.playing.as_ref().unwrap().key(), source.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), source.key());
        assert_eq!(app.recent[0].library_liked, Some(true));
        assert_eq!(app.owned_library_key(&source), Some((42, 999)));
        app.apply_library_change(&ctx, &source, LibraryChange::Added((42, 999)));
        assert_eq!(
            app.library
                .tracks
                .iter()
                .filter(|track| track.key() == (42, 999))
                .count(),
            1
        );
    }

    #[test]
    fn removing_playing_library_track_keeps_queue_and_rejects_stale_refresh_rows() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.refresh(like_test_page(100, &[1, 2, 3]));
        let mut original = app.library.tracks[0].clone();
        original.library_liked = Some(true);
        original.release_key = Some((-5, 71));
        app.library.tracks[0] = original.clone();
        app.playing = Some(original.clone());
        app.selected = Some(original.key());
        app.recent = vec![original.clone()];
        app.queue_origin = Some("library".into());
        app.synchronize_order();
        app.apply_library_change(&ctx, &original, LibraryChange::Removed(vec![(42, 1)]));
        assert_eq!(
            app.library
                .tracks
                .iter()
                .map(Track::key)
                .collect::<Vec<_>>(),
            vec![(42, 2), (42, 3)]
        );
        assert_eq!(app.playing.as_ref().unwrap().key(), (42, 1));
        assert_eq!(app.playing.as_ref().unwrap().library_liked, Some(false));
        assert_eq!(app.recent[0].library_liked, Some(false));
        assert_eq!(app.adjacent_track(true), Some((42, 2)));
        assert!(app.library_queue_active());
        assert!(!app.allow_queue_wrap());
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(Reply::Audio(
                like_test_page(100, &[1, 2, 3, 4]),
                true,
                None,
            )))
            .ok()
            .unwrap();
        app.pending = Some(receiver);
        app.poll(&ctx);
        assert_eq!(
            app.library.offset, 4,
            "Pagination must count even a hidden stale row"
        );
        assert_eq!(app.library.fresh_prefix, 3);
        assert!(
            !app.library
                .tracks
                .iter()
                .any(|track| track.key() == (42, 1))
        );
        assert!(
            app.play_queue
                .as_ref()
                .unwrap()
                .iter()
                .any(|track| track.key() == (42, 4))
        );
        assert!(!app.track_is_liked(&original));
        app.apply_library_change(&ctx, &original, LibraryChange::Added(original.key()));
        assert!(app.track_is_liked(&original));
        assert_eq!(app.library.tracks[0].key(), original.key());
    }

    #[test]
    fn error_notices_hide_by_themselves_and_short_notices_fit_their_text() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let size = egui::vec2(1100.0, 760.0);
        let mut frame = 0;
        app.track_menu_error = Some(WebAudioError::Network);
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..2 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let until = app.error_closing.expect("errors hide by themselves");
        assert!(until > Instant::now() + Duration::from_secs(5));
        let color = |label: &str| {
            account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.job.sections[0].format.color)
                    }
                    _ => None,
                })
        };
        let title = color("Could not complete the action").unwrap();
        let body = color(&WebAudioError::Network.localized(Language::English)).unwrap();
        assert_eq!((title, body), (Color32::WHITE, MUTED));
        // The same error keeps its time; it ends with a fade.
        account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        assert_eq!(app.error_closing, Some(until));
        app.error_closing = Some(Instant::now());
        account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        assert!(app.track_menu_error.is_none() && app.error_closing.is_none());
        // A short notice is its text plus the frame's margins.
        app.playlist_notice("Link copied");
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..2 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let text = account_ui_text_rect(&output, "Link copied").unwrap();
        let area = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("track-menu-notice")))
            .unwrap();
        assert!(
            area.height() <= text.height() + 2.0 * 12.0 + 6.0,
            "{area:?} for {text:?}"
        );
    }

    #[test]
    fn removal_notice_offers_undo_that_restores_the_row_in_place() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.refresh(like_test_page(3, &[1, 2, 3]));
        let track = app.library.tracks[1].clone();
        app.like_request = Some(like_reply(
            42,
            track.clone(),
            Ok(LibraryChange::Removed(vec![track.key()])),
        ));
        app.poll_like(&ctx);
        assert_eq!(app.library.tracks.len(), 2);
        let (_, until) = app.track_menu_notice.clone().unwrap();
        let undo = app.library_undo.as_ref().unwrap();
        assert_eq!(undo.until, until, "the undo belongs to this notice");
        assert_eq!((undo.row.key(), undo.index), (track.key(), Some(1)));
        // A newer notice hides the button; its own expiry drops the undo.
        app.playlist_notice("Link copied");
        assert_ne!(app.track_menu_notice.as_ref().unwrap().1, until);
        let undo = app.library_undo.take().unwrap();
        let mut request = like_reply(42, undo.row, Ok(LibraryChange::Added(track.key())));
        request.restore_index = undo.index;
        app.like_request = Some(request);
        app.poll_like(&ctx);
        let keys: Vec<_> = app.library.tracks.iter().map(Track::key).collect();
        assert_eq!(keys, [(42, 1), (42, 2), (42, 3)]);
        assert!(app.track_is_liked(&track));
        assert!(app.library_undo.is_none(), "restoring offers no new undo");
        app.reset();
        assert!(app.library_undo.is_none() && app.error_closing.is_none());
    }

    #[test]
    fn my_music_dates_are_when_the_copy_was_added_not_the_sources() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        // A track from someone's playlist: its date is when it joined that
        // playlist, or its release.
        let mut source = like_test_page(1, &[9]).items.remove(0);
        source.owner_id = -10;
        source.added = Some(1_500_000_000);
        let before = chrono::Utc::now().timestamp();
        app.apply_library_change(&ctx, &source, LibraryChange::Added((42, 77)));
        let saved = app
            .library
            .tracks
            .iter()
            .find(|track| track.key() == (42, 77))
            .unwrap();
        assert!(saved.added.is_some_and(|added| added >= before));
        // VK's own date for the copy replaces a wrong one in My music, but a
        // playlist row keeps the date it joined that playlist.
        let mut wrong = like_test_page(1, &[5]).items.remove(0);
        wrong.added = Some(1_400_000_000);
        app.library.tracks.push(wrong.clone());
        let album = AlbumRef {
            owner_id: 42,
            id: 3,
            title: "Mine".into(),
            access_key: None,
            cover_url: None,
        };
        let mut in_playlist = wrong.clone();
        in_playlist.added = Some(1_450_000_000);
        app.details.insert(
            MusicPage::Playlist(album.clone()).detail_key().unwrap(),
            Arc::new(DetailPage::Playlist(crate::web_audio::AlbumPage {
                album,
                artists: Vec::new(),
                year: None,
                description: None,
                creator: None,
                tracks: vec![in_playlist],
                total_tracks: Some(1),
                stats: crate::web_audio::AlbumStats::default(),
            })),
        );
        let mut fresh = wrong.clone();
        fresh.added = Some(1_700_000_000);
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Ok(vec![fresh])).ok().unwrap();
        app.metadata_request = Some(MetadataRequest {
            owner: 42,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.poll_metadata(&ctx);
        let library = app
            .library
            .tracks
            .iter()
            .find(|track| track.key() == wrong.key())
            .unwrap();
        assert_eq!(library.added, Some(1_700_000_000));
        let DetailPage::Playlist(page) = app.details.values().next().unwrap().as_ref() else {
            panic!()
        };
        assert_eq!(page.tracks[0].added, Some(1_450_000_000));
    }

    #[test]
    fn library_like_errors_and_previous_account_replies_leave_membership_unchanged() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.refresh(like_test_page(1, &[1]));
        let track = app.library.tracks[0].clone();
        app.like_request = Some(like_reply(42, track.clone(), Err(WebAudioError::Network)));
        app.poll_like(&ctx);
        assert_eq!(app.like_error, Some(WebAudioError::Network));
        assert!(app.track_is_liked(&track));
        app.like_request = Some(like_reply(
            99,
            track.clone(),
            Ok(LibraryChange::Removed(vec![track.key()])),
        ));
        app.poll_like(&ctx);
        assert_eq!(app.library.tracks.len(), 1);
        assert!(app.removed_from_library.is_empty());
        let request = like_reply(42, track, Ok(LibraryChange::Added((42, 8))));
        let canceled = request.canceled.clone();
        app.like_request = Some(request);
        app.reset();
        assert!(canceled.load(Ordering::Acquire));
        assert!(app.like_request.is_none());
        assert!(app.like_aliases.is_empty());
    }

    #[test]
    fn fresh_membership_replaces_cached_flags_but_cannot_undo_a_newer_local_change() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let mut source = like_test_page(1, &[9]).items.remove(0);
        source.owner_id = -8;
        source.library_liked = Some(false);
        app.recent = vec![source.clone()];
        let mut fresh = source.clone();
        fresh.library_liked = Some(true);
        for locally_removed in [false, true] {
            if locally_removed {
                app.removed_from_library.insert(source.key());
            }
            let (sender, receiver) = mpsc::sync_channel(1);
            sender.send(Ok(vec![fresh.clone()])).ok().unwrap();
            app.metadata_request = Some(MetadataRequest {
                owner: 42,
                canceled: Arc::new(AtomicBool::new(false)),
                receiver,
            });
            app.poll_metadata(&ctx);
            assert_eq!(app.recent[0].library_liked, Some(!locally_removed));
        }
        app.removed_from_library.clear();
        app.recent[0].library_liked = Some(false);
        app.apply_library_change(&ctx, &source, LibraryChange::AlreadyAdded);
        assert!(
            app.library.tracks.is_empty(),
            "Unknown owned identities must never be invented"
        );
        assert_eq!(app.recent[0].library_liked, Some(true));
        assert!(app.library_refresh_due);
    }

    #[test]
    fn hearts_recover_confirmed_source_identity_and_preserve_explicit_unliked_history() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let mut owned = like_test_page(1, &[1]).items.remove(0);
        owned.release_key = Some((-6, 99));
        owned.library_source_key = Some((-6, 99));
        app.library.tracks = vec![owned.clone()];
        let mut source = owned.clone();
        source.owner_id = -6;
        source.id = 99;
        source.release_key = None;
        source.library_source_key = None;
        assert_eq!(app.owned_library_key(&source), Some(owned.key()));
        assert!(app.track_is_liked(&source));
        let mut unrelated = source.clone();
        unrelated.id = 100;
        assert!(
            !app.track_is_liked(&unrelated),
            "Matching text must never imply membership"
        );
        app.apply_library_change(&ctx, &source, LibraryChange::Removed(vec![owned.key()]));
        owned.library_liked = Some(false);
        app.removed_from_library.clear(); // Simulate a restart with persisted recent metadata.
        assert!(!app.track_is_liked(&owned));
    }

    #[test]
    fn row_hearts_are_left_of_duration_and_do_not_trigger_the_row_play_action() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        app.library.refresh(like_test_page(1, &[1]));
        let track = app.library.tracks[0].clone();
        let mut action = None;
        let mut tick = 0;
        let mut frame = |app: &mut MusicApp, events: Vec<egui::Event>| {
            tick += 1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 100.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = app.track_row(ui, &track, 0, false, track_sort::Extra::default());
                },
            );
            let duration = account_ui_text_rect(&output, &track.duration_label());
            // The heart has a fixed 22px hitbox, six pixels before the duration.
            let position = duration
                .map(|time| egui::pos2(time.left() - 17.0, time.center().y))
                .unwrap();
            output.textures_delta.clear();
            position
        };
        frame(&mut app, Vec::new());
        let position = frame(&mut app, Vec::new());
        frame(&mut app, seek_pointer(position, true));
        frame(&mut app, seek_pointer(position, false));
        // Disabled until a verified music session exists, and never falls
        // through to starting the row when clicked in that state.
        assert!(action.is_none());
        assert!(app.playing.is_none());
        app.session.as_mut().unwrap().music = Some(
            WebAudioClient::new(vec![SessionCookie {
                domain: ".vk.ru".into(),
                path: "/".into(),
                name: "remixsid".into(),
                value: "synthetic-session".into(),
                secure: true,
                expires_unix: None,
            }])
            .unwrap(),
        );
        // A new closure avoids holding the previous frame's action borrow.
        let mut enabled_action = None;
        for (tick, events) in [
            Vec::new(),
            seek_pointer(position, true),
            seek_pointer(position, false),
        ]
        .into_iter()
        .enumerate()
        {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 100.0),
                    )),
                    time: Some(1.0 + tick as f64 * 0.1),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    enabled_action =
                        app.track_row(ui, &track, 0, false, track_sort::Extra::default());
                },
            );
            output.textures_delta.clear();
        }
        assert!(
            matches!(enabled_action,Some(TrackAction::ToggleLike(ref selected)) if selected.key()==track.key())
        );
        assert!(app.like_request.is_none() && app.playing.is_none());
    }

    #[test]
    fn track_menus_open_on_child_controls_in_rows_and_player_and_copy_the_original_link() {
        for footer in [false, true] {
            for target in 0..4 {
                let ctx = egui::Context::default();
                let mut app = search_test_app(&ctx);
                let mut track = page(1, &[71]).items.remove(0);
                track.title = "Context song".into();
                track.artist = "Context artist".into();
                track.artists = vec![ArtistRef {
                    id: "context-artist".into(),
                    name: track.artist.clone(),
                }];
                track.release_key = Some((-7, 99));
                track.url = Some("https://example.invalid/private-stream".into());
                app.library.tracks.push(track.clone());
                app.library.exhausted = true;
                app.playing = Some(track.clone());
                app.play_queue = Some(vec![track.clone()]);
                let size = egui::vec2(1040.0, 720.0);
                let mut frame = 0;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let label_rect = |label: &str| {
                    let mut rects: Vec<_> = account_ui_shapes(&output)
                        .into_iter()
                        .filter_map(|shape| match shape {
                            egui::Shape::Text(text) if text.galley.text() == label => {
                                Some(text.visual_bounding_rect())
                            }
                            _ => None,
                        })
                        .collect();
                    rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
                    *if footer { rects.last() } else { rects.first() }.unwrap()
                };
                let title = label_rect(&track.title);
                let position = match target {
                    0 => title.center(),
                    1 => label_rect(&track.artist).center(),
                    2 => title.left_center() - egui::vec2(25.0, 0.0),
                    _ if footer => title.right_center() + egui::vec2(17.0, 0.0),
                    _ => {
                        let duration = label_rect(&track.duration_label());
                        duration.left_center() - egui::vec2(17.0, 0.0)
                    }
                };
                for pressed in [true, false] {
                    account_ui_frame(
                        &ctx,
                        &mut app,
                        size,
                        &mut frame,
                        vec![
                            egui::Event::PointerMoved(position),
                            egui::Event::PointerButton {
                                pos: position,
                                button: egui::PointerButton::Secondary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    );
                }
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                // In the player only the title and artists open the menu,
                // not the cover or the heart next to them.
                if footer && target >= 2 {
                    assert!(account_ui_text_rect(&output, "Copy link").is_none());
                    continue;
                }
                for label in ["Add to queue", "Go to artist", "Go to album", "Copy link"] {
                    assert!(
                        account_ui_text_rect(&output, label).is_some(),
                        "Missing {label}, footer={footer}, target={target}"
                    );
                }
                let copy = account_ui_text_rect(&output, "Copy link").unwrap().center();
                let copied = account_ui_click(&ctx, &mut app, size, &mut frame, copy);
                assert!(copied.platform_output.commands.iter().any(|command| matches!(command,
                    egui::OutputCommand::CopyText(text) if text == "https://vk.ru/audio-7_99"
                )));
                assert!(app.page == MusicPage::Library);
                assert!(app.selected.is_none());
                assert_eq!(app.playing.as_ref().unwrap().key(), track.key());
                assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), track.key());
                assert!(app.like_request.is_none() && app.playlist_add_request.is_none());
            }
        }
    }

    #[test]
    fn disabled_track_rows_cannot_open_menus_during_a_search_transition() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let track = page(1, &[71]).items.remove(0);
        for tick in 0..5 {
            let point = egui::pos2(130.0, 30.0);
            let events = if tick == 2 || tick == 3 {
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Secondary,
                        pressed: tick == 2,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            } else {
                Vec::new()
            };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 400.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.add_enabled_ui(false, |ui| {
                        assert!(
                            app.track_row(ui, &track, 0, false, track_sort::Extra::default())
                                .is_none()
                        );
                    });
                },
            );
            assert!(account_ui_text_rect(&output, "Copy link").is_none());
            assert!(!egui::Popup::is_any_open(&ctx));
            output.textures_delta.clear();
        }
    }

    fn playlist_add_reply(
        owner: i64,
        source: Track,
        target: AlbumRef,
        result: Result<(i64, i64), WebAudioError>,
    ) -> PlaylistAddRequest {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        PlaylistAddRequest {
            owner,
            source,
            target,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    #[test]
    fn playlist_addition_accepts_only_current_account_results_and_preserves_playback() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let mut source = page(1, &[9]).items.remove(0);
        source.library_liked = Some(false);
        let mut target = search_test_playlist(14).album;
        target.owner_id = 42;
        app.playing = Some(source.clone());
        app.play_queue = Some(vec![source.clone()]);
        app.queue_origin = Some("search:context".into());
        app.library = Library::from_cached(like_test_page(1, &[1]).items);
        for (owner, canceled, result) in [
            (43, false, Ok((43, 999))),
            (42, true, Ok((42, 999))),
            (42, false, Err(WebAudioError::Rejected)),
        ] {
            let request = playlist_add_reply(owner, source.clone(), target.clone(), result);
            request.canceled.store(canceled, Ordering::Release);
            app.playlist_add_request = Some(request);
            app.poll_playlist_add(&ctx);
            assert_eq!(app.library.tracks.len(), 1);
            assert_eq!(app.library.tracks[0].id, 1);
            assert!(!app.track_is_liked(&source));
        }
        assert!(matches!(
            app.track_menu_error,
            Some(WebAudioError::Rejected)
        ));
        app.track_menu_error = None;
        app.playlist_add_request = Some(playlist_add_reply(
            42,
            source.clone(),
            target.clone(),
            Ok((42, 999)),
        ));
        app.poll_playlist_add(&ctx);
        assert_eq!(app.library.tracks[0].key(), (42, 999));
        assert_eq!(app.library.tracks[0].release_key, Some(source.key()));
        assert!(app.track_is_liked(&source));
        assert!(app.track_menu_notice.is_some());
        assert_eq!(app.playing.as_ref().unwrap().key(), source.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), source.key());
        assert_eq!(app.queue_origin.as_deref(), Some("search:context"));
        let request = playlist_add_reply(42, source, target, Ok((42, 999)));
        let canceled = request.canceled.clone();
        app.playlist_add_request = Some(request);
        app.reset();
        assert!(canceled.load(Ordering::Acquire));
        assert!(app.playlist_add_request.is_none());
        assert!(app.editable_playlists.is_empty() && app.track_menu_notice.is_none());
    }

    fn mix_reply(
        owner: i64,
        start: bool,
        seed: Option<MixSeed>,
        result: Result<MixPage, WebAudioError>,
    ) -> MixRequest {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        MixRequest {
            owner,
            start,
            seed,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    #[test]
    fn track_mix_switches_source_only_after_success_and_rejects_unrelated_continuations() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let original = page(1, &[1]).items.remove(0);
        app.playing = Some(original.clone());
        app.play_queue = Some(vec![original]);
        app.queue_origin = Some("mix".into());
        app.mix_active = true;
        app.mix_has_more = true;
        let seed = Some(MixSeed::Track((-7, 71)));
        for result in [
            Err(WebAudioError::Network),
            Ok(MixPage {
                tracks: Vec::new(),
                has_more: false,
            }),
        ] {
            app.mix_request = Some(mix_reply(42, true, seed, result));
            app.poll_mix(&ctx);
            assert!(app.mix_active && app.mix_has_more);
            assert_eq!(app.mix_seed, None);
            assert_eq!(app.queue_origin.as_deref(), Some("mix"));
            assert_eq!(app.play_queue.as_ref().unwrap()[0].id, 1);
        }
        app.mix_request = Some(mix_reply(
            42,
            true,
            seed,
            Ok(MixPage {
                tracks: page(2, &[10, 11]).items,
                has_more: true,
            }),
        ));
        app.poll_mix(&ctx);
        assert_eq!(app.mix_seed, seed);
        assert_eq!(app.queue_origin.as_deref(), Some("track-mix:-7_71"));
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 2);
        app.mix_request = Some(mix_reply(
            42,
            false,
            None,
            Ok(MixPage {
                tracks: page(1, &[99]).items,
                has_more: false,
            }),
        ));
        app.poll_mix(&ctx);
        assert!(app.mix_has_more);
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 2);
        app.mix_request = Some(mix_reply(
            42,
            false,
            seed,
            Ok(MixPage {
                tracks: page(2, &[11, 12]).items,
                has_more: false,
            }),
        ));
        app.poll_mix(&ctx);
        assert!(!app.mix_has_more);
        assert_eq!(
            app.play_queue
                .as_ref()
                .unwrap()
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [10, 11, 12]
        );
        let request = mix_reply(
            42,
            true,
            None,
            Ok(MixPage {
                tracks: page(1, &[99]).items,
                has_more: true,
            }),
        );
        request.canceled.store(true, Ordering::Release);
        app.mix_request = Some(request);
        app.poll_mix(&ctx);
        assert_eq!(app.mix_seed, seed);
        assert_eq!(app.queue_origin.as_deref(), Some("track-mix:-7_71"));
    }

    #[test]
    fn track_click_mode_changes_row_activation_but_hover_play_always_works_once() {
        for double_click in [false, true] {
            for index_button in [false, true] {
                let ctx = egui::Context::default();
                let mut app = account_ui_app(&ctx, Language::English);
                app.demo = true;
                app.double_click_to_play = double_click;
                let track = page(1, &[71]).items.remove(0);
                app.library.tracks.push(track.clone());
                let size = egui::vec2(1040.0, 720.0);
                let mut tick = 0;
                account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
                let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
                let index = account_ui_text_rect(&output, "1").unwrap();
                let title = account_ui_text_rect(&output, &track.title).unwrap();
                let position = if index_button {
                    index.center()
                } else {
                    title.center()
                };
                let hovered = account_ui_frame(
                    &ctx,
                    &mut app,
                    size,
                    &mut tick,
                    vec![egui::Event::PointerMoved(position)],
                );
                assert!(account_ui_text_rect(&hovered, "1").is_none());
                assert!(app.selected.is_none(), "Hover started playback");
                assert!(
                    account_ui_shapes(&hovered).iter().any(|shape| match shape {
                        egui::Shape::Path(path) =>
                            path.fill == Color32::WHITE
                                && path.points.iter().all(|p| p.x < title.left()
                                    && p.y > title.top() - 10.0
                                    && p.y < title.bottom() + 32.0),
                        _ => false,
                    }),
                    "Hovered index is missing its white Play icon"
                );
                account_ui_click(&ctx, &mut app, size, &mut tick, position);
                if double_click && !index_button {
                    assert!(
                        app.selected.is_none(),
                        "First row click must not play in double-click mode"
                    );
                    account_ui_click(&ctx, &mut app, size, &mut tick, position);
                }
                assert_eq!(
                    app.selected,
                    Some(track.key()),
                    "double={double_click}, index={index_button}"
                );
                assert!(app.playback_pending.is_none());
                let away = account_ui_frame(
                    &ctx,
                    &mut app,
                    size,
                    &mut tick,
                    vec![egui::Event::PointerGone],
                );
                assert!(account_ui_text_rect(&away, "1").is_some());
            }
        }
    }

    #[test]
    fn rapid_double_clicks_on_different_tracks_do_not_depend_on_global_triple_click_counts() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.demo = true;
        app.double_click_to_play = true;
        let mut tracks = page(3, &[71, 72, 73]).items;
        for track in &mut tracks {
            track.title = format!("Track {}", track.id);
        }
        app.library.tracks = tracks.clone();
        let size = egui::vec2(1040.0, 720.0);
        let mut tick = 0;
        account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        let positions: Vec<_> = tracks
            .iter()
            .map(|track| {
                account_ui_text_rect(&output, &track.title)
                    .unwrap()
                    .center()
            })
            .collect();
        let mut previous = None;
        for (track, position) in tracks.iter().zip(positions) {
            account_ui_click(&ctx, &mut app, size, &mut tick, position);
            assert_eq!(
                app.selected, previous,
                "The first click must only begin a pair for this exact row"
            );
            account_ui_click(&ctx, &mut app, size, &mut tick, position);
            assert_eq!(app.selected, Some(track.key()));
            previous = Some(track.key());
        }
        assert!(app.playback_pending.is_none() && app.workers.is_empty());
    }

    #[test]
    fn double_click_preference_is_an_interface_switch_and_settings_search_finds_it() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.settings_open = true;
            app.settings_section = SettingsSection::Interface;
            let size = egui::vec2(1040.0, 800.0);
            let mut tick = 0;
            let label = language.text(
                "Запускать трек двойным щелчком",
                "Play tracks with a double click",
            );
            account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            let position = account_ui_text_rect(&output, label).unwrap().center();
            account_ui_click(&ctx, &mut app, size, &mut tick, position);
            assert!(app.double_click_to_play && !app.click_behavior_save_failed);
            account_ui_click(&ctx, &mut app, size, &mut tick, position);
            assert!(!app.double_click_to_play);
            app.settings_section = SettingsSection::General;
            app.settings_query = "double click".into();
            let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            assert!(account_ui_text_rect(&output, label).is_some());
            assert!(app.playing.is_none() && app.cache_pending.is_none());
        }
    }

    fn playlist_create_reply(
        owner: i64,
        result: Result<crate::web_audio::UserPlaylist, WebAudioError>,
    ) -> PlaylistCreateRequest {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        PlaylistCreateRequest {
            owner,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    #[test]
    fn playlist_creation_keeps_errors_editable_then_inserts_the_confirmed_playlist_without_interrupting_audio()
     {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.playlist_create_form = Some(PlaylistCreateForm {
            name: "New playlist".into(),
            focus_name: true,
            error: None,
        });
        let mut created = search_test_playlist(123);
        created.album.owner_id = 42;
        created.album.title = "New playlist".into();
        created.can_add_tracks = true;
        let track = page(1, &[71]).items.remove(0);
        app.playing = Some(track.clone());
        app.play_queue = Some(vec![track.clone()]);
        app.queue_origin = Some("library".into());
        app.playlist_create_request = Some(playlist_create_reply(42, Err(WebAudioError::Network)));
        app.poll_playlist_create(&ctx);
        let form = app.playlist_create_form.as_ref().unwrap();
        assert_eq!(form.name, "New playlist");
        assert!(matches!(form.error, Some(WebAudioError::Network)));
        assert!(app.playlist_items.is_empty() && app.playlist_create_request.is_none());
        for (owner, canceled) in [(43, false), (42, true)] {
            let request = playlist_create_reply(owner, Ok(created.clone()));
            request.canceled.store(canceled, Ordering::Release);
            app.playlist_create_request = Some(request);
            app.poll_playlist_create(&ctx);
            assert!(app.playlist_items.is_empty() && app.playlist_create_form.is_some());
        }
        let (old_sender, old_receiver) = mpsc::sync_channel(1);
        let old_cancel = Arc::new(AtomicBool::new(false));
        app.playlist_request = Some(PlaylistRequest {
            owner: 42,
            canceled: old_cancel.clone(),
            receiver: old_receiver,
        });
        app.playlist_create_request = Some(playlist_create_reply(42, Ok(created.clone())));
        app.poll_playlist_create(&ctx);
        assert!(app.playlist_create_form.is_none());
        assert!(old_cancel.load(Ordering::Acquire));
        assert!(old_sender.send(Ok(Vec::new())).is_err());
        assert_eq!(app.playlist_items[0].reference, created.album);
        assert_eq!(app.playlist_items[0].subtitle, created.byline);
        assert_eq!(app.editable_playlists[0], created.album);
        assert!(matches!(&app.page, MusicPage::Playlist(reference) if *reference == created.album));
        assert_eq!(app.playing.as_ref().unwrap().key(), track.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), track.key());
        assert_eq!(app.queue_origin.as_deref(), Some("library"));
        let request = playlist_create_reply(42, Ok(created));
        let canceled = request.canceled.clone();
        app.playlist_create_request = Some(request);
        app.reset();
        assert!(canceled.load(Ordering::Acquire));
        assert!(app.playlist_create_form.is_none() && app.playlist_create_request.is_none());
    }

    #[test]
    fn playlist_dialog_blocks_player_shortcuts_and_background_history() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.session.as_mut().unwrap().music = Some(
            WebAudioClient::new(vec![SessionCookie {
                domain: "vk.ru".into(),
                path: "/".into(),
                name: "remixsid".into(),
                value: "synthetic-create-session".into(),
                secure: true,
                expires_unix: None,
            }])
            .unwrap(),
        );
        app.library.exhausted = true;
        app.playlists_requested = true;
        app.back_history.push(NavigationLocation {
            page: MusicPage::Home,
            settings: false,
            section: SettingsSection::default(),
            filter: String::new(),
        });
        let volume = app.player.snapshot().volume;
        let size = egui::vec2(1040.0, 720.0);
        let mut tick = 0;
        account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        let header = account_ui_text_rect(&output, "Library").unwrap();
        account_ui_click(
            &ctx,
            &mut app,
            size,
            &mut tick,
            egui::pos2(188.0, header.center().y),
        );
        assert!(
            app.playlist_create_form.is_some(),
            "Sidebar plus did not open the form"
        );
        let opened = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        assert!(account_ui_text_rect(&opened, "New playlist").is_some());
        assert!(app.playlist_create_request.is_none());
        app.playlist_create_form.as_mut().unwrap().name = "Draft".into();
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut tick,
            vec![
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: egui::pos2(10.0, 10.0),
                    button: egui::PointerButton::Extra1,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.player.snapshot().volume, volume);
        assert!(app.page == MusicPage::Library && app.back_history.len() == 1);
        assert_eq!(app.playlist_create_form.as_ref().unwrap().name, "Draft");
    }

    pub(super) fn search_test_playlist(id: i64) -> crate::web_audio::UserPlaylist {
        crate::web_audio::UserPlaylist {
            kind: crate::web_audio::CollectionKind::Playlist,
            byline: String::new(),
            original: None,
            album: AlbumRef {
                owner_id: -42,
                id,
                title: format!("Result Playlist {id}"),
                access_key: Some("synthetic-search-access".into()),
                cover_url: None,
            },
            subtitle: "Native subtitle".into(),
            track_count: Some(12),
            can_add_tracks: false,
        }
    }

    fn search_test_results(ids: &[i64], next_offset: usize) -> crate::web_audio::SearchResults {
        crate::web_audio::SearchResults {
            tracks: page(ids.len(), ids).items,
            artists: Vec::new(),
            artist_images: HashMap::new(),
            albums: Vec::new(),
            playlists: Vec::new(),
            has_more: true,
            next_offset,
        }
    }

    fn search_test_reply<T>(
        query: &str,
        offset: usize,
        generation: u64,
        result: Result<T, WebAudioError>,
    ) -> SearchRequest<T> {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        SearchRequest {
            owner: 42,
            query: query.into(),
            offset,
            generation,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        }
    }

    fn chrome_commands(output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    egui::ViewportCommand::StartDrag
                        | egui::ViewportCommand::BeginResize(_)
                        | egui::ViewportCommand::Minimized(_)
                        | egui::ViewportCommand::Maximized(_)
                        | egui::ViewportCommand::Close
                        | egui::ViewportCommand::Decorations(_)
                )
            })
            .cloned()
            .collect()
    }

    #[test]
    fn queued_launch_activation_waits_for_the_first_ui_frame_and_survives_window_recreation() {
        let contexts = [egui::Context::default(), egui::Context::default()];
        let mut app = account_ui_app(&contexts[0], Language::English);
        app.resume_position = Some(42.0);
        let activation = crate::single_instance::Activation::default();
        app.attach_instance(activation.clone());

        for (index, ctx) in contexts.iter().enumerate() {
            if index != 0 {
                app.prepare_reopened_window(ctx);
            }
            activation.request_for_test();
            let hidden = ctx.run_logic(&egui::RawInput::default(), |ctx| {
                app.poll_instance_activation(ctx);
            });
            assert!(
                hidden
                    .viewport_commands
                    .get(&egui::ViewportId::ROOT)
                    .is_none_or(|commands| !commands
                        .contains(&egui::ViewportCommand::Visible(true))
                        && !commands.contains(&egui::ViewportCommand::Focus))
            );

            let mut first_frame = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label("Synthetic initial frame");
            });
            first_frame.textures_delta.clear();
            let shown = ctx.run_logic(&egui::RawInput::default(), |ctx| {
                app.poll_instance_activation(ctx);
            });
            let commands = &shown.viewport_commands[&egui::ViewportId::ROOT];
            assert!(commands.contains(&egui::ViewportCommand::Visible(true)));
            assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
            assert!(commands.contains(&egui::ViewportCommand::Focus));
            assert!(!activation.take_pending());
            assert_eq!(app.resume_position, Some(42.0));
            assert_eq!(app.player.decoder_starts(), 0);
        }
    }

    fn custom_caption_glyphs(
        output: &egui::FullOutput,
        size: egui::Vec2,
    ) -> (egui::Rect, egui::Rect) {
        let shapes = account_ui_shapes(output);
        let minimize = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - 12.0).abs() < 0.1
                        && (rect.rect.height() - 1.8).abs() < 0.1
                        && rect.rect.top() < 50.0
                        && rect.rect.left() > size.x - 130.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("Custom minimize glyph");
        let maximize = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - 11.0).abs() < 0.1
                        && (rect.rect.height() - 11.0).abs() < 0.1
                        && rect.rect.top() < 50.0
                        && rect.rect.left() > size.x - 85.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("Custom maximize glyph");
        (minimize, maximize)
    }

    #[test]
    fn custom_caption_buttons_share_the_toolbar_to_the_right_of_the_avatar() {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            app.system_titlebar = false;
            let mut tick = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            }
            let (minimize, maximize) = custom_caption_glyphs(&output, size);
            let avatar = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Circle(circle)
                        if circle.radius == 19.0 && circle.center.y < 50.0 =>
                    {
                        Some(circle.center)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(minimize.left() > avatar.x + 19.0);
            assert!((minimize.center().y - avatar.y).abs() < 0.5);
            assert!((maximize.center().x - minimize.center().x - 36.0).abs() < 0.5);
            assert!((maximize.center().y - avatar.y).abs() < 0.5);
            let header = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == BACKGROUND
                            && rect.rect.top() == 0.0
                            && rect.rect.left() > 250.0
                            && rect.rect.width() > 400.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                header.height(),
                50.0,
                "Chrome must not introduce another row"
            );
            let pressed = account_ui_frame(
                &ctx,
                &mut app,
                size,
                &mut tick,
                seek_pointer(minimize.center(), true),
            );
            assert!(
                chrome_commands(&pressed).is_empty(),
                "Caption buttons must not start a background drag"
            );
            let released = account_ui_frame(
                &ctx,
                &mut app,
                size,
                &mut tick,
                seek_pointer(minimize.center(), false),
            );
            assert_eq!(
                chrome_commands(&released),
                [egui::ViewportCommand::Minimized(true)]
            );
        }
    }

    #[test]
    fn custom_header_drag_does_not_capture_search_or_home_clicks() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.system_titlebar = false;
        let size = egui::vec2(1040.0, 720.0);
        let mut tick = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        }
        let home = account_ui_text_rect(&output, "Home").unwrap().center();
        let pressed = account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(home, true));
        assert!(chrome_commands(&pressed).is_empty());
        account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(home, false));
        assert!(app.page == MusicPage::Home);
        let search = egui::pos2(400.0, 25.0);
        let pressed = account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(search, true));
        assert!(chrome_commands(&pressed).is_empty());
        account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(search, false));
        assert!(ctx.text_edit_focused());
        let search_rect = account_ui_shapes(&output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == SURFACE
                        && rect.rect.top() < 50.0
                        && rect.rect.left() >= 252.0
                        && rect.rect.width() > 200.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("Global search frame");
        let hint = account_ui_text_rect(&output, "Tracks, artists, albums, playlists").unwrap();
        assert!(search_rect.right() - hint.right() > 18.0);
        let caption_gap = egui::pos2(search_rect.right() + 16.0, search_rect.center().y);
        let pressed = account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut tick,
            seek_pointer(caption_gap, true),
        );
        assert_eq!(
            chrome_commands(&pressed),
            [egui::ViewportCommand::StartDrag]
        );
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut tick,
            seek_pointer(caption_gap, false),
        );
        let blank = egui::pos2(120.0, 12.0);
        let pressed = account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(blank, true));
        assert_eq!(
            chrome_commands(&pressed),
            [egui::ViewportCommand::StartDrag]
        );
    }

    #[test]
    fn interface_settings_switch_window_decoration_live_and_preserve_playback() {
        for language in [Language::Russian, Language::English] {
            let mut ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.system_titlebar = false;
            app.settings_open = true;
            app.settings_section = SettingsSection::Interface;
            let playing = page(1, &[9]).items.remove(0);
            app.playing = Some(playing.clone());
            let size = egui::vec2(1040.0, 720.0);
            let mut tick = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            }
            custom_caption_glyphs(&output, size);
            let label = language.text("Системный заголовок окна", "System title bar");
            let point = account_ui_text_rect(&output, label).unwrap().center();
            account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(point, true));
            let released =
                account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(point, false));
            assert!(app.system_titlebar);
            assert_eq!(chrome_commands(&released), [egui::ViewportCommand::Close]);
            assert!(app.window_reopen_requested());
            let replacement = egui::Context::default();
            app.prepare_reopened_window(&replacement);
            ctx = replacement;
            tick = 0;
            assert!(!app.window_reopen_requested());
            output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            assert!(!account_ui_shapes(&output).iter().any(|shape|matches!(shape,egui::Shape::Rect(rect)
                if (rect.rect.width()-12.0).abs()<0.1 && (rect.rect.height()-1.8).abs()<0.1 && rect.rect.top()<50.0)));
            let point = account_ui_text_rect(&output, label).unwrap().center();
            account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(point, true));
            let released =
                account_ui_frame(&ctx, &mut app, size, &mut tick, seek_pointer(point, false));
            assert!(!app.system_titlebar);
            assert_eq!(chrome_commands(&released), [egui::ViewportCommand::Close]);
            assert!(!app.titlebar_save_failed);
            assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
            assert!(app.playback_pending.is_none());
        }
    }

    #[test]
    fn window_recreation_retains_music_and_pending_requests_and_rebuilds_qr_textures() {
        let old = egui::Context::default();
        let mut app = account_ui_app(&old, Language::English);
        let (player, _receiver) = Player::test_paused(200, 73.875);
        app.player = player;
        let track = page(1, &[8]).items.remove(0);
        app.playing = Some(track.clone());
        app.library.tracks = vec![track.clone()];
        app.recent = vec![track.clone()];
        app.play_queue = Some(vec![track.clone()]);
        app.settings_open = true;
        app.settings_section = SettingsSection::Interface;
        app.search.input = "kept query".into();
        let (sender, receiver) = mpsc::sync_channel(1);
        app.pending = Some(receiver);
        app.window_reopen = true;
        app.qr_open = true;
        app.qr_url = Some("https://vk.ru/qr?synthetic=1".into());
        app.qr_texture = qr_image(&old, app.qr_url.as_deref().unwrap());
        let replacement = egui::Context::default();
        app.prepare_reopened_window(&replacement);
        assert!(!app.window_reopen_requested());
        assert_eq!(app.player.snapshot().state, PlaybackState::Paused);
        assert_eq!(app.player.snapshot().position, 73.875);
        assert_eq!(app.playing.as_ref().unwrap().key(), track.key());
        assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), track.key());
        assert_eq!(app.library.tracks[0].key(), track.key());
        assert!(app.settings_open && app.settings_section == SettingsSection::Interface);
        assert_eq!(app.search.input, "kept query");
        assert!(app.qr_open && app.qr_texture.is_some());
        let mut uploaded = replacement.tex_manager().write().take_delta();
        assert!(!uploaded.is_empty());
        uploaded.clear();
        // A reply in flight while the old window closed is still accepted.
        sender.send(Ok(Reply::NoSavedSession)).ok().unwrap();
        app.poll(&replacement);
        assert!(app.pending.is_none());
        drop(app);
        old.tex_manager().write().take_delta().clear();
        replacement.tex_manager().write().take_delta().clear();
    }

    fn settings_search_test_frame(
        ctx: &egui::Context,
        app: &mut MusicApp,
        frame: &mut u32,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        *frame += 1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1040.0, 720.0),
                )),
                time: Some(f64::from(*frame) * 0.1),
                events,
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        output.textures_delta.clear();
        output
    }

    #[test]
    fn settings_search_filters_all_sections_and_leaves_the_normalization_switch_editable() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, language);
            app.settings_open = true;
            app.settings_section = SettingsSection::General;
            app.search.input = "music query stays here".into();
            let mut frame = 0;
            let mut output = settings_search_test_frame(&ctx, &mut app, &mut frame, Vec::new());
            for _ in 0..3 {
                output = settings_search_test_frame(&ctx, &mut app, &mut frame, Vec::new());
            }
            let heading =
                account_ui_text_rect(&output, language.text("Настройки", "Settings")).unwrap();
            let field = account_ui_text_rect(
                &output,
                language.text("Поиск по настройкам", "Search settings"),
            )
            .unwrap();
            assert!(heading.top() >= 60.0 && field.top() > heading.bottom());
            assert!(
                (8.0..=34.0).contains(&(field.top() - heading.bottom())),
                "The settings search should sit directly under the page heading: {heading:?}; {field:?}"
            );
            let point = field.center();
            settings_search_test_frame(&ctx, &mut app, &mut frame, seek_pointer(point, true));
            settings_search_test_frame(&ctx, &mut app, &mut frame, seek_pointer(point, false));
            output = settings_search_test_frame(
                &ctx,
                &mut app,
                &mut frame,
                vec![egui::Event::Text(
                    language.text("ГРОМКОСТЬ", "VoLuMe").into(),
                )],
            );
            assert_eq!(app.settings_query, language.text("ГРОМКОСТЬ", "VoLuMe"));
            assert_eq!(app.search.input, "music query stays here");
            assert!(app.settings_section == SettingsSection::General);
            assert!(
                account_ui_text_rect(
                    &output,
                    language.text("Нормализация громкости", "Normalize volume")
                )
                .is_some()
            );
            assert!(account_ui_text_rect(&output, language.text("Громкость", "Volume")).is_none());
            for label in [
                language.text("Язык интерфейса", "Interface language"),
                language.text("Системный заголовок окна", "System title bar"),
                language.text("Лимит аудиокэша", "Audio cache limit"),
                language.text(
                    "Треки, исполнители, альбомы, плейлисты",
                    "Tracks, artists, albums, playlists",
                ),
                "music query stays here",
            ] {
                assert!(
                    account_ui_text_rect(&output, label).is_none(),
                    "Unmatched or global search leaked: {label}"
                );
            }
            let point = account_ui_text_rect(
                &output,
                language.text("Нормализация громкости", "Normalize volume"),
            )
            .unwrap()
            .center();
            let volume = app.player.snapshot().volume;
            settings_search_test_frame(&ctx, &mut app, &mut frame, seek_pointer(point, true));
            settings_search_test_frame(&ctx, &mut app, &mut frame, seek_pointer(point, false));
            assert!(app.player.normalize_volume());
            assert!(!app.normalization_save_failed);
            assert_eq!(app.player.snapshot().volume, volume);
            assert!(app.volume_save_at.is_none());
            assert!(
                app.workers.is_empty()
                    && app.search.results_request.is_none()
                    && app.search.hints_due.is_none()
            );
        }
    }

    #[test]
    fn clearing_settings_search_restores_the_selected_section_without_refreshing_cache_stats() {
        let directory = CacheScheduleDirectory::new();
        let cache = AudioCache::test_at(directory.0.join("audio"), 4096).unwrap();
        let initial = cache.stats();
        cache
            .write_range(&cache_schedule_key(9), 0, &[0; 64])
            .unwrap();
        assert!(cache.stats().used_bytes > initial.used_bytes);
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.audio_cache = Some(cache);
        app.cache_stats = Some(initial);
        app.settings_open = true;
        app.settings_section = SettingsSection::Interface;
        app.settings_query = "cache".into();
        app.search.input = "untouched music search".into();
        let mut frame = 0;
        let mut output = settings_search_test_frame(&ctx, &mut app, &mut frame, Vec::new());
        for _ in 0..3 {
            output = settings_search_test_frame(&ctx, &mut app, &mut frame, Vec::new());
        }
        assert!(account_ui_text_rect(&output, "Audio cache limit").is_some());
        assert!(account_ui_text_rect(&output, "System title bar").is_none());
        assert_eq!(
            app.cache_stats.as_ref().unwrap().used_bytes,
            initial.used_bytes
        );
        let heading = account_ui_text_rect(&output, "Settings").unwrap();
        let field = account_ui_shapes(&output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == SURFACE
                        && (rect.rect.height() - 38.0).abs() < 0.5
                        && rect.rect.top() > heading.bottom()
                        && rect.rect.top() < heading.bottom() + 32.0
                        && rect.rect.width() > 400.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        let clear = egui::pos2(field.right() - 24.0, field.center().y);
        settings_search_test_frame(&ctx, &mut app, &mut frame, seek_pointer(clear, true));
        settings_search_test_frame(&ctx, &mut app, &mut frame, seek_pointer(clear, false));
        assert!(app.settings_query.is_empty());
        output = settings_search_test_frame(&ctx, &mut app, &mut frame, Vec::new());
        assert!(account_ui_text_rect(&output, "System title bar").is_some());
        assert!(account_ui_text_rect(&output, "Audio cache limit").is_none());
        assert_eq!(app.search.input, "untouched music search");
        assert!(app.settings_section == SettingsSection::Interface);
        assert_eq!(
            app.cache_stats.as_ref().unwrap().used_bytes,
            initial.used_bytes
        );
        assert!(
            app.cache_pending.is_none() && app.workers.is_empty() && app.cache_save_at.is_none()
        );
    }

    #[test]
    fn shell_has_full_height_navigation_compact_toolbar_and_only_player_separator() {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let shapes = account_ui_shapes(&output);
            let player = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == SURFACE
                            && (rect.rect.width() - size.x).abs() < 0.5
                            && (rect.rect.bottom() - size.y).abs() < 0.5 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .unwrap();
            let sidebar = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == SURFACE
                            && rect.rect.left() == 0.0
                            && rect.rect.top() == 0.0
                            && (rect.rect.width() - 252.0).abs() < 1.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("Navigation must reach the top edge");
            assert!((sidebar.bottom() - player.top()).abs() < 2.0);
            let header = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == BACKGROUND
                            && rect.rect.top() == 0.0
                            && (rect.rect.left() - 252.0).abs() < 1.0
                            && rect.rect.width() > 400.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("Toolbar must start to the right of navigation");
            assert!(header.height() <= 52.0);
            for caption in ["VK", "FAST", "prototype 0.1", "MUSIC"] {
                assert!(account_ui_text_rect(&output, caption).is_none());
            }
            let fields = shapes.iter().filter(|shape| matches!(shape,egui::Shape::Text(text) if text.galley.text()=="Tracks, artists, albums, playlists")).count();
            assert_eq!(fields, 1);
            let field = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == SURFACE
                            && rect.rect.width() > 200.0
                            && header.contains_rect(rect.rect) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("Global search frame");
            let settings = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if (rect.rect.width() - 38.0).abs() < 0.5
                            && (rect.rect.height() - 38.0).abs() < 0.5
                            && header.contains_rect(rect.rect) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("Settings button");
            let avatar = shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Circle(circle)
                        if circle.radius == 19.0
                            && circle.center.x > size.x - 50.0
                            && header.contains(circle.center) =>
                    {
                        Some(circle.center)
                    }
                    _ => None,
                })
                .expect("Account avatar");
            assert!((field.height() - settings.height()).abs() < 0.5);
            assert!(
                (field.center().y - settings.center().y).abs() < 0.5
                    && (field.center().y - avatar.y).abs() < 0.5
            );
            let separators: Vec<_> = shapes
                .iter()
                .filter_map(|shape| match shape {
                    egui::Shape::LineSegment { points, stroke }
                        if stroke.width > 0.0 && points[0].distance(points[1]) > 200.0 =>
                    {
                        Some(*points)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                separators.len(),
                1,
                "Only the player separator should remain: {separators:?}"
            );
            assert!((separators[0][0].y - player.top()).abs() < 2.0);
        }
    }

    #[test]
    fn toolbar_search_works_from_library_and_keeps_focus_queue_and_return_location() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.tracks = page(2, &[1, 2]).items;
        app.library.exhausted = true;
        app.filter = "library filter".into();
        app.play_queue = Some(page(1, &[99]).items);
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let input = account_ui_text_rect(&output, "Tracks, artists, albums, playlists").unwrap();
        assert!(input.center().y < 50.0);
        account_ui_click(&ctx, &mut app, size, &mut frame, input.center());
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::Text("nor".into())],
        );
        assert!(matches!(app.page, MusicPage::Search(_)));
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::Text("t".into())],
        );
        assert_eq!(app.search.input, "nort");
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: Some(egui::Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(matches!(&app.page,MusicPage::Search(query) if query=="nort"));
        assert_eq!(app.library.tracks.len(), 2);
        assert_eq!(app.play_queue.as_ref().unwrap()[0].id, 99);
        app.go_back(&ctx);
        assert!(app.page == MusicPage::Library);
        assert_eq!(app.filter, "library filter");
    }

    #[test]
    fn search_defaults_to_all_and_category_content_starts_next_to_the_pills() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.language = language;
            assert!(app.search.category == SearchCategory::All);
            app.page = MusicPage::Search("query".into());
            app.search.input = "query".into();
            app.search.results_query = "query".into();
            let mut results = search_test_results(&[1, 2, 3], 100);
            for (index, track) in results.tracks.iter_mut().enumerate() {
                track.title = format!("Result track {}", index + 1);
            }
            results.artists = vec![ArtistRef {
                id: "result-artist".into(),
                name: "Result Artist".into(),
            }];
            results.albums = vec![AlbumRef {
                id: 9,
                owner_id: -42,
                title: "Result Album".into(),
                access_key: None,
                cover_url: None,
            }];
            results.playlists = vec![search_test_playlist(7)];
            results.has_more = false;
            app.search.results = Some(Arc::new(results));
            let size = egui::vec2(1040.0, 1000.0);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            for name in [
                "Result Artist",
                "Result Album",
                "Result Playlist 7",
                "Result track 1",
            ] {
                assert!(account_ui_text_rect(&output, name).is_some());
            }
            let captions = [
                language.text("Всё", "All"),
                language.text("Треки", "Tracks"),
                language.text("Исполнители", "Artists"),
                language.text("Альбомы", "Albums"),
                language.text("Плейлисты", "Playlists"),
            ];
            for (category, caption, height) in [
                (SearchCategory::Tracks, captions[1], 54.0),
                (SearchCategory::Artists, captions[2], 64.0),
                (SearchCategory::Albums, captions[3], 64.0),
                (SearchCategory::Playlists, captions[4], 64.0),
            ] {
                let target = account_ui_text_rect(&output, caption).unwrap().center();
                account_ui_click(&ctx, &mut app, size, &mut frame, target);
                assert!(app.search.category == category);
                for _ in 0..4 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let shapes = account_ui_shapes(&output);
                for caption in &captions[1..] {
                    assert_eq!(shapes.iter().filter(|shape|matches!(shape,egui::Shape::Text(text) if text.galley.text()==*caption)).count(),1,"Category heading was repeated for {caption}");
                }
                let pills_bottom = captions
                    .iter()
                    .map(|caption| account_ui_text_rect(&output, caption).unwrap().bottom())
                    .fold(0.0, f32::max);
                let first = shapes
                    .iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if (rect.rect.height() - height).abs() < 0.5
                                && rect.rect.left() >= 252.0
                                && rect.rect.top() > pills_bottom
                                && rect.rect.width() > 150.0 =>
                        {
                            Some(rect.rect.top())
                        }
                        _ => None,
                    })
                    .min_by(f32::total_cmp)
                    .unwrap();
                let selected_text = account_ui_text_rect(&output, caption).unwrap();
                let pill = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == theme::SELECTED
                                && rect.rect.contains(selected_text.center()) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Selected category pill");
                let search = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == SURFACE
                                && rect.rect.top() < 52.0
                                && rect.rect.left() >= 252.0
                                && rect.rect.width() > 200.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Global search field");
                let before = pill.top() - search.bottom();
                let after = first - pill.bottom();
                assert!(
                    (after - before).abs() < 1.0,
                    "Category gaps differ: search-to-pill={before}, pill-to-content={after}"
                );
            }
            app.submit_search(&ctx, "another query".into());
            assert!(app.search.category == SearchCategory::All);
        }
    }

    #[test]
    fn search_playlists_open_the_exact_playlist_from_all_and_its_own_tab_without_changing_playback()
    {
        for category in [SearchCategory::All, SearchCategory::Playlists] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.page = MusicPage::Search("query".into());
            app.search.input = "query".into();
            app.search.results_query = "query".into();
            app.search.category = category;
            let playlist = search_test_playlist(17);
            let reference = playlist.album.clone();
            let mut results = search_test_results(&[], 0);
            results.playlists = vec![playlist];
            results.has_more = false;
            app.search.results = Some(Arc::new(results));
            let playing = page(1, &[99]).items.remove(0);
            app.playing = Some(playing.clone());
            app.play_queue = Some(vec![playing.clone()]);
            let size = egui::vec2(1040.0, 720.0);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            assert!(account_ui_text_rect(&output, "No results found.").is_none());
            let title = account_ui_text_rect(&output, &reference.title).unwrap();
            account_ui_click(&ctx, &mut app, size, &mut frame, title.center());
            assert!(matches!(&app.page,MusicPage::Playlist(found) if *found==reference));
            assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
            assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
            assert!(app.playback_pending.is_none());
            assert!(!app.back_history.is_empty());
        }
    }

    #[test]
    fn search_swipe_is_clipped_to_results_and_does_not_expand_the_page_sideways() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.page = MusicPage::Search("query".into());
        app.search.input = "query".into();
        app.search.results_query = "query".into();
        let mut results = search_test_results(&[1, 2], 100);
        results.tracks[0].title = "Swipe row".into();
        results.artists = vec![ArtistRef {
            id: "swipe-artist".into(),
            name: "Swipe Artist".into(),
        }];
        results.has_more = false;
        app.search.results = Some(Arc::new(results));
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let tabs = account_ui_text_rect(&output, "Artists").unwrap();
        account_ui_click(&ctx, &mut app, size, &mut frame, tabs.center());
        output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        let moving = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Swipe row"
                        || text.galley.text() == "Swipe Artist" =>
                {
                    Some((shape.clip_rect, text.visual_bounding_rect()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!moving.is_empty());
        for (clip, rect) in moving {
            let visible = clip.intersect(rect);
            if visible.is_positive() {
                assert!(visible.left() >= 260.0 && visible.right() <= size.x - 8.0);
            }
        }
        assert!(app.playing.is_none() && app.playback_pending.is_none());
        for _ in 0..4 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        assert!(account_ui_text_rect(&output, "Swipe Artist").is_some());
        assert!(account_ui_text_rect(&output, "Swipe row").is_none());
    }

    #[test]
    fn search_is_the_second_tab_and_popular_queries_form_a_vertical_list() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.open_search(&ctx, None);
        app.search.popular = vec![
            "First popular query".into(),
            "Second popular query".into(),
            "Third popular query".into(),
        ];
        app.search.popular_requested = true;
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let home = account_ui_text_rect(&output, "Home").unwrap();
        let search = account_ui_text_rect(&output, "Search").unwrap();
        let library = account_ui_text_rect(&output, "My music").unwrap();
        assert!(home.top() < search.top() && search.top() < library.top());
        let mut previous: Option<egui::Rect> = None;
        for query in &app.search.popular {
            let rect = account_ui_shapes(&output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text() == query => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .unwrap();
            if let Some(previous) = previous {
                assert!(
                    (rect.left() - previous.left()).abs() < 0.5 && rect.top() > previous.bottom()
                );
            }
            previous = Some(rect);
        }
        let second = account_ui_text_rect(&output, "Second popular query")
            .unwrap()
            .center();
        account_ui_click(&ctx, &mut app, size, &mut frame, second);
        assert_eq!(app.search.input, "Second popular query");
        assert!(matches!(&app.page, MusicPage::Search(query) if query == "Second popular query"));
        assert!(app.playing.is_none());
    }

    #[test]
    fn search_keyboard_selects_the_hint_before_enter_and_cancels_old_requests_on_edit() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.open_search(&ctx, None);
        let size = egui::vec2(1040.0, 720.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let input = account_ui_text_rect(&output, "Tracks, artists, albums, playlists")
            .unwrap()
            .center();
        account_ui_click(&ctx, &mut app, size, &mut frame, input);
        let old = search_test_reply("old", 0, app.search.generation, Ok(vec!["stale".into()]));
        let canceled = old.canceled.clone();
        app.search.hints_request = Some(old);
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::Text("nor".into())],
        );
        assert!(canceled.load(Ordering::Acquire));
        assert_eq!(app.search.input, "nor");
        app.search.hints = vec!["nortmirage".into(), "north".into()];
        app.search.hints_for = "nor".into();
        let key = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![key(egui::Key::ArrowDown)],
        );
        assert_eq!(app.search.selected_hint, Some(0));
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![key(egui::Key::Enter)],
        );
        assert_eq!(app.search.input, "nortmirage");
        assert!(matches!(&app.page, MusicPage::Search(query) if query == "nortmirage"));
    }

    #[test]
    fn search_pagination_keeps_the_library_and_playing_queue_and_rejects_stale_replies() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.library.tracks = page(1, &[99]).items;
        app.search.input = "test".into();
        app.search.results_query = "test".into();
        app.page = MusicPage::Search("test".into());
        let mut initial = search_test_results(&[1, 2], 100);
        initial.artists.push(ArtistRef {
            id: "canonical".into(),
            name: "Artist".into(),
        });
        initial.playlists.push(search_test_playlist(7));
        app.search.results = Some(Arc::new(initial));
        app.play_queue = Some(page(1, &[1]).items);
        app.search.results_request = Some(search_test_reply(
            "test",
            100,
            0,
            Ok(search_test_results(&[2, 3], 200)),
        ));
        app.poll_search(&ctx);
        let results = app.search.results.as_ref().unwrap();
        assert_eq!(
            results.tracks.iter().map(Track::key).collect::<Vec<_>>(),
            [(1, 1), (1, 2), (1, 3)]
        );
        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.playlists.len(), 1);
        assert_eq!(results.playlists[0].album, search_test_playlist(7).album);
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 1);
        assert_eq!(app.library.tracks[0].id, 99);
        app.search.results_request = Some(search_test_reply(
            "test",
            200,
            0,
            Ok(search_test_results(&[1, 2, 3], 300)),
        ));
        app.poll_search(&ctx);
        assert!(!app.search.results.as_ref().unwrap().has_more);
        app.search.results_request =
            Some(search_test_reply("test", 0, 0, Err(WebAudioError::Network)));
        app.poll_search(&ctx);
        assert_eq!(app.search.failed_offset, 0);
        app.search.results_request = Some(search_test_reply(
            "test",
            100,
            0,
            Err(WebAudioError::Timeout),
        ));
        app.poll_search(&ctx);
        assert_eq!(app.search.failed_offset, 100);
        app.search.generation = 1;
        app.search.results_request = Some(search_test_reply(
            "test",
            0,
            0,
            Ok(search_test_results(&[8], 100)),
        ));
        app.poll_search(&ctx);
        assert_eq!(app.search.results.as_ref().unwrap().tracks.len(), 3);
        app.search.input = "new draft".into();
        app.search.edit();
        assert!(
            app.view_tracks().is_empty(),
            "Hidden old results must not start via the player"
        );
    }

    #[test]
    fn search_history_restores_the_query_and_cached_results_without_changing_playback() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let results = Arc::new(search_test_results(&[1, 2], 100));
        app.search.cache.insert("test".into(), results.clone());
        app.open_search(&ctx, Some("test".into()));
        app.play_queue = Some(page(1, &[99]).items);
        app.navigate_music(
            &ctx,
            MusicPage::Artist(ArtistRef {
                id: "canonical".into(),
                name: "Artist".into(),
            }),
        );
        app.search.input = "draft".into();
        app.search.edit();
        app.go_back(&ctx);
        assert_eq!(app.search.input, "test");
        assert!(Arc::ptr_eq(app.search.results.as_ref().unwrap(), &results));
        assert_eq!(app.play_queue.as_ref().unwrap()[0].id, 99);
        assert!(app.workers.is_empty());
    }

    #[test]
    fn search_waits_for_verified_bootstrap_and_debounces_typing_without_a_poll_loop() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.page = MusicPage::Search(String::new());
        app.search.input = "nor".into();
        app.drive_search(&ctx);
        assert!(app.search.hints_due.is_none());
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        app.session.as_mut().unwrap().music = Some(client);
        app.library.offset = 100;
        let before = Instant::now();
        app.drive_search(&ctx);
        let deadline = app.search.hints_due.unwrap();
        assert!(deadline >= before + Duration::from_millis(250));
        app.drive_search(&ctx);
        assert_eq!(app.search.hints_due, Some(deadline));
        app.search.input = "north".into();
        app.search.edit();
        assert!(app.search.hints_due.is_none());
        assert!(app.workers.is_empty());
        app.reset();
        assert!(app.search.input.is_empty() && app.search.cache.is_empty());
    }

    #[test]
    fn playlists_open_their_own_page_without_replacing_the_playing_queue() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.session.as_mut().unwrap().music = Some(
            WebAudioClient::new(vec![SessionCookie {
                domain: "vk.ru".into(),
                path: "/".into(),
                name: "remixsid".into(),
                value: "synthetic-playlist-session".into(),
                secure: true,
                expires_unix: None,
            }])
            .unwrap(),
        );
        let reference = AlbumRef {
            owner_id: -99,
            id: 7,
            title: "Road playlist".into(),
            access_key: Some("synthetic-playlist-access-key".into()),
            cover_url: None,
        };
        let key = MusicPage::Playlist(reference.clone()).detail_key().unwrap();
        assert_ne!(
            key,
            MusicPage::Album(reference.clone()).detail_key().unwrap()
        );
        app.details.insert(
            key.clone(),
            Arc::new(DetailPage::Playlist(crate::web_audio::AlbumPage {
                album: reference.clone(),
                artists: Vec::new(),
                year: None,
                description: Some("Hand picked tracks".into()),
                creator: Some(crate::web_audio::PlaylistCreator {
                    owner_id: -99,
                    name: "Creator group".into(),
                }),
                tracks: page(2, &[1, 2])
                    .items
                    .into_iter()
                    .map(|mut track| {
                        track.title = format!("Playlist track {}", track.id);
                        track
                    })
                    .collect(),
                total_tracks: None,
                stats: crate::web_audio::AlbumStats {
                    plays: Some(861),
                    created_at: Some(1785426660),
                    updated_at: Some(1785426660),
                },
            })),
        );
        app.playlist_items = vec![playlist_ui::PlaylistItem {
            kind: crate::web_audio::CollectionKind::Playlist,
            reference: reference.clone(),
            subtitle: "Personal selection".into(),
            can_edit: false,
            pinned: false,
        }];
        app.playlists_requested = true;
        app.library.tracks = page(1, &[99]).items;
        app.library.exhausted = true;
        app.play_queue = Some(page(1, &[88]).items);
        app.playing = app.play_queue.as_ref().unwrap().first().cloned();
        let size = egui::vec2(1040.0, 900.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let library = account_ui_text_rect(&output, "My music").unwrap();
        let playlists = account_ui_text_rect(&output, "Library").unwrap();
        let divider = account_ui_shapes(&output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::LineSegment { points, stroke }
                    if stroke.width == 1.0
                        && points[0].y > library.bottom()
                        && points[0].y < playlists.top()
                        && points[1].x < 252.0 =>
                {
                    Some(*points)
                }
                _ => None,
            })
            .expect("Inset divider after My music");
        assert!(divider[0].x > 18.0 && divider[1].x < 234.0);
        assert!(
            playlists.top() - divider[0].y < 20.0,
            "Playlist heading should stay near the divider"
        );
        let target = account_ui_text_rect(&output, "Road playlist")
            .unwrap()
            .center();
        account_ui_click(&ctx, &mut app, size, &mut frame, target);
        assert!(matches!(&app.page, MusicPage::Playlist(playlist) if *playlist == reference));
        assert_eq!(app.view_queue_origin(), key);
        assert_eq!(
            app.view_tracks()
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(app.playing.as_ref().unwrap().id, 88);
        assert_eq!(app.play_queue.as_ref().unwrap()[0].id, 88);
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        assert!(account_ui_text_rect(&output, "PLAYLIST").is_some());
        let description = account_ui_text_rect(&output, "Hand picked tracks").unwrap();
        // "ALBUM" only names the track column, never the playlist itself.
        assert!(
            account_ui_shapes(&output)
                .into_iter()
                .all(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text() == "ALBUM" => {
                        text.visual_bounding_rect().top() > description.bottom()
                    }
                    _ => true,
                })
        );
        let heading = account_ui_text_rect(&output, "Tracks").unwrap();
        let first = account_ui_text_rect(&output, &app.view_tracks()[0].title).unwrap();
        let last = account_ui_text_rect(&output, &app.view_tracks()[1].title).unwrap();
        let plays = account_ui_text_rect(&output, "861 plays").unwrap();
        assert!(description.bottom() < heading.top() && heading.bottom() < first.top());
        assert!(plays.top() > last.bottom());
        for prefix in ["created ", "updated "] {
            assert!(account_ui_shapes(&output).iter().any(|shape| matches!(shape,egui::Shape::Text(text) if text.galley.text().starts_with(prefix))));
        }
        let creator = account_ui_text_rect(&output, "Creator group")
            .unwrap()
            .center();
        let publisher = crate::web_audio::PlaylistCreator {
            owner_id: -99,
            name: "Creator group".into(),
        };
        app.details.insert(
            MusicPage::Owner(publisher.clone()).detail_key().unwrap(),
            Arc::new(DetailPage::Owner(crate::web_audio::OwnerPage {
                creator: publisher.clone(),
                image_url: None,
                description: None,
                closed: None,
                playlists: Vec::new(),
                total: Some(0),
                offset: 0,
                next_offset: None,
                playlists_error: None,
                retry_offset: None,
                tracks: Vec::new(),
                tracks_error: None,
            })),
        );
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            seek_pointer(creator, true),
        );
        let released = account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            seek_pointer(creator, false),
        );
        assert!(released.platform_output.commands.is_empty());
        assert!(app.page == MusicPage::Owner(publisher));
        assert_eq!(app.playing.as_ref().unwrap().id, 88);
        app.go_back(&ctx);
        assert!(matches!(&app.page, MusicPage::Playlist(found) if *found==reference));
        app.prepare_view_queue();
        assert_eq!(
            app.play_queue
                .as_ref()
                .unwrap()
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(app.library.tracks[0].id, 99);
        app.reset();
        let output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        assert!(account_ui_text_rect(&output, "Library").is_none());
        assert!(account_ui_text_rect(&output, "Road playlist").is_none());
    }

    #[test]
    fn playlist_replies_are_account_bound_cancelable_and_not_retried_every_frame() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let reference = AlbumRef {
            owner_id: -99,
            id: 7,
            title: "Saved playlist".into(),
            access_key: None,
            cover_url: None,
        };
        let response = || {
            vec![crate::web_audio::UserPlaylist {
                kind: crate::web_audio::CollectionKind::Playlist,
                byline: String::new(),
                original: None,
                album: reference.clone(),
                subtitle: "Selection".into(),
                track_count: Some(2),
                can_add_tracks: false,
            }]
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        app.playlists_requested = true;
        app.playlist_request = Some(PlaylistRequest {
            owner: 42,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        assert!(sender.send(Ok(response())).is_ok());
        app.poll_playlists(&ctx);
        assert_eq!(app.playlist_items[0].reference, reference);
        assert!(app.playlists_requested && app.playlist_request.is_none());
        app.poll_playlists(&ctx);
        assert!(app.workers.is_empty());
        app.playlist_items.clear();
        for (owner, canceled) in [(43, false), (42, true)] {
            let (sender, receiver) = mpsc::sync_channel(1);
            app.playlist_request = Some(PlaylistRequest {
                owner,
                canceled: Arc::new(AtomicBool::new(canceled)),
                receiver,
            });
            assert!(sender.send(Ok(response())).is_ok());
            app.poll_playlists(&ctx);
            assert!(app.playlist_items.is_empty());
        }
        let canceled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(1);
        app.playlist_request = Some(PlaylistRequest {
            owner: 42,
            canceled: canceled.clone(),
            receiver,
        });
        app.playlist_error = Some(WebAudioError::Network);
        app.reset();
        assert!(canceled.load(Ordering::Acquire));
        assert!(sender.send(Ok(response())).is_err());
        assert!(!app.playlists_requested && app.playlist_error.is_none());
        app.poll_playlists(&ctx);
        assert!(app.playlist_items.is_empty());
    }

    #[test]
    fn playlists_wait_for_the_verified_library_before_starting_a_request() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.session.as_mut().unwrap().music = Some(
            WebAudioClient::new(vec![SessionCookie {
                domain: "vk.ru".into(),
                path: "/".into(),
                name: "remixsid".into(),
                value: "synthetic-playlist-session".into(),
                secure: true,
                expires_unix: None,
            }])
            .unwrap(),
        );
        app.library.tracks = page(1, &[1]).items;
        assert!(
            !app.playlist_ready(),
            "Cached rows alone are not a verified bootstrap"
        );
        app.load_playlists(&ctx, false);
        assert!(
            !app.playlists_requested && app.playlist_request.is_none() && app.workers.is_empty()
        );
        app.library.offset = 100;
        assert!(app.playlist_ready());
        app.library.offset = 0;
        app.library.exhausted = true;
        assert!(
            app.playlist_ready(),
            "A verified empty library may still have playlists"
        );
        app.playlists_requested = true;
        app.load_playlists(&ctx, false);
        assert!(app.playlist_request.is_none() && app.workers.is_empty());
        app.session = None;
        assert!(!app.playlist_ready());
    }

    #[test]
    fn release_information_is_below_tracks_and_only_on_release_pages() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let album = AlbumRef {
            owner_id: -42,
            id: 7,
            title: "Release".into(),
            access_key: None,
            cover_url: None,
        };
        let artist = ArtistRef {
            id: "canonical".into(),
            name: "Artist".into(),
        };
        let mut tracks = page(3, &[1, 2, 3]).items;
        for (index, duration) in [114, 121, 142].into_iter().enumerate() {
            tracks[index].duration = duration;
            tracks[index].title = format!("Album track {}", index + 1);
        }
        app.details.insert(
            MusicPage::Album(album.clone()).detail_key().unwrap(),
            Arc::new(DetailPage::Album(crate::web_audio::AlbumPage {
                album: album.clone(),
                artists: vec![artist.clone()],
                year: Some(2026),
                description: None,
                creator: None,
                tracks: tracks.clone(),
                total_tracks: None,
                stats: crate::web_audio::AlbumStats {
                    plays: Some(861),
                    created_at: Some(1785426660),
                    updated_at: Some(1785426660),
                },
            })),
        );
        app.details.insert(
            MusicPage::Artist(artist.clone()).detail_key().unwrap(),
            Arc::new(DetailPage::Artist(crate::web_audio::ArtistPage {
                artist: artist.clone(),
                image_url: None,
                tracks: tracks.clone(),
                latest_releases: Vec::new(),
                discography: Vec::new(),
                releases_error: None,
                releases_pending: false,
                related: Vec::new(),
            })),
        );
        app.navigate_music(&ctx, MusicPage::Album(album));
        let size = egui::vec2(1040.0, 1100.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let last_track = account_ui_text_rect(&output, "Album track 3").unwrap();
        let summary = account_ui_text_rect(&output, "3 tracks").unwrap();
        assert!(summary.top() > last_track.bottom());
        assert!(account_ui_text_rect(&output, "6 minutes 17 seconds").is_some());
        assert!(account_ui_text_rect(&output, "861 plays").is_some());
        for prefix in ["created ", "updated "] {
            assert!(account_ui_shapes(&output).iter().any(|shape| matches!(shape,egui::Shape::Text(text) if text.galley.text().starts_with(prefix))));
        }
        app.library.tracks = tracks.clone();
        app.library.exhausted = true;
        app.recent = tracks;
        for page in [
            MusicPage::Artist(artist),
            MusicPage::Library,
            MusicPage::Recent,
            MusicPage::Search(String::new()),
        ] {
            app.navigate_music(&ctx, page);
            let output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            assert!(account_ui_text_rect(&output, "3 tracks").is_none());
            assert!(account_ui_text_rect(&output, "861 plays").is_none());
        }
    }

    #[test]
    fn track_versions_are_gray_next_to_the_title_in_rows_and_player() {
        for title in ["Current single", "Current single (Slowed)"] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            app.demo = true;
            let mut track = page(1, &[1]).items.remove(0);
            track.title = title.into();
            track.subtitle = Some("Slowed".into());
            app.library.tracks.push(track.clone());
            app.playing = Some(track.clone());
            let mut frame = 0;
            let mut output = account_ui_frame(
                &ctx,
                &mut app,
                egui::vec2(1040.0, 720.0),
                &mut frame,
                Vec::new(),
            );
            for _ in 0..3 {
                output = account_ui_frame(
                    &ctx,
                    &mut app,
                    egui::vec2(1040.0, 720.0),
                    &mut frame,
                    Vec::new(),
                );
            }
            let labels: Vec<_> = account_ui_shapes(&output)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Text(text) if text.galley.text() == track.display_title() => {
                        Some(text)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                labels.len(),
                2,
                "Version must appear in both the row and the player"
            );
            for text in labels {
                assert_eq!(text.galley.job.sections.len(), 2);
                let version = &text.galley.job.sections[1];
                assert_eq!(version.format.color, MUTED);
                assert_eq!(
                    &text.galley.job.text[version.byte_range.start.0..version.byte_range.end.0],
                    " Slowed"
                );
                assert!(text.override_text_color.is_none());
            }
        }
    }

    #[test]
    fn compact_track_gutter_is_fixed_and_scrollbar_has_its_own_space() {
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            app.demo = true;
            let scroll_style = ctx.style_of(egui::Theme::Dark).spacing.scroll;
            assert!(!scroll_style.floating);
            assert_eq!(scroll_style.bar_width, 8.0);
            assert_eq!(scroll_style.handle_min_length, 28.0);
            let mut frame = 0;
            let mut previous_width: Option<f32> = None;
            for count in [3, 60] {
                app.library.tracks = page(count, &(1..=count as i64).collect::<Vec<_>>()).items;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..4 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let shapes = account_ui_shapes(&output);
                let row = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.rect.height() == 54.0 && rect.rect.width() > 200.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Track row");
                let art = shapes
                    .iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Rect(rect)
                            if rect.rect.width() == 40.0
                                && rect.rect.height() == 40.0
                                && row.contains_rect(rect.rect) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Track artwork");
                assert!(
                    (art.left() - row.left() - 34.0).abs() < 0.5,
                    "Excessive left padding: {art:?} in {row:?}"
                );
                if let Some(width) = previous_width {
                    assert!(
                        (width - row.width() - 16.0).abs() < 0.5,
                        "Only a visible scrollbar should reserve the right gutter"
                    );
                }
                if count == 3 {
                    assert!(
                        (size.x - row.right() - 8.0).abs() < 0.5,
                        "Empty scrollbar gutter must not offset short pages"
                    );
                }
                previous_width = Some(row.width());
                assert!(
                    (row.left() - 260.0).abs() < 0.5,
                    "Page left margin was not reduced"
                );
                let bar = shapes.iter().find_map(|shape| match shape {
                    egui::Shape::Rect(rect)
                        if (rect.rect.width() - 8.0).abs() < 0.5
                            && rect.rect.height() >= 27.5
                            && rect.rect.left() > row.right()
                            && rect.fill == theme::SCROLL_HANDLE =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                });
                if count == 3 {
                    assert!(bar.is_none(), "Scrollbar must be hidden without overflow");
                    continue;
                }
                let bar = bar.expect("Lighter scrollbar with a minimum thumb height");
                assert!(
                    bar.left() - row.right() >= 5.5,
                    "Scrollbar overlaps a track"
                );
                assert!(
                    (size.x - bar.right() - 10.0).abs() < 0.5,
                    "Scrollbar did not use the right margin"
                );
            }
        }
    }

    #[test]
    fn each_artist_in_track_rows_and_player_opens_its_own_page_without_playing() {
        for footer in [false, true] {
            for target in 0..2 {
                let ctx = egui::Context::default();
                let mut app = account_ui_app(&ctx, Language::English);
                app.demo = true;
                let artists = vec![
                    ArtistRef {
                        id: "canonical-one".into(),
                        name: "First, Artist".into(),
                    },
                    ArtistRef {
                        id: "canonical-two".into(),
                        name: "Second & Artist".into(),
                    },
                ];
                let mut track = page(1, &[1]).items.remove(0);
                track.artist = "First, Artist, Second & Artist".into();
                track.artists = artists.clone();
                app.library.tracks.push(track.clone());
                if footer {
                    app.playing = Some(track.clone());
                }
                let size = egui::vec2(1040.0, 720.0);
                let mut frame = 0;
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
                }
                let mut labels: Vec<_> = account_ui_shapes(&output)
                    .into_iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Text(text) if text.galley.text() == artists[target].name => {
                            Some(text.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .collect();
                labels.sort_by(|a, b| a.top().total_cmp(&b.top()));
                let position = if footer {
                    labels.last()
                } else {
                    labels.first()
                }
                .unwrap()
                .center();
                account_ui_click(&ctx, &mut app, size, &mut frame, position);
                assert!(
                    matches!(&app.page, MusicPage::Artist(artist) if *artist == artists[target])
                );
                assert!(app.selected.is_none(), "Artist link triggered row playback");
                assert!(app.playback_pending.is_none());
                assert_eq!(
                    app.playing.as_ref().map(Track::key),
                    footer.then_some(track.key())
                );
                app.go_back(&ctx);
                assert!(app.page == MusicPage::Library);
            }
        }
    }

    #[test]
    fn player_title_opens_the_exact_release_and_old_cached_titles_resolve_by_track_identity() {
        for known_album in [false, true] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            app.demo = true;
            let mut track = page(1, &[7]).items.remove(0);
            track.title = "Current single".into();
            let album = AlbumRef {
                id: 11,
                owner_id: -42,
                title: "Official release".into(),
                access_key: Some("synthetic".into()),
                cover_url: None,
            };
            track.album = known_album.then_some(album.clone());
            app.playing = Some(track.clone());
            app.play_queue = Some(vec![track.clone()]);
            let size = egui::vec2(1040.0, 720.0);
            let mut frame = 0;
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let title = account_ui_text_rect(&output, &track.title)
                .unwrap()
                .center();
            account_ui_click(&ctx, &mut app, size, &mut frame, title);
            if known_album {
                assert!(matches!(&app.page, MusicPage::Album(value) if *value == album));
            } else {
                assert!(matches!(&app.page, MusicPage::TrackAlbum(key, _) if *key == track.key()));
            }
            assert_eq!(app.playing.as_ref().unwrap().key(), track.key());
            assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), track.key());
            assert!(app.playback_pending.is_none());
        }
    }

    fn detail_test_error(app: &mut MusicApp, owner: i64, key: String, error: WebAudioError) {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err(error)).ok().unwrap();
        app.detail_request = Some(DetailRequest {
            owner,
            key,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
    }

    #[test]
    fn unavailable_artist_opens_name_search_and_back_skips_the_failed_page() {
        for (id, error) in [
            ("canonical", WebAudioError::Format),
            ("canonical", WebAudioError::Rejected),
            ("artist-with-hyphens", WebAudioError::NoStream),
        ] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            let playing = page(1, &[99]).items.remove(0);
            app.playing = Some(playing.clone());
            app.play_queue = Some(vec![playing.clone()]);
            let artist = ArtistRef {
                id: id.into(),
                name: "Artist - without a page".into(),
            };
            app.navigate_music(&ctx, MusicPage::Artist(artist.clone()));
            let key = app.page.detail_key().unwrap();
            detail_test_error(&mut app, 42, key, error);
            app.poll_detail(&ctx);
            assert!(matches!(&app.page,MusicPage::Search(query) if query==&artist.name));
            assert_eq!(app.search.input, artist.name);
            assert!(app.search.category == SearchCategory::All);
            assert!(app.detail_error.is_none());
            assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
            assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
            assert!(app.workers.is_empty());
            app.go_back(&ctx);
            assert!(app.page == MusicPage::Library);
        }
    }

    #[test]
    fn artist_network_errors_and_playlist_or_stale_replies_never_redirect_to_search() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let artist = ArtistRef {
            id: "canonical".into(),
            name: "Artist".into(),
        };
        app.navigate_music(&ctx, MusicPage::Artist(artist.clone()));
        let key = app.page.detail_key().unwrap();
        detail_test_error(&mut app, 42, key.clone(), WebAudioError::Network);
        app.poll_detail(&ctx);
        assert!(app.page == MusicPage::Artist(artist.clone()));
        assert_eq!(app.detail_error, Some(WebAudioError::Network));
        app.detail_error = None;
        detail_test_error(&mut app, 42, "artist:another".into(), WebAudioError::Format);
        app.poll_detail(&ctx);
        assert!(app.page == MusicPage::Artist(artist));
        assert!(app.detail_error.is_none());
        let album = search_test_playlist(7).album;
        app.navigate_music(&ctx, MusicPage::Playlist(album.clone()));
        let key = app.page.detail_key().unwrap();
        detail_test_error(&mut app, 42, key, WebAudioError::Format);
        app.poll_detail(&ctx);
        assert!(app.page == MusicPage::Playlist(album));
        assert_eq!(app.detail_error, Some(WebAudioError::Format));
    }

    #[test]
    fn display_only_artist_names_search_whole_spaces_and_hyphens_without_a_detail_request() {
        for (id, name) in [
            ("", "Artist With Spaces"),
            ("", "Artist - With Hyphens"),
            ("Artist With Spaces", "  Artist With Spaces  "),
            ("invalid/artist", "Artist – With A Dash"),
            ("0", "A, B & C"),
        ] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            let playing = page(1, &[99]).items.remove(0);
            app.playing = Some(playing.clone());
            app.play_queue = Some(vec![playing.clone()]);
            app.search.category = SearchCategory::Albums;
            let artist = ArtistRef {
                id: id.into(),
                name: name.into(),
            };
            app.navigate_music(&ctx, MusicPage::Artist(artist));
            let query = name.trim();
            assert!(matches!(&app.page, MusicPage::Search(actual) if actual == query));
            assert_eq!(app.search.input, query);
            assert!(app.search.category == SearchCategory::All);
            assert!(app.detail_request.is_none() && app.detail_error.is_none());
            assert!(app.workers.is_empty());
            assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
            assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
            app.go_back(&ctx);
            assert!(app.page == MusicPage::Library);
        }
    }

    #[test]
    fn missing_releases_and_tracks_without_albums_search_the_title_and_skip_failed_history() {
        let album = AlbumRef {
            title: "Missing release - Extended Mix".into(),
            ..search_test_playlist(7).album
        };
        for (target, title) in [
            (MusicPage::Album(album.clone()), album.title.clone()),
            (
                MusicPage::TrackAlbum((42, 9), "Single without release - Slowed".into()),
                "Single without release - Slowed".into(),
            ),
        ] {
            for error in [
                WebAudioError::Format,
                WebAudioError::Rejected,
                WebAudioError::NoAlbum,
                WebAudioError::NoStream,
            ] {
                let ctx = egui::Context::default();
                let mut app = search_test_app(&ctx);
                let playing = page(1, &[99]).items.remove(0);
                app.playing = Some(playing.clone());
                app.play_queue = Some(vec![playing.clone()]);
                app.navigate_music(&ctx, target.clone());
                let key = app.page.detail_key().unwrap();
                detail_test_error(&mut app, 42, key, error);
                app.poll_detail(&ctx);
                assert!(matches!(&app.page, MusicPage::Search(query) if query == &title));
                assert_eq!(app.search.input, title);
                assert!(app.detail_error.is_none() && app.detail_request.is_none());
                assert!(app.search.category == SearchCategory::All && app.workers.is_empty());
                assert_eq!(app.playing.as_ref().unwrap().key(), playing.key());
                assert_eq!(app.play_queue.as_ref().unwrap()[0].key(), playing.key());
                app.go_back(&ctx);
                assert!(app.page == MusicPage::Library);
            }
        }
    }

    #[test]
    fn missing_unnamed_release_uses_its_source_track_or_opens_empty_search() {
        for has_source in [false, true] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            let mut album = search_test_playlist(7).album;
            album.title.clear();
            if has_source {
                let mut playing = page(1, &[99]).items.remove(0);
                playing.title = "Single - Slowed".into();
                playing.album = Some(album.clone());
                app.playing = Some(playing);
            }
            app.navigate_music(&ctx, MusicPage::Album(album));
            let key = app.page.detail_key().unwrap();
            detail_test_error(&mut app, 42, key, WebAudioError::NoAlbum);
            app.poll_detail(&ctx);
            let expected = if has_source { "Single - Slowed" } else { "" };
            assert!(matches!(&app.page, MusicPage::Search(query) if query == expected));
            assert_eq!(app.search.input, expected);
            assert!(app.detail_error.is_none() && app.workers.is_empty());
            app.go_back(&ctx);
            assert!(app.page == MusicPage::Library);
        }
    }

    #[test]
    fn valid_empty_release_and_playlist_pages_remain_open() {
        for playlist in [false, true] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            let album = search_test_playlist(7).album;
            let target = if playlist {
                MusicPage::Playlist(album.clone())
            } else {
                MusicPage::Album(album.clone())
            };
            app.navigate_music(&ctx, target.clone());
            let key = app.page.detail_key().unwrap();
            let data = crate::web_audio::AlbumPage {
                album,
                artists: Vec::new(),
                year: None,
                description: None,
                creator: None,
                tracks: Vec::new(),
                total_tracks: None,
                stats: crate::web_audio::AlbumStats::default(),
            };
            let (sender, receiver) = mpsc::sync_channel(1);
            sender
                .send(Ok(if playlist {
                    DetailPage::Playlist(data)
                } else {
                    DetailPage::Album(data)
                }))
                .ok()
                .unwrap();
            app.detail_request = Some(DetailRequest {
                owner: 42,
                key: key.clone(),
                canceled: Arc::new(AtomicBool::new(false)),
                receiver,
            });
            app.poll_detail(&ctx);
            assert!(app.page == target);
            assert!(app.details.get(&key).unwrap().tracks().is_empty());
            assert!(app.detail_error.is_none() && app.search.input.is_empty());
        }
    }

    #[test]
    fn release_network_authentication_and_stale_errors_do_not_become_missing_page_searches() {
        let album = search_test_playlist(7).album;
        for target in [
            MusicPage::Artist(ArtistRef {
                id: "canonical-artist".into(),
                name: "Artist With Spaces".into(),
            }),
            MusicPage::Album(album),
            MusicPage::TrackAlbum((42, 9), "Track - Single".into()),
        ] {
            for error in [
                WebAudioError::Network,
                WebAudioError::Timeout,
                WebAudioError::Http(404),
                WebAudioError::Expired,
                WebAudioError::NoSession,
                WebAudioError::Unverified,
                WebAudioError::TooLarge,
                WebAudioError::Pagination,
            ] {
                let ctx = egui::Context::default();
                let mut app = search_test_app(&ctx);
                app.navigate_music(&ctx, target.clone());
                let key = app.page.detail_key().unwrap();
                detail_test_error(&mut app, 42, key, error.clone());
                app.poll_detail(&ctx);
                assert!(app.page == target);
                assert_eq!(app.detail_error, Some(error));
            }
            for (owner, canceled, settings) in
                [(43, false, false), (42, true, false), (42, false, true)]
            {
                let ctx = egui::Context::default();
                let mut app = search_test_app(&ctx);
                app.navigate_music(&ctx, target.clone());
                let key = app.page.detail_key().unwrap();
                app.settings_open = settings;
                detail_test_error(&mut app, owner, key, WebAudioError::Format);
                app.detail_request
                    .as_ref()
                    .unwrap()
                    .canceled
                    .store(canceled, Ordering::Release);
                app.poll_detail(&ctx);
                assert!(app.page == target);
                assert_eq!(app.settings_open, settings);
            }
        }
    }

    #[test]
    fn my_music_heading_shows_the_full_count_as_smaller_muted_text_while_rows_load() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.language = language;
            app.library.refresh(page(638, &[1, 2]));
            let (_sender, receiver) = mpsc::sync_channel(1);
            app.pending = Some(receiver);
            let size = egui::vec2(1040.0, 720.0);
            let mut tick = 0;
            for extra in [None, Some(3)] {
                if let Some(id) = extra {
                    app.library.append(page(638, &[id]));
                }
                let mut output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
                for _ in 0..3 {
                    output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
                }
                let title = account_ui_shapes(&output)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Text(text)
                            if text.galley.text() == language.text("Моя музыка", "My music")
                                && text.galley.job.sections[0].format.font_id.size == 26.0 =>
                        {
                            Some(text.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .unwrap();
                let badge = account_ui_shapes(&output)
                    .into_iter()
                    .find_map(|shape| match shape {
                        egui::Shape::Text(text) if text.galley.text() == "638" => Some(text),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(badge.galley.job.sections[0].format.color, MUTED);
                assert_eq!(badge.galley.job.sections[0].format.font_id.size, 16.0);
                assert!(badge.visual_bounding_rect().left() > title.right());
                assert!(badge.visual_bounding_rect().left() - title.right() < 14.0);
                assert!(
                    (badge.visual_bounding_rect().center().y - title.center().y).abs() < 3.0,
                    "title={title:?}, badge={:?}",
                    badge.visual_bounding_rect()
                );
                assert!(app.library.tracks.len() < 638);
            }
        }
    }

    #[test]
    fn library_count_ignores_stale_zero_and_clears_cached_rows_for_confirmed_empty_refresh() {
        let mut library = Library::from_cached(page(100, &[1, 2]).items);
        library.total = Some(0);
        library.append(AudioPage {
            count: None,
            items: page(1, &[3]).items,
            exhausted: Some(false),
        });
        assert_eq!(library.total, None);
        assert_eq!(library.tracks.len(), 3);
        library.refresh(page(0, &[]));
        assert_eq!(library.total, Some(0));
        assert!(library.tracks.is_empty() && library.exhausted);
    }

    #[test]
    fn detail_history_keeps_queue_and_cancels_a_different_pending_page_even_when_target_is_cached()
    {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let artist = ArtistRef {
            id: "canonical".into(),
            name: "Artist".into(),
        };
        let album = AlbumRef {
            id: 9,
            owner_id: -42,
            title: "Release".into(),
            access_key: None,
            cover_url: None,
        };
        let tracks = page(2, &[1, 2]).items;
        app.play_queue = Some(tracks.clone());
        app.playing = tracks.first().cloned();
        app.filter = "saved search".into();
        app.details.insert(
            MusicPage::Album(album.clone()).detail_key().unwrap(),
            Arc::new(DetailPage::Album(crate::web_audio::AlbumPage {
                album: album.clone(),
                artists: vec![artist.clone()],
                year: Some(2026),
                description: None,
                creator: None,
                total_tracks: None,
                stats: Default::default(),
                tracks,
            })),
        );
        app.navigate_music(&ctx, MusicPage::Artist(artist));
        let canceled = Arc::new(AtomicBool::new(false));
        let (_sender, receiver) = mpsc::sync_channel(1);
        app.detail_request = Some(DetailRequest {
            owner: 42,
            key: app.page.detail_key().unwrap(),
            canceled: canceled.clone(),
            receiver,
        });
        app.detail_error = Some(WebAudioError::Network);
        app.navigate_music(&ctx, MusicPage::Album(album));
        assert!(canceled.load(Ordering::Acquire));
        assert!(app.detail_request.is_none());
        assert!(app.detail_error.is_none());
        assert_eq!(app.view_tracks().len(), 2);
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 2);
        assert_eq!(app.playing.as_ref().unwrap().key(), (1, 1));
        let mut frame = 0;
        account_ui_frame(
            &ctx,
            &mut app,
            egui::vec2(1040.0, 720.0),
            &mut frame,
            Vec::new(),
        );
        app.go_back(&ctx);
        assert!(matches!(app.page, MusicPage::Artist(_)));
        account_ui_frame(
            &ctx,
            &mut app,
            egui::vec2(1040.0, 720.0),
            &mut frame,
            Vec::new(),
        );
        app.go_back(&ctx);
        assert!(app.page == MusicPage::Library);
        assert_eq!(app.filter, "saved search");
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn reset_discards_detail_metadata_and_cancels_both_types_of_request() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let detail_cancel = Arc::new(AtomicBool::new(false));
        let metadata_cancel = Arc::new(AtomicBool::new(false));
        let (detail_sender, detail_receiver) = mpsc::sync_channel(1);
        let (metadata_sender, metadata_receiver) = mpsc::sync_channel(1);
        app.detail_request = Some(DetailRequest {
            owner: 42,
            key: "artist:old".into(),
            canceled: detail_cancel.clone(),
            receiver: detail_receiver,
        });
        app.metadata_request = Some(MetadataRequest {
            owner: 42,
            canceled: metadata_cancel.clone(),
            receiver: metadata_receiver,
        });
        app.metadata_attempted.insert((1, 1));
        app.reset();
        assert!(detail_cancel.load(Ordering::Acquire));
        assert!(metadata_cancel.load(Ordering::Acquire));
        assert!(app.metadata_attempted.is_empty());
        assert!(detail_sender.send(Err(WebAudioError::Network)).is_err());
        assert!(metadata_sender.send(Ok(page(1, &[1]).items)).is_err());
        app.poll_detail(&ctx);
        app.poll_metadata(&ctx);
        assert!(app.details.is_empty());
        assert!(app.library.tracks.is_empty());
    }

    #[test]
    fn missing_saved_session_leaves_the_sign_in_screen_idle() {
        let ctx = egui::Context::default();
        let mut app = MusicApp::new(&ctx, false, Language::Russian);
        let (sender, receiver) = mpsc::sync_channel(1);
        app.pending = Some(receiver);
        assert!(sender.send(Ok(Reply::NoSavedSession)).is_ok());
        app.poll(&ctx);
        assert!(!app.restoring);
        assert!(app.pending.is_none());
        assert!(app.session.is_none());
    }

    #[test]
    fn phone_code_input_is_available_before_the_optional_otp_status() {
        let ctx = egui::Context::default();
        let mut app = MusicApp::new(&ctx, false, Language::Russian);
        let (_sender, receiver) = mpsc::sync_channel(1);
        app.qr_pending = Some(receiver);
        app.qr_texture = qr_image(&ctx, "https://vk.ru/?synthetic=1");
        app.qr_expires = Some(Instant::now() + Duration::from_secs(60));
        for stage in [qr_auth::Stage::Scanned, qr_auth::Stage::Confirming] {
            app.qr_stage = stage;
            assert!(app.qr_accepts_code());
        }
        for stage in [qr_auth::Stage::Creating, qr_auth::Stage::Waiting] {
            app.qr_stage = stage;
            assert!(!app.qr_accepts_code());
        }
        app.qr_stage = qr_auth::Stage::Finishing;
        assert!(!app.qr_accepts_code());
        app.qr_stage = qr_auth::Stage::Scanned;
        app.qr_expires = Some(Instant::now() - Duration::from_secs(1));
        assert!(!app.qr_accepts_code());
        app.qr_expires = Some(Instant::now() + Duration::from_secs(60));
        app.cancel_qr();
        assert!(!app.qr_accepts_code());
    }

    #[test]
    fn qr_dialog_fits_without_scrolling_and_renders_confirmation_digits() {
        fn centered_labels(shape: &egui::Shape) -> bool {
            match shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Введите код"
                        || text.galley.text() == "Enter your code" =>
                {
                    text.galley.job.halign == Align::Center
                }
                egui::Shape::Vec(shapes) => shapes.iter().all(centered_labels),
                _ => true,
            }
        }
        fn blue_button(shape: &egui::Shape, target: egui::Rect) -> bool {
            match shape {
                egui::Shape::Rect(rect) => {
                    rect.fill == PRIMARY
                        && rect.rect.center().distance(target.center()) <= 1.0
                        && (rect.rect.width() - target.width()).abs() <= 2.0
                }
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| blue_button(shape, target)),
                _ => false,
            }
        }
        fn contains_text(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, needle)),
                _ => false,
            }
        }
        for size in [egui::vec2(800.0, 540.0), egui::vec2(1040.0, 720.0)] {
            for language in [Language::Russian, Language::English] {
                let ctx = egui::Context::default();
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                let mut app = MusicApp::new(&ctx, false, language);
                let (_sender, receiver) = mpsc::sync_channel(1);
                app.qr_pending = Some(receiver);
                app.qr_open = true;
                app.qr_texture = qr_image(&ctx, "https://vk.ru/?synthetic=1");
                app.qr_expires = Some(Instant::now() + Duration::from_secs(60));
                app.qr_code_input = "012345".into();
                let mut frame = 0;
                let mut waiting_size = None;
                for stage in [
                    qr_auth::Stage::Waiting,
                    qr_auth::Stage::Scanned,
                    qr_auth::Stage::Confirming,
                    qr_auth::Stage::Finishing,
                    qr_auth::Stage::Creating,
                    qr_auth::Stage::Waiting,
                ] {
                    for show_error in [false, true] {
                        app.qr_stage = stage;
                        app.qr_texture = (stage != qr_auth::Stage::Creating)
                            .then(|| qr_image(&ctx, "https://vk.ru/?synthetic=1").unwrap());
                        app.qr_error =
                            show_error.then_some(UiError::QrCode(qr_auth::Error::Network));
                        let mut rects = None;
                        let mut rendered_code = false;
                        let mut shapes = Vec::new();
                        for _ in 0..4 {
                            frame += 1;
                            let mut output = ctx.run_ui(
                                egui::RawInput {
                                    screen_rect: Some(screen),
                                    time: Some(f64::from(frame) * 0.1),
                                    ..Default::default()
                                },
                                |ui| {
                                    rects = app.qr_window(ui.ctx());
                                },
                            );
                            rendered_code = output
                                .shapes
                                .iter()
                                .any(|shape| contains_text(&shape.shape, "012345"));
                            output.textures_delta.clear();
                            shapes = output.shapes;
                        }
                        let (window, content, confirmation, actions) = rects.unwrap();
                        if stage == qr_auth::Stage::Waiting && !show_error {
                            waiting_size.get_or_insert(window.size());
                        }
                        for label in [
                            language.text("Вход через QR", "Sign in via QR"),
                            language.text("Введите код", "Enter your code"),
                        ] {
                            if let Some(text) = shapes.iter().find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if text.galley.text() == label => {
                                    Some(text)
                                }
                                _ => None,
                            }) {
                                let text_rect = text.galley.rect.translate(text.pos.to_vec2());
                                assert!(
                                    (text_rect.center().x - window.center().x).abs() <= 1.0,
                                    "QR label is off-center: {label} at {text_rect:?} in {window:?}"
                                );
                            }
                        }
                        assert!(
                            screen.shrink(4.0).contains_rect(window),
                            "QR window outside {size:?}: {window:?}"
                        );
                        assert!(
                            window.expand(1.0).contains_rect(content),
                            "Clipped QR contents {size:?}: {content:?} in {window:?}"
                        );
                        let control_center = if confirmation.is_some() {
                            window.center().x
                        } else {
                            actions[0].center().x
                        };
                        for controls in confirmation.into_iter().chain(std::iter::once(actions)) {
                            let actual = controls[0].union(controls[1]);
                            assert!(
                                (actual.center().x - control_center).abs() <= 1.0,
                                "QR controls are off-center: {actual:?} in {window:?}"
                            );
                            assert!(
                                (controls[0].width() - controls[1].width()).abs() <= 1.0,
                                "QR controls have different widths: {controls:?}"
                            );
                            assert!(content.expand(1.0).contains_rect(actual));
                            for control in controls {
                                assert!(
                                    (control.center().x - control_center).abs() <= 1.0,
                                    "QR control is not individually centered: {control:?}"
                                );
                            }
                        }
                        assert!(
                            (stage == qr_auth::Stage::Finishing && !show_error)
                                || actions[1].top() >= actions[0].bottom() + 4.0,
                            "Cancel is not stacked below Refresh: {actions:?}"
                        );
                        assert!(shapes.iter().all(|shape| centered_labels(&shape.shape)));
                        if let Some([field, button]) = confirmation {
                            let waiting = waiting_size.unwrap();
                            assert!(
                                (window.size() - waiting).length() <= 1.0,
                                "Dialog must not jump between steps: {window:?}, waiting {waiting:?}"
                            );
                            assert!(
                                button.top() >= field.bottom(),
                                "Confirmation is not below its field"
                            );
                            assert!((field.center().x - window.center().x).abs() <= 1.0);
                            assert!((button.center().x - window.center().x).abs() <= 1.0);
                            assert!(
                                actions[0].top() >= button.bottom() + 4.0,
                                "Refresh is not stacked below Confirm"
                            );
                            assert!((button.width() - actions[0].width()).abs() <= 1.0);
                            assert!(
                                shapes.iter().any(|shape| blue_button(&shape.shape, button)),
                                "Confirmation is not blue"
                            );
                        }
                        assert_eq!(
                            rendered_code,
                            matches!(stage, qr_auth::Stage::Scanned | qr_auth::Stage::Confirming)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cancel_qr_discards_a_late_signed_in_session() {
        let ctx = egui::Context::default();
        let mut app = MusicApp::new(&ctx, false, Language::Russian);
        let (sender, receiver) = mpsc::sync_channel(1);
        app.qr_pending = Some(receiver);
        app.qr_control = Some(qr_auth::Control::default());
        app.qr_open = true;
        let cookies = vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid6".into(),
            value: "synthetic".into(),
            secure: true,
            expires_unix: None,
        }];
        let music = WebAudioClient::new(cookies.clone()).unwrap();
        app.cancel_qr();
        assert!(
            sender
                .send(Ok(QrConnected {
                    resume: None,
                    cached: Vec::new(),
                    recent: Vec::new(),
                    profile: Profile {
                        id: 42,
                        first_name: "Test".into(),
                        last_name: String::new(),
                        avatar_url: None,
                    },
                    cookies,
                    music,
                    page: page(0, &[]),
                }))
                .is_err()
        );
        app.poll_qr(&ctx);
        assert!(app.session.is_none());
        assert!(!app.qr_open);
    }

    #[test]
    fn refresh_merges_new_tracks_and_updated_metadata_without_dropping_cached_tail() {
        let mut library = Library::from_cached(page(4, &[1, 2, 3, 4]).items);
        let mut fresh = page(6, &[9, 1]);
        fresh.exhausted = Some(false);
        fresh.items[1].title = "Updated title".into();
        library.refresh(fresh);
        assert_eq!(
            library
                .tracks
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [9, 1, 2, 3, 4]
        );
        assert_eq!(library.tracks[1].title, "Updated title");
        assert_eq!(library.offset, 2);
        assert_eq!(library.fresh_prefix, 2);
        let mut next = page(6, &[2, 8]);
        next.exhausted = Some(false);
        library.append(next);
        assert_eq!(
            library
                .tracks
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [9, 1, 2, 8, 3, 4]
        );
        assert_eq!(library.offset, 4);
        let mut refreshed = page(7, &[10, 9]);
        refreshed.exhausted = Some(false);
        library.refresh(refreshed);
        assert_eq!(
            library
                .tracks
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            [10, 9, 1, 2, 8, 3, 4]
        );
        assert_eq!(library.offset, 2);
    }

    #[test]
    fn queued_library_snapshots_survive_reset_and_keep_their_account() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let (sender, receiver) = mpsc::sync_channel(1);
        app.library_save_pending = Some(receiver);
        app.library_save_current = Some((42, Arc::new(page(1, &[1]).items)));
        app.library_save_queue
            .insert(42, Arc::new(page(3, &[1, 2, 3]).items));
        app.library_save_queue
            .insert(43, Arc::new(page(2, &[7, 8]).items));
        app.reset();
        assert_eq!(app.library_save_queue[&42].len(), 3);
        assert_eq!(app.library_save_queue[&43].len(), 2);
        assert_eq!(app.restored_library(42, Vec::new()).tracks.len(), 3);
        assert_eq!(app.restored_library(43, Vec::new()).tracks.len(), 2);
        sender.send(true).unwrap();
        app.poll_library_save(&ctx);
        assert!(app.session.is_none());
        assert!(app.library_save_pending.is_some());
        let (owner, snapshot) = app.library_save_current.as_ref().unwrap();
        assert!(matches!((*owner, snapshot.len()), (42, 3) | (43, 2)));
    }

    #[test]
    fn clicking_track_title_or_number_starts_playback_without_text_selection() {
        for label in ["T", "1"] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            app.session = Some(Session {
                profile: Profile {
                    id: 42,
                    first_name: "Test".into(),
                    last_name: "User".into(),
                    avatar_url: None,
                },
                music: None,
                cache_warning: false,
            });
            app.library.tracks = page(1, &[1]).items;
            app.library.exhausted = true;
            let mut frame = 0;
            let mut render = |app: &mut MusicApp, events| {
                frame += 1;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1040.0, 720.0),
                        )),
                        time: Some(f64::from(frame) * 0.1),
                        events,
                        ..Default::default()
                    },
                    |ui| app.render(ui),
                );
                output.textures_delta.clear();
                output
            };
            let mut output = render(&mut app, Vec::new());
            for _ in 0..3 {
                output = render(&mut app, Vec::new());
            }
            let position = account_ui_text_rect(&output, label).unwrap().center();
            render(&mut app, seek_pointer(position, true));
            render(&mut app, seek_pointer(position, false));
            assert_eq!(app.playing.as_ref().map(Track::key), Some((1, 1)));
            assert_eq!(app.selected, Some((1, 1)));
            assert!(app.playback_pending.is_some());
        }
    }

    #[test]
    fn overlapping_pages_do_not_duplicate_tracks_or_skip_the_next_offset() {
        let mut library = Library::default();
        library.append(page(5, &[1, 2]));
        library.append(page(5, &[2, 3]));
        assert_eq!(library.offset, 4);
        assert_eq!(library.tracks.len(), 3);
        assert!(!library.exhausted);
        library.append(page(5, &[]));
        assert!(library.exhausted);
    }

    #[test]
    fn long_detail_lists_only_build_visible_widgets_and_keep_the_last_row_reachable() {
        for columns in [1, 2] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            let mut tracks = page(1001, &(1..=1001).collect::<Vec<_>>()).items;
            for track in &mut tracks {
                track.title = format!("Visible track {}", track.id);
            }
            let expected_height = tracks.len().div_ceil(columns) as f32 * 58.0 - 4.0;
            let bottom_offset = expected_height - 720.0;
            let mut tick = 0;
            for offset in [0.0, 58.0 * 200.0, bottom_offset] {
                let mut content_height = 0.0;
                let mut output = egui::FullOutput::default();
                for _ in 0..3 {
                    tick += 1;
                    output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1040.0, 720.0),
                            )),
                            time: Some(f64::from(tick) / 144.0),
                            ..Default::default()
                        },
                        |ui| {
                            let scroll = theme::scroll_area(
                                ui,
                                egui::ScrollArea::vertical()
                                    .id_salt("clipped-list-test")
                                    .vertical_scroll_offset(offset)
                                    .auto_shrink([false, false]),
                                |ui| {
                                    app.clipped_track_rows(
                                        ui,
                                        &tracks,
                                        columns,
                                        false,
                                        "test-rows",
                                        track_sort::Extra::default(),
                                        false,
                                    );
                                },
                            );
                            content_height = scroll.content_size.y;
                        },
                    );
                    output.textures_delta.clear();
                }
                assert!((content_height - expected_height).abs() < 1.0);
                let widgets = ctx.viewport(|viewport| {
                    viewport
                        .prev_pass
                        .widgets
                        .layers()
                        .map(|(_, widgets)| widgets.len())
                        .sum::<usize>()
                });
                assert!(widgets < 500, "Off-screen rows created widgets: {widgets}");
                let labels: Vec<_> = account_ui_shapes(&output)
                    .into_iter()
                    .filter_map(|shape| match shape {
                        egui::Shape::Text(text)
                            if text.galley.text().starts_with("Visible track ") =>
                        {
                            Some(text.galley.text().to_owned())
                        }
                        _ => None,
                    })
                    .collect();
                assert!(
                    !labels.is_empty(),
                    "No visible rows: columns={columns}, offset={offset}"
                );
                if offset == 0.0 {
                    assert!(labels.iter().any(|label| label == "Visible track 1"));
                } else if offset == bottom_offset {
                    assert!(labels.iter().any(|label| label == "Visible track 1001"));
                } else {
                    let expected = format!("Visible track {}", 200 * columns + 1);
                    assert!(
                        labels.contains(&expected),
                        "Incorrect row after scrolling: {labels:?}"
                    );
                }
            }
            assert!(app.workers.is_empty());
        }
    }

    #[test]
    fn reset_discards_late_results() {
        let ctx = egui::Context::default();
        let mut app = MusicApp::new(&ctx, false, Language::Russian);
        let (sender, receiver) = mpsc::sync_channel(1);
        app.pending = Some(receiver);
        app.reset();
        assert!(
            sender
                .send(Ok(Reply::Audio(page(1, &[1]), false, None)))
                .is_err()
        );
        app.poll(&ctx);
        assert!(app.library.tracks.is_empty());
    }

    #[test]
    fn clipped_track_row_ids_and_click_keys_survive_scrolling_with_a_header_in_both_layouts() {
        fn render(
            ctx: &egui::Context,
            app: &mut MusicApp,
            tracks: &[Track],
            columns: usize,
            offset: f32,
            events: Vec<egui::Event>,
            tick: &mut u32,
        ) -> (egui::FullOutput, Option<TrackAction>) {
            *tick += 1;
            let mut action = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1040.0, 720.0),
                    )),
                    time: Some(f64::from(*tick) / 144.0),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    theme::scroll_area(
                        ui,
                        egui::ScrollArea::vertical()
                            .id_salt("stable-detail-rows")
                            .vertical_scroll_offset(offset)
                            .auto_shrink([false, false]),
                        |ui| {
                            ui.allocate_space(egui::vec2(ui.available_width(), 120.0));
                            action = app.clipped_track_rows(
                                ui,
                                tracks,
                                columns,
                                false,
                                "stable-track-rows",
                                track_sort::Extra::default(),
                                false,
                            );
                        },
                    );
                },
            );
            output.textures_delta.clear();
            (output, action)
        }
        fn primary_row(
            ctx: &egui::Context,
            output: &egui::FullOutput,
            title: &str,
        ) -> (egui::Id, egui::Rect) {
            let title = account_ui_text_rect(output, title).expect("Visible target track");
            ctx.viewport(|viewport| {
                viewport
                    .prev_pass
                    .widgets
                    .layers()
                    .flat_map(|(_, widgets)| widgets.iter())
                    .find(|widget| {
                        widget.sense.senses_click()
                            && (widget.rect.height() - 54.0).abs() < 0.5
                            && widget.rect.contains(title.center())
                    })
                    .map(|widget| (widget.id, widget.rect))
                    .expect("Primary track row response")
            })
        }
        for columns in [1, 2] {
            let ctx = egui::Context::default();
            let mut app = account_ui_app(&ctx, Language::English);
            ctx.all_styles_mut(|style| style.animation_time = 0.0);
            let mut tracks = page(1001, &(1..=1001).collect::<Vec<_>>()).items;
            for track in &mut tracks {
                track.title = format!("Stable track {}", track.id);
            }
            let target = &tracks[203 * columns + columns - 1];
            let mut tick = 0;
            let first_offset = 120.0 + 58.0 * 200.0;
            let mut output = egui::FullOutput::default();
            for _ in 0..3 {
                output = render(
                    &ctx,
                    &mut app,
                    &tracks,
                    columns,
                    first_offset,
                    Vec::new(),
                    &mut tick,
                )
                .0;
            }
            let first = primary_row(&ctx, &output, &target.title);
            let second_offset = first_offset + 58.0;
            for _ in 0..3 {
                output = render(
                    &ctx,
                    &mut app,
                    &tracks,
                    columns,
                    second_offset,
                    Vec::new(),
                    &mut tick,
                )
                .0;
            }
            let second = primary_row(&ctx, &output, &target.title);
            assert_eq!(
                first.0, second.0,
                "Primary row ID moved at {columns} columns"
            );
            assert!(
                (first.1.width() - second.1.width()).abs() < 0.5,
                "Row width changed: {first:?} to {second:?}"
            );
            assert!((first.1.top() - second.1.top() - 58.0).abs() < 0.5);
            let point = account_ui_text_rect(&output, &target.title)
                .unwrap()
                .center();
            let (_, pressed) = render(
                &ctx,
                &mut app,
                &tracks,
                columns,
                second_offset,
                seek_pointer(point, true),
                &mut tick,
            );
            assert!(pressed.is_none());
            let (_, released) = render(
                &ctx,
                &mut app,
                &tracks,
                columns,
                second_offset,
                seek_pointer(point, false),
                &mut tick,
            );
            assert!(matches!(released, Some(TrackAction::Play(key)) if key == target.key()));
            assert!(app.playback_pending.is_none() && app.workers.is_empty());
        }
    }
}
