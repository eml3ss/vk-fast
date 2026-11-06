//! Following keeps the original and the server-created library identity apart.
use super::*;

type Key = (i64, i64);
fn key(reference: &AlbumRef) -> Key {
    (reference.owner_id, reference.id)
}

#[derive(Clone)]
struct Followed {
    original: AlbumRef,
    saved: AlbumRef,
}

#[derive(Default)]
pub(super) struct FollowState {
    generation: u64,
    revision: u64,
    ready: bool,
    items: HashMap<Key, Followed>,
}

pub struct PlaylistFollowChange {
    pub original: AlbumRef,
    pub saved: Option<AlbumRef>,
    pub removed: Option<Key>,
}

pub(super) fn original_reference(
    value: &Value,
    fallback: &AlbumRef,
) -> Result<Option<AlbumRef>, WebAudioError> {
    let Some(original) = value.get("original").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let owner = original
        .get("owner_id")
        .and_then(number)
        .ok_or(WebAudioError::Format)?;
    let id = original
        .get("playlist_id")
        .or_else(|| original.get("id"))
        .and_then(number)
        .ok_or(WebAudioError::Format)?;
    if owner == 0 || owner.checked_abs().is_none() || id <= 0 {
        return Err(WebAudioError::Format);
    }
    Ok(Some(AlbumRef {
        owner_id: owner,
        id,
        title: fallback.title.clone(),
        cover_url: fallback.cover_url.clone(),
        access_key: original
            .get("access_key")
            .and_then(Value::as_str)
            .filter(|key| key.len() <= 4096 && !key.chars().any(char::is_control))
            .map(str::to_owned),
    }))
}

fn current(
    state: &State,
    owner: i64,
    generation: u64,
    canceled: &AtomicBool,
) -> Result<(), WebAudioError> {
    if canceled.load(Ordering::Acquire) {
        Err(WebAudioError::Canceled)
    } else if state.verified_owner != Some(owner) || state.generation != generation {
        Err(WebAudioError::WrongAccount)
    } else {
        Ok(())
    }
}

impl WebAudioClient {
    pub fn playlist_followed(&self, owner: i64, target: &AlbumRef) -> Option<bool> {
        let state = self.state.lock().ok()?;
        let cache = &state.followed_playlists;
        (state.verified_owner == Some(owner) && cache.generation == state.generation && cache.ready)
            .then(|| cache.items.contains_key(&key(target)))
    }

    pub(super) fn playlist_follow_revision(
        &self,
        owner: i64,
        generation: u64,
        canceled: &AtomicBool,
    ) -> Result<u64, WebAudioError> {
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        current(&state, owner, generation, canceled)?;
        if state.followed_playlists.generation != generation {
            state.followed_playlists = FollowState {
                generation,
                ..Default::default()
            };
        }
        Ok(state.followed_playlists.revision)
    }

    pub(super) fn cache_followed_playlists(
        &self,
        owner: i64,
        generation: u64,
        revision: u64,
        playlists: &[UserPlaylist],
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        let mut items = HashMap::new();
        for playlist in playlists.iter().filter(|playlist| {
            !playlist.can_add_tracks
                && (playlist.original.is_some() || playlist.album.owner_id != owner)
        }) {
            let followed = Followed {
                original: playlist
                    .original
                    .as_ref()
                    .unwrap_or(&playlist.album)
                    .clone(),
                saved: playlist.album.clone(),
            };
            items.insert(key(&followed.original), followed.clone());
            items.insert(key(&followed.saved), followed);
        }
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        current(&state, owner, generation, canceled)?;
        let cache = &mut state.followed_playlists;
        // A fetch started before a mutation must not undo its confirmed state.
        if cache.generation == generation && cache.revision == revision {
            cache.items = items;
            cache.ready = true;
        }
        Ok(())
    }

    pub fn set_playlist_follow(
        &self,
        owner: i64,
        target: &AlbumRef,
        follow: bool,
        canceled: &AtomicBool,
    ) -> Result<PlaylistFollowChange, WebAudioError> {
        self.set_playlist_follow_request(
            owner,
            target,
            follow,
            canceled,
            |generation, method, params| {
                self.detail_request(owner, generation, method, params, canceled)
            },
        )
    }

