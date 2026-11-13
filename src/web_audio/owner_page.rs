//! A publisher's visible playlists, independent of the authenticated viewer's library.
use super::*;

const MAX_PLAYLISTS: usize = 10_000;

/// Why VK shows none of a page's music to this viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerClosed {
    /// The page was deleted or blocked.
    Gone,
    /// The user's profile is private.
    Profile,
    /// The user hid their music in privacy settings.
    Music,
    /// A closed or private community the viewer has not joined.
    Community,
}

#[derive(Clone)]
pub struct OwnerPage {
    pub creator: PlaylistCreator,
    pub image_url: Option<String>,
    pub description: Option<String>,
    /// Set when the profile itself says its music is out of reach; then no
    /// music is asked for.
    pub closed: Option<OwnerClosed>,
    pub playlists: Vec<UserPlaylist>,
    pub total: Option<usize>,
    /// Zero is a complete header plus the first batch; later batches append.
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub playlists_error: Option<WebAudioError>,
    pub retry_offset: Option<usize>,
    /// The owner's first tracks, read with the first playlist batch.
    pub tracks: Vec<Track>,
    /// How many tracks the owner has in all, as VK counts them.
    pub track_total: Option<usize>,
    pub tracks_error: Option<WebAudioError>,
}

impl OwnerPage {
    /// Accept only the expected continuation; a stale page cannot replace a profile.
    pub fn append(&mut self, batch: Self) -> bool {
        if batch.creator.owner_id != self.creator.owner_id
            || batch.offset == 0
            || self.next_offset != Some(batch.offset)
        {
            return false;
        }
        if batch.playlists_error.is_some() && batch.playlists.is_empty() {
            self.playlists_error = batch.playlists_error;
            self.retry_offset = batch.retry_offset;
            return true;
        }
        let mut known: HashSet<_> = self
            .playlists
            .iter()
            .map(|playlist| (playlist.album.owner_id, playlist.album.id))
            .collect();
        let before = self.playlists.len();
        self.playlists.extend(
            batch
                .playlists
                .into_iter()
                .filter(|playlist| known.insert((playlist.album.owner_id, playlist.album.id))),
        );
        if self.playlists.len() == before && batch.next_offset.is_some() {
            self.playlists_error = Some(WebAudioError::Pagination);
            self.retry_offset = Some(batch.offset);
            return true;
        }
        self.total = batch.total.or(self.total);
        self.next_offset = batch.next_offset;
        self.playlists_error = batch.playlists_error;
        self.retry_offset = batch.retry_offset;
        true
    }
}

impl WebAudioClient {
    pub fn owner_page(
        &self,
        viewer: i64,
        creator: &PlaylistCreator,
        offset: usize,
        canceled: &AtomicBool,
    ) -> Result<OwnerPage, WebAudioError> {
        self.owner_page_request(
            viewer,
            creator,
            offset,
            canceled,
            |generation, method, params| {
                self.detail_request(viewer, generation, method, params, canceled)
            },
        )
    }

