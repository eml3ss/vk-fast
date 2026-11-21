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
/// Releases asked for per continuation page. VK sends 20 unless asked,
/// and every page is one more paced request; a hundred loads most
/// discographies' blocks in one go.
const PAGE: usize = 100;

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
    /// Groups whose release block in the section being read continues
    /// through its own cursor: their header's Show all would load the same
    /// list again, one more paced request.
    paginated: HashSet<Group>,
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
            if method == "catalog.getBlockItems" {
                params.push(("count", PAGE.to_string()));
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
                && !self
                    .paginated
                    .contains(&inherited_group(block, group_of(button).unwrap_or(context)))
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
            self.paginated = blocks
                .iter()
                .filter(|block| {
                    !unrelated(block)
                        && block.get("data_type").and_then(Value::as_str) == Some("music_playlists")
                        && (dedicated
                            || block.pointer("/layout/style").and_then(Value::as_str)
                                == Some("artist_releases")
                            || group_of(block).is_some())
                        && opaque(block.get("next_from")).is_some()
                        && opaque(block.get("id")).is_some()
                })
                .map(|block| inherited_group(block, category))
                .collect();
            if let Some(id) = opaque(section.get("id")) {
                self.seen
                    .insert(("catalog.getSection", id.into(), String::new(), category));
            }
            for block in blocks {
                self.block(block, category, dedicated, opaque(section.get("id")), page)?;
            }
            self.paginated.clear();
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

#[cfg(test)]
pub fn load(
    page: &mut ArtistPage,
    initial: &Value,
    canceled: &AtomicBool,
    request: impl FnMut(&'static str, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
) -> Result<(), WebAudioError> {
    load_progressive(page, initial, canceled, request, |_| {})
}

/// Like `load`, and hands over the page after each reply that added to
/// it, so the discography fills in while the rest still loads.
pub fn load_progressive(
    page: &mut ArtistPage,
    initial: &Value,
    canceled: &AtomicBool,
    mut request: impl FnMut(&'static str, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
    mut progress: impl FnMut(&ArtistPage),
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
        if !reader.pending.is_empty() {
            progress(page);
        }
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
        let before = page.discography.len();
        reader.payload(&payload, next.group, next.section.as_deref(), page)?;
        count += 1;
        if page.discography.len() > before && !reader.pending.is_empty() {
            progress(page);
        }
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

/// The Show all reference of VK's "Appears on" block on the artist page:
/// its header opens `…/artist/<id>/featuring_releases`. With the section's
/// item count when VK gave it.
fn featuring_header(blocks: &[Value]) -> Option<(&Value, Option<&str>, Option<u64>)> {
    blocks.iter().find_map(|block| {
        let button = [block.get("actions"), block.get("buttons")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_array)
            .flatten()
            .find(|button| {
                let action = button.get("action").unwrap_or(button);
                action
                    .get("url")
                    .and_then(Value::as_str)
                    .and_then(|url| reqwest::Url::parse(url).ok())
                    .is_some_and(|url| url.path().ends_with("/featuring_releases"))
            })?;
        let section = opaque(
            button
                .get("section_id")
                .or_else(|| button.pointer("/action/section_id")),
        );
        Some((
            block,
            section,
            button.get("ref_items_count").and_then(Value::as_u64),
        ))
    })
}

fn featuring_title(text: &str) -> bool {
    let text = text.to_lowercase();
    ["участи", "appears on", "featuring", "featured on"]
        .iter()
        .any(|word| text.contains(word))
}

/// The release IDs of the artist page's "Appears on" block, in VK's order:
/// the releases block under the header that opens `featuring_releases`, or
/// one titled so.
pub fn featuring_ids(blocks: &[Value]) -> Vec<String> {
    featuring_block(blocks)
        .and_then(|block| {
            block
                .get("playlists_ids")
                .or_else(|| block.get("playlist_ids"))
                .and_then(Value::as_array)
        })
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn featuring_block(blocks: &[Value]) -> Option<&Value> {
    let header_title = featuring_header(blocks).and_then(|(header, _, _)| {
        header
            .get("title")
            .or_else(|| header.pointer("/layout/title"))
            .and_then(Value::as_str)
    });
    blocks.iter().find(|block| {
        block.get("data_type").and_then(Value::as_str) == Some("music_playlists")
            && block.pointer("/layout/style").and_then(Value::as_str) != Some("artist_releases")
            && (block.pointer("/layout/style").and_then(Value::as_str)
                == Some("featuring_releases")
                || block
                    .get("title")
                    .and_then(Value::as_str)
                    .is_some_and(|title| Some(title) == header_title || featuring_title(title)))
    })
}

/// Loads the rest of "Appears on" through its Show all section when the
/// artist page held only part of it (VK puts up to ten on the page).
pub fn load_featuring(
    page: &mut ArtistPage,
    initial: &Value,
    canceled: &AtomicBool,
    mut request: impl FnMut(&'static str, Vec<(&'static str, String)>) -> Result<Value, WebAudioError>,
) -> Result<(), WebAudioError> {
    const MAX_FEATURING_REQUESTS: usize = 16;
    let default = initial
        .pointer("/catalog/default_section")
        .and_then(Value::as_str);
    let sections = initial
        .pointer("/catalog/sections")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let Some(blocks) = sections
        .iter()
        .find(|section| section.get("id").and_then(Value::as_str) == default)
        .or_else(|| sections.first())
        .and_then(|section| section.get("blocks"))
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    let (section, count) = featuring_header(blocks)
        .map(|(_, section, count)| (section, count))
        .unwrap_or_default();
    if count.is_some_and(|count| count as usize <= page.featuring.len()) {
        return Ok(());
    }
    // The block on the page continues through its own cursor when it has
    // one: one paced request fewer than opening its Show all section.
    let block = featuring_block(blocks);
    let first = match (
        block.and_then(|block| opaque(block.get("id"))),
        block.and_then(|block| opaque(block.get("next_from"))),
        section,
    ) {
        (Some(id), Some(cursor), _) => (
            "catalog.getBlockItems",
            vec![
                ("block_id", id.to_owned()),
                ("start_from", cursor.to_owned()),
                ("extended", "1".to_owned()),
                ("count", PAGE.to_string()),
            ],
        ),
        (_, _, Some(section)) => (
            "catalog.getSection",
            vec![
                ("section_id", section.to_owned()),
                ("need_blocks", "1".to_owned()),
                ("extended", "1".to_owned()),
            ],
        ),
        _ => return Ok(()),
    };
    let mut pending = VecDeque::from([first]);
    let mut seen = HashSet::new();
    let mut requests = 0;
    while let Some((method, params)) = pending.pop_front() {
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        if requests >= MAX_FEATURING_REQUESTS {
            return Err(WebAudioError::Pagination);
        }
        requests += 1;
        let value = request(method, params)?;
        if canceled.load(Ordering::Acquire) {
            return Err(WebAudioError::Canceled);
        }
        let pool: HashMap<String, Release> = value
            .get("playlists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Release::from_value)
            .map(|release| {
                (
                    format!("{}_{}", release.album.owner_id, release.album.id),
                    release,
                )
            })
            .collect();
        let (blocks, section_value): (Vec<&Value>, Option<&Value>) =
            if let Some(section) = value.get("section") {
                (
                    section
                        .get("blocks")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .collect(),
                    Some(section),
                )
            } else {
                (value.get("block").into_iter().collect(), None)
            };
        for block in blocks {
            if block.get("data_type").and_then(Value::as_str) != Some("music_playlists") {
                continue;
            }
            let batch: Vec<_> = block
                .get("playlists_ids")
                .or_else(|| block.get("playlist_ids"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|id| pool.get(id).cloned())
                .collect();
            artist_releases::merge(&mut page.featuring, batch);
            if page.featuring.len() > MAX_RELEASES {
                return Err(WebAudioError::TooLarge);
            }
            if let (Some(id), Some(cursor)) =
                (opaque(block.get("id")), opaque(block.get("next_from")))
                && seen.insert(("block", id.to_owned(), cursor.to_owned()))
            {
                pending.push_back((
                    "catalog.getBlockItems",
                    vec![
                        ("block_id", id.to_owned()),
                        ("start_from", cursor.to_owned()),
                        ("extended", "1".to_owned()),
                        ("count", PAGE.to_string()),
                    ],
                ));
            }
        }
        if let Some(section) = section_value
            && let (Some(id), Some(cursor)) =
                (opaque(section.get("id")), opaque(section.get("next_from")))
            && seen.insert(("section", id.to_owned(), cursor.to_owned()))
        {
            pending.push_back((
                "catalog.getSection",
                vec![
                    ("section_id", id.to_owned()),
                    ("start_from", cursor.to_owned()),
                    ("need_blocks", "1".to_owned()),
                    ("extended", "1".to_owned()),
                ],
            ));
        }
    }
    Ok(())
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
            featuring: Vec::new(),
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
    #[test]
    fn appears_on_comes_from_its_own_block_and_the_rest_from_its_show_all() {
        let blocks = json!([
            {"data_type":"artist"},
            {"data_type":"music_playlists","layout":{"style":"artist_releases"},"title":"Releases","playlists_ids":["-42_1"]},
            {"data_type":"none","title":"Appears in releases","actions":[{
                "action":{"type":"open_section","url":"https://vk.ru/artist/synthetic-artist/featuring_releases"},
                "section_id":"featuring","ref_items_count":3
            }]},
            {"data_type":"music_playlists","title":"Appears in releases","playlists_ids":["-42_7","-42_8"]},
            {"data_type":"music_playlists","title":"Recommendations","playlists_ids":["-42_99"]}
        ]);
        let blocks = blocks.as_array().unwrap();
        assert_eq!(featuring_ids(blocks), ["-42_7", "-42_8"]);
        // Never part of the artist's own discography.
        let mut page = page();
        let initial =
            json!({"catalog":{"default_section":"s","sections":[{"id":"s","blocks":blocks}]}});
        page.featuring = vec![Release::from_value(&release(7, json!({}))).unwrap()];
        let mut calls = Vec::new();
        load_featuring(&mut page, &initial, &AtomicBool::new(false), |method, params| {
            calls.push(method);
            let get=|key|params.iter().find(|(name,_)|*name==key).map(|(_,value)|value.as_str());
            Ok(match method {
                "catalog.getSection" => {
                    assert_eq!(get("section_id"), Some("featuring"));
                    json!({"section":{"id":"featuring","blocks":[
                        {"id":"b","data_type":"music_playlists","playlists_ids":["-42_7","-42_8"],"next_from":"more"}
                    ]},"playlists":[
                        release(7,json!({})),
                        release(8,json!({"year":2020,"main_artists":[{"id":"o","name":"Other"}],"featured_artists":[{"id":"s","name":"Synthetic artist"}]}))
                    ]})
                }
                "catalog.getBlockItems" => {
                    assert_eq!(get("start_from"), Some("more"));
                    json!({"block":{"id":"b","data_type":"music_playlists","playlists_ids":["-42_9"]},"playlists":[release(9,json!({}))]})
                }
                _ => panic!("unexpected method: {method}"),
            })
        })
        .unwrap();
        assert_eq!(calls, ["catalog.getSection", "catalog.getBlockItems"]);
        assert_eq!(
            page.featuring
                .iter()
                .map(|release| release.album.id)
                .collect::<Vec<_>>(),
            [7, 8, 9]
        );
        assert_eq!(
            page.featuring[1].featuring_caption(crate::i18n::Language::English),
            "2020 · Other feat. Synthetic artist"
        );
        // Everything already on the page: no request.
        page.featuring.truncate(3);
        load_featuring(&mut page, &initial, &AtomicBool::new(false), |_, _| {
            panic!("no request")
        })
        .unwrap();
    }
    #[test]
    fn a_block_that_continues_itself_needs_no_show_all_and_shows_as_it_loads() {
        // The artist page: the Releases header's Show all opens the same
        // list the block under it continues through its cursor.
        let initial = json!({"catalog":{"default_section":"o","sections":[{"id":"o","blocks":[
            {"data_type":"artist"},
            {"data_type":"none","title":"Releases","actions":[{
                "action":{"type":"open_section","url":"https://vk.ru/artist/synthetic-artist/releases"},
                "section_id":"releases"
            }]},
            {"id":"r","data_type":"music_playlists","layout":{"style":"artist_releases"},"title":"Releases","playlists_ids":["-42_1"],"next_from":"more"},
            {"data_type":"none","title":"Appears on","actions":[{
                "action":{"type":"open_section","url":"https://vk.ru/artist/synthetic-artist/featuring_releases"},
                "section_id":"featuring","ref_items_count":3
            }]},
            {"id":"f","data_type":"music_playlists","title":"Appears on","playlists_ids":["-42_7"],"next_from":"guest"}
        ]}]},"playlists":[release(1,json!({"type":"album"})),release(7,json!({}))]});
        let mut page = page();
        let mut calls = Vec::new();
        let mut shown = Vec::new();
        load_progressive(
            &mut page,
            &initial,
            &AtomicBool::new(false),
            |method, params| {
                let get = |key| params.iter().find(|(name, _)| *name == key).map(|(_, value)| value.as_str());
                calls.push(method);
                assert_eq!(method, "catalog.getBlockItems", "no Show all section");
                assert_eq!((get("block_id"), get("count")), (Some("r"), Some("100")));
                Ok(if get("start_from") == Some("more") {
                    json!({"block":{"id":"r","data_type":"music_playlists","playlists_ids":["-42_2"],"next_from":"last"},
                        "playlists":[release(2,json!({"type":"single"}))]})
                } else {
                    json!({"block":{"id":"r","data_type":"music_playlists","playlists_ids":["-42_3"]},
                        "playlists":[release(3,json!({"type":"single"}))]})
                })
            },
            |so_far| shown.push(so_far.discography.len()),
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(page.discography.len(), 3);
        // Shown after the reply that left more to load; the artist page
        // itself and the finished discography the caller shows itself.
        assert_eq!(shown, [2]);
        // "Appears on" goes on from its own block, not its Show all.
        page.featuring = vec![Release::from_value(&release(7, json!({}))).unwrap()];
        load_featuring(&mut page, &initial, &AtomicBool::new(false), |method, params| {
            let get = |key| params.iter().find(|(name, _)| *name == key).map(|(_, value)| value.as_str());
            assert_eq!(method, "catalog.getBlockItems");
            assert_eq!((get("block_id"), get("start_from"), get("count")), (Some("f"), Some("guest"), Some("100")));
            Ok(json!({"block":{"id":"f","data_type":"music_playlists","playlists_ids":["-42_8","-42_9"]},
                "playlists":[release(8,json!({})),release(9,json!({}))]}))
        })
        .unwrap();
        assert_eq!(
            page.featuring
                .iter()
                .map(|release| release.album.id)
                .collect::<Vec<_>>(),
            [7, 8, 9]
        );
    }
}
