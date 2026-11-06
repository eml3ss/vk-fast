//! Native playlist editing and removal, including VK's own cover upload flow.
use std::io::Cursor as ImageCursor;

use super::*;

pub const PLAYLIST_DESCRIPTION_MAX_CHARS: usize = 4096;
const COVER_MAX_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PlaylistEditDetails {
    pub playlist: AlbumRef,
    pub description: String,
    pub can_edit: bool,
    /// Includes removing a followed copy from the current account's library.
    pub can_delete: bool,
    pub is_following: bool,
    /// Discovery/search visibility, not privacy or access control.
    pub discoverable: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PlaylistCoverChange {
    #[default]
    Keep,
    Replace(Vec<u8>),
    Remove,
}

#[derive(Clone, Debug)]
pub struct PlaylistEditUpdate {
    pub title: String,
    pub description: String,
    pub discoverable: Option<bool>,
    pub cover: PlaylistCoverChange,
}

impl WebAudioClient {
    pub fn playlist_edit_details(
        &self,
        expected_owner: i64,
        target: &AlbumRef,
        canceled: &AtomicBool,
    ) -> Result<PlaylistEditDetails, WebAudioError> {
        self.playlist_manage_metadata(
            expected_owner,
            target,
            canceled,
            &mut |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
        .map(|(_, details)| details)
    }

    pub fn edit_playlist(
        &self,
        expected_owner: i64,
        target: &AlbumRef,
        update: &PlaylistEditUpdate,
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        self.edit_playlist_request(
            expected_owner,
            target,
            update,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
            |generation, url, bytes| {
                self.upload_playlist_cover(expected_owner, generation, url, bytes, canceled)
            },
        )
    }

    pub fn delete_playlist(
        &self,
        expected_owner: i64,
        target: &AlbumRef,
        canceled: &AtomicBool,
    ) -> Result<(), WebAudioError> {
        self.delete_playlist_request(
            expected_owner,
            target,
            canceled,
            |generation, method, params| {
                self.detail_request(expected_owner, generation, method, params, canceled)
            },
        )
    }

    pub(super) fn playlist_manage_metadata(
        &self,
        expected_owner: i64,
        target: &AlbumRef,
        canceled: &AtomicBool,
        request: &mut impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<(u64, PlaylistEditDetails), WebAudioError> {
        if target.owner_id == 0 || target.owner_id.checked_abs().is_none() || target.id <= 0 {
            return Err(WebAudioError::Format);
        }
        let generation = self.home_owner(expected_owner, canceled)?;
        let mut params = vec![
            ("owner_id", target.owner_id.to_string()),
            ("playlist_id", target.id.to_string()),
        ];
        if let Some(access_key) = &target.access_key {
            params.push(("access_key", access_key.clone()));
        }
        let value = request(generation, "audio.getPlaylistById", params)?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        let source = value.get("playlist").unwrap_or(&value);
        let mut playlist = AlbumRef::from_value(source).ok_or(WebAudioError::Format)?;
        if playlist.owner_id != target.owner_id || playlist.id != target.id {
            return Err(WebAudioError::Format);
        }
        playlist.title = home_text(&playlist.title, 4096);
        let permission = |key| {
            native_bool(source.get("permissions").and_then(|value| value.get(key))) == Some(true)
        };
        // The native following service removes the current user's followed
        // identity with audio.deletePlaylist even when permissions.delete is
        // false. Never substitute the original creator's identity here.
        let own_followed_copy = playlist.owner_id == expected_owner
            && native_bool(source.get("is_following")) == Some(true);
        let details = PlaylistEditDetails {
            playlist,
            description: playlist_description(source.get("description")).unwrap_or_default(),
            can_edit: playlist_can_add_tracks(source),
            can_delete: permission("delete") || own_followed_copy,
            is_following: native_bool(source.get("is_following")) == Some(true),
            discoverable: native_bool(source.get("no_discover")).map(|hidden| !hidden),
        };
        Ok((generation, details))
    }

    fn edit_playlist_request(
        &self,
        expected_owner: i64,
        target: &AlbumRef,
        update: &PlaylistEditUpdate,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
        mut upload: impl FnMut(u64, &str, &[u8]) -> Result<Value, WebAudioError>,
    ) -> Result<(), WebAudioError> {
        let title = update.title.trim();
        if title.is_empty()
            || title.chars().count() > PLAYLIST_TITLE_MAX_CHARS
            || title.chars().any(char::is_control)
            || update.description.chars().count() > PLAYLIST_DESCRIPTION_MAX_CHARS
            || update
                .description
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(WebAudioError::Format);
        }
        if let PlaylistCoverChange::Replace(bytes) = &update.cover {
            validate_cover(bytes)?;
        }
        let (generation, details) =
            self.playlist_manage_metadata(expected_owner, target, canceled, &mut request)?;
        if !details.can_edit || (update.discoverable.is_some() && details.discoverable.is_none()) {
            return Err(WebAudioError::Rejected);
        }
        let mut params = vec![
            ("owner_id", target.owner_id.to_string()),
            ("playlist_id", target.id.to_string()),
            ("title", title.into()),
            ("description", update.description.clone()),
        ];
        if let Some(discoverable) = update.discoverable {
            params.push(("no_discover", if discoverable { "0" } else { "1" }.into()));
        }
        // Omitting audio_ids preserves the list of tracks. An empty audio_ids
        // value in a metadata-only editor must not accidentally erase it.
        let edited = request(generation, "audio.editPlaylist", params)?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        mutation_succeeded(&edited)?;
        match &update.cover {
            PlaylistCoverChange::Keep => {}
            PlaylistCoverChange::Remove => {
                let result = request(
                    generation,
                    "audio.deletePlaylistCoverPhoto",
                    vec![
                        ("owner_id", target.owner_id.to_string()),
                        ("playlist_id", target.id.to_string()),
                    ],
                )?;
                self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
                mutation_succeeded(&result)?;
            }
            PlaylistCoverChange::Replace(bytes) => {
                let server = request(
                    generation,
                    "photos.getAudioPlaylistCoverUploadServer",
                    vec![("owner_id", target.owner_id.to_string())],
                )?;
                self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
                let url = server
                    .get("upload_url")
                    .or_else(|| server.get("fallback_upload_url"))
                    .and_then(Value::as_str)
                    .filter(|url| valid_upload_url(url))
                    .ok_or(WebAudioError::Format)?;
                let uploaded = upload(generation, url, bytes)?;
                self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
                if !uploaded.is_object()
                    || uploaded.as_object().is_none_or(|object| object.is_empty())
                    || uploaded.get("error_code").is_some()
                    || uploaded.get("error").is_some()
                {
                    return Err(WebAudioError::Rejected);
                }
                let photo = request(
                    generation,
                    "photos.saveAudioPlaylistCover",
                    vec![
                        ("owner_id", target.owner_id.to_string()),
                        ("response_json", uploaded.to_string()),
                    ],
                )?;
                self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
                let hash = photo
                    .get("hash")
                    .and_then(Value::as_str)
                    .filter(|hash| !hash.is_empty() && hash.len() <= 65536)
                    .ok_or(WebAudioError::Format)?;
                let image = photo
                    .get("photo")
                    .filter(|value| value.is_object() || value.is_array())
                    .ok_or(WebAudioError::Format)?;
                let result = request(
                    generation,
                    "audio.setPlaylistCoverPhoto",
                    vec![
                        ("hash", hash.into()),
                        ("photo", image.to_string()),
                        ("playlist_id", target.id.to_string()),
                        ("playlist_owner_id", target.owner_id.to_string()),
                    ],
                )?;
                self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
                if !result.is_object() {
                    return Err(WebAudioError::Format);
                }
            }
        }
        Ok(())
    }

    fn delete_playlist_request(
        &self,
        expected_owner: i64,
        target: &AlbumRef,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
    ) -> Result<(), WebAudioError> {
        let (generation, details) =
            self.playlist_manage_metadata(expected_owner, target, canceled, &mut request)?;
        if !details.can_delete {
            return Err(WebAudioError::Rejected);
        }
        let result = request(
            generation,
            "audio.deletePlaylist",
            vec![
                ("owner_id", target.owner_id.to_string()),
                ("playlist_id", target.id.to_string()),
            ],
        )?;
        self.commit_home_streams(expected_owner, generation, HashMap::new(), canceled)?;
        mutation_succeeded(&result)
    }

    fn upload_playlist_cover(
        &self,
        owner: i64,
        generation: u64,
        url: &str,
        bytes: &[u8],
        canceled: &AtomicBool,
    ) -> Result<Value, WebAudioError> {
        validate_cover(bytes)?;
        if !valid_upload_url(url) {
            return Err(WebAudioError::Format);
        }
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| WebAudioError::Network)?;
        let boundary = format!(
            "vk-fast-{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let mut data = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"cover.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").into_bytes();
        data.extend_from_slice(bytes);
        data.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        // This client has no cookie jar. The upload receives only the chosen
        // image, without the website cookies or API token. Redirects are off.
        let response = self
            .http
            .post(url)
            .header(
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(data)
            .send()
            .map_err(network)?;
        let text = body(response)?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        serde_json::from_str(&text).map_err(|_| WebAudioError::Format)
    }
}

fn native_bool(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::Number(value) if value.as_u64() == Some(0) => Some(false),
        Value::Number(value) if value.as_u64() == Some(1) => Some(true),
        _ => None,
    }
}

pub(super) fn mutation_succeeded(value: &Value) -> Result<(), WebAudioError> {
    if native_bool(Some(value)) == Some(true) || native_bool(value.get("success")) == Some(true) {
        Ok(())
    } else {
        Err(WebAudioError::Rejected)
    }
}

pub(super) fn valid_upload_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none_or(|port| port == 443)
            && url.fragment().is_none()
            && url.host_str().is_some_and(|host| {
                ["vk.ru", "vk.com", "userapi.com", "vkuserphoto.ru"]
                    .iter()
                    .any(|base| {
                        host == *base
                            || host
                                .strip_suffix(base)
                                .is_some_and(|prefix| prefix.ends_with('.'))
                    })
            })
    })
}

