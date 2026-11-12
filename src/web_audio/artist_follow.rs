//! Account-bound artist subscriptions; catalog recommendations are not follows.
use super::*;

#[derive(Clone)]
pub struct FollowedArtist {
    pub artist: ArtistRef,
    pub image_url: Option<String>,
}

#[derive(Default)]
pub(super) struct FollowState {
    generation: u64,
    revision: u64,
    items: Option<HashMap<String, FollowedArtist>>,
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

fn parse(value: &Value) -> Result<(Vec<FollowedArtist>, usize, Option<usize>), WebAudioError> {
    let rows = value
        .get("items")
        .or_else(|| value.get("artists"))
        .unwrap_or(value)
        .as_array()
        .ok_or(WebAudioError::Format)?;
    if rows.len() > 100 {
        return Err(WebAudioError::TooLarge);
    }
    let total = value
        .get("count")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .and_then(|count| usize::try_from(count).ok())
                .ok_or(WebAudioError::Format)
        })
        .transpose()?;
    let mut artists = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let artist = artists_from_object(&serde_json::json!({"artists":[row]}))
            .pop()
            .filter(|artist| valid_artist_id(&artist.id))
            .ok_or(WebAudioError::Format)?;
        if seen.insert(artist.id.clone()) {
            artists.push(FollowedArtist {
                artist,
                image_url: artist_image(row).or_else(|| cover_from_object(row)),
            });
        }
    }
    Ok((artists, rows.len(), total))
}

impl WebAudioClient {
    pub fn followed_artists(
        &self,
        owner: i64,
        canceled: &AtomicBool,
    ) -> Result<Vec<FollowedArtist>, WebAudioError> {
        self.followed_artists_request(owner, canceled, |generation, method, params| {
            self.detail_request(owner, generation, method, params, canceled)
        })
    }

