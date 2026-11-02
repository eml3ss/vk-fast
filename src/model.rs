use std::borrow::Cow;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(from = "ProfileWire")]
pub struct Profile {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct ProfileWire {
    id: i64,
    first_name: String,
    last_name: String,
    #[serde(default)]
    avatar_url: Option<serde_json::Value>,
    #[serde(default)]
    photo_200_orig: Option<serde_json::Value>,
    #[serde(default)]
    photo_200: Option<serde_json::Value>,
    #[serde(default)]
    avatar: Option<serde_json::Value>,
    #[serde(default)]
    photo_100: Option<serde_json::Value>,
    #[serde(default)]
    photo_50: Option<serde_json::Value>,
}

impl From<ProfileWire> for Profile {
    fn from(wire: ProfileWire) -> Self {
        let avatar_url = avatar_from_values([
            wire.avatar_url.as_ref(),
            wire.photo_200_orig.as_ref(),
            wire.photo_200.as_ref(),
            wire.avatar.as_ref(),
            wire.photo_100.as_ref(),
            wire.photo_50.as_ref(),
        ]);
        Self {
            id: wire.id,
            first_name: wire.first_name,
            last_name: wire.last_name,
            avatar_url,
        }
    }
}

/// Call only on metadata already verified as belonging to the current account.
/// VK ID uses avatar; users.get and the website's auth user use photo_200.
pub(crate) fn profile_avatar_from_object(value: &serde_json::Value) -> Option<String> {
    avatar_from_values([
        value.get("avatar_url"),
        value.get("photo_200_orig"),
        value.get("photo_200"),
        value.get("avatar"),
        value.get("photo_100"),
        value.get("photo_50"),
    ])
}

fn avatar_from_values<'a>(
    values: impl IntoIterator<Item = Option<&'a serde_json::Value>>,
) -> Option<String> {
    values.into_iter().find_map(|value| {
        value
            .and_then(serde_json::Value::as_str)
            .and_then(valid_cover_url)
    })
}

impl Profile {
    pub fn has_name(&self) -> bool {
        (!self.first_name.trim().is_empty() || !self.last_name.trim().is_empty())
            && !(self.first_name == "VK" && self.last_name == self.id.to_string())
            && [&self.first_name, &self.last_name]
                .into_iter()
                .all(|name| name.len() <= 1024 && !name.chars().any(char::is_control))
    }

    /// Missing website metadata must not replace a previously known name/photo.
    pub fn merge_metadata(&mut self, fresh: &Self) -> bool {
        if self.id <= 0 || self.id != fresh.id {
            return false;
        }
        let mut changed = false;
        if fresh.has_name()
            && (self.first_name != fresh.first_name || self.last_name != fresh.last_name)
        {
            self.first_name.clone_from(&fresh.first_name);
            self.last_name.clone_from(&fresh.last_name);
            changed = true;
        }
        if let Some(avatar) = fresh
            .avatar_url
            .as_ref()
            .filter(|url| valid_cover_url(url).is_some())
            && self.avatar_url.as_ref() != Some(avatar)
        {
            self.avatar_url = Some(avatar.clone());
            changed = true;
        }
        changed
    }

    pub fn display_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
            .trim()
            .to_owned()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(from = "TrackWire")]
pub struct Track {
    pub id: i64,
    pub owner_id: i64,
    pub artist: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub duration: u32,
    // VK may omit the stream even when the track itself is visible.
    pub url: Option<String>,
    pub cover_url: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    /// VK's explicit My Music membership; absence means it was not supplied.
    pub library_liked: Option<bool>,
    /// Canonical release audio identity, separate from an account's saved copy.
    pub release_key: Option<(i64, i64)>,
    /// Explicit VK restriction state. Missing stream URLs are not evidence.
    pub unavailable: Option<bool>,
}

/// Canonical VK identity, kept separately from the display-only artist string.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AlbumRef {
    pub id: i64,
    pub owner_id: i64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

fn metadata_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

pub(crate) fn valid_artist_id(id: &str) -> bool {
    metadata_text(id, 256)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
        && id != "0"
}

/// Match VK's getIsBlocked and legacy isClaimedAudio predicates. Preserve
/// missing evidence so sparse metadata cannot clear a previously known block.
pub(crate) fn audio_unavailable_from_object(value: &serde_json::Value) -> Option<bool> {
    audio_unavailable_from_fields(
        value
            .get("content_restricted")
            .or_else(|| value.get("restrictionStatus")),
        value.get("dmca_blocked"),
        // Only the legacy tuple contract proves CLAIMED lives in bit 4.
        // Modern API objects use explicit restriction fields; their generic
        // `flags` field must not be interpreted as a rights mask.
        None,
        value.get("isClaimed"),
        value.get("unavailable"),
    )
}

