use super::*;
use crate::{
    app::{
        MusicApp, SettingsSection,
        tests::{account_ui_app, account_ui_click, account_ui_frame, account_ui_text_rect},
    },
    updater::tests::release,
};

fn ready_check(
    state: &mut State,
    result: Result<Option<Release>, Error>,
) -> mpsc::SyncSender<Event> {
    let (sender, receiver) = mpsc::sync_channel(8);
    state.receiver = Some(receiver);
    state.phase = Phase::Checking;
    sender.send(Event::Checked(result)).unwrap();
    sender
}

#[test]
fn new_release_sets_a_persistent_badge_without_opening_or_installing_anything() {
    let mut state = State::default();
    let target = RepaintTarget::default();
    let _sender = ready_check(&mut state, Ok(Some(release("0.1.2"))));
    assert!(!state.tick(&target));
    assert!(state.has_notification() && !state.open);
    assert_eq!(state.phase, Phase::Available);
    assert!(state.prepared.is_none());
    assert!(!state.tick(&target));
    assert!(state.has_notification());
}

#[test]
fn only_confirmed_helper_handoff_requests_real_exit_including_when_in_tray() {
    for result in [Ok(()), Err(Error::Helper)] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        app.minimize_to_tray = true;
        let (sender, receiver) = mpsc::sync_channel(1);
        app.updater.phase = Phase::Starting;
        app.updater.receiver = Some(receiver);
        sender.send(Event::Handoff(result)).unwrap();
        let output = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            if app.updater.tick(&app.repaint_target) {
                app.request_quit(ctx);
            }
        });
        assert_eq!(app.tray_quit_requested(), result.is_ok());
        if result.is_ok() {
            assert!(
                output.viewport_commands[&egui::ViewportId::ROOT]
                    .contains(&egui::ViewportCommand::Close)
            );
        }
    }
}

#[test]
fn automatic_check_runs_at_launch_once_can_be_disabled_and_has_no_per_frame_request_loop() {
    let mut state = State::new(true);
    let target = RepaintTarget::default();
    assert_eq!(state.next_check_in(), Some(Duration::ZERO));
    state.tick(&target);
    assert_eq!(state.phase, Phase::Checking);
    assert!(state.next_check.is_none());
    for _ in 0..20 {
        assert!(!state.tick(&target));
    }
    assert_eq!(state.phase, Phase::Checking);
    assert!(state.next_check_in().is_none());
    let mut disabled = State::new(false);
    disabled.tick(&target);
    assert!(disabled.next_check_in().is_none());
    assert_eq!(disabled.phase, Phase::Idle);
}

#[test]
fn cancel_detaches_a_download_so_stale_progress_or_completion_cannot_reopen_it() {
    let mut state = State::new(false);
    state.phase = Phase::Downloading {
        received: 20,
        total: 100,
    };
    state.release = Some(release("0.1.2"));
    let canceled = Arc::new(AtomicBool::new(false));
    state.canceled = Some(canceled.clone());
    let (sender, receiver) = mpsc::sync_channel(2);
    state.receiver = Some(receiver);
    state.cancel_download();
    assert!(canceled.load(Ordering::Acquire));
    assert!(sender.send(Event::Progress(50, 100)).is_err());
    assert_eq!(state.phase, Phase::Available);
    assert!(state.has_notification());
}

#[test]
fn settings_search_finds_update_controls_and_toggling_keeps_music_state() {
    for language in [Language::Russian, Language::English] {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, language);
        app.settings_open = true;
        app.settings_section = SettingsSection::Interface;
        app.settings_query = language.text("обновления", "updates").into();
        let size = egui::vec2(1120.0, 850.0);
        let mut tick = 0;
        let _ = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
        let toggle = account_ui_text_rect(
            &output,
            language.text(
                "Автоматически проверять обновления",
                "Automatically check for updates",
            ),
        )
        .unwrap();
        let volume = app.player.snapshot().volume;
        account_ui_click(&ctx, &mut app, size, &mut tick, toggle.center());
        assert!(app.updater.automatic && !app.updater.save_failed);
        assert!(app.updater.receiver.is_none());
        assert_eq!(app.player.snapshot().volume, volume);
        assert!(app.workers.is_empty());
    }
}

#[test]
fn notification_is_left_of_settings_and_opens_the_update_dialog_in_both_languages() {
    for language in [Language::Russian, Language::English] {
        for width in [800.0, 1120.0] {
            let ctx = egui::Context::default();
            let mut app: MusicApp = account_ui_app(&ctx, language);
            app.updater.phase = Phase::Available;
            app.updater.release = Some(release("0.1.2"));
            let size = egui::vec2(width, 720.0);
            let mut tick = 0;
            let _ = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            let badge = account_ui_text_rect(&output, "0.1.2").expect("new-version badge");
            assert!(
                badge.left() > 252.0 && badge.right() < width - 80.0,
                "The version badge must precede the two 38px settings/account controls: {badge:?}"
            );
            account_ui_click(&ctx, &mut app, size, &mut tick, badge.center());
            assert!(app.updater.open);
            let output = account_ui_frame(&ctx, &mut app, size, &mut tick, Vec::new());
            assert!(
                account_ui_text_rect(
                    &output,
                    language.text("Загрузить обновление", "Download update")
                )
                .is_some()
            );
            assert!(app.updater.prepared.is_none());
        }
    }
}