    fn owner_page_request(
        &self,
        viewer: i64,
        creator: &PlaylistCreator,
        offset: usize,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<OwnerPage, WebAudioError> {
        let id = creator.owner_id;
        if id == 0
            || id.checked_abs().is_none()
            || offset >= MAX_PLAYLISTS
            || !offset.is_multiple_of(100)
        {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(viewer, canceled)?;
        let mut page = OwnerPage {
            creator: creator.clone(),
            image_url: None,
            description: None,
            closed: None,
            playlists: Vec::new(),
            total: None,
            offset,
            next_offset: None,
            playlists_error: None,
            retry_offset: None,
            tracks: Vec::new(),
            track_total: None,
            tracks_error: None,
        };
        if offset == 0 {
            let (method, params) = if id > 0 {
                (
                    "users.get",
                    vec![
                        ("user_ids", id.to_string()),
                        (
                            "fields",
                            "photo_200,photo_100,photo_50,status,can_see_audio".into(),
                        ),
                    ],
                )
            } else {
                (
                    "groups.getById",
                    vec![
                        ("group_ids", id.unsigned_abs().to_string()),
                        ("fields", "description,status".into()),
                    ],
                )
            };
            let value = request(generation, method, params)?;
            self.commit_home_streams(viewer, generation, HashMap::new(), canceled)?;
            let record = owner_record(&value, id).ok_or(WebAudioError::Format)?;
            page.creator = playlist_creator_from_lookup(&value, id).ok_or(WebAudioError::Format)?;
            page.image_url = profile_avatar_from_object(record);
            page.description =
                playlist_description(record.get(if id > 0 { "status" } else { "description" }));
            page.closed = owner_closed(record, id);
            if page.closed.is_some() {
                return Ok(page);
            }
        }
        let result = request(
            generation,
            "audio.getPlaylists",
            vec![
                ("owner_id", id.to_string()),
                ("count", "100".into()),
                ("offset", offset.to_string()),
            ],
        );
        self.commit_home_streams(viewer, generation, HashMap::new(), canceled)?;
        match result.and_then(|value| parse_user_playlists(&value)) {
            Ok(batch) => {
                page.next_offset = batch.next_offset(offset);
                page.total = batch.total;
                page.playlists = batch.items;
                if page.next_offset.is_some_and(|next| next >= MAX_PLAYLISTS) {
                    page.next_offset = None;
                    page.playlists_error = Some(WebAudioError::TooLarge);
                }
            }
            Err(
                error @ (WebAudioError::WrongAccount
                | WebAudioError::Canceled
                | WebAudioError::Unverified
                | WebAudioError::NoSession
                | WebAudioError::Expired),
            ) => return Err(error),
            Err(error) => {
                page.playlists_error = Some(error);
                page.retry_offset = Some(offset);
            }
        }
        if offset == 0 {
            // The community's or user's own audio list, best effort: a closed
            // list leaves the playlists usable.
            let result = request(
                generation,
                "audio.get",
                vec![
                    ("owner_id", id.to_string()),
                    ("count", "100".into()),
                    ("offset", "0".into()),
                ],
            );
            self.commit_home_streams(viewer, generation, HashMap::new(), canceled)?;
            match result.and_then(|value| {
                let rows = value
                    .get("items")
                    .and_then(Value::as_array)
                    .ok_or(WebAudioError::Format)?;
                let total = value
                    .get("count")
                    .and_then(Value::as_u64)
                    .and_then(|count| usize::try_from(count).ok());
                Ok((home_tracks(rows)?.0, total))
            }) {
                Ok((mut tracks, total)) => {
                    for track in &mut tracks {
                        track.url = None;
                    }
                    // Never fewer than the tracks already read.
                    page.track_total = total.map(|total| total.max(tracks.len()));
                    page.tracks = tracks;
                }
                Err(
                    error @ (WebAudioError::WrongAccount
                    | WebAudioError::Canceled
                    | WebAudioError::Unverified
                    | WebAudioError::NoSession
                    | WebAudioError::Expired),
                ) => return Err(error),
                Err(error) => page.tracks_error = Some(error),
            }
        }
        Ok(page)
    }
}

/// What the profile itself says about reaching its music. Only explicit
/// marks count: a missing field leaves the music to be asked for.
fn owner_closed(record: &Value, owner: i64) -> Option<OwnerClosed> {
    let flag = |key: &str| {
        record.get(key).and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_i64().map(|flag| flag != 0))
        })
    };
    if record
        .get("deactivated")
        .and_then(Value::as_str)
        .is_some_and(|state| !state.is_empty())
    {
        return Some(OwnerClosed::Gone);
    }
    if owner > 0 {
        if flag("is_closed") == Some(true) && flag("can_access_closed") == Some(false) {
            return Some(OwnerClosed::Profile);
        }
        if flag("can_see_audio") == Some(false) {
            return Some(OwnerClosed::Music);
        }
    } else if flag("is_closed") == Some(true) && flag("is_member") == Some(false) {
        return Some(OwnerClosed::Community);
    }
    None
}