pub(crate) fn audio_unavailable_from_fields(
    restriction: Option<&serde_json::Value>,
    dmca: Option<&serde_json::Value>,
    flags: Option<&serde_json::Value>,
    claimed: Option<&serde_json::Value>,
    cached: Option<&serde_json::Value>,
) -> Option<bool> {
    // Modern getIsBlocked: dmca_blocked or integer content_restricted != 0.
    // Legacy EAudioTupleIndex.FLAGS=10, CLAIMED bit=4, RESTRICTION=21.
    let boolean_flag = |value: Option<&serde_json::Value>| {
        value.and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_i64().map(|flag| flag != 0))
        })
    };
    let evidence = [
        restriction
            .and_then(serde_json::Value::as_i64)
            .map(|status| status != 0),
        boolean_flag(dmca),
        flags
            .and_then(serde_json::Value::as_u64)
            .map(|flags| flags & 4 != 0),
        boolean_flag(claimed),
        cached.and_then(serde_json::Value::as_bool),
    ];
    if evidence.contains(&Some(true)) {
        Some(true)
    } else if evidence.contains(&Some(false)) {
        Some(false)
    } else {
        None
    }
}

fn value_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(crate) fn artists_from_object(value: &serde_json::Value) -> Vec<ArtistRef> {
    let mut artists = Vec::<ArtistRef>::new();
    for key in [
        "main_artists",
        "mainArtists",
        "featured_artists",
        "featuredArtists",
    ] {
        append_artists(value.get(key), &mut artists);
    }
    if artists.is_empty() {
        append_artists(value.get("artists"), &mut artists);
    }
    artists
}

fn append_artists(value: Option<&serde_json::Value>, artists: &mut Vec<ArtistRef>) {
    for item in value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if artists.len() >= 64 {
            return;
        }
        let (Some(id), Some(name)) = (
            item.get("id").and_then(value_id),
            item.get("name").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        if valid_artist_id(&id)
            && metadata_text(name, 4096)
            && !artists.iter().any(|artist| artist.id == id)
        {
            artists.push(ArtistRef {
                id,
                name: name.trim().to_owned(),
            });
        }
    }
}

impl AlbumRef {
    pub(crate) fn from_value(value: &serde_json::Value) -> Option<Self> {
        let number = |key: &str| {
            value.get(key).and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            })
        };
        let id = number("id")?;
        let owner_id = number("owner_id")?;
        let title = value
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if id == 0 || owner_id == 0 || title.len() > 4096 || title.chars().any(char::is_control) {
            return None;
        }
        let access_key = value
            .get("access_key")
            .and_then(serde_json::Value::as_str)
            .filter(|value| metadata_text(value, 512))
            .map(str::to_owned);
        let cover_url = cover_from_fields(
            value.get("cover_url"),
            value.get("coverUrl"),
            None,
            value.get("thumb"),
        )
        .or_else(|| cover_from_fields(None, None, None, value.get("photo")));
        Some(Self {
            id,
            owner_id,
            title: title.trim().to_owned(),
            access_key,
            cover_url,
        })
    }
}

pub(crate) fn album_from_object(value: &serde_json::Value) -> Option<AlbumRef> {
    AlbumRef::from_value(value.get("album")?)
}

/// Both the modern audio API and VK's website expose explicit membership.
/// Do not coerce numeric flags or infer membership from an audio's owner.
pub(crate) fn library_liked_from_object(value: &serde_json::Value) -> Option<bool> {
    library_liked_from_fields(
        value.get("like"),
        value.get("extra"),
        value.get("library_liked"),
    )
}

fn library_liked_from_fields(
    native: Option<&serde_json::Value>,
    extra: Option<&serde_json::Value>,
    stored: Option<&serde_json::Value>,
) -> Option<bool> {
    native.and_then(serde_json::Value::as_bool).or_else(|| {
        extra
            .and_then(|extra| match extra {
                serde_json::Value::Object(extra) => {
                    extra.get("like").and_then(serde_json::Value::as_bool)
                }
                serde_json::Value::String(extra) if extra.len() <= 65_536 => {
                    serde_json::from_str::<serde_json::Value>(extra)
                        .ok()?
                        .get("like")
                        .and_then(serde_json::Value::as_bool)
                }
                _ => None,
            })
            .or_else(|| stored.and_then(serde_json::Value::as_bool))
    })
}

