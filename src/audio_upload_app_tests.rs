use super::*;
use crate::app::tests::{account_ui_app, account_ui_frame, account_ui_text_rect, page};
use crate::app::tests::{account_ui_click, account_ui_shapes};

fn connected(ctx: &egui::Context, language: Language) -> MusicApp {
    let mut app = account_ui_app(ctx, language);
    let client = WebAudioClient::new(vec![crate::web_session::SessionCookie {
        domain: ".vk.ru".into(),
        path: "/".into(),
        name: "remixsid".into(),
        value: "synthetic-upload-test".into(),
        secure: true,
        expires_unix: None,
    }])
    .unwrap();
    app.session = Some(Session {
        profile: Profile {
            id: 42,
            first_name: "Test".into(),
            last_name: "Account".into(),
            avatar_url: None,
        },
        client: None,
        cache_warning: false,
        music: Some(client),
    });
    app
}

#[test]
fn upload_form_fits_both_languages_and_exposes_explicit_upload_without_network() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = connected(&ctx, language);
        app.open_audio_upload();
        let mut tick = 0;
        let size = egui::vec2(800.0, 540.0);
        for _ in 0..2 {
            account_ui_frame(&ctx, &mut app, size, &mut tick, vec![]);
        }
        let output = account_ui_frame(&ctx, &mut app, size, &mut tick, vec![]);
        for caption in [
            language.text("Название трека", "Track title"),
            language.text("Исполнитель", "Artist"),
            language.text("Загрузить в VK", "Upload to VK"),
            language.text("Отмена", "Cancel"),
        ] {
            let rect = account_ui_text_rect(&output, caption).unwrap();
            assert!(rect.top() >= 0.0 && rect.bottom() < 540.0);
        }
        assert!(
            app.audio_upload.request.is_none()
                && app.audio_upload.picker.is_none()
                && app.workers.is_empty()
        );
    }
}

#[test]
fn library_plus_is_to_the_left_of_icon_only_refresh_and_opens_the_upload_form() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = connected(&ctx, language);
        app.page = MusicPage::Library;
        let size = egui::vec2(1120.0, 720.0);
        let mut tick = 0;
        account_ui_frame(&ctx, &mut app, size, &mut tick, vec![]);
        let output = account_ui_frame(&ctx, &mut app, size, &mut tick, vec![]);
        assert!(account_ui_text_rect(&output, language.text("Обновить", "Refresh")).is_none());
        let heading = account_ui_shapes(&output)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text)
                    if text.galley.text() == language.text("Моя музыка", "My music")
                        && text.visual_bounding_rect().left() > 252.0 =>
                {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .unwrap();
        account_ui_click(
            &ctx,
            &mut app,
            size,
            &mut tick,
            egui::pos2(size.x - 8.0 - 38.0 - 8.0 - 19.0, heading.center().y),
        );
        assert!(app.audio_upload.has_modal());
        assert!(app.pending.is_none());
    }
}

#[test]
fn confirmed_upload_merges_one_record_preserves_existing_library_and_requests_refresh() {
    let ctx = egui::Context::default();
    let mut app = connected(&ctx, Language::English);
    app.open_audio_upload();
    app.library.tracks = page(1, &[1]).items;
    let mut track = page(1, &[71]).items.remove(0);
    track.owner_id = 42;
    let (sender, receiver) = mpsc::sync_channel(1);
    app.audio_upload.request = Some(Request {
        owner: 42,
        canceled: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(AtomicU64::new(10000)),
        receiver,
    });
    sender.send(Ok(track)).unwrap();
    app.poll_audio_upload(&ctx);
    assert!(!app.audio_upload.has_modal());
    assert_eq!(app.library.tracks.len(), 2);
    assert_eq!(app.library.tracks[0].key(), (42, 71));
    assert_eq!(app.library.tracks[1].id, 1);
    assert!(app.library_refresh_due);
    assert!(app.track_menu_notice.is_some());
}

#[test]
fn changing_account_discards_late_upload_and_uncertain_save_keeps_form_without_retry() {
    let ctx = egui::Context::default();
    let mut app = connected(&ctx, Language::English);
    app.open_audio_upload();
    let (sender, receiver) = mpsc::sync_channel(1);
    let canceled = Arc::new(AtomicBool::new(false));
    app.audio_upload.request = Some(Request {
        owner: 42,
        canceled: canceled.clone(),
        progress: Arc::new(AtomicU64::new(0)),
        receiver,
    });
    app.session.as_mut().unwrap().profile.id = 77;
    app.poll_audio_upload(&ctx);
    assert!(canceled.load(Ordering::Acquire));
    assert!(sender.send(Err(Error::Read)).is_err());
    assert!(!app.audio_upload.has_modal());
    assert!(app.library.tracks.is_empty());
    app.session.as_mut().unwrap().profile.id = 42;
    app.open_audio_upload();
    let (sender, receiver) = mpsc::sync_channel(1);
    app.audio_upload.request = Some(Request {
        owner: 42,
        canceled: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(AtomicU64::new(10000)),
        receiver,
    });
    sender.send(Err(Error::Uncertain)).unwrap();
    app.poll_audio_upload(&ctx);
    assert_eq!(
        app.audio_upload.form.as_ref().unwrap().error,
        Some(Error::Uncertain)
    );
    assert!(app.audio_upload.request.is_none());
}
