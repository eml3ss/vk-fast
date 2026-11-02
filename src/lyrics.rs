//! Musixmatch lyrics. Requests contain track metadata, never VK credentials.
use crate::{i18n::Language, model::Track};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::Value;
use std::{io::Read, sync::Arc, time::Duration};
use zeroize::Zeroizing;

#[path = "lyrics_service.rs"]
mod service;
#[path = "lyrics_store.rs"]
mod store;
pub use service::{ConfigAction, Event, Service};
pub use store::{Mode, Status};

const API: &str = "https://api.musixmatch.com/ws/1.1/";
const ANONYMOUS_API: &str = "https://apic-appmobile.musixmatch.com/ws/1.1/";
const APP_ID: &str = "mac-ios-v2.0";
const MAX_RESPONSE: u64 = 2 * 1024 * 1024;
const MAX_TEXT: usize = 256 * 1024;
const MAX_LINES: usize = 4000;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Query {
    pub key: (i64, i64),
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32,
    speed_variant: bool,
}
impl Query {
    pub fn from_track(track: &Track) -> Self {
        let title = track.display_title().into_owned();
        let lower = title.to_lowercase();
        Self {
            key: track.key(),
            title,
            artist: track
                .artists
                .iter()
                .find(|artist| !artist.name.trim().is_empty())
                .map_or_else(|| track.artist.clone(), |artist| artist.name.clone()),
            album: track
                .album
                .as_ref()
                .map_or_else(String::new, |album| album.title.clone()),
            duration: track.duration,
            speed_variant: ["slowed", "sped up", "nightcore", "замедлен", "ускорен"]
                .iter()
                .any(|word| lower.contains(word)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub at: Option<f64>,
    pub text: String,
}

#[derive(Clone)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    pub synced: bool,
    pub instrumental: bool,
    pub copyright: String,
    pub source_url: Option<String>,
    tracking_url: Option<String>,
}
impl std::fmt::Debug for Lyrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lyrics")
            .field("line_count", &self.lines.len())
            .field("synced", &self.synced)
            .field("instrumental", &self.instrumental)
            .finish_non_exhaustive()
    }
}
impl Lyrics {
    pub fn active_at(&self, seconds: f64) -> Option<usize> {
        if !self.synced || !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        self.lines
            .partition_point(|line| line.at.is_some_and(|time| time <= seconds))
            .checked_sub(1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    MissingKey,
    Auth,
    Restricted,
    Captcha,
    RateLimited(u64),
    NotFound,
    WrongLyrics,
    Network,
    Format,
    Storage,
    Canceled,
}
impl Error {
    pub fn message(self, language: Language) -> &'static str {
        let t = |ru, en| language.text(ru, en);
        match self {
            Self::MissingKey => t(
                "Добавьте API-ключ Musixmatch или выберите анонимный режим.",
                "Add a Musixmatch API key or choose anonymous mode.",
            ),
            Self::Auth => t(
                "Musixmatch отклонил ключ или токен. Проверьте подключение в настройках.",
                "Musixmatch rejected the key or token. Check the connection in Settings.",
            ),
            Self::Restricted => t(
                "Musixmatch ограничил доступ к тексту этого трека.",
                "Musixmatch has restricted these lyrics.",
            ),
            Self::Captcha => t(
                "Musixmatch запросил проверку. Попробуйте позже или используйте свой API-ключ.",
                "Musixmatch requires verification. Try later or use your API key.",
            ),
            Self::RateLimited(_) => t(
                "Достигнут лимит Musixmatch. Повторите запрос позже.",
                "Musixmatch's request limit was reached. Try again later.",
            ),
            Self::NotFound => t(
                "Текст этого трека не найден.",
                "Lyrics for this track were not found.",
            ),
            Self::WrongLyrics => t(
                "Musixmatch вернул неподходящий текст. Попробуйте позже.",
                "Musixmatch returned incorrect lyrics. Try again later.",
            ),
            Self::Network => t(
                "Не удалось связаться с Musixmatch.",
                "Could not connect to Musixmatch.",
            ),
            Self::Format => t(
                "Не удалось прочитать ответ Musixmatch.",
                "Could not read the Musixmatch response.",
            ),
            Self::Storage => t(
                "Не удалось сохранить подключение Musixmatch.",
                "Could not save the Musixmatch connection.",
            ),
            Self::Canceled => t("Запрос отменён.", "Request canceled."),
        }
    }
}

struct Provider {
    http: Client,
}
impl Provider {
    fn record_display(&self, lyrics: &Lyrics, canceled: &impl Fn() -> bool) {
        if cfg!(test) || canceled() {
            return;
        }
        if let Some(url) = &lyrics.tracking_url {
            let _ = self.http.get(url).timeout(Duration::from_secs(2)).send();
        }
    }
    fn new() -> Result<Self, Error> {
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(12))
                .connect_timeout(Duration::from_secs(6))
                .redirect(Policy::none())
                .user_agent("VK Fast/0.1.0 (+https://github.com/eml3ss/vk-fast)")
                .build()
                .map_err(|_| Error::Network)?,
        })
    }

    fn request(
        &self,
        base: &str,
        method: &'static str,
        params: &[(&str, &str)],
        canceled: &impl Fn() -> bool,
    ) -> Result<Value, Error> {
        if canceled() {
            return Err(Error::Canceled);
        }
        // Test fixtures exercise the parser and worker without real services.
        if cfg!(test) {
            return Err(Error::NotFound);
        }
        let response = self
            .http
            .get(format!("{base}{method}"))
            .query(params)
            .header("Accept", "application/json")
            .send()
            .map_err(|_| Error::Network)?;
        let http = response.status().as_u16();
        let wait = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(300)
            .clamp(30, 86400);
        if http == 429 {
            return Err(Error::RateLimited(wait));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE)
        {
            return Err(Error::Format);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        response
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Network)?;
        if canceled() {
            return Err(Error::Canceled);
        }
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err(Error::Format);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| match http {
            401 => Error::Auth,
            403 => Error::Captcha,
            _ => Error::Format,
        })?;
        if !(200..300).contains(&http) {
            return Err(
                if value
                    .pointer("/message/header/hint")
                    .and_then(Value::as_str)
                    == Some("captcha")
                {
                    Error::Captcha
                } else {
                    status_error(http, wait)
                },
            );
        }
        body(value)
    }

    fn token(&self, canceled: &impl Fn() -> bool) -> Result<Zeroizing<String>, Error> {
        let mut data = self.request(
            ANONYMOUS_API,
            "token.get",
            &[("app_id", APP_ID), ("format", "json")],
            canceled,
        )?;
        let Value::String(token) = data
            .get_mut("user_token")
            .map(Value::take)
            .ok_or(Error::Auth)?
        else {
            return Err(Error::Auth);
        };
        if !store::valid_user_token(&token) {
            return Err(Error::Auth);
        }
        Ok(Zeroizing::new(token))
    }

    fn fetch(
        &self,
        config: &store::Config,
        query: &Query,
        canceled: &impl Fn() -> bool,
    ) -> Result<Arc<Lyrics>, Error> {
        if query.title.trim().is_empty()
            || query.artist.trim().is_empty()
            || query.title.len() > 4096
            || query.artist.len() > 4096
            || query.album.len() > 4096
        {
            return Err(Error::NotFound);
        }
        let duration = query.duration.to_string();
        let deviation = query.duration.div_ceil(20).max(8).to_string();
        let mut params = vec![
            ("q_track", query.title.as_str()),
            ("q_artist", query.artist.as_str()),
            ("format", "json"),
        ];
        if !query.album.is_empty() {
            params.push(("q_album", query.album.as_str()));
        }
        match config.mode {
            Mode::Anonymous => {
                params.extend([
                    ("app_id", APP_ID),
                    ("usertoken", config.user_token.as_str()),
                    ("subtitle_format", "mxm"),
                ]);
                if query.duration > 0 {
                    params.extend([
                        ("q_duration", duration.as_str()),
                        ("f_subtitle_length", duration.as_str()),
                        ("f_subtitle_length_max_deviation", deviation.as_str()),
                    ]);
                }
                let value =
                    self.request(ANONYMOUS_API, "macro.subtitles.get", &params, canceled)?;
                parse_macro(&value, query).map(Arc::new)
            }
            Mode::ApiKey => {
                if config.api_key.is_empty() {
                    return Err(Error::MissingKey);
                }
                params.push(("apikey", config.api_key.as_str()));
                let matched = self.request(API, "matcher.track.get", &params, canceled)?;
                let track = matched.get("track").ok_or(Error::NotFound)?;
                validate_match(track, query)?;
                if flag(track.get("instrumental")) {
                    let mut lyrics = Lyrics {
                        lines: Vec::new(),
                        synced: false,
                        instrumental: true,
                        copyright: String::new(),
                        source_url: None,
                        tracking_url: None,
                    };
                    complete_metadata(&mut lyrics, Some(track), query);
                    return Ok(Arc::new(lyrics));
                }
                let id = track
                    .get("track_id")
                    .and_then(number)
                    .filter(|id| *id > 0)
                    .ok_or(Error::NotFound)?
                    .to_string();
                let auth = [
                    ("apikey", config.api_key.as_str()),
                    ("track_id", id.as_str()),
                    ("format", "json"),
                ];
                let mut subtitle_params = auth.to_vec();
                subtitle_params.push(("subtitle_format", "lrc"));
                if query.duration > 0 {
                    subtitle_params.extend([
                        ("f_subtitle_length", duration.as_str()),
                        ("f_subtitle_length_max_deviation", deviation.as_str()),
                    ]);
                }
                let subtitle = self.request(API, "track.subtitle.get", &subtitle_params, canceled);
                if let Err(
                    error @ (Error::Auth
                    | Error::Captcha
                    | Error::RateLimited(_)
                    | Error::Canceled
                    | Error::Network),
                ) = subtitle
                {
                    return Err(error);
                }
                if let Ok(data) = subtitle
                    && let Some(subtitle) = data.get("subtitle")
                    && let Ok(mut lyrics) = parse_subtitle(subtitle)
                {
                    complete_metadata(&mut lyrics, Some(track), query);
                    return Ok(Arc::new(lyrics));
                }
                let data = self.request(API, "track.lyrics.get", &auth, canceled)?;
                let mut lyrics = parse_plain(data.get("lyrics").ok_or(Error::NotFound)?)?;
                complete_metadata(&mut lyrics, Some(track), query);
                Ok(Arc::new(lyrics))
            }
        }
    }
}

