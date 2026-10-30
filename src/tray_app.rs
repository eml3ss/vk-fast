//! Closing may hide the native window only while a working tray can restore it.
use super::*;
use crate::system_tray::{Command, SystemTray};

#[derive(Default)]
pub(super) struct State {
    service: Option<SystemTray>,
    attempted: bool,
    failed: bool,
    hidden: bool,
    native_hidden: bool,
    quitting: bool,
    wayland: bool,
}

impl State {
    pub(super) fn detach(&mut self) {
        self.service = None;
    }
    pub(super) fn unavailable(&self) -> bool {
        self.failed
            || (self.attempted
                && self
                    .service
                    .as_ref()
                    .is_none_or(|service| !service.available()))
    }
}

impl MusicApp {
    pub(super) fn set_tray_enabled(&mut self, ctx: &egui::Context, enabled: bool) {
        self.minimize_to_tray = enabled;
        self.tray_save_failed = !save_tray_preference(enabled);
        self.tray.attempted = false;
        self.tray.failed = false;
        if !enabled {
            if self.tray.hidden {
                self.show_application(ctx);
            }
            self.tray.detach();
        }
        ctx.request_repaint();
    }

    pub(super) fn request_quit(&mut self, ctx: &egui::Context) {
        self.tray.quitting = true;
        self.tray.native_hidden = false;
        self.window_reopen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    pub(super) fn show_application(&mut self, ctx: &egui::Context) {
        if self.tray.native_hidden {
            self.tray.native_hidden = false;
            self.tray.hidden = false;
            self.window_reopen = true;
            crate::diagnostics::event("tray", "restore: recreate_closed_surface");
            ctx.request_repaint();
            return;
        }
        let recreate = self.tray.wayland
            && (self.tray.hidden || !ctx.input(|input| input.focused))
            && !self.tray.quitting;
        self.tray.hidden = false;
        if recreate {
            // Wayland has no generic unminimize/focus operation in winit.
            // Reuse the resident-window handoff: audio and account state stay
            // alive while a fresh, visible native surface replaces this one.
            self.window_reopen = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if self.tray.wayland {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Critical,
            ));
        }
    }

    pub(super) fn handle_tray_command(&mut self, ctx: &egui::Context, command: Command) {
        if self.tray.quitting {
            return;
        }
        match command {
            Command::Show => self.show_application(ctx),
            Command::TogglePlayback => {
                self.handle_system_media_command(ctx, crate::system_media::Command::Toggle)
            }
            Command::Next => {
                self.handle_system_media_command(ctx, crate::system_media::Command::Next)
            }
            Command::Quit => self.request_quit(ctx),
            Command::Unavailable => {
                self.tray.failed = true;
                if self.tray.hidden {
                    self.show_application(ctx);
                }
            }
        }
    }

