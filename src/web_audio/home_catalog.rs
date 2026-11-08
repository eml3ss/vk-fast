//! VK's catalog Home blocks that the legacy `al_audio.php` Home lacks: genre
//! and mood mixes, friends' playlists with a taste match, and artist mixes.
//! Blocks are identified by VK's own block metadata, never by header text.
use super::*;

const MAX_TILES: usize = 48;
const MAX_FRIENDS: usize = 24;
const MAX_OPTION_LEN: usize = 64;

/// A server-advertised stream mix started by `play_vk_mix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogMix {
    /// `mix_id=common` with the tile's options, e.g. `additionals: pop`.
    Common { options: Vec<(String, String)> },
    /// `mix_id=artist_mix` with VK's opaque artist-mix entity.
    Artist { entity_id: String },
}

impl CatalogMix {
    /// VK's player sends the tile options as `{"id":<session>,"key":[value]}`;
    /// the random session id stays the same for every batch of one mix.
    pub(super) fn params(&self, session: u64) -> Vec<(&'static str, String)> {
        match self {
            Self::Common { options } => {
                let mut json = format!("{{\"id\":{session}");
                for (key, value) in options {
                    json.push(',');
                    json.push_str(&Value::String(key.clone()).to_string());
                    json.push_str(":[");
                    json.push_str(&Value::String(value.clone()).to_string());
                    json.push(']');
                }
                json.push('}');
                vec![("mix_id", "common".into()), ("options", json)]
            }
            Self::Artist { entity_id } => vec![
                ("mix_id", "artist_mix".into()),
                ("entity_id", entity_id.clone()),
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixTile {
    pub id: String,
    pub title: String,
    pub mix: CatalogMix,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistMix {
    pub title: String,
    pub description: String,
    pub photo_url: Option<String>,
    pub mix: CatalogMix,
}

#[derive(Clone)]
pub struct FriendPlaylist {
    pub playlist: AlbumRef,
    pub owner_name: String,
    pub owner_photo: Option<String>,
    /// VK's "match with your taste", 0–100.
    pub match_percent: Option<u8>,
    pub color: Option<[u8; 3]>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Default)]
pub struct HomeCatalog {
    pub genres: Vec<MixTile>,
    pub moods: Vec<MixTile>,
    pub friends: Vec<FriendPlaylist>,
    pub artist_mixes: Vec<ArtistMix>,
}

#[cfg(test)]
impl HomeCatalog {
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty()
            && self.moods.is_empty()
            && self.friends.is_empty()
            && self.artist_mixes.is_empty()
    }
}

fn option_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_OPTION_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

/// Mirrors VK's own check: an object whose values are arrays of strings. Only
/// the first value per option is sent, exactly like the first-party player.
fn mix_options(raw: &str) -> Option<Vec<(String, String)>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    let mut options = Vec::new();
    for (key, values) in object {
        let first = values.as_array()?.first()?.as_str()?;
        if !option_value(key) || !option_value(first) || key == "id" {
            return None;
        }
        options.push((key.clone(), first.to_owned()));
    }
    (!options.is_empty() && options.len() <= 4).then_some(options)
}

fn text(value: &Value, key: &str, limit: usize) -> Option<String> {
    let text = value.get(key)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.chars().take(limit).collect())
}

fn mix_actions(block: &Value) -> impl Iterator<Item = &Value> {
    block
        .get("actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|action| {
            action.pointer("/action/type").and_then(Value::as_str) == Some("play_vk_mix")
        })
}

fn mix_tiles(block: &Value) -> Vec<MixTile> {
    let mut seen = HashSet::new();
    mix_actions(block)
        .filter(|action| action.get("mix_id").and_then(Value::as_str) == Some("common"))
        .filter_map(|action| {
            let id = text(action, "id", 64)?;
            let title = text(action, "title", 48)?;
            let options = mix_options(action.get("mix_options")?.as_str()?)?;
            seen.insert(id.clone()).then_some(MixTile {
                id,
                title,
                mix: CatalogMix::Common { options },
            })
        })
        .take(MAX_TILES)
        .collect()
}