fn owner_record(value: &Value, owner: i64) -> Option<&Value> {
    let keys: &[&str] = if owner > 0 {
        &["users", "profiles"]
    } else {
        &["groups"]
    };
    let expected = owner.checked_abs()?;
    value
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            keys.iter()
                .filter_map(|key| value.get(key)?.as_array())
                .flatten(),
        )
        .find(|row| row.get("id").and_then(number) == Some(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-owner-page".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        client
    }

    fn creator(id: i64) -> PlaylistCreator {
        PlaylistCreator {
            owner_id: id,
            name: "Publisher".into(),
        }
    }

    fn playlist(owner: i64, id: i64) -> Value {
        json!({"owner_id":owner,"id":id,"title":"Public playlist","count":4})
    }

    #[test]
    fn users_and_groups_are_loaded_by_exact_publisher_without_switching_the_viewer() {
        for owner in [71, -71] {
            let client = client();
            let mut calls = Vec::new();
            let page = client.owner_page_request(42, &creator(owner), 0, &AtomicBool::new(false), |_, method, params| {
                calls.push(method);
                if method == "audio.getPlaylists" {
                    assert!(params.contains(&("owner_id", owner.to_string())));
                    return Ok(json!({"count":201,"items":[playlist(owner,8),playlist(-99,9)]}));
                }
                if method == "audio.get" {
                    // The owner's own tracks, read with the first page only.
                    assert!(params.contains(&("owner_id", owner.to_string())));
                    return Ok(json!({"count":1,"items":[{"id":5,"owner_id":owner,"title":"Song","artist":"Band","duration":120,"url":"https://cs.example.test/private.mp3","date":1_660_000_000}]}));
                }
                let record = json!({"id":71,"name":"Group &amp; name","first_name":"Test","last_name":"Person", "photo_200":"https://images.example.test/avatar.jpg", "description":"<b>Music</b>","status":"Hello"});
                if owner > 0 {
                    assert_eq!(method,"users.get");
                    assert!(params.contains(&("user_ids", "71".into())));
                    Ok(json!([{"id":99,"first_name":"Wrong"},record]))
                } else {
                    assert_eq!(method,"groups.getById");
                    assert!(params.contains(&("group_ids", "71".into())));
                    Ok(json!({"groups":[{"id":99,"name":"Wrong"},record]}))
                }
            }).unwrap();
            assert_eq!(page.creator.owner_id, owner);
            assert_eq!(
                page.creator.name,
                if owner > 0 {
                    "Test Person"
                } else {
                    "Group & name"
                }
            );
            assert!(page.image_url.is_some());
            assert_eq!(page.next_offset, Some(100));
            assert_eq!(page.playlists[1].album.owner_id, -99);
            assert_eq!(client.state.lock().unwrap().verified_owner, Some(42));
            assert_eq!(calls.len(), 3);
            assert_eq!(page.tracks.len(), 1);
            assert_eq!(page.track_total, Some(1));
            assert!(page.tracks[0].url.is_none(), "stream links are not kept");
            assert_eq!(page.tracks[0].added, Some(1_660_000_000));

            let next = client
                .owner_page_request(
                    42,
                    &page.creator,
                    100,
                    &AtomicBool::new(false),
                    |_, method, params| {
                        assert_eq!(method, "audio.getPlaylists");
                        assert!(params.contains(&("offset", "100".into())));
                        Ok(json!({"count":201,"items":[playlist(owner,8),playlist(owner,10)]}))
                    },
                )
                .unwrap();
            let mut cached = page;
            assert!(cached.append(next));
            assert_eq!(cached.playlists.len(), 3);
            assert_eq!(cached.next_offset, Some(200));
            assert!(cached.image_url.is_some());
        }
    }

    #[test]
    fn private_missing_canceled_and_stale_profiles_do_not_expose_another_account() {
        let client = client();
        let canceled = AtomicBool::new(false);
        for owner in [0, i64::MIN] {
            assert!(
                client
                    .owner_page_request(42, &creator(owner), 0, &canceled, |_, _, _| panic!(
                        "invalid owner"
                    ))
                    .is_err()
            );
        }
        assert!(matches!(
            client.owner_page_request(99, &creator(71), 0, &canceled, |_, _, _| panic!(
                "wrong viewer"
            )),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.owner_page_request(
                42,
                &creator(71),
                0,
                &AtomicBool::new(true),
                |_, _, _| panic!("canceled")
            ),
            Err(WebAudioError::Canceled)
        ));
        assert!(matches!(
            client.owner_page_request(42, &creator(71), 0, &canceled, |_, _, _| Ok(
                json!([{"id":42,"first_name":"Viewer"}])
            )),
            Err(WebAudioError::Format)
        ));
        let private = client.owner_page_request(42, &creator(71), 0, &canceled, |_,method,_| {
            assert_eq!(method, "users.get");
            Ok(json!([{"id":71,"first_name":"Private","is_closed":true,"can_access_closed":false}]))
        }).unwrap();
        assert_eq!(private.closed, Some(OwnerClosed::Profile));
        assert!(private.playlists.is_empty());
        for (owner, record, closed) in [
            (
                71,
                json!({"id":71,"first_name":"Hidden","can_see_audio":0}),
                Some(OwnerClosed::Music),
            ),
            (
                71,
                json!({"id":71,"first_name":"Open","can_see_audio":1}),
                None,
            ),
            (
                71,
                json!({"id":71,"first_name":"Gone","deactivated":"deleted"}),
                Some(OwnerClosed::Gone),
            ),
            (
                -71,
                json!({"id":71,"name":"Club","is_closed":1,"is_member":0}),
                Some(OwnerClosed::Community),
            ),
            (
                -71,
                json!({"id":71,"name":"Member","is_closed":1,"is_member":1}),
                None,
            ),
            (-71, json!({"id":71,"name":"Public","is_closed":0}), None),
        ] {
            let page = client
                .owner_page_request(42, &creator(owner), 0, &canceled, |_, method, params| {
                    if closed.is_some() {
                        assert!(
                            matches!(method, "users.get" | "groups.getById"),
                            "no music asked for"
                        );
                    }
                    if method == "users.get" {
                        assert!(params.iter().any(
                            |(key, value)| *key == "fields" && value.contains("can_see_audio")
                        ));
                    }
                    Ok(match method {
                        "users.get" => json!([record.clone()]),
                        "groups.getById" => json!({"groups":[record.clone()]}),
                        _ => json!({"count":0,"items":[]}),
                    })
                })
                .unwrap();
            assert_eq!(page.closed, closed, "{record}");
        }
        assert!(matches!(
            client.owner_page_request(42, &creator(71), 0, &canceled, |_, _, _| {
                canceled.store(true, Ordering::Release);
                Ok(json!([{"id":71,"first_name":"Canceled"}]))
            }),
            Err(WebAudioError::Canceled)
        ));
    }
}