fn status_error(code: u16, wait: u64) -> Error {
    match code {
        401 => Error::Auth,
        402 | 429 => Error::RateLimited(wait),
        403 => Error::Restricted,
        404 => Error::NotFound,
        _ => Error::Network,
    }
}
fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
fn flag(value: Option<&Value>) -> bool {
    value
        .is_some_and(|value| value.as_bool() == Some(true) || number(value).is_some_and(|n| n != 0))
}
fn body(mut value: Value) -> Result<Value, Error> {
    let message = value.get_mut("message").ok_or(Error::Format)?;
    let header = message.get("header").ok_or(Error::Format)?;
    if header.get("hint").and_then(Value::as_str) == Some("captcha") {
        return Err(Error::Captcha);
    }
    let status = header
        .get("status_code")
        .and_then(number)
        .ok_or(Error::Format)?;
    if status != 200 {
        return Err(status_error(status.try_into().unwrap_or(500), 300));
    }
    let data = message.get_mut("body").ok_or(Error::Format)?.take();
    if !data.is_object() {
        return Err(Error::Format);
    }
    Ok(data)
}

fn source_url(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    if text.len() > 8192 {
        return None;
    }
    let mut url = reqwest::Url::parse(text).ok()?;
    (url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("musixmatch.com" | "www.musixmatch.com")
        )
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| {
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    })
}