    fn intercept_tray_close(&mut self, ctx: &egui::Context, available: bool) {
        if self.minimize_to_tray
            && available
            && !self.tray.quitting
            && !self.window_reopen
            && ctx.input(|input| input.viewport().close_requested())
        {
            self.tray.hidden = true;
            if self.tray.wayland {
                self.tray.native_hidden = true;
                crate::diagnostics::event("tray", "close: park_resident_app");
                return;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            crate::diagnostics::event("tray", "close: hide_native_window");
        }
    }

    pub(super) fn poll_system_tray(&mut self, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        #[cfg(target_os = "linux")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Some(frame) = frame {
                self.tray.wayland = frame
                    .window_handle()
                    .ok()
                    .is_some_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)));
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = frame;
        if self.demo {
            return;
        }
        if self.minimize_to_tray && !self.tray.attempted {
            self.tray.attempted = true;
            match SystemTray::new(self.repaint_target.with_context(ctx), self.language) {
                Ok(service) => self.tray.service = Some(service),
                Err(error) => {
                    eprintln!("VK Fast system tray: {error}");
                    self.tray.failed = true;
                }
            }
        }
        let commands = self
            .tray
            .service
            .as_mut()
            .map(SystemTray::drain_commands)
            .unwrap_or_default();
        for command in commands {
            self.handle_tray_command(ctx, command);
        }
        let available = self
            .tray
            .service
            .as_ref()
            .is_some_and(SystemTray::available);
        if available {
            self.tray.failed = false;
        }
        if self.tray.hidden && !available && !self.tray.quitting {
            self.show_application(ctx);
        }
        self.intercept_tray_close(ctx, available);
        let playing = matches!(
            self.playback_snapshot().state,
            PlaybackState::Playing | PlaybackState::Loading
        );
        if let Some(tray) = &mut self.tray.service {
            tray.set_state(self.language, playing);
        }
    }

    pub(crate) fn tray_background_requested(&self) -> bool {
        self.tray.native_hidden && !self.tray.quitting
    }
    pub(crate) fn tray_quit_requested(&self) -> bool {
        self.tray.quitting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{page, search_test_app};

    fn close(app: &mut MusicApp, available: bool) -> egui::LogicOutput {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput {
            focused: false,
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        ctx.run_logic(&raw, |ctx| app.intercept_tray_close(ctx, available))
    }

    #[test]
    fn enabled_close_hides_without_touching_the_current_track_or_committed_time() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let track = page(1, &[1]).items.remove(0);
        app.playing = Some(track.clone());
        app.play_queue = Some(vec![track]);
        app.resume_position = Some(42.0);
        app.minimize_to_tray = true;
        let output = close(&mut app, true);
        let commands = &output.viewport_commands[&egui::ViewportId::ROOT];
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(commands.contains(&egui::ViewportCommand::Visible(false)));
        assert!(app.tray.hidden);
        assert_eq!(app.resume_position, Some(42.0));
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn wayland_restore_recreates_only_the_native_surface_and_retains_playback_state() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.tray.wayland = true;
        app.tray.hidden = true;
        let track = page(1, &[1]).items.remove(0);
        app.playing = Some(track.clone());
        app.play_queue = Some(vec![track.clone()]);
        app.resume_position = Some(42.0);
        let output = ctx.run_logic(&egui::RawInput::default(), |ctx| app.show_application(ctx));
        assert!(
            output.viewport_commands[&egui::ViewportId::ROOT]
                .contains(&egui::ViewportCommand::Close)
        );
        assert!(app.window_reopen && !app.tray.hidden);
        let replacement = egui::Context::default();
        app.prepare_reopened_window(&replacement);
        assert!(!app.window_reopen && !app.tray.hidden);
        assert_eq!(app.playing.as_ref().unwrap().key(), track.key());
        assert_eq!(app.play_queue.as_ref().unwrap().len(), 1);
        assert_eq!(app.resume_position, Some(42.0));
        assert_eq!(app.player.decoder_starts(), 0);
    }

    #[test]
    fn wayland_tray_close_releases_the_surface_and_restore_does_not_close_twice() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.minimize_to_tray = true;
        app.tray.wayland = true;
        app.resume_position = Some(42.0);
        let output = close(&mut app, true);
        assert!(app.tray_background_requested());
        assert!(app.tray.hidden);
        // Wayland cannot hide a winit surface with Visible(false). Let the
        // actual close finish so the shell can park the resident app instead.
        assert!(
            output
                .viewport_commands
                .get(&egui::ViewportId::ROOT)
                .is_none_or(
                    |commands| !commands.contains(&egui::ViewportCommand::CancelClose)
                        && !commands.contains(&egui::ViewportCommand::Visible(false))
                )
        );
        let output = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            app.handle_tray_command(ctx, Command::Show);
        });
        assert!(!app.tray_background_requested());
        assert!(app.window_reopen && !app.tray.hidden);
        assert!(
            output
                .viewport_commands
                .get(&egui::ViewportId::ROOT)
                .is_none_or(|commands| !commands.contains(&egui::ViewportCommand::Close))
        );
        assert_eq!(app.resume_position, Some(42.0));
    }

    #[test]
    fn disabled_unavailable_quit_and_window_recreation_never_cancel_a_close() {
        for (enabled, available, quit, reopen) in [
            (false, true, false, false),
            (true, false, false, false),
            (true, true, true, false),
            (true, true, false, true),
        ] {
            let ctx = egui::Context::default();
            let mut app = search_test_app(&ctx);
            app.minimize_to_tray = enabled;
            app.tray.quitting = quit;
            app.window_reopen = reopen;
            let output = close(&mut app, available);
            assert!(
                output
                    .viewport_commands
                    .get(&egui::ViewportId::ROOT)
                    .is_none_or(|commands| !commands.contains(&egui::ViewportCommand::CancelClose))
            );
            assert!(!app.tray.hidden);
        }
    }

    #[test]
    fn show_host_loss_disable_and_exit_have_distinct_safe_window_actions() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.tray.hidden = true;
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            app.handle_tray_command(ctx, Command::Unavailable)
        });
        assert!(!app.tray.hidden && app.tray.failed);
        app.tray.hidden = true;
        app.set_tray_enabled(&ctx, false);
        assert!(!app.tray.hidden && !app.minimize_to_tray);
        app.window_reopen = true;
        app.handle_tray_command(&ctx, Command::Quit);
        assert!(app.tray.quitting && !app.window_reopen);
        let output = close(&mut app, true);
        assert!(
            output
                .viewport_commands
                .get(&egui::ViewportId::ROOT)
                .is_none_or(|commands| !commands.contains(&egui::ViewportCommand::CancelClose))
        );
        app.handle_tray_command(&ctx, Command::Show);
        assert!(app.tray.quitting);
    }
}