/// Prefer a square image near the card's on-screen size.
fn image_near(images: Option<&Value>, edge: i64) -> Option<String> {
    let mut best: Option<(i64, String)> = None;
    for image in images.and_then(Value::as_array).into_iter().flatten() {
        let Some(url) = image
            .get("url")
            .and_then(Value::as_str)
            .and_then(valid_cover_url)
        else {
            continue;
        };
        let width = image.get("width").and_then(number).unwrap_or(0);
        let score = if width >= edge {
            width - edge
        } else {
            (edge - width) * 4
        };
        if best.as_ref().is_none_or(|(current, _)| score < *current) {
            best = Some((score, url));
        }
    }
    best.map(|(_, url)| url)
}

fn artist_mixes(block: &Value) -> Vec<ArtistMix> {
    let mut seen = HashSet::new();
    mix_actions(block)
        .filter(|action| action.get("mix_id").and_then(Value::as_str) == Some("artist_mix"))
        .filter_map(|action| {
            let entity_id = text(action, "entity_id", 64).filter(|id| option_value(id))?;
            let title = text(action, "title", 96)?;
            seen.insert(entity_id.clone()).then(|| ArtistMix {
                title,
                description: text(action, "description", 160).unwrap_or_default(),
                photo_url: image_near(action.get("foreground_images"), 240),
                mix: CatalogMix::Artist { entity_id },
            })
        })
        .take(MAX_TILES)
        .collect()
}

fn pair(value: &str) -> Option<(i64, i64)> {
    let (owner, id) = value.split_once('_')?;
    Some((owner.parse().ok()?, id.parse().ok()?))
}

fn color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |range| u8::from_str_radix(hex.get(range)?, 16).ok();
    Some([channel(0..2)?, channel(2..4)?, channel(4..6)?])
}

fn owner_identity(value: &Value, owner: i64) -> (String, Option<String>) {
    let (collection, id) = if owner > 0 {
        ("profiles", owner)
    } else {
        ("groups", -owner)
    };
    let Some(entry) = value
        .get(collection)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.get("id").and_then(number) == Some(id))
    else {
        return (String::new(), None);
    };
    let name = if owner > 0 {
        [text(entry, "first_name", 64), text(entry, "last_name", 64)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        text(entry, "name", 128).unwrap_or_default()
    };
    let photo = ["photo_50", "photo_100"].into_iter().find_map(|key| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .and_then(valid_cover_url)
    });
    (name, photo)
}

fn lookup<'a>(list: &[&'a Value], key: (i64, i64)) -> Option<&'a Value> {
    list.iter().copied().find(|entry| {
        entry.get("owner_id").and_then(number) == Some(key.0)
            && entry.get("id").and_then(number) == Some(key.1)
    })
}

