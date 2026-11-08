//! Artist release navigation follows opaque section/block references returned by VK.
use crate::{
    artist_releases::{self, Kind, Release},
    web_audio::{ArtistPage, WebAudioError},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_REQUESTS: usize = 64;
const MAX_RELEASES: usize = 10_000;

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
enum Group {
    #[default]
    All,
    Albums,
    Singles,
}
impl Group {
    fn kind(self) -> Kind {
        match self {
            Self::All => Kind::Unknown,
            Self::Albums => Kind::Album,
            Self::Singles => Kind::Single,
        }
    }
}

struct Request {
    method: &'static str,
    params: Vec<(&'static str, String)>,
    group: Group,
    section: Option<String>,
}
#[derive(Default)]
struct Reader {
    pool: HashMap<String, Release>,
    pending: VecDeque<Request>,
    seen: HashSet<(&'static str, String, String, Group)>,
    cursor_loop: bool,
    active_continuation: Option<(&'static str, String)>,
}

fn opaque(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|text| {
        !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control)
    })
}
fn group(text: &str) -> Option<Group> {
    let text = text.trim().to_lowercase().replace('&', "and");
    match text.as_str() {
        "albums" | "all albums" | "альбомы" | "все альбомы" => Some(Group::Albums),
        "singles"
        | "singles and eps"
        | "singles and ep"
        | "singles & eps"
        | "синглы"
        | "синглы и ep"
        | "синглы и мини-альбомы" => Some(Group::Singles),
        "all"
        | "all releases"
        | "latest releases"
        | "releases"
        | "discography"
        | "все"
        | "всё"
        | "релизы"
        | "все релизы"
        | "последние релизы"
        | "дискография" => Some(Group::All),
        _ => None,
    }
}
fn group_of(value: &Value) -> Option<Group> {
    [
        value.get("title"),
        value.get("text"),
        value.pointer("/layout/title"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find_map(group)
    .or_else(
        || match value.pointer("/layout/style").and_then(Value::as_str) {
            Some("artist_albums") => Some(Group::Albums),
            Some("artist_singles" | "artist_eps") => Some(Group::Singles),
            Some("artist_releases") => Some(Group::All),
            _ => None,
        },
    )
}
fn inherited_group(value: &Value, context: Group) -> Group {
    match group_of(value) {
        Some(Group::All) | None => context,
        Some(category) => category,
    }
}
fn unrelated(block: &Value) -> bool {
    [block.get("title"), block.pointer("/layout/style")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|text| {
            let text = text.to_lowercase();
            [
                "featuring_releases",
                "appears on",
                "similar",
                "recommend",
                "похож",
                "рекоменд",
                "участи",
            ]
            .iter()
            .any(|word| text.contains(word))
        })
}

impl Reader {
    fn pool(&mut self, value: &Value) -> Result<(), WebAudioError> {
        for release in [value.get("playlists"), value.get("albums")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(Release::from_value)
        {
            let key = format!("{}_{}", release.album.owner_id, release.album.id);
            if let Some(existing) = self.pool.get_mut(&key) {
                existing.enrich(release);
            } else {
                self.pool.insert(key, release);
            }
            if self.pool.len() > MAX_RELEASES {
                return Err(WebAudioError::TooLarge);
            }
        }
        Ok(())
    }

    fn enqueue(
        &mut self,
        method: &'static str,
        key: &'static str,
        id: &str,
        cursor: Option<&str>,
        group: Group,
        section: Option<&str>,
    ) {
        if self
            .seen
            .insert((method, id.into(), cursor.unwrap_or_default().into(), group))
        {
            let mut params = vec![(key, id.into()), ("extended", "1".into())];
            if method == "catalog.getSection" {
                params.push(("need_blocks", "1".into()));
            }
            if let Some(cursor) = cursor {
                params.push(("start_from", cursor.into()));
            }
            self.pending.push_back(Request {
                method,
                params,
                group,
                section: section.map(str::to_owned),
            });
        } else if cursor.is_some()
            && self
                .active_continuation
                .as_ref()
                .is_some_and(|(active_method, active_id)| {
                    *active_method == method && active_id == id
                })
        {
            self.cursor_loop = true;
        }
    }

    fn actions(&mut self, block: &Value, context: Group, allow_open: bool, section: Option<&str>) {
        if allow_open && let Some(id) = opaque(block.pointer("/meta/show_all_info/section_id")) {
            self.enqueue(
                "catalog.getSection",
                "section_id",
                id,
                None,
                context,
                Some(id),
            );
        }
        for button in [block.get("actions"), block.get("buttons")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_array)
            .flatten()
        {
            let action = button.get("action").unwrap_or(button);
            let kind = action.get("type").and_then(Value::as_str);
            if matches!(kind, Some("open_section" | "switch_section"))
                && (allow_open || group_of(button).is_some())
                && let Some(id) = opaque(
                    button
                        .get("section_id")
                        .or_else(|| action.get("section_id")),
                )
            {
                self.enqueue(
                    "catalog.getSection",
                    "section_id",
                    id,
                    None,
                    group_of(button).unwrap_or(context),
                    Some(id),
                );
            }
            for option in button
                .get("options")
                .or_else(|| action.get("options"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(category) = group_of(option) else {
                    continue;
                };
                if option.get("selected").is_some_and(|selected| {
                    selected.as_bool() == Some(true) || selected.as_u64() == Some(1)
                }) {
                    continue;
                }
                if let Some(id) = opaque(option.get("replacement_id")) {
                    self.enqueue(
                        "catalog.replaceBlocks",
                        "replacement_ids",
                        id,
                        None,
                        category,
                        section,
                    );
                } else if let Some(id) = opaque(option.get("section_id")) {
                    self.enqueue(
                        "catalog.getSection",
                        "section_id",
                        id,
                        None,
                        category,
                        Some(id),
                    );
                }
            }
        }
    }

    fn block(
        &mut self,
        block: &Value,
        context: Group,
        dedicated: bool,
        section: Option<&str>,
        page: &mut ArtistPage,
    ) -> Result<(), WebAudioError> {
        if unrelated(block) {
            return Ok(());
        }
        let category = inherited_group(block, context);
        let releases = block.get("data_type").and_then(Value::as_str) == Some("music_playlists");
        let marked = block.pointer("/layout/style").and_then(Value::as_str)
            == Some("artist_releases")
            || group_of(block).is_some();
        if releases && (dedicated || marked) {
            let ids = block
                .get("playlists_ids")
                .or_else(|| block.get("playlist_ids"))
                .and_then(Value::as_array);
            if let Some(ids) = ids {
                let mut batch = Vec::new();
                for key in ids {
                    let key = key.as_str().ok_or(WebAudioError::Format)?;
                    let mut release = self.pool.get(key).cloned().ok_or(WebAudioError::Format)?;
                    if release.kind == Kind::Unknown {
                        release.kind = category.kind();
                    }
                    batch.push(release);
                }
                artist_releases::merge(&mut page.discography, batch);
            }
            if let Some(cursor) = opaque(block.get("next_from")) {
                let id = opaque(block.get("id")).ok_or(WebAudioError::Format)?;
                self.enqueue(
                    "catalog.getBlockItems",
                    "block_id",
                    id,
                    Some(cursor),
                    category,
                    section,
                );
            }
        }
        if dedicated || marked {
            self.actions(block, category, releases || marked, section);
        }
        Ok(())
    }

    fn section(
        &mut self,
        section: &Value,
        context: Group,
        dedicated: bool,
        page: &mut ArtistPage,
    ) -> Result<(), WebAudioError> {
        let category = inherited_group(section, context);
        let selected = section
            .get("blocks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|block| [block.get("actions"), block.get("buttons")])
            .flatten()
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|button| {
                button
                    .get("options")
                    .or_else(|| button.pointer("/action/options"))
                    .and_then(Value::as_array)
            })
            .flatten()
            .find(|option| {
                option.get("selected").is_some_and(|selected| {
                    selected.as_bool() == Some(true) || selected.as_u64() == Some(1)
                })
            })
            .and_then(group_of);
        let category = if category == Group::All {
            selected.unwrap_or(category)
        } else {
            category
        };
        if let Some(blocks) = section.get("blocks").and_then(Value::as_array) {
            if let Some(id) = opaque(section.get("id")) {
                self.seen
                    .insert(("catalog.getSection", id.into(), String::new(), category));
            }
            for block in blocks {
                self.block(block, category, dedicated, opaque(section.get("id")), page)?;
            }
        } else if let Some(id) = opaque(section.get("id")) {
            self.enqueue(
                "catalog.getSection",
                "section_id",
                id,
                None,
                category,
                Some(id),
            );
        }
        if dedicated && let Some(cursor) = opaque(section.get("next_from")) {
            let id = opaque(section.get("id")).ok_or(WebAudioError::Format)?;
            self.enqueue(
                "catalog.getSection",
                "section_id",
                id,
                Some(cursor),
                category,
                Some(id),
            );
        }
        Ok(())
    }

    fn payload(
        &mut self,
        value: &Value,
        context: Group,
        section: Option<&str>,
        page: &mut ArtistPage,
    ) -> Result<(), WebAudioError> {
        self.pool(value)?;
        if let Some(section) = value.get("section") {
            return self.section(section, context, true, page);
        }
        if let Some(block) = value.get("block") {
            return self.block(block, context, true, section, page);
        }
        if let Some(replacements) = value
            .pointer("/replacements/replacements")
            .and_then(Value::as_array)
        {
            for replacement in replacements {
                for block in replacement
                    .get("to_blocks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    self.block(block, context, true, section, page)?;
                }
            }
            if let (Some(id), Some(cursor)) = (
                section,
                opaque(value.pointer("/replacements/new_next_from")),
            ) {
                self.enqueue(
                    "catalog.getSection",
                    "section_id",
                    id,
                    Some(cursor),
                    context,
                    Some(id),
                );
            }
            return Ok(());
        }
        let sections = value
            .pointer("/catalog/sections")
            .and_then(Value::as_array)
            .ok_or(WebAudioError::Format)?;
        let default = value
            .pointer("/catalog/default_section")
            .and_then(Value::as_str);
        let Some(section) = sections
            .iter()
            .find(|section| section.get("id").and_then(Value::as_str) == default)
            .or_else(|| sections.first())
        else {
            return Ok(());
        };
        // If the server keeps the artist overview, use only its release blocks.
        let overview = section
            .get("blocks")
            .and_then(Value::as_array)
            .is_some_and(|blocks| blocks.iter().any(|block| block["data_type"] == "artist"));
        self.section(section, context, !overview, page)
    }
}

pub fn load(
    page: &mut ArtistPage,
    initial: &Value,
    canceled: &AtomicBool,
    mut request: impl FnMut(&'static str, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
) -> Result<(), WebAudioError> {
    let mut reader = Reader::default();
    reader.pool(initial)?;
    if canceled.load(Ordering::Acquire) {
        return Err(WebAudioError::Canceled);
    }
    // Prefer the artist page's own Show all and continuation references.
    if initial.get("catalog").is_some() || initial.get("section").is_some() {
        reader.payload(initial, Group::All, None, page)?;
    }
    let mut count = 0;
    if reader.pending.is_empty() {
        let domain = initial
            .get("artists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|artist| {
                artist.get("id").is_some_and(|id| {
                    id.as_str() == Some(page.artist.id.as_str())
                        || id
                            .as_i64()
                            .is_some_and(|id| id.to_string() == page.artist.id)
                })
            })
            .and_then(|artist| artist.get("domain"))
            .and_then(Value::as_str)
            .filter(|domain| crate::model::valid_artist_id(domain))
            .unwrap_or(&page.artist.id);
        let root = request(
            "catalog.getAudioArtist",
            vec![
                ("artist_id", page.artist.id.clone()),
                ("need_blocks", "1".into()),
                ("url", format!("https://vk.ru/artist/{domain}/releases")),
            ],
        )?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        reader.seen.clear();
        reader.payload(&root, Group::All, None, page)?;
        count = 1;
    }
    let mut failed = None;
    while let Some(next) = reader.pending.pop_front() {
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        if count >= MAX_REQUESTS || reader.cursor_loop {
            return Err(WebAudioError::Pagination);
        }
        let continuation = next
            .params
            .iter()
            .any(|(key, _)| *key == "start_from")
            .then(|| (next.method, next.params[0].1.clone()));
        let payload = match request(next.method, next.params) {
            Ok(payload) => payload,
            Err(
                error @ (WebAudioError::Canceled
                | WebAudioError::WrongAccount
                | WebAudioError::Unverified
                | WebAudioError::Expired
                | WebAudioError::NoSession),
            ) => return Err(error),
            Err(error) => {
                failed.get_or_insert(error);
                count += 1;
                continue;
            }
        };
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        reader.active_continuation = continuation;
        reader.payload(&payload, next.group, next.section.as_deref(), page)?;
        count += 1;
    }
    for latest in &mut page.latest_releases {
        if let Some(fresh) = page.discography.iter().find(|release| {
            release.album.owner_id == latest.album.owner_id && release.album.id == latest.album.id
        }) {
            latest.enrich(fresh.clone());
        }
    }
    if page.latest_releases.is_empty() {
        page.latest_releases = page.discography.iter().take(10).cloned().collect();
    }
    if reader.cursor_loop {
        return Err(WebAudioError::Pagination);
    }
    failed.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArtistRef;
    use serde_json::json;

    fn page() -> ArtistPage {
        ArtistPage {
            artist: ArtistRef {
                id: "synthetic-artist".into(),
                name: "Synthetic artist".into(),
            },
            image_url: None,
            tracks: Vec::new(),
            latest_releases: Vec::new(),
            discography: Vec::new(),
            releases_error: None,
            releases_pending: false,
            related: Vec::new(),
        }
    }
    fn release(id: i64, metadata: Value) -> Value {
        let mut value = json!({"id":id,"owner_id":-42,"title":format!("Release {id}")});
        value
            .as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        value
    }

    #[test]
    fn follows_catalog_sections_tabs_and_cursors_without_legacy_audio_calls() {
        let mut page = page();
        let mut calls = Vec::new();
        load(&mut page,&json!({}),&AtomicBool::new(false),|method,params| {
            calls.push(method);
            let get=|key|params.iter().find(|(name,_)|*name==key).map(|(_,value)|value.as_str());
            Ok(match method {
                "catalog.getAudioArtist" => {
                    assert_eq!(get("url"),Some("https://vk.ru/artist/synthetic-artist/releases"));
                    json!({"catalog":{"default_section":"s","sections":[{"id":"s"}]}})
                }
                "catalog.getSection" if get("start_from").is_none() => json!({
                    "section":{"id":"s","title":"Releases","blocks":[
                        {"data_type":"none","buttons":[{"action":{"type":"subsection_tabs"},"options":[
                            {"text":"Albums","replacement_id":"#s/#albums","selected":1},
                            {"text":"Singles & EPs","replacement_id":"#s/#singles"}
                        ]},{"action":{"type":"open_section"},"title":"Home","section_id":"must-not-open"}]},
                        {"id":"all","data_type":"music_playlists","playlists_ids":["-42_1"]}
                    ]},"playlists":[release(1,json!({"album":{"type":"album"}})),release(99,json!({}))]
                }),
                "catalog.replaceBlocks" if get("replacement_ids")==Some("#s/#albums") => json!({
                    "replacements":{"replacements":[{"to_blocks":[{"id":"a","data_type":"music_playlists","playlists_ids":["-42_1"]}]}]}
                }),
                "catalog.replaceBlocks" if get("replacement_ids")==Some("#s/#singles") => json!({
                    "replacements":{"new_next_from":"section-next","replacements":[{"to_blocks":[{"id":"singles","data_type":"music_playlists","layout":{"style":"artist_releases"},"playlists_ids":["-42_2","-42_3"],"next_from":"block-next"}]}]},
                    "playlists":[release(2,json!({})),release(3,json!({"meta":{"type":"ep"}}))]
                }),
                "catalog.getBlockItems" => {
                    assert_eq!(get("block_id"),Some("singles"));assert_eq!(get("start_from"),Some("block-next"));
                    json!({"block":{"id":"singles","data_type":"music_playlists","playlists_ids":["-42_4"]},"playlists":[release(4,json!({}))]})
                }
                "catalog.getSection" => {
                    assert_eq!(get("section_id"),Some("s"));assert_eq!(get("start_from"),Some("section-next"));
                    json!({"section":{"id":"s","title":"Releases","blocks":[{"data_type":"music_playlists","playlists_ids":["-42_5"]}]},"playlists":[release(5,json!({}))]})
                }
                _=>panic!("unexpected method: {method}"),
            })
        }).unwrap();
        assert_eq!(calls.len(), 5);
        assert_eq!(
            page.discography
                .iter()
                .map(|release| (release.album.id, release.kind))
                .collect::<Vec<_>>(),
            [
                (1, Kind::Album),
                (2, Kind::Single),
                (3, Kind::Ep),
                (4, Kind::Single),
                (5, Kind::Single)
            ]
        );
    }

    #[test]
    fn uses_the_artist_pages_show_all_reference_without_another_artist_lookup() {
        let mut page = page();
        let initial = json!({"catalog":{"default_section":"overview","sections":[{
            "id":"overview","next_from":"unrelated-overview-page","blocks":[
                {"data_type":"artist"},
                {"data_type":"music_playlists","layout":{"style":"artist_releases"},"playlists_ids":["-42_1"],"meta":{"show_all_info":{"section_id":"full"}}}
            ]
        }]},"playlists":[release(1,json!({"type":"single"}))]});
        let mut calls = 0;
        load(&mut page,&initial,&AtomicBool::new(false),|method,params| {
            calls+=1;
            assert_eq!(method,"catalog.getSection");
            assert!(params.contains(&("section_id","full".into())));
            Ok(json!({"section":{"id":"full","blocks":[{
                "data_type":"music_playlists","playlists_ids":["-42_1"],"meta":{"show_all_info":{"section_id":"full"}}
            }]}}))
        }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(page.discography.len(), 1);
        assert_eq!(page.discography[0].kind, Kind::Single);
    }

    #[test]
    fn overview_ignores_editorial_playlists_and_repeated_cursors_stop() {
        let mut page = page();
        let mut calls = 0;
        let result = load(
            &mut page,
            &json!({}),
            &AtomicBool::new(false),
            |method, _| {
                calls += 1;
                Ok(if method == "catalog.getAudioArtist" {
                    json!({
                        "catalog":{"default_section":"root","sections":[{"id":"root","blocks":[
                            {"data_type":"artist"},{"id":"r","data_type":"music_playlists","layout":{"style":"artist_releases"},"playlists_ids":["-42_1"],"next_from":"repeat"},
                            {"data_type":"music_playlists","title":"Recommendations","playlists_ids":["-42_99"]}
                        ]}]},"playlists":[release(1,json!({"album":{"type":"single"}})),release(99,json!({}))]
                    })
                } else {
                    json!({"block":{"id":"r","data_type":"music_playlists","playlists_ids":["-42_1"],"next_from":"repeat"}})
                })
            },
        );
        assert_eq!(result, Err(WebAudioError::Pagination));
        assert_eq!(calls, 2);
        assert_eq!(page.discography.len(), 1);
        assert_eq!(page.discography[0].kind, Kind::Single);
        assert_eq!(
            load(
                &mut page,
                &json!({}),
                &AtomicBool::new(true),
                |_, _| panic!("canceled")
            ),
            Err(WebAudioError::Canceled)
        );
    }
}