fn metadata(value: &Value) -> Result<Lyrics, Error> {
    if flag(value.get("restricted")) {
        return Err(Error::Restricted);
    }
    let copyright = value
        .get("lyrics_copyright")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if copyright.len() > 8192 {
        return Err(Error::Format);
    }
    Ok(Lyrics {
        lines: Vec::new(),
        synced: false,
        instrumental: flag(value.get("instrumental")),
        copyright: copyright.into(),
        source_url: None,
        tracking_url: value
            .get("pixel_tracking_url")
            .and_then(Value::as_str)
            .filter(|text| text.len() <= 8192)
            .and_then(|text| reqwest::Url::parse(text).ok())
            .filter(|url| {
                url.scheme() == "https"
                    && url.host_str() == Some("tracking.musixmatch.com")
                    && url.port_or_known_default() == Some(443)
                    && url.username().is_empty()
                    && url.password().is_none()
            })
            .map(|url| url.to_string()),
    })
}

fn parse_plain(value: &Value) -> Result<Lyrics, Error> {
    let mut lyrics = metadata(value)?;
    let text = value
        .get("lyrics_body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.len() > MAX_TEXT {
        return Err(Error::Format);
    }
    lyrics.lines = text
        .lines()
        .map(|text| Line {
            at: None,
            text: text.trim_end().to_owned(),
        })
        .collect();
    validate_lines(&lyrics.lines)?;
    if lyrics.lines.iter().all(|line| line.text.trim().is_empty()) && !lyrics.instrumental {
        return Err(Error::NotFound);
    }
    Ok(lyrics)
}