pub(crate) fn valid_release_key((owner_id, audio_id): (i64, i64)) -> bool {
    owner_id != 0 && audio_id > 0
}

/// A release ID contains exactly two decimal IDs, without an access key.
pub(crate) fn parse_release_key(value: &str) -> Option<(i64, i64)> {
    let (owner, audio) = value.split_once('_')?;
    let owner_digits = owner.strip_prefix('-').unwrap_or(owner);
    if owner_digits.is_empty()
        || !owner_digits.bytes().all(|byte| byte.is_ascii_digit())
        || audio.is_empty()
        || !audio.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let key = (owner.parse().ok()?, audio.parse().ok()?);
    valid_release_key(key).then_some(key)
}

fn release_key_from_value(value: &serde_json::Value) -> Option<(i64, i64)> {
    let pair = value.as_array()?;
    if pair.len() != 2 {
        return None;
    }
    let key = (pair[0].as_i64()?, pair[1].as_i64()?);
    valid_release_key(key).then_some(key)
}

pub(crate) fn release_key_from_object(value: &serde_json::Value) -> Option<(i64, i64)> {
    ["release_audio_id", "trackPageId"]
        .into_iter()
        .find_map(|key| value.get(key)?.as_str().and_then(parse_release_key))
        .or_else(|| value.get("release_key").and_then(release_key_from_value))
}

/// VK's API calls this `subtitle`; its legacy website object calls it subTitle.
/// It identifies Slowed, Sped Up, remixes and similar published versions.
pub(crate) fn subtitle_from_value(value: Option<&serde_json::Value>) -> Option<String> {
    let raw = value?.as_str()?;
    if raw.len() > 4096 {
        return None;
    }
    let decoded = html_escape::decode_html_entities(raw);
    let decoded = decoded.trim();
    (!decoded.is_empty() && decoded.len() <= 4096 && !decoded.chars().any(char::is_control))
        .then(|| decoded.to_owned())
}

#[derive(Deserialize)]
struct TrackWire {
    id: i64,
    owner_id: i64,
    #[serde(default)]
    artist: String,
    title: String,
    #[serde(default, alias = "subTitle")]
    subtitle: Option<serde_json::Value>,
    duration: u32,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    cover_url: Option<serde_json::Value>,
    #[serde(default, rename = "coverUrl")]
    web_cover: Option<serde_json::Value>,
    #[serde(default)]
    album: Option<serde_json::Value>,
    #[serde(default)]
    thumb: Option<serde_json::Value>,
    #[serde(default, alias = "mainArtists")]
    main_artists: Option<serde_json::Value>,
    #[serde(default, alias = "featuredArtists")]
    featured_artists: Option<serde_json::Value>,
    #[serde(default)]
    artists: Option<serde_json::Value>,
    #[serde(default)]
    like: Option<serde_json::Value>,
    #[serde(default)]
    extra: Option<serde_json::Value>,
    #[serde(default)]
    library_liked: Option<serde_json::Value>,
    #[serde(default)]
    release_audio_id: Option<serde_json::Value>,
    #[serde(default, rename = "trackPageId")]
    web_release_audio_id: Option<serde_json::Value>,
    #[serde(default)]
    release_key: Option<serde_json::Value>,
    #[serde(default, alias = "restrictionStatus")]
    content_restricted: Option<serde_json::Value>,
    #[serde(default)]
    dmca_blocked: Option<serde_json::Value>,
    #[serde(default, rename = "isClaimed")]
    is_claimed: Option<serde_json::Value>,
    #[serde(default)]
    unavailable: Option<serde_json::Value>,
}

