//! Lyrics panel selection, background jobs and local provider settings.
use super::*;
use crate::{
    lyrics::{self, ConfigAction, Error, Event, Mode, Query, Service, Status},
    lyrics_ui,
};
use zeroize::Zeroizing;

#[cfg(test)]
#[path = "lyrics_app_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct State {
    pub(super) active: bool,
    pub(super) open: bool,
    pub(super) fullscreen: bool,
    previous_fullscreen: bool,
    service: Option<Service>,
    query: Option<Query>,
    pending: Option<u64>,
    data: Option<Arc<lyrics::Lyrics>>,
    error: Option<Error>,
    status: Option<Status>,
    settings_busy: bool,
    settings_error: Option<Error>,
    key_input: Zeroizing<String>,
    view: lyrics_ui::State,
    font: Option<Arc<egui::FontData>>,
    font_pending: bool,
    font_frame: Option<u64>,
}

impl State {
    fn cancel_pending(&mut self) {
        if let Some(service) = &self.service {
            service.cancel_fetch();
        }
        self.pending = None;
    }
    pub(super) fn clear_track(&mut self) {
        if let Some(service) = &self.service {
            service.cancel_fetch();
        }
        self.query = None;
        self.pending = None;
        self.data = None;
        self.error = None;
        self.view.reset();
    }
    pub(super) fn rebind(&mut self, ctx: &egui::Context) {
        if let Some(service) = &self.service {
            service.rebind(ctx);
        }
        self.font_pending = self.font.is_some();
        self.view.reset();
    }
    pub(super) fn install_font(&mut self, ctx: &egui::Context) {
        if self.font_pending
            && let Some(font) = &self.font
        {
            let mut fonts = ctx.fonts(|fonts| fonts.definitions().clone());
            let name = "VK Fast lyrics fallback".to_owned();
            fonts.font_data.insert(name.clone(), font.clone());
            for family in [egui::FontFamily::Proportional, theme::semibold()] {
                let fallbacks = fonts.families.entry(family).or_default();
                if !fallbacks.contains(&name) {
                    fallbacks.push(name.clone());
                }
            }
            ctx.set_fonts(fonts);
            self.font_pending = false;
            self.font_frame = Some(ctx.cumulative_frame_nr() + 1);
            self.view.reset();
            ctx.request_repaint();
        }
        if self
            .font_frame
            .is_some_and(|frame| ctx.cumulative_frame_nr() >= frame)
        {
            self.font_frame = None;
            self.view.reset();
        }
    }
}

impl MusicApp {
    fn ensure_lyrics_service(&mut self, ctx: &egui::Context) {
        if self.lyrics.service.is_none() && self.lyrics.settings_error.is_none() {
            match Service::new(ctx) {
                Ok(service) => self.lyrics.service = Some(service),
                Err(error) => {
                    self.lyrics.error = Some(error);
                    self.lyrics.settings_error = Some(error);
                }
            }
        }
    }

    fn apply_lyrics_event(&mut self, event: Event) {
        match event {
            Event::Font(font) => {
                self.lyrics.font = Some(Arc::new(egui::FontData::from_owned(font)));
                self.lyrics.font_pending = true;
            }
            Event::Settings { result, completed } => {
                if completed {
                    self.lyrics.settings_busy = false;
                    self.lyrics.pending = None;
                }
                if completed || !self.lyrics.settings_busy {
                    match result {
                        Ok(status) => {
                            self.lyrics.status = Some(status);
                            if completed {
                                self.lyrics.settings_error = None;
                                self.lyrics.key_input.zeroize();
                                self.lyrics.error = None;
                                self.lyrics.data = None;
                                self.lyrics.view.reset();
                            }
                        }
                        Err(error) => {
                            self.lyrics.settings_error = Some(error);
                            if completed {
                                self.lyrics.error = Some(error);
                            }
                        }
                    }
                }
            }
            Event::Lyrics { id, query, result } => {
                if self.lyrics.pending == Some(id) && self.lyrics.query.as_ref() == Some(&query) {
                    self.lyrics.pending = None;
                    match result {
                        Ok(lyrics) => {
                            self.lyrics.data = Some(lyrics);
                            self.lyrics.error = None;
                            self.lyrics.view.reset();
                        }
                        Err(error) => self.lyrics.error = Some(error),
                    }
                }
            }
        }
    }
    pub(super) fn poll_lyrics(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.lyrics.service.as_ref().and_then(Service::poll) {
            self.apply_lyrics_event(event);
        }
        if self.tray_background_requested()
            || ctx.input(|input| input.viewport().minimized == Some(true))
        {
            if self.lyrics.pending.is_some() {
                self.lyrics.cancel_pending();
            }
            return;
        }
        if !self.lyrics.open || !self.lyrics.active {
            return;
        }
        let query = self.playing.as_ref().map(Query::from_track);
        if query != self.lyrics.query {
            self.lyrics.clear_track();
            self.lyrics.query = query;
        }
        if self.lyrics.open
            && self.lyrics.active
            && self.lyrics.data.is_none()
            && self.lyrics.error.is_none()
            && self.lyrics.pending.is_none()
            && !self.lyrics.settings_busy
            && let Some(query) = self.lyrics.query.clone()
        {
            self.ensure_lyrics_service(ctx);
            if let Some(service) = &self.lyrics.service {
                self.lyrics.pending = Some(service.fetch(query));
            }
        }
    }

