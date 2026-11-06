//! VK's private web music endpoint, isolated from VK ID credentials.
//! Parses website sections, opaque pagination cursors and audio reload replies.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Read,
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::{
    blocking::{Client, Response},
    header::{CONTENT_TYPE, COOKIE, HeaderValue, LOCATION, ORIGIN, REFERER},
    redirect::Policy,
};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{
    i18n::Language,
    model::{
        AlbumRef, ArtistRef, AudioPage, Profile, Track, album_from_object, artists_from_object,
        audio_unavailable_from_fields, audio_unavailable_from_object, cover_from_object,
        library_liked_from_object, parse_release_key, profile_avatar_from_object,
        release_key_from_object, subtitle_from_value, valid_artist_id, valid_cover_url,
    },
    web_session::SessionCookie,
};

#[path = "web_audio/audio_upload.rs"]
mod audio_upload;
mod owner_page;
mod playlist_follow;
#[path = "web_audio/playlist_manage.rs"]
mod playlist_manage;
pub use owner_page::OwnerPage;
pub use playlist_follow::PlaylistFollowChange;
pub use playlist_manage::{
    PLAYLIST_DESCRIPTION_MAX_CHARS, PlaylistCoverChange, PlaylistEditDetails, PlaylistEditUpdate,
};

const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
const MAX_COOKIE_HEADER: usize = 32_768;
const LOGICAL_PAGE_SIZE: usize = 100;
const MAX_BATCH_REQUESTS: usize = 20;
const MAX_LOOKUP_REQUESTS: usize = 2500;
const MAX_QUEUED_TRACKS: usize = 50_000;
pub const PLAYLIST_TITLE_MAX_CHARS: usize = 100;
const LOOKUP_BUDGET: Duration = Duration::from_secs(60);
pub(crate) const USER_AGENT: &str = crate::client_platform::USER_AGENT;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebAudioError {
    NoSession,
    Expired,
    WrongAccount,
    Unverified,
    Network,
    Timeout,
    Http(u16),
    Rejected,
    Format,
    TooLarge,
    Pagination,
    NoStream,
    NoAlbum,
    Canceled,
}

impl WebAudioError {
    pub fn localized(&self, language: Language) -> String {
        let text = match self {
            Self::Canceled => language.text("Загрузка трека отменена.", "Track loading was canceled."),
            Self::NoSession => language.text(
                "Сессия VK не найдена. Нажмите «Войти через VK» или используйте QR.",
                "No VK session was found. Choose Sign in with VK or use QR.",
            ),
            Self::Expired => language.text(
                "Сессия VK истекла. Войдите через VK или QR заново.",
                "The VK session has expired. Sign in with VK or QR again.",
            ),
            Self::WrongAccount => language.text(
                "Сессия VK не соответствует подтверждённому аккаунту. Повторите вход по QR-коду.",
                "The VK session does not match the approved account. Sign in with a QR code again.",
            ),
            Self::Unverified => language.text(
                "Не удалось подтвердить аккаунт сессии сайта VK. Повторите подключение музыки.",
                "Could not verify the VK website account. Reconnect your music session.",
            ),
            Self::Network => language.text("Не удалось связаться с сайтом VK.", "Could not connect to the VK website."),
            Self::Timeout => language.text("Сайт VK не ответил вовремя. Повторите попытку.", "The VK website timed out. Try again."),
            Self::Http(code) => return format!("{} {code}.", language.text("Сайт VK вернул HTTP", "The VK website returned HTTP")),
            Self::Rejected => language.text(
                "Сайт VK отклонил запрос к музыке. Проверьте, доступна ли музыка в обычном браузере.",
                "The VK website rejected the music request. Check that music is available in your normal browser.",
            ),
            Self::Format => language.text(
                "Формат музыкальной страницы VK не распознан. Возможно, сайт изменился.",
                "The VK music page format was not recognized. The website may have changed.",
            ),
            Self::TooLarge => language.text("Ответ сайта VK слишком большой.", "The VK website response is too large."),
            Self::Pagination => language.text(
                "VK вернул некорректное продолжение списка. Обновите музыку.",
                "VK returned an invalid continuation of the list. Refresh your music.",
            ),
            Self::NoStream => language.text(
                "Не удалось получить аудиопоток VK. Попробуйте ещё раз.",
                "Could not resolve the VK audio stream. Try again.",
            ),
            Self::NoAlbum => language.text(
                "VK не указал альбом или сингл для этого трека.",
                "VK did not specify an album or single for this track.",
            ),
        };
        text.into()
    }
}

/// Server-provided Home playlist metadata. Access keys stay in memory only.
/// This deliberately does not label a regular playlist as VK Mix.
#[derive(Clone)]
pub struct HomeCollection {
    pub id: String,
    pub owner_id: i64,
    pub playlist_id: i64,
    pub title: String,
    pub subtitle: String,
    pub cover_url: Option<String>,
    pub access_key: Option<Zeroizing<String>>,
    pub tracks: Vec<Track>,
}

struct ParsedHome {
    collections: Vec<HomeCollection>,
    streams: HashMap<(i64, i64), StreamInfo>,
}

type HomeTracks = (Vec<Track>, HashMap<(i64, i64), StreamInfo>);
type ParsedArtist = (ArtistPage, HashMap<(i64, i64), StreamInfo>);

/// One fresh server-selected batch from the personalized VK Mix stream.
pub struct MixPage {
    pub tracks: Vec<Track>,
    pub has_more: bool,
}

/// Search is independent of the library and keeps temporary media links in memory.
#[derive(Clone)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub artists: Vec<ArtistRef>,
    pub albums: Vec<AlbumRef>,
    pub playlists: Vec<UserPlaylist>,
    pub has_more: bool,
    pub next_offset: usize,
}

/// Public artist data returned by VK's own catalog. No synthesized library page.
#[derive(Clone)]
pub struct ArtistPage {
    pub artist: ArtistRef,
    pub image_url: Option<String>,
    pub tracks: Vec<Track>,
    pub latest_releases: Vec<crate::artist_releases::Release>,
    pub discography: Vec<crate::artist_releases::Release>,
    pub releases_error: Option<WebAudioError>,
}

#[derive(Clone)]
pub struct AlbumPage {
    pub album: AlbumRef,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u32>,
    pub description: Option<String>,
    pub creator: Option<PlaylistCreator>,
    pub tracks: Vec<Track>,
    /// VK's reported count, independent of the rows successfully returned.
    pub total_tracks: Option<usize>,
    pub stats: AlbumStats,
}

impl AlbumPage {
    pub fn confirmed_empty(&self) -> bool {
        self.tracks.is_empty() && self.total_tracks == Some(0)
    }

    fn observe_track_count(&mut self, count: Option<usize>) {
        // An empty/incomplete items response does not erase a positive count
        // supplied by playlist metadata. Absence is established only by zero.
        if let Some(count) = count {
            self.total_tracks = Some(self.total_tracks.unwrap_or(0).max(count));
        }
        if !self.tracks.is_empty() {
            self.total_tracks = Some(self.total_tracks.unwrap_or(0).max(self.tracks.len()));
        }
    }
}

/// The person or community that owns the original playlist, independent of
/// musical artists and of a saved copy's local playback identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistCreator {
    pub owner_id: i64,
    pub name: String,
}

/// Playlists returned by the current account's library. References and access
/// keys remain in memory and are never substituted with Home recommendations.
#[derive(Clone)]
pub struct UserPlaylist {
    pub album: AlbumRef,
    /// Server-provided source of a followed copy; never guessed from its title.
    pub original: Option<AlbumRef>,
    pub subtitle: String,
    pub track_count: Option<usize>,
    /// Native edit permission, never inferred from a followed copy's local ID.
    pub can_add_tracks: bool,
}

/// Optional release statistics from VK's own playlist metadata. Missing values
/// stay absent: follower counts and the local fetch time are not substitutes.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct AlbumStats {
    pub plays: Option<u64>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum MixSource {
    #[default]
    Personal,
    Track((i64, i64)),
    Playlist((i64, i64)),
}

impl MixSource {
    fn params(self) -> Vec<(&'static str, String)> {
        match self {
            Self::Personal => vec![("mix_id", "vk_mix".into())],
            Self::Track((owner, audio)) => vec![
                ("mix_id", "track_mix".into()),
                ("entity_id", format!("{owner}_{audio}")),
            ],
            Self::Playlist((owner, playlist)) => vec![
                ("mix_id", "playlist_mix".into()),
                ("entity_id", format!("{owner}_{playlist}")),
            ],
        }
    }
}

#[cfg(test)]
#[path = "web_audio_playlist_mix_tests.rs"]
mod playlist_mix_tests;

// No Debug: web tokens are credentials and only live in this verified client.
#[derive(Clone)]
struct MixSession {
    token: Zeroizing<String>,
    expires: i64,
    api_host: &'static str,
    previous_batch: Vec<(i64, i64)>,
    previous_source: MixSource,
}

// No Debug: both the client and cookies hold session credentials.
#[derive(Clone)]
pub struct WebAudioClient {
    http: Client,
    host: &'static str,
    cookies: Arc<Vec<SessionCookie>>,
    state: Arc<Mutex<State>>,
    page_gate: Arc<Mutex<()>>,
    mix_gate: Arc<Mutex<()>>,
}

#[derive(Default)]
struct State {
    verified_owner: Option<i64>,
    generation: u64,
    cursor: Option<Cursor>,
    streams: HashMap<(i64, i64), StreamInfo>,
    mix: Option<MixSession>,
    followed_playlists: playlist_follow::FollowState,
}

// Per-track URL hashes are credentials for the private reload endpoint.
#[derive(Clone, Default)]
struct StreamInfo {
    reload_id: Option<Zeroizing<String>>,
    original_url: Option<Zeroizing<String>>,
    access_key: Option<Zeroizing<String>>,
    track_code: Option<String>,
}

#[derive(Clone, Copy)]
enum LibraryAction {
    Add,
    Remove,
    RemoveSource,
    Restore,
}

impl LibraryAction {
    fn method(self) -> &'static str {
        match self {
            Self::Add => "audio.add",
            Self::Remove | Self::RemoveSource => "audio.delete",
            Self::Restore => "audio.restore",
        }
    }
}

#[derive(Default)]
struct Cursor {
    owner: i64,
    // Logical delivery offset; the website's actual position is opaque `next`.
    offset: usize,
    section: Option<String>,
    next: Option<String>,
    started: bool,
    count: Option<usize>,
    count_is_authoritative: bool,
    pending: VecDeque<Track>,
    seen_tracks: HashSet<(i64, i64)>,
    consumed_cursors: HashSet<String>,
}

struct CatalogLookup<'a> {
    key: (i64, i64),
    canceled: &'a AtomicBool,
    deadline: Instant,
}
impl CatalogLookup<'_> {
    fn check(&self) -> Result<(), WebAudioError> {
        if self.canceled.load(Ordering::Acquire) {
            Err(WebAudioError::Canceled)
        } else if Instant::now() >= self.deadline {
            Err(WebAudioError::Timeout)
        } else {
            Ok(())
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn domain_matches(domain: &str, host: &str) -> bool {
    // An explicit allowlist also protects restored local session files.
    matches!(
        domain,
        "vk.ru"
            | ".vk.ru"
            | "vk.com"
            | ".vk.com"
            | "www.vk.ru"
            | ".www.vk.ru"
            | "www.vk.com"
            | ".www.vk.com"
    ) && (domain == host
        || domain.strip_prefix('.') == Some(host)
        || matches!(host, "login.vk.ru" | "login.vk.com")
            && domain
                .strip_prefix('.')
                .is_some_and(|domain| host.strip_prefix("login.") == Some(domain)))
}

fn path_matches(cookie: &str, request: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/')))
}

fn live(cookie: &SessionCookie) -> bool {
    cookie.expires_unix.is_none_or(|expiry| expiry > now())
}

fn valid_cookie(cookie: &SessionCookie) -> bool {
    !cookie.name.is_empty()
        && cookie.name.len() <= 1024
        && cookie.value.len() <= 16_384
        && cookie.path.starts_with('/')
        && cookie.path.len() <= 2048
        && cookie
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        && cookie
            .value
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && !b"\";,\\".contains(&b))
}

fn cookie_header(
    cookies: &[SessionCookie],
    host: &str,
    path: &str,
) -> Result<HeaderValue, WebAudioError> {
    let mut selected: Vec<_> = cookies
        .iter()
        .filter(|c| domain_matches(&c.domain, host) && path_matches(&c.path, path) && live(c))
        .collect();
    selected.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
    let mut value = Zeroizing::new(String::new());
    for cookie in selected {
        if !valid_cookie(cookie) {
            return Err(WebAudioError::NoSession);
        }
        if !value.is_empty() {
            value.push_str("; ");
        }
        value.push_str(&cookie.name);
        value.push('=');
        value.push_str(&cookie.value);
        if value.len() > MAX_COOKIE_HEADER {
            return Err(WebAudioError::NoSession);
        }
    }
    let mut header = HeaderValue::from_str(&value).map_err(|_| WebAudioError::NoSession)?;
    header.set_sensitive(true);
    Ok(header)
}

fn network(error: reqwest::Error) -> WebAudioError {
    if error.is_timeout() {
        WebAudioError::Timeout
    } else {
        WebAudioError::Network
    }
}

fn body(response: Response) -> Result<Zeroizing<String>, WebAudioError> {
    let status = response.status();
    if status.as_u16() == 401 {
        return Err(WebAudioError::Expired);
    }
    if !status.is_success() {
        return Err(WebAudioError::Http(status.as_u16()));
    }
    let windows_1251 = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|header| header.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("windows-1251"));
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| WebAudioError::Network)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(WebAudioError::TooLarge);
    }
    if windows_1251 {
        let (text, _, errors) = encoding_rs::WINDOWS_1251.decode(&bytes);
        if errors {
            return Err(WebAudioError::Format);
        }
        Ok(Zeroizing::new(text.into_owned()))
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| WebAudioError::Format)?;
        Ok(Zeroizing::new(text.to_owned()))
    }
}

impl WebAudioClient {
    pub fn new(cookies: Vec<SessionCookie>) -> Result<Self, WebAudioError> {
        if cookies.len() > 256 || cookies.iter().any(|c| !valid_cookie(c)) {
            return Err(WebAudioError::NoSession);
        }
        let host = ["vk.ru", "vk.com"]
            .into_iter()
            .find(|host| {
                cookies.iter().any(|c| {
                    domain_matches(&c.domain, host)
                        && matches!(c.name.as_str(), "remixsid" | "remixsid6")
                        && !c.value.is_empty()
                        && path_matches(&c.path, "/")
                        && live(c)
                })
            })
            .ok_or(WebAudioError::NoSession)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .redirect(Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| WebAudioError::Network)?;
        Ok(Self {
            http,
            host,
            cookies: Arc::new(cookies),
            state: Arc::new(Mutex::new(State::default())),
            page_gate: Arc::new(Mutex::new(())),
            mix_gate: Arc::new(Mutex::new(())),
        })
    }

    /// Verify the website's current viewer, not the owner of a requested playlist.
    pub fn verify_owner(&self, expected_id: i64) -> Result<(), WebAudioError> {
        self.reset_verification()?;
        if expected_id <= 0 {
            return Err(WebAudioError::WrongAccount);
        }
        let profile = self.fetch_viewer()?;
        if profile.id != expected_id {
            return Err(WebAudioError::WrongAccount);
        }
        self.accept_profile(profile.id)
    }

    /// Identify the current account of this client's own website session. This
    /// does not use playlist owner IDs or read any external browser profile.
    pub fn session_profile(&self) -> Result<Profile, WebAudioError> {
        self.reset_verification()?;
        let profile = self.fetch_viewer()?;
        self.accept_profile(profile.id)?;
        Ok(profile)
    }

    /// Recheck the viewer during a library refresh without discarding the
    /// current radio, prefetched audio, or another view's stream credentials.
    pub fn refresh_profile(&self, expected_owner: i64) -> Result<Profile, WebAudioError> {
        let profile = self.refresh_profile_request(expected_owner, || self.fetch_viewer())?;
        if profile.has_name() {
            return Ok(profile);
        }
        let canceled = AtomicBool::new(false);
        let generation = self.home_owner(expected_owner, &canceled)?;
        self.profile_metadata_request(profile, generation, &canceled, |method, params| {
            self.detail_request(expected_owner, generation, method, params, &canceled)
        })
    }