    fn followed_artists_request(
        &self,
        owner: i64,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<Vec<FollowedArtist>, WebAudioError> {
        let generation = self.home_owner(owner, canceled)?;
        let revision = {
            let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
            current(&state, owner, generation, canceled)?;
            if state.followed_artists.generation != generation {
                state.followed_artists = FollowState {
                    generation,
                    ..Default::default()
                };
            }
            state.followed_artists.revision
        };
        let mut all = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0usize;
        loop {
            if offset >= 10_000 {
                return Err(WebAudioError::TooLarge);
            }
            let value = request(
                generation,
                "audio.getFollowedArtists",
                vec![
                    ("owner_id", owner.to_string()),
                    ("count", "100".into()),
                    ("offset", offset.to_string()),
                ],
            )?;
            self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
            let (items, received, total) = parse(&value)?;
            let before = all.len();
            all.extend(
                items
                    .into_iter()
                    .filter(|item| seen.insert(item.artist.id.clone())),
            );
            offset += received;
            let done = total.map_or(received < 100, |total| offset >= total);
            if done {
                break;
            }
            if received == 0 || before == all.len() {
                return Err(WebAudioError::Pagination);
            }
        }
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        current(&state, owner, generation, canceled)?;
        let cache = &mut state.followed_artists;
        if cache.generation != generation || cache.revision != revision {
            return Err(WebAudioError::Canceled);
        }
        cache.items = Some(
            all.iter()
                .cloned()
                .map(|item| (item.artist.id.clone(), item))
                .collect(),
        );
        Ok(all)
    }

    pub fn artist_followed(&self, owner: i64, artist: &ArtistRef) -> Option<bool> {
        let state = self.state.lock().ok()?;
        let cache = &state.followed_artists;
        if state.verified_owner != Some(owner) || cache.generation != state.generation {
            return None;
        }
        cache
            .items
            .as_ref()
            .map(|items| items.contains_key(&artist.id))
    }

    pub fn set_artist_follow(
        &self,
        owner: i64,
        artist: &ArtistRef,
        follow: bool,
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        self.set_artist_follow_request(
            owner,
            artist,
            follow,
            canceled,
            |generation, method, params| {
                self.detail_request(owner, generation, method, params, canceled)
            },
        )
    }

    fn set_artist_follow_request(
        &self,
        owner: i64,
        artist: &ArtistRef,
        follow: bool,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<(), WebAudioError> {
        if !valid_artist_id(&artist.id) {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(owner, canceled)?;
        if self
            .artist_followed(owner, artist)
            .ok_or(WebAudioError::Unverified)?
            == follow
        {
            return Ok(());
        }
        let result = request(
            generation,
            if follow {
                "audio.followArtist"
            } else {
                "audio.unfollowArtist"
            },
            vec![("artist_id", artist.id.clone()), ("ref", "artist".into())],
        )?;
        playlist_manage::mutation_succeeded(&result)?;
        let mut state = self.state.lock().map_err(|_| WebAudioError::Unverified)?;
        current(&state, owner, generation, canceled)?;
        let cache = &mut state.followed_artists;
        if cache.generation != generation {
            return Err(WebAudioError::WrongAccount);
        }
        let items = cache.items.as_mut().ok_or(WebAudioError::Unverified)?;
        if follow {
            items.insert(
                artist.id.clone(),
                FollowedArtist {
                    artist: artist.clone(),
                    image_url: None,
                },
            );
        } else {
            items.remove(&artist.id);
        }
        cache.revision = cache.revision.wrapping_add(1);
        Ok(())
    }
}

/// One page of VK's "Кого вы слушаете?" artists for tuning recommendations.
pub struct ArtistPicks {
    pub artists: Vec<FollowedArtist>,
    /// VK's opaque cursor for the next page; none after the last one.
    pub next_from: Option<String>,
}

fn next_cursor(value: &Value) -> Result<Option<String>, WebAudioError> {
    match value.get("next_from") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(cursor)) if cursor.is_empty() => Ok(None),
        Some(Value::String(cursor))
            if cursor.len() <= 512 && !cursor.chars().any(char::is_control) =>
        {
            Ok(Some(cursor.clone()))
        }
        Some(_) => Err(WebAudioError::Format),
    }
}

impl WebAudioClient {
    /// The artists VK's recommendation settings offer to pick from, a page
    /// at a time (`audio.getOnboardingArtists`, as on vk.ru/music/profile).
    pub fn artist_picks(
        &self,
        owner: i64,
        start_from: Option<&str>,
        canceled: &AtomicBool,
    ) -> Result<ArtistPicks, WebAudioError> {
        self.artist_picks_request(owner, start_from, canceled, |generation, method, params| {
            self.detail_request(owner, generation, method, params, canceled)
        })
    }