fn parse_subtitle(value: &Value) -> Result<Lyrics, Error> {
    let mut lyrics = metadata(value)?;
    let text = value
        .get("subtitle_body")
        .and_then(Value::as_str)
        .ok_or(Error::Format)?;
    if text.len() > MAX_TEXT {
        return Err(Error::Format);
    }
    lyrics.lines = if let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(text) {
        if rows.len() > MAX_LINES {
            return Err(Error::Format);
        }
        rows.into_iter()
            .map(|row| {
                let at = row
                    .pointer("/time/total")
                    .and_then(Value::as_f64)
                    .filter(|t| t.is_finite() && (0.0..=86400.0).contains(t))
                    .ok_or(Error::Format)?;
                let text = row
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(Error::Format)?
                    .to_owned();
                Ok(Line { at: Some(at), text })
            })
            .collect::<Result<_, Error>>()?
    } else {
        parse_lrc(text)?
    };
    validate_lines(&lyrics.lines)?;
    lyrics.lines.sort_by(|a, b| {
        a.at.unwrap_or_default()
            .total_cmp(&b.at.unwrap_or_default())
    });
    if lyrics.lines.iter().all(|line| line.text.trim().is_empty()) {
        return Err(Error::NotFound);
    }
    let mut merged: Vec<Line> = Vec::new();
    for line in lyrics.lines {
        if let Some(last) = merged.last_mut()
            && last.at == line.at
        {
            if !line.text.is_empty() {
                if !last.text.is_empty() {
                    last.text.push('\n');
                }
                last.text.push_str(&line.text);
            }
        } else {
            merged.push(line);
        }
    }
    lyrics.lines = merged;
    lyrics.synced = true;
    Ok(lyrics)
}

fn validate_lines(lines: &[Line]) -> Result<(), Error> {
    if lines.len() > MAX_LINES
        || lines.iter().map(|line| line.text.len()).sum::<usize>() > MAX_TEXT
        || lines.iter().any(|line| {
            line.text.len() > 4096
                || line
                    .text
                    .chars()
                    .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
        })
    {
        return Err(Error::Format);
    }
    // A retired API can return a successful response with this placeholder.
    let prefix = lines
        .iter()
        .flat_map(|line| line.text.split_whitespace())
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if prefix == "wob gopini den tefe woxica fero" {
        return Err(Error::WrongLyrics);
    }
    Ok(())
}