    pub(super) fn toggle_lyrics(&mut self, ctx: &egui::Context) {
        let next = !(self.lyrics.active && self.lyrics.open);
        if self.lyrics.fullscreen {
            self.set_lyrics_fullscreen(ctx, false);
        }
        self.queue_ui.open = false;
        self.lyrics.active = true;
        self.lyrics.open = next;
        if !next {
            self.lyrics.cancel_pending();
        }
        self.lyrics.view.reset();
        self.poll_lyrics(ctx);
        ctx.request_repaint();
    }

    pub(super) fn toggle_queue(&mut self, ctx: &egui::Context) {
        let next = !self.queue_ui.open;
        if self.lyrics.fullscreen {
            self.set_lyrics_fullscreen(ctx, false);
        }
        self.lyrics.open = false;
        self.lyrics.cancel_pending();
        self.lyrics.active = false;
        self.queue_ui.open = next;
        if next {
            self.synchronize_order();
        }
        ctx.request_repaint();
    }

    pub(super) fn set_lyrics_fullscreen(&mut self, ctx: &egui::Context, fullscreen: bool) {
        if self.lyrics.fullscreen == fullscreen {
            return;
        }
        if fullscreen {
            self.lyrics.previous_fullscreen =
                ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            self.queue_ui.open = false;
            self.lyrics.open = true;
            self.lyrics.active = true;
        }
        self.lyrics.fullscreen = fullscreen;
        self.lyrics.view.reset();
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(if fullscreen {
            true
        } else {
            self.lyrics.previous_fullscreen
        }));
        ctx.request_repaint();
    }

    pub(super) fn right_panel(&mut self, ui: &mut egui::Ui) {
        if !self.lyrics.active {
            self.queue_panel(ui);
            return;
        }
        let mut open = self.lyrics.open;
        let width = (ui.available_width() * 0.44).clamp(260.0, 340.0);
        let mut action = None;
        egui::Panel::right("playback-queue-panel")
            .exact_size(width)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .inner_margin(12)
                    .outer_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 6,
                        bottom: 6,
                    })
                    .corner_radius(14),
            )
            .show_collapsible(ui, &mut open, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                action = self.lyrics_contents(ui, false);
            });
        self.lyrics.open = open;
        if let Some(action) = action {
            self.handle_lyrics_action(ui.ctx(), action);
        }
    }

    fn lyrics_contents(
        &mut self,
        ui: &mut egui::Ui,
        fullscreen: bool,
    ) -> Option<lyrics_ui::Action> {
        self.lyrics.install_font(ui.ctx());
        let position = self.playback_snapshot().position;
        let retry_allowed = self
            .lyrics
            .status
            .is_none_or(|status| status.request_retry_after <= unix_now());
        lyrics_ui::show(
            ui,
            &mut self.lyrics.view,
            &mut self.covers,
            lyrics_ui::View {
                language: self.language,
                track: self.playing.as_ref(),
                lyrics: self.lyrics.data.as_deref(),
                error: self.lyrics.error,
                loading: self.lyrics.pending.is_some() || self.lyrics.settings_busy,
                position,
                fullscreen,
                retry_allowed,
            },
        )
    }

    pub(super) fn lyrics_fullscreen(&mut self, ui: &mut egui::Ui) {
        if let Some(action) = self.lyrics_contents(ui, true) {
            self.handle_lyrics_action(ui.ctx(), action);
        }
    }

    fn handle_lyrics_action(&mut self, ctx: &egui::Context, action: lyrics_ui::Action) {
        match action {
            lyrics_ui::Action::Close => {
                self.lyrics.open = false;
                self.lyrics.cancel_pending();
            }
            lyrics_ui::Action::Fullscreen => {
                self.set_lyrics_fullscreen(ctx, !self.lyrics.fullscreen)
            }
            lyrics_ui::Action::Retry => {
                self.lyrics.error = None;
                self.lyrics.pending = None;
                self.lyrics.settings_error = None;
                self.poll_lyrics(ctx);
            }
            lyrics_ui::Action::Settings => {
                self.set_lyrics_fullscreen(ctx, false);
                self.lyrics.open = false;
                self.queue_ui.open = false;
                self.settings_section = SettingsSection::Playback;
                self.settings_query = "Musixmatch".into();
                self.open_settings(ctx);
            }
            lyrics_ui::Action::Seek(at) => {
                self.cancel_delayed_advance();
                self.seek_position = None;
                self.finished_handled = false;
                self.commit_playback_seek(at);
            }
        }
        ctx.request_repaint();
    }

    fn configure_lyrics(&mut self, ctx: &egui::Context, action: ConfigAction) {
        self.ensure_lyrics_service(ctx);
        if let Some(service) = &self.lyrics.service {
            service.configure(action);
            self.lyrics.settings_busy = true;
            self.lyrics.settings_error = None;
            self.lyrics.pending = None;
            self.lyrics.error = None;
        }
    }

    pub(super) fn lyrics_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.ensure_lyrics_service(&ctx);
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        ui.label(
            RichText::new(t("Тексты · Musixmatch", "Lyrics · Musixmatch"))
                .family(theme::semibold()),
        );
        ui.add_space(4.0);
        let status = self.lyrics.status.unwrap_or_default();
        let mut mode = status.mode;
        ui.add_enabled_ui(
            !self.lyrics.settings_busy && self.lyrics.status.is_some(),
            |ui| {
                ui.radio_value(
                    &mut mode,
                    Mode::Anonymous,
                    t("Анонимный доступ", "Anonymous access"),
                );
                ui.radio_value(
                    &mut mode,
                    Mode::ApiKey,
                    t("Свой API-ключ", "Personal API key"),
                );
            },
        );
        if mode != status.mode {
            self.lyrics.status = Some(Status { mode, ..status });
            self.configure_lyrics(&ctx, ConfigAction::Mode(mode));
        }
        ui.add_space(10.0);
        match mode {
            Mode::Anonymous => {
                ui.label(
                    RichText::new(if status.token_rejected {
                        t(
                            "Токен отклонён. Создайте новый или используйте API-ключ.",
                            "The token was rejected. Generate another one or use an API key.",
                        )
                    } else if status.token_ready {
                        t("Анонимный токен готов.", "Anonymous token is ready.")
                    } else {
                        t(
                            "Токен создастся при первом открытии текста.",
                            "A token will be created when you first open lyrics.",
                        )
                    })
                    .small()
                    .color(MUTED),
                );
                if ui
                    .add_enabled(
                        !self.lyrics.settings_busy
                            && self.lyrics.pending.is_none()
                            && self.lyrics.status.is_some()
                            && status.token_retry_after <= unix_now(),
                        egui::Button::new(if status.token_ready {
                            t("Обновить токен", "Renew token")
                        } else {
                            t("Создать анонимный токен", "Generate anonymous token")
                        }),
                    )
                    .clicked()
                {
                    self.configure_lyrics(&ctx, ConfigAction::Generate);
                }
                if status.token_retry_after > unix_now() {
                    ui.label(
                        RichText::new(t(
                            "Повторная генерация будет доступна позже.",
                            "Token generation will be available again later.",
                        ))
                        .small()
                        .color(MUTED),
                    );
                }
            }
            Mode::ApiKey => {
                if status.key_ready {
                    ui.label(
                        RichText::new(t("API-ключ задан.", "API key is configured."))
                            .small()
                            .color(MUTED),
                    );
                }
                let field = ui.add_enabled(
                    !self.lyrics.settings_busy,
                    egui::TextEdit::singleline(&mut *self.lyrics.key_input)
                        .password(true)
                        .char_limit(4096)
                        .desired_width(ui.available_width())
                        .hint_text(t("API-ключ Musixmatch", "Musixmatch API key")),
                );
                if let Some(mut state) = egui::TextEdit::load_state(&ctx, field.id) {
                    state.clear_undoer();
                    egui::TextEdit::store_state(&ctx, field.id, state);
                }
                if ui
                    .add_enabled(
                        !self.lyrics.settings_busy && !self.lyrics.key_input.trim().is_empty(),
                        egui::Button::new(t("Сохранить ключ", "Save key")),
                    )
                    .clicked()
                {
                    self.configure_lyrics(
                        &ctx,
                        ConfigAction::ApiKey(Zeroizing::new(
                            self.lyrics.key_input.trim().to_owned(),
                        )),
                    );
                }
            }
        }
        if self.lyrics.settings_busy {
            ui.add(egui::Spinner::new().size(16.0));
        }
        if let Some(error) = self.lyrics.settings_error {
            ui.label(
                RichText::new(error.message(language)).color(Color32::from_rgb(255, 158, 153)),
            );
            if ui.button(t("Повторить", "Retry")).clicked() {
                self.lyrics.settings_error = None;
                self.configure_lyrics(&ctx, ConfigAction::Reload);
            }
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(t(
                "Синхронизация доступна для треков, у которых есть таймкоды Musixmatch.",
                "Timing is available for tracks with synchronized lyrics on Musixmatch.",
            ))
            .small()
            .color(MUTED),
        );
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