impl From<TrackWire> for Track {
    fn from(wire: TrackWire) -> Self {
        let unavailable = audio_unavailable_from_fields(
            wire.content_restricted.as_ref(),
            wire.dmca_blocked.as_ref(),
            None,
            wire.is_claimed.as_ref(),
            wire.unavailable.as_ref(),
        );
        let cover_url = cover_from_fields(
            wire.cover_url.as_ref(),
            wire.web_cover.as_ref(),
            wire.album.as_ref(),
            wire.thumb.as_ref(),
        );
        let mut artists = Vec::new();
        append_artists(wire.main_artists.as_ref(), &mut artists);
        append_artists(wire.featured_artists.as_ref(), &mut artists);
        if artists.is_empty() {
            append_artists(wire.artists.as_ref(), &mut artists);
        }
        let album = wire.album.as_ref().and_then(AlbumRef::from_value);
        let library_liked = library_liked_from_fields(
            wire.like.as_ref(),
            wire.extra.as_ref(),
            wire.library_liked.as_ref(),
        );
        let release_key = wire
            .release_audio_id
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .and_then(parse_release_key)
            .or_else(|| {
                wire.web_release_audio_id
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .and_then(parse_release_key)
            })
            .or_else(|| wire.release_key.as_ref().and_then(release_key_from_value));
        let artist = if wire.artist.trim().is_empty() {
            artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            wire.artist
        };
        Self {
            id: wire.id,
            owner_id: wire.owner_id,
            artist,
            title: wire.title,
            subtitle: subtitle_from_value(wire.subtitle.as_ref()),
            duration: wire.duration,
            url: wire.url,
            cover_url,
            artists,
            album,
            library_liked,
            release_key,
            unavailable,
        }
    }
}

/// Track artwork only: no artist or recommendation images are substituted.
/// VK web objects use coverUrl; modern API objects use album.thumb or thumb.
pub(crate) fn cover_from_object(value: &serde_json::Value) -> Option<String> {
    cover_from_fields(
        value.get("cover_url"),
        value.get("coverUrl"),
        value.get("album"),
        value.get("thumb"),
    )
}

fn cover_from_fields(
    plain: Option<&serde_json::Value>,
    web: Option<&serde_json::Value>,
    album: Option<&serde_json::Value>,
    thumb: Option<&serde_json::Value>,
) -> Option<String> {
    let direct = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_str)
            .and_then(valid_cover_url)
    };
    let thumbnail = |value: Option<&serde_json::Value>| {
        let value = value?;
        [
            "photo_600",
            "photo_300",
            "photo_270",
            "photo_135",
            "photo_68",
            "photo_34",
            "photo_1200",
        ]
        .into_iter()
        .find_map(|key| direct(value.get(key)))
    };
    direct(plain)
        .or_else(|| direct(web))
        .or_else(|| thumbnail(album.and_then(|album| album.get("thumb"))))
        .or_else(|| thumbnail(thumb))
}

pub(crate) fn valid_cover_url(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 8192 {
        return None;
    }
    let decoded = html_escape::decode_html_entities(value);
    if decoded
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return None;
    }
    let url = reqwest::Url::parse(&decoded).ok()?;
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
    {
        return None;
    }
    Some(url.into())
}

impl Track {
    pub fn key(&self) -> (i64, i64) {
        (self.owner_id, self.id)
    }

    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.title.to_lowercase().contains(query)
            || self
                .subtitle
                .as_ref()
                .is_some_and(|subtitle| subtitle.to_lowercase().contains(query))
            || self.artist.to_lowercase().contains(query)
    }

    /// Presentation-only parts. A version is separated only when VK supplied
    /// explicit metadata; punctuation in unrelated track names stays intact.
    pub fn title_parts(&self) -> (&str, Option<&str>) {
        let Some(subtitle) = self
            .subtitle
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return (&self.title, None);
        };
        let title = self.title.trim_end();
        let lower_title = title.to_lowercase();
        let lower_subtitle = subtitle.to_lowercase();
        if lower_title == lower_subtitle {
            return (&self.title, None);
        }
        for suffix in [
            format!("({subtitle})"),
            format!("[{subtitle}]"),
            format!(" - {subtitle}"),
            format!(" — {subtitle}"),
            format!(" {subtitle}"),
        ] {
            // Character boundaries avoid slicing Unicode metadata by bytes.
            let Some((start, _)) = title.char_indices().rev().nth(suffix.chars().count() - 1)
            else {
                continue;
            };
            if title[start..].to_lowercase() == suffix.to_lowercase() {
                return (title[..start].trim_end(), Some(subtitle));
            }
        }
        (&self.title, Some(subtitle))
    }

    pub fn display_title(&self) -> Cow<'_, str> {
        match self.title_parts() {
            ("", Some(subtitle)) => Cow::Borrowed(subtitle),
            (title, Some(subtitle)) => Cow::Owned(format!("{title} {subtitle}")),
            (title, None) => Cow::Borrowed(title),
        }
    }

    pub fn duration_label(&self) -> String {
        format!("{}:{:02}", self.duration / 60, self.duration % 60)
    }

    pub fn has_stream(&self) -> bool {
        self.url.as_ref().is_some_and(|url| !url.is_empty())
    }
}