fn normalized_name(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            result.push(ch);
        } else if !matches!(ch, '\'' | '’') && !result.ends_with(' ') {
            result.push(' ');
        }
    }
    result.trim().to_owned()
}

fn same_title(left: &str, right: &str) -> bool {
    let left = normalized_name(left);
    let right = normalized_name(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    [(&left, &right), (&right, &left)]
        .into_iter()
        .any(|(longer, shorter)| {
            longer
                .strip_prefix(shorter.as_str())
                .and_then(|suffix| suffix.strip_prefix(' '))
                .and_then(|suffix| suffix.split_whitespace().next())
                .is_some_and(|word| {
                    matches!(
                        word,
                        "feat"
                            | "ft"
                            | "featuring"
                            | "with"
                            | "remix"
                            | "edit"
                            | "version"
                            | "versions"
                            | "remaster"
                            | "remastered"
                            | "slowed"
                            | "sped"
                            | "nightcore"
                            | "instrumental"
                            | "live"
                            | "radio"
                    )
                })
        })
}

fn same_artist(left: &str, right: &str) -> bool {
    let left = normalized_name(left);
    let right = normalized_name(right);
    !left.is_empty()
        && !right.is_empty()
        && (format!(" {left} ").contains(&format!(" {right} "))
            || format!(" {right} ").contains(&format!(" {left} ")))
}

fn validate_match(track: &Value, query: &Query) -> Result<(), Error> {
    let matches_name =
        |field, aliases, alias_field, wanted: &str, compare: fn(&str, &str) -> bool| {
            track
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|name| compare(name, wanted))
                || track
                    .get(aliases)
                    .and_then(Value::as_array)
                    .is_some_and(|list| {
                        list.iter().any(|alias| {
                            alias
                                .get(alias_field)
                                .unwrap_or(alias)
                                .as_str()
                                .is_some_and(|name| compare(name, wanted))
                        })
                    })
        };
    if matches_name(
        "track_name",
        "track_name_translation_list",
        "track_name",
        &query.title,
        same_title,
    ) && matches_name(
        "artist_name",
        "artist_alias_list",
        "artist_alias",
        &query.artist,
        same_artist,
    ) {
        Ok(())
    } else {
        Err(Error::WrongLyrics)
    }
}

fn language(value: Option<&Value>) -> Option<&str> {
    value?
        .as_str()
        .filter(|lang| !lang.is_empty() && *lang != "und")
}

fn parse_lrc(text: &str) -> Result<Vec<Line>, Error> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            if let Some((minutes, seconds)) = tag.0.split_once(':')
                && let (Ok(minutes), Ok(seconds)) = (minutes.parse::<u32>(), seconds.parse::<f64>())
            {
                let at = f64::from(minutes) * 60.0 + seconds;
                if minutes > 1440
                    || !seconds.is_finite()
                    || !(0.0..60.0).contains(&seconds)
                    || at > 86400.0
                {
                    return Err(Error::Format);
                }
                times.push(at);
            } else if tag.0.starts_with("offset:") && tag.0 != "offset:0" {
                // Never guess timing for a provider export with an unsupported offset.
                return Err(Error::Format);
            }
            rest = tag.1;
        }
        for at in times {
            lines.push(Line {
                at: Some(at),
                text: rest.trim().into(),
            });
        }
        if lines.len() > MAX_LINES {
            return Err(Error::Format);
        }
    }
    Ok(lines)
}