fn validate_cover(bytes: &[u8]) -> Result<(), WebAudioError> {
    if bytes.len() > COVER_MAX_BYTES {
        return Err(WebAudioError::TooLarge);
    }
    if image::guess_format(bytes).ok() != Some(image::ImageFormat::Jpeg) {
        return Err(WebAudioError::Format);
    }
    let (width, height) =
        image::ImageReader::with_format(ImageCursor::new(bytes), image::ImageFormat::Jpeg)
            .into_dimensions()
            .map_err(|_| WebAudioError::Format)?;
    if !(300..=7000).contains(&width) || !(300..=7000).contains(&height) {
        return Err(WebAudioError::Format);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use serde_json::json;

    fn client() -> WebAudioClient {
        let client = WebAudioClient::new(vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-management-session".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        {
            let mut state = client.state.lock().unwrap();
            state.verified_owner = Some(42);
            state.generation = 7;
            state.cursor = Some(Cursor {
                owner: 42,
                offset: 100,
                ..Default::default()
            });
            state.mix = Some(MixSession {
                token: Zeroizing::new("synthetic-radio-token".into()),
                expires: now() + 3600,
                api_host: "web.api.vk.ru",
                previous_batch: vec![(42, 71)],
                previous_source: MixSource::Personal,
            });
        }
        client
    }

    fn reference(owner: i64) -> AlbumRef {
        AlbumRef {
            owner_id: owner,
            id: 10,
            title: "Playlist".into(),
            access_key: Some("synthetic-access".into()),
            cover_url: None,
        }
    }

    fn metadata(owner: i64) -> Value {
        json!({"owner_id":owner,"id":10,"title":"Playlist","description":"First<br>Second &amp; third","type":"ugc","is_following":false,"permissions":{"edit":true,"delete":true},"no_discover":false})
    }

    fn update() -> PlaylistEditUpdate {
        PlaylistEditUpdate {
            title: "  New name  ".into(),
            description: "New description\nSecond line".into(),
            discoverable: None,
            cover: PlaylistCoverChange::Keep,
        }
    }

    fn cover(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&image::DynamicImage::new_rgb8(width, height))
            .unwrap();
        bytes
    }

    #[test]
    fn metadata_verifies_identity_permissions_and_discovery_without_rewriting_originals() {
        let client = client();
        let canceled = AtomicBool::new(false);
        let (_, details) = client
            .playlist_manage_metadata(42, &reference(42), &canceled, &mut |_, method, params| {
                assert_eq!(method, "audio.getPlaylistById");
                assert_eq!(
                    params,
                    [
                        ("owner_id", "42".into()),
                        ("playlist_id", "10".into()),
                        ("access_key", "synthetic-access".into())
                    ]
                );
                Ok(metadata(42))
            })
            .unwrap();
        assert!(details.can_edit && details.can_delete && !details.is_following);
        assert_eq!(details.discoverable, Some(true));
        assert_eq!(details.description, "First\nSecond & third");
        let mut saved = metadata(42);
        saved["is_following"] = json!(true);
        saved["permissions"] = json!({"edit":false,"delete":false});
        saved["original"] = json!({"owner_id":-99,"playlist_id":999});
        saved.as_object_mut().unwrap().remove("no_discover");
        let (_, details) = client
            .playlist_manage_metadata(42, &reference(42), &canceled, &mut |_, _, _| {
                Ok(saved.clone())
            })
            .unwrap();
        assert!(!details.can_edit && details.can_delete && details.is_following);
        assert_eq!(details.playlist.owner_id, 42);
        assert_eq!(details.playlist.id, 10);
        assert_eq!(details.discoverable, None);
        assert!(matches!(
            client.playlist_manage_metadata(42, &reference(42), &canceled, &mut |_, _, _| Ok(
                metadata(-99)
            )),
            Err(WebAudioError::Format)
        ));
    }

    #[test]
    fn metadata_only_edit_preserves_tracks_and_radio_and_sends_one_native_update() {
        let client = client();
        let calls = RefCell::new(Vec::new());
        let mut update = update();
        update.discoverable = Some(false);
        client
            .edit_playlist_request(
                42,
                &reference(42),
                &update,
                &AtomicBool::new(false),
                |_, method, params| {
                    calls.borrow_mut().push(method);
                    if method == "audio.getPlaylistById" {
                        return Ok(metadata(42));
                    }
                    assert_eq!(method, "audio.editPlaylist");
                    assert_eq!(
                        params,
                        [
                            ("owner_id", "42".into()),
                            ("playlist_id", "10".into()),
                            ("title", "New name".into()),
                            ("description", "New description\nSecond line".into()),
                            ("no_discover", "1".into())
                        ]
                    );
                    assert!(!params.iter().any(|(key, _)| *key == "audio_ids"));
                    Ok(json!(1))
                },
                |_, _, _| panic!("Metadata edit must not upload a cover"),
            )
            .unwrap();
        assert_eq!(
            *calls.borrow(),
            ["audio.getPlaylistById", "audio.editPlaylist"]
        );
        let state = client.state.lock().unwrap();
        assert_eq!(state.cursor.as_ref().unwrap().offset, 100);
        assert_eq!(state.mix.as_ref().unwrap().previous_batch, [(42, 71)]);
    }

    #[test]
    fn followed_copy_removal_uses_only_the_current_users_local_identity() {
        let client = client();
        let mut saved = metadata(42);
        saved["permissions"] = json!({"edit":false,"delete":false});
        saved["is_following"] = json!(true);
        saved["original"] = json!({"owner_id":-99,"playlist_id":999});
        let calls = RefCell::new(Vec::new());
        client
            .delete_playlist_request(
                42,
                &reference(42),
                &AtomicBool::new(false),
                |_, method, params| {
                    calls.borrow_mut().push(method);
                    if method == "audio.getPlaylistById" {
                        return Ok(saved.clone());
                    }
                    assert_eq!(method, "audio.deletePlaylist");
                    assert_eq!(
                        params,
                        [("owner_id", "42".into()), ("playlist_id", "10".into())]
                    );
                    Ok(json!(1))
                },
            )
            .unwrap();
        assert_eq!(
            *calls.borrow(),
            ["audio.getPlaylistById", "audio.deletePlaylist"]
        );
        saved["owner_id"] = json!(-99);
        assert_eq!(
            client.delete_playlist_request(
                42,
                &reference(-99),
                &AtomicBool::new(false),
                |_, method, _| {
                    assert_eq!(method, "audio.getPlaylistById");
                    Ok(saved.clone())
                }
            ),
            Err(WebAudioError::Rejected)
        );
    }

    #[test]
    fn edit_permissions_are_fresh_and_rejected_writes_are_never_retried() {
        let client = client();
        let mut denied = metadata(42);
        denied["permissions"] = json!({"edit":false,"delete":false});
        let count = Cell::new(0);
        assert_eq!(
            client.edit_playlist_request(
                42,
                &reference(42),
                &update(),
                &AtomicBool::new(false),
                |_, method, _| {
                    count.set(count.get() + 1);
                    assert_eq!(method, "audio.getPlaylistById");
                    Ok(denied.clone())
                },
                |_, _, _| panic!("Denied edit uploaded")
            ),
            Err(WebAudioError::Rejected)
        );
        assert_eq!(count.get(), 1);
        assert_eq!(
            client.delete_playlist_request(
                42,
                &reference(42),
                &AtomicBool::new(false),
                |_, method, _| {
                    assert_eq!(method, "audio.getPlaylistById");
                    Ok(denied.clone())
                }
            ),
            Err(WebAudioError::Rejected)
        );
        count.set(0);
        assert_eq!(
            client.edit_playlist_request(
                42,
                &reference(42),
                &update(),
                &AtomicBool::new(false),
                |_, method, _| {
                    count.set(count.get() + 1);
                    if method == "audio.getPlaylistById" {
                        Ok(metadata(42))
                    } else {
                        Err(WebAudioError::Network)
                    }
                },
                |_, _, _| panic!()
            ),
            Err(WebAudioError::Network)
        );
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn cover_replacement_follows_verified_native_upload_save_and_attachment_protocol() {
        let client = client();
        let calls = RefCell::new(Vec::new());
        let bytes = cover(512, 512);
        let mut update = update();
        update.cover = PlaylistCoverChange::Replace(bytes.clone());
        client.edit_playlist_request(42, &reference(42), &update, &AtomicBool::new(false), |_,method,params| {
            calls.borrow_mut().push(method);
            Ok(match method {
                "audio.getPlaylistById" => metadata(42),
                "audio.editPlaylist" => json!(1),
                "photos.getAudioPlaylistCoverUploadServer" => { assert_eq!(params,[("owner_id","42".into())]);json!({"upload_url":"https://pu.vk.ru/c123/upload?opaque=value"}) },
                "photos.saveAudioPlaylistCover" => { assert_eq!(params,[("owner_id","42".into()),("response_json",json!({"server":12,"photos_list":"opaque","hash":"upload-hash"}).to_string())]);json!({"hash":"saved-hash","photo":{"id":12}}) },
                "audio.setPlaylistCoverPhoto" => { assert_eq!(params,[("hash","saved-hash".into()),("photo",json!({"id":12}).to_string()),("playlist_id","10".into()),("playlist_owner_id","42".into())]);json!({"photo_300":"https://sun9-1.userapi.com/cover.jpg"}) },
                _ => panic!("Unexpected method"),
            })
        }, |_,url,image| {
            calls.borrow_mut().push("upload");
            assert_eq!(url,"https://pu.vk.ru/c123/upload?opaque=value");
            assert_eq!(image,bytes);
            Ok(json!({"server":12,"photos_list":"opaque","hash":"upload-hash"}))
        }).unwrap();
        assert_eq!(
            *calls.borrow(),
            [
                "audio.getPlaylistById",
                "audio.editPlaylist",
                "photos.getAudioPlaylistCoverUploadServer",
                "upload",
                "photos.saveAudioPlaylistCover",
                "audio.setPlaylistCoverPhoto"
            ]
        );
        update.cover = PlaylistCoverChange::Remove;
        client
            .edit_playlist_request(
                42,
                &reference(42),
                &update,
                &AtomicBool::new(false),
                |_, method, params| {
                    if method == "audio.getPlaylistById" {
                        return Ok(metadata(42));
                    }
                    assert!(matches!(
                        method,
                        "audio.editPlaylist" | "audio.deletePlaylistCoverPhoto"
                    ));
                    if method == "audio.deletePlaylistCoverPhoto" {
                        assert_eq!(
                            params,
                            [("owner_id", "42".into()), ("playlist_id", "10".into())]
                        );
                    }
                    Ok(json!(1))
                },
                |_, _, _| panic!(),
            )
            .unwrap();
    }

    #[test]
    fn canceled_or_stale_management_cannot_continue_to_a_write_or_cover_attachment() {
        let client = client();
        assert!(matches!(
            client.playlist_manage_metadata(
                43,
                &reference(42),
                &AtomicBool::new(false),
                &mut |_, _, _| panic!()
            ),
            Err(WebAudioError::WrongAccount)
        ));
        assert!(matches!(
            client.playlist_manage_metadata(
                42,
                &reference(42),
                &AtomicBool::new(true),
                &mut |_, _, _| panic!()
            ),
            Err(WebAudioError::Canceled)
        ));
        let canceled = AtomicBool::new(false);
        assert_eq!(
            client.edit_playlist_request(
                42,
                &reference(42),
                &update(),
                &canceled,
                |_, method, _| {
                    assert_eq!(method, "audio.getPlaylistById");
                    canceled.store(true, Ordering::Release);
                    Ok(metadata(42))
                },
                |_, _, _| panic!()
            ),
            Err(WebAudioError::Canceled)
        );
        let client = self::client();
        assert_eq!(
            client.delete_playlist_request(
                42,
                &reference(42),
                &AtomicBool::new(false),
                |_, method, _| {
                    assert_eq!(method, "audio.getPlaylistById");
                    client.state.lock().unwrap().generation += 1;
                    Ok(metadata(42))
                }
            ),
            Err(WebAudioError::Unverified)
        );
        let client = self::client();
        let canceled = AtomicBool::new(false);
        let mut update = update();
        update.cover = PlaylistCoverChange::Replace(cover(512, 512));
        assert_eq!(
            client.edit_playlist_request(
                42,
                &reference(42),
                &update,
                &canceled,
                |_, method, _| Ok(match method {
                    "audio.getPlaylistById" => metadata(42),
                    "audio.editPlaylist" => json!(1),
                    "photos.getAudioPlaylistCoverUploadServer" =>
                        json!({"upload_url":"https://pu.vk.ru/upload"}),
                    _ => panic!("Canceled upload progressed to {method}"),
                }),
                |_, _, _| {
                    canceled.store(true, Ordering::Release);
                    Ok(json!({"server":1}))
                }
            ),
            Err(WebAudioError::Canceled)
        );
    }

    #[test]
    fn invalid_names_images_and_untrusted_upload_destinations_fail_before_sending_data() {
        for invalid in [
            "http://pu.vk.ru/upload",
            "https://pu.vk.ru.evil.test/upload",
            "https://evilvk.ru/upload",
            "https://user:pass@pu.vk.ru/upload",
            "https://pu.vk.ru:444/upload",
            "https://127.0.0.1/upload",
            "file:///tmp/cover",
            "https://pu.vk.ru/upload#fragment",
        ] {
            assert!(!valid_upload_url(invalid), "{invalid}");
        }
        assert!(valid_upload_url("https://pu.vk.ru/upload?opaque=1"));
        assert_eq!(validate_cover(b"invalid"), Err(WebAudioError::Format));
        assert_eq!(
            validate_cover(&vec![0; COVER_MAX_BYTES + 1]),
            Err(WebAudioError::TooLarge)
        );
        assert_eq!(validate_cover(&cover(299, 512)), Err(WebAudioError::Format));
        let client = client();
        for title in [
            String::new(),
            " ".into(),
            "a".repeat(101),
            "Control\0name".into(),
        ] {
            let mut update = update();
            update.title = title;
            assert_eq!(
                client.edit_playlist_request(
                    42,
                    &reference(42),
                    &update,
                    &AtomicBool::new(false),
                    |_, _, _| panic!(),
                    |_, _, _| panic!()
                ),
                Err(WebAudioError::Format)
            );
        }
        let mut update = update();
        update.cover = PlaylistCoverChange::Replace(b"invalid".to_vec());
        assert_eq!(
            client.edit_playlist_request(
                42,
                &reference(42),
                &update,
                &AtomicBool::new(false),
                |_, _, _| panic!(),
                |_, _, _| panic!()
            ),
            Err(WebAudioError::Format)
        );
    }
}