#[derive(Debug, Deserialize)]
pub struct AudioPage {
    #[serde(default)]
    pub count: Option<usize>,
    pub items: Vec<Track>,
    #[serde(skip)]
    pub exhausted: Option<bool>,
}

pub fn demo_tracks() -> Vec<Track> {
    [
        ("Северный ветер", "Demo · Тихий берег", 214),
        ("Свет в окнах", "Demo · После полуночи", 187),
        ("Morning Lines", "Demo · Paper Planes", 242),
        ("Остаться здесь", "Demo · Комната 7", 198),
        ("Slow Motion", "Demo · Blue Avenue", 271),
        ("За городом", "Demo · Радиоволны", 203),
        ("Тёплый октябрь", "Demo · Тихий берег", 226),
        ("Somewhere Near", "Demo · Paper Planes", 193),
        ("На повторе", "Demo · Комната 7", 178),
        ("Empty Streets", "Demo · Blue Avenue", 256),
        ("Первый снег", "Demo · После полуночи", 219),
        ("Дальше", "Demo · Радиоволны", 231),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (title, artist, duration))| Track {
        id: i as i64,
        owner_id: 0,
        artist: artist.into(),
        title: title.into(),
        subtitle: None,
        duration,
        url: None,
        cover_url: None,
        artists: Vec::new(),
        album: None,
        library_liked: None,
        release_key: None,
        unavailable: None,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_library_membership_accepts_native_and_website_shapes() {
        let base = serde_json::json!({
            "id": 7, "owner_id": 42, "artist": "Artist", "title": "Song", "duration": 120
        });
        let original: Track = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(
            original.library_liked, None,
            "An account-owned ID alone is not a like"
        );
        for (field, metadata) in [
            ("like", serde_json::json!(false)),
            ("extra", serde_json::json!({"like": false})),
            (
                "extra",
                serde_json::json!(r#"{"like":false,"unrelated":"ignored"}"#),
            ),
            ("library_liked", serde_json::json!(false)),
        ] {
            for expected in [false, true] {
                let mut value = base.clone();
                value[field] = match &metadata {
                    serde_json::Value::Bool(_) => serde_json::json!(expected),
                    serde_json::Value::Object(_) => serde_json::json!({"like": expected}),
                    serde_json::Value::String(_) => {
                        serde_json::json!(format!(r#"{{"like":{expected}}}"#))
                    }
                    _ => unreachable!(),
                };
                let parsed: Track = serde_json::from_value(value.clone()).unwrap();
                assert_eq!(parsed.library_liked, Some(expected));
                assert_eq!(library_liked_from_object(&value), Some(expected));
            }
        }
        let mut value = base;
        value["like"] = serde_json::json!(false);
        value["extra"] = serde_json::json!({"like": true});
        assert_eq!(library_liked_from_object(&value), Some(false));
        assert_eq!(
            serde_json::from_value::<Track>(value)
                .unwrap()
                .library_liked,
            Some(false)
        );
    }

    #[test]
    fn malformed_membership_metadata_does_not_invent_likes_or_hide_tracks() {
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!(0),
            serde_json::json!(1),
            serde_json::json!("true"),
            serde_json::json!([true]),
            serde_json::json!({"unexpected": true}),
        ] {
            for field in ["like", "library_liked", "extra"] {
                let mut value = serde_json::json!({
                    "id": 7, "owner_id": 42, "title": "Song", "duration": 120
                });
                value[field] = invalid.clone();
                assert_eq!(library_liked_from_object(&value), None);
                assert_eq!(
                    serde_json::from_value::<Track>(value)
                        .unwrap()
                        .library_liked,
                    None
                );
            }
            for extra in [
                serde_json::json!({"like": invalid}),
                serde_json::json!(format!(r#"{{"like":{invalid}}}"#)),
            ] {
                let value = serde_json::json!({
                    "id": 7, "owner_id": 42, "title": "Song", "duration": 120, "extra": extra
                });
                assert_eq!(library_liked_from_object(&value), None);
                assert_eq!(
                    serde_json::from_value::<Track>(value)
                        .unwrap()
                        .library_liked,
                    None
                );
            }
        }
        let fallback = serde_json::json!({"like": "invalid", "extra": {"like": false}});
        assert_eq!(library_liked_from_object(&fallback), Some(false));
        assert_eq!(
            library_liked_from_object(&serde_json::json!({"extra": "{broken"})),
            None
        );
    }

    #[test]
    fn canonical_release_identity_is_optional_strict_and_separate_from_saved_copy() {
        for (text, expected) in [
            ("42_7", (42, 7)),
            ("-9000000000_8", (-9_000_000_000, 8)),
            (
                "-9223372036854775808_9223372036854775807",
                (i64::MIN, i64::MAX),
            ),
        ] {
            assert_eq!(parse_release_key(text), Some(expected));
            for field in ["release_audio_id", "trackPageId"] {
                let mut value = serde_json::json!({
                    "id": 901, "owner_id": 99, "title": "Song", "duration": 120
                });
                value[field] = text.into();
                let parsed: Track = serde_json::from_value(value.clone()).unwrap();
                assert_eq!(parsed.key(), (99, 901));
                assert_eq!(parsed.release_key, Some(expected));
                assert_eq!(release_key_from_object(&value), Some(expected));
            }
        }
        for invalid in [
            "",
            "7",
            "0_7",
            "42_0",
            "42_-7",
            "+42_7",
            "42_+7",
            " 42_7",
            "42_7 ",
            "42_7_secret",
            "42_7_8",
            "--42_7",
            "42_7.0",
            "42_9223372036854775808",
            "9223372036854775808_7",
            "-9223372036854775809_7",
            "42_７",
        ] {
            assert_eq!(parse_release_key(invalid), None, "{invalid:?}");
            let value = serde_json::json!({
                "id": 901, "owner_id": 99, "title": "Song", "duration": 120, "release_audio_id": invalid
            });
            assert_eq!(release_key_from_object(&value), None);
            assert_eq!(
                serde_json::from_value::<Track>(value).unwrap().release_key,
                None
            );
        }
        for invalid in [
            serde_json::json!(true),
            serde_json::json!([42, 7]),
            serde_json::json!({"owner_id": 42, "id": 7}),
        ] {
            assert_eq!(
                release_key_from_object(&serde_json::json!({"release_audio_id": invalid})),
                None
            );
        }
        let mut value = serde_json::json!({
            "id": 901, "owner_id": 99, "title": "Song", "duration": 120, "release_key": [-42, 7]
        });
        assert_eq!(
            serde_json::from_value::<Track>(value.clone())
                .unwrap()
                .release_key,
            Some((-42, 7))
        );
        value["release_key"] = serde_json::json!([42, 0]);
        assert_eq!(
            serde_json::from_value::<Track>(value).unwrap().release_key,
            None
        );
    }

    #[test]
    fn version_subtitles_are_optional_decoded_and_searchable_without_duplicate_labels() {
        let base = serde_json::json!({"id":7,"owner_id":42,"artist":"Artist","title":"CICADATEKK","duration":120});
        let original: Track = serde_json::from_value(base.clone()).unwrap();
        assert!(original.subtitle.is_none());
        assert_eq!(original.display_title(), "CICADATEKK");
        for field in ["subtitle", "subTitle"] {
            let mut value = base.clone();
            value[field] = serde_json::json!(" Slowed &amp; Reverb ");
            let track: Track = serde_json::from_value(value).unwrap();
            assert_eq!(track.title, "CICADATEKK");
            assert_eq!(track.subtitle.as_deref(), Some("Slowed & Reverb"));
            assert_eq!(track.display_title(), "CICADATEKK Slowed & Reverb");
            assert_eq!(track.title_parts(), ("CICADATEKK", Some("Slowed & Reverb")));
            assert!(track.matches("slowed"));
            assert!(track.matches("reverb"));
            for title in [
                "CICADATEKK (Slowed & Reverb)",
                "CICADATEKK [slowed & reverb]",
                "CICADATEKK - Slowed & Reverb",
                "CICADATEKK — Slowed & Reverb",
                "CICADATEKK Slowed & Reverb",
            ] {
                let mut track = track.clone();
                track.title = title.into();
                assert_eq!(track.title, title);
                assert_eq!(track.title_parts(), ("CICADATEKK", Some("Slowed & Reverb")));
                assert_eq!(track.title_parts().1, Some("Slowed & Reverb"));
                assert_eq!(track.display_title(), "CICADATEKK Slowed & Reverb");
            }
        }
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!(false),
            serde_json::json!({"unexpected":1}),
            serde_json::json!(""),
            serde_json::json!("bad\nlabel"),
            serde_json::json!("x".repeat(4097)),
        ] {
            let mut value = base.clone();
            value["subtitle"] = invalid;
            let track: Track = serde_json::from_value(value).unwrap();
            assert!(track.subtitle.is_none());
            assert_eq!(track.display_title(), "CICADATEKK");
        }
        let mut unrelated: Track = serde_json::from_value(base).unwrap();
        unrelated.title = "Song (Live)".into();
        assert_eq!(unrelated.title_parts(), ("Song (Live)", None));
        unrelated.subtitle = Some("Slowed".into());
        assert_eq!(unrelated.title_parts(), ("Song (Live)", Some("Slowed")));
        unrelated.title = "Песня (ЗАМЕДЛЕННАЯ)".into();
        unrelated.subtitle = Some("Замедленная".into());
        assert_eq!(unrelated.title_parts(), ("Песня", Some("Замедленная")));
    }

    #[test]
    fn profile_avatar_is_optional_and_safe_across_old_saved_data_and_vk_responses() {
        let old = serde_json::json!({"id":42,"first_name":"Test","last_name":"User"});
        let profile: Profile = serde_json::from_value(old.clone()).unwrap();
        assert!(profile.avatar_url.is_none());
        assert_eq!(profile.display_name(), "Test User");
        let mut value = old;
        value["avatar"] = serde_json::json!("https://images.example.test/avatar.jpg?a=1&amp;b=2");
        let profile: Profile = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://images.example.test/avatar.jpg?a=1&b=2")
        );
        value["photo_200"] = serde_json::json!("https://images.example.test/200.jpg");
        let profile: Profile = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://images.example.test/200.jpg")
        );
        let restored: Profile =
            serde_json::from_slice(&serde_json::to_vec(&profile).unwrap()).unwrap();
        assert_eq!(restored.avatar_url, profile.avatar_url);
        value["avatar"] = serde_json::Value::Null;
        for invalid in [
            "http://images.example.test/a.jpg",
            "https://localhost/a.jpg",
            "https://127.0.0.1/a.jpg",
            "https://user:secret@images.example.test/a.jpg",
            "file:///tmp/a.jpg",
        ] {
            value["photo_200"] = serde_json::json!(invalid);
            assert!(
                serde_json::from_value::<Profile>(value.clone())
                    .unwrap()
                    .avatar_url
                    .is_none()
            );
        }
        value["photo_200"] = serde_json::json!({"unexpected":"shape"});
        assert!(
            serde_json::from_value::<Profile>(value)
                .unwrap()
                .avatar_url
                .is_none()
        );
    }

    #[test]
    fn accepts_audio_without_a_stream_and_signed_64_bit_owner() {
        let track: Track = serde_json::from_str(
            r#"{"id":7,"owner_id":-9000000000,"artist":"Группа","title":"Трек","duration":245}"#,
        )
        .unwrap();
        assert_eq!(track.key(), (-9_000_000_000, 7));
        assert!(!track.has_stream());
        assert_eq!(track.unavailable, None);
        assert!(track.cover_url.is_none());
        assert_eq!(track.duration_label(), "4:05");
        assert!(track.matches("группа"));
        assert!(!track.matches("other"));
    }

    #[test]
    fn native_restrictions_distinguish_blocked_clear_and_missing_evidence() {
        use serde_json::json;
        let base = json!({"id":7,"owner_id":42,"artist":"Artist","title":"Title","duration":120});
        for (fields, expected) in [
            (json!({}), None),
            (json!({"url":""}), None),
            (
                json!({"url":"https://vk.ru/mp3/audio_api_unavailable.mp3"}),
                None,
            ),
            (
                json!({"is_licensed":false,"legal_notices_type":4,"flags_context":4}),
                None,
            ),
            (json!({"dmca_blocked":true}), Some(true)),
            (json!({"dmca_blocked":1,"content_restricted":0}), Some(true)),
            (json!({"dmca_blocked":false}), Some(false)),
            (json!({"content_restricted":2}), Some(true)),
            (json!({"restrictionStatus":1}), Some(true)),
            (json!({"content_restricted":0}), Some(false)),
            (json!({"content_restricted":"1"}), None),
            (json!({"content_restricted":true}), None),
            (json!({"content_restricted":1.5}), None),
            (json!({"flags":4}), None),
            (json!({"flags":12}), None),
            (json!({"flags":8}), None),
            (json!({"flags":4,"content_restricted":0}), Some(false)),
            (json!({"isClaimed":true}), Some(true)),
            (json!({"unavailable":true}), Some(true)),
            (json!({"unavailable":false}), Some(false)),
        ] {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let track: Track = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(track.unavailable, expected, "{fields}");
            assert_eq!(audio_unavailable_from_object(&value), expected, "{fields}");
        }
    }

    #[test]
    fn reads_album_and_direct_thumbnails_without_requiring_artwork() {
        let mut value = serde_json::json!({"id":7,"owner_id":42,"artist":"Artist","title":"Title","duration":120,
            "album":{"thumb":{"photo_300":"https://images.example.test/300.jpg","photo_600":"https://images.example.test/600.jpg?a=1&amp;b=2"}}});
        let track: Track = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            track.cover_url.as_deref(),
            Some("https://images.example.test/600.jpg?a=1&b=2")
        );
        value["album"] = serde_json::Value::Null;
        value["thumb"] = serde_json::json!({"photo_300":"https://images.example.test/direct.jpg"});
        let track: Track = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            track.cover_url.as_deref(),
            Some("https://images.example.test/direct.jpg")
        );
        value["thumb"] = serde_json::json!({"photo_600":12});
        value["cover_url"] = serde_json::json!(false);
        let track: Track = serde_json::from_value(value).unwrap();
        assert!(track.cover_url.is_none());
    }

    #[test]
    fn keeps_real_artist_identities_including_names_containing_separators() {
        let value = serde_json::json!({
            "id":7,"owner_id":42,"artist":"Earth, Wind & Fire, Guest","title":"Song","duration":120,
            "main_artists":[{"id":"opaque-artist-a","name":"Earth, Wind & Fire"},{"id":12345,"name":"Guest"}],
            "featured_artists":[{"id":"12345","name":"Duplicate"},{"id":"featured-b","name":"Featured"},{"id":"","name":"Invalid"}],
            "album":{"id":9,"owner_id":-42,"title":"Album","access_key":"in-memory-only","thumb":{"photo_600":"https://images.example.test/album.jpg"}}
        });
        let parsed: Track = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.artists, artists_from_object(&value));
        assert_eq!(parsed.artists.len(), 3);
        assert_eq!(parsed.artists[0].name, "Earth, Wind & Fire");
        assert_eq!(parsed.artists[1].id, "12345");
        assert_eq!(parsed.artists[1].name, "Guest");
        assert_eq!(parsed.album, album_from_object(&value));
        let album = parsed.album.unwrap();
        assert_eq!(album.id, 9);
        assert_eq!(album.owner_id, -42);
        assert_eq!(album.access_key.as_deref(), Some("in-memory-only"));
        assert_eq!(
            album.cover_url.as_deref(),
            Some("https://images.example.test/album.jpg")
        );

        let mut old = value;
        old.as_object_mut().unwrap().remove("main_artists");
        old.as_object_mut().unwrap().remove("featured_artists");
        old["album"] =
            serde_json::json!({"thumb":{"photo_600":"https://images.example.test/old.jpg"}});
        let old: Track = serde_json::from_value(old).unwrap();
        assert_eq!(old.artist, "Earth, Wind & Fire, Guest");
        assert!(
            old.artists.is_empty(),
            "Do not invent identities by splitting names"
        );
        assert!(old.album.is_none());
        assert_eq!(
            old.cover_url.as_deref(),
            Some("https://images.example.test/old.jpg")
        );
    }

    #[test]
    fn malformed_optional_artist_and_album_metadata_does_not_hide_tracks() {
        let value = serde_json::json!({
            "id":7,"owner_id":42,"artist":"Raw artist","title":"Song","duration":120,
            "main_artists":false,
            "featured_artists":[{"id":"../unsafe","name":"Invalid"},{"id":"real-id","name":"Valid"}],
            "album":{"id":0,"owner_id":42,"title":"Invalid"}
        });
        let track: Track = serde_json::from_value(value).unwrap();
        assert_eq!(track.artists.len(), 1);
        assert_eq!(track.artists[0].id, "real-id");
        assert!(track.album.is_none());
    }

    #[test]
    fn ignores_unrelated_images_and_rejects_invalid_cover_urls() {
        let value =
            serde_json::json!({"artist":{"photo_600":"https://images.example.test/artist.jpg"}});
        assert!(cover_from_object(&value).is_none());
        for url in [
            "file:///tmp/cover.jpg",
            "http://images.example.test/cover.jpg",
            "https://localhost/cover.jpg",
            "https://127.0.0.1/cover.jpg",
            "https://user:pass@images.example.test/cover.jpg",
            "https://images.example.test/cover.jpg\n",
        ] {
            assert!(valid_cover_url(url).is_none());
        }
    }
}