fn complete_metadata(lyrics: &mut Lyrics, track: Option<&Value>, query: &Query) {
    lyrics.source_url = track.and_then(|track| source_url(track.get("track_share_url")));
    lyrics.instrumental |= track.is_some_and(|track| flag(track.get("instrumental")));
    let length = track
        .and_then(|track| track.get("track_length"))
        .and_then(number)
        .filter(|length| *length > 0);
    let different_version = length.is_some_and(|length| {
        query.duration > 0 && length.abs_diff(u64::from(query.duration)) > (8_u64.max(length / 20))
    });
    if lyrics.synced && (different_version || (query.speed_variant && length.is_none())) {
        lyrics.synced = false;
        for line in &mut lyrics.lines {
            line.at = None;
        }
    }
}

fn parse_macro(value: &Value, query: &Query) -> Result<Lyrics, Error> {
    let calls = value.get("macro_calls").ok_or(Error::Format)?;
    let track_data = body(
        calls
            .get("matcher.track.get")
            .or_else(|| calls.get("track.get"))
            .ok_or(Error::NotFound)?
            .clone(),
    )?;
    let track = track_data.get("track").ok_or(Error::NotFound)?;
    validate_match(track, query)?;
    let plain_data = calls.get("track.lyrics.get").map(|call| body(call.clone()));
    let plain_value = plain_data
        .as_ref()
        .and_then(|data| data.as_ref().ok())
        .and_then(|data| data.get("lyrics"));
    let original_language = language(plain_value.and_then(|value| value.get("lyrics_language")))
        .or_else(|| language(track.get("lyrics_language")));
    let mut subtitle_error = None;
    if let Some(call) = calls
        .get("track.subtitles.get")
        .or_else(|| calls.get("track.subtitle.get"))
    {
        match body(call.clone()) {
            Ok(data) => {
                let subtitles = data
                    .get("subtitle_list")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry.get("subtitle"))
                    .chain(data.get("subtitle"));
                for subtitle in subtitles {
                    if let (Some(original), Some(actual)) = (
                        original_language,
                        language(subtitle.get("subtitle_language")),
                    ) && !original.eq_ignore_ascii_case(actual)
                    {
                        subtitle_error = Some(Error::WrongLyrics);
                        continue;
                    }
                    match parse_subtitle(subtitle) {
                        Ok(mut lyrics) => {
                            complete_metadata(&mut lyrics, Some(track), query);
                            return Ok(lyrics);
                        }
                        Err(error) => subtitle_error = Some(error),
                    }
                }
            }
            Err(error @ (Error::Auth | Error::Captcha | Error::RateLimited(_))) => {
                return Err(error);
            }
            Err(error) => subtitle_error = Some(error),
        }
    }
    if let Some(result) = plain_data {
        let data = result?;
        let mut lyrics = parse_plain(data.get("lyrics").ok_or(Error::NotFound)?)?;
        complete_metadata(&mut lyrics, Some(track), query);
        return Ok(lyrics);
    }
    if flag(track.get("instrumental")) {
        let mut lyrics = Lyrics {
            lines: Vec::new(),
            synced: false,
            instrumental: true,
            copyright: String::new(),
            source_url: None,
            tracking_url: None,
        };
        complete_metadata(&mut lyrics, Some(track), query);
        return Ok(lyrics);
    }
    Err(subtitle_error.unwrap_or(Error::NotFound))
}

