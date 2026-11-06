use super::*;
use serde_json::json;

fn client() -> WebAudioClient {
    let client = WebAudioClient::new(vec![SessionCookie {
        domain: ".vk.ru".into(),
        path: "/".into(),
        name: "remixsid".into(),
        value: "synthetic-upload-session".into(),
        secure: true,
        expires_unix: None,
    }])
    .unwrap();
    client.accept_profile(42).unwrap();
    client
}
fn upload_response() -> Value {
    json!({"server":7,"audio":"opaque selected file","hash":"synthetic-upload-hash"})
}

#[test]
fn native_upload_saves_once_with_chosen_metadata_and_returns_exact_owned_identity() {
    let client = client();
    let canceled = AtomicBool::new(false);
    let mut calls = Vec::new();
    let track=client.upload_audio_with(42," Мой трек "," Исполнитель ",&canceled,|_,method,params| {
        calls.push(method);
        if method=="audio.getUploadServer" {assert!(params.is_empty());return Ok(json!({"upload_url":"https://pu.vk.ru/upload?opaque=1"}));}
        assert_eq!(method,"audio.save");
        assert_eq!(params,vec![("server","7".into()),("audio","opaque selected file".into()),("hash","synthetic-upload-hash".into()),("title","Мой трек".into()),("artist","Исполнитель".into())]);
        Ok(json!({"owner_id":42,"id":71,"title":"Мой трек","artist":"Исполнитель","duration":120}))
    },|_,url| {assert_eq!(url,"https://pu.vk.ru/upload?opaque=1");Ok(upload_response())}).unwrap();
    assert_eq!(calls, ["audio.getUploadServer", "audio.save"]);
    assert_eq!(track.key(), (42, 71));
    assert_eq!(track.library_liked, Some(true));
}
#[test]
fn rejects_untrusted_upload_hosts_and_stale_accounts_before_saving() {
    for url in [
        "https://evil.test/upload",
        "http://pu.vk.ru/upload",
        "https://vk.ru.evil.test/upload",
    ] {
        let client = client();
        let canceled = AtomicBool::new(false);
        let mut calls = 0;
        let result = client.upload_audio_with(
            42,
            "Title",
            "Artist",
            &canceled,
            |_, _, _| {
                calls += 1;
                Ok(json!({"upload_url":url}))
            },
            |_, _| panic!("file must not leave device"),
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
    let client = client();
    let canceled = AtomicBool::new(false);
    let mut calls = 0;
    let result = client.upload_audio_with(
        42,
        "Title",
        "Artist",
        &canceled,
        |_, _, _| {
            calls += 1;
            Ok(json!({"upload_url":"https://pu.vk.ru/upload"}))
        },
        |_, _| {
            client.accept_profile(77).unwrap();
            Ok(upload_response())
        },
    );
    assert!(result.is_err());
    assert_eq!(calls, 1);
}
#[test]
fn failed_upload_and_cancellation_do_not_attempt_a_save() {
    let client = client();
    let canceled = AtomicBool::new(false);
    let mut calls = 0;
    assert!(
        client
            .upload_audio_with(
                42,
                "Title",
                "Artist",
                &canceled,
                |_, _, _| {
                    calls += 1;
                    Ok(json!({"upload_url":"https://pu.vk.ru/upload"}))
                },
                |_, _| Err(Error::Read)
            )
            .is_err()
    );
    assert_eq!(calls, 1);
    canceled.store(true, Ordering::Release);
    assert!(
        client
            .upload_audio_with(
                42,
                "Title",
                "Artist",
                &canceled,
                |_, _, _| panic!("no request after cancel"),
                |_, _| panic!("no upload after cancel")
            )
            .is_err()
    );
}
#[test]
fn ambiguous_save_never_retries_a_possible_committed_mutation() {
    let client = client();
    let canceled = AtomicBool::new(false);
    let mut saves = 0;
    let result = client.upload_audio_with(
        42,
        "Title",
        "Artist",
        &canceled,
        |_, method, _| {
            if method == "audio.getUploadServer" {
                Ok(json!({"upload_url":"https://pu.vk.ru/upload"}))
            } else {
                saves += 1;
                Err(WebAudioError::Timeout)
            }
        },
        |_, _| Ok(upload_response()),
    );
    assert!(matches!(result, Err(Error::Uncertain)));
    assert_eq!(saves, 1);
}
#[test]
fn progress_reader_streams_selected_file_and_stops_on_cancel() {
    let (_temp, source) = crate::audio_upload::tests::source();
    let canceled = Arc::new(AtomicBool::new(false));
    let mut reader = ProgressReader {
        file: source.reader().unwrap().take(source.size),
        canceled: canceled.clone(),
        done: 0,
        total: source.size,
        last: 0,
        progress: |_, _| {},
    };
    let mut buffer = [0; 64];
    assert_eq!(reader.read(&mut buffer).unwrap(), 64);
    canceled.store(true, Ordering::Release);
    assert!(reader.read(&mut buffer).is_err());
    assert_eq!(reader.done, 64);
}