    fn set_playlist_follow_request(
        &self,
        owner: i64,
        target: &AlbumRef,
        follow: bool,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<PlaylistFollowChange, WebAudioError> {
        let (generation, details) =
            self.playlist_manage_metadata(owner, target, canceled, &mut request)?;
        if details.can_edit {
            return Err(WebAudioError::Rejected);
        }
        let record = {
            let state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            current(&state, owner, generation, canceled)?;
            let cache = &state.followed_playlists;
            if !cache.ready || cache.generation != generation {
                return Err(WebAudioError::Unverified);
            }
            cache.items.get(&key(target)).cloned()
        };
        if follow == record.is_some() {
            return Ok(PlaylistFollowChange {
                original: record
                    .as_ref()
                    .map_or_else(|| target.clone(), |record| record.original.clone()),
                saved: record.map(|record| record.saved),
                removed: None,
            });
        }
        let mut original = record.as_ref().map_or_else(
            || details.playlist.clone(),
            |record| record.original.clone(),
        );
        if original.access_key.is_none() && key(&original) == key(target) {
            original.access_key = target.access_key.clone();
        }
        let (saved, removed) = if follow {
            // An own non-editable identity is not proof of an original playlist.
            if original.owner_id == owner {
                return Err(WebAudioError::Rejected);
            }
            let mut params = vec![
                ("owner_id", original.owner_id.to_string()),
                ("playlist_id", original.id.to_string()),
            ];
            if let Some(access_key) = &original.access_key {
                params.push(("access_key", access_key.clone()));
            }
            let response = request(generation, "audio.followPlaylist", params)?;
            let mut saved = original.clone();
            saved.id = response
                .get("playlist_id")
                .or_else(|| response.get("id"))
                .and_then(number)
                .ok_or(WebAudioError::Format)?;
            saved.owner_id = response
                .get("owner_id")
                .and_then(number)
                .ok_or(WebAudioError::Format)?;
            if saved.id <= 0 || (saved.owner_id != owner && key(&saved) != key(&original)) {
                return Err(WebAudioError::Format);
            }
            saved.access_key = response
                .get("access_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    (key(&saved) == key(&original))
                        .then(|| original.access_key.clone())
                        .flatten()
                });
            (Some(saved), None)
        } else {
            let saved = &record.as_ref().ok_or(WebAudioError::Unverified)?.saved;
            // Delete only the identity returned in this account's saved list,
            // never substitute the creator's original for a followed copy.
            let response = request(
                generation,
                "audio.deletePlaylist",
                vec![
                    ("owner_id", saved.owner_id.to_string()),
                    ("playlist_id", saved.id.to_string()),
                ],
            )?;
            playlist_manage::mutation_succeeded(&response)?;
            (None, Some(key(saved)))
        };
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        current(&state, owner, generation, canceled)?;
        let cache = &mut state.followed_playlists;
        if cache.generation != generation || !cache.ready {
            return Err(WebAudioError::Unverified);
        }
        cache.revision = cache.revision.wrapping_add(1);
        cache
            .items
            .retain(|_, record| key(&record.original) != key(&original));
        if let Some(saved) = &saved {
            let record = Followed {
                original: original.clone(),
                saved: saved.clone(),
            };
            cache.items.insert(key(&original), record.clone());
            cache.items.insert(key(saved), record);
        }
        Ok(PlaylistFollowChange {
            original,
            saved,
            removed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-follow-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        seed(&client, &json!({"count":0,"items":[]}));
        client
    }
    fn seed(client: &WebAudioClient, response: &Value) {
        let canceled = AtomicBool::new(false);
        let generation = client.home_owner(42, &canceled).unwrap();
        let revision = client
            .playlist_follow_revision(42, generation, &canceled)
            .unwrap();
        let page = parse_user_playlists(response).unwrap();
        client
            .cache_followed_playlists(42, generation, revision, &page.items, &canceled)
            .unwrap();
    }
    fn reference() -> AlbumRef {
        AlbumRef {
            owner_id: -99,
            id: 10,
            title: "Synthetic playlist".into(),
            access_key: Some("synthetic-key".into()),
            cover_url: None,
        }
    }
    fn metadata(target: &AlbumRef, edit: bool) -> Value {
        json!({"id":target.id,"owner_id":target.owner_id,"title":target.title,"permissions":{"edit":edit},"is_following":false})
    }

    #[test]
    fn follow_and_unfollow_use_the_server_saved_identity_and_resist_a_stale_library_fetch() {
        let client = client();
        let original = reference();
        let canceled = AtomicBool::new(false);
        let generation = client.home_owner(42, &canceled).unwrap();
        let old_revision = client
            .playlist_follow_revision(42, generation, &canceled)
            .unwrap();
        assert_eq!(client.playlist_followed(42, &original), Some(false));
        let mut calls = Vec::new();
        let change = client
            .set_playlist_follow_request(42, &original, true, &canceled, |_, method, params| {
                calls.push((method, params.clone()));
                Ok(if method == "audio.getPlaylistById" {
                    metadata(&original, false)
                } else {
                    json!({"owner_id":42,"playlist_id":77})
                })
            })
            .unwrap();
        let saved = change.saved.unwrap();
        assert_eq!(key(&saved), (42, 77));
        assert_eq!(
            calls[1],
            (
                "audio.followPlaylist",
                vec![
                    ("owner_id", "-99".into()),
                    ("playlist_id", "10".into()),
                    ("access_key", "synthetic-key".into())
                ]
            )
        );
        assert_eq!(client.playlist_followed(42, &original), Some(true));
        assert_eq!(client.playlist_followed(42, &saved), Some(true));
        client
            .cache_followed_playlists(42, generation, old_revision, &[], &canceled)
            .unwrap();
        assert_eq!(client.playlist_followed(42, &original), Some(true));
        let mut deleted = None;
        let change = client
            .set_playlist_follow_request(42, &original, false, &canceled, |_, method, params| {
                if method == "audio.getPlaylistById" {
                    Ok(metadata(&original, false))
                } else {
                    deleted = Some((method, params));
                    Ok(json!(1))
                }
            })
            .unwrap();
        assert_eq!(
            deleted.unwrap(),
            (
                "audio.deletePlaylist",
                vec![("owner_id", "42".into()), ("playlist_id", "77".into())]
            )
        );
        assert_eq!(change.removed, Some((42, 77)));
        assert_eq!(change.original, original);
        assert_eq!(client.playlist_followed(42, &original), Some(false));
    }

    #[test]
    fn library_original_mapping_survives_refresh_and_failed_removal_preserves_the_heart() {
        let client = client();
        let original = reference();
        seed(
            &client,
            &json!({"count":1,"items":[{
                "id":77,"owner_id":42,"title":"Copy","is_following":true,"permissions":{"edit":false},
                "original":{"playlist_id":10,"owner_id":-99,"access_key":"synthetic-key"}
            }]}),
        );
        let saved = AlbumRef {
            owner_id: 42,
            id: 77,
            ..original.clone()
        };
        assert_eq!(client.playlist_followed(42, &original), Some(true));
        let result = client.set_playlist_follow_request(
            42,
            &saved,
            false,
            &AtomicBool::new(false),
            |_, method, _| {
                if method == "audio.getPlaylistById" {
                    Ok(metadata(&saved, false))
                } else {
                    Err(WebAudioError::Network)
                }
            },
        );
        assert!(matches!(result, Err(WebAudioError::Network)));
        assert_eq!(client.playlist_followed(42, &original), Some(true));
        assert_eq!(client.playlist_followed(43, &original), None);
        client.accept_profile(43).unwrap();
        assert_eq!(client.playlist_followed(43, &original), None);
    }

    #[test]
    fn editable_playlists_cancellation_and_changed_accounts_never_issue_or_commit_a_mutation() {
        let client = client();
        let original = reference();
        let canceled = AtomicBool::new(false);
        for edit in [true, false] {
            let mut calls = 0;
            let result =
                client.set_playlist_follow_request(42, &original, true, &canceled, |_, _, _| {
                    calls += 1;
                    if !edit {
                        canceled.store(true, Ordering::Release);
                    }
                    Ok(metadata(&original, edit))
                });
            assert!(matches!(
                result,
                Err(WebAudioError::Rejected | WebAudioError::Canceled)
            ));
            assert_eq!(calls, 1);
        }
        canceled.store(false, Ordering::Release);
        let result =
            client.set_playlist_follow_request(42, &original, true, &canceled, |_, method, _| {
                if method == "audio.getPlaylistById" {
                    Ok(metadata(&original, false))
                } else {
                    client.accept_profile(43).unwrap();
                    Ok(json!({"owner_id":42,"playlist_id":77}))
                }
            });
        assert!(matches!(result, Err(WebAudioError::WrongAccount)));
        assert_eq!(client.playlist_followed(43, &original), None);
    }

    #[test]
    fn an_own_readonly_identity_without_a_followed_source_is_never_deleted() {
        let client = client();
        seed(
            &client,
            &json!({"count":1,"items":[{"id":10,"owner_id":42,"title":"Own","permissions":{"edit":false}}]}),
        );
        let target = AlbumRef {
            owner_id: 42,
            ..reference()
        };
        assert_eq!(client.playlist_followed(42, &target), Some(false));
        let mut calls = Vec::new();
        let change = client
            .set_playlist_follow_request(
                42,
                &target,
                false,
                &AtomicBool::new(false),
                |_, method, _| {
                    calls.push(method);
                    Ok(metadata(&target, false))
                },
            )
            .unwrap();
        assert_eq!(calls, ["audio.getPlaylistById"]);
        assert!(change.removed.is_none());
    }

    #[test]
    fn official_albums_use_following_even_when_vk_reports_an_edit_permission() {
        let client = client();
        let original = reference();
        let change = client
            .set_playlist_follow_request(
                42,
                &original,
                true,
                &AtomicBool::new(false),
                |_, method, _| {
                    if method == "audio.getPlaylistById" {
                        let mut value = metadata(&original, true);
                        value["type"] = json!(1);
                        Ok(value)
                    } else {
                        assert_eq!(method, "audio.followPlaylist");
                        Ok(json!({"owner_id":42,"playlist_id":77}))
                    }
                },
            )
            .unwrap();
        assert_eq!(key(&change.saved.unwrap()), (42, 77));
        assert_eq!(client.playlist_followed(42, &original), Some(true));
    }
}