fn friends(
    value: &Value,
    block: &Value,
    tracks: &HashMap<(i64, i64), Track>,
) -> Vec<FriendPlaylist> {
    let entries: Vec<&Value> = value
        .get("recommended_playlists")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let playlists: Vec<&Value> = value
        .get("playlists")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let mut seen = HashSet::new();
    block
        .get("playlists_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(pair)
        .filter(|key| key.0 != 0 && key.1 > 0 && seen.insert(*key))
        .filter_map(|key| {
            let entry = lookup(&entries, key)?;
            let playlist = lookup(&playlists, key);
            let title = playlist.and_then(|playlist| text(playlist, "title", 128))?;
            let (owner_name, owner_photo) = owner_identity(value, key.0);
            let preview = entry
                .get("audios")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(pair)
                .filter_map(|audio| tracks.get(&audio).cloned())
                .take(3)
                .collect();
            Some(FriendPlaylist {
                playlist: AlbumRef {
                    id: key.1,
                    owner_id: key.0,
                    title,
                    access_key: playlist.and_then(|playlist| text(playlist, "access_key", 128)),
                    cover_url: entry
                        .get("cover")
                        .and_then(Value::as_str)
                        .and_then(valid_cover_url),
                },
                owner_name,
                owner_photo,
                match_percent: entry
                    .get("percentage")
                    .and_then(Value::as_f64)
                    .filter(|share| share.is_finite() && (0.0..=1.0).contains(share))
                    .map(|share| (share * 100.0).round() as u8),
                color: entry.get("color").and_then(Value::as_str).and_then(color),
                tracks: preview,
            })
        })
        .take(MAX_FRIENDS)
        .collect()
}

pub(super) fn parse_home_catalog(value: &Value) -> Result<(HomeCatalog, StreamMap), WebAudioError> {
    let sections = value
        .pointer("/catalog/sections")
        .and_then(Value::as_array)
        .ok_or(WebAudioError::Format)?;
    let default = value
        .pointer("/catalog/default_section")
        .and_then(Value::as_str);
    let section = sections
        .iter()
        .find(|section| default.is_some() && section.get("id").and_then(Value::as_str) == default)
        .or_else(|| sections.first())
        .ok_or(WebAudioError::Format)?;
    // One unavailable or unusual row must not hide every other Home block.
    let mut tracks = HashMap::new();
    let mut streams = StreamMap::new();
    for raw in value
        .get("audios")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_QUEUED_TRACKS)
    {
        if let Ok(track) = parse_track(raw) {
            streams.insert(track.key(), stream_info(raw, track.key()));
            tracks.entry(track.key()).or_insert(track);
        }
    }
    let mut catalog = HomeCatalog::default();
    for block in section
        .get("blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let data_type = block.get("data_type").and_then(Value::as_str);
        let anchor = block.pointer("/meta/anchor").and_then(Value::as_str);
        let style = block.pointer("/layout/style").and_then(Value::as_str);
        match (data_type, anchor, style) {
            (Some("action"), _, Some("artist_mix")) if catalog.artist_mixes.is_empty() => {
                catalog.artist_mixes = artist_mixes(block);
            }
            (Some("action"), Some("genres"), _) if catalog.genres.is_empty() => {
                catalog.genres = mix_tiles(block);
            }
            (Some("action"), Some("vibes"), _) if catalog.moods.is_empty() => {
                catalog.moods = mix_tiles(block);
            }
            (Some("music_recommended_playlists"), _, _) if catalog.friends.is_empty() => {
                catalog.friends = friends(value, block, &tracks);
            }
            _ => {}
        }
    }
    Ok((catalog, streams))
}