#[cfg(test)]
pub(crate) fn test_data(count: usize, synced: bool) -> Lyrics {
    Lyrics {
        lines: (0..count)
            .map(|index| Line {
                at: synced.then_some(index as f64 * 10.0),
                text: format!("Synthetic lyric line {index}"),
            })
            .collect(),
        synced,
        instrumental: false,
        copyright: "© Synthetic test".into(),
        source_url: None,
        tracking_url: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn query() -> Query {
        Query {
            key: (42, 7),
            title: "Synthetic song".into(),
            artist: "Synthetic artist".into(),
            album: String::new(),
            duration: 180,
            speed_variant: false,
        }
    }
    fn call(data: Value) -> Value {
        json!({"message":{"header":{"status_code":200},"body":data}})
    }
    #[test]
    fn anonymous_mxm_lines_are_sorted_merged_and_follow_exact_playback_time() {
        let subtitle = json!({"subtitle_body":serde_json::to_string(&json!([
            {"text":"Second synthetic line","time":{"total":20.0}},
            {"text":"First synthetic line","time":{"total":1.25}},
            {"text":"Same timestamp","time":{"total":20.0}},
            {"text":"","time":{"total":30.0}}
        ])).unwrap(),"restricted":false,"lyrics_copyright":"© Synthetic test"});
        let data = json!({"macro_calls":{
            "matcher.track.get":call(json!({"track":{"track_name":"Synthetic song","artist_name":"Synthetic artist","track_length":180,"track_share_url":"https://www.musixmatch.com/lyrics/synthetic/test"}})),
            "track.subtitles.get":call(json!({"subtitle_list":[{"subtitle":subtitle}]}))
        }});
        let lyrics = parse_macro(&data, &query()).unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(lyrics.active_at(0.0), None);
        assert_eq!(lyrics.active_at(1.25), Some(0));
        assert_eq!(lyrics.active_at(20.0), Some(1));
        assert_eq!(lyrics.active_at(f64::NAN), None);
        assert_eq!(
            lyrics.lines[1].text,
            "Second synthetic line\nSame timestamp"
        );
        assert_eq!(lyrics.copyright, "© Synthetic test");
        assert!(lyrics.source_url.is_some());
    }
    #[test]
    fn lrc_and_plain_text_keep_unicode_and_do_not_invent_timing() {
        let timed=parse_subtitle(&json!({"subtitle_body":"[ar:Synthetic]\n[00:01.50][00:10.50]Тестの行\n[00:20.00]Last synthetic line"})).unwrap();
        assert_eq!(timed.lines.len(), 3);
        assert_eq!(timed.lines[1].at, Some(10.5));
        assert!(
            parse_subtitle(&json!({"subtitle_body":"[offset:500]\n[00:01.00]Synthetic"})).is_err()
        );
        let plain =
            parse_plain(&json!({"lyrics_body":"First synthetic line\n\nSecond synthetic line"}))
                .unwrap();
        assert!(!plain.synced);
        assert_eq!(plain.active_at(30.0), None);
        assert_eq!(plain.lines.len(), 3);
        let mut different = timed.clone();
        complete_metadata(&mut different, Some(&json!({"track_length":90})), &query());
        assert!(!different.synced && different.lines.iter().all(|line| line.at.is_none()));
    }
    #[test]
    fn access_denials_challenges_and_quotas_never_become_visible_lyrics() {
        for flag in [json!(true), json!(1), json!("1")] {
            assert_eq!(
                parse_plain(&json!({"restricted":flag,"lyrics_body":"Not displayed"})).unwrap_err(),
                Error::Restricted
            );
        }
        for (code, error) in [
            (401, Error::Auth),
            (403, Error::Restricted),
            (404, Error::NotFound),
            (429, Error::RateLimited(300)),
        ] {
            assert_eq!(
                body(json!({"message":{"header":{"status_code":code},"body":{}}})).unwrap_err(),
                error
            );
        }
        assert_eq!(
            body(json!({"message":{"header":{"status_code":401,"hint":"captcha"},"body":{}}}))
                .unwrap_err(),
            Error::Captcha
        );
        let data = json!({"macro_calls":{
            "matcher.track.get":call(json!({"track":{"track_name":"Synthetic song","artist_name":"Synthetic artist"}})),
            "track.subtitles.get":{"message":{"header":{"status_code":401},"body":{}}},
            "track.lyrics.get":call(json!({"lyrics":{"lyrics_body":"Must not be returned"}}))
        }});
        assert_eq!(parse_macro(&data, &query()).unwrap_err(), Error::Auth);
        assert!(source_url(Some(&json!("https://example.test/lyrics"))).is_none());
        assert!(source_url(Some(&json!("https://user:secret@musixmatch.com/lyrics"))).is_none());
    }
    #[test]
    fn unavailable_subtitles_can_use_authorized_plain_text_and_invalid_bodies_stay_bounded() {
        let data = json!({"macro_calls":{
            "matcher.track.get":call(json!({"track":{"track_name":"Synthetic song","artist_name":"Synthetic artist"}})),
            "track.subtitles.get":{"message":{"header":{"status_code":404},"body":{}}},
            "track.lyrics.get":call(json!({"lyrics":{"lyrics_body":"Synthetic plain text","lyrics_copyright":"© Synthetic test"}}))
        }});
        assert!(!parse_macro(&data, &query()).unwrap().synced);
        assert!(parse_plain(&json!({"lyrics_body":"x".repeat(MAX_TEXT+1)})).is_err());
        assert!(parse_subtitle(&json!({"subtitle_body":"[99:99.0]Invalid timestamp"})).is_err());
        assert!(parse_plain(&json!({"lyrics_body":"bad\u{0000}line"})).is_err());
    }

    #[test]
    fn successful_but_wrong_matches_and_placeholder_text_are_rejected() {
        let data = json!({"macro_calls":{
            "matcher.track.get":call(json!({"track":{"track_name":"A different song","artist_name":"Another artist"}})),
            "track.lyrics.get":call(json!({"lyrics":{"lyrics_body":"Wrong synthetic text"}}))
        }});
        assert_eq!(
            parse_macro(&data, &query()).unwrap_err(),
            Error::WrongLyrics
        );
        for text in [
            "Wob gopini den\nTefe woxica fero",
            " WOB   gopini den\n\nTefe woxica fero ",
        ] {
            assert_eq!(
                parse_plain(&json!({"lyrics_body":text})).unwrap_err(),
                Error::WrongLyrics
            );
        }
        assert_eq!(
            parse_subtitle(
                &json!({"subtitle_body":"[00:01]Wob gopini den\n[00:03]Tefe woxica fero"})
            )
            .unwrap_err(),
            Error::WrongLyrics
        );
        assert!(same_title(
            "Посмотри",
            "Посмотри (feat. Synthetic guest) [Test Remix]"
        ));
        assert!(same_title("Synthetic—Song", "SYNTHETIC SONG (Slowed)"));
        assert!(!same_title("Synthetic song", "Synthetic song Two"));
        assert!(same_artist(
            "Тестовый исполнитель",
            "Тестовый исполнитель feat. Synthetic guest"
        ));
        assert!(!same_artist("Ann", "Anna"));
    }

    #[test]
    fn originals_keep_their_language_and_never_use_translated_subtitle_bodies() {
        for (lang, original) in [
            ("ru", "Тестовая строка"),
            ("ja", "テストの行"),
            ("en", "Synthetic original line"),
        ] {
            let subtitle = |language: &str, text: &str| {
                json!({"subtitle":{
                    "subtitle_language":language,
                    "subtitle_body":serde_json::to_string(&json!([{"text":text,"time":{"total":2.0}}])).unwrap(),
                    "subtitle_translated":{"subtitle_body":"[00:02]Must not replace the original"}
                }})
            };
            let data = json!({"macro_calls":{
                "matcher.track.get":call(json!({"track":{"track_name":"Synthetic song","artist_name":"Synthetic artist","track_length":180}})),
                "track.lyrics.get":call(json!({"lyrics":{"lyrics_language":lang,"lyrics_body":original}})),
                "track.subtitles.get":call(json!({"subtitle_list":[subtitle("other", "Wrong language"),subtitle(lang, original)]}))
            }});
            let lyrics = parse_macro(&data, &query()).unwrap();
            assert!(lyrics.synced);
            assert_eq!(lyrics.lines[0].text, original);
            let mut plain_fallback = data;
            plain_fallback["macro_calls"]["track.subtitles.get"] =
                call(json!({"subtitle_list":[subtitle("other", "Wrong language")]}));
            let lyrics = parse_macro(&plain_fallback, &query()).unwrap();
            assert!(!lyrics.synced);
            assert_eq!(lyrics.lines[0].text, original);
        }
    }
}