    fn profile_metadata_request(
        &self,
        mut profile: Profile,
        generation: u64,
        canceled: &AtomicBool,
        request: impl FnOnce(&'static str, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
    ) -> Result<Profile, WebAudioError> {
        self.commit_home_streams(profile.id, generation, HashMap::new(), canceled)?;
        let result = request(
            "users.get",
            vec![
                ("user_ids", profile.id.to_string()),
                ("fields", "photo_200".into()),
            ],
        );
        self.commit_home_streams(profile.id, generation, HashMap::new(), canceled)?;
        match result {
            Ok(value) => {
                if let Some(fresh) = value
                    .as_array()
                    .or_else(|| value.get("response").and_then(Value::as_array))
                    .into_iter()
                    .flatten()
                    .find_map(|user| profile_from_user(user, profile.id))
                {
                    profile.merge_metadata(&fresh);
                }
            }
            Err(
                error @ (WebAudioError::WrongAccount
                | WebAudioError::Unverified
                | WebAudioError::Canceled),
            ) => return Err(error),
            // The website session is already verified; optional name lookup
            // failure must not prevent access to the user's music.
            Err(_) => {}
        }
        Ok(profile)
    }

    fn refresh_profile_request(
        &self,
        expected_owner: i64,
        fetch: impl FnOnce() -> Result<Profile, WebAudioError>,
    ) -> Result<Profile, WebAudioError> {
        let invalidate = |state: &mut State| {
            state.verified_owner = None;
            state.generation = state.generation.wrapping_add(1);
            state.cursor = None;
            state.streams.clear();
            state.mix = None;
        };
        let (generation, previous_owner) = {
            let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            if expected_owner <= 0
                || state
                    .verified_owner
                    .is_some_and(|owner| owner != expected_owner)
            {
                invalidate(&mut state);
                return Err(WebAudioError::WrongAccount);
            }
            (state.generation, state.verified_owner)
        };
        let response = fetch();
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        if state.generation != generation || state.verified_owner != previous_owner {
            return Err(WebAudioError::Unverified);
        }
        match response {
            Ok(profile) if profile.id == expected_owner => {
                if previous_owner.is_none() {
                    // Only an initial verification starts a new generation.
                    invalidate(&mut state);
                    state.verified_owner = Some(expected_owner);
                }
                Ok(profile)
            }
            Ok(_) => {
                invalidate(&mut state);
                Err(WebAudioError::WrongAccount)
            }
            Err(
                error @ (WebAudioError::Expired
                | WebAudioError::NoSession
                | WebAudioError::WrongAccount
                | WebAudioError::Unverified),
            ) => {
                invalidate(&mut state);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn reset_verification(&self) -> Result<(), WebAudioError> {
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        state.verified_owner = None;
        state.generation = state.generation.wrapping_add(1);
        state.cursor = None;
        state.streams.clear();
        state.mix = None;
        Ok(())
    }

    fn accept_profile(&self, id: i64) -> Result<(), WebAudioError> {
        if id <= 0 {
            return Err(WebAudioError::Expired);
        }
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        state.verified_owner = Some(id);
        state.generation = state.generation.wrapping_add(1);
        state.cursor = None;
        state.streams.clear();
        state.mix = None;
        Ok(())
    }

    fn fetch_viewer(&self) -> Result<Profile, WebAudioError> {
        let mut url = reqwest::Url::parse(&format!("https://{}/feed", self.host))
            .map_err(|_| WebAudioError::Format)?;
        // Follow only HTTPS redirects on the same exact website host, rebuilding
        // the cookie header for each path. Nothing is sent to login or third parties.
        for _ in 0..5 {
            let response = self
                .http
                .get(url.clone())
                .header(COOKIE, cookie_header(&self.cookies, self.host, url.path())?)
                .send()
                .map_err(network)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or(WebAudioError::Expired)?;
                let target = url.join(location).map_err(|_| WebAudioError::Expired)?;
                if target.scheme() != "https"
                    || target.host_str() != Some(self.host)
                    || target.port_or_known_default() != Some(443)
                    || !target.username().is_empty()
                    || target.password().is_some()
                {
                    return Err(WebAudioError::Expired);
                }
                url = target;
                continue;
            }
            let html = body(response)?;
            return profile_from_html(&html);
        }
        Err(WebAudioError::Expired)
    }

    /// Read one website response (the initial library normally contains 100).
    pub fn page(&self, owner_id: i64, offset: usize) -> Result<AudioPage, WebAudioError> {
        let page = self.catalog_batch(owner_id, offset, LOGICAL_PAGE_SIZE, 1, |params| {
            self.request_catalog(owner_id, params)
        })?;
        self.page_with_library_total(owner_id, offset, page)
    }

    /// Return one logical page even when VK supplies smaller continuations.
    /// Surplus tracks stay buffered; callers advance by returned items.len().
    pub fn page_batch(
        &self,
        owner_id: i64,
        offset: usize,
        limit: usize,
    ) -> Result<AudioPage, WebAudioError> {
        if !(1..=LOGICAL_PAGE_SIZE).contains(&limit) {
            return Err(WebAudioError::Pagination);
        }
        let page = self.catalog_batch(owner_id, offset, limit, MAX_BATCH_REQUESTS, |params| {
            self.request_catalog(owner_id, params)
        })?;
        self.page_with_library_total(owner_id, offset, page)
    }

    /// Read the account's catalog total without downloading its track list or
    /// consuming the UI's opaque website continuation.
    pub fn library_count(
        &self,
        owner_id: i64,
        canceled: &AtomicBool,
    ) -> Result<usize, WebAudioError> {
        self.library_count_request(owner_id, canceled, |generation, params| {
            self.detail_request(owner_id, generation, "audio.get", params, canceled)
        })
    }

    fn library_count_request(
        &self,
        owner_id: i64,
        canceled: &AtomicBool,
        request: impl FnOnce(u64, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
    ) -> Result<usize, WebAudioError> {
        let generation = self.home_owner(owner_id, canceled)?;
        let response = request(
            generation,
            vec![
                ("owner_id", owner_id.to_string()),
                ("count", "1".into()),
                ("offset", "0".into()),
            ],
        )?;
        let count = response
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or(WebAudioError::Format)?;
        self.commit_home_streams(owner_id, generation, HashMap::new(), canceled)?;
        Ok(count)
    }

    fn page_with_library_total(
        &self,
        owner_id: i64,
        offset: usize,
        page: AudioPage,
    ) -> Result<AudioPage, WebAudioError> {
        let canceled = AtomicBool::new(false);
        let (page, confirmed) = self.enrich_library_total(owner_id, offset, page, || {
            self.library_count(owner_id, &canceled)
        })?;
        if let Some(total) = confirmed {
            // This is already on the caller's library-loading worker. A failed
            // metadata-cache write must not break otherwise playable music.
            let _ = crate::library_cache::save_total(owner_id, total);
        }
        Ok(page)
    }

    fn enrich_library_total(
        &self,
        owner_id: i64,
        offset: usize,
        mut page: AudioPage,
        count: impl FnOnce() -> Result<usize, WebAudioError>,
    ) -> Result<(AudioPage, Option<usize>), WebAudioError> {
        if offset != 0 {
            return Ok((page, None));
        }
        let generation = self
            .state
            .lock()
            .map_err(|_| WebAudioError::Unverified)?
            .generation;
        let total = match count() {
            Ok(0) if !page.items.is_empty() => None,
            Ok(total) => Some(total),
            Err(
                error @ (WebAudioError::WrongAccount
                | WebAudioError::Unverified
                | WebAudioError::Canceled),
            ) => return Err(error),
            Err(_) => None,
        };
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        if state.verified_owner != Some(owner_id) || state.generation != generation {
            return Err(WebAudioError::Unverified);
        }
        if let Some(total) = total {
            page.count = Some(total);
            if let Some(cursor) = &mut state.cursor {
                cursor.count = Some(total);
                cursor.count_is_authoritative = true;
            }
        } else {
            // Website totalCount=0 is often a placeholder despite real rows.
            page.count = page
                .count
                .filter(|total| *total > 0 || page.items.is_empty());
        }
        Ok((page, total))
    }

    fn request_catalog(
        &self,
        owner_id: i64,
        params: Vec<(&'static str, String)>,
    ) -> Result<ParsedPage, WebAudioError> {
        let response = self
            .http
            .post(format!("https://{}/al_audio.php", self.host))
            .header(
                COOKIE,
                cookie_header(&self.cookies, self.host, "/al_audio.php")?,
            )
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/audios{}", self.host, owner_id))
            .form(&params)
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        parse_page(&body(response)?)
    }

    fn catalog_batch(
        &self,
        owner_id: i64,
        offset: usize,
        limit: usize,
        max_requests: usize,
        fetch: impl FnMut(Vec<(&'static str, String)>) -> Result<ParsedPage, WebAudioError>,
    ) -> Result<AudioPage, WebAudioError> {
        self.catalog_fetch(owner_id, offset, limit, max_requests, None, fetch)
    }

    fn catalog_fetch(
        &self,
        owner_id: i64,
        offset: usize,
        limit: usize,
        max_requests: usize,
        lookup: Option<&CatalogLookup<'_>>,
        mut fetch: impl FnMut(Vec<(&'static str, String)>) -> Result<ParsedPage, WebAudioError>,
    ) -> Result<AudioPage, WebAudioError> {
        // Serialize catalog calls without locking stream resolution during HTTP.
        let _page = if let Some(lookup) = lookup {
            loop {
                lookup.check()?;
                match self.page_gate.try_lock() {
                    Ok(guard) => break guard,
                    Err(TryLockError::Poisoned(_)) => return Err(WebAudioError::Pagination),
                    Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(10)),
                }
            }
        } else {
            self.page_gate
                .lock()
                .map_err(|_| WebAudioError::Pagination)?
        };
        let generation = {
            let mut state = self.state.lock().map_err(|_| WebAudioError::Pagination)?;
            if state.verified_owner != Some(owner_id) {
                return Err(WebAudioError::Unverified);
            }
            if lookup.is_some() {
                if state.cursor.is_none() {
                    state.cursor = Some(Cursor {
                        owner: owner_id,
                        ..Default::default()
                    });
                }
            } else if offset == 0 {
                state.cursor = Some(Cursor {
                    owner: owner_id,
                    ..Default::default()
                });
                // Rebase only the library continuation. Radio and other views
                // can still need their already-prefetched stream identities.
            } else if state
                .cursor
                .as_ref()
                .is_none_or(|cursor| cursor.owner != owner_id || cursor.offset != offset)
            {
                return Err(WebAudioError::Pagination);
            }
            state.generation
        };
        let mut items = Vec::with_capacity(limit);
        let mut requests = 0;
        loop {
            if let Some(lookup) = lookup {
                lookup.check()?;
            }
            let params = {
                let mut state = self.state.lock().map_err(|_| WebAudioError::Pagination)?;
                if state.verified_owner != Some(owner_id) || state.generation != generation {
                    return Err(WebAudioError::Unverified);
                }
                if lookup.is_some_and(|lookup| state.streams.contains_key(&lookup.key)) {
                    return Ok(AudioPage {
                        count: None,
                        items: Vec::new(),
                        exhausted: Some(false),
                    });
                }
                let cursor = state.cursor.as_mut().ok_or(WebAudioError::Pagination)?;
                while lookup.is_none() && items.len() < limit {
                    let Some(track) = cursor.pending.pop_front() else {
                        break;
                    };
                    items.push(track);
                    cursor.offset += 1;
                }
                let exhausted = cursor.started
                    && cursor.next.is_none()
                    && (lookup.is_some() || cursor.pending.is_empty());
                if lookup.is_some() && (exhausted || requests == max_requests) {
                    return Err(WebAudioError::NoStream);
                }
                if (lookup.is_none() && items.len() == limit)
                    || exhausted
                    || requests == max_requests
                {
                    if items.is_empty() && !exhausted {
                        return Err(WebAudioError::Pagination);
                    }
                    return Ok(AudioPage {
                        count: cursor.count,
                        items,
                        exhausted: Some(exhausted),
                    });
                }
                if !cursor.started {
                    vec![
                        ("act", "section".into()),
                        ("al", "1".into()),
                        ("claim", "0".into()),
                        ("is_layer", "0".into()),
                        ("owner_id", owner_id.to_string()),
                        ("section", "all".into()),
                    ]
                } else {
                    vec![
                        ("act", "load_catalog_section".into()),
                        ("al", "1".into()),
                        (
                            "section_id",
                            cursor.section.clone().ok_or(WebAudioError::Pagination)?,
                        ),
                        (
                            "start_from",
                            cursor.next.clone().ok_or(WebAudioError::Pagination)?,
                        ),
                    ]
                }
            };
            let requested_cursor = params
                .iter()
                .find(|(key, _)| *key == "start_from")
                .map(|(_, value)| value.clone());
            requests += 1;
            let parsed = fetch(params);
            if let Some(lookup) = lookup {
                lookup.check()?;
            }
            let mut state = self.state.lock().map_err(|_| WebAudioError::Pagination)?;
            if state.verified_owner != Some(owner_id) || state.generation != generation {
                return Err(WebAudioError::Unverified);
            }
            let valid = parsed.and_then(|parsed| {
                let cursor = state.cursor.as_ref().ok_or(WebAudioError::Pagination)?;
                if parsed.next.as_ref().is_some_and(|next| {
                    Some(next) == requested_cursor.as_ref()
                        || cursor.consumed_cursors.contains(next)
                }) || (parsed.next.is_some() && parsed.page.items.is_empty())
                    || (parsed.next.is_some()
                        && parsed.section.is_none()
                        && cursor.section.is_none())
                {
                    return Err(WebAudioError::Pagination);
                }
                Ok(parsed)
            });
            let parsed = match valid {
                Ok(parsed) => parsed,
                // Earlier responses remain committed and are returned rather
                // than advancing the cursor past tracks the UI never received.
                Err(error) if items.is_empty() => return Err(error),
                Err(_) => {
                    let cursor = state.cursor.as_ref().ok_or(WebAudioError::Pagination)?;
                    return Ok(AudioPage {
                        count: cursor.count,
                        items,
                        exhausted: Some(false),
                    });
                }
            };
            let cursor = state.cursor.as_mut().ok_or(WebAudioError::Pagination)?;
            let new_ids: HashSet<_> = parsed
                .page
                .items
                .iter()
                .map(Track::key)
                .filter(|key| !cursor.seen_tracks.contains(key))
                .collect();
            if cursor.pending.len().saturating_add(new_ids.len()) > MAX_QUEUED_TRACKS {
                return Err(WebAudioError::TooLarge);
            }
            if let Some(requested) = requested_cursor {
                cursor.consumed_cursors.insert(requested);
            }
            cursor.started = true;
            cursor.section = parsed.section.or_else(|| cursor.section.take());
            cursor.next = parsed.next;
            if !cursor.count_is_authoritative {
                let count = parsed.page.count.filter(|total| {
                    *total > 0 || (cursor.seen_tracks.is_empty() && parsed.page.items.is_empty())
                });
                cursor.count = count.or(cursor.count);
            }
            for track in parsed.page.items {
                if cursor.seen_tracks.insert(track.key()) {
                    cursor.pending.push_back(track);
                }
            }
            state.streams.extend(parsed.streams);
        }
    }

    /// Fetch the actual website Home section without touching library pagination.
    pub fn home(
        &self,
        expected_owner: i64,
        canceled: &AtomicBool,
    ) -> Result<Vec<HomeCollection>, WebAudioError> {
        let generation = self.home_owner(expected_owner, canceled)?;
        let text = self.home_request(
            expected_owner,
            vec![
                ("act", "section".into()),
                ("al", "1".into()),
                ("claim", "0".into()),
                ("is_layer", "0".into()),
                ("owner_id", expected_owner.to_string()),
                ("section", "general".into()),
            ],
            canceled,
        )?;
        let parsed = parse_home(&text)?;
        self.commit_home_streams(expected_owner, generation, parsed.streams, canceled)?;
        Ok(parsed.collections)
    }

    /// VK's real continuous personalized stream. Its current browser client
    /// repeats getStreamMixAudios(mix_id="vk_mix") without a remote cursor;
    /// each call appends the server's next batch. This has no library fallback.
    /// Sources: current VK web chunks c7995f35.54855bb3.js (fetchNext),
    /// common.1151d726.js (AudioApi/API_VERSION), and core_spa_vk.c13d3121.js
    /// (webToken and the official web_token transport).
    pub fn vk_mix(
        &self,
        expected_owner: i64,
        canceled: &AtomicBool,
    ) -> Result<MixPage, WebAudioError> {
        self.stream_mix_request(
            expected_owner,
            MixSource::Personal,
            canceled,
            |session, params| self.request_stream_mix(session, params),
        )
    }

    /// Current first-party Track mix identity sends its source type separately
    /// from OwnedIdentity.rawId: mix_id=track_mix, entity_id=owner_id_audio_id.
    pub fn track_mix(
        &self,
        expected_owner: i64,
        source: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<MixPage, WebAudioError> {
        if source.0 == 0 || source.0.checked_abs().is_none() || source.1 <= 0 {
            return Err(WebAudioError::Format);
        }
        self.stream_mix_request(
            expected_owner,
            MixSource::Track(source),
            canceled,
            |session, params| self.request_stream_mix(session, params),
        )
    }

    /// VK's native continuous playlist radio. The first-party playlist source
    /// uses its displayed data identity, not an access key or an audio ID:
    /// mix_id=playlist_mix, entity_id=owner_id_playlist_id.
    pub fn playlist_mix(
        &self,
        expected_owner: i64,
        playlist: &AlbumRef,
        canceled: &AtomicBool,
    ) -> Result<MixPage, WebAudioError> {
        if playlist.owner_id == 0 || playlist.owner_id.checked_abs().is_none() || playlist.id <= 0 {
            return Err(WebAudioError::Format);
        }
        self.stream_mix_request(
            expected_owner,
            MixSource::Playlist((playlist.owner_id, playlist.id)),
            canceled,
            |session, params| self.request_stream_mix(session, params),
        )
    }

    fn request_stream_mix(
        &self,
        session: &MixSession,
        params: Vec<(&'static str, String)>,
    ) -> Result<Zeroizing<String>, WebAudioError> {
        let mut form: Vec<_> = params
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        form.push(("access_token", session.token.as_str()));
        let response = self
            .http
            .post(format!(
                "https://{}/method/audio.getStreamMixAudios?v=5.289",
                session.api_host
            ))
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/", self.host))
            .form(&form)
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        body(response)
    }

    fn stream_mix_request(
        &self,
        expected_owner: i64,
        source: MixSource,
        canceled: &AtomicBool,
        request: impl FnOnce(
            &MixSession,
            Vec<(&'static str, String)>,
        ) -> Result<Zeroizing<String>, WebAudioError>,
    ) -> Result<MixPage, WebAudioError> {
        let generation = self.home_owner(expected_owner, canceled)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let _mix = loop {
            if canceled.load(Ordering::Acquire) {
                return Err(WebAudioError::Canceled);
            }
            match self.mix_gate.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(_)) => return Err(WebAudioError::Pagination),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(TryLockError::WouldBlock) => return Err(WebAudioError::Timeout),
            }
        };
        let cached = {
            let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            if state.verified_owner != Some(expected_owner) || state.generation != generation {
                return Err(WebAudioError::Unverified);
            }
            state
                .mix
                .as_ref()
                .filter(|mix| mix.expires > now() + 60)
                .cloned()
        };
        let mut session = match cached {
            Some(session) => session,
            None => self.fetch_mix_session(expected_owner, canceled)?,
        };
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        if session.previous_source != source {
            session.previous_batch.clear();
        }
        let text = request(&session, source.params()).inspect_err(|error| {
            if *error == WebAudioError::Expired {
                self.forget_mix_session(expected_owner, generation);
            }
        })?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        let (mut tracks, streams) = parse_mix_page(&text).inspect_err(|error| {
            if *error == WebAudioError::Expired {
                self.forget_mix_session(expected_owner, generation);
            }
        })?;
        // Only reload credentials and media URLs in `state.streams` are used for
        // playback. Public queue rows do not carry a short-lived media URL.
        for track in &mut tracks {
            track.url = None;
        }
        let keys: Vec<_> = tracks.iter().map(Track::key).collect();
        if !keys.is_empty() && keys == session.previous_batch {
            return Err(WebAudioError::Pagination);
        }
        session.previous_batch = keys;
        session.previous_source = source;
        self.commit_home_streams(expected_owner, generation, streams, canceled)?;
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        if state.verified_owner != Some(expected_owner) || state.generation != generation {
            return Err(WebAudioError::Unverified);
        }
        state.mix = Some(session);
        Ok(MixPage {
            has_more: !tracks.is_empty(),
            tracks,
        })
    }

    /// Suggestions from VK's explicit popular-search block, never local history.
    pub fn search_popular(
        &self,
        owner: i64,
        canceled: &AtomicBool,
    ) -> Result<Vec<String>, WebAudioError> {
        let generation = self.home_owner(owner, canceled)?;
        let response = self.detail_request(
            owner,
            generation,
            "catalog.getAudioSearch",
            vec![("query", String::new()), ("need_blocks", "1".into())],
            canceled,
        )?;
        parse_popular_queries(&response)
    }

    pub fn search_hints(
        &self,
        owner: i64,
        query: &str,
        canceled: &AtomicBool,
    ) -> Result<Vec<String>, WebAudioError> {
        let generation = self.home_owner(owner, canceled)?;
        let query = search_query(query)?;
        // The current native method rejects count=100; its website limit is 10.
        let response = self.detail_request(
            owner,
            generation,
            "audio.getSearchSuggestions",
            vec![("query", query.into()), ("count", "10".into())],
            canceled,
        )?;
        parse_query_hints(&response)
    }

    pub fn search(
        &self,
        owner: i64,
        query: &str,
        offset: usize,
        canceled: &AtomicBool,
    ) -> Result<SearchResults, WebAudioError> {
        self.search_request(
            owner,
            query,
            offset,
            canceled,
            |generation, method, params| {
                self.detail_request(owner, generation, method, params, canceled)
            },
        )
    }

    fn search_request(
        &self,
        owner: i64,
        query: &str,
        offset: usize,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<SearchResults, WebAudioError> {
        let generation = self.home_owner(owner, canceled)?;
        let query = search_query(query)?;
        if offset >= MAX_QUEUED_TRACKS {
            return Err(WebAudioError::TooLarge);
        }
        let response = request(
            generation,
            "audio.search",
            vec![
                ("q", query.into()),
                ("count", "100".into()),
                ("offset", offset.to_string()),
            ],
        )?;
        let (mut page, streams) = parse_search_tracks(&response, offset)?;
        if offset == 0 {
            let artists = request(
                generation,
                "audio.searchArtists",
                vec![
                    ("q", query.into()),
                    ("count", "12".into()),
                    ("offset", "0".into()),
                ],
            )?;
            page.artists = parse_search_artists(&artists)?;
            let albums = request(
                generation,
                "audio.searchAlbums",
                vec![
                    ("q", query.into()),
                    ("count", "12".into()),
                    ("offset", "0".into()),
                ],
            )?;
            page.albums = parse_search_albums(&albums)?;
            let playlists = request(
                generation,
                "audio.searchPlaylists",
                vec![
                    ("q", query.into()),
                    ("count", "20".into()),
                    ("offset", "0".into()),
                ],
            )?;
            page.playlists = parse_search_playlists(&playlists)?;
        }
        self.commit_home_streams(owner, generation, streams, canceled)?;
        Ok(page)
    }

    /// Resolve canonical artist/album references for an older cached track.
    /// The returned row never exposes its temporary playback URL to the UI.
    pub fn track_metadata(
        &self,
        expected_owner: i64,
        key: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<Track, WebAudioError> {
        self.tracks_metadata(expected_owner, &[key], canceled)?
            .pop()
            .ok_or(WebAudioError::Format)
    }

    /// Lyrics use the exact audio identity and the existing verified web session.
    pub fn track_lyrics(
        &self,
        expected_owner: i64,
        key: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<Value, WebAudioError> {
        if key.0 == 0 || key.0.checked_abs().is_none() || key.1 <= 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        self.detail_request(
            expected_owner,
            generation,
            "audio.getLyrics",
            vec![("audio_id", format!("{}_{}", key.0, key.1))],
            canceled,
        )
    }

    /// Bounded metadata hydration for older cache rows. It preserves requested
    /// identities/order and never advances the library's delivery cursor.
    pub fn tracks_metadata(
        &self,
        expected_owner: i64,
        keys: &[(i64, i64)],
        canceled: &AtomicBool,
    ) -> Result<Vec<Track>, WebAudioError> {
        if keys.is_empty() || keys.len() > 100 || keys.iter().any(|key| key.0 == 0 || key.1 <= 0) {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let value = self.detail_request(
            expected_owner,
            generation,
            "audio.getById",
            vec![(
                "audios",
                keys.iter()
                    .map(|key| format!("{}_{}", key.0, key.1))
                    .collect::<Vec<_>>()
                    .join(","),
            )],
            canceled,
        )?;
        let list = value.as_array().ok_or(WebAudioError::Format)?;
        let requested: HashSet<_> = keys.iter().copied().collect();
        let rows: Vec<_> = list
            .iter()
            .filter(|row| {
                row.get("owner_id")
                    .and_then(number)
                    .zip(row.get("id").and_then(number))
                    .is_some_and(|key| requested.contains(&key))
            })
            .cloned()
            .collect();
        let (tracks, streams) = home_tracks(&rows)?;
        let mut pool: HashMap<_, _> = tracks
            .into_iter()
            .map(|mut track| {
                track.url = None;
                (track.key(), track)
            })
            .collect();
        let tracks = keys.iter().filter_map(|key| pool.remove(key)).collect();
        self.commit_home_streams(expected_owner, generation, streams, canceled)?;
        Ok(tracks)
    }

    /// Confirm the exact public link identity without resolving an audio URL.
    /// Only a successful empty response establishes absence; errors stay unknown.
    pub fn verify_track_page(
        &self,
        expected_owner: i64,
        key: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<bool, WebAudioError> {
        self.verify_track_page_request(
            expected_owner,
            key,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    fn verify_track_page_request(
        &self,
        expected_owner: i64,
        key: (i64, i64),
        canceled: &AtomicBool,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<bool, WebAudioError> {
        if key.0 == 0 || key.1 <= 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let value = request(
            generation,
            "audio.getById",
            vec![("audios", format!("{}_{}", key.0, key.1))],
        )?;
        // This validates account/generation/cancellation without adding streams.
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let rows = value.as_array().ok_or(WebAudioError::Format)?;
        if rows.is_empty() {
            return Ok(false);
        }
        if rows.len() != 1 || parse_track(&rows[0])?.key() != key {
            return Err(WebAudioError::Format);
        }
        Ok(true)
    }

    /// Read only the album's metadata, without private access keys, track pages
    /// or creator lookups. A matching response confirms this link's identity.
    pub fn verify_album_page(
        &self,
        expected_owner: i64,
        album_owner: i64,
        album_id: i64,
        canceled: &AtomicBool,
    ) -> Result<bool, WebAudioError> {
        self.verify_album_page_request(
            expected_owner,
            album_owner,
            album_id,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    fn verify_album_page_request(
        &self,
        expected_owner: i64,
        album_owner: i64,
        album_id: i64,
        canceled: &AtomicBool,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<bool, WebAudioError> {
        if album_owner == 0 || album_id <= 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let value = request(
            generation,
            "audio.getPlaylistById",
            vec![
                ("owner_id", album_owner.to_string()),
                ("playlist_id", album_id.to_string()),
            ],
        )?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let source = value.get("playlist").unwrap_or(&value);
        if source.is_null()
            || source.as_object().is_some_and(|value| value.is_empty())
            || source.as_array().is_some_and(|value| value.is_empty())
        {
            return Ok(false);
        }
        let album = AlbumRef::from_value(source).ok_or(WebAudioError::Format)?;
        if (album.owner_id, album.id) != (album_owner, album_id) {
            return Err(WebAudioError::Format);
        }
        Ok(true)
    }

    /// Add a source audio to My Music. VK may allocate a different ID for the
    /// owned copy; callers must retain this exact source-to-owned association.
    pub fn add_to_library(
        &self,
        expected_owner: i64,
        source: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<(i64, i64), WebAudioError> {
        let response = self.library_action_request(
            expected_owner,
            source,
            LibraryAction::Add,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )?;
        parse_library_add(&response, expected_owner)
    }

    /// Create an empty playlist in the verified account. The native service
    /// takes title/description/audio_ids and has no verified privacy switch.
    /// This mutation is sent once; an ambiguous network result is not retried.
    pub fn create_playlist(
        &self,
        expected_owner: i64,
        title: &str,
        canceled: &AtomicBool,
    ) -> Result<UserPlaylist, WebAudioError> {
        self.create_playlist_request(
            expected_owner,
            title,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    fn create_playlist_request(
        &self,
        expected_owner: i64,
        title: &str,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<UserPlaylist, WebAudioError> {
        let title = title.trim();
        if title.is_empty()
            || title.chars().count() > PLAYLIST_TITLE_MAX_CHARS
            || title.chars().any(char::is_control)
        {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        // Match first-party audio/playlists.createFx. New playlists are empty;
        // neither the current queue nor library is implicitly copied into one.
        let response = request(
            generation,
            "audio.createPlaylist",
            vec![
                ("owner_id", expected_owner.to_string()),
                ("title", title.into()),
                ("description", String::new()),
                ("audio_ids", String::new()),
            ],
        )?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let returned_owner = response.get("owner_id").and_then(number);
        if returned_owner.is_some_and(|owner| owner != expected_owner) {
            return Err(WebAudioError::WrongAccount);
        }
        if returned_owner != Some(expected_owner)
            || response.get("id").and_then(number).is_none_or(|id| id <= 0)
        {
            return Err(WebAudioError::Format);
        }
        // Parse the direct native response without inventing edit permission.
        // A subsequent sidebar refresh supplies it if creation omitted it.
        let mut parsed = parse_user_playlists(&serde_json::json!({"items": [response]}))?;
        parsed.items.pop().ok_or(WebAudioError::Format)
    }

    /// Append one audio using VK's own playlist-owning API. The returned ID
    /// belongs to the target playlist owner and can differ from the source.
    /// Permissions are read afresh before this non-retried mutation.
    pub fn add_to_playlist(
        &self,
        expected_owner: i64,
        playlist: &AlbumRef,
        source: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<(i64, i64), WebAudioError> {
        self.add_to_playlist_request(
            expected_owner,
            playlist,
            source,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    fn add_to_playlist_request(
        &self,
        expected_owner: i64,
        playlist: &AlbumRef,
        source: (i64, i64),
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<(i64, i64), WebAudioError> {
        if playlist.owner_id == 0
            || playlist.owner_id.checked_abs().is_none()
            || playlist.id <= 0
            || source.0 == 0
            || source.0.checked_abs().is_none()
            || source.1 <= 0
        {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let mut identity = vec![
            ("owner_id", playlist.owner_id.to_string()),
            ("playlist_id", playlist.id.to_string()),
        ];
        if let Some(access_key) = &playlist.access_key {
            identity.push(("access_key", access_key.clone()));
        }
        let metadata = request(generation, "audio.getPlaylistById", identity)?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let fresh = metadata.get("playlist").unwrap_or(&metadata);
        let returned = AlbumRef::from_value(fresh).ok_or(WebAudioError::Format)?;
        if returned.owner_id != playlist.owner_id || returned.id != playlist.id {
            return Err(WebAudioError::Format);
        }
        if !playlist_can_add_tracks(fresh) {
            return Err(WebAudioError::Rejected);
        }
        let access_raw_id = {
            let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            if canceled.load(Ordering::Acquire) {
                return Err(WebAudioError::Canceled);
            }
            if state.verified_owner != Some(expected_owner) || state.generation != generation {
                return Err(WebAudioError::Unverified);
            }
            let raw_id = format!("{}_{}", source.0, source.1);
            if let Some(key) = state
                .streams
                .get(&source)
                .and_then(|info| info.access_key.as_ref())
                .filter(|key| !key.is_empty())
            {
                // audio_ids is a comma-separated native API field. An opaque
                // access key must never introduce another audio identity.
                if key.len() > 16_384
                    || key.chars().any(|character| {
                        character == ',' || character.is_whitespace() || character.is_control()
                    })
                {
                    return Err(WebAudioError::Format);
                }
                format!("{raw_id}_{}", key.as_str())
            } else {
                raw_id
            }
        };
        // Match MusicPlaylistAudiosOwningService.add exactly: source access ID
        // goes in audio_ids, not a separate access_key or an audio.add call.
        let response = request(
            generation,
            "audio.addToPlaylist",
            vec![
                ("owner_id", playlist.owner_id.to_string()),
                ("playlist_id", playlist.id.to_string()),
                ("audio_ids", access_raw_id),
            ],
        )?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let (owned, track_code) = parse_playlist_add(&response, playlist.owner_id)?;
        if let Some(track_code) = track_code {
            let mut info = self
                .state
                .lock()
                .map_err(|_| WebAudioError::Unverified)?
                .streams
                .get(&owned)
                .cloned()
                .unwrap_or_default();
            info.track_code = Some(track_code);
            self.commit_home_streams(
                expected_owner,
                generation,
                HashMap::from([(owned, info)]),
                canceled,
            )?;
        }
        Ok(owned)
    }

    /// Remove an owned audio. The native API can return several deleted copies,
    /// so every confirmed identity is returned for local library reconciliation.
    pub fn remove_from_library(
        &self,
        expected_owner: i64,
        owned: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<Vec<(i64, i64)>, WebAudioError> {
        let response = self.library_action_request(
            expected_owner,
            owned,
            LibraryAction::Remove,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )?;
        parse_library_remove(&response, expected_owner, owned)
    }

    /// VK's own owning service also deletes by original source identity. This
    /// handles saved copies with no canonical release ID, after independently
    /// confirming membership in a fresh authenticated metadata response.
    pub fn remove_source_from_library(
        &self,
        expected_owner: i64,
        source: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<Vec<(i64, i64)>, WebAudioError> {
        self.remove_source_from_library_request(
            expected_owner,
            source,
            canceled,
            || self.track_metadata(expected_owner, source, canceled),
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    fn remove_source_from_library_request(
        &self,
        expected_owner: i64,
        source: (i64, i64),
        canceled: &AtomicBool,
        metadata: impl FnOnce() -> Result<Track, WebAudioError>,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<Vec<(i64, i64)>, WebAudioError> {
        if source.0 == 0 || source.1 <= 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let fresh = metadata()?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        if fresh.key() != source || fresh.library_liked != Some(true) {
            return Err(WebAudioError::Rejected);
        }
        let response = self.library_action_request(
            expected_owner,
            source,
            LibraryAction::RemoveSource,
            canceled,
            request,
        )?;
        parse_library_source_remove(&response, expected_owner)
    }

    /// Re-add a just-removed owned track without duplicating it. VK's own
    /// owning service uses audio.restore before attempting audio.add again.
    pub fn restore_to_library(
        &self,
        expected_owner: i64,
        owned: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<(i64, i64), WebAudioError> {
        let response = self.library_action_request(
            expected_owner,
            owned,
            LibraryAction::Restore,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )?;
        parse_library_restore(&response, owned)
    }

    fn library_action_request(
        &self,
        expected_owner: i64,
        key: (i64, i64),
        action: LibraryAction,
        canceled: &AtomicBool,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<Value, WebAudioError> {
        if expected_owner <= 0 || key.0 == 0 || key.1 <= 0 {
            return Err(WebAudioError::Format);
        }
        if matches!(action, LibraryAction::Remove | LibraryAction::Restore)
            && key.0 != expected_owner
        {
            return Err(WebAudioError::WrongAccount);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let mut params = vec![
            ("audio_id", key.1.to_string()),
            ("owner_id", key.0.to_string()),
        ];
        {
            let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            if canceled.load(Ordering::Acquire) {
                return Err(WebAudioError::Canceled);
            }
            if state.verified_owner != Some(expected_owner) || state.generation != generation {
                return Err(WebAudioError::Unverified);
            }
            if let Some(info) = state.streams.get(&key) {
                if !matches!(action, LibraryAction::Restore)
                    && let Some(access_key) = &info.access_key
                {
                    params.push(("access_key", access_key.to_string()));
                }
                if matches!(action, LibraryAction::Add)
                    && let Some(track_code) = &info.track_code
                {
                    params.push(("track_code", track_code.clone()));
                }
            }
        }
        // No automatic retry: these requests change the account's library.
        let response = request(generation, action.method(), params)?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        Ok(response)
    }

    /// Current first-party catalog.getAudioArtist returns the canonical banner,
    /// popular block and release references independently of library pagination.
    pub fn artist_page(
        &self,
        expected_owner: i64,
        reference: &ArtistRef,
        canceled: &AtomicBool,
    ) -> Result<ArtistPage, WebAudioError> {
        let generation = self.home_owner(expected_owner, canceled)?;
        let reference = if reference.id.is_empty() {
            if reference.name.trim().is_empty() || reference.name.len() > 4096 {
                return Err(WebAudioError::Format);
            }
            let result = self.detail_request(
                expected_owner,
                generation,
                "audio.searchArtists",
                vec![("q", reference.name.clone()), ("count", "20".into())],
                canceled,
            )?;
            exact_artist(&result, &reference.name)?
        } else if valid_artist_id(&reference.id) {
            reference.clone()
        } else {
            return Err(WebAudioError::Format);
        };
        let value = self.detail_request(
            expected_owner,
            generation,
            "catalog.getAudioArtist",
            vec![
                ("artist_id", reference.id.clone()),
                ("need_blocks", "1".into()),
            ],
            canceled,
        )?;
        let (mut page, streams) = parse_artist_page(&value, &reference)?;
        self.commit_home_streams(expected_owner, generation, streams, canceled)?;
        if let Err(error) =
            crate::artist_catalog::load(&mut page, &value, canceled, |method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            })
        {
            if matches!(
                error,
                WebAudioError::Canceled | WebAudioError::WrongAccount | WebAudioError::Unverified
            ) {
                return Err(error);
            }
            page.releases_error = Some(error);
        }
        Ok(page)
    }

    /// Read the current account's created and saved playlists. The default
    /// endpoint list deliberately has no `owned` filter: followed references
    /// may belong to a different owner while still being in this library.
    pub fn playlists(
        &self,
        expected_owner: i64,
        canceled: &AtomicBool,
    ) -> Result<Vec<UserPlaylist>, WebAudioError> {
        const MAX_PLAYLISTS: usize = 10_000;
        let generation = self.home_owner(expected_owner, canceled)?;
        let revision = self.playlist_follow_revision(expected_owner, generation, canceled)?;
        let mut playlists = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0;
        loop {
            if offset >= MAX_PLAYLISTS {
                return Err(WebAudioError::TooLarge);
            }
            let response = self.detail_request(
                expected_owner,
                generation,
                "audio.getPlaylists",
                vec![
                    ("owner_id", expected_owner.to_string()),
                    ("count", "100".into()),
                    ("offset", offset.to_string()),
                ],
                canceled,
            )?;
            let page = parse_user_playlists(&response)?;
            let next_offset = page.next_offset(offset);
            let before = playlists.len();
            for playlist in page.items {
                if seen.insert((playlist.album.owner_id, playlist.album.id)) {
                    playlists.push(playlist);
                }
            }
            if page.received > 0 && playlists.len() == before {
                return Err(WebAudioError::Pagination);
            }
            let Some(next_offset) = next_offset else {
                self.cache_followed_playlists(
                    expected_owner,
                    generation,
                    revision,
                    &playlists,
                    canceled,
                )?;
                return Ok(playlists);
            };
            offset = next_offset;
        }
    }

    /// Fetch official album metadata plus every track, with a bounded cursor
    /// walk. Access keys stay in memory and are sent only to VK's own API host.
    pub fn album_page(
        &self,
        expected_owner: i64,
        reference: &AlbumRef,
        canceled: &AtomicBool,
    ) -> Result<AlbumPage, WebAudioError> {
        if reference.owner_id == 0 || reference.id == 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let identity = |album: &AlbumRef| {
            vec![
                ("owner_id", album.owner_id.to_string()),
                ("playlist_id", album.id.to_string()),
                ("access_key", album.access_key.clone().unwrap_or_default()),
            ]
        };
        let mut metadata_params = identity(reference);
        // First-party getPlaylistByIdExtended asks for owner side tables using
        // extra_fields, not a different method or the public extended flag.
        metadata_params.push(("extra_fields", "owner,duration".into()));
        let value = self.detail_request(
            expected_owner,
            generation,
            "audio.getPlaylistById",
            metadata_params,
            canceled,
        )?;
        let mut page = parse_album_page(&value, reference)?;
        let source = value.get("playlist").unwrap_or(&value);
        if page.creator.is_none() && source.get("type").and_then(Value::as_str) != Some("album") {
            page.creator = self.playlist_creator(expected_owner, generation, &value, canceled)?;
        }
        let mut offset = 0;
        let mut seen = HashSet::new();
        loop {
            if canceled.load(Ordering::Acquire) {
                return Err(WebAudioError::Canceled);
            }
            if offset >= MAX_QUEUED_TRACKS {
                return Err(WebAudioError::TooLarge);
            }
            let mut params = identity(&page.album);
            params.extend([("count", "100".into()), ("offset", offset.to_string())]);
            let response =
                self.detail_request(expected_owner, generation, "audio.get", params, canceled)?;
            let rows = response
                .get("items")
                .and_then(Value::as_array)
                .ok_or(WebAudioError::Format)?;
            let (tracks, streams) = home_tracks(rows)?;
            let before = page.tracks.len();
            for mut track in tracks {
                if seen.insert(track.key()) {
                    track.url = None;
                    page.tracks.push(track);
                }
            }
            self.commit_home_streams(expected_owner, generation, streams, canceled)?;
            offset += rows.len();
            let count = response.get("count").and_then(Value::as_u64);
            page.observe_track_count(count.and_then(|count| usize::try_from(count).ok()));
            if rows.is_empty()
                || count.is_some_and(|count| offset as u64 >= count)
                || (count.is_none() && rows.len() < 100)
            {
                return Ok(page);
            }
            if page.tracks.len() == before {
                return Err(WebAudioError::Pagination);
            }
        }
    }

    fn playlist_creator(
        &self,
        expected_owner: i64,
        generation: u64,
        metadata: &Value,
        canceled: &AtomicBool,
    ) -> Result<Option<PlaylistCreator>, WebAudioError> {
        self.playlist_creator_request(
            expected_owner,
            generation,
            metadata,
            canceled,
            |method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    fn playlist_creator_request(
        &self,
        expected_owner: i64,
        generation: u64,
        metadata: &Value,
        canceled: &AtomicBool,
        request: impl FnOnce(&'static str, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
    ) -> Result<Option<PlaylistCreator>, WebAudioError> {
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let Some(owner_id) = playlist_creator_id(metadata.get("playlist").unwrap_or(metadata))
        else {
            return Ok(None);
        };
        let (method, params) = if owner_id > 0 {
            ("users.get", vec![("user_ids", owner_id.to_string())])
        } else {
            let Some(group_id) = owner_id.checked_abs() else {
                return Ok(None);
            };
            ("groups.getById", vec![("group_ids", group_id.to_string())])
        };
        let result = request(method, params);
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        match result {
            Ok(value) => Ok(playlist_creator_from_lookup(&value, owner_id)),
            Err(
                error @ (WebAudioError::WrongAccount
                | WebAudioError::Canceled
                | WebAudioError::Unverified),
            ) => Err(error),
            // A missing optional owner name must not prevent playback.
            Err(_) => Ok(None),
        }
    }

    fn detail_request(
        &self,
        owner: i64,
        generation: u64,
        method: &'static str,
        params: Vec<(&'static str, String)>,
        canceled: &AtomicBool,
    ) -> Result<Value, WebAudioError> {
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        let cached = self
            .state
            .lock()
            .map_err(|_| WebAudioError::Unverified)?
            .mix
            .as_ref()
            .filter(|session| session.expires > now() + 60)
            .cloned();
        let session = match cached {
            Some(session) => session,
            None => {
                let session = self.fetch_mix_session(owner, canceled)?;
                let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
                if canceled.load(Ordering::Acquire) {
                    return Err(WebAudioError::Canceled);
                }
                if state.verified_owner != Some(owner) || state.generation != generation {
                    return Err(WebAudioError::Unverified);
                }
                // Do not replace another request's already-established Mix
                // continuation state when concurrent pages share the token.
                if state
                    .mix
                    .as_ref()
                    .is_none_or(|cached| cached.expires <= now() + 60)
                {
                    state.mix = Some(session.clone());
                }
                session
            }
        };
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        let mut form: Vec<_> = params
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        form.push(("access_token", session.token.as_str()));
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        let response = self
            .http
            .post(format!(
                "https://{}/method/{method}?v=5.289",
                session.api_host
            ))
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/", self.host))
            .form(&form)
            .send()
            .map_err(network)?;
        let text = body(response).inspect_err(|error| {
            if *error == WebAudioError::Expired {
                self.forget_mix_session(owner, generation);
            }
        })?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        detail_response(&text).inspect_err(|error| {
            if *error == WebAudioError::Expired {
                self.forget_mix_session(owner, generation);
            }
        })
    }

    fn forget_mix_session(&self, owner: i64, generation: u64) {
        if let Ok(mut state) = self.state.lock()
            && state.verified_owner == Some(owner)
            && state.generation == generation
        {
            state.mix = None;
        }
    }

    fn fetch_mix_session(
        &self,
        owner: i64,
        canceled: &AtomicBool,
    ) -> Result<MixSession, WebAudioError> {
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        // Token metadata is read only from the app's own authenticated official
        // page, after checking that page's independent global viewer identity.
        let response = self
            .http
            .get(format!("https://{}/feed", self.host))
            .header(COOKIE, cookie_header(&self.cookies, self.host, "/feed")?)
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        let html = body(response)?;
        let viewer = viewer_record(&html).ok_or(WebAudioError::Unverified)?;
        if viewer.id != owner {
            return Err(WebAudioError::WrongAccount);
        }
        // A verified feed can expose only vk.id, without a full vk object.
        // This public API host is also specified in VK's current first-party
        // audio page; missing optional page metadata must not block the
        // official token endpoint, whose returned user_id binds the account.
        let api_host = viewer
            .object
            .and_then(|object| mix_api_host(object).ok())
            .unwrap_or("web.api.vk.ru");
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        if let Some(token) = viewer
            .object
            .and_then(|object| js_object_field(object, "webToken").ok().flatten())
            && let Ok(mut session) = mix_page_token(token, owner, false)
        {
            session.api_host = api_host;
            return Ok(session);
        }
        // The official browser uses this endpoint when the page has no BFF
        // token. Only domain-scoped VK cookies reach the explicit VK login host;
        // host-only cookies remain confined to their original host.
        let login_host = if self.host == "vk.ru" {
            "login.vk.ru"
        } else {
            "login.vk.com"
        };
        let response = self
            .http
            .post(format!("https://{login_host}/?act=web_token"))
            .header(COOKIE, cookie_header(&self.cookies, login_host, "/")?)
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/", self.host))
            .form(&[("app_id", "6287487"), ("version", "1")])
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        let text = body(response)?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        let mut session = mix_login_token(&text, owner)?;
        session.api_host = api_host;
        Ok(session)
    }

    /// Load a finite playlist advertised by VK Home. The library's opaque
    /// cursor and pending rows are independent and are never reset or consumed.
    pub fn playlist_tracks(
        &self,
        expected_owner: i64,
        collection: &HomeCollection,
        canceled: &AtomicBool,
    ) -> Result<Vec<Track>, WebAudioError> {
        let generation = self.home_owner(expected_owner, canceled)?;
        if collection.owner_id == 0 || collection.playlist_id == 0 {
            return Err(WebAudioError::Format);
        }
        let context = if collection.owner_id < 0 {
            "group_list"
        } else if collection.owner_id == expected_owner {
            "my"
        } else {
            "user_playlists"
        };
        let text = self.home_request(
            expected_owner,
            vec![
                ("act", "load_section".into()),
                ("al", "1".into()),
                ("claim", "0".into()),
                (
                    "access_hash",
                    collection
                        .access_key
                        .as_ref()
                        .map_or_else(String::new, |key| key.to_string()),
                ),
                ("context", context.into()),
                ("from_id", collection.owner_id.to_string()),
                ("is_loading_all", "1".into()),
                ("is_preload", "0".into()),
                ("offset", "0".into()),
                ("owner_id", collection.owner_id.to_string()),
                ("playlist_id", collection.playlist_id.to_string()),
                ("type", "playlist".into()),
            ],
            canceled,
        )?;
        let (tracks, streams) =
            parse_home_playlist(&text, collection.owner_id, collection.playlist_id)?;
        self.commit_home_streams(expected_owner, generation, streams, canceled)?;
        Ok(tracks)
    }

    fn home_owner(&self, expected_owner: i64, canceled: &AtomicBool) -> Result<u64, WebAudioError> {
        if expected_owner <= 0 {
            return Err(WebAudioError::WrongAccount);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if canceled.load(Ordering::Acquire) {
                return Err(WebAudioError::Canceled);
            }
            {
                let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
                if let Some(owner) = state.verified_owner {
                    return if owner == expected_owner {
                        Ok(state.generation)
                    } else {
                        Err(WebAudioError::WrongAccount)
                    };
                }
            }
            if Instant::now() >= deadline {
                return Err(WebAudioError::Unverified);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn home_request(
        &self,
        owner: i64,
        params: Vec<(&'static str, String)>,
        canceled: &AtomicBool,
    ) -> Result<Zeroizing<String>, WebAudioError> {
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        let response = self
            .http
            .post(format!("https://{}/al_audio.php", self.host))
            .header(
                COOKIE,
                cookie_header(&self.cookies, self.host, "/al_audio.php")?,
            )
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/audios{}", self.host, owner))
            .form(&params)
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        let text = body(response)?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        Ok(text)
    }

    fn commit_home_streams(
        &self,
        owner: i64,
        generation: u64,
        streams: HashMap<(i64, i64), StreamInfo>,
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        if state.verified_owner != Some(owner) || state.generation != generation {
            return Err(WebAudioError::Unverified);
        }
        for (key, info) in streams {
            if let Some(existing) = state.streams.get_mut(&key) {
                // Home can contain metadata-only previews. They must not erase
                // playable credentials already obtained from the library.
                if info.reload_id.is_some() {
                    existing.reload_id = info.reload_id;
                }
                if info.original_url.is_some() {
                    existing.original_url = info.original_url;
                }
                if info.access_key.is_some() {
                    existing.access_key = info.access_key;
                }
                if info.track_code.is_some() {
                    existing.track_code = info.track_code;
                }
            } else {
                state.streams.insert(key, info);
            }
        }
        Ok(())
    }

    /// Restore volatile playback information for a persisted metadata row.
    /// Current VK code uses the plural metadata reload before the hash-protected
    /// singular stream reload. No private hashes are read from or written to disk.
    pub fn resolve_cached(
        &self,
        key: (i64, i64),
        canceled: &AtomicBool,
    ) -> Result<String, WebAudioError> {
        let lookup = CatalogLookup {
            key,
            canceled,
            deadline: Instant::now() + LOOKUP_BUDGET,
        };
        lookup.check()?;
        let verification_deadline = Instant::now() + Duration::from_secs(20);
        let (owner, generation, mut known) = loop {
            lookup.check()?;
            {
                let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
                if let Some(owner) = state.verified_owner {
                    break (owner, state.generation, state.streams.contains_key(&key));
                }
            }
            if Instant::now() >= verification_deadline {
                return Err(WebAudioError::Unverified);
            }
            thread::sleep(Duration::from_millis(25));
        };
        if key.0 == 0 || key.1 <= 0 {
            return Err(WebAudioError::NoStream);
        }
        if known {
            let result = self.resolve(key);
            lookup.check()?;
            match result {
                Ok(url) => return Ok(url),
                // Catalog placeholders and expired per-track metadata can be
                // refreshed without discarding the library's logical cursor.
                Err(WebAudioError::NoStream) => known = false,
                Err(error) => return Err(error),
            }
        }
        if !known {
            match self.reload_metadata(owner, generation, key, &lookup) {
                Ok(()) => (),
                Err(
                    WebAudioError::NoStream
                    | WebAudioError::Format
                    | WebAudioError::Rejected
                    | WebAudioError::Http(400 | 403 | 404 | 405 | 501),
                ) => {
                    // Private tracks may need an access key that only the user's
                    // catalog provides. Preserve every fetched row for the UI.
                    self.catalog_fetch(
                        owner,
                        0,
                        0,
                        MAX_LOOKUP_REQUESTS,
                        Some(&lookup),
                        |params| self.request_catalog(owner, params),
                    )?;
                }
                Err(error) => return Err(error),
            }
        }
        lookup.check()?;
        let result = self.resolve(key);
        lookup.check()?;
        result
    }

    fn reload_metadata(
        &self,
        owner: i64,
        generation: u64,
        key: (i64, i64),
        lookup: &CatalogLookup<'_>,
    ) -> Result<(), WebAudioError> {
        lookup.check()?;
        // VK's PlaylistQueue normalizes a missing access key to the empty
        // string, producing owner_id_audio_id_. This is not a guessed hashless
        // call to the distinct singular `reload_audio` endpoint.
        let id = format!("{}_{}_", key.0, key.1);
        let response = self
            .http
            .post(format!("https://{}/al_audio.php", self.host))
            .header(
                COOKIE,
                cookie_header(&self.cookies, self.host, "/al_audio.php")?,
            )
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/audios{}", self.host, owner))
            .form(&[
                ("act", "reload_audios"),
                ("al", "1"),
                ("audio_ids", id.as_str()),
            ])
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        let row = parse_reload_row(&body(response)?, key)?;
        lookup.check()?;
        self.commit_reload_metadata(owner, generation, key, &row)
    }

    fn commit_reload_metadata(
        &self,
        owner: i64,
        generation: u64,
        key: (i64, i64),
        row: &Value,
    ) -> Result<(), WebAudioError> {
        let info = stream_info(row, key);
        if info.reload_id.is_none() && info.original_url.is_none() {
            return Err(WebAudioError::NoStream);
        }
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        if state.verified_owner != Some(owner) || state.generation != generation {
            return Err(WebAudioError::Unverified);
        }
        state.streams.insert(key, info);
        Ok(())
    }

    /// Resolve one track already returned by this verified website session.
    /// The short-lived media URL is returned to the decoder, never logged.
    pub fn resolve(&self, key: (i64, i64)) -> Result<String, WebAudioError> {
        let (viewer, info) = {
            let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            let viewer = state.verified_owner.ok_or(WebAudioError::Unverified)?;
            let info = state
                .streams
                .get(&key)
                .cloned()
                .ok_or(WebAudioError::NoStream)?;
            (viewer, info)
        };
        let Some(ids) = info.reload_id else {
            return decode_stream(
                info.original_url
                    .as_deref()
                    .map(|s| s.as_str())
                    .ok_or(WebAudioError::NoStream)?,
                viewer,
            );
        };
        let response = self
            .http
            .post(format!("https://{}/al_audio.php", self.host))
            .header(
                COOKIE,
                cookie_header(&self.cookies, self.host, "/al_audio.php")?,
            )
            .header(ORIGIN, format!("https://{}", self.host))
            .header(REFERER, format!("https://{}/audios{}", self.host, viewer))
            .form(&[("act", "reload_audio"), ("al", "1"), ("ids", ids.as_str())])
            .send()
            .map_err(network)?;
        if response.status().is_redirection() {
            return Err(WebAudioError::Expired);
        }
        let text = body(response)?;
        parse_reload(&text, key, viewer)
    }
}

fn search_query(query: &str) -> Result<&str, WebAudioError> {
    let query = query.trim();
    if query.is_empty() || query.len() > 4096 || query.chars().any(char::is_control) {
        return Err(WebAudioError::Format);
    }
    Ok(query)
}

fn query_text(value: &Value) -> Result<Option<String>, WebAudioError> {
    let value = value.as_str().ok_or(WebAudioError::Format)?;
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err(WebAudioError::Format);
    }
    let value = html_escape::decode_html_entities(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if value.len() > 4096 {
        return Err(WebAudioError::TooLarge);
    }
    Ok((!value.is_empty()).then_some(value))
}

fn search_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| number(value).map(|id| id.to_string()))
}

fn parse_popular_queries(value: &Value) -> Result<Vec<String>, WebAudioError> {
    let sections = value["catalog"]["sections"]
        .as_array()
        .ok_or(WebAudioError::Format)?;
    let default = value["catalog"]["default_section"].as_str();
    let section = sections
        .iter()
        .find(|section| section["id"].as_str() == default)
        .or_else(|| sections.first())
        .ok_or(WebAudioError::Format)?;
    let blocks = section["blocks"].as_array().ok_or(WebAudioError::Format)?;
    let block = blocks
        .iter()
        .find(|block| {
            block["data_type"] == "search_suggestions"
                && block["title"].as_str().is_some_and(|title| {
                    matches!(
                        title.trim().to_lowercase().as_str(),
                        "популярные запросы"
                            | "popular searches"
                            | "popular queries"
                            | "popular requests"
                            | "trending searches"
                    )
                })
        })
        .ok_or(WebAudioError::Format)?;
    let ids = block["suggestions_ids"]
        .as_array()
        .ok_or(WebAudioError::Format)?;
    if ids.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    let suggestions = value["suggestions"]
        .as_array()
        .ok_or(WebAudioError::Format)?;
    if suggestions.len() > 1000 {
        return Err(WebAudioError::TooLarge);
    }
    let pool: HashMap<_, _> = suggestions
        .iter()
        .filter_map(|suggestion| search_id(&suggestion["id"]).map(|id| (id, suggestion)))
        .collect();
    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    for id in ids.iter().take(20) {
        let id = search_id(id).ok_or(WebAudioError::Format)?;
        let suggestion = pool.get(&id).ok_or(WebAudioError::Format)?;
        if let Some(query) = query_text(&suggestion["title"])?
            && seen.insert(query.to_lowercase())
        {
            queries.push(query);
        }
    }
    Ok(queries)
}

fn parse_query_hints(value: &Value) -> Result<Vec<String>, WebAudioError> {
    let list = value["suggestions"]
        .as_array()
        .ok_or(WebAudioError::Format)?;
    if list.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    let mut queries = Vec::new();
    let mut seen = HashSet::new();
    for item in list.iter().take(10) {
        if let Some(query) = query_text(item)?
            && seen.insert(query.to_lowercase())
        {
            queries.push(query);
        }
    }
    Ok(queries)
}

type ParsedSearch = (SearchResults, HashMap<(i64, i64), StreamInfo>);

fn parse_search_tracks(value: &Value, offset: usize) -> Result<ParsedSearch, WebAudioError> {
    let rows = value["items"].as_array().ok_or(WebAudioError::Format)?;
    if rows.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    let (mut tracks, streams) = home_tracks(rows)?;
    for track in &mut tracks {
        track.url = None;
    }
    let next_offset = offset
        .checked_add(rows.len())
        .ok_or(WebAudioError::TooLarge)?;
    let count = match value.get("count") {
        None => None,
        Some(value) => Some(value.as_u64().ok_or(WebAudioError::Format)?),
    };
    let has_more = !rows.is_empty()
        && next_offset < MAX_QUEUED_TRACKS
        && count.map_or(rows.len() == 100, |count| (next_offset as u64) < count);
    Ok((
        SearchResults {
            tracks,
            artists: Vec::new(),
            albums: Vec::new(),
            playlists: Vec::new(),
            has_more,
            next_offset,
        },
        streams,
    ))
}

fn parse_search_artists(value: &Value) -> Result<Vec<ArtistRef>, WebAudioError> {
    let rows = value["items"].as_array().ok_or(WebAudioError::Format)?;
    if rows.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    Ok(artists_from_object(&serde_json::json!({"artists": rows}))
        .into_iter()
        .take(12)
        .collect())
}

fn parse_search_albums(value: &Value) -> Result<Vec<AlbumRef>, WebAudioError> {
    let rows = value["items"].as_array().ok_or(WebAudioError::Format)?;
    if rows.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    let mut seen = HashSet::new();
    Ok(rows
        .iter()
        .filter_map(AlbumRef::from_value)
        .filter(|album| seen.insert((album.owner_id, album.id)))
        .take(12)
        .collect())
}

fn parse_search_playlists(value: &Value) -> Result<Vec<UserPlaylist>, WebAudioError> {
    // The native search returns the same playlist metadata as getPlaylists,
    // including the original owner/id and temporary access key for opening it.
    Ok(parse_user_playlists(value)?
        .items
        .into_iter()
        .take(20)
        .collect())
}

fn detail_response(text: &str) -> Result<Value, WebAudioError> {
    let mut value: Value = serde_json::from_str(text).map_err(|_| WebAudioError::Format)?;
    if let Some(error) = value.get("error") {
        return Err(match error.get("error_code").and_then(number) {
            Some(5 | 27 | 28) => WebAudioError::Expired,
            _ => WebAudioError::Rejected,
        });
    }
    value
        .get_mut("response")
        .map(Value::take)
        .ok_or(WebAudioError::Format)
}

fn parse_library_add(value: &Value, owner: i64) -> Result<(i64, i64), WebAudioError> {
    // Current first-party API returns an items array; retain compatibility with
    // the older public method's integer response without accepting boolean 1.
    let id = value
        .get("items")
        .and_then(Value::as_array)
        .filter(|items| items.len() == 1)
        .and_then(|items| items[0].get("new_audio_id"))
        .and_then(number)
        .or_else(|| number(value))
        .filter(|id| *id > 0)
        .ok_or(WebAudioError::Format)?;
    if owner <= 0 {
        return Err(WebAudioError::WrongAccount);
    }
    Ok((owner, id))
}

fn parse_library_remove(
    value: &Value,
    owner: i64,
    requested: (i64, i64),
) -> Result<Vec<(i64, i64)>, WebAudioError> {
    if owner <= 0 || requested.0 != owner || requested.1 <= 0 {
        return Err(WebAudioError::WrongAccount);
    }
    if number(value) == Some(1) {
        return Ok(vec![requested]);
    }
    let removed = parse_library_source_remove(value, owner)?;
    if !removed.contains(&requested) {
        return Err(WebAudioError::Format);
    }
    Ok(removed)
}

fn parse_library_source_remove(
    value: &Value,
    owner: i64,
) -> Result<Vec<(i64, i64)>, WebAudioError> {
    if owner <= 0 {
        return Err(WebAudioError::WrongAccount);
    }
    let ids = value
        .get("audio_ids")
        .and_then(Value::as_array)
        .filter(|ids| ids.len() <= 100)
        .ok_or(WebAudioError::Format)?;
    let mut seen = HashSet::new();
    let mut removed = Vec::new();
    for value in ids {
        let key = value
            .as_str()
            .and_then(parse_release_key)
            .ok_or(WebAudioError::Format)?;
        if key.0 != owner {
            return Err(WebAudioError::WrongAccount);
        }
        if seen.insert(key) {
            removed.push(key);
        }
    }
    if removed.is_empty() {
        return Err(WebAudioError::Format);
    }
    Ok(removed)
}

fn parse_library_restore(value: &Value, owned: (i64, i64)) -> Result<(i64, i64), WebAudioError> {
    if number(value) == Some(1)
        || value == &Value::Bool(true)
        || (value.get("owner_id").and_then(number) == Some(owned.0)
            && value.get("id").and_then(number) == Some(owned.1))
    {
        Ok(owned)
    } else {
        Err(WebAudioError::Format)
    }
}

fn exact_artist(value: &Value, name: &str) -> Result<ArtistRef, WebAudioError> {
    let rows = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    let folded = name.trim().to_lowercase();
    let mut matches = rows
        .iter()
        .filter_map(|value| artists_from_object(&serde_json::json!({"main_artists":[value]})).pop())
        .filter(|artist| artist.name.to_lowercase() == folded);
    let artist = matches.next().ok_or(WebAudioError::NoStream)?;
    if matches.any(|other| other.id != artist.id) {
        return Err(WebAudioError::Format);
    }
    Ok(artist)
}

fn artist_image(value: &Value) -> Option<String> {
    value
        .get("photo")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|photo| {
            let url = photo
                .get("url")
                .and_then(Value::as_str)
                .and_then(valid_cover_url)?;
            let width = photo.get("width").and_then(Value::as_u64).unwrap_or(0);
            Some((width, url))
        })
        .max_by_key(|(width, _)| *width)
        .map(|(_, url)| url)
}

fn parse_artist_page(value: &Value, reference: &ArtistRef) -> Result<ParsedArtist, WebAudioError> {
    let sections = value
        .get("catalog")
        .and_then(|v| v.get("sections"))
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    let default = value["catalog"]["default_section"].as_str();
    let section = sections
        .iter()
        .find(|s| s["id"].as_str() == default)
        .or_else(|| sections.first())
        .ok_or(WebAudioError::Format)?;
    let blocks = section
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    // The banner's explicit references identify the requested artist; related
    // artist cards and editorial playlists must not become this artist's data.
    let banner = blocks.iter().find(|block| block["data_type"] == "artist");
    let ids = banner
        .and_then(|b| b.get("artists_ids"))
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    let raw_artist = value
        .get("artists")
        .and_then(Value::as_array)
        .and_then(|artists| {
            artists.iter().find(|artist| {
                let Some(id) = artist.get("id") else {
                    return false;
                };
                let id_text = id
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| number(id).map(|id| id.to_string()));
                ids.iter().any(|candidate| candidate == id)
                    && (id_text.as_deref() == Some(reference.id.as_str())
                        || artist.get("domain").and_then(Value::as_str)
                            == Some(reference.id.as_str()))
            })
        })
        .ok_or(WebAudioError::Format)?;
    let artist = artists_from_object(&serde_json::json!({"main_artists":[raw_artist]}))
        .pop()
        .ok_or(WebAudioError::Format)?;
    let audio_block = blocks
        .iter()
        .find(|block| block["data_type"] == "music_audios");
    let order = audio_block
        .and_then(|block| block.get("audios_ids"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let audio_pool: HashMap<_, _> = value
        .get("audios")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|raw| {
            raw.get("owner_id")
                .and_then(number)
                .zip(raw.get("id").and_then(number))
                .map(|(owner, id)| (format!("{owner}_{id}"), raw))
        })
        .collect();
    let ordered: Vec<_> = order
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|id| audio_pool.get(id).copied().cloned())
        .collect();
    if ordered.len() != order.len() {
        return Err(WebAudioError::Format);
    }
    let (mut tracks, streams) = home_tracks(&ordered)?;
    for track in &mut tracks {
        track.url = None;
    }
    let release_block = blocks.iter().find(|block| {
        block["data_type"] == "music_playlists" && block["layout"]["style"] == "artist_releases"
    });
    let order = release_block
        .and_then(|block| block.get("playlists_ids"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let album_pool: HashMap<_, _> = value
        .get("playlists")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(crate::artist_releases::Release::from_value)
        .map(|release| {
            (
                format!("{}_{}", release.album.owner_id, release.album.id),
                release,
            )
        })
        .collect();
    let latest_releases: Vec<_> = order
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|id| album_pool.get(id).cloned())
        .collect();
    Ok((
        ArtistPage {
            artist,
            image_url: artist_image(raw_artist),
            tracks,
            discography: latest_releases.clone(),
            latest_releases,
            releases_error: None,
        },
        streams,
    ))
}

struct UserPlaylistPage {
    items: Vec<UserPlaylist>,
    received: usize,
    total: Option<usize>,
}

impl UserPlaylistPage {
    fn next_offset(&self, offset: usize) -> Option<usize> {
        // VK's count includes playlist slots that are no longer visible to
        // this account. Offset addresses requested slots, not surviving rows.
        let next = offset + 100;
        if self.received == 0
            || self.total.is_some_and(|total| next >= total)
            || (self.total.is_none() && self.received < 100)
        {
            None
        } else {
            Some(next)
        }
    }
}

fn parse_user_playlists(value: &Value) -> Result<UserPlaylistPage, WebAudioError> {
    let rows = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    if rows.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    let total = match value.get("count") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|count| usize::try_from(count).ok())
                .ok_or(WebAudioError::Format)?,
        ),
    };
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for row in rows {
        let mut album = AlbumRef::from_value(row).ok_or(WebAudioError::Format)?;
        album.title = home_text(&album.title, 4096);
        if album.title.is_empty() {
            return Err(WebAudioError::Format);
        }
        if !seen.insert((album.owner_id, album.id)) {
            continue;
        }
        let subtitle = ["subtitle", "subTitle", "description"]
            .into_iter()
            .filter_map(|key| row.get(key).and_then(Value::as_str))
            .map(|text| home_text(text, 512))
            .find(|text| !text.is_empty())
            .unwrap_or_default();
        let track_count = row
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok());
        items.push(UserPlaylist {
            original: playlist_follow::original_reference(row, &album)?,
            album,
            subtitle,
            track_count,
            can_add_tracks: playlist_can_add_tracks(row),
        });
    }
    Ok(UserPlaylistPage {
        items,
        received: rows.len(),
        total,
    })
}

fn playlist_can_add_tracks(value: &Value) -> bool {
    let enabled = |value: Option<&Value>| {
        value.is_some_and(|value| value == &Value::Bool(true) || value.as_u64() == Some(1))
    };
    value.get("id").and_then(number).is_some_and(|id| id > 0)
        && value
            .get("owner_id")
            .and_then(number)
            .is_some_and(|owner| owner != 0 && owner.checked_abs().is_some())
        && enabled(
            value
                .get("permissions")
                .and_then(|permissions| permissions.get("edit")),
        )
        && !enabled(value.get("is_following"))
        && !enabled(value.get("is_album"))
        && !matches!(
            value.get("type").and_then(Value::as_str),
            Some("album" | "single")
        )
        && value
            .get("type")
            .and_then(Value::as_u64)
            .is_none_or(|kind| kind == 0)
}

fn parse_playlist_add(
    value: &Value,
    owner: i64,
) -> Result<((i64, i64), Option<String>), WebAudioError> {
    let item = value
        .as_array()
        .filter(|rows| rows.len() == 1)
        .and_then(|rows| rows.first())
        .ok_or(WebAudioError::Format)?;
    let id = item
        .get("audio_id")
        .and_then(number)
        .filter(|id| *id > 0)
        .ok_or(WebAudioError::Format)?;
    if item
        .get("owner_id")
        .is_some_and(|returned| number(returned) != Some(owner))
    {
        return Err(WebAudioError::Format);
    }
    let track_code = item
        .get("track_code")
        .and_then(Value::as_str)
        .filter(|code| {
            !code.is_empty() && code.len() <= 16_384 && !code.chars().any(char::is_control)
        })
        .map(str::to_owned);
    Ok(((owner, id), track_code))
}

fn parse_album_page(value: &Value, reference: &AlbumRef) -> Result<AlbumPage, WebAudioError> {
    let source = match value.get("playlist") {
        Some(source) if source.is_object() => source,
        Some(_) => return Err(WebAudioError::Format),
        None => value,
    };
    let mut album = AlbumRef::from_value(source).ok_or(WebAudioError::Format)?;
    if album.id != reference.id || album.owner_id != reference.owner_id {
        return Err(WebAudioError::Format);
    }
    album.access_key = album.access_key.or_else(|| reference.access_key.clone());
    album.cover_url = album.cover_url.or_else(|| reference.cover_url.clone());
    Ok(AlbumPage {
        album,
        artists: artists_from_object(source),
        year: source
            .get("year")
            .and_then(Value::as_u64)
            .and_then(|year| u32::try_from(year).ok())
            .filter(|year| *year > 0),
        description: playlist_description(source.get("description")),
        creator: playlist_creator_id(source)
            .and_then(|owner_id| playlist_creator_from_lookup(value, owner_id)),
        tracks: Vec::new(),
        total_tracks: source
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok()),
        stats: album_stats(source),
    })
}

fn playlist_creator_id(playlist: &Value) -> Option<i64> {
    let valid = |owner_id: i64| owner_id != 0 && owner_id.checked_abs().is_some();
    playlist
        .get("original")
        .and_then(|original| original.get("owner_id"))
        .and_then(number)
        .filter(|owner_id| valid(*owner_id))
        .or_else(|| {
            playlist
                .get("owner_id")
                .and_then(number)
                .filter(|owner_id| valid(*owner_id))
        })
}

fn playlist_creator_from_lookup(value: &Value, owner_id: i64) -> Option<PlaylistCreator> {
    let creator = |record: &Value| {
        let id = record.get("id").and_then(number)?;
        if Some(id) != owner_id.checked_abs() {
            return None;
        }
        let name = if owner_id > 0 {
            ["first_name", "last_name"]
                .into_iter()
                .filter_map(|key| record.get(key).and_then(Value::as_str))
                .map(|name| home_text(name, 256))
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            home_text(record.get("name").and_then(Value::as_str)?, 512)
        };
        let name = name.chars().take(512).collect::<String>();
        (!name.is_empty()).then_some(PlaylistCreator { owner_id, name })
    };
    let side_tables: &[&str] = if owner_id > 0 {
        &["profiles", "users"]
    } else {
        &["groups"]
    };
    for key in side_tables {
        if let Some(found) = value
            .get(key)
            .and_then(Value::as_array)
            .and_then(|rows| rows.iter().find_map(creator))
        {
            return Some(found);
        }
    }
    if let Some(found) = value
        .as_array()
        .and_then(|rows| rows.iter().find_map(creator))
    {
        return Some(found);
    }
    value
        .get("owner")
        .or_else(|| {
            value
                .get("playlist")
                .and_then(|playlist| playlist.get("owner"))
        })
        .and_then(|owner| {
            owner
                .get(if owner_id > 0 { "user" } else { "group" })
                .or(Some(owner))
        })
        .and_then(creator)
}

/// Extract display text only. Native playlist descriptions can contain HTML;
/// keep their paragraphs without making links or page code executable in egui.
fn playlist_description(value: Option<&Value>) -> Option<String> {
    const INPUT_LIMIT: usize = 16_384;
    const OUTPUT_LIMIT: usize = 4096;
    let source = value?.as_str()?;
    let mut plain = String::new();
    let mut tag = String::new();
    let mut inside_tag = false;
    let mut quoted = None;
    let mut hidden = None;
    let mut characters = source.chars().take(INPUT_LIMIT).peekable();
    while let Some(character) = characters.next() {
        if inside_tag {
            if tag.starts_with("!--") {
                if character == '>' && tag.ends_with("--") {
                    tag.clear();
                    inside_tag = false;
                } else {
                    tag.push(character);
                }
                continue;
            }
            match character {
                '"' | '\'' if quoted == Some(character) => quoted = None,
                '"' | '\'' if quoted.is_none() => quoted = Some(character),
                '>' if quoted.is_none() && (!tag.starts_with("!--") || tag.ends_with("--")) => {
                    let name = tag.trim_start();
                    let closing = name.starts_with('/');
                    let name: String = name
                        .trim_start_matches('/')
                        .trim_start()
                        .chars()
                        .take_while(char::is_ascii_alphanumeric)
                        .collect::<String>()
                        .to_ascii_lowercase();
                    if matches!(name.as_str(), "script" | "style") {
                        if closing && hidden.as_deref() == Some(name.as_str()) {
                            hidden = None;
                        } else if !closing && hidden.is_none() {
                            hidden = Some(name.clone());
                        }
                    } else if hidden.is_none()
                        && matches!(
                            name.as_str(),
                            "br" | "p"
                                | "div"
                                | "li"
                                | "ul"
                                | "ol"
                                | "section"
                                | "blockquote"
                                | "pre"
                        )
                    {
                        let previous = plain
                            .chars()
                            .rev()
                            .find(|character| !matches!(character, ' ' | '\t'));
                        if name == "br" || previous.is_some_and(|character| character != '\n') {
                            plain.push('\n');
                        }
                    }
                    tag.clear();
                    inside_tag = false;
                    continue;
                }
                _ => (),
            }
            tag.push(character);
        } else if character == '<'
            && characters
                .peek()
                .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!'))
        {
            inside_tag = true;
            quoted = None;
            tag.clear();
        } else if hidden.is_none()
            && (!character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        {
            plain.push(character);
        }
    }
    let decoded = html_escape::decode_html_entities(&plain);
    let normalized = decoded.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = normalized
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let description: String = lines.join("\n").trim().chars().take(OUTPUT_LIMIT).collect();
    (!description.is_empty()).then_some(description)
}

fn album_stats(value: &Value) -> AlbumStats {
    let timestamp = |key| {
        value
            .get(key)
            .and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            })
            .filter(|value| (1..=253_402_300_799).contains(value))
    };
    // The first-party playlist entity maps exactly plays/create_time/update_time
    // to playsCount/createdAt/updatedAt (683ba166.9b4ba9aa.js).
    AlbumStats {
        plays: value.get("plays").and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        }),
        created_at: timestamp("create_time"),
        updated_at: timestamp("update_time"),
    }
}

fn parse_mix_page(text: &str) -> Result<HomeTracks, WebAudioError> {
    let value: Value = serde_json::from_str(text).map_err(|_| WebAudioError::Format)?;
    if let Some(error) = value.get("error") {
        return Err(match error.get("error_code").and_then(number) {
            Some(5 | 27 | 28) => WebAudioError::Expired,
            _ => WebAudioError::Rejected,
        });
    }
    let audios = value
        .get("response")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    home_tracks(audios)
}

fn mix_api_host(object: &str) -> Result<&'static str, WebAudioError> {
    let domains = js_object_field(object, "apiConfigDomains")?.ok_or(WebAudioError::Format)?;
    let host = js_object_field(domains, "apiDomain")?
        .and_then(viewer_string)
        .ok_or(WebAudioError::Format)?;
    match host.as_str() {
        "web.api.vk.ru" => Ok("web.api.vk.ru"),
        "api.vk.ru" => Ok("api.vk.ru"),
        "web.api.vk.com" => Ok("web.api.vk.com"),
        "api.vk.com" => Ok("api.vk.com"),
        _ => Err(WebAudioError::Format),
    }
}

fn mix_token_valid(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 16_384
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
}

fn mix_page_token(
    object: &str,
    owner: i64,
    require_owner: bool,
) -> Result<MixSession, WebAudioError> {
    let anonymous = js_object_field(object, "is_anonym")?;
    if anonymous.is_some_and(|value| value.trim() != "false")
        || !require_owner && anonymous != Some("false")
    {
        return Err(WebAudioError::Expired);
    }
    let returned_owner = js_object_field(object, "user_id")?
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .as_ref()
        .and_then(number);
    if returned_owner.is_some_and(|id| id != 0 && id != owner) {
        return Err(WebAudioError::WrongAccount);
    }
    if require_owner && returned_owner != Some(owner) {
        return Err(WebAudioError::Unverified);
    }
    let token = Zeroizing::new(
        js_object_field(object, "access_token")?
            .and_then(viewer_string)
            .ok_or(WebAudioError::Expired)?,
    );
    if !mix_token_valid(&token) {
        return Err(WebAudioError::Expired);
    }
    let expires = js_object_field(object, "expired_at")?
        .or(js_object_field(object, "expires")?)
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .as_ref()
        .and_then(number)
        .unwrap_or_else(|| now() + 3600);
    if expires <= now() + 60 {
        return Err(WebAudioError::Expired);
    }
    Ok(MixSession {
        token,
        expires,
        api_host: "web.api.vk.ru",
        previous_batch: Vec::new(),
        previous_source: MixSource::Personal,
    })
}

fn mix_login_token(text: &str, owner: i64) -> Result<MixSession, WebAudioError> {
    let value: Value = serde_json::from_str(text).map_err(|_| WebAudioError::Format)?;
    if value.get("error").is_some()
        || value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "okay")
    {
        return Err(WebAudioError::Expired);
    }
    let value = value.get("data").unwrap_or(&value);
    let token = if let Some(profiles) = value.as_array() {
        profiles
            .iter()
            .find(|profile| profile.get("user_id").and_then(number) == Some(owner))
            .ok_or_else(|| {
                if profiles.iter().any(|profile| {
                    profile
                        .get("user_id")
                        .and_then(number)
                        .is_some_and(|id| id > 0)
                }) {
                    WebAudioError::WrongAccount
                } else {
                    WebAudioError::Unverified
                }
            })?
    } else {
        value
    };
    let object = Zeroizing::new(serde_json::to_string(token).map_err(|_| WebAudioError::Format)?);
    mix_page_token(&object, owner, true)
}

/// Read a single top-level JavaScript object field without executing page code.
/// Values remain borrowed slices of the bounded, zeroizing response buffer.
fn js_object_field<'a>(object: &'a str, wanted: &str) -> Result<Option<&'a str>, WebAudioError> {
    let bytes = object.as_bytes();
    if bytes.first() != Some(&b'{') {
        return Err(WebAudioError::Format);
    }
    let mut i = 1;
    let mut found = None;
    loop {
        js_space(bytes, &mut i).ok_or(WebAudioError::Format)?;
        if bytes.get(i) == Some(&b'}') {
            return Ok(found);
        }
        let start = i;
        let key = if matches!(bytes.get(i), Some(b'\'' | b'"')) {
            let end = js_string_end(bytes, i).ok_or(WebAudioError::Format)?;
            let key = object.get(i + 1..end - 1).ok_or(WebAudioError::Format)?;
            i = end;
            key
        } else {
            while bytes
                .get(i)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"_$".contains(byte))
            {
                i += 1;
            }
            if start == i {
                return Err(WebAudioError::Format);
            }
            object.get(start..i).ok_or(WebAudioError::Format)?
        };
        js_space(bytes, &mut i).ok_or(WebAudioError::Format)?;
        if bytes.get(i) != Some(&b':') {
            return Err(WebAudioError::Format);
        }
        i += 1;
        js_space(bytes, &mut i).ok_or(WebAudioError::Format)?;
        let start = i;
        i = js_value_end(bytes, i).ok_or(WebAudioError::Format)?;
        if key == wanted {
            if found.is_some() {
                return Err(WebAudioError::Format);
            }
            found = Some(object.get(start..i).ok_or(WebAudioError::Format)?.trim());
        }
        js_space(bytes, &mut i).ok_or(WebAudioError::Format)?;
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return Ok(found),
            _ => return Err(WebAudioError::Format),
        }
    }
}

fn parse_home_playlist(
    text: &str,
    owner_id: i64,
    playlist_id: i64,
) -> Result<HomeTracks, WebAudioError> {
    let parts = music_payload(text)?;
    let playlist = parts
        .first()
        .filter(|value| value.is_object())
        .ok_or(WebAudioError::NoStream)?;
    if playlist
        .get("ownerId")
        .or_else(|| playlist.get("owner_id"))
        .and_then(number)
        .is_some_and(|owner| owner != owner_id)
        || playlist
            .get("id")
            .or_else(|| playlist.get("playlist_id"))
            .and_then(number)
            .is_some_and(|id| id != playlist_id)
    {
        return Err(WebAudioError::Format);
    }
    let list = playlist
        .get("list")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::NoStream)?;
    home_tracks(list)
}

fn music_payload(text: &str) -> Result<Vec<Value>, WebAudioError> {
    let text = text.trim_start();
    let text = text.strip_prefix("<!--").unwrap_or(text);
    let envelope: Value = serde_json::from_str(text).map_err(|_| WebAudioError::Format)?;
    let payload = envelope
        .get("payload")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    match payload.first().and_then(number) {
        Some(0) => (),
        Some(3 | 4) => return Err(WebAudioError::Expired),
        Some(_) => return Err(WebAudioError::Rejected),
        None => return Err(WebAudioError::Format),
    }
    payload
        .get(1)
        .and_then(Value::as_array)
        .cloned()
        .ok_or(WebAudioError::Format)
}

fn home_text(value: &str, limit: usize) -> String {
    let mut plain = String::new();
    let mut inside = false;
    for character in value.chars().take(16_384) {
        match character {
            '<' => {
                inside = true;
                plain.push(' ');
            }
            '>' => inside = false,
            _ if !inside => plain.push(character),
            _ => (),
        }
    }
    html_escape::decode_html_entities(&plain)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn home_access(value: Option<&Value>) -> Option<Zeroizing<String>> {
    value
        .and_then(Value::as_str)
        .filter(|key| {
            !key.is_empty()
                && key.len() <= 1024
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
        .map(|key| Zeroizing::new(key.to_owned()))
}

fn home_tracks(list: &[Value]) -> Result<HomeTracks, WebAudioError> {
    if list.len() > MAX_QUEUED_TRACKS {
        return Err(WebAudioError::TooLarge);
    }
    let mut seen = HashSet::new();
    let mut tracks = Vec::new();
    let mut streams = HashMap::new();
    for raw in list {
        let track = parse_track(raw)?;
        streams.insert(track.key(), stream_info(raw, track.key()));
        if seen.insert(track.key()) {
            tracks.push(track);
        }
    }
    Ok((tracks, streams))
}

fn home_collection(value: &Value) -> Option<HomeCollection> {
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| {
            matches!(kind, "radio" | "vkmix" | "mix")
                || kind.starts_with("vkmix:")
                || kind.starts_with("radio:")
        })
    {
        return None;
    }
    if value.get("duration").is_some() {
        return None;
    }
    let owner_id = value
        .get("ownerId")
        .or_else(|| value.get("owner_id"))
        .and_then(number)
        .filter(|owner| *owner != 0)?;
    let playlist_id = value
        .get("playlist_id")
        .or_else(|| value.get("id"))
        .and_then(number)
        .filter(|id| *id != 0)?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(|title| home_text(title, 256))
        .filter(|title| !title.is_empty())?;
    let subtitle = [
        "subtitle",
        "subTitle",
        "description",
        "authorLine",
        "authorName",
    ]
    .iter()
    .filter_map(|key| value.get(key).and_then(Value::as_str))
    .map(|text| home_text(text, 512))
    .find(|text| !text.is_empty())
    .unwrap_or_default();
    let access_key = home_access(
        value
            .get("accessHash")
            .or_else(|| value.get("access_hash"))
            .or_else(|| value.get("access_key")),
    );
    let tracks = value
        .get("list")
        .and_then(Value::as_array)
        .filter(|list| {
            let total = value
                .get("totalCount")
                .or_else(|| value.get("size"))
                .or_else(|| value.get("count"))
                .and_then(Value::as_u64);
            total.is_some_and(|total| total <= list.len() as u64)
        })
        .and_then(|list| home_tracks(list).ok().map(|(tracks, _)| tracks))
        .unwrap_or_default();
    Some(HomeCollection {
        id: format!("{owner_id}_{playlist_id}"),
        owner_id,
        playlist_id,
        title,
        subtitle,
        cover_url: cover_from_object(value),
        access_key,
        tracks,
    })
}

fn parse_home(text: &str) -> Result<ParsedHome, WebAudioError> {
    let parts = music_payload(text)?;
    let html = parts.first().and_then(Value::as_str).unwrap_or_default();
    let data = parts.get(1).filter(|value| value.is_object());
    let mut collections = Vec::new();
    let mut streams = HashMap::new();
    let mut seen = HashSet::new();
    if let Some(values) = data
        .and_then(|data| data.get("playlists"))
        .and_then(Value::as_array)
    {
        for value in values.iter().take(64) {
            if let Some(collection) = home_collection(value)
                && seen.insert(collection.id.clone())
            {
                if !collection.tracks.is_empty()
                    && let Some(list) = value.get("list").and_then(Value::as_array)
                {
                    streams.extend(home_tracks(list)?.1);
                }
                collections.push(collection);
            }
        }
    }
    // Legacy Home often renders playlist cards as HTML while its JSON contains
    // only section cursors. Parse only real known playlist containers/actions.
    for collection in home_html_collections(html) {
        if let Some(existing) = collections
            .iter_mut()
            .find(|existing| existing.id == collection.id)
        {
            if existing.cover_url.is_none() {
                existing.cover_url = collection.cover_url;
            }
            if existing.access_key.is_none() {
                existing.access_key = collection.access_key;
            }
            if existing.subtitle.is_empty() {
                existing.subtitle = collection.subtitle;
            }
        } else if seen.insert(collection.id.clone()) && collections.len() < 64 {
            collections.push(collection);
        }
    }
    if collections.is_empty()
        && !data.is_some_and(|data| data.get("playlists").is_some_and(Value::is_array))
    {
        return Err(WebAudioError::Format);
    }
    Ok(ParsedHome {
        collections,
        streams,
    })
}

fn home_html_collections(html: &str) -> Vec<HomeCollection> {
    let mut collections = Vec::new();
    for element in home_elements(
        html,
        &["_audio_pl_item", "RecomsPlaylist", "RecommendedPlaylist"],
    ) {
        let Some((owner_id, playlist_id, access_key)) = home_html_action(element) else {
            continue;
        };
        let title = home_elements(
            element,
            &[
                "audio_item__title",
                "audio_pl__title",
                "RecomsPlaylist__title",
                "RecommendedPlaylist__title",
            ],
        )
        .first()
        .map_or_else(String::new, |html| home_text(html, 256));
        if title.is_empty() {
            continue;
        }
        let subtitle = home_elements(
            element,
            &[
                "audio_pl__subtitle",
                "audio_pl__year_subtitle",
                "RecomsPlaylist__subtitle",
            ],
        )
        .first()
        .map_or_else(String::new, |html| home_text(html, 512));
        collections.push(HomeCollection {
            id: format!("{owner_id}_{playlist_id}"),
            owner_id,
            playlist_id,
            title,
            subtitle,
            cover_url: home_html_cover(element),
            access_key,
            tracks: Vec::new(),
        });
    }
    collections
}

fn home_html_action(html: &str) -> Option<(i64, i64, Option<Zeroizing<String>>)> {
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        let (name, attributes, body) = html_tag(&rest[at..])?;
        rest = body;
        if matches!(name, "script" | "style") {
            rest = body
                .split_once(&format!("</{name}>"))
                .map_or("", |(_, tail)| tail);
            continue;
        }
        let fields = html_attributes(attributes);
        let Some(action) = fields.get("onclick") else {
            continue;
        };
        let Some(at) = action.find("showAudioPlaylist(") else {
            continue;
        };
        let args = action[at + "showAudioPlaylist(".len()..].split_once(')')?.0;
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let owner_id = args.first()?.parse::<i64>().ok().filter(|id| *id != 0)?;
        let playlist_id = args.get(1)?.parse::<i64>().ok().filter(|id| *id != 0)?;
        let key = args
            .get(2)
            .map(|value| Value::String(value.trim_matches(['\'', '"']).to_owned()));
        return Some((owner_id, playlist_id, home_access(key.as_ref())));
    }
    None
}

fn home_html_cover(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        let (name, attributes, tail) = html_tag(&rest[at..])?;
        rest = tail;
        let fields = html_attributes(attributes);
        if name == "img"
            && let Some(url) = fields.get("src").and_then(|url| valid_cover_url(url))
        {
            return Some(url);
        }
        if let Some(style) = fields.get("style")
            && let Some((_, value)) = style.split_once("url(")
            && let Some((value, _)) = value.split_once(')')
            && let Some(url) = valid_cover_url(value.trim().trim_matches(['\'', '"']))
        {
            return Some(url);
        }
    }
    None
}

/// Small bounded HTML element extractor; no scripts or event handlers execute.
fn home_elements<'a>(html: &'a str, classes: &[&str]) -> Vec<&'a str> {
    let mut elements = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        let fragment = &rest[at..];
        if let Some(comment) = fragment.strip_prefix("<!--") {
            rest = comment.split_once("-->").map_or("", |(_, tail)| tail);
            continue;
        }
        let Some((name, attributes, body)) = html_tag(fragment) else {
            break;
        };
        rest = body;
        if matches!(name, "script" | "style") {
            rest = body
                .split_once(&format!("</{name}>"))
                .map_or("", |(_, tail)| tail);
            continue;
        }
        let fields = html_attributes(attributes);
        if !fields.get("class").is_some_and(|value| {
            value
                .split_whitespace()
                .any(|class| classes.contains(&class))
        }) {
            continue;
        }
        let mut tail = body;
        let mut depth = 1;
        while let Some(next) = tail.find('<') {
            let fragment = &tail[next..];
            if let Some(close) = fragment.strip_prefix("</")
                && close
                    .split(|character: char| character.is_ascii_whitespace() || character == '>')
                    .next()
                    == Some(name)
            {
                depth -= 1;
                if depth == 0 {
                    let length = html.len() - body.len() + (body.len() - tail.len()) + next;
                    let start = html.len() - body.len();
                    elements.push(&html[start..length]);
                    rest = close.split_once('>').map_or("", |(_, tail)| tail);
                    break;
                }
            }
            let Some((nested, _, after)) = html_tag(fragment) else {
                break;
            };
            if nested == name {
                depth += 1;
                if depth > 128 {
                    break;
                }
            }
            tail = after;
        }
        if elements.len() >= 64 {
            break;
        }
    }
    elements
}

struct ParsedPage {
    page: AudioPage,
    section: Option<String>,
    next: Option<String>,
    streams: HashMap<(i64, i64), StreamInfo>,
}

fn number(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn opaque(value: Option<&Value>) -> Result<Option<String>, WebAudioError> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) if text.len() <= 8192 => Ok(Some(text.clone())),
        Some(Value::Number(n)) if n.as_u64() == Some(0) => Ok(None),
        Some(Value::Number(n)) if n.as_u64().is_some() => Ok(Some(n.to_string())),
        _ => Err(WebAudioError::Format),
    }
}

fn parse_page(text: &str) -> Result<ParsedPage, WebAudioError> {
    let text = text
        .trim_start()
        .strip_prefix("<!--")
        .unwrap_or(text.trim_start());
    let envelope: Value = serde_json::from_str(text).map_err(|_| WebAudioError::Format)?;
    let payload = envelope
        .get("payload")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    match payload.first().and_then(number) {
        Some(0) => (),
        Some(3 | 4) => return Err(WebAudioError::Expired),
        Some(_) => return Err(WebAudioError::Rejected),
        None => return Err(WebAudioError::Format),
    }
    let data = payload
        .get(1)
        .and_then(Value::as_array)
        .and_then(|parts| parts.get(1))
        .filter(|v| v.is_object())
        .ok_or(WebAudioError::Format)?;
    let playlist = data
        .get("playlist")
        .filter(|v| v.is_object())
        .ok_or(WebAudioError::Format)?;
    let list = playlist
        .get("list")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    let items: Vec<Track> = list.iter().map(parse_track).collect::<Result<_, _>>()?;
    let streams = list
        .iter()
        .zip(&items)
        .map(|(raw, track)| (track.key(), stream_info(raw, track.key())))
        .collect();
    let section = opaque(data.get("section_id").or_else(|| data.get("sectionId")))?;
    let next = opaque(
        data.get("next_from")
            .or_else(|| data.get("nextFrom"))
            .or_else(|| data.get("nextOffset")),
    )?;
    let count = playlist
        .get("totalCount")
        .or_else(|| playlist.get("total_count"))
        .or_else(|| playlist.get("count"))
        .and_then(number)
        .and_then(|n| usize::try_from(n).ok());
    Ok(ParsedPage {
        page: AudioPage {
            count,
            items,
            exhausted: Some(next.is_none()),
        },
        section,
        next,
        streams,
    })
}

fn stream_info(raw: &Value, key: (i64, i64)) -> StreamInfo {
    let (url, action_hash, url_hash) = if let Some(row) = raw.as_array() {
        let hashes: Vec<_> = row
            .get(13)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('/')
            .collect();
        (
            row.get(2).and_then(Value::as_str),
            hashes.get(2).copied(),
            hashes.get(5).copied(),
        )
    } else {
        (
            raw.get("url").and_then(Value::as_str),
            raw.get("actionHash").and_then(Value::as_str),
            raw.get("urlHash").and_then(Value::as_str),
        )
    };
    fn hash(value: Option<&str>) -> Option<&str> {
        value.filter(|s| {
            !s.is_empty()
                && s.len() <= 512
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
    }
    let reload_id = hash(action_hash)
        .zip(hash(url_hash))
        .map(|(a, u)| Zeroizing::new(format!("{}_{}_{}_{}", key.0, key.1, a, u)));
    let original_url = url
        .filter(|url| !url.is_empty() && url.len() <= 32_768)
        .map(|url| Zeroizing::new(url.to_owned()));
    StreamInfo {
        reload_id,
        original_url,
        // These are request credentials/context, kept only in this verified
        // client's RAM. They are never serialized into Track or metadata caches.
        access_key: home_access(if let Some(row) = raw.as_array() {
            row.get(24)
        } else {
            raw.get("access_key").or_else(|| raw.get("accessKey"))
        })
        .or_else(|| hash(url_hash).map(|key| Zeroizing::new(key.to_owned()))),
        track_code: (if let Some(row) = raw.as_array() {
            row.get(20)
        } else {
            raw.get("track_code").or_else(|| raw.get("trackCode"))
        })
        .and_then(Value::as_str)
        .filter(|code| {
            !code.is_empty() && code.len() <= 8192 && !code.chars().any(char::is_control)
        })
        .map(str::to_owned),
    }
}

fn parse_reload_row(text: &str, key: (i64, i64)) -> Result<Value, WebAudioError> {
    let text = text
        .trim_start()
        .strip_prefix("<!--")
        .unwrap_or(text.trim_start());
    let envelope: Value = serde_json::from_str(text).map_err(|_| WebAudioError::Format)?;
    let payload = envelope
        .get("payload")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    match payload.first().and_then(number) {
        Some(0) => (),
        Some(3 | 4) => return Err(WebAudioError::Expired),
        Some(_) => return Err(WebAudioError::Rejected),
        None => return Err(WebAudioError::Format),
    }
    let rows = payload
        .get(1)
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    let row_key = |row: &Value| {
        if let Some(row) = row.as_array() {
            row.get(1)
                .and_then(number)
                .zip(row.first().and_then(number))
        } else {
            row.get("owner_id")
                .and_then(number)
                .zip(row.get("id").and_then(number))
        }
    };
    let mut matches = rows.iter().filter(|row| row_key(row) == Some(key));
    let row = matches.next().ok_or(WebAudioError::NoStream)?;
    if matches.next().is_some() {
        return Err(WebAudioError::Format);
    }
    Ok(row.clone())
}

fn parse_reload(text: &str, key: (i64, i64), viewer: i64) -> Result<String, WebAudioError> {
    let row = parse_reload_row(text, key)?;
    let url = if let Some(row) = row.as_array() {
        row.get(2)
    } else {
        row.get("url")
    };
    decode_stream(
        url.and_then(Value::as_str).ok_or(WebAudioError::NoStream)?,
        viewer,
    )
}

const VK_ALPHABET: &[u8; 65] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN0PQRSTUVWXYZO123456789+/=";

/// Decode VK's masked stream URL with the account-specific web transformations.
/// The transformations run in Rust; no website JavaScript is evaluated.
fn decode_stream(url: &str, viewer: i64) -> Result<String, WebAudioError> {
    let result = decode_stream_inner(url, viewer);
    if result.is_err() {
        crate::diagnostics::event(
            "stream",
            if url.contains("audio_api_unavailable") {
                "decode_failed kind=masked"
            } else {
                "decode_failed kind=direct"
            },
        );
    }
    result
}

fn decode_stream_inner(url: &str, viewer: i64) -> Result<String, WebAudioError> {
    if url.is_empty() || url.len() > 32_768 {
        return Err(WebAudioError::NoStream);
    }
    let decoded = if url.contains("audio_api_unavailable") {
        let extra = url.split_once("?extra=").ok_or(WebAudioError::NoStream)?.1;
        let (data, operations) = extra.split_once('#').ok_or(WebAudioError::NoStream)?;
        let mut data = decode_vk_base64(data)?;
        let operations = decode_vk_base64(operations)?;
        let commands: Vec<_> = operations
            .split(|b| *b == b'\t')
            .filter(|cmd| !cmd.is_empty())
            .collect();
        if commands.len() > 32 {
            return Err(WebAudioError::NoStream);
        }
        for command in commands.into_iter().rev() {
            let mut parts = command.split(|b| *b == 11);
            let op = parts.next().ok_or(WebAudioError::NoStream)?;
            let arg = parts.next();
            if parts.next().is_some() {
                return Err(WebAudioError::NoStream);
            }
            let integer = || -> Result<i64, WebAudioError> {
                std::str::from_utf8(arg.ok_or(WebAudioError::NoStream)?)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or(WebAudioError::NoStream)
            };
            match op {
                b"v" => data.reverse(),
                b"x" => {
                    let key = *arg.and_then(|a| a.first()).ok_or(WebAudioError::NoStream)?;
                    for byte in &mut data {
                        *byte ^= key;
                    }
                }
                b"r" => {
                    let shift = integer()?;
                    for byte in &mut data {
                        if let Some(index) = VK_ALPHABET.iter().position(|b| b == byte) {
                            *byte = VK_ALPHABET[(index as i64 - shift).rem_euclid(65) as usize];
                        }
                    }
                }
                b"s" => shuffle(&mut data, integer()?)?,
                b"i" => {
                    crate::diagnostics::event(
                        "stream",
                        if viewer > i64::from(i32::MAX) {
                            "decode_operation=i bigint=true viewer_large=true"
                        } else {
                            "decode_operation=i bigint=true viewer_large=false"
                        },
                    );
                    // Current first-party audioUnmaskSource converts both
                    // operands to BigInt, unlike the separate numeric `s`
                    // operation. Truncating a newer VK account ID to i32
                    // corrupts every masked URL using this operation.
                    // parseInt and vk.id are JS Numbers before BigInt, so keep
                    // their IEEE integer rounding for values above 2^53 too.
                    let argument = (integer()? as f64) as i128;
                    let viewer = (viewer as f64) as i128;
                    shuffle_bigint(&mut data, argument ^ viewer);
                }
                _ => return Err(WebAudioError::NoStream),
            }
        }
        String::from_utf8(data).map_err(|_| WebAudioError::NoStream)?
    } else {
        url.to_owned()
    };
    validate_stream(&decoded)
}

fn decode_vk_base64(input: &str) -> Result<Vec<u8>, WebAudioError> {
    if input.len() > 32_768 || input.len() % 4 == 1 {
        return Err(WebAudioError::NoStream);
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut count = 0;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = VK_ALPHABET[..64]
            .iter()
            .position(|b| *b == byte)
            .ok_or(WebAudioError::NoStream)? as u32;
        buffer = (buffer << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            output.push(((buffer >> count) & 255) as u8);
        }
        buffer &= (1 << count) - 1;
    }
    Ok(output)
}

fn shuffle(data: &mut [u8], seed: i64) -> Result<(), WebAudioError> {
    let len = data.len();
    if len == 0 {
        return Ok(());
    }
    let mut seed = seed.checked_abs().ok_or(WebAudioError::NoStream)?;
    let mut indices = vec![0; len];
    for position in (0..len).rev() {
        // JS bitwise XOR first converts both operands to signed 32-bit values.
        let lhs = (len * (position + 1)) as i32;
        let rhs = seed.wrapping_add(position as i64) as i32;
        seed = i64::from(lhs ^ rhs) % len as i64;
        indices[position] = if seed < 0 {
            (len as i64 + seed) as usize
        } else {
            seed as usize
        };
    }
    for position in 1..len {
        data.swap(position, indices[len - 1 - position]);
    }
    Ok(())
}

/// The native BigInt branch of `s` uses arbitrary-width XOR/addition until the
/// first modulo, after which the seed is smaller than the bounded URL length.
/// The inputs are bounded 64-bit numbers; u128 covers every intermediate here.
fn shuffle_bigint(data: &mut [u8], seed: i128) {
    let len = data.len();
    if len == 0 {
        return;
    }
    let mut seed = seed.unsigned_abs();
    let mut indices = vec![0; len];
    for position in (0..len).rev() {
        seed = ((len as u128 * (position + 1) as u128) ^ (seed + position as u128)) % len as u128;
        indices[position] = seed as usize;
    }
    for position in 1..len {
        data.swap(position, indices[len - 1 - position]);
    }
}

fn validate_stream(value: &str) -> Result<String, WebAudioError> {
    if value
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(WebAudioError::NoStream);
    }
    let url = reqwest::Url::parse(value).map_err(|_| WebAudioError::NoStream)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
        || url.host_str().is_none_or(|host| {
            host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || !host.contains('.')
                || host.parse::<std::net::IpAddr>().is_ok()
        })
        || value.contains("audio_api_unavailable")
    {
        return Err(WebAudioError::NoStream);
    }
    Ok(url.into())
}

fn parse_track(value: &Value) -> Result<Track, WebAudioError> {
    let (id, owner, title, artist, duration, url) = if let Some(a) = value.as_array() {
        (a.first(), a.get(1), a.get(3), a.get(4), a.get(5), a.get(2))
    } else if value.is_object() {
        (
            value.get("id"),
            value.get("owner_id"),
            value.get("title"),
            value.get("artist").or_else(|| value.get("performer")),
            value.get("duration"),
            value.get("url"),
        )
    } else {
        return Err(WebAudioError::Format);
    };
    let id = id
        .and_then(number)
        .filter(|id| *id > 0)
        .ok_or(WebAudioError::Format)?;
    let owner_id = owner
        .and_then(number)
        .filter(|id| *id != 0)
        .ok_or(WebAudioError::Format)?;
    let duration = duration
        .and_then(number)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(WebAudioError::Format)?;
    let text = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .filter(|s| s.len() <= 16_384)
            .map(|s| html_escape::decode_html_entities(s).into_owned())
            .ok_or(WebAudioError::Format)
    };
    // The encoded placeholder requires a separate resolution request; do not
    // advertise it as a playable stream in the metadata-only library.
    let url = url
        .and_then(Value::as_str)
        .filter(|s| {
            s.starts_with("https://") && !s.contains("audio_api_unavailable") && s.len() <= 16_384
        })
        .map(str::to_owned);
    // The audio tuple's index 14 contains small and player cover URLs.
    // Prefer the player image so the same texture also works in now-playing.
    let cover_url = if let Some(row) = value.as_array() {
        row.get(14).and_then(Value::as_str).and_then(|urls| {
            let mut pair = urls.split(',');
            let small = pair.next();
            let player = pair.next();
            player
                .and_then(valid_cover_url)
                .or_else(|| small.and_then(valid_cover_url))
        })
    } else {
        cover_from_object(value)
    };
    // VK's current EAudioTupleIndex: MAIN_ARTISTS=17, FEAT_ARTISTS=18,
    // ALBUM=19. These are canonical independent references, never name splits.
    let references = value.as_array().map(|row| {
        let album = row.get(19).and_then(Value::as_array).map(|album| {
            serde_json::json!({
                "owner_id":album.first(), "id":album.get(1), "access_key":album.get(2), "title":"",
                "cover_url":cover_url,
            })
        });
        serde_json::json!({"main_artists":row.get(17),"featured_artists":row.get(18),"album":album,
            "extra":row.get(12),"release_audio_id":row.get(26)})
    });
    let metadata = references.as_ref().unwrap_or(value);
    let mut artists = artists_from_object(metadata);
    for artist in &mut artists {
        artist.name = html_escape::decode_html_entities(&artist.name).into_owned();
    }
    Ok(Track {
        id,
        owner_id,
        title: text(title)?,
        // First-party EAudioTupleIndex.SUBTITLE = 16; objectToApiAudio maps
        // website subTitle to the API's subtitle without changing TITLE.
        subtitle: subtitle_from_value(if let Some(row) = value.as_array() {
            row.get(16)
        } else {
            value.get("subtitle").or_else(|| value.get("subTitle"))
        }),
        artist: text(artist)?,
        duration,
        url,
        cover_url,
        artists,
        album: album_from_object(metadata),
        library_liked: library_liked_from_object(metadata),
        release_key: release_key_from_object(metadata),
        library_source_key: None,
        unavailable: if let Some(row) = value.as_array() {
            audio_unavailable_from_fields(row.get(21), None, row.get(10), None, None)
        } else {
            audio_unavailable_from_object(value)
        },
    })
}

/// Read only VK's global viewer assignment inside script elements. Never infer
/// identity from playlist owner IDs or arbitrary occurrences of "user_id".
fn profile_from_html(html: &str) -> Result<Profile, WebAudioError> {
    let viewer = viewer_record(html).ok_or(WebAudioError::Unverified)?;
    let id = viewer.id;
    if id <= 0 {
        return Err(WebAudioError::Expired);
    }
    let prefetched = prefetch_profile(html, id);
    let (first_name, last_name) = viewer.object.and_then(viewer_names).unwrap_or_default();
    let first_name = first_name.filter(|name| !name.is_empty()).or_else(|| {
        prefetched
            .as_ref()
            .map(|profile| profile.first_name.clone())
    });
    let last_name = last_name
        .filter(|name| !name.is_empty())
        .or_else(|| prefetched.as_ref().map(|profile| profile.last_name.clone()));
    let has_name = first_name.as_ref().is_some_and(|name| !name.is_empty())
        || last_name.as_ref().is_some_and(|name| !name.is_empty());
    let mut profile = Profile {
        id,
        first_name: if has_name {
            first_name.unwrap_or_default()
        } else {
            "VK".into()
        },
        last_name: if has_name {
            last_name.unwrap_or_default()
        } else {
            id.to_string()
        },
        avatar_url: viewer
            .object
            .and_then(viewer_avatar)
            .or_else(|| prefetched.and_then(|profile| profile.avatar_url)),
    };
    if (!profile.has_name() || profile.avatar_url.is_none())
        && let Some(header) = header_profile(html, id)
    {
        if !profile.has_name() && header.has_name() {
            profile.first_name = header.first_name;
            profile.last_name = header.last_name;
        }
        if profile.avatar_url.is_none() {
            profile.avatar_url = header.avatar_url;
        }
    }
    Ok(profile)
}

struct ViewerRecord<'a> {
    id: i64,
    object: Option<&'a str>,
}

#[cfg(test)]
fn viewer_id(html: &str) -> Option<i64> {
    viewer_record(html).map(|viewer| viewer.id)
}

fn viewer_record(html: &str) -> Option<ViewerRecord<'_>> {
    for part in html.split("<script").skip(1) {
        let Some((_, script)) = part.split_once('>') else {
            continue;
        };
        let script = script.split("</script>").next().unwrap_or_default();
        let bytes = script.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Ignore identifiers embedded in JS strings, templates or comments.
            if matches!(bytes[i], b'\'' | b'"' | b'`') {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            if bytes[i..].starts_with(b"//") {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i..].starts_with(b"/*") {
                i += 2;
                while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            let boundary =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || b"_$.".contains(&bytes[i - 1]));
            if boundary {
                for prefix in ["window.vk", "globalThis.vk", "vk"] {
                    if bytes[i..].starts_with(prefix.as_bytes()) {
                        let rest = script[i + prefix.len()..].trim_start();
                        if let Some(object) = rest.strip_prefix('=')
                            && let Some(id) = object_id(object.trim_start())
                        {
                            return Some(ViewerRecord {
                                id,
                                object: Some(object.trim_start()),
                            });
                        }
                    }
                }
                for prefix in ["window.vk.id", "globalThis.vk.id", "vk.id"] {
                    if bytes[i..].starts_with(prefix.as_bytes()) {
                        let rest = script[i + prefix.len()..].trim_start();
                        if let Some(value) = rest.strip_prefix('=') {
                            let value = value.trim_start();
                            let digits: String =
                                value.chars().take_while(char::is_ascii_digit).collect();
                            let tail = value[digits.len()..].trim_start();
                            if (tail.is_empty() || tail.starts_with([';', ',']))
                                && let Ok(id) = digits.parse()
                            {
                                return Some(ViewerRecord { id, object: None });
                            }
                        }
                    }
                }
            }
            i += 1;
        }
    }
    None
}

/// Optional names must be top-level fields of the same global object whose ID
/// was verified above. User cards, playlist metadata and page text are ignored.
fn viewer_names(object: &str) -> Option<(Option<String>, Option<String>)> {
    let fields = viewer_fields(object)?;
    let name = |key| fields.get(key).filter(|value| value.len() <= 1024).cloned();
    Some((name("first_name"), name("last_name")))
}

fn viewer_avatar(object: &str) -> Option<String> {
    let fields = viewer_fields(object)?;
    [
        "avatar_url",
        "photo_200_orig",
        "photo_200",
        "avatar",
        "photo_100",
        "photo_50",
    ]
    .into_iter()
    .find_map(|key| fields.get(key).and_then(|url| valid_cover_url(url)))
}

fn viewer_fields(object: &str) -> Option<HashMap<String, String>> {
    let bytes = object.as_bytes();
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut i = 1;
    let mut fields = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    loop {
        js_space(bytes, &mut i)?;
        if bytes.get(i) == Some(&b'}') {
            return Some(fields);
        }
        let start = i;
        let key = if matches!(bytes.get(i), Some(b'\'' | b'"')) {
            let end = js_string_end(bytes, i)?;
            let key = object.get(i + 1..end - 1)?;
            i = end;
            key
        } else {
            while bytes
                .get(i)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"_$".contains(byte))
            {
                i += 1;
            }
            if i == start {
                return None;
            }
            object.get(start..i)?
        };
        js_space(bytes, &mut i)?;
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        js_space(bytes, &mut i)?;
        let start = i;
        i = js_value_end(bytes, i)?;
        if matches!(
            key,
            "first_name"
                | "last_name"
                | "avatar_url"
                | "photo_200_orig"
                | "photo_200"
                | "avatar"
                | "photo_100"
                | "photo_50"
        ) {
            if !seen.insert(key) {
                return None;
            }
            if let Some(value) = viewer_string(object.get(start..i)?) {
                fields.insert(key.to_owned(), value);
            }
        }
        js_space(bytes, &mut i)?;
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => {
                return Some(fields);
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
fn viewer_name(value: &str) -> Option<String> {
    viewer_string(value).filter(|name| name.len() <= 1024)
}

fn viewer_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() > 8192 {
        return None;
    }
    let name = if value.starts_with('"') {
        serde_json::from_str::<String>(value).ok()?
    } else {
        let name = value.strip_prefix('\'')?.strip_suffix('\'')?;
        // Unsupported JavaScript-only escapes are omitted instead of guessing
        // a person's name. Standard JSON strings support Unicode escapes.
        if name.contains(['\\', '\'']) {
            return None;
        }
        name.to_owned()
    };
    if name.len() > 8192 || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.trim().to_owned())
}

/// Names and photos are metadata, never a replacement for session verification.
fn profile_from_user(user: &Value, viewer_id: i64) -> Option<Profile> {
    if viewer_id <= 0 || user.get("id").and_then(number) != Some(viewer_id) {
        return None;
    }
    let name = |key| {
        user.get(key)
            .and_then(Value::as_str)
            .filter(|name| name.len() <= 1024 && !name.chars().any(char::is_control))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    Some(Profile {
        id: viewer_id,
        first_name: name("first_name"),
        last_name: name("last_name"),
        avatar_url: profile_avatar_from_object(user),
    })
}

/// Read users.get entries under window.cur.apiPrefetchCache for this viewer.
fn prefetch_profile(html: &str, viewer_id: i64) -> Option<Profile> {
    let mut profile = Profile {
        id: viewer_id,
        first_name: String::new(),
        last_name: String::new(),
        avatar_url: None,
    };
    for value in script_values(
        html,
        &[
            "window.cur.apiPrefetchCache",
            "globalThis.cur.apiPrefetchCache",
            "cur.apiPrefetchCache",
            "window.cur",
            "globalThis.cur",
            "cur",
        ],
    ) {
        let Some(Ok(value)) = serde_json::Deserializer::from_str(value)
            .into_iter::<Value>()
            .next()
        else {
            continue;
        };
        let entries = value
            .as_array()
            .or_else(|| value.get("apiPrefetchCache").and_then(Value::as_array));
        for entry in entries.into_iter().flatten() {
            if entry.get("method").and_then(Value::as_str) != Some("users.get")
                || entry
                    .get("error")
                    .is_some_and(|error| !error.is_null() && error != &Value::Bool(false))
            {
                continue;
            }
            let Some(response) = entry.get("response") else {
                continue;
            };
            let users = response
                .as_array()
                .or_else(|| response.get("response").and_then(Value::as_array));
            for user in users.into_iter().flatten() {
                let Some(fresh) = profile_from_user(user, viewer_id) else {
                    continue;
                };
                if !profile.has_name() {
                    profile.first_name = fresh.first_name;
                    profile.last_name = fresh.last_name;
                }
                if profile.avatar_url.is_none() {
                    profile.avatar_url = fresh.avatar_url;
                }
                if profile.has_name() && profile.avatar_url.is_some() {
                    return Some(profile);
                }
            }
        }
    }
    (profile.has_name() || profile.avatar_url.is_some()).then_some(profile)
}

/// The actual self-profile header has this marker in VK's public frontend.
/// Without matching users.get metadata, only an explicit /id<viewer> link can
/// establish ownership; short vanity links and arbitrary feed images are ignored.
fn header_profile(html: &str, viewer_id: i64) -> Option<Profile> {
    let mut rest = html;
    let mut found = None;
    let mut headers = 0;
    while let Some(index) = rest.find('<') {
        rest = &rest[index..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.split_once("-->").map_or("", |(_, tail)| tail);
            continue;
        }
        let Some((name, attributes, body)) = html_tag(rest) else {
            break;
        };
        rest = body;
        if name.eq_ignore_ascii_case("script") {
            rest = body.split_once("</script>").map_or("", |(_, tail)| tail);
            continue;
        }
        if !name.eq_ignore_ascii_case("a") {
            continue;
        }
        let fields = html_attributes(attributes);
        if fields.get("data-testid").map(String::as_str) != Some("header-profile-menu-button")
            && fields.get("id").map(String::as_str) != Some("top_profile_link")
        {
            continue;
        }
        headers += 1;
        if headers > 1 {
            return None;
        }
        let href = fields.get("href").map(String::as_str).unwrap_or_default();
        let path = format!("/id{viewer_id}");
        let own = if href.starts_with('/') && !href.starts_with("//") {
            href.split(['?', '#'])
                .next()
                .is_some_and(|href| href.trim_end_matches('/') == path)
        } else {
            reqwest::Url::parse(href).is_ok_and(|url| {
                url.scheme() == "https"
                    && matches!(url.host_str(), Some("vk.ru" | "vk.com"))
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.port_or_known_default() == Some(443)
                    && url.path().trim_end_matches('/') == path
            })
        };
        let Some((contents, _)) = body.split_once("</a>") else {
            continue;
        };
        if !own || contents.len() > 16_384 {
            continue;
        }
        let mut images = 0;
        let mut contents = contents;
        while let Some(index) = contents.find('<') {
            contents = &contents[index..];
            if let Some(comment) = contents.strip_prefix("<!--") {
                contents = comment.split_once("-->").map_or("", |(_, tail)| tail);
                continue;
            }
            let Some((name, attributes, tail)) = html_tag(contents) else {
                break;
            };
            contents = tail;
            if name.eq_ignore_ascii_case("script") {
                contents = contents
                    .split_once("</script>")
                    .map_or("", |(_, tail)| tail);
                continue;
            }
            if name.eq_ignore_ascii_case("img") {
                images += 1;
                let attributes = html_attributes(attributes);
                // The frontend uses the viewer's first name as the header
                // image's alt text. Do not guess a surname from a full label.
                let first_name = attributes
                    .get("alt")
                    .filter(|name| name.len() <= 1024 && !name.chars().any(char::is_control))
                    .map(|name| name.trim().to_owned())
                    .unwrap_or_default();
                let profile = Profile {
                    id: viewer_id,
                    first_name,
                    last_name: String::new(),
                    avatar_url: attributes.get("src").and_then(|url| valid_cover_url(url)),
                };
                found = (profile.has_name() || profile.avatar_url.is_some()).then_some(profile);
            }
        }
        if images != 1 {
            found = None;
        }
    }
    found
}

fn html_tag(text: &str) -> Option<(&str, &str, &str)> {
    let text = text.strip_prefix('<')?;
    let end = text.find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')?;
    if end == 0 {
        // A closing tag is not a candidate, but consume it to continue scanning.
        let (_, tail) = text.split_once('>')?;
        return Some(("", "", tail));
    }
    let name = &text[..end];
    let mut quote = None;
    for (index, character) in text[end..].char_indices() {
        if let Some(current) = quote {
            if character == current {
                quote = None;
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character == '>' {
            return Some((name, &text[end..end + index], &text[end + index + 1..]));
        }
    }
    None
}

fn html_attributes(mut text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    while !text.trim_start().is_empty() {
        text = text.trim_start();
        let end = text
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(text.len());
        if end == 0 {
            break;
        }
        let key = text[..end].to_ascii_lowercase();
        text = text[end..].trim_start();
        let Some(value) = text.strip_prefix('=') else {
            continue;
        };
        text = value.trim_start();
        let value;
        if let Some(quote) = text.chars().next().filter(|c| matches!(c, '\'' | '"')) {
            let tail = &text[quote.len_utf8()..];
            let Some(end) = tail.find(quote) else {
                break;
            };
            value = &tail[..end];
            text = &tail[end + quote.len_utf8()..];
        } else {
            let end = text.find(char::is_whitespace).unwrap_or(text.len());
            value = &text[..end];
            text = &text[end..];
        }
        if value.len() <= 16_384 {
            // Duplicate attributes are ambiguous, so make them unusable.
            let value = html_escape::decode_html_entities(value).into_owned();
            fields
                .entry(key)
                .and_modify(|old: &mut String| old.clear())
                .or_insert(value);
        }
        if fields.len() >= 64 {
            break;
        }
    }
    fields
}

/// Extract assignment values only inside scripts, outside strings/comments.
/// Values stay borrowed; serde parses only the first strict JSON value.
fn script_values<'a>(html: &'a str, prefixes: &[&str]) -> Vec<&'a str> {
    let mut output = Vec::new();
    for part in html.split("<script").skip(1) {
        let Some((_, script)) = part.split_once('>') else {
            continue;
        };
        let script = script.split("</script>").next().unwrap_or_default();
        let bytes = script.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if matches!(bytes[i], b'\'' | b'"' | b'`') {
                i = js_string_end(bytes, i).unwrap_or(bytes.len());
                continue;
            }
            if bytes[i..].starts_with(b"//") {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i..].starts_with(b"/*") {
                i += 2;
                while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            let boundary =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || b"_$.".contains(&bytes[i - 1]));
            if boundary {
                for prefix in prefixes {
                    if bytes[i..].starts_with(prefix.as_bytes()) {
                        let rest = script[i + prefix.len()..].trim_start();
                        if let Some(value) = rest.strip_prefix('=') {
                            output.push(value.trim_start());
                            if output.len() >= 64 {
                                return output;
                            }
                        }
                    }
                }
            }
            i += 1;
        }
    }
    output
}

fn object_id(object: &str) -> Option<i64> {
    let bytes = object.as_bytes();
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut i = 1;
    let mut found = None;
    loop {
        js_space(bytes, &mut i)?;
        if bytes.get(i) == Some(&b'}') {
            return found;
        }
        let key_start = i;
        let key = if matches!(bytes.get(i), Some(b'\'' | b'"')) {
            let end = js_string_end(bytes, i)?;
            let key = object.get(i + 1..end - 1)?;
            i = end;
            key
        } else {
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || b"_$".contains(b))
            {
                i += 1;
            }
            if i == key_start {
                return None;
            }
            object.get(key_start..i)?
        };
        js_space(bytes, &mut i)?;
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        js_space(bytes, &mut i)?;
        let start = i;
        i = js_value_end(bytes, i)?;
        if key == "id" {
            let value = object.get(start..i)?.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let id: i64 = value.parse().ok()?;
            if found.is_some_and(|previous| previous != id) {
                return None;
            }
            found = Some(id);
        }
        js_space(bytes, &mut i)?;
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return found,
            _ => return None,
        }
    }
}

fn js_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let quote = *bytes.get(start)?;
    let mut i = start + 1;
    while let Some(&b) = bytes.get(i) {
        if b == b'\\' {
            i += 2;
        } else if b == quote {
            return Some(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

fn js_space(bytes: &[u8], i: &mut usize) -> Option<()> {
    loop {
        while bytes.get(*i).is_some_and(|b| b.is_ascii_whitespace()) {
            *i += 1;
        }
        if bytes.get(*i..)?.starts_with(b"//") {
            *i += 2;
            while bytes.get(*i).is_some_and(|b| *b != b'\n') {
                *i += 1;
            }
        } else if bytes.get(*i..)?.starts_with(b"/*") {
            *i += 2;
            while !bytes.get(*i..)?.starts_with(b"*/") {
                *i += 1;
            }
            *i += 2;
        } else {
            return Some(());
        }
    }
}

fn js_value_end(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut nesting = Vec::new();
    loop {
        js_space(bytes, &mut i)?;
        match *bytes.get(i)? {
            b'\'' | b'"' | b'`' => i = js_string_end(bytes, i)?,
            b'{' | b'[' | b'(' => {
                if nesting.len() >= 64 {
                    return None;
                }
                nesting.push(bytes[i]);
                i += 1;
            }
            b'}' if nesting.is_empty() => return Some(i),
            b'}' | b']' | b')' => {
                let opening = nesting.pop()?;
                if !matches!(
                    (opening, bytes[i]),
                    (b'{', b'}') | (b'[', b']') | (b'(', b')')
                ) {
                    return None;
                }
                i += 1;
            }
            b',' if nesting.is_empty() => return Some(i),
            _ => i += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_track_artwork_from_raw_pair_and_web_object() {
        let mut row = json!([
            42,
            -7,
            "",
            "Title",
            "Artist",
            180,
            0,
            0,
            "",
            0,
            0,
            "",
            "",
            "",
            "https://images.example.test/small.jpg,https://images.example.test/player.jpg?a=1&amp;b=2"
        ]);
        let track = parse_track(&row).unwrap();
        assert_eq!(
            track.cover_url.as_deref(),
            Some("https://images.example.test/player.jpg?a=1&b=2")
        );
        row[14] = json!("https://images.example.test/only.jpg");
        assert_eq!(
            parse_track(&row).unwrap().cover_url.as_deref(),
            Some("https://images.example.test/only.jpg")
        );
        row[14] = json!("");
        assert!(parse_track(&row).unwrap().cover_url.is_none());
        let object = json!({"id":42,"owner_id":-7,"title":"Title","artist":"Artist","duration":180,"coverUrl":"https://images.example.test/object.jpg"});
        assert_eq!(
            parse_track(&object).unwrap().cover_url.as_deref(),
            Some("https://images.example.test/object.jpg")
        );
        let object = json!({"id":42,"owner_id":-7,"title":"Title","artist":"Artist","duration":180,"album":{"thumb":{"photo_300":"https://images.example.test/album.jpg"}}});
        assert_eq!(
            parse_track(&object).unwrap().cover_url.as_deref(),
            Some("https://images.example.test/album.jpg")
        );
    }

    #[test]
    fn raw_and_object_audio_restrictions_keep_blocked_rows_without_guessing_from_the_url() {
        let base = serde_json::json!([7, 42, "", "Title", "Artist", 120]);
        assert_eq!(parse_track(&base).unwrap().unavailable, None);
        for (flags, restriction, expected) in [
            (0, 0, Some(false)),
            (4, 0, Some(true)),
            (8, 0, Some(false)),
            (0, 1, Some(true)),
            (8, 2, Some(true)),
        ] {
            let mut row = base.as_array().unwrap().clone();
            row.resize(22, Value::Null);
            row[10] = serde_json::json!(flags);
            row[21] = serde_json::json!(restriction);
            let raw = Value::Array(row);
            let (tracks, _) = home_tracks(&[raw]).unwrap();
            assert_eq!(tracks.len(), 1, "Unavailable audio must remain visible");
            assert_eq!(tracks[0].unavailable, expected);
        }
        for (extra, expected) in [
            (serde_json::json!({"dmca_blocked":true}), Some(true)),
            (serde_json::json!({"content_restricted":3}), Some(true)),
            (serde_json::json!({"content_restricted":0}), Some(false)),
            (
                serde_json::json!({"url":"https://vk.ru/mp3/audio_api_unavailable.mp3","legal_notices_type":4}),
                None,
            ),
        ] {
            let mut object = serde_json::json!({"id":7,"owner_id":42,"title":"Title","artist":"Artist","duration":120});
            object
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert_eq!(parse_track(&object).unwrap().unavailable, expected);
        }
    }

    #[test]
    fn resolves_only_matching_row_and_requires_public_https_url() {
        let response = json!({"payload":[0,[[[9,-7,"https://audio.example.test/a.m3u8"],[8,-7,"https://audio.example.test/b.mp3"]]]]}).to_string();
        assert_eq!(
            parse_reload(&response, (-7, 8), 42).unwrap(),
            "https://audio.example.test/b.mp3"
        );
        assert_eq!(
            parse_reload(&response, (-7, 10), 42),
            Err(WebAudioError::NoStream)
        );
        for value in [
            "file:///tmp/test",
            "http://audio.example.test/a.mp3",
            "https://127.0.0.1/a",
            "https://localhost/a",
            "https://user:secret@audio.example.test/a",
            "https://audio.example.test:8888/a",
            "https://audio.example.test/a\n",
        ] {
            assert!(validate_stream(value).is_err());
        }
    }

    #[test]
    fn keeps_reload_hashes_separate_from_track_metadata() {
        let row = json!([
            42,
            -7,
            "",
            "Title",
            "Artist",
            180,
            0,
            0,
            "",
            0,
            0,
            "",
            "",
            "/one/action/three/four/urlhash"
        ]);
        let info = stream_info(&row, (-7, 42));
        assert_eq!(
            info.reload_id.as_deref().map(|s| s.as_str()),
            Some("-7_42_action_urlhash")
        );
        assert!(info.original_url.is_none());
        let invalid = json!({"actionHash":"not,safe","urlHash":"other","url":""});
        assert!(stream_info(&invalid, (-7, 42)).reload_id.is_none());
    }

    #[test]
    fn decodes_url_operation_vectors_without_executing_javascript() {
        let expected = "https://audio.example.test/track/index.m3u8?token=synthetic";
        let prefix = "https://vk.ru/mp3/audio_api_unavailable.mp3?extra=";
        for (data, ops) in [
            (
                "AhrOChm6lY9HDwrPBY5LEgfTCgXLlNrLC3qVDhjHy2SVAw5KzxGUBtn1od9OB2TLBJ1ZEw5OAgvOAwm",
                "",
            ),
            (
                "y2LOzwHOBNLZpw5LA29OpZH1m2OUEgvKBMKVA2nHCNqVDhnLDc5LBhbTyxHLlM9PzhvHlY86C3bODgG",
                "DG",
            ),
            (
                "iZ8/oZHXzgqQpI8IjguUmY0MoYCUzt8Uod9KpZKQkcbKiIuVlJnLjNG+C3q/jcaUjxy4mIu/iY4/iIG",
                "EaTl",
            ),
            (
                "kci/lIm/jti4DIuUicq/Dhm+EczLmY4VjsjKicGQot9KpZGUp2uUjZSMkJmUzsqIlZ4QzgrXodS/pYm",
                "DGL4cOS",
            ),
        ] {
            assert_eq!(
                decode_stream(&format!("{prefix}{data}#{ops}"), 42).unwrap(),
                expected
            );
        }
        let mut data = b"abcdefghijklmnopqrstuvwxyz".to_vec();
        shuffle(&mut data, 37).unwrap();
        assert_eq!(&data, b"fycnwadlkjboutvxzerpgmishq");
        assert!(decode_stream(&format!("{prefix}not-valid#unknown"), 42).is_err());
    }

    #[test]
    fn account_bound_url_operations_use_native_bigint_math_without_truncating_new_vk_ids() {
        // Golden vectors generated by the first-party BigInt branch in
        // common.1151d726.js module238725, not by this Rust decoder. `s` still
        // uses the distinct JS Number/i32 branch covered by the preceding test.
        let expected = "https://audio.example.test/track/index.m3u8?token=synthetic";
        for (viewer, data, operations) in [
            (
                42,
                "ChbOzgvZp21HB3qUzweUAxH1Ahr0C3rRlMLLB3rUBc9LDc8Vl3nOBwvPywnUztHKExiZDt1REhrUyZ0=",
                "AqSZnW==",
            ),
            (
                3_000_000_001,
                "DgTLDgG4zhnUywLUCgvLCY5PCMf1Bc94Dc5TB3LWAs8VyZ9OmZP4DgvZDhrLy2qVBNvHps50zw1OA28=",
                "AqSXnW==",
            ),
            (
                4_294_967_301,
                "Ac8VBw1WC3HOzhrOC3j5Axn4zwfOlI9LDdnPogXLyw9OpZ1OAwnRBI46BMfWB3vOBNvLyY50zwrLl2S=",
                "AqS3mW==",
            ),
            (
                9_007_199_254_740_991,
                "AgvWl29Ol3rLlI9KDhHZpw55C2vHl2vZm29Oyw5TDwT1y3bOAwLOBI46BgLLlMrLCMnHp3H0Dg1OogS=",
                "AqSTmty=",
            ),
            (
                42,
                "AgvUDgvTEhbVy2rZy3i6Dgv5lI9Ol3nVCgLOptm4DhmVDwuVyw54DgfRBhq/zwqUAxvUys50zw1PA3q=",
                "AqS5mda3mtK5mJuOnZqWotKZ",
            ),
        ] {
            let data = data.trim_end_matches('=');
            let operations = operations.trim_end_matches('=');
            let url =
                format!("https://vk.ru/mp3/audio_api_unavailable.mp3?extra={data}#{operations}");
            assert_eq!(
                decode_stream(&url, viewer).unwrap(),
                expected,
                "viewer={viewer}"
            );
        }
        let mut legacy = decode_vk_base64(
            "DgTLDgG4zhnUywLUCgvLCY5PCMf1Bc94Dc5TB3LWAs8VyZ9OmZP4DgvZDhrLy2qVBNvHps50zw1OA28=",
        )
        .unwrap();
        shuffle(&mut legacy, i64::from(17_i32 ^ 3_000_000_001_u32 as i32)).unwrap();
        assert_ne!(
            String::from_utf8_lossy(&legacy),
            expected,
            "The old 32-bit path must expose the regression"
        );
    }

    fn cookie(domain: &str, path: &str, name: &str, value: &str) -> SessionCookie {
        SessionCookie {
            domain: domain.into(),
            path: path.into(),
            name: name.into(),
            value: value.into(),
            secure: true,
            expires_unix: None,
        }
    }

    #[test]
    fn accepts_current_vk_website_session_cookie_name() {
        assert!(WebAudioClient::new(vec![cookie(".vk.ru", "/", "remixsid6", "synthetic")]).is_ok());
    }

    #[test]
    fn headers_are_host_path_scoped_and_sensitive() {
        let cookies = vec![
            cookie(".vk.ru", "/", "remixsid", "synthetic"),
            cookie(".vk.com", "/", "other", "excluded"),
            cookie(".vk.ru", "/id", "narrow", "excluded"),
            cookie("evilvk.ru", "/", "bad", "excluded"),
        ];
        let header = cookie_header(&cookies, "vk.ru", "/al_audio.php").unwrap();
        assert_eq!(header.to_str().unwrap(), "remixsid=synthetic");
        assert!(header.is_sensitive());
        assert!(!path_matches("/audio", "/audios"));
        assert!(path_matches("/audio", "/audio/test"));
    }

    #[test]
    fn injected_and_expired_cookies_are_not_used() {
        assert!(
            cookie_header(
                &[cookie(".vk.ru", "/", "remixsid", "x\r\nBad: value")],
                "vk.ru",
                "/"
            )
            .is_err()
        );
        let mut expired = cookie(".vk.ru", "/", "remixsid", "synthetic");
        expired.expires_unix = Some(1);
        assert_eq!(
            cookie_header(&[expired], "vk.ru", "/")
                .unwrap()
                .to_str()
                .unwrap(),
            ""
        );
    }

    fn catalog_test_client() -> WebAudioClient {
        let client =
            WebAudioClient::new(vec![cookie("vk.ru", "/", "remixsid", "synthetic")]).unwrap();
        client.accept_profile(42).unwrap();
        client
    }
    fn catalog_test_page(
        start: i64,
        count: usize,
        next: Option<&str>,
        total: Option<usize>,
    ) -> ParsedPage {
        let list: Vec<_> = (start..start + count as i64)
            .map(|id| json!([id, 42, "", "Title", "Artist", 180]))
            .collect();
        parse_page(
            &json!({"payload":[0,["",{
                "sectionId":"library", "next_from":next,
                "playlist":{"totalCount":total,"list":list}
            }]]})
            .to_string(),
        )
        .unwrap()
    }
    fn catalog_track_ids(page: &AudioPage) -> Vec<i64> {
        page.items.iter().map(|track| track.id).collect()
    }

    #[test]
    fn logical_page_collects_five_small_continuations_without_locking_stream_resolution() {
        let client = catalog_test_client();
        let mut calls = 0;
        let page = client
            .catalog_batch(42, 0, 100, MAX_BATCH_REQUESTS, |params| {
                assert!(
                    client.state.try_lock().is_ok(),
                    "HTTP must not hold the stream-state lock"
                );
                let act = params
                    .iter()
                    .find(|(key, _)| *key == "act")
                    .unwrap()
                    .1
                    .as_str();
                assert_eq!(
                    act,
                    if calls == 0 {
                        "section"
                    } else {
                        "load_catalog_section"
                    }
                );
                if calls > 0 {
                    assert_eq!(
                        params
                            .iter()
                            .find(|(key, _)| *key == "start_from")
                            .unwrap()
                            .1,
                        format!("next-{calls}")
                    );
                }
                let parsed = catalog_test_page(
                    calls * 20 + 1,
                    20,
                    Some(&format!("next-{}", calls + 1)),
                    Some(250),
                );
                calls += 1;
                Ok(parsed)
            })
            .unwrap();
        assert_eq!(calls, 5);
        assert_eq!(catalog_track_ids(&page), (1..=100).collect::<Vec<_>>());
        assert_eq!(page.count, Some(250));
        assert_eq!(page.exhausted, Some(false));
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 100);
        assert_eq!(state.streams.len(), 100);
    }

    #[test]
    fn logical_page_preserves_overflow_and_deduplicates_only_this_pagination_sequence() {
        let client = catalog_test_client();
        let first = client
            .catalog_batch(42, 0, 100, MAX_BATCH_REQUESTS, |_| {
                Ok(catalog_test_page(1, 130, Some("more"), Some(240)))
            })
            .unwrap();
        assert_eq!(catalog_track_ids(&first), (1..=100).collect::<Vec<_>>());
        let mut requests = 0;
        let second = client
            .catalog_batch(42, 100, 100, MAX_BATCH_REQUESTS, |params| {
                requests += 1;
                assert_eq!(
                    params
                        .iter()
                        .find(|(key, _)| *key == "start_from")
                        .unwrap()
                        .1,
                    "more"
                );
                // Overlap is removed without making the UI offset skip any track.
                Ok(catalog_test_page(121, 120, None, None))
            })
            .unwrap();
        assert_eq!(requests, 1);
        assert_eq!(catalog_track_ids(&second), (101..=200).collect::<Vec<_>>());
        assert_eq!(second.count, Some(240));
        assert_eq!(second.exhausted, Some(false));
        let third = client
            .catalog_batch(42, 200, 100, MAX_BATCH_REQUESTS, |_| {
                panic!("buffered EOF must not fetch")
            })
            .unwrap();
        assert_eq!(catalog_track_ids(&third), (201..=240).collect::<Vec<_>>());
        assert_eq!(third.exhausted, Some(true));
        assert_eq!(
            client.state.lock().unwrap().cursor.as_ref().unwrap().offset,
            240
        );
    }

    #[test]
    fn later_network_failure_returns_committed_partial_batch_and_retries_the_same_cursor() {
        let client = catalog_test_client();
        let mut calls = 0;
        let first = client
            .catalog_batch(42, 0, 100, MAX_BATCH_REQUESTS, |_| {
                calls += 1;
                if calls == 1 {
                    Ok(catalog_test_page(1, 20, Some("retry-me"), Some(120)))
                } else {
                    Err(WebAudioError::Timeout)
                }
            })
            .unwrap();
        assert_eq!(first.items.len(), 20);
        assert_eq!(first.exhausted, Some(false));
        let next = client
            .catalog_batch(42, 20, 100, MAX_BATCH_REQUESTS, |params| {
                assert_eq!(
                    params
                        .iter()
                        .find(|(key, _)| *key == "start_from")
                        .unwrap()
                        .1,
                    "retry-me"
                );
                Ok(catalog_test_page(21, 100, None, Some(120)))
            })
            .unwrap();
        assert_eq!(catalog_track_ids(&next), (21..=120).collect::<Vec<_>>());
        assert_eq!(next.exhausted, Some(true));
    }

    #[test]
    fn repeated_or_empty_continuations_do_not_advance_past_unreturned_tracks() {
        for empty in [false, true] {
            let client = catalog_test_client();
            let mut calls = 0;
            let page = client
                .catalog_batch(42, 0, 100, MAX_BATCH_REQUESTS, |_| {
                    calls += 1;
                    Ok(if calls == 1 {
                        catalog_test_page(1, 20, Some("same"), None)
                    } else {
                        catalog_test_page(
                            21,
                            if empty { 0 } else { 20 },
                            Some(if empty { "different" } else { "same" }),
                            None,
                        )
                    })
                })
                .unwrap();
            assert_eq!(page.items.len(), 20);
            assert_eq!(calls, 2);
            assert_eq!(
                client.state.lock().unwrap().cursor.as_ref().unwrap().offset,
                20
            );
            let retry = client
                .catalog_batch(42, 20, 20, MAX_BATCH_REQUESTS, |_| {
                    Ok(catalog_test_page(21, 20, None, None))
                })
                .unwrap();
            assert_eq!(catalog_track_ids(&retry), (21..=40).collect::<Vec<_>>());
        }
    }

    #[test]
    fn cursor_cycles_and_duplicate_only_sequences_are_bounded() {
        let client = catalog_test_client();
        let mut calls = 0;
        let page = client
            .catalog_batch(42, 0, 100, MAX_BATCH_REQUESTS, |_| {
                calls += 1;
                let next = match calls {
                    1 => "a",
                    2 => "b",
                    _ => "a",
                };
                Ok(catalog_test_page(
                    (calls - 1) * 20 + 1,
                    20,
                    Some(next),
                    None,
                ))
            })
            .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(page.items.len(), 40);
        let client = catalog_test_client();
        let mut calls = 0;
        let page = client
            .catalog_batch(42, 0, 100, 4, |_| {
                calls += 1;
                Ok(catalog_test_page(
                    1,
                    20,
                    Some(&format!("cursor-{calls}")),
                    None,
                ))
            })
            .unwrap();
        assert_eq!(calls, 4);
        assert_eq!(page.items.len(), 20);
        assert_eq!(page.exhausted, Some(false));
    }

    #[test]
    fn profile_change_during_catalog_request_discards_the_old_account_response() {
        let client = catalog_test_client();
        let result = client.catalog_batch(42, 0, 100, MAX_BATCH_REQUESTS, |_| {
            assert!(client.state.try_lock().is_ok());
            client.accept_profile(99).unwrap();
            Ok(catalog_test_page(1, 100, None, None))
        });
        assert!(matches!(result, Err(WebAudioError::Unverified)));
        let state = client.state.lock().unwrap();
        assert_eq!(state.verified_owner, Some(99));
        assert!(state.cursor.is_none());
        assert!(state.streams.is_empty());
    }

    #[test]
    fn cached_key_lookup_queues_later_pages_without_advancing_the_ui_offset() {
        let client = catalog_test_client();
        client
            .catalog_batch(42, 0, 100, 1, |_| {
                Ok(catalog_test_page(1, 100, Some("after-100"), Some(400)))
            })
            .unwrap();
        let canceled = AtomicBool::new(false);
        let lookup = CatalogLookup {
            key: (42, 250),
            canceled: &canceled,
            deadline: Instant::now() + LOOKUP_BUDGET,
        };
        let mut calls = 0;
        client
            .catalog_fetch(42, 0, 0, MAX_LOOKUP_REQUESTS, Some(&lookup), |params| {
                calls += 1;
                let from = params
                    .iter()
                    .find(|(key, _)| *key == "start_from")
                    .unwrap()
                    .1
                    .as_str();
                assert_eq!(from, if calls == 1 { "after-100" } else { "after-200" });
                Ok(catalog_test_page(
                    if calls == 1 { 101 } else { 201 },
                    100,
                    Some(if calls == 1 { "after-200" } else { "after-300" }),
                    Some(400),
                ))
            })
            .unwrap();
        assert_eq!(calls, 2);
        {
            let state = client.state.lock().unwrap();
            let cursor = state.cursor.as_ref().unwrap();
            assert_eq!(cursor.offset, 100);
            assert_eq!(cursor.pending.len(), 200);
            assert!(state.streams.contains_key(&(42, 250)));
        }
        for offset in [100, 200] {
            let page = client
                .catalog_batch(42, offset, 100, MAX_BATCH_REQUESTS, |_| {
                    panic!("lookup results must remain queued for the UI")
                })
                .unwrap();
            assert_eq!(
                catalog_track_ids(&page),
                ((offset as i64 + 1)..=(offset as i64 + 100)).collect::<Vec<_>>()
            );
        }
        let next = client
            .catalog_batch(42, 300, 100, MAX_BATCH_REQUESTS, |params| {
                assert_eq!(
                    params
                        .iter()
                        .find(|(key, _)| *key == "start_from")
                        .unwrap()
                        .1,
                    "after-300"
                );
                Ok(catalog_test_page(301, 100, None, Some(400)))
            })
            .unwrap();
        assert_eq!(catalog_track_ids(&next), (301..=400).collect::<Vec<_>>());
    }

    #[test]
    fn canceled_lookup_retains_completed_pages_and_waiting_for_the_gate_is_cancelable() {
        let client = catalog_test_client();
        client
            .catalog_batch(42, 0, 100, 1, |_| {
                Ok(catalog_test_page(1, 100, Some("after-100"), None))
            })
            .unwrap();
        let canceled = AtomicBool::new(false);
        let lookup = CatalogLookup {
            key: (42, 500),
            canceled: &canceled,
            deadline: Instant::now() + LOOKUP_BUDGET,
        };
        let mut calls = 0;
        let result = client.catalog_fetch(42, 0, 0, MAX_LOOKUP_REQUESTS, Some(&lookup), |_| {
            calls += 1;
            if calls == 2 {
                canceled.store(true, Ordering::Release);
            }
            Ok(catalog_test_page(
                if calls == 1 { 101 } else { 201 },
                100,
                Some(if calls == 1 { "after-200" } else { "after-300" }),
                None,
            ))
        });
        assert!(matches!(result, Err(WebAudioError::Canceled)));
        let page = client
            .catalog_batch(42, 100, 100, MAX_BATCH_REQUESTS, |_| {
                panic!("already fetched metadata must not be lost")
            })
            .unwrap();
        assert_eq!(catalog_track_ids(&page), (101..=200).collect::<Vec<_>>());
        assert_eq!(
            client
                .state
                .lock()
                .unwrap()
                .cursor
                .as_ref()
                .unwrap()
                .next
                .as_deref(),
            Some("after-200")
        );
        let gate = client.page_gate.lock().unwrap();
        let worker_client = client.clone();
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = flag.clone();
        let worker = thread::spawn(move || {
            let lookup = CatalogLookup {
                key: (42, 500),
                canceled: &worker_flag,
                deadline: Instant::now() + LOOKUP_BUDGET,
            };
            worker_client.catalog_fetch(42, 0, 0, 1, Some(&lookup), |_| {
                panic!("held gate must not fetch")
            })
        });
        flag.store(true, Ordering::Release);
        assert!(matches!(
            worker.join().unwrap(),
            Err(WebAudioError::Canceled)
        ));
        drop(gate);
    }

    #[test]
    fn plural_metadata_reload_validates_the_key_and_never_changes_pagination() {
        let client = catalog_test_client();
        client
            .catalog_batch(42, 0, 100, 1, |_| {
                Ok(catalog_test_page(1, 100, Some("after-100"), None))
            })
            .unwrap();
        let raw = json!([
            250,
            42,
            "",
            "Title",
            "Artist",
            180,
            0,
            0,
            "",
            0,
            0,
            "",
            "",
            "x/y/action_hash/x/y/url_hash"
        ]);
        let payload = json!({"payload":[0,[[raw]]]}).to_string();
        let row = parse_reload_row(&payload, (42, 250)).unwrap();
        assert!(matches!(
            parse_reload_row(&payload, (42, 999)),
            Err(WebAudioError::NoStream)
        ));
        let generation = client.state.lock().unwrap().generation;
        client
            .commit_reload_metadata(42, generation, (42, 250), &row)
            .unwrap();
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 100);
        assert!(state.cursor.as_ref().unwrap().pending.is_empty());
        assert_eq!(
            state.streams[&(42, 250)]
                .reload_id
                .as_ref()
                .unwrap()
                .as_str(),
            "42_250_action_hash_url_hash"
        );
        drop(state);
        client.accept_profile(99).unwrap();
        assert_eq!(
            client.commit_reload_metadata(42, generation, (42, 250), &row),
            Err(WebAudioError::Unverified)
        );
        assert!(client.state.lock().unwrap().streams.is_empty());
    }

    #[test]
    fn cached_resolution_waits_for_initial_verification_and_can_be_canceled_without_http() {
        let client =
            WebAudioClient::new(vec![cookie("vk.ru", "/", "remixsid", "synthetic")]).unwrap();
        let worker_client = client.clone();
        let worker =
            thread::spawn(move || worker_client.resolve_cached((42, 7), &AtomicBool::new(false)));
        thread::sleep(Duration::from_millis(30));
        {
            let mut state = client.state.lock().unwrap();
            state.verified_owner = Some(42);
            state.generation = state.generation.wrapping_add(1);
            state.streams.insert(
                (42, 7),
                StreamInfo {
                    reload_id: None,
                    original_url: Some(Zeroizing::new("https://cdn.example.test/test.mp3".into())),
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            worker.join().unwrap().unwrap(),
            "https://cdn.example.test/test.mp3"
        );
        client.reset_verification().unwrap();
        let worker_client = client.clone();
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = flag.clone();
        let worker = thread::spawn(move || worker_client.resolve_cached((42, 7), &worker_flag));
        flag.store(true, Ordering::Release);
        assert_eq!(worker.join().unwrap(), Err(WebAudioError::Canceled));
    }

    fn home_test_audio(id: i64) -> Value {
        json!([
            id,
            42,
            "",
            "Title",
            "Artist",
            180,
            0,
            0,
            "",
            0,
            0,
            "",
            "",
            "x/y/action_hash/x/y/url_hash"
        ])
    }

    #[test]
    fn home_accepts_vk_generated_negative_playlist_ids_and_camel_case_subtitles() {
        let response = json!({"payload":[0,["",{"playlists":[
            {"ownerId":42,"id":-25,"title":"Generated playlist","description":"","authorLine":"<b>From VK</b>","coverUrl":"https://images.example.test/generated.jpg"},
            {"ownerId":42,"id":-26,"title":"Discover","subTitle":"New for you"},
            {"ownerId":42,"id":-27,"title":"Infinite mix","type":"vkmix:vk_mix"}
        ]}]]}).to_string();
        let home = parse_home(&response).unwrap();
        assert_eq!(home.collections.len(), 2);
        assert_eq!(home.collections[0].playlist_id, -25);
        assert_eq!(home.collections[0].subtitle, "From VK");
        assert_eq!(home.collections[1].subtitle, "New for you");
        let playlist =
            json!({"payload":[0,[{"ownerId":42,"id":-25,"list":[home_test_audio(1001)]}]]})
                .to_string();
        assert_eq!(parse_home_playlist(&playlist, 42, -25).unwrap().0.len(), 1);
    }

    #[test]
    fn home_uses_only_actual_playlist_metadata_and_does_not_invent_a_mix() {
        let response = json!({"payload":[0,["",{"playlists":[
            {"ownerId":-7,"id":10,"title":"Daily &amp; fresh","subtitle":"<b>From VK</b>","coverUrl":"https://images.example.test/card.jpg?a=1&amp;b=2","accessHash":"synthetic_key","totalCount":2,"list":[home_test_audio(1001),home_test_audio(1002)]},
            {"ownerId":42,"id":11,"title":"Preview","totalCount":20,"list":[home_test_audio(1003)]},
            {"ownerId":42,"id":12,"title":"VK Mix","type":"vkmix"},
            {"ownerId":42,"id":0,"title":"My music"}
        ],"playlist":{"list":[home_test_audio(77)]}}]]}).to_string();
        let parsed = parse_home(&response).unwrap();
        assert_eq!(parsed.collections.len(), 2);
        let first = &parsed.collections[0];
        assert_eq!(first.id, "-7_10");
        assert_eq!(first.title, "Daily & fresh");
        assert_eq!(first.subtitle, "From VK");
        assert_eq!(
            first.cover_url.as_deref(),
            Some("https://images.example.test/card.jpg?a=1&b=2")
        );
        assert_eq!(first.access_key.as_ref().unwrap().as_str(), "synthetic_key");
        assert_eq!(first.tracks.len(), 2);
        assert!(
            parsed.collections[1].tracks.is_empty(),
            "preview must not be treated as a full queue"
        );
        assert_eq!(parsed.streams.len(), 2);
        assert!(
            !parsed.streams.contains_key(&(42, 77)),
            "aggregated library records are not recommendations"
        );
    }

    #[test]
    fn home_parses_known_html_cards_and_merges_missing_metadata_without_running_scripts() {
        let card = r#"<div class="_audio_pl_item"><a onclick="showAudioPlaylist(-7, 10, 'synthetic_key', 'general')"><span class="audio_item__title">Real &amp; Card</span></a><div class="audio_pl__cover" style="background-image:url('https://images.example.test/card.jpg')"></div><span class="audio_pl__subtitle">From VK</span></div>"#;
        let html = format!(
            r#"<script>var fake="<div class='_audio_pl_item'>Fake</div>";</script><div class='CatalogBlock'>{card}</div>"#
        );
        let response = json!({"payload":[0,[html,{"playlists":[{"ownerId":-7,"id":10,"title":"Real &amp; Card"}]}]]}).to_string();
        let parsed = parse_home(&response).unwrap();
        assert_eq!(parsed.collections.len(), 1);
        assert_eq!(parsed.collections[0].title, "Real & Card");
        assert_eq!(parsed.collections[0].subtitle, "From VK");
        assert_eq!(
            parsed.collections[0].cover_url.as_deref(),
            Some("https://images.example.test/card.jpg")
        );
        assert_eq!(
            parsed.collections[0].access_key.as_ref().unwrap().as_str(),
            "synthetic_key"
        );
        let script_only =
            json!({"payload":[0,["<script>showAudioPlaylist(42, 10, 'x')</script>",{}]]})
                .to_string();
        assert!(matches!(
            parse_home(&script_only),
            Err(WebAudioError::Format)
        ));
    }

    #[test]
    fn home_and_collection_tracks_do_not_reset_the_library_cursor_or_change_accounts() {
        let client = catalog_test_client();
        client
            .catalog_batch(42, 0, 100, 1, |_| {
                Ok(catalog_test_page(1, 100, Some("after-100"), Some(200)))
            })
            .unwrap();
        let generation = client.state.lock().unwrap().generation;
        let response = json!({"payload":[0,[{"ownerId":-7,"id":10,"list":[home_test_audio(1001),home_test_audio(1002)]}]]}).to_string();
        let (tracks, mut streams) = parse_home_playlist(&response, -7, 10).unwrap();
        assert_eq!(tracks.len(), 2);
        client
            .state
            .lock()
            .unwrap()
            .streams
            .get_mut(&(42, 1))
            .unwrap()
            .reload_id = Some(Zeroizing::new("42_1_existing_action_existing_url".into()));
        streams.insert(
            (42, 1),
            StreamInfo {
                reload_id: None,
                original_url: None,
                ..Default::default()
            },
        );
        let canceled = AtomicBool::new(false);
        client
            .commit_home_streams(42, generation, streams, &canceled)
            .unwrap();
        {
            let state = client.state.lock().unwrap();
            let cursor = state.cursor.as_ref().unwrap();
            assert_eq!(cursor.offset, 100);
            assert_eq!(cursor.next.as_deref(), Some("after-100"));
            assert!(cursor.pending.is_empty());
            assert!(state.streams.contains_key(&(42, 1001)));
            assert_eq!(
                state.streams[&(42, 1)].reload_id.as_ref().unwrap().as_str(),
                "42_1_existing_action_existing_url"
            );
        }
        let next = client
            .catalog_batch(42, 100, 100, MAX_BATCH_REQUESTS, |params| {
                assert_eq!(
                    params
                        .iter()
                        .find(|(key, _)| *key == "start_from")
                        .unwrap()
                        .1,
                    "after-100"
                );
                Ok(catalog_test_page(101, 100, None, Some(200)))
            })
            .unwrap();
        assert_eq!(catalog_track_ids(&next), (101..=200).collect::<Vec<_>>());
        assert!(matches!(
            parse_home_playlist(&response, -99, 10),
            Err(WebAudioError::Format)
        ));
        canceled.store(true, Ordering::Release);
        assert!(matches!(
            client.commit_home_streams(42, generation, HashMap::new(), &canceled),
            Err(WebAudioError::Canceled)
        ));
        canceled.store(false, Ordering::Release);
        client.accept_profile(99).unwrap();
        assert!(matches!(
            client.commit_home_streams(42, generation, HashMap::new(), &canceled),
            Err(WebAudioError::Unverified)
        ));
        assert!(matches!(
            client.home(42, &canceled),
            Err(WebAudioError::WrongAccount)
        ));
    }

    #[test]
    fn home_rejects_unknown_or_expired_responses_and_unsafe_optional_fields() {
        assert!(matches!(
            parse_home(r#"{"payload":[3,[]]}"#),
            Err(WebAudioError::Expired)
        ));
        assert!(matches!(
            parse_home(r#"{"payload":[0,["",{}]]}"#),
            Err(WebAudioError::Format)
        ));
        assert!(
            parse_home(r#"{"payload":[0,["",{"playlists":[]}]]}"#)
                .unwrap()
                .collections
                .is_empty()
        );
        let collection = home_collection(&json!({"ownerId":42,"id":5,"title":"<b>Safe</b>","coverUrl":"file:///tmp/test.jpg","accessHash":"bad\nkey"})).unwrap();
        assert_eq!(collection.title, "Safe");
        assert!(collection.cover_url.is_none());
        assert!(collection.access_key.is_none());
    }

    #[test]
    fn parses_metadata_and_opaque_continuation() {
        let text = json!({"payload":[0,["html",{"sectionId":"library","next_from":"opaque:next","playlist":{"totalCount":12,"list":[[42,-90,"https://vk.ru/mp3/audio_api_unavailable.mp3","A &amp; B","Artist &#39;X&#39;",145]]}}]]}).to_string();
        let page = parse_page(&format!("<!--{text}")).unwrap();
        assert_eq!(page.page.items[0].title, "A & B");
        assert_eq!(page.page.items[0].artist, "Artist 'X'");
        assert_eq!(page.page.items[0].key(), (-90, 42));
        assert!(!page.page.items[0].has_stream());
        assert_eq!(page.page.count, Some(12));
        assert_eq!(page.page.exhausted, Some(false));
        assert_eq!(page.next.as_deref(), Some("opaque:next"));
        assert_eq!(page.section.as_deref(), Some("library"));
    }

    #[test]
    fn recognizes_explicit_empty_library_but_rejects_missing_playlist() {
        let empty = parse_page(r#"{"payload":[0,["",{"playlist":{"list":[]}}]]}"#).unwrap();
        assert!(empty.page.items.is_empty());
        assert_eq!(empty.page.exhausted, Some(true));
        assert!(matches!(
            parse_page(r#"{"payload":[0,["",{}]]}"#),
            Err(WebAudioError::Format)
        ));
        assert!(matches!(
            parse_page(r#"{"payload":[3,[]]}"#),
            Err(WebAudioError::Expired)
        ));
        assert!(matches!(
            parse_page(r#"{"payload":[8,[]]}"#),
            Err(WebAudioError::Rejected)
        ));
    }

    #[test]
    fn only_global_viewer_identifies_the_cookie_account() {
        assert_eq!(
            viewer_id("<script>var vk = {id: 42, other: 7};</script>"),
            Some(42)
        );
        assert_eq!(
            viewer_id("<script>window.vk = {\"id\":42};</script>"),
            Some(42)
        );
        assert_eq!(viewer_id("<script>vk.id = 42;</script>"), Some(42));
        assert_eq!(
            viewer_id("<script>window.vk = window.vk || {}; window.vk.id = 42;</script>"),
            Some(42)
        );
        assert_eq!(
            viewer_id("<div>vk.id = 42;</div><script>var owner_id=42;</script>"),
            None
        );
        assert_eq!(
            viewer_id(
                "<script>var x='var vk = {id: 99}'; /* vk.id=98; */ var vk={id:42};</script>"
            ),
            Some(42)
        );
        assert_eq!(object_id("{id: 42oops}"), None);
        assert_eq!(
            viewer_id("<script>vk = {api:{id:99}, title:'id:98', id:42, flags:[]};</script>"),
            Some(42)
        );
        assert_eq!(object_id("{nested:{id:99}, id:42, end:[{id:7}]}"), Some(42));
        assert_eq!(object_id("{id:42,id:43}"), None);
        assert_eq!(object_id("{nested:{id:99}}"), None);
        assert_eq!(object_id("{id:'42'}"), Some(42));
    }

    #[test]
    fn global_profile_names_ignore_unrelated_cards_and_nested_metadata() {
        let html = r#"<div>Other Person</div><script>
            var card = {id:99, first_name:"Other", last_name:"Person"};
            window.vk = {id:42, first_name:"\u0410\u043d\u043d\u0430", last_name:"Test", nested:{first_name:"Ignored"}};
        </script>"#;
        let profile = profile_from_html(html).unwrap();
        assert_eq!(profile.id, 42);
        assert_eq!(profile.first_name, "Анна");
        assert_eq!(profile.last_name, "Test");
        let profile =
            profile_from_html("<script>vk={id:42,first_name:'Test',last_name:'User'};</script>")
                .unwrap();
        assert_eq!(profile.display_name(), "Test User");
        let profile = profile_from_html(
            "<script>var card={id:99,first_name:'Other'}; vk={id:42,nested:{first_name:'Ignored'}};</script>",
        )
        .unwrap();
        assert_eq!(profile.display_name(), "VK 42");
        let profile = profile_from_html("<script>vk.id=42;</script>").unwrap();
        assert_eq!(profile.display_name(), "VK 42");
    }

    #[test]
    fn session_profile_rejects_anonymous_identity_and_does_not_invent_invalid_names() {
        assert!(matches!(
            profile_from_html("<script>vk={id:0,first_name:'Anonymous'};</script>"),
            Err(WebAudioError::Expired)
        ));
        assert!(matches!(
            profile_from_html("<script>var playlist={owner_id:42,first_name:'Other'};</script>"),
            Err(WebAudioError::Unverified)
        ));
        let html = format!(
            "<script>vk={{id:42,first_name:{},last_name:15}};</script>",
            serde_json::to_string(&"x".repeat(1025)).unwrap()
        );
        assert_eq!(profile_from_html(&html).unwrap().display_name(), "VK 42");
        assert!(viewer_name(r#""bad\nname""#).is_none());
    }

    #[test]
    fn optional_profile_lookup_checks_identity_and_preserves_verified_session_on_failure() {
        let client =
            WebAudioClient::new(vec![cookie("vk.ru", "/", "remixsid", "synthetic")]).unwrap();
        client.accept_profile(42).unwrap();
        let generation = client.state.lock().unwrap().generation;
        let canceled = AtomicBool::new(false);
        let fallback = Profile {
            id: 42,
            first_name: "VK".into(),
            last_name: "42".into(),
            avatar_url: None,
        };
        let fresh = client.profile_metadata_request(fallback.clone(), generation, &canceled, |method, params| {
            assert_eq!(method,"users.get");
            assert_eq!(params,vec![("user_ids","42".into()),("fields","photo_200".into())]);
            Ok(json!([{"id":99,"first_name":"Other"},{"id":42,"first_name":"Анна","last_name":"Test"}]))
        }).unwrap();
        assert_eq!(fresh.display_name(), "Анна Test");
        for result in [
            Err(WebAudioError::Network),
            Ok(json!([{"id":99,"first_name":"Other"}])),
        ] {
            let retained = client
                .profile_metadata_request(fallback.clone(), generation, &canceled, |_, _| result)
                .unwrap();
            assert_eq!(retained.display_name(), "VK 42");
            assert_eq!(client.state.lock().unwrap().verified_owner, Some(42));
            assert_eq!(client.state.lock().unwrap().generation, generation);
        }
        assert!(matches!(
            client.profile_metadata_request(fallback, generation, &canceled, |_, _| {
                client.accept_profile(99).unwrap();
                Ok(json!([{"id":42,"first_name":"Stale"}]))
            }),
            Err(WebAudioError::Unverified)
        ));
    }

    #[test]
    fn session_profile_recovers_names_from_matching_prefetched_users() {
        let entries = json!([
            {"method":"users.get","response":[{"id":99,"first_name":"Wrong","last_name":"Account"}]},
            {"method":"wall.get","response":[{"id":42,"first_name":"Wrong method"}]},
            {"method":"users.get","response":{"response":[{"id":"42","first_name":" Анна ","last_name":"Test"}]}}
        ]);
        for assignment in [
            format!("window.cur.apiPrefetchCache={entries};"),
            format!("cur={{\"apiPrefetchCache\":{entries}}};"),
        ] {
            let html = format!("<script>vk.id=42; {assignment}</script>");
            let profile = profile_from_html(&html).unwrap();
            assert_eq!(profile.id, 42);
            assert_eq!(profile.display_name(), "Анна Test");
            assert!(
                profile.avatar_url.is_none(),
                "A name must not require an avatar"
            );
        }
        for entries in [
            json!([{"method":"users.get","error":true,"response":[{"id":42,"first_name":"Rejected"}]}]),
            json!([{"method":"users.get","response":[{"id":99,"first_name":"Other"}]}]),
            json!([{"method":"users.get","response":[{"id":42,"first_name":"x".repeat(1025),"last_name":"bad\nname"}]}]),
        ] {
            let html = format!("<script>vk.id=42;cur.apiPrefetchCache={entries};</script>");
            assert_eq!(profile_from_html(&html).unwrap().display_name(), "VK 42");
        }
        let quoted = format!(
            "<script>vk.id=42; var text={};</script>",
            serde_json::to_string(&format!("cur.apiPrefetchCache={entries};")).unwrap()
        );
        assert_eq!(profile_from_html(&quoted).unwrap().display_name(), "VK 42");
    }

    #[test]
    fn session_profile_header_name_must_belong_to_the_unique_self_link() {
        let marker = "<a data-testid=\"header-profile-menu-button\" href=\"/id42\"><img alt=\"Анна &amp; Test\" src=\"https://images.example.test/self.jpg\"></a>";
        let html = format!("<script>vk.id=42;</script>{marker}");
        let profile = profile_from_html(&html).unwrap();
        assert_eq!(profile.display_name(), "Анна & Test");
        assert!(
            profile.last_name.is_empty(),
            "Do not split a label into guessed names"
        );
        for html in [
            html.replace("/id42", "/id99"),
            format!("{html}{marker}"),
            format!("<script>vk.id=42;</script><!-- {marker} -->"),
        ] {
            assert_eq!(profile_from_html(&html).unwrap().display_name(), "VK 42");
        }
        let html = format!(
            "<script>vk={{id:42,first_name:'Verified',last_name:'Name'}};</script>{marker}"
        );
        assert_eq!(
            profile_from_html(&html).unwrap().display_name(),
            "Verified Name"
        );
    }

    #[test]
    fn viewer_avatar_uses_only_global_self_fields_or_prefetch_with_matching_user_id() {
        let own = "https://images.example.test/self.jpg";
        let other = "https://images.example.test/other.jpg";
        let global = format!(
            "<script>var card={{id:99,photo_200:'{other}'}}; vk={{id:42,photo_200:'{own}'}};</script>"
        );
        assert_eq!(
            profile_from_html(&global).unwrap().avatar_url.as_deref(),
            Some(own)
        );
        let entries = json!([
            {"method":"users.get","response":[{"id":99,"photo_200":other}]},
            {"method":"wall.get","response":[{"id":42,"photo_200":other}]},
            {"method":"users.get","response":[{"id":42,"photo_50":own}]}
        ]);
        for assignment in [
            format!("window.cur.apiPrefetchCache={entries};"),
            format!("cur={{\"apiPrefetchCache\":{entries}}};"),
        ] {
            let html = format!("<script>vk.id=42; {assignment}</script>");
            assert_eq!(
                profile_from_html(&html).unwrap().avatar_url.as_deref(),
                Some(own)
            );
        }
        let wrong = format!(
            "<script>vk.id=42; cur.apiPrefetchCache={};</script>",
            json!([{"method":"users.get","response":[{"id":99,"photo_50":other}]}])
        );
        assert!(profile_from_html(&wrong).unwrap().avatar_url.is_none());
        let quoted = format!(
            "<script>vk.id=42; var quoted={}; /* cur.apiPrefetchCache={entries}; */</script>",
            serde_json::to_string(&format!("cur.apiPrefetchCache={entries};")).unwrap()
        );
        assert!(profile_from_html(&quoted).unwrap().avatar_url.is_none());
    }

    #[test]
    fn self_header_avatar_requires_unique_real_header_and_verified_numeric_profile_link() {
        let own = "https://images.example.test/self.jpg";
        let marker = format!(
            "<a data-testid=\"header-profile-menu-button\" href=\"/id42\"><span><img src=\"{own}\"></span></a>"
        );
        let html = format!(
            "<img src=\"https://images.example.test/other.jpg\"><script>vk.id=42;</script>{marker}"
        );
        assert_eq!(
            profile_from_html(&html).unwrap().avatar_url.as_deref(),
            Some(own)
        );
        for href in ["/id99", "/vanity-name", "https://other.example.test/id42"] {
            let wrong = html.replace("href=\"/id42\"", &format!("href=\"{href}\""));
            assert!(profile_from_html(&wrong).unwrap().avatar_url.is_none());
        }
        let duplicate = format!("{html}{marker}");
        assert!(profile_from_html(&duplicate).unwrap().avatar_url.is_none());
        let commented = format!("<script>vk.id=42;</script><!-- {marker} -->");
        assert!(profile_from_html(&commented).unwrap().avatar_url.is_none());
        let quoted = format!(
            "<script>vk.id=42; var html={};</script>",
            serde_json::to_string(&marker).unwrap()
        );
        assert!(profile_from_html(&quoted).unwrap().avatar_url.is_none());
        let unsafe_photo = html.replace(own, "http://images.example.test/self.jpg");
        assert!(
            profile_from_html(&unsafe_photo)
                .unwrap()
                .avatar_url
                .is_none()
        );
    }

    #[test]
    fn accepting_a_fresh_profile_discards_old_pagination_and_stream_credentials() {
        let client =
            WebAudioClient::new(vec![cookie("vk.ru", "/", "remixsid", "synthetic")]).unwrap();
        {
            let mut state = client.state.lock().unwrap();
            state.verified_owner = Some(99);
            state.cursor = Some(Cursor {
                owner: 99,
                offset: 10,
                section: Some("synthetic-section".into()),
                next: Some("synthetic-cursor".into()),
                ..Default::default()
            });
            state.streams.insert(
                (99, 1),
                StreamInfo {
                    reload_id: Some(Zeroizing::new("synthetic-hash".into())),
                    original_url: None,
                    ..Default::default()
                },
            );
            state.mix = Some(MixSession {
                token: Zeroizing::new("synthetic-token".into()),
                expires: now() + 3600,
                api_host: "web.api.vk.ru",
                previous_batch: vec![(99, 1)],
                previous_source: MixSource::Personal,
            });
        }
        client.reset_verification().unwrap();
        assert!(client.state.lock().unwrap().verified_owner.is_none());
        client.accept_profile(42).unwrap();
        let state = client.state.lock().unwrap();
        assert_eq!(state.verified_owner, Some(42));
        assert!(state.cursor.is_none());
        assert!(state.streams.is_empty());
        assert!(state.mix.is_none());
    }

    #[test]
    fn mix_parses_real_api_audio_batches_and_never_accepts_home_as_a_mix() {
        let track = json!({
            "id":1001,"owner_id":-7,"title":"A &amp; B","artist":"Performer","duration":180,
            "url":"https://audio.example.test/current.mp3",
            "album":{"thumb":{"photo_300":"https://images.example.test/album.jpg"}}
        });
        let response = json!({"response":[track.clone(),track]}).to_string();
        let (tracks, streams) = parse_mix_page(&response).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].key(), (-7, 1001));
        assert_eq!(tracks[0].title, "A & B");
        assert!(streams[&(-7, 1001)].original_url.is_some());
        assert!(parse_mix_page(r#"{"response":[]}"#).unwrap().0.is_empty());
        let home = json!({"payload":[0,["",{"playlist":{"type":"playlist","list":[home_test_audio(1001)]}}]]}).to_string();
        assert!(matches!(parse_mix_page(&home), Err(WebAudioError::Format)));
        assert!(matches!(
            parse_mix_page(r#"{"error":{"error_code":5}}"#),
            Err(WebAudioError::Expired)
        ));
        assert!(matches!(
            parse_mix_page(r#"{"error":{"error_code":15}}"#),
            Err(WebAudioError::Rejected)
        ));
    }

    #[test]
    fn mix_tokens_are_bound_to_the_verified_profile_and_reject_anonymous_or_expired_tokens() {
        let response = json!([
            {"user_id":99,"access_token":"another-token","expires":now()+3600},
            {"user_id":42,"access_token":"synthetic-token","expires":now()+3600}
        ])
        .to_string();
        let session = mix_login_token(&response, 42).unwrap();
        assert_eq!(session.token.as_str(), "synthetic-token");
        let wrapped = json!({"type":"okay","data":{"user_id":42,"access_token":"synthetic-token","expires":now()+3600}}).to_string();
        assert_eq!(
            mix_login_token(&wrapped, 42).unwrap().token.as_str(),
            "synthetic-token"
        );
        assert!(matches!(
            mix_login_token(&response, 7),
            Err(WebAudioError::WrongAccount)
        ));
        for token in [
            json!({"user_id":42,"access_token":"synthetic","expires":now()-1}),
            json!({"user_id":42,"access_token":"synthetic","is_anonym":true}),
            json!({"user_id":42,"access_token":"bad\nvalue"}),
            json!({"access_token":"synthetic"}),
        ] {
            assert!(mix_login_token(&token.to_string(), 42).is_err());
        }
        let page = "{id:42,apiConfigDomains:{apiDomain:'web.api.vk.ru'},webToken:{access_token:'synthetic',is_anonym:false}}";
        assert_eq!(mix_api_host(page).unwrap(), "web.api.vk.ru");
        let token = js_object_field(page, "webToken").unwrap().unwrap();
        assert_eq!(
            mix_page_token(token, 42, false).unwrap().token.as_str(),
            "synthetic"
        );
        assert!(mix_api_host(&page.replace("web.api.vk.ru", "other.example.test")).is_err());
        assert!(
            js_object_field(
                "{access_token:'one',nested:{access_token:'decoy'},access_token:'two'}",
                "access_token"
            )
            .is_err()
        );
        assert!(
            js_object_field("{nested:{access_token:'decoy'}}", "access_token")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn login_token_request_respects_cookie_domain_and_path_scope() {
        let cookies = [
            cookie(".vk.ru", "/", "domain_session", "synthetic"),
            cookie("vk.ru", "/", "host_session", "synthetic"),
            cookie(".vk.ru", "/audio", "music_path", "synthetic"),
            cookie(".vk.com", "/", "other_domain", "synthetic"),
        ];
        let header = cookie_header(&cookies, "login.vk.ru", "/").unwrap();
        assert_eq!(header.to_str().unwrap(), "domain_session=synthetic");
        assert!(!domain_matches(".vk.ru", "other.example.test"));
        assert!(!domain_matches(".vk.ru", "vk.ru.example.test"));
    }

    #[test]
    fn mix_rejects_another_account_or_cancellation_before_touching_library_state() {
        let client =
            WebAudioClient::new(vec![cookie("vk.ru", "/", "remixsid", "synthetic")]).unwrap();
        client.accept_profile(42).unwrap();
        {
            let mut state = client.state.lock().unwrap();
            state.cursor = Some(Cursor {
                owner: 42,
                offset: 17,
                next: Some("library-cursor".into()),
                ..Default::default()
            });
            state.streams.insert(
                (42, 1),
                StreamInfo {
                    reload_id: Some(Zeroizing::new("existing".into())),
                    original_url: None,
                    ..Default::default()
                },
            );
        }
        let canceled = AtomicBool::new(false);
        assert!(matches!(
            client.vk_mix(99, &canceled),
            Err(WebAudioError::WrongAccount)
        ));
        canceled.store(true, Ordering::Release);
        assert!(matches!(
            client.vk_mix(42, &canceled),
            Err(WebAudioError::Canceled)
        ));
        let state = client.state.lock().unwrap();
        let cursor = state.cursor.as_ref().unwrap();
        assert_eq!(cursor.offset, 17);
        assert_eq!(cursor.next.as_deref(), Some("library-cursor"));
        assert!(state.streams.contains_key(&(42, 1)));
        assert!(state.mix.is_none());
    }
}

#[cfg(test)]
mod playlist_tests {
    use super::*;
    use serde_json::json;

    fn playlist(owner: i64, id: i64) -> Value {
        json!({
            "owner_id":owner,"id":id,"title":"Road &amp; Sea","count":0,
            "access_key":"synthetic-access-key",
            "description":"<b>Saved</b> &amp; followed",
            "photo":{"photo_300":"https://images.example.test/playlist.jpg"}
        })
    }

    #[test]
    fn own_library_playlists_keep_order_and_followed_reference_identity() {
        let response = json!({"count":3,"items":[
            playlist(42, 8), playlist(-99, 3), playlist(42, 8)
        ]});
        let page = parse_user_playlists(&response).unwrap();
        assert_eq!(page.total, Some(3));
        assert_eq!(page.received, 3);
        assert_eq!(
            page.items
                .iter()
                .map(|item| (item.album.owner_id, item.album.id))
                .collect::<Vec<_>>(),
            [(42, 8), (-99, 3)]
        );
        let followed = &page.items[1];
        assert_eq!(followed.album.title, "Road & Sea");
        assert_eq!(
            followed.album.access_key.as_deref(),
            Some("synthetic-access-key")
        );
        assert_eq!(
            followed.album.cover_url.as_deref(),
            Some("https://images.example.test/playlist.jpg")
        );
        assert_eq!(followed.subtitle, "Saved & followed");
        assert_eq!(followed.track_count, Some(0));
    }

    #[test]
    fn empty_library_is_valid_and_optional_fields_remain_optional() {
        let page = parse_user_playlists(&json!({"count":0,"items":[]})).unwrap();
        assert_eq!(page.total, Some(0));
        assert_eq!(page.received, 0);
        assert!(page.items.is_empty());
        let page = parse_user_playlists(&json!({"items":[{
            "owner_id":42,"id":1,"title":"A playlist"
        }]}))
        .unwrap();
        assert_eq!(page.total, None);
        assert_eq!(page.items[0].track_count, None);
        assert!(page.items[0].subtitle.is_empty());
    }

    #[test]
    fn pagination_addresses_requested_slots_when_vk_omits_hidden_playlists() {
        let items: Vec<_> = (1..=35).map(|id| playlist(42, id)).collect();
        let page = parse_user_playlists(&json!({"count":38,"items":items})).unwrap();
        assert_eq!(page.received, 35);
        assert_eq!(page.next_offset(0), None);
        let items: Vec<_> = (1..=98).map(|id| playlist(42, id)).collect();
        let page = parse_user_playlists(&json!({"count":206,"items":items})).unwrap();
        assert_eq!(page.next_offset(0), Some(100));
        assert_eq!(page.next_offset(100), Some(200));
        assert_eq!(page.next_offset(200), None);
    }

    #[test]
    fn invalid_library_shapes_and_references_fail_instead_of_becoming_recommendations() {
        for response in [
            json!({}),
            json!({"items":{}}),
            json!({"count":-1,"items":[]}),
            json!({"count":"3","items":[]}),
            json!({"items":[{"owner_id":0,"id":1,"title":"Invalid"}]}),
            json!({"items":[{"owner_id":42,"id":0,"title":"Invalid"}]}),
            json!({"items":[{"owner_id":42,"id":1,"title":""}]}),
            json!({"items":[{"owner_id":42,"id":1}]}),
        ] {
            assert!(matches!(
                parse_user_playlists(&response),
                Err(WebAudioError::Format)
            ));
        }
        assert!(matches!(
            parse_user_playlists(&json!({"items":vec![playlist(42, 1);101]})),
            Err(WebAudioError::TooLarge)
        ));
    }

    #[test]
    fn canceled_or_wrong_owner_playlist_requests_preserve_library_and_mix_state() {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        {
            let mut state = client.state.lock().unwrap();
            state.cursor = Some(Cursor {
                owner: 42,
                offset: 17,
                next: Some("library-cursor".into()),
                ..Default::default()
            });
            state.streams.insert(
                (42, 7),
                StreamInfo {
                    reload_id: None,
                    original_url: Some(Zeroizing::new(
                        "https://audio.example.test/track.mp3".into(),
                    )),
                    ..Default::default()
                },
            );
            state.mix = Some(MixSession {
                token: Zeroizing::new("synthetic-token".into()),
                expires: now() + 3600,
                api_host: "web.api.vk.ru",
                previous_batch: vec![(42, 7)],
                previous_source: MixSource::Personal,
            });
        }
        assert!(matches!(
            client.playlists(42, &AtomicBool::new(true)),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.playlists(99, &AtomicBool::new(false)),
            Err(WebAudioError::WrongAccount)
        ));
        let state = client.state.lock().unwrap();
        let cursor = state.cursor.as_ref().unwrap();
        assert_eq!(cursor.offset, 17);
        assert_eq!(cursor.next.as_deref(), Some("library-cursor"));
        assert_eq!(state.streams.len(), 1);
        assert_eq!(state.mix.as_ref().unwrap().previous_batch, [(42, 7)]);
    }
}

#[cfg(test)]
mod library_count_tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        client
    }

    fn parsed(first: i64, count: usize, next: Option<&str>, total: Option<usize>) -> ParsedPage {
        let list: Vec<_> = (first..first + count as i64)
            .map(|id| json!([id, 42, "", "Title", "Artist", 180]))
            .collect();
        parse_page(
            &json!({"payload":[0,["",{"sectionId":"library","next_from":next,
            "playlist":{"totalCount":total,"list":list}}]]})
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn total_query_reads_one_native_row_without_advancing_library_or_mix() {
        let client = client();
        client.state.lock().unwrap().cursor = Some(Cursor {
            owner: 42,
            offset: 17,
            next: Some("opaque".into()),
            ..Default::default()
        });
        let canceled = AtomicBool::new(false);
        let count = client
            .library_count_request(42, &canceled, |_, params| {
                assert_eq!(
                    params,
                    vec![
                        ("owner_id", "42".into()),
                        ("count", "1".into()),
                        ("offset", "0".into())
                    ]
                );
                assert!(client.state.try_lock().is_ok());
                Ok(json!({"count":638,"items":[{"id":1}]}))
            })
            .unwrap();
        assert_eq!(count, 638);
        let state = client.state.lock().unwrap();
        let cursor = state.cursor.as_ref().unwrap();
        assert_eq!(cursor.offset, 17);
        assert_eq!(cursor.next.as_deref(), Some("opaque"));
        assert!(state.streams.is_empty() && state.mix.is_none());
    }

    #[test]
    fn first_page_gets_the_full_count_and_continuations_cannot_replace_it_with_zero() {
        let client = client();
        let first = client
            .catalog_batch(42, 0, 100, 1, |_| Ok(parsed(1, 100, Some("next"), Some(0))))
            .unwrap();
        assert_eq!(first.count, None);
        let (first, confirmed) = client
            .enrich_library_total(42, 0, first, || Ok(638))
            .unwrap();
        assert_eq!(first.items.len(), 100);
        assert_eq!(first.count, Some(638));
        assert_eq!(confirmed, Some(638));
        let next = client
            .catalog_batch(42, 100, 100, 1, |_| Ok(parsed(101, 100, None, Some(0))))
            .unwrap();
        let (next, confirmed) = client
            .enrich_library_total(42, 100, next, || {
                panic!("Do not poll the native total for each continuation")
            })
            .unwrap();
        assert_eq!(next.count, Some(638));
        assert_eq!(confirmed, None);
        assert_eq!(
            client.state.lock().unwrap().cursor.as_ref().unwrap().offset,
            200
        );
    }

    #[test]
    fn unavailable_total_stays_unknown_instead_of_becoming_the_loaded_count_or_false_zero() {
        for result in [
            Err(WebAudioError::Network),
            Err(WebAudioError::Expired),
            Ok(0),
        ] {
            let client = client();
            let page = client
                .catalog_batch(42, 0, 100, 1, |_| Ok(parsed(1, 100, None, Some(0))))
                .unwrap();
            let (page, confirmed) = client.enrich_library_total(42, 0, page, || result).unwrap();
            assert_eq!(page.count, None);
            assert_eq!(confirmed, None);
        }
        let client = client();
        let empty = client
            .catalog_batch(42, 0, 100, 1, |_| Ok(parsed(1, 0, None, Some(0))))
            .unwrap();
        let (empty, confirmed) = client.enrich_library_total(42, 0, empty, || Ok(0)).unwrap();
        assert_eq!(empty.count, Some(0));
        assert_eq!(confirmed, Some(0));
    }

    #[test]
    fn total_queries_reject_malformed_counts_cancellation_and_a_changed_account_generation() {
        let client = client();
        let canceled = AtomicBool::new(false);
        for value in [
            json!({}),
            json!({"count":null}),
            json!({"count":-1}),
            json!({"count":"638"}),
            json!({"count":1.5}),
        ] {
            assert!(matches!(
                client.library_count_request(42, &canceled, |_, _| Ok(value)),
                Err(WebAudioError::Format)
            ));
        }
        assert!(matches!(
            client.library_count_request(43, &canceled, |_, _| panic!(
                "Wrong account must stop before network"
            )),
            Err(WebAudioError::WrongAccount)
        ));
        canceled.store(true, Ordering::Release);
        assert!(matches!(
            client.library_count_request(42, &canceled, |_, _| panic!(
                "Canceled request must stop before network"
            )),
            Err(WebAudioError::Canceled)
        ));
        canceled.store(false, Ordering::Release);
        assert!(matches!(
            client.library_count_request(42, &canceled, |_, _| {
                client.state.lock().unwrap().generation += 1;
                Ok(json!({"count":638}))
            }),
            Err(WebAudioError::Unverified)
        ));
    }

    #[test]
    #[ignore = "requires a saved VK Fast session; reads count metadata only"]
    fn native_library_total_readonly() {
        let (profile, cookies) = crate::music_session_store::restore_profile()
            .unwrap()
            .expect("saved VK session");
        let client = WebAudioClient::new(cookies).unwrap();
        client.verify_owner(profile.id).unwrap();
        let legacy = client
            .catalog_batch(profile.id, 0, 100, 1, |params| {
                client.request_catalog(profile.id, params)
            })
            .unwrap();
        let native = client
            .library_count(profile.id, &AtomicBool::new(false))
            .unwrap();
        println!(
            "library total: native={}, legacy={:?}, first_page_rows={}",
            native,
            legacy.count,
            legacy.items.len()
        );
        let (enriched, _) = client
            .enrich_library_total(profile.id, 0, legacy, || Ok(native))
            .unwrap();
        assert_eq!(enriched.count, Some(native));
        assert!(native >= enriched.items.len());
        if enriched.exhausted == Some(false) {
            let next = client
                .catalog_batch(
                    profile.id,
                    enriched.items.len(),
                    100,
                    MAX_BATCH_REQUESTS,
                    |params| client.request_catalog(profile.id, params),
                )
                .unwrap();
            println!(
                "library next page: total={:?}, rows={}",
                next.count,
                next.items.len()
            );
            assert_eq!(next.count, Some(native));
        }
    }
}

#[cfg(test)]
mod track_mix_tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        let mut state = client.state.lock().unwrap();
        state.cursor = Some(Cursor {
            owner: 42,
            offset: 17,
            next: Some("library-cursor".into()),
            ..Default::default()
        });
        state.mix = Some(MixSession {
            token: Zeroizing::new("synthetic-token".into()),
            expires: now() + 3600,
            api_host: "web.api.vk.ru",
            previous_batch: Vec::new(),
            previous_source: MixSource::Personal,
        });
        drop(state);
        client
    }

    fn batch(ids: &[i64]) -> Zeroizing<String> {
        Zeroizing::new(
            json!({"response": ids.iter().map(|id| json!({
            "owner_id":-7,"id":id,"title":"Track","artist":"Artist","duration":180,
            "url":"https://audio.example.test/stream.mp3"
        })).collect::<Vec<_>>()})
            .to_string(),
        )
    }

    #[test]
    fn native_track_mix_uses_entity_identity_and_source_specific_continuations_without_changing_library()
     {
        let client = client();
        let canceled = AtomicBool::new(false);
        let seed = MixSource::Track((-9, 11));
        let page = client
            .stream_mix_request(42, seed, &canceled, |session, params| {
                assert!(session.previous_batch.is_empty());
                assert_eq!(
                    params,
                    vec![
                        ("mix_id", "track_mix".into()),
                        ("entity_id", "-9_11".into())
                    ]
                );
                Ok(batch(&[101]))
            })
            .unwrap();
        assert!(page.has_more);
        assert_eq!(page.tracks[0].key(), (-7, 101));
        assert!(page.tracks[0].url.is_none());
        client
            .stream_mix_request(42, seed, &canceled, |session, params| {
                assert_eq!(session.previous_batch, [(-7, 101)]);
                assert_eq!(params, seed.params());
                Ok(batch(&[102]))
            })
            .unwrap();
        // A new source may legitimately begin with the preceding source's last
        // batch; that must not be rejected as a stuck remote continuation.
        client
            .stream_mix_request(42, MixSource::Personal, &canceled, |session, params| {
                assert!(session.previous_batch.is_empty());
                assert_eq!(params, vec![("mix_id", "vk_mix".into())]);
                Ok(batch(&[102]))
            })
            .unwrap();
        client
            .stream_mix_request(42, MixSource::Track((42, 12)), &canceled, |session, _| {
                assert!(session.previous_batch.is_empty());
                Ok(batch(&[102]))
            })
            .unwrap();
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
        assert_eq!(
            state.cursor.as_ref().unwrap().next.as_deref(),
            Some("library-cursor")
        );
        assert!(state.streams[&(-7, 101)].original_url.is_some());
        assert!(state.mix.as_ref().unwrap().previous_source == MixSource::Track((42, 12)));
    }

    #[test]
    fn repeated_same_mix_batch_fails_without_committing_it_and_empty_mix_is_not_a_library_fallback()
    {
        let client = client();
        let canceled = AtomicBool::new(false);
        let seed = MixSource::Track((-9, 11));
        client
            .stream_mix_request(42, seed, &canceled, |_, _| Ok(batch(&[101])))
            .unwrap();
        assert!(matches!(
            client.stream_mix_request(42, seed, &canceled, |_, _| Ok(batch(&[101]))),
            Err(WebAudioError::Pagination)
        ));
        let empty = client
            .stream_mix_request(42, seed, &canceled, |_, _| Ok(batch(&[])))
            .unwrap();
        assert!(empty.tracks.is_empty() && !empty.has_more);
        assert_eq!(
            client.state.lock().unwrap().cursor.as_ref().unwrap().offset,
            17
        );
    }

    #[test]
    fn track_mix_rejects_bad_identities_cancellation_wrong_accounts_and_stale_generation() {
        let client = client();
        for seed in [(0, 1), (42, 0), (i64::MIN, 1), (-7, -1)] {
            assert!(matches!(
                client.track_mix(42, seed, &AtomicBool::new(false)),
                Err(WebAudioError::Format)
            ));
        }
        let seed = MixSource::Track((-9, 11));
        assert!(matches!(
            client.stream_mix_request(99, seed, &AtomicBool::new(false), |_, _| panic!(
                "wrong account reached network"
            )),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.stream_mix_request(42, seed, &AtomicBool::new(true), |_, _| panic!(
                "canceled request reached network"
            )),
            Err(WebAudioError::Canceled)
        ));
        let canceled = AtomicBool::new(false);
        assert!(matches!(
            client.stream_mix_request(42, seed, &canceled, |_, _| {
                canceled.store(true, Ordering::Release);
                Ok(batch(&[101]))
            }),
            Err(WebAudioError::Canceled)
        ));
        assert!(client.state.lock().unwrap().streams.is_empty());
        assert!(
            client
                .state
                .lock()
                .unwrap()
                .mix
                .as_ref()
                .unwrap()
                .previous_batch
                .is_empty()
        );
        let fresh = super::track_mix_tests::client();
        assert!(matches!(
            fresh.stream_mix_request(42, seed, &AtomicBool::new(false), |_, _| {
                fresh.accept_profile(42).unwrap();
                Ok(batch(&[101]))
            }),
            Err(WebAudioError::Unverified)
        ));
        assert!(fresh.state.lock().unwrap().streams.is_empty());
        assert!(fresh.state.lock().unwrap().mix.is_none());
    }

    fn profile(id: i64) -> Profile {
        Profile {
            id,
            first_name: "Updated".into(),
            last_name: "Viewer".into(),
            avatar_url: None,
        }
    }

    #[test]
    fn refreshing_the_same_viewer_preserves_radio_and_prefetched_foreign_audio_through_library_rebase()
     {
        let client = client();
        let generation = client.state.lock().unwrap().generation;
        {
            let mut state = client.state.lock().unwrap();
            let mix = state.mix.as_mut().unwrap();
            mix.previous_source = MixSource::Track((-9, 11));
            mix.previous_batch = vec![(-7, 101)];
            state.streams.insert(
                (-7, 101),
                StreamInfo {
                    original_url: Some(Zeroizing::new(
                        "https://audio.example.test/radio.mp3".into(),
                    )),
                    ..Default::default()
                },
            );
        }
        let refreshed = client
            .refresh_profile_request(42, || Ok(profile(42)))
            .unwrap();
        assert_eq!(refreshed.display_name(), "Updated Viewer");
        {
            let state = client.state.lock().unwrap();
            assert_eq!(state.generation, generation);
            assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
            assert_eq!(state.mix.as_ref().unwrap().previous_batch, [(-7, 101)]);
        }
        let track: Track = serde_json::from_value(
            json!({"id":9,"owner_id":42,"title":"Library track","duration":180}),
        )
        .unwrap();
        let page = client
            .catalog_batch(42, 0, 100, 1, |params| {
                assert_eq!(
                    params.iter().find(|(key, _)| *key == "act").unwrap().1,
                    "section"
                );
                Ok(ParsedPage {
                    page: AudioPage {
                        count: Some(1),
                        items: vec![track.clone()],
                        exhausted: None,
                    },
                    section: Some("rebased-library".into()),
                    next: None,
                    streams: HashMap::from([(
                        (42, 9),
                        StreamInfo {
                            original_url: Some(Zeroizing::new(
                                "https://audio.example.test/library.mp3".into(),
                            )),
                            ..Default::default()
                        },
                    )]),
                })
            })
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let state = client.state.lock().unwrap();
        assert_eq!(state.generation, generation);
        assert_eq!(state.cursor.as_ref().unwrap().offset, 1);
        assert_eq!(
            state.cursor.as_ref().unwrap().section.as_deref(),
            Some("rebased-library")
        );
        assert!(state.streams[&(-7, 101)].original_url.is_some());
        assert!(state.streams[&(42, 9)].original_url.is_some());
        assert!(state.mix.as_ref().unwrap().previous_source == MixSource::Track((-9, 11)));
        assert_eq!(state.mix.as_ref().unwrap().previous_batch, [(-7, 101)]);
    }

    #[test]
    fn viewer_refresh_preserves_existing_music_on_network_error_and_verifies_an_initial_client() {
        let client = client();
        let generation = client.state.lock().unwrap().generation;
        assert!(matches!(
            client.refresh_profile_request(42, || Err(WebAudioError::Network)),
            Err(WebAudioError::Network)
        ));
        {
            let state = client.state.lock().unwrap();
            assert_eq!(state.generation, generation);
            assert_eq!(state.verified_owner, Some(42));
            assert!(state.mix.is_some() && state.cursor.is_some());
        }
        client.reset_verification().unwrap();
        let generation = client.state.lock().unwrap().generation;
        assert_eq!(
            client
                .refresh_profile_request(42, || Ok(profile(42)))
                .unwrap()
                .id,
            42
        );
        let state = client.state.lock().unwrap();
        assert_eq!(state.verified_owner, Some(42));
        assert!(state.generation > generation);
        assert!(state.mix.is_none() && state.cursor.is_none() && state.streams.is_empty());
    }

    #[test]
    fn viewer_refresh_invalidates_an_account_change_but_cannot_erase_a_fresh_generation() {
        for error in [None, Some(WebAudioError::Expired)] {
            let client = client();
            let generation = client.state.lock().unwrap().generation;
            let result = client.refresh_profile_request(42, || match error.clone() {
                Some(error) => Err(error),
                None => Ok(profile(99)),
            });
            assert_eq!(
                result.err(),
                Some(error.unwrap_or(WebAudioError::WrongAccount))
            );
            let state = client.state.lock().unwrap();
            assert!(
                state.verified_owner.is_none()
                    && state.mix.is_none()
                    && state.cursor.is_none()
                    && state.streams.is_empty()
            );
            assert!(state.generation > generation);
        }
        let mismatch = client();
        assert!(matches!(
            mismatch
                .refresh_profile_request(99, || panic!("mismatched caller must stop before HTTP")),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(mismatch.state.lock().unwrap().verified_owner.is_none());
        let stale = client();
        let result = stale.refresh_profile_request(42, || {
            stale.accept_profile(99).unwrap();
            Ok(profile(42))
        });
        assert_eq!(result.err(), Some(WebAudioError::Unverified));
        assert_eq!(stale.state.lock().unwrap().verified_owner, Some(99));
    }

    #[test]
    #[ignore = "requires a saved VK Fast session; only fetches music recommendations"]
    fn native_track_mix_readonly() {
        let (profile, cookies) = crate::music_session_store::restore_profile()
            .unwrap()
            .unwrap();
        let client = WebAudioClient::new(cookies).unwrap();
        client.verify_owner(profile.id).unwrap();
        let first = client.page(profile.id, 0).unwrap();
        let seed = first
            .items
            .iter()
            .find(|track| !track.artists.is_empty())
            .or_else(|| first.items.first())
            .unwrap();
        let canceled = AtomicBool::new(false);
        let mix = client.track_mix(profile.id, seed.key(), &canceled).unwrap();
        let next = client.track_mix(profile.id, seed.key(), &canceled).unwrap();
        println!(
            "track mix recommendations: first={}, next={}, has_more={}, next_has_more={}",
            mix.tracks.len(),
            next.tracks.len(),
            mix.has_more,
            next.has_more
        );
        assert!(!mix.tracks.is_empty());
        client.resolve(mix.tracks[0].key()).unwrap();
        println!("track mix first stream resolved without playback");
    }
}

#[cfg(test)]
mod library_action_tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        {
            let mut state = client.state.lock().unwrap();
            state.cursor = Some(Cursor {
                owner: 42,
                offset: 17,
                next: Some("ui-cursor".into()),
                ..Default::default()
            });
            state.mix = Some(MixSession {
                token: Zeroizing::new("synthetic-token".into()),
                expires: now() + 3600,
                api_host: "web.api.vk.ru",
                previous_batch: vec![(42, 3)],
                previous_source: MixSource::Personal,
            });
        }
        client
    }

    fn audio(owner: i64, id: i64, release: Option<&str>) -> Value {
        json!({"id":id,"owner_id":owner,"title":"Identical title","artist":"Identical artist",
            "duration":120,"release_audio_id":release})
    }

    /// Opt-in, read-only protocol diagnostic. Prints counts, never account IDs,
    /// track titles, private response bodies, credentials, or request URLs.
    #[test]
    #[ignore = "requires a saved VK Fast session and reads the current catalog"]
    fn native_library_like_counts_readonly() {
        let (profile, cookies) = crate::music_session_store::restore_profile()
            .expect("saved session should be readable")
            .expect("this opt-in diagnostic requires a saved session");
        let client = WebAudioClient::new(cookies).expect("saved session should be valid");
        client
            .verify_owner(profile.id)
            .expect("saved account should match VK");
        let canceled = AtomicBool::new(false);
        let generation = client.home_owner(profile.id, &canceled).unwrap();
        let native = client
            .detail_request(
                profile.id,
                generation,
                "audio.get",
                vec![
                    ("owner_id", profile.id.to_string()),
                    ("count", "100".into()),
                    ("offset", "0".into()),
                ],
                &canceled,
            )
            .expect("read-only native library request should succeed");
        let (tracks, _) = home_tracks(native["items"].as_array().unwrap()).unwrap();
        let report = |label: &str, tracks: &[Track]| {
            println!(
                "{label}: total={}, own={}, like_true={}, like_false={}, like_missing={}, own_false={}, release_ids={}",
                tracks.len(),
                tracks
                    .iter()
                    .filter(|track| track.owner_id == profile.id)
                    .count(),
                tracks
                    .iter()
                    .filter(|track| track.library_liked == Some(true))
                    .count(),
                tracks
                    .iter()
                    .filter(|track| track.library_liked == Some(false))
                    .count(),
                tracks
                    .iter()
                    .filter(|track| track.library_liked.is_none())
                    .count(),
                tracks
                    .iter()
                    .filter(
                        |track| track.owner_id == profile.id && track.library_liked == Some(false)
                    )
                    .count(),
                tracks
                    .iter()
                    .filter(|track| track.release_key.is_some())
                    .count()
            );
        };
        report("native own library first100", &tracks);
        let legacy = client
            .page_batch(profile.id, 0, 100)
            .expect("read-only website library request should succeed");
        report("legacy own library first100", &legacy.items);
        if let Some(first) = tracks.first() {
            let search = client
                .search(profile.id, &first.title, 0, &canceled)
                .expect("read-only search should succeed");
            report("native search first page", &search.tracks);
            if let Some(liked) = search
                .tracks
                .iter()
                .find(|track| track.library_liked == Some(true))
            {
                let fresh = client
                    .track_metadata(profile.id, liked.key(), &canceled)
                    .expect("read-only original metadata should succeed");
                report("fresh getById saved search source", &[fresh]);
            }
        }
        if let Some(album) = tracks.iter().find_map(|track| track.album.as_ref()) {
            let response = client
                .detail_request(
                    profile.id,
                    generation,
                    "audio.get",
                    vec![
                        ("owner_id", album.owner_id.to_string()),
                        ("playlist_id", album.id.to_string()),
                        ("access_key", album.access_key.clone().unwrap_or_default()),
                        ("count", "100".into()),
                        ("offset", "0".into()),
                    ],
                    &canceled,
                )
                .expect("read-only album request should succeed");
            let (rows, _) = home_tracks(response["items"].as_array().unwrap()).unwrap();
            report("native album first100", &rows);
        }
    }

    #[test]
    fn native_add_returns_owned_identity_and_delete_validates_every_reported_copy() {
        assert_eq!(
            parse_library_add(&json!({"items":[{"new_audio_id":901}]}), 42),
            Ok((42, 901))
        );
        assert_eq!(parse_library_add(&json!(901), 42), Ok((42, 901)));
        for invalid in [
            json!({"items":[]}),
            json!({"items":[{"new_audio_id":0}]}),
            json!(true),
            json!({"items":[{"new_audio_id":3},{"new_audio_id":4}]}),
        ] {
            assert_eq!(parse_library_add(&invalid, 42), Err(WebAudioError::Format));
        }
        assert_eq!(
            parse_library_remove(
                &json!({"audio_ids":["42_901","42_902","42_901"]}),
                42,
                (42, 901)
            ),
            Ok(vec![(42, 901), (42, 902)])
        );
        assert_eq!(
            parse_library_remove(&json!(1), 42, (42, 901)),
            Ok(vec![(42, 901)])
        );
        assert_eq!(
            parse_library_remove(&json!({"audio_ids":["99_901"]}), 42, (42, 901)),
            Err(WebAudioError::WrongAccount)
        );
        for invalid in [
            json!({"audio_ids":[]}),
            json!({"audio_ids":["42_902"]}),
            json!({"audio_ids":["42_901_extra"]}),
            json!(0),
        ] {
            assert_eq!(
                parse_library_remove(&invalid, 42, (42, 901)),
                Err(WebAudioError::Format)
            );
        }
        assert_eq!(parse_library_restore(&json!(1), (42, 901)), Ok((42, 901)));
        assert_eq!(
            parse_library_restore(&json!({"owner_id":42,"id":901}), (42, 901)),
            Ok((42, 901))
        );
        assert_eq!(
            parse_library_restore(&json!({"owner_id":99,"id":901}), (42, 901)),
            Err(WebAudioError::Format)
        );
    }

    #[test]
    fn mutation_transport_uses_source_credentials_and_restores_without_duplicate_add() {
        let client = client();
        let info = stream_info(
            &json!({"access_key":"synthetic_access","track_code":"synthetic/tracking=code"}),
            (-200, 7),
        );
        client.state.lock().unwrap().streams.insert((-200, 7), info);
        let response = client
            .library_action_request(
                42,
                (-200, 7),
                LibraryAction::Add,
                &AtomicBool::new(false),
                |_, method, params| {
                    assert_eq!(method, "audio.add");
                    assert_eq!(
                        params,
                        vec![
                            ("audio_id", "7".into()),
                            ("owner_id", "-200".into()),
                            ("access_key", "synthetic_access".into()),
                            ("track_code", "synthetic/tracking=code".into())
                        ]
                    );
                    Ok(json!({"items":[{"new_audio_id":901}]}))
                },
            )
            .unwrap();
        assert_eq!(parse_library_add(&response, 42), Ok((42, 901)));
        client
            .library_action_request(
                42,
                (42, 901),
                LibraryAction::Restore,
                &AtomicBool::new(false),
                |_, method, params| {
                    assert_eq!(method, "audio.restore");
                    assert_eq!(
                        params,
                        vec![("audio_id", "901".into()), ("owner_id", "42".into())]
                    );
                    Ok(json!(1))
                },
            )
            .unwrap();
        client
            .library_action_request(
                42,
                (42, 901),
                LibraryAction::Remove,
                &AtomicBool::new(false),
                |_, method, params| {
                    assert_eq!(method, "audio.delete");
                    assert_eq!(
                        params,
                        vec![("audio_id", "901".into()), ("owner_id", "42".into())]
                    );
                    Ok(json!({"audio_ids":["42_901"]}))
                },
            )
            .unwrap();
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
        assert_eq!(
            state.cursor.as_ref().unwrap().next.as_deref(),
            Some("ui-cursor")
        );
        assert_eq!(state.mix.as_ref().unwrap().previous_batch, vec![(42, 3)]);
    }

    #[test]
    fn invalid_account_cancellation_and_stale_session_cannot_commit_library_actions() {
        let client = client();
        for (owner, key, action, canceled, expected) in [
            (
                99,
                (-200, 7),
                LibraryAction::Add,
                false,
                WebAudioError::WrongAccount,
            ),
            (
                42,
                (99, 7),
                LibraryAction::Remove,
                false,
                WebAudioError::WrongAccount,
            ),
            (
                42,
                (99, 7),
                LibraryAction::Restore,
                false,
                WebAudioError::WrongAccount,
            ),
            (
                42,
                (-200, 7),
                LibraryAction::Add,
                true,
                WebAudioError::Canceled,
            ),
            (
                42,
                (-200, 0),
                LibraryAction::Add,
                false,
                WebAudioError::Format,
            ),
        ] {
            assert_eq!(
                client.library_action_request(
                    owner,
                    key,
                    action,
                    &AtomicBool::new(canceled),
                    |_, _, _| panic!("invalid action must not reach HTTP")
                ),
                Err(expected)
            );
        }
        let mut requests = 0;
        assert_eq!(
            client.library_action_request(
                42,
                (-200, 7),
                LibraryAction::Add,
                &AtomicBool::new(false),
                |_, _, _| {
                    requests += 1;
                    Err(WebAudioError::Timeout)
                }
            ),
            Err(WebAudioError::Timeout)
        );
        assert_eq!(requests, 1);
        let canceled = AtomicBool::new(false);
        assert_eq!(
            client.library_action_request(
                42,
                (-200, 7),
                LibraryAction::Add,
                &canceled,
                |_, _, _| {
                    canceled.store(true, Ordering::Release);
                    Ok(json!({"items":[{"new_audio_id":901}]}))
                }
            ),
            Err(WebAudioError::Canceled)
        );
        assert_eq!(
            client.library_action_request(
                42,
                (-200, 7),
                LibraryAction::Add,
                &AtomicBool::new(false),
                |_, _, _| {
                    client.accept_profile(99).unwrap();
                    Ok(json!({"items":[{"new_audio_id":901}]}))
                }
            ),
            Err(WebAudioError::Unverified)
        );
    }

    #[test]
    fn source_delete_checks_fresh_membership_and_returns_only_exact_owned_copies() {
        let client = client();
        let read = std::cell::Cell::new(false);
        let result = client
            .remove_source_from_library_request(
                42,
                (-200, 7),
                &AtomicBool::new(false),
                || {
                    read.set(true);
                    let mut source = audio(-200, 7, None);
                    source["like"] = json!(true);
                    parse_track(&source)
                },
                |_, method, params| {
                    assert!(read.get());
                    assert_eq!(method, "audio.delete");
                    assert_eq!(
                        params,
                        vec![("audio_id", "7".into()), ("owner_id", "-200".into())]
                    );
                    Ok(json!({"audio_ids":["42_901","42_902"]}))
                },
            )
            .unwrap();
        assert_eq!(result, vec![(42, 901), (42, 902)]);
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
        assert_eq!(
            state.cursor.as_ref().unwrap().next.as_deref(),
            Some("ui-cursor")
        );
        assert_eq!(state.mix.as_ref().unwrap().previous_batch, vec![(42, 3)]);
        assert_eq!(
            parse_library_source_remove(&json!({"audio_ids":["42_901","99_902"]}), 42),
            Err(WebAudioError::WrongAccount)
        );
        assert_eq!(
            parse_library_source_remove(&json!(1), 42),
            Err(WebAudioError::Format)
        );
        assert_eq!(
            parse_library_source_remove(&json!({"audio_ids":[]}), 42),
            Err(WebAudioError::Format)
        );
    }

    #[test]
    fn source_delete_rejects_unknown_or_stale_membership_before_mutation() {
        let client = client();
        let flag = AtomicBool::new(false);
        for like in [None, Some(false)] {
            let mut source = audio(-200, 7, None);
            source["like"] = json!(like);
            assert_eq!(
                client.remove_source_from_library_request(
                    42,
                    (-200, 7),
                    &flag,
                    || parse_track(&source),
                    |_, _, _| panic!("unknown membership cannot mutate")
                ),
                Err(WebAudioError::Rejected)
            );
        }
        assert_eq!(
            client.remove_source_from_library_request(
                99,
                (-200, 7),
                &flag,
                || panic!("wrong account cannot read"),
                |_, _, _| panic!("wrong account cannot mutate")
            ),
            Err(WebAudioError::WrongAccount)
        );
        assert_eq!(
            client.remove_source_from_library_request(
                42,
                (-200, 7),
                &AtomicBool::new(true),
                || panic!("canceled action cannot read"),
                |_, _, _| panic!("canceled action cannot mutate")
            ),
            Err(WebAudioError::Canceled)
        );
        assert_eq!(
            client.remove_source_from_library_request(
                42,
                (-200, 7),
                &flag,
                || {
                    let mut source = audio(-200, 7, None);
                    source["like"] = json!(true);
                    client.accept_profile(99).unwrap();
                    parse_track(&source)
                },
                |_, _, _| panic!("stale session cannot mutate")
            ),
            Err(WebAudioError::Unverified)
        );
    }

    #[test]
    fn legacy_tuple_retains_native_like_release_and_only_volatile_action_credentials() {
        let mut row = json!([7, -200, "", "Song", "Artist", 120]);
        row.as_array_mut().unwrap().resize(27, Value::Null);
        row[12] = json!("{\"like\":true}");
        row[20] = json!("tracking/context");
        row[24] = json!("synthetic_access");
        row[26] = json!("-200_777");
        let track = parse_track(&row).unwrap();
        assert_eq!(track.library_liked, Some(true));
        assert_eq!(track.release_key, Some((-200, 777)));
        let info = stream_info(&row, track.key());
        assert_eq!(
            info.access_key.as_ref().unwrap().as_str(),
            "synthetic_access"
        );
        assert_eq!(info.track_code.as_deref(), Some("tracking/context"));
        let native=parse_track(&json!({"id":7,"owner_id":-200,"title":"Song","artist":"Artist","duration":120,"like":false,"release_audio_id":"-200_777"})).unwrap();
        assert_eq!(native.library_liked, Some(false));
        assert_eq!(native.release_key, Some((-200, 777)));
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;
    use serde_json::json;

    fn artist() -> ArtistRef {
        ArtistRef {
            id: "canonical-1".into(),
            name: "Canonical".into(),
        }
    }

    fn album() -> AlbumRef {
        AlbumRef {
            owner_id: -200,
            id: 3,
            title: "Album".into(),
            access_key: None,
            cover_url: None,
        }
    }

    fn track(id: i64) -> Value {
        json!({"id":id,"owner_id":-200,"artist":"Canonical","title":"Song","duration":120,
            "main_artists":[{"id":"canonical-1","name":"Canonical"}],
            "url":"https://audio.example.test/file.mp3"})
    }

    fn catalog() -> Value {
        json!({"catalog":{"default_section":"main","sections":[{"id":"main","blocks":[
            {"data_type":"artist","artists_ids":["canonical-1"]},
            {"data_type":"music_audios","audios_ids":["-200_2","-200_1"]},
            {"data_type":"music_playlists","layout":{"style":"artist_releases"},"playlists_ids":["-200_3","-200_4"]},
            {"data_type":"music_playlists","playlists_ids":["-200_99"]}
        ]}]},"artists":[
            {"id":"related","name":"Other"},
            {"id":"canonical-1","name":"Canonical","photo":[
                {"width":40,"url":"https://images.example.test/small.jpg"},
                {"width":900,"url":"https://images.example.test/large.jpg"},
                {"width":1200,"url":"http://localhost/wrong.jpg"}]}
        ],"audios":[track(1),track(99),track(2)],"playlists":[
            {"id":4,"owner_id":-200,"title":"Older"},
            {"id":99,"owner_id":-200,"title":"Editorial"},
            {"id":3,"owner_id":-200,"title":"Latest","photo":{"photo_600":"https://images.example.test/album.jpg"}}
        ]})
    }

    #[test]
    fn artist_catalog_obeys_explicit_blocks_and_excludes_auxiliary_and_editorial_data() {
        let (page, streams) = parse_artist_page(&catalog(), &artist()).unwrap();
        assert_eq!(page.artist, artist());
        assert_eq!(
            page.tracks.iter().map(Track::key).collect::<Vec<_>>(),
            [(-200, 2), (-200, 1)]
        );
        assert_eq!(
            page.latest_releases
                .iter()
                .map(|release| release.album.id)
                .collect::<Vec<_>>(),
            [3, 4]
        );
        assert!(page.tracks.iter().all(|track| track.url.is_none()));
        assert_eq!(streams.len(), 2);
        assert_eq!(
            page.image_url.as_deref(),
            Some("https://images.example.test/large.jpg")
        );
        assert_eq!(
            page.latest_releases[0].album.cover_url.as_deref(),
            Some("https://images.example.test/album.jpg")
        );
        let mut missing = catalog();
        missing["audios"] = json!([track(1)]);
        assert!(matches!(
            parse_artist_page(&missing, &artist()),
            Err(WebAudioError::Format)
        ));
        let unrelated = ArtistRef {
            id: "related".into(),
            name: "Other".into(),
        };
        assert!(matches!(
            parse_artist_page(&catalog(), &unrelated),
            Err(WebAudioError::Format)
        ));
    }

    #[test]
    fn exact_name_fallback_rejects_partial_or_ambiguous_artist_search_results() {
        let results = json!({"items":[{"id":"partial","name":"Canonical Remix"},{"id":"canonical-1","name":"CANONICAL"}]});
        assert_eq!(
            exact_artist(&results, " canonical ").unwrap().id,
            "canonical-1"
        );
        assert!(exact_artist(&results, "Unknown").is_err());
        let ambiguous = json!({"items":[{"id":"one","name":"Same"},{"id":"two","name":"same"}]});
        assert!(exact_artist(&ambiguous, "Same").is_err());
        let duplicates = json!({"items":[{"id":"one","name":"Same"},{"id":"one","name":"Same"},{"id":"two","name":"Same"}]});
        assert!(exact_artist(&duplicates, "Same").is_err());
    }

    #[test]
    fn track_versions_come_from_vk_subtitle_fields_in_all_payload_shapes() {
        let mut row = json!([7, 42, "", "CICADATEKK", "Artist", 120]);
        row.as_array_mut().unwrap().resize(17, Value::Null);
        for subtitle in ["Slowed", "Sped Up", "Remix &amp; Reverb"] {
            row[16] = json!(subtitle);
            let parsed = parse_track(&row).unwrap();
            assert_eq!(parsed.title, "CICADATEKK");
            assert_eq!(
                parsed.subtitle.as_deref(),
                Some(html_escape::decode_html_entities(subtitle).as_ref())
            );
            assert!(
                parsed
                    .display_title()
                    .contains(parsed.subtitle.as_deref().unwrap())
            );
            for field in ["subtitle", "subTitle"] {
                let mut value = json!({"id":7,"owner_id":42,"title":"CICADATEKK","artist":"Artist","duration":120});
                value[field] = json!(subtitle);
                assert_eq!(
                    parse_track(&value).unwrap().display_title(),
                    parsed.display_title()
                );
            }
        }
        for missing in [
            Value::Null,
            json!(false),
            json!(""),
            json!({"unexpected":1}),
        ] {
            row[16] = missing;
            assert!(parse_track(&row).unwrap().subtitle.is_none());
        }
    }

    #[test]
    fn legacy_rows_keep_each_canonical_artist_and_album_without_guessing_name_separators() {
        let mut row = json!([7, 42, "", "Song", "A &amp; B, Guest", 120]);
        row.as_array_mut().unwrap().resize(20, Value::Null);
        row[17] = json!([{"id":"artist-a","name":"A &amp; B"}]);
        row[18] = json!([{"id":"artist-b","name":"Guest"}]);
        row[19] = json!([-200, 3, "private-album-key"]);
        let parsed = parse_track(&row).unwrap();
        assert_eq!(parsed.artists.len(), 2);
        assert_eq!(parsed.artists[0].name, "A & B");
        assert_eq!(parsed.artists[1].id, "artist-b");
        assert_eq!(parsed.album.as_ref().unwrap().id, 3);
        assert_eq!(
            parsed.album.unwrap().access_key.as_deref(),
            Some("private-album-key")
        );
        row[17] = Value::Null;
        row[18] = Value::Null;
        assert!(parse_track(&row).unwrap().artists.is_empty());
    }

    #[test]
    fn album_metadata_is_bound_to_requested_identity_and_keeps_new_access_key() {
        let reference = album();
        let response = json!({"id":3,"owner_id":-200,"title":"Official title","access_key":"fresh",
            "year":2026,"main_artists":[{"id":"canonical-1","name":"Canonical"}],
            "photo":{"photo_600":"https://images.example.test/album.jpg"}});
        let page = parse_album_page(&response, &reference).unwrap();
        assert_eq!(page.album.title, "Official title");
        assert_eq!(page.album.access_key.as_deref(), Some("fresh"));
        assert_eq!(page.artists, [artist()]);
        assert_eq!(page.year, Some(2026));
        let mut other = response.clone();
        other["owner_id"] = json!(-201);
        assert!(matches!(
            parse_album_page(&other, &reference),
            Err(WebAudioError::Format)
        ));
    }

    #[test]
    fn saved_playlists_keep_their_playback_identity_but_show_the_original_group_creator() {
        let reference = AlbumRef {
            owner_id: 42,
            id: 3,
            title: "Saved".into(),
            access_key: Some("saved-key".into()),
            cover_url: None,
        };
        let value = json!({"playlist":{"id":3,"owner_id":42,"title":"Saved","access_key":"fresh-saved-key",
            "original":{"owner_id":-200,"playlist_id":99},"description":"Description","plays":861,
            "main_artists":[{"id":"canonical-1","name":"Musical performer"}]},
            "profiles":[{"id":42,"first_name":"Local","last_name":"Viewer"},{"id":200,"name":"Wrong namespace"}],
            "groups":[{"id":200,"name":"Original &amp; creator"}]});
        let page = parse_album_page(&value, &reference).unwrap();
        assert_eq!((page.album.owner_id, page.album.id), (42, 3));
        assert_eq!(page.album.access_key.as_deref(), Some("fresh-saved-key"));
        assert_eq!(
            page.creator,
            Some(PlaylistCreator {
                owner_id: -200,
                name: "Original & creator".into()
            })
        );
        assert_eq!(page.artists[0].name, "Musical performer");
        assert_eq!(page.description.as_deref(), Some("Description"));
        assert_eq!(page.stats.plays, Some(861));
    }

    #[test]
    fn playlist_person_creators_are_exact_optional_plain_text_and_do_not_fall_back_to_the_viewer_or_artist()
     {
        let reference = album();
        let mut value = json!({"playlist":{"id":3,"owner_id":-200,"title":"Playlist",
            "original":{"owner_id":7,"playlist_id":91},"main_artists":[{"id":"canonical-1","name":"Artist"}]},
            "profiles":[{"id":7,"first_name":"<b>First</b>","last_name":"Last &amp; Guest"}],
            "groups":[{"id":7,"name":"Wrong group"},{"id":200,"name":"Local group"}]});
        assert_eq!(
            parse_album_page(&value, &reference).unwrap().creator,
            Some(PlaylistCreator {
                owner_id: 7,
                name: "First Last & Guest".into()
            })
        );
        value["profiles"] = json!([{"id":42,"first_name":"Viewer"}]);
        assert!(
            parse_album_page(&value, &reference)
                .unwrap()
                .creator
                .is_none()
        );
        value["profiles"] =
            json!([{"id":7,"first_name":"X".repeat(2000),"last_name":"Y".repeat(2000)}]);
        assert_eq!(
            parse_album_page(&value, &reference)
                .unwrap()
                .creator
                .unwrap()
                .name
                .chars()
                .count(),
            512
        );
        assert!(playlist_creator_id(&json!({"owner_id":0})).is_none());
        assert!(playlist_creator_id(&json!({"owner_id":i64::MIN})).is_none());
    }

    #[test]
    fn optional_creator_lookup_uses_the_original_person_or_group_and_failure_cannot_block_playback()
    {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        let canceled = AtomicBool::new(false);
        let generation = client.home_owner(42, &canceled).unwrap();
        for (owner, expected_method, expected_key, response) in [
            (
                7_i64,
                "users.get",
                "user_ids",
                json!([{"id":7,"first_name":"First","last_name":"Last"}]),
            ),
            (
                -8_i64,
                "groups.getById",
                "group_ids",
                json!({"groups":[{"id":8,"name":"Community"}]}),
            ),
        ] {
            let metadata = json!({"owner_id":42,"original":{"owner_id":owner}});
            let creator = client
                .playlist_creator_request(42, generation, &metadata, &canceled, |method, params| {
                    assert_eq!(method, expected_method);
                    assert_eq!(params, vec![(expected_key, owner.abs().to_string())]);
                    Ok(response)
                })
                .unwrap()
                .unwrap();
            assert_eq!(creator.owner_id, owner);
        }
        assert!(
            client
                .playlist_creator_request(
                    42,
                    generation,
                    &json!({"owner_id":7}),
                    &canceled,
                    |_, _| Err(WebAudioError::Network)
                )
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            client.playlist_creator_request(
                42,
                generation,
                &json!({"owner_id":7}),
                &canceled,
                |_, _| Err(WebAudioError::WrongAccount)
            ),
            Err(WebAudioError::WrongAccount)
        ));
        canceled.store(true, Ordering::Release);
        assert!(matches!(
            client.playlist_creator_request(
                42,
                generation,
                &json!({"owner_id":7}),
                &canceled,
                |_, _| panic!("Canceled metadata lookup must not access the network")
            ),
            Err(WebAudioError::Canceled)
        ));
    }

    #[test]
    fn playlist_descriptions_preserve_multiline_text_and_strip_html_without_executing_it() {
        let reference = album();
        let value = json!({"id":3,"owner_id":-200,"title":"Playlist",
            "description":"<p>First &amp; second</p><p>Second<br/>third &#x2665;</p>Last &lt;3"});
        let page = parse_album_page(&value, &reference).unwrap();
        assert_eq!(
            page.description.as_deref(),
            Some("First & second\nSecond\nthird ♥\nLast <3")
        );
        let description = json!(
            "Before <b>bold</b> <a href=\"https://example.test/?q=>\">link</a><!-- don't show > this --><script>hidden()</script><style>hidden{}</style> after"
        );
        assert_eq!(
            playlist_description(Some(&description)).as_deref(),
            Some("Before bold link after")
        );
        assert_eq!(
            playlist_description(Some(&json!("One\rTwo\r\nThree\nFour"))).as_deref(),
            Some("One\nTwo\nThree\nFour")
        );
        assert_eq!(
            playlist_description(Some(&json!("I <3 music; 1 < 2"))).as_deref(),
            Some("I <3 music; 1 < 2")
        );
    }

    #[test]
    fn playlist_descriptions_are_optional_and_bounded_by_unicode_characters() {
        for value in [
            Value::Null,
            json!(true),
            json!(4),
            json!({}),
            json!([]),
            json!(" \n\t "),
            json!("<p></p>"),
            json!("<!-- only markup -->"),
        ] {
            assert!(playlist_description(Some(&value)).is_none());
        }
        assert!(playlist_description(None).is_none());
        let long = json!("я".repeat(20_000));
        let description = playlist_description(Some(&long)).unwrap();
        assert_eq!(description.chars().count(), 4096);
        let mut value = json!({"id":3,"owner_id":-200,"title":"Playlist"});
        assert!(
            parse_album_page(&value, &album())
                .unwrap()
                .description
                .is_none()
        );
        value["description"] = json!({"unexpected":"shape"});
        assert!(
            parse_album_page(&value, &album())
                .unwrap()
                .description
                .is_none()
        );
    }

    #[test]
    #[ignore = "requires a saved VK Fast session; prints metadata-presence flags only"]
    fn native_playlist_description_and_stats_readonly() {
        let (profile, cookies) = crate::music_session_store::restore_profile()
            .unwrap()
            .expect("saved session");
        let client = WebAudioClient::new(cookies).unwrap();
        client.verify_owner(profile.id).unwrap();
        let canceled = AtomicBool::new(false);
        let playlists = client.playlists(profile.id, &canceled).unwrap();
        let reference = &playlists.first().expect("saved playlist").album;
        let page = client.album_page(profile.id, reference, &canceled).unwrap();
        println!(
            "playlist page metadata flags: description={}, plays={}, created={}, updated={}, creator={}, tracks={}",
            page.description.is_some(),
            page.stats.plays.is_some(),
            page.stats.created_at.is_some(),
            page.stats.updated_at.is_some(),
            page.creator.is_some(),
            page.tracks.len()
        );
        assert_eq!(
            (page.album.owner_id, page.album.id),
            (reference.owner_id, reference.id)
        );
        assert!(
            page.creator.as_ref().is_some_and(
                |creator| !creator.name.is_empty() && creator.name.chars().count() <= 512
            )
        );
    }

    #[test]
    fn release_statistics_preserve_real_values_without_inventing_missing_fields() {
        let reference = album();
        let mut response = json!({"id":3,"owner_id":-200,"title":"Release","plays":861,
            "create_time":1785430260,"update_time":1785433860,"followers":9999});
        let page = parse_album_page(&response, &reference).unwrap();
        assert_eq!(
            page.stats,
            AlbumStats {
                plays: Some(861),
                created_at: Some(1_785_430_260),
                updated_at: Some(1_785_433_860),
            }
        );
        response["plays"] = json!(0);
        assert_eq!(
            parse_album_page(&response, &reference).unwrap().stats.plays,
            Some(0)
        );
        response.as_object_mut().unwrap().remove("plays");
        response.as_object_mut().unwrap().remove("create_time");
        response.as_object_mut().unwrap().remove("update_time");
        assert_eq!(
            parse_album_page(&response, &reference).unwrap().stats,
            AlbumStats::default()
        );
        response["plays"] = json!("861");
        response["create_time"] = json!("1785430260");
        assert_eq!(
            parse_album_page(&response, &reference).unwrap().stats.plays,
            Some(861)
        );
        assert_eq!(
            parse_album_page(&response, &reference)
                .unwrap()
                .stats
                .created_at,
            Some(1_785_430_260)
        );
    }

    #[test]
    fn release_statistics_ignore_invalid_numbers_and_non_second_timestamps() {
        for invalid in [
            json!(-1),
            json!(1.5),
            json!(true),
            json!({}),
            json!("unknown"),
            Value::Null,
        ] {
            let stats =
                album_stats(&json!({"plays":invalid,"create_time":invalid,"update_time":invalid}));
            assert_eq!(stats, AlbumStats::default());
        }
        for invalid_timestamp in [0, -1, 1_785_430_260_000_i64, i64::MAX] {
            let stats = album_stats(
                &json!({"create_time":invalid_timestamp,"update_time":invalid_timestamp}),
            );
            assert!(stats.created_at.is_none() && stats.updated_at.is_none());
        }
    }

    #[test]
    fn canceled_or_wrong_account_detail_requests_stop_before_any_network_or_library_mutation() {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        let canceled = AtomicBool::new(true);
        assert!(matches!(
            client.artist_page(42, &artist(), &canceled),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.album_page(42, &album(), &canceled),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.tracks_metadata(42, &[(42, 7)], &canceled),
            Err(WebAudioError::Canceled)
        ));
        canceled.store(false, Ordering::Release);
        assert!(matches!(
            client.artist_page(99, &artist(), &canceled),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.album_page(99, &album(), &canceled),
            Err(WebAudioError::WrongAccount)
        ));
        let state = client.state.lock().unwrap();
        assert!(state.cursor.is_none());
        assert!(state.streams.is_empty());
    }
}

#[cfg(test)]
mod page_verification_tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-page-verification".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        client
    }

    fn row() -> Value {
        json!({"owner_id":-200,"id":7,"artist":"Synthetic Artist","title":"Track","duration":180,"dmca_blocked":true})
    }

    #[test]
    fn exact_track_metadata_proves_existence_without_a_stream_and_only_empty_success_proves_absence()
     {
        let client = client();
        let canceled = AtomicBool::new(false);
        for (value, expected) in [(json!([row()]), Ok(true)), (json!([]), Ok(false))] {
            let result =
                client.verify_track_page_request(42, (-200, 7), &canceled, |_, method, params| {
                    assert_eq!(method, "audio.getById");
                    assert_eq!(params, vec![("audios", "-200_7".into())]);
                    Ok(value)
                });
            assert_eq!(result, expected);
        }
        for value in [
            Value::Null,
            json!({}),
            json!([{"owner_id":42,"id":7,"artist":"Other","title":"Track","duration":180}]),
            json!([{"owner_id":-200,"id":7}]),
            json!([row(), row()]),
            json!([null]),
        ] {
            assert_eq!(
                client.verify_track_page_request(42, (-200, 7), &canceled, |_, _, _| Ok(value)),
                Err(WebAudioError::Format)
            );
        }
        assert!(client.state.lock().unwrap().streams.is_empty());
        assert!(client.state.lock().unwrap().cursor.is_none());
    }

    #[test]
    fn album_verification_only_reads_keyless_metadata_and_preserves_unknown_malformed_responses() {
        let client = client();
        let canceled = AtomicBool::new(false);
        let album = json!({"owner_id":-200,"id":9,"title":"Album","access_key":"synthetic-private-response-key"});
        for value in [album.clone(), json!({"playlist":album})] {
            assert_eq!(
                client.verify_album_page_request(42, -200, 9, &canceled, |_, method, params| {
                    assert_eq!(method, "audio.getPlaylistById");
                    // Exact parameters prohibit access_key, extra_fields,
                    // creator lookups and any count/offset or audio.get walk.
                    assert_eq!(
                        params,
                        vec![("owner_id", "-200".into()), ("playlist_id", "9".into())]
                    );
                    Ok(value)
                }),
                Ok(true)
            );
        }
        for value in [
            Value::Null,
            json!({}),
            json!([]),
            json!({"playlist":null}),
            json!({"playlist":{}}),
            json!({"playlist":[]}),
        ] {
            assert_eq!(
                client.verify_album_page_request(42, -200, 9, &canceled, |_, _, _| Ok(value)),
                Ok(false)
            );
        }
        for value in [
            json!(false),
            json!(""),
            json!({"items":[]}),
            json!({"playlist":{"owner_id":42,"id":9,"title":"Wrong owner"}}),
            json!({"owner_id":-200,"id":8,"title":"Wrong album"}),
            json!({"owner_id":-200,"title":"Missing identity"}),
            json!({"playlist":"unexpected"}),
        ] {
            assert_eq!(
                client.verify_album_page_request(42, -200, 9, &canceled, |_, _, _| Ok(value)),
                Err(WebAudioError::Format)
            );
        }
        assert!(client.state.lock().unwrap().streams.is_empty());
        assert!(client.state.lock().unwrap().cursor.is_none());
    }

    #[test]
    fn verification_errors_remain_unknown_and_preflight_rejects_invalid_scope_without_requests() {
        let client = client();
        let canceled = AtomicBool::new(false);
        for error in [
            WebAudioError::Network,
            WebAudioError::Timeout,
            WebAudioError::NoStream,
            WebAudioError::Rejected,
            WebAudioError::Http(404),
            WebAudioError::Format,
        ] {
            assert_eq!(
                client.verify_track_page_request(42, (-200, 7), &canceled, |_, _, _| Err(
                    error.clone()
                )),
                Err(error.clone())
            );
            assert_eq!(
                client.verify_album_page_request(42, -200, 9, &canceled, |_, _, _| Err(
                    error.clone()
                )),
                Err(error)
            );
        }
        for key in [(0, 7), (-200, 0), (-200, -1)] {
            assert_eq!(
                client.verify_track_page_request(42, key, &canceled, |_, _, _| panic!(
                    "invalid track requested metadata"
                )),
                Err(WebAudioError::Format)
            );
        }
        for key in [(0, 9), (-200, 0), (-200, -1)] {
            assert_eq!(
                client.verify_album_page_request(42, key.0, key.1, &canceled, |_, _, _| panic!(
                    "invalid album requested metadata"
                )),
                Err(WebAudioError::Format)
            );
        }
        canceled.store(true, Ordering::Release);
        assert_eq!(
            client.verify_track_page(42, (-200, 7), &canceled),
            Err(WebAudioError::Canceled)
        );
        assert_eq!(
            client.verify_album_page(42, -200, 9, &canceled),
            Err(WebAudioError::Canceled)
        );
        canceled.store(false, Ordering::Release);
        assert_eq!(
            client.verify_track_page(99, (-200, 7), &canceled),
            Err(WebAudioError::WrongAccount)
        );
        assert_eq!(
            client.verify_album_page(99, -200, 9, &canceled),
            Err(WebAudioError::WrongAccount)
        );
        assert!(client.state.lock().unwrap().streams.is_empty());
    }

    #[test]
    fn late_cancellation_and_account_generation_changes_cannot_confirm_or_remove_links() {
        for album in [false, true] {
            for change in 0..3 {
                let client = client();
                let canceled = AtomicBool::new(false);
                let request = |_, _, _| {
                    match change {
                        0 => canceled.store(true, Ordering::Release),
                        1 => client.accept_profile(99).unwrap(),
                        _ => client.state.lock().unwrap().generation += 1,
                    }
                    // Even successful absence must not overwrite a new scope.
                    Ok(if album { Value::Null } else { json!([]) })
                };
                let result = if album {
                    client.verify_album_page_request(42, -200, 9, &canceled, request)
                } else {
                    client.verify_track_page_request(42, (-200, 7), &canceled, request)
                };
                assert_eq!(
                    result,
                    Err(if change == 0 {
                        WebAudioError::Canceled
                    } else {
                        WebAudioError::Unverified
                    })
                );
                assert!(client.state.lock().unwrap().streams.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use serde_json::json;

    fn track(id: i64) -> Value {
        json!({"owner_id":42,"id":id,"artist":"Artist","title":"Song","duration":120,"url":"https://cdn.example.test/audio.mp3","subtitle":"Slowed"})
    }

    #[test]
    fn popular_queries_follow_only_the_explicit_popular_block_and_keep_vk_order() {
        let value = json!({"catalog":{"default_section":"search","sections":[{"id":"search","blocks":[
            {"data_type":"search_suggestions","title":"История поиска","suggestions_ids":["private-history"]},
            {"data_type":"search_suggestions","title":"Популярные запросы","suggestions_ids":["two","one","duplicate"]}
        ]}]},"suggestions":[
            {"id":"one","title":"Artist &amp; Guest"}, {"id":"two","title":"Second"},
            {"id":"duplicate","title":"SECOND"}, {"id":"private-history","title":"Never show as popular"},
            {"id":"auxiliary","title":"Never show unreferenced"}
        ]});
        assert_eq!(
            parse_popular_queries(&value).unwrap(),
            ["Second", "Artist & Guest"]
        );
        let mut missing = value.clone();
        missing["suggestions"] = json!([]);
        assert!(matches!(
            parse_popular_queries(&missing),
            Err(WebAudioError::Format)
        ));
        let mut only_history = value;
        only_history["catalog"]["sections"][0]["blocks"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(matches!(
            parse_popular_queries(&only_history),
            Err(WebAudioError::Format)
        ));
    }

    #[test]
    fn native_search_hints_are_bounded_deduplicated_and_do_not_invent_completions() {
        assert_eq!(
            parse_query_hints(&json!({"suggestions":["nort","NORT","  nort &amp; guest  ",""]}))
                .unwrap(),
            ["nort", "nort & guest"]
        );
        assert!(parse_query_hints(&json!({"suggestions":[{"unrecognized":"shape"}]})).is_err());
        assert!(parse_query_hints(&json!({"suggestions":["bad\nquery"]})).is_err());
        assert!(matches!(
            parse_query_hints(&json!({"suggestions":vec!["query";101]})),
            Err(WebAudioError::TooLarge)
        ));
        assert!(
            parse_query_hints(&json!({"suggestions":[]}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn search_offsets_count_server_rows_while_tracks_deduplicate_and_urls_stay_private() {
        let value = json!({"count":150,"items":[track(2),track(2),track(1)]});
        let (page, streams) = parse_search_tracks(&value, 100).unwrap();
        assert_eq!(
            page.tracks.iter().map(Track::key).collect::<Vec<_>>(),
            [(42, 2), (42, 1)]
        );
        assert_eq!(page.next_offset, 103);
        assert!(page.has_more);
        assert_eq!(streams.len(), 2);
        assert!(page.tracks.iter().all(|track| track.url.is_none()));
        assert_eq!(page.tracks[0].subtitle.as_deref(), Some("Slowed"));
        let (empty, _) = parse_search_tracks(&json!({"count":150,"items":[]}), 100).unwrap();
        assert!(!empty.has_more);
        assert_eq!(empty.next_offset, 100);
        let (last, _) = parse_search_tracks(&json!({"count":101,"items":[track(1)]}), 100).unwrap();
        assert!(!last.has_more);
        assert!(matches!(
            parse_search_tracks(&json!({"count":1,"items":vec![track(1);101]}), 0),
            Err(WebAudioError::TooLarge)
        ));
    }

    #[test]
    fn search_metadata_preserves_canonical_artist_album_identity_and_server_order() {
        let artists = parse_search_artists(&json!({"items":[{"id":"duo","name":"A & B"},{"id":"solo","name":"Other"},{"id":"duo","name":"A & B"}]})).unwrap();
        assert_eq!(artists.len(), 2);
        assert_eq!(artists[0].name, "A & B");
        let albums = parse_search_albums(&json!({"items":[{"id":3,"owner_id":-42,"title":"Official","access_key":"synthetic"},{"id":3,"owner_id":-42,"title":"Duplicate"}]})).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!((albums[0].owner_id, albums[0].id), (-42, 3));
        assert_eq!(albums[0].access_key.as_deref(), Some("synthetic"));
    }

    #[test]
    fn search_playlists_keep_server_order_original_identity_and_optional_metadata() {
        let value = json!({"count":3,"items":[
            {"owner_id":-42,"id":7,"title":"Mix &amp; guests","description":"  A &amp; B\ncollection ","count":53,"access_key":"synthetic-private-key",
             "original":{"owner_id":99,"playlist_id":100},"photo":{"photo_300":"https://cdn.example.test/cover.jpg"}},
            {"owner_id":-42,"id":7,"title":"Duplicate"},
            {"owner_id":99,"id":7,"title":"Same id, different owner"}
        ]});
        let playlists = parse_search_playlists(&value).unwrap();
        assert_eq!(
            playlists
                .iter()
                .map(|playlist| (playlist.album.owner_id, playlist.album.id))
                .collect::<Vec<_>>(),
            [(-42, 7), (99, 7)]
        );
        assert_eq!(playlists[0].album.title, "Mix & guests");
        assert_eq!(playlists[0].subtitle, "A & B collection");
        assert_eq!(playlists[0].track_count, Some(53));
        assert_eq!(
            playlists[0].album.access_key.as_deref(),
            Some("synthetic-private-key")
        );
        assert_eq!(
            playlists[0].album.cover_url.as_deref(),
            Some("https://cdn.example.test/cover.jpg")
        );
        assert!(playlists[1].subtitle.is_empty());
        assert_eq!(playlists[1].track_count, None);
        assert!(playlists[1].album.cover_url.is_none());
    }

    #[test]
    fn search_playlist_empty_results_are_valid_but_malformed_or_oversized_lists_fail() {
        assert!(
            parse_search_playlists(&json!({"count":0,"items":[]}))
                .unwrap()
                .is_empty()
        );
        for value in [
            json!({}),
            json!({"items":{}}),
            json!({"count":"bad","items":[]}),
            json!({"items":[{"owner_id":0,"id":7,"title":"Invalid"}]}),
            json!({"items":[{"owner_id":-42,"id":0,"title":"Invalid"}]}),
            json!({"items":[{"owner_id":-42,"id":7,"title":""}]}),
        ] {
            assert!(matches!(
                parse_search_playlists(&value),
                Err(WebAudioError::Format)
            ));
        }
        assert!(matches!(
            parse_search_playlists(
                &json!({"items":vec![json!({"owner_id":42,"id":1,"title":"Too many"});101]})
            ),
            Err(WebAudioError::TooLarge)
        ));
        let rows: Vec<_> = (1..=25)
            .map(|id| json!({"owner_id":-42,"id":id,"title":format!("Playlist {id}")}))
            .collect();
        let playlists = parse_search_playlists(&json!({"items":rows})).unwrap();
        assert_eq!(playlists.len(), 20);
        assert_eq!(playlists.last().unwrap().album.id, 20);
    }

    #[test]
    fn first_search_fetches_playlist_metadata_once_and_keeps_the_library_cursor() {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        client.state.lock().unwrap().cursor = Some(Cursor {
            owner: 42,
            offset: 17,
            next: Some("library-cursor".into()),
            ..Default::default()
        });
        let canceled = AtomicBool::new(false);
        let mut calls = Vec::new();
        let first=client.search_request(42,"  query  ",0,&canceled,|_,method,params|{
            calls.push(method);
            assert!(params.contains(&("q","query".into())));
            assert!(params.contains(&("offset","0".into())));
            match method {
                "audio.search"=>Ok(json!({"count":101,"items":[track(1)]})),
                "audio.searchArtists"|"audio.searchAlbums"=>Ok(json!({"items":[]})),
                "audio.searchPlaylists"=>{
                    assert!(params.contains(&("count","20".into())));
                    Ok(json!({"items":[{"owner_id":-77,"id":9,"title":"Actual playlist","access_key":"synthetic"}]}))
                },
                _=>panic!("Unexpected search method"),
            }
        }).unwrap();
        assert_eq!(
            calls,
            [
                "audio.search",
                "audio.searchArtists",
                "audio.searchAlbums",
                "audio.searchPlaylists"
            ]
        );
        assert_eq!(first.playlists.len(), 1);
        assert_eq!(
            (
                first.playlists[0].album.owner_id,
                first.playlists[0].album.id
            ),
            (-77, 9)
        );
        let mut next_calls = 0;
        let next = client
            .search_request(42, "query", 100, &canceled, |_, method, params| {
                next_calls += 1;
                assert_eq!(method, "audio.search");
                assert!(params.contains(&("offset", "100".into())));
                Ok(json!({"count":101,"items":[track(2)]}))
            })
            .unwrap();
        assert_eq!(next_calls, 1);
        assert!(next.playlists.is_empty());
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
        assert_eq!(
            state.cursor.as_ref().unwrap().next.as_deref(),
            Some("library-cursor")
        );
        assert!(state.mix.is_none());
    }

    #[test]
    fn failed_or_stale_playlist_search_cannot_commit_the_partial_track_response() {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        let canceled = AtomicBool::new(false);
        for stale in [false, true] {
            let result =
                client.search_request(42, "query", 0, &canceled, |_, method, _| match method {
                    "audio.search" => Ok(json!({"count":1,"items":[track(1)]})),
                    "audio.searchPlaylists" => {
                        if stale {
                            client.state.lock().unwrap().generation += 1;
                            Ok(json!({"items":[]}))
                        } else {
                            Err(WebAudioError::Network)
                        }
                    }
                    _ => Ok(json!({"items":[]})),
                });
            assert!(matches!(
                result,
                Err(WebAudioError::Network | WebAudioError::Unverified)
            ));
            assert!(client.state.lock().unwrap().streams.is_empty());
        }
    }

    #[test]
    fn search_checks_cancellation_and_account_before_http_without_touching_library_cursor() {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        let canceled = AtomicBool::new(true);
        assert!(matches!(
            client.search_popular(42, &canceled),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.search_hints(42, "query", &canceled),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.search(42, "query", 0, &canceled),
            Err(WebAudioError::Canceled)
        ));
        canceled.store(false, Ordering::Release);
        assert!(matches!(
            client.search_popular(99, &canceled),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.search_hints(99, "query", &canceled),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.search(99, "query", 0, &canceled),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.search(42, "query", MAX_QUEUED_TRACKS, &canceled),
            Err(WebAudioError::TooLarge)
        ));
        assert!(matches!(
            client.search_hints(42, "\n", &canceled),
            Err(WebAudioError::Format)
        ));
        let state = client.state.lock().unwrap();
        assert!(state.cursor.is_none());
        assert!(state.streams.is_empty());
    }
}

#[cfg(test)]
mod playlist_action_tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "requires a saved VK Fast session and only reads native playlist permissions"]
    fn native_playlist_edit_permissions_readonly() {
        let (profile, cookies) = crate::music_session_store::restore_profile()
            .unwrap()
            .unwrap();
        let client = WebAudioClient::new(cookies).unwrap();
        client.verify_owner(profile.id).unwrap();
        let canceled = AtomicBool::new(false);
        let generation = client.home_owner(profile.id, &canceled).unwrap();
        let response = client
            .detail_request(
                profile.id,
                generation,
                "audio.getPlaylists",
                vec![
                    ("owner_id", profile.id.to_string()),
                    ("count", "100".into()),
                    ("offset", "0".into()),
                ],
                &canceled,
            )
            .unwrap();
        let rows = response.get("items").and_then(Value::as_array).unwrap();
        println!(
            "playlist permissions: rows={}, permissions_present={}, edit_true={}, edit_false={}, following={}, original={}, own={}, albums={}",
            rows.len(),
            rows.iter()
                .filter(|row| row.get("permissions").is_some())
                .count(),
            rows.iter()
                .filter(|row| row["permissions"]["edit"] == true)
                .count(),
            rows.iter()
                .filter(|row| row["permissions"]["edit"] == false)
                .count(),
            rows.iter()
                .filter(|row| row["is_following"] == true)
                .count(),
            rows.iter()
                .filter(|row| row.get("original").is_some())
                .count(),
            rows.iter()
                .filter(|row| row["owner_id"].as_i64() == Some(profile.id))
                .count(),
            rows.iter().filter(|row| row["type"] == "album").count()
        );
    }

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        {
            let mut state = client.state.lock().unwrap();
            state.cursor = Some(Cursor {
                owner: 42,
                offset: 17,
                next: Some("existing-catalog-cursor".into()),
                ..Default::default()
            });
            state.mix = Some(MixSession {
                token: Zeroizing::new("synthetic-native-token".into()),
                expires: now() + 3600,
                api_host: "web.api.vk.ru",
                previous_batch: vec![(-200, 1)],
                previous_source: MixSource::Personal,
            });
            state.streams.insert(
                (-200, 7),
                StreamInfo {
                    access_key: Some(Zeroizing::new("source-access".into())),
                    original_url: Some(Zeroizing::new(
                        "https://audio.example.test/source.mp3".into(),
                    )),
                    ..Default::default()
                },
            );
        }
        client
    }

    fn playlist(owner: i64) -> AlbumRef {
        AlbumRef {
            owner_id: owner,
            id: 10,
            title: "Editable".into(),
            access_key: Some("playlist-access".into()),
            cover_url: None,
        }
    }

    fn editable(owner: i64) -> Value {
        json!({"owner_id":owner,"id":10,"title":"Editable","type":"playlist","permissions":{"edit":true}})
    }

    #[test]
    fn playlist_edit_permission_never_follows_local_owner_or_saved_copy_identity() {
        let cases = [
            (editable(42), true),
            (editable(-200), true),
            (
                json!({"owner_id":42,"id":10,"title":"Missing permission"}),
                false,
            ),
            (
                json!({"owner_id":42,"id":10,"title":"Following","permissions":{"edit":true},"is_following":true,
                "original":{"owner_id":-9,"playlist_id":2}}),
                false,
            ),
            (
                json!({"owner_id":42,"id":10,"title":"Album","type":"album","permissions":{"edit":true}}),
                false,
            ),
            (
                json!({"owner_id":42,"id":10,"title":"Album","type":1,"permissions":{"edit":true}}),
                false,
            ),
            (
                json!({"owner_id":42,"id":-10,"title":"Generated","permissions":{"edit":true}}),
                false,
            ),
            (
                json!({"owner_id":42,"id":10,"title":"String permission","permissions":{"edit":"true"}}),
                false,
            ),
        ];
        // Independent rows check the field while retaining read-only rows for
        // the sidebar/search rather than filtering those public lists away.
        for (row, expected) in cases {
            assert_eq!(playlist_can_add_tracks(&row), expected);
            let parsed = parse_user_playlists(&json!({"items":[row]})).unwrap();
            assert_eq!(parsed.items.len(), 1);
            assert_eq!(parsed.items[0].can_add_tracks, expected);
        }
    }

    #[test]
    fn playlist_add_confirms_permission_sends_source_access_identity_and_retains_new_owner() {
        for target_owner in [42, -50] {
            let client = client();
            let target = playlist(target_owner);
            let mut calls = Vec::new();
            let owned = client
                .add_to_playlist_request(
                    42,
                    &target,
                    (-200, 7),
                    &AtomicBool::new(false),
                    |_, method, params| {
                        calls.push(method);
                        if method == "audio.getPlaylistById" {
                            assert_eq!(
                                params,
                                vec![
                                    ("owner_id", target_owner.to_string()),
                                    ("playlist_id", "10".into()),
                                    ("access_key", "playlist-access".into())
                                ]
                            );
                            Ok(json!({"playlist":editable(target_owner)}))
                        } else {
                            assert_eq!(method, "audio.addToPlaylist");
                            assert_eq!(
                                params,
                                vec![
                                    ("owner_id", target_owner.to_string()),
                                    ("playlist_id", "10".into()),
                                    ("audio_ids", "-200_7_source-access".into())
                                ]
                            );
                            Ok(json!([{"audio_id":901,"track_code":"new-tracking"}]))
                        }
                    },
                )
                .unwrap();
            assert_eq!(calls, ["audio.getPlaylistById", "audio.addToPlaylist"]);
            assert_eq!(owned, (target_owner, 901));
            let state = client.state.lock().unwrap();
            assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
            assert_eq!(
                state.cursor.as_ref().unwrap().next.as_deref(),
                Some("existing-catalog-cursor")
            );
            assert_eq!(state.mix.as_ref().unwrap().previous_batch, vec![(-200, 1)]);
            let new = state.streams.get(&owned).unwrap();
            assert_eq!(new.track_code.as_deref(), Some("new-tracking"));
            assert!(
                new.original_url.is_none() && new.reload_id.is_none() && new.access_key.is_none(),
                "A new owned audio must not inherit the source's credentials or reload identity"
            );
        }
    }

    #[test]
    fn stale_or_read_only_playlist_metadata_is_rejected_before_any_write() {
        let client = client();
        let target = playlist(42);
        for metadata in [
            json!({"owner_id":42,"id":10,"title":"Saved","permissions":{"edit":false}}),
            json!({"owner_id":42,"id":10,"title":"Saved","permissions":{"edit":true},"is_following":true}),
            editable(99),
        ] {
            let mut calls = 0;
            let result = client.add_to_playlist_request(
                42,
                &target,
                (-200, 7),
                &AtomicBool::new(false),
                |_, method, _| {
                    calls += 1;
                    assert_eq!(method, "audio.getPlaylistById");
                    Ok(metadata.clone())
                },
            );
            assert!(matches!(
                result,
                Err(WebAudioError::Rejected | WebAudioError::Format)
            ));
            assert_eq!(calls, 1);
        }
        assert!(
            !client
                .state
                .lock()
                .unwrap()
                .streams
                .contains_key(&(42, 901))
        );
    }

    #[test]
    fn playlist_add_rejects_invalid_account_cancel_or_generation_before_mutation() {
        let client = client();
        let target = playlist(42);
        assert!(matches!(
            client.add_to_playlist_request(
                99,
                &target,
                (-200, 7),
                &AtomicBool::new(false),
                |_, _, _| panic!("wrong account reached request")
            ),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.add_to_playlist_request(
                42,
                &target,
                (-200, 7),
                &AtomicBool::new(true),
                |_, _, _| panic!("canceled request reached API")
            ),
            Err(WebAudioError::Canceled)
        ));
        let canceled = AtomicBool::new(false);
        let mut calls = 0;
        assert!(matches!(
            client.add_to_playlist_request(42, &target, (-200, 7), &canceled, |_, method, _| {
                calls += 1;
                assert_eq!(method, "audio.getPlaylistById");
                canceled.store(true, Ordering::Release);
                Ok(editable(42))
            }),
            Err(WebAudioError::Canceled)
        ));
        assert_eq!(calls, 1);
        let stale = super::playlist_action_tests::client();
        let mut calls = 0;
        assert!(matches!(
            stale.add_to_playlist_request(
                42,
                &target,
                (-200, 7),
                &AtomicBool::new(false),
                |_, method, _| {
                    calls += 1;
                    assert_eq!(method, "audio.getPlaylistById");
                    stale.accept_profile(42).unwrap();
                    Ok(editable(42))
                }
            ),
            Err(WebAudioError::Unverified)
        ));
        assert_eq!(calls, 1);
    }

    #[test]
    fn playlist_add_never_retries_failures_and_requires_a_confirmed_new_audio_id() {
        let client = client();
        let target = playlist(42);
        let mut calls = 0;
        assert!(matches!(
            client.add_to_playlist_request(
                42,
                &target,
                (-200, 7),
                &AtomicBool::new(false),
                |_, method, _| {
                    calls += 1;
                    if method == "audio.getPlaylistById" {
                        Ok(editable(42))
                    } else {
                        Err(WebAudioError::Network)
                    }
                }
            ),
            Err(WebAudioError::Network)
        ));
        assert_eq!(calls, 2);
        for value in [
            json!(true),
            json!(1),
            json!([]),
            json!([{"audio_id":0}]),
            json!([{"audio_id":9},{"audio_id":10}]),
            json!([{"audio_id":9,"owner_id":99}]),
        ] {
            assert!(matches!(
                parse_playlist_add(&value, 42),
                Err(WebAudioError::Format)
            ));
        }
        assert_eq!(
            parse_playlist_add(&json!([{"audio_id":"901"}]), 42).unwrap(),
            ((42, 901), None)
        );
        assert!(
            !client
                .state
                .lock()
                .unwrap()
                .streams
                .contains_key(&(42, 901))
        );
    }

    #[test]
    fn playlist_add_cannot_append_multiple_audio_ids_through_an_access_key() {
        let client = client();
        client
            .state
            .lock()
            .unwrap()
            .streams
            .get_mut(&(-200, 7))
            .unwrap()
            .access_key = Some(Zeroizing::new("key,99_123".into()));
        let mut calls = 0;
        assert!(matches!(
            client.add_to_playlist_request(
                42,
                &playlist(42),
                (-200, 7),
                &AtomicBool::new(false),
                |_, method, _| {
                    calls += 1;
                    assert_eq!(method, "audio.getPlaylistById");
                    Ok(editable(42))
                }
            ),
            Err(WebAudioError::Format)
        ));
        assert_eq!(calls, 1);
    }
}

#[cfg(test)]
mod playlist_creation_tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-create-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        client
    }

    fn created() -> Value {
        json!({"owner_id":42,"id":71,"title":"My &amp; music","description":"","count":0,"permissions":{"edit":true}})
    }

    #[test]
    fn creation_sends_one_native_empty_playlist_request_and_keeps_library_and_radio_state() {
        let client = client();
        let generation = {
            let mut state = client.state.lock().unwrap();
            state.cursor = Some(Cursor {
                owner: 42,
                offset: 17,
                next: Some("existing-cursor".into()),
                ..Default::default()
            });
            state.streams.insert(
                (-9, 7),
                StreamInfo {
                    original_url: Some(Zeroizing::new(
                        "https://audio.example.test/song.mp3".into(),
                    )),
                    ..Default::default()
                },
            );
            state.mix = Some(MixSession {
                token: Zeroizing::new("synthetic-radio-token".into()),
                expires: now() + 3600,
                api_host: "web.api.vk.ru",
                previous_batch: vec![(-9, 7)],
                previous_source: MixSource::Personal,
            });
            state.generation
        };
        let mut calls = 0;
        let playlist = client
            .create_playlist_request(
                42,
                "  My & music  ",
                &AtomicBool::new(false),
                |returned_generation, method, params| {
                    calls += 1;
                    assert_eq!(returned_generation, generation);
                    assert_eq!(method, "audio.createPlaylist");
                    assert_eq!(
                        params,
                        [
                            ("owner_id", "42".into()),
                            ("title", "My & music".into()),
                            ("description", String::new()),
                            ("audio_ids", String::new())
                        ]
                    );
                    Ok(created())
                },
            )
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!((playlist.album.owner_id, playlist.album.id), (42, 71));
        assert_eq!(playlist.album.title, "My & music");
        assert_eq!(playlist.track_count, Some(0));
        assert!(playlist.can_add_tracks);
        assert!(playlist.subtitle.is_empty());
        let state = client.state.lock().unwrap();
        assert_eq!(state.generation, generation);
        assert_eq!(state.cursor.as_ref().unwrap().offset, 17);
        assert_eq!(
            state.cursor.as_ref().unwrap().next.as_deref(),
            Some("existing-cursor")
        );
        assert_eq!(state.streams.len(), 1);
        assert_eq!(state.mix.as_ref().unwrap().previous_batch, [(-9, 7)]);
    }

    #[test]
    fn creation_validates_names_before_dispatch_and_counts_unicode_characters() {
        let client = client();
        for title in [
            String::new(),
            " \t\n ".into(),
            "Name\ninside".into(),
            "nul\0name".into(),
            "a".repeat(PLAYLIST_TITLE_MAX_CHARS + 1),
        ] {
            assert!(matches!(
                client.create_playlist_request(
                    42,
                    &title,
                    &AtomicBool::new(false),
                    |_, _, _| panic!("Invalid name reached API")
                ),
                Err(WebAudioError::Format)
            ));
        }
        let title = "Я".repeat(PLAYLIST_TITLE_MAX_CHARS);
        let playlist = client
            .create_playlist_request(42, &title, &AtomicBool::new(false), |_, _, params| {
                assert_eq!(params[1].1, title);
                Ok(json!({"owner_id":42,"id":71,"title":title,"count":0}))
            })
            .unwrap();
        assert_eq!(
            playlist.album.title.chars().count(),
            PLAYLIST_TITLE_MAX_CHARS
        );
        assert!(
            !playlist.can_add_tracks,
            "Missing native permission must wait for the sidebar refresh"
        );
    }

    #[test]
    fn creation_rejects_wrong_owner_and_cancellation_without_dispatching() {
        let client = client();
        for owner in [0, -42, 43] {
            assert!(matches!(
                client.create_playlist_request(
                    owner,
                    "Playlist",
                    &AtomicBool::new(false),
                    |_, _, _| panic!("Wrong account reached API")
                ),
                Err(WebAudioError::WrongAccount)
            ));
        }
        assert!(matches!(
            client.create_playlist_request(
                42,
                "Playlist",
                &AtomicBool::new(true),
                |_, _, _| panic!("Canceled request reached API")
            ),
            Err(WebAudioError::Canceled)
        ));
    }

    #[test]
    fn creation_discards_canceled_or_stale_generation_replies() {
        let client = client();
        let canceled = AtomicBool::new(false);
        assert!(matches!(
            client.create_playlist_request(42, "Playlist", &canceled, |_, _, _| {
                canceled.store(true, Ordering::Release);
                Ok(created())
            }),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.create_playlist_request(42, "Playlist", &AtomicBool::new(false), |_, _, _| {
                client.accept_profile(42).unwrap();
                Ok(created())
            }),
            Err(WebAudioError::Unverified)
        ));
    }

    #[test]
    fn creation_requires_direct_confirmed_account_playlist_metadata_and_native_permission() {
        let client = client();
        for response in [
            json!(1),
            json!(null),
            json!({"playlist":created()}),
            json!({"owner_id":42,"id":0,"title":"Invalid"}),
            json!({"owner_id":42,"id":-71,"title":"Generated"}),
            json!({"owner_id":42,"id":71}),
            json!({"owner_id":42,"id":71,"title":"   "}),
        ] {
            assert!(matches!(
                client.create_playlist_request(
                    42,
                    "Playlist",
                    &AtomicBool::new(false),
                    |_, _, _| Ok(response.clone())
                ),
                Err(WebAudioError::Format)
            ));
        }
        for owner in [0, -42, 43] {
            assert!(matches!(
                client.create_playlist_request(
                    42,
                    "Playlist",
                    &AtomicBool::new(false),
                    |_, _, _| Ok(json!({"owner_id":owner,"id":71,"title":"Other owner"}))
                ),
                Err(WebAudioError::WrongAccount)
            ));
        }
        let playlist = client
            .create_playlist_request(42, "Playlist", &AtomicBool::new(false), |_, _, _| {
                Ok(json!({"owner_id":42,"id":71,"title":"Playlist","permissions":{"edit":false}}))
            })
            .unwrap();
        assert!(!playlist.can_add_tracks);
    }

    #[test]
    fn creation_never_retries_an_ambiguous_or_rejected_mutation() {
        let client = client();
        for error in [
            WebAudioError::Network,
            WebAudioError::Timeout,
            WebAudioError::Rejected,
            WebAudioError::Expired,
        ] {
            let mut calls = 0;
            let returned = client.create_playlist_request(
                42,
                "Playlist",
                &AtomicBool::new(false),
                |_, _, _| {
                    calls += 1;
                    Err(error.clone())
                },
            );
            assert!(matches!(returned, Err(actual) if actual == error));
            assert_eq!(calls, 1);
        }
    }
}

#[cfg(test)]
mod reported_track_count_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_items_without_zero_count_do_not_establish_an_empty_playlist() {
        let reference = AlbumRef {
            id: 71,
            owner_id: 42,
            title: "Fixture".into(),
            access_key: None,
            cover_url: None,
        };
        for count in [Some(0), Some(7), None] {
            let mut source = json!({"id":71,"owner_id":42,"title":"Fixture"});
            if let Some(count) = count {
                source["count"] = json!(count);
            }
            let mut page = parse_album_page(&source, &reference).unwrap();
            assert_eq!(page.confirmed_empty(), count == Some(0));
            page.observe_track_count(Some(0));
            assert_eq!(page.confirmed_empty(), count != Some(7));
            if count == Some(7) {
                assert_eq!(page.total_tracks, Some(7));
            }
        }
    }
}