pub(super) type StreamMap = HashMap<(i64, i64), StreamInfo>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn audio(owner: i64, id: i64, title: &str) -> Value {
        json!({
            "owner_id": owner, "id": id, "title": title, "artist": "Synthetic Artist",
            "duration": 120, "url": format!("https://audio.example.test/{owner}_{id}.mp3"),
            "access_key": "synthetic"
        })
    }

    fn catalog() -> Value {
        json!({
            "audios": [audio(7, 1, "First"), audio(7, 2, "Second"), audio(7, 3, "Third"), audio(7, 4, "Fourth")],
            "playlists": [{"owner_id": 7, "id": 36, "title": "Friend list", "access_key": "synthetic-key"}],
            "profiles": [{"id": 7, "first_name": "Synthetic", "last_name": "Friend",
                          "photo_50": "https://images.example.test/friend.jpg"}],
            "recommended_playlists": [{
                "owner_id": 7, "id": 36, "percentage": 0.96, "color": "#3681FF",
                "cover": "https://images.example.test/cover.png",
                "audios": ["7_1", "7_2", "7_3", "7_4"]
            }],
            "catalog": {
                "default_section": "home",
                "sections": [
                    {"id": "other", "blocks": []},
                    {"id": "home", "blocks": [
                        {"data_type": "none", "layout": {"name": "header_extended", "title": "Anything"}},
                        {"data_type": "action", "meta": {"anchor": "genres"}, "layout": {"name": "crop_slider"},
                         "actions": [
                            {"action": {"type": "play_vk_mix"}, "id": "pop_genre", "title": "Поп",
                             "mix_id": "common", "mix_options": "{\"additionals\":[\"pop\"]}"},
                            {"action": {"type": "play_vk_mix"}, "id": "pop_genre", "title": "Duplicate",
                             "mix_id": "common", "mix_options": "{\"additionals\":[\"pop\"]}"},
                            {"action": {"type": "open_url"}, "id": "link", "title": "Not a mix",
                             "mix_id": "common", "mix_options": "{\"additionals\":[\"x\"]}"},
                            {"action": {"type": "play_vk_mix"}, "id": "bad", "title": "Injected",
                             "mix_id": "common", "mix_options": "{\"additionals\":[\"a\\\"b\"]}"}
                         ]},
                        {"data_type": "action", "meta": {"anchor": "vibes"}, "layout": {"name": "crop_slider"},
                         "actions": [{"action": {"type": "play_vk_mix"}, "id": "happy_vibe", "title": "Радостно",
                                      "mix_id": "common", "mix_options": "{\"vibes\":[\"happy\"]}"}]},
                        {"data_type": "music_recommended_playlists", "meta": {"anchor": "listenothers"},
                         "playlists_ids": ["7_36", "7_99"]},
                        {"data_type": "action", "layout": {"name": "large_slider", "style": "artist_mix"},
                         "actions": [{"action": {"type": "play_vk_mix", "style": "artist_mix"},
                                      "entity_id": "3424231346320955355", "title": "Synthetic Artist",
                                      "description": "A, B, C", "mix_id": "artist_mix",
                                      "foreground_images": [
                                        {"width": 72, "height": 72, "url": "https://images.example.test/72.jpg"},
                                        {"width": 240, "height": 240, "url": "https://images.example.test/240.jpg"},
                                        {"width": 1080, "height": 1080, "url": "https://images.example.test/1080.jpg"}]}]}
                    ]}
                ]
            }
        })
    }

    #[test]
    fn catalog_blocks_are_recognized_by_vk_metadata_and_validated() {
        let (catalog, streams) = parse_home_catalog(&catalog()).unwrap();
        assert_eq!(
            catalog.genres,
            [MixTile {
                id: "pop_genre".into(),
                title: "Поп".into(),
                mix: CatalogMix::Common {
                    options: vec![("additionals".into(), "pop".into())]
                },
            }]
        );
        assert_eq!(catalog.moods.len(), 1);
        assert_eq!(catalog.moods[0].title, "Радостно");
        let friend = &catalog.friends[..];
        assert_eq!(friend.len(), 1, "an unknown playlist id is skipped");
        assert_eq!(friend[0].playlist.title, "Friend list");
        assert_eq!(
            friend[0].playlist.access_key.as_deref(),
            Some("synthetic-key")
        );
        assert_eq!(friend[0].owner_name, "Synthetic Friend");
        assert_eq!(friend[0].match_percent, Some(96));
        assert_eq!(friend[0].color, Some([0x36, 0x81, 0xFF]));
        assert_eq!(
            friend[0].tracks.iter().map(Track::key).collect::<Vec<_>>(),
            [(7, 1), (7, 2), (7, 3)]
        );
        assert_eq!(catalog.artist_mixes.len(), 1);
        assert_eq!(
            catalog.artist_mixes[0].photo_url.as_deref(),
            Some("https://images.example.test/240.jpg")
        );
        assert!(streams.contains_key(&(7, 1)));
    }

    #[test]
    fn mix_requests_match_the_first_party_player() {
        let genre = CatalogMix::Common {
            options: vec![("additionals".into(), "pop".into())],
        };
        assert_eq!(
            genre.params(3_342_033_616_310),
            [
                ("mix_id", "common".to_owned()),
                (
                    "options",
                    r#"{"id":3342033616310,"additionals":["pop"]}"#.to_owned()
                )
            ]
        );
        let artist = CatalogMix::Artist {
            entity_id: "3424231346320955355".into(),
        };
        assert_eq!(
            artist.params(1),
            [
                ("mix_id", "artist_mix".to_owned()),
                ("entity_id", "3424231346320955355".to_owned())
            ]
        );
        assert_eq!(mix_options(r#"{"id":["x"]}"#), None);
        assert_eq!(mix_options(r#"{"vibes":"happy"}"#), None);
        assert_eq!(mix_options("[]"), None);
    }

    #[test]
    fn a_catalog_without_sections_is_a_format_error() {
        assert!(matches!(
            parse_home_catalog(&json!({"audios": []})),
            Err(WebAudioError::Format)
        ));
        let (empty, _) =
            parse_home_catalog(&json!({"catalog": {"sections": [{"id": "x", "blocks": []}]}}))
                .unwrap();
        assert!(empty.is_empty());
    }
}
