//! Minimal parser for the modern music catalog. Only explicitly referenced
//! library audio is displayed; aggregated helper/recent records are not a playlist.
use crate::model::Track;
use reqwest::Url;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub enum CatalogError {
    InvalidResponse,
    UnsupportedSection,
    MissingTrack,
}

pub struct CatalogPage {
    pub items: Vec<Track>,
    pub next_from: Option<String>,
}

fn vk_url(raw: &str) -> Option<Url> {
    let base = Url::parse("https://vk.ru/").ok()?;
    let url = base.join(raw).ok()?;
    (url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("vk.ru" | "vk.com" | "www.vk.ru" | "www.vk.com")
        ))
    .then_some(url)
}

pub fn select_section(value: Value, owner_id: i64) -> Result<String, CatalogError> {
    let sections = value["catalog"]["sections"]
        .as_array()
        .ok_or(CatalogError::InvalidResponse)?;
    let mut choices = Vec::new();
    for section in sections {
        let Some(id) = section["id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let title = section["title"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let named_library = matches!(
            title.as_str(),
            "моя музыка" | "мои аудиозаписи" | "my music" | "my audios"
        );
        let raw = section["url"].as_str().unwrap_or_default();
        let mut all = false;
        let mut own = false;
        let mut wrong = false;
        if !raw.is_empty() {
            let Some(url) = vk_url(raw) else { continue };
            if let Some(owner) = url
                .path()
                .strip_prefix("/audios")
                .filter(|part| !part.is_empty())
                && let Ok(id) = owner.parse::<i64>()
            {
                own = id == owner_id;
                wrong = id != owner_id;
            }
            let library_path = own || matches!(url.path(), "/audio" | "/audios");
            for (key, val) in url.query_pairs() {
                if key == "owner_id"
                    && let Ok(id) = val.parse::<i64>()
                {
                    own |= id == owner_id;
                    wrong |= id != owner_id;
                }
                if key == "section" {
                    all = val == "all" && library_path;
                    wrong |= matches!(
                        val.as_ref(),
                        "recoms" | "recommendations" | "popular" | "new" | "updates" | "friends"
                    );
                }
            }
        }
        if wrong {
            continue;
        }
        let rank = if all {
            0
        } else if named_library {
            1
        } else if own {
            2
        } else {
            continue;
        };
        choices.push((rank, id.to_owned()));
    }
    choices.sort_by_key(|(rank, _)| *rank);
    choices
        .into_iter()
        .next()
        .map(|(_, id)| id)
        .ok_or(CatalogError::UnsupportedSection)
}

pub fn parse_page(value: Value) -> Result<CatalogPage, CatalogError> {
    let section = value
        .get("section")
        .filter(|section| section.is_object())
        .ok_or(CatalogError::InvalidResponse)?;
    let next_from = match section.get("next_from") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.is_empty() => None,
        Some(Value::String(value)) => Some(value.clone()),
        _ => return Err(CatalogError::InvalidResponse),
    };
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    if let Some(blocks) = section.get("blocks") {
        for block in blocks.as_array().ok_or(CatalogError::InvalidResponse)? {
            if block["data_type"] != "music_audios" {
                continue;
            }
            let url = block["url"].as_str().ok_or(CatalogError::InvalidResponse)?;
            let recent = vk_url(url).is_some_and(|url| {
                url.query_pairs()
                    .any(|(key, val)| key == "block" && val == "recent")
            });
            if recent {
                continue;
            }
            let ids = block["audios_ids"]
                .as_array()
                .ok_or(CatalogError::InvalidResponse)?;
            for id in ids {
                let id = id.as_str().ok_or(CatalogError::InvalidResponse)?;
                if seen.insert(id.to_owned()) {
                    order.push(id.to_owned());
                }
            }
        }
    }
    let audios = match value.get("audios") {
        None => &[][..],
        Some(value) => value
            .as_array()
            .ok_or(CatalogError::InvalidResponse)?
            .as_slice(),
    };
    if order.is_empty() && !audios.is_empty() {
        return Err(CatalogError::InvalidResponse);
    }
    let mut pool = HashMap::new();
    for audio in audios {
        let (Some(owner), Some(id)) = (audio["owner_id"].as_i64(), audio["id"].as_i64()) else {
            continue;
        };
        let key = format!("{owner}_{id}");
        if seen.contains(&key) {
            let track: Track =
                serde_json::from_value(audio.clone()).map_err(|_| CatalogError::InvalidResponse)?;
            pool.insert(key, track);
        }
    }
    let items = order
        .into_iter()
        .map(|id| pool.remove(&id).ok_or(CatalogError::MissingTrack))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CatalogPage { items, next_from })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn track(id: i64) -> Value {
        json!({"owner_id":42,"id":id,"title":"Test","artist":"Artist","duration":123})
    }
    #[test]
    fn chooses_own_library_instead_of_recommended_default() {
        let value = json!({"catalog":{"default_section":"recommendations","sections":[
            {"id":"recommendations","title":"For you","url":"/audio?section=recoms"},
            {"id":"own","title":"Моя музыка","url":"/audios42?section=all"}
        ]}});
        assert_eq!(select_section(value, 42).unwrap(), "own");
        let other = json!({"catalog":{"sections":[{"id":"wrong","title":"My music","url":"/audios99?section=all"}]}});
        assert!(select_section(other, 42).is_err());
    }
    #[test]
    fn follows_block_order_and_ignores_recent_and_auxiliary_audio() {
        let page = parse_page(json!({"section":{"next_from":"opaque:cursor/+1","blocks":[
            {"data_type":"music_audios","url":"/audio?section=all&block=recent","audios_ids":["42_3"]},
            {"data_type":"music_audios","url":"/audio?section=all&block=all","audios_ids":["42_2","42_1"]}
        ]},"audios":[track(1),track(3),track(2),track(4)]})).unwrap();
        assert_eq!(
            page.items.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(page.next_from.as_deref(), Some("opaque:cursor/+1"));
    }
    #[test]
    fn missing_referenced_track_is_not_reported_as_empty_library() {
        let page = json!({"section":{"blocks":[{"data_type":"music_audios","url":"/audio?block=all","audios_ids":["42_2"]}]},"audios":[track(1)]});
        assert!(matches!(parse_page(page), Err(CatalogError::MissingTrack)));
    }
    #[test]
    fn accepts_genuinely_empty_library_without_a_next_cursor() {
        let page =
            parse_page(json!({"section":{"blocks":[],"next_from":null},"audios":[]})).unwrap();
        assert!(page.items.is_empty());
        assert!(page.next_from.is_none());
    }
}