    fn artist_picks_request(
        &self,
        owner: i64,
        start_from: Option<&str>,
        canceled: &AtomicBool,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<ArtistPicks, WebAudioError> {
        let generation = self.home_owner(owner, canceled)?;
        let params = start_from
            .map(|cursor| vec![("start_from", cursor.to_owned())])
            .unwrap_or_default();
        let value = request(generation, "audio.getOnboardingArtists", params)?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        let (artists, _, _) = parse(&value)?;
        Ok(ArtistPicks {
            artists,
            next_from: next_cursor(&value)?,
        })
    }

    /// Artists matching `query` to pick for recommendations.
    pub fn search_artist_picks(
        &self,
        owner: i64,
        query: &str,
        canceled: &AtomicBool,
    ) -> Result<Vec<FollowedArtist>, WebAudioError> {
        self.search_artist_picks_request(owner, query, canceled, |generation, method, params| {
            self.detail_request(owner, generation, method, params, canceled)
        })
    }

    fn search_artist_picks_request(
        &self,
        owner: i64,
        query: &str,
        canceled: &AtomicBool,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<Vec<FollowedArtist>, WebAudioError> {
        let query = search_query(query)?;
        let generation = self.home_owner(owner, canceled)?;
        let value = request(
            generation,
            "audio.searchArtists",
            vec![("q", query.to_owned()), ("count", "50".into())],
        )?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        Ok(parse(&value)?.0)
    }

    /// Follow `follow` and unfollow `unfollow`, one artist at a time, in
    /// order; stops at the first failure (earlier changes stay applied).
    pub fn update_followed_artists(
        &self,
        owner: i64,
        follow: &[ArtistRef],
        unfollow: &[ArtistRef],
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        for (artists, state) in [(follow, true), (unfollow, false)] {
            for artist in artists {
                self.set_artist_follow(owner, artist, state, canceled)?;
            }
        }
        Ok(())
    }
}

impl WebAudioClient {
    /// VK's "Не нравится" for one track, the signal its recommendations and
    /// mixes learn from. VK's web player sends `audio_ids=<owner>_<id>`.
    pub fn set_dislike(
        &self,
        owner: i64,
        track: (i64, i64),
        dislike: bool,
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        self.set_dislike_request(
            owner,
            track,
            dislike,
            canceled,
            |generation, method, params| {
                self.detail_request(owner, generation, method, params, canceled)
            },
        )
    }

    fn set_dislike_request(
        &self,
        owner: i64,
        track: (i64, i64),
        dislike: bool,
        canceled: &AtomicBool,
        request: impl FnOnce(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<(), WebAudioError> {
        if track.0 == 0 || track.1 <= 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(owner, canceled)?;
        let result = request(
            generation,
            if dislike {
                "audio.addDislike"
            } else {
                "audio.removeDislike"
            },
            vec![("audio_ids", format!("{}_{}", track.0, track.1))],
        )?;
        playlist_manage::mutation_succeeded(&result)
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
            value: "synthetic-artists".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        client.accept_profile(42).unwrap();
        client
            .followed_artists_request(42, &AtomicBool::new(false), |_, _, _| {
                Ok(json!({"count":0,"items":[]}))
            })
            .unwrap();
        client
    }
    fn artist() -> ArtistRef {
        ArtistRef {
            id: "synthetic-artist".into(),
            name: "Artist".into(),
        }
    }

    #[test]
    fn fetches_every_subscription_in_hundreds_without_treating_a_partial_batch_as_empty() {
        let client = client();
        let canceled = AtomicBool::new(false);
        let mut offsets = Vec::new();
        let items = client.followed_artists_request(42, &canceled, |_, method, params| {
            assert_eq!(method, "audio.getFollowedArtists");
            assert!(params.contains(&("owner_id", "42".into())));
            assert!(params.contains(&("count", "100".into())));
            let offset: usize = params.iter().find(|(key, _)| *key == "offset").unwrap().1.parse().unwrap();
            offsets.push(offset);
            Ok(json!({"count":101,"items":(offset..(offset+100).min(101)).map(|id| json!({"id":format!("artist-{id}"),"name":format!("Artist {id}")})).collect::<Vec<_>>()}))
        }).unwrap();
        assert_eq!(offsets, [0, 100]);
        assert_eq!(items.len(), 101);
        assert_eq!(client.artist_followed(42, &items[100].artist), Some(true));
        let failed = client
            .followed_artists_request(42, &canceled, |_, _, _| Ok(json!({"count":12,"items":[]})));
        assert!(matches!(failed, Err(WebAudioError::Pagination)));
        assert_eq!(client.artist_followed(42, &items[100].artist), Some(true));
    }

    #[test]
    fn mutations_update_only_after_success_and_an_older_list_cannot_undo_a_follow() {
        let client = client();
        let target = artist();
        let canceled = AtomicBool::new(false);
        let mut methods = Vec::new();
        let stale = client.followed_artists_request(42, &canceled, |_, _, _| {
            client.set_artist_follow_request(
                42,
                &target,
                true,
                &canceled,
                |_, method, params| {
                    assert!(params.contains(&("artist_id", target.id.clone())));
                    methods.push(method);
                    Ok(json!(1))
                },
            )?;
            Ok(json!({"count":0,"items":[]}))
        });
        assert!(matches!(stale, Err(WebAudioError::Canceled)));
        assert_eq!(methods, ["audio.followArtist"]);
        assert_eq!(client.artist_followed(42, &target), Some(true));
        let failure = client.set_artist_follow_request(42, &target, false, &canceled, |_, _, _| {
            Err(WebAudioError::Network)
        });
        assert!(matches!(failure, Err(WebAudioError::Network)));
        assert_eq!(client.artist_followed(42, &target), Some(true));
        client
            .set_artist_follow_request(42, &target, false, &canceled, |_, method, _| {
                assert_eq!(method, "audio.unfollowArtist");
                Ok(json!(1))
            })
            .unwrap();
        assert_eq!(client.artist_followed(42, &target), Some(false));
    }

    #[test]
    fn dislike_sends_the_web_players_method_and_track_id() {
        let client = client();
        let canceled = AtomicBool::new(false);
        for (dislike, expected) in [(true, "audio.addDislike"), (false, "audio.removeDislike")] {
            client
                .set_dislike_request(42, (-7, 91), dislike, &canceled, |_, method, params| {
                    assert_eq!(method, expected);
                    assert_eq!(params, vec![("audio_ids", "-7_91".to_owned())]);
                    Ok(json!(1))
                })
                .unwrap();
        }
        assert!(matches!(
            client.set_dislike_request(42, (-7, 91), true, &canceled, |_, _, _| Ok(json!(0))),
            Err(WebAudioError::Rejected)
        ));
        assert!(matches!(
            client.set_dislike_request(42, (0, 91), true, &canceled, |_, _, _| unreachable!()),
            Err(WebAudioError::Format)
        ));
    }

    #[test]
    fn artist_picks_page_through_vks_cursor_and_search_by_name() {
        let client = client();
        let canceled = AtomicBool::new(false);
        let first = client
            .artist_picks_request(42, None, &canceled, |_, method, params| {
                assert_eq!(method, "audio.getOnboardingArtists");
                assert!(params.is_empty());
                Ok(json!({"items":[
                    {"id":"artist-1","name":"One","photo":[{"url":"https://sun.example.test/1.jpg","width":300,"height":300}]},
                    {"id":"artist-2","name":"Two"}
                ],"next_from":"page-2"}))
            })
            .unwrap();
        assert_eq!(first.artists.len(), 2);
        assert_eq!(first.artists[0].artist.name, "One");
        assert!(first.artists[0].image_url.is_some());
        assert_eq!(first.next_from.as_deref(), Some("page-2"));
        let last = client
            .artist_picks_request(42, Some("page-2"), &canceled, |_, _, params| {
                assert_eq!(params, vec![("start_from", "page-2".to_owned())]);
                Ok(json!({"items":[{"id":"artist-3","name":"Three"}],"next_from":""}))
            })
            .unwrap();
        assert_eq!(last.next_from, None);
        assert!(matches!(
            client.artist_picks_request(42, None, &canceled, |_, _, _| {
                Ok(json!({"items":[],"next_from":7}))
            }),
            Err(WebAudioError::Format)
        ));
        let found = client
            .search_artist_picks_request(42, "  One ", &canceled, |_, method, params| {
                assert_eq!(method, "audio.searchArtists");
                assert!(params.contains(&("q", "One".into())));
                Ok(json!({"count":1,"items":[{"id":"artist-1","name":"One"}]}))
            })
            .unwrap();
        assert_eq!(found[0].artist.id, "artist-1");
        assert!(
            client
                .search_artist_picks_request(42, "  ", &canceled, |_, _, _| unreachable!())
                .is_err()
        );
    }

    #[test]
    fn account_changes_and_cancellation_discard_late_subscriptions() {
        let client = client();
        let target = artist();
        let canceled = AtomicBool::new(false);
        let result = client.set_artist_follow_request(42, &target, true, &canceled, |_, _, _| {
            canceled.store(true, Ordering::Release);
            Ok(json!(1))
        });
        assert!(matches!(result, Err(WebAudioError::Canceled)));
        assert_eq!(client.artist_followed(42, &target), Some(false));
        canceled.store(false, Ordering::Release);
        let result = client.followed_artists_request(42, &canceled, |_, _, _| {
            client.accept_profile(43).unwrap();
            Ok(json!({"count":0,"items":[]}))
        });
        assert!(matches!(result, Err(WebAudioError::Unverified)));
        assert_eq!(client.artist_followed(43, &target), None);
    }
}
