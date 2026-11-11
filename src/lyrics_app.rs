//! Lyrics panel selection, background jobs and local provider settings.
use super::*;
use crate::{
    lyrics::{self, ConfigAction, Error, Event, Query, Service, Source, Status, VkSession},
    lyrics_ui,
};

#[cfg(test)]
#[path = "lyrics_app_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct State {
    pub(super) active: bool,
    pub(super) open: bool,
    pub(super) fullscreen: bool,
    previous_fullscreen: bool,
    previous_maximized: bool,
    service: Option<Service>,
    query: Option<Query>,
    pending: Option<u64>,
    data: Option<Arc<lyrics::Lyrics>>,
    error: Option<Error>,
    status: Option<Status>,
    settings_busy: bool,
    settings_error: Option<Error>,
    view: lyrics_ui::State,
    backdrop: crate::backdrop::Backdrop,
}

impl State {
    /// For the diagnostic log: what the lyrics panel holds, by kind only.
    pub(super) fn diagnostic(&self) -> String {
        if self.pending.is_some() {
            return "loading".into();
        }
        if let Some(error) = self.error {
            return format!("error {error:?}");
        }
        match &self.data {
            None => "none".into(),
            Some(data) if data.instrumental => "instrumental".into(),
            Some(data) if data.synced => format!("synced, {} lines", data.lines.len()),
            Some(data) => format!("plain, {} lines", data.lines.len()),
        }
    }

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
        self.view.reset();
    }
}

impl MusicApp {
    pub fn right_panel_state(&self) -> crate::window_state::RightPanel {
        crate::window_state::RightPanel {
            lyrics_open: self.lyrics.active && self.lyrics.open,
            queue_open: self.queue_ui.open,
            split: if self.queue_ui.split > 0.0 && self.queue_ui.split < 1.0 {
                self.queue_ui.split
            } else {
                0.5
            },
        }
    }

    pub fn restore_right_panel_state(&mut self, state: crate::window_state::RightPanel) {
        self.lyrics.active = state.lyrics_open;
        self.lyrics.open = state.lyrics_open;
        self.queue_ui.open = state.queue_open;
        self.queue_ui.split = if state.split > 0.0 && state.split < 1.0 {
            state.split
        } else {
            0.5
        };
    }

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
            let same_track = query.as_ref().map(|query| query.key)
                == self.lyrics.query.as_ref().map(|query| query.key);
            if same_track && self.lyrics.data.is_some() {
                // Richer metadata for the same recording (opening fullscreen
                // asks for it) keeps the lyrics already found.
                self.lyrics.query = query;
            } else if same_track && self.lyrics.pending.is_some() {
                // The search under way answers for this recording too; its
                // reply is matched against the query it was asked with.
            } else {
                self.lyrics.clear_track();
                self.lyrics.query = query;
            }
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
                let vk = self.session.as_ref().and_then(|session| {
                    session.music.clone().map(|client| VkSession {
                        owner: session.profile.id,
                        client,
                    })
                });
                self.lyrics.pending = Some(service.fetch(query, vk));
            }
        }
    }

    pub(super) fn toggle_lyrics(&mut self, ctx: &egui::Context) {
        let next = !(self.lyrics.active && self.lyrics.open);
        if self.lyrics.fullscreen {
            self.set_lyrics_fullscreen(ctx, false);
        }
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
            self.lyrics.previous_maximized =
                ctx.input(|input| input.viewport().maximized.unwrap_or(false));
            self.lyrics.open = true;
            self.lyrics.active = true;
        }
        self.lyrics.fullscreen = fullscreen;
        if fullscreen && let Some(key) = self.playing.as_ref().map(Track::key) {
            self.metadata_attempted.remove(&key);
            self.hydrate_metadata(ctx, vec![key]);
        }
        self.lyrics.view.reset();
        if cfg!(windows) && !self.lyrics.previous_fullscreen {
            // Windows' borderless fullscreen breaks the custom title bar:
            // the taskbar vanishes and the classic frame shows through.
            // The lyrics fill a maximized window there instead.
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(
                fullscreen || self.lyrics.previous_maximized,
            ));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(if fullscreen {
                true
            } else {
                self.lyrics.previous_fullscreen
            }));
        }
        ctx.request_repaint();
    }

    pub(super) fn right_panel(&mut self, ui: &mut egui::Ui) {
        let lyrics = self.lyrics.active && self.lyrics.open;
        if lyrics && self.queue_ui.open {
            self.queue_and_lyrics_panel(ui);
            return;
        }
        if self.queue_ui.open || !self.lyrics.active {
            self.queue_panel(ui);
            return;
        }
        // The edge only resizes: a drag or double click must not close it.
        let mut open = self.lyrics.open;
        let mut action = None;
        let shown = self
            .right_side_panel(ui)
            .show_collapsible(ui, &mut open, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                action = self.lyrics_contents(ui, false, true);
            });
        if let Some(shown) = shown {
            self.remember_panel_width(
                ui.ctx(),
                1,
                super::RIGHT_PANEL_ID,
                shown.response.rect.width(),
            );
        }
        if let Some(action) = action {
            self.handle_lyrics_action(ui.ctx(), action);
        }
    }

    /// Queue on top, lyrics below, split by a draggable divider.
    fn queue_and_lyrics_panel(&mut self, ui: &mut egui::Ui) {
        const MIN_PART: f32 = 140.0;
        let mut open = true;
        let mut close_queue = false;
        let mut action = None;
        let shown = self
            .right_side_panel(ui)
            .show_collapsible(ui, &mut open, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                let full = ui.available_rect_before_wrap();
                let split = if self.queue_ui.split > 0.0 {
                    self.queue_ui.split
                } else {
                    0.5
                };
                let low = (MIN_PART / full.height().max(1.0)).min(0.5);
                let divider = full.top() + full.height() * split.clamp(low, 1.0 - low);
                let top =
                    egui::Rect::from_min_max(full.min, egui::pos2(full.right(), divider - 8.0));
                let bottom =
                    egui::Rect::from_min_max(egui::pos2(full.left(), divider + 8.0), full.max);
                let clip = ui.clip_rect();
                close_queue = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("queue-above-lyrics")
                            .max_rect(top),
                        |ui| {
                            ui.set_clip_rect(clip.intersect(top));
                            self.queue_contents(ui)
                        },
                    )
                    .inner;
                let handle = egui::Rect::from_min_max(
                    egui::pos2(full.left(), divider - 6.0),
                    egui::pos2(full.right(), divider + 6.0),
                );
                let response =
                    ui.interact(handle, ui.id().with("queue-lyrics-divider"), Sense::drag());
                if response.hovered() || response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                if response.dragged()
                    && let Some(pointer) = response.interact_pointer_pos()
                {
                    self.queue_ui.split =
                        ((pointer.y - full.top()) / full.height().max(1.0)).clamp(low, 1.0 - low);
                }
                ui.painter().hline(
                    full.x_range(),
                    divider,
                    egui::Stroke::new(
                        1.0,
                        if response.hovered() || response.dragged() {
                            theme::ACCENT
                        } else {
                            ui.visuals().widgets.noninteractive.bg_stroke.color
                        },
                    ),
                );
                action = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("lyrics-below-queue")
                            .max_rect(bottom),
                        |ui| {
                            ui.set_clip_rect(clip.intersect(bottom));
                            self.lyrics_contents(ui, false, false)
                        },
                    )
                    .inner;
                ui.allocate_rect(full, Sense::hover());
            });
        self.queue_ui.open &= !close_queue;
        if let Some(shown) = shown {
            self.remember_panel_width(
                ui.ctx(),
                1,
                super::RIGHT_PANEL_ID,
                shown.response.rect.width(),
            );
        }
        if let Some(action) = action {
            self.handle_lyrics_action(ui.ctx(), action);
        }
    }

    fn lyrics_contents(
        &mut self,
        ui: &mut egui::Ui,
        fullscreen: bool,
        show_track: bool,
    ) -> Option<lyrics_ui::Action> {
        let snapshot = self.playback_snapshot();
        let retry_allowed = self
            .lyrics
            .status
            .is_none_or(|status| status.request_retry_after <= unix_now());
        lyrics_ui::show(
            ui,
            &mut self.lyrics.view,
            &mut self.covers,
            self.animated_backdrop.then_some(&mut self.lyrics.backdrop),
            lyrics_ui::View {
                language: self.language,
                track: self.playing.as_ref(),
                lyrics: self.lyrics.data.as_deref(),
                error: self.lyrics.error,
                loading: self.lyrics.pending.is_some() || self.lyrics.settings_busy,
                position: snapshot.position,
                playing: matches!(snapshot.state, PlaybackState::Playing),
                fullscreen,
                retry_allowed,
                show_track,
            },
        )
    }

    pub(super) fn lyrics_fullscreen(&mut self, ui: &mut egui::Ui) {
        if let Some(action) = self.lyrics_contents(ui, true, true) {
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
            lyrics_ui::Action::Reload => {
                self.lyrics.cancel_pending();
                self.lyrics.error = None;
                self.lyrics.settings_error = None;
                self.ensure_lyrics_service(ctx);
                if let (Some(service), Some(query)) =
                    (&self.lyrics.service, self.lyrics.query.clone())
                {
                    let vk = self.session.as_ref().and_then(|session| {
                        session.music.clone().map(|client| VkSession {
                            owner: session.profile.id,
                            client,
                        })
                    });
                    self.lyrics.pending = Some(service.reload(query, vk));
                }
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
                self.settings_query = "lyrics".into();
                self.open_settings(ctx);
            }
            lyrics_ui::Action::Seek(at) => {
                self.cancel_delayed_advance();
                self.seek_position = None;
                self.finished_handled = false;
                self.commit_playback_seek(at);
            }
            lyrics_ui::Action::Album => {
                if let Some(track) = self.playing.clone() {
                    if self.lyrics.fullscreen {
                        self.set_lyrics_fullscreen(ctx, false);
                    }
                    let action = track.album.as_ref().map_or_else(
                        || TrackAction::TrackAlbum(track.key(), track.display_title().into_owned()),
                        |album| TrackAction::Album(album.clone()),
                    );
                    self.handle_track_action(ctx, action);
                }
            }
            lyrics_ui::Action::Artist(artist) => {
                if self.lyrics.fullscreen {
                    self.set_lyrics_fullscreen(ctx, false);
                }
                self.navigate_music(ctx, MusicPage::Artist(artist));
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
        ui.label(RichText::new(t("Тексты", "Lyrics")).family(theme::semibold()));
        ui.add_space(4.0);
        let status = self.lyrics.status.unwrap_or_default();
        let mut source = status.source;
        ui.add_enabled_ui(
            !self.lyrics.settings_busy && self.lyrics.status.is_some(),
            |ui| {
                ui.label(t("Источник текстов", "Lyrics source"));
                ui.label(
                    RichText::new(t(
                        "VK и Musixmatch поддерживают таймкоды, если они доступны. Genius — обычный текст.",
                        "VK and Musixmatch support timing when available. Genius provides plain lyrics.",
                    ))
                    .small()
                    .color(MUTED),
                );
                egui::ComboBox::from_id_salt("lyrics-source")
                    .selected_text(source.name())
                    .show_ui(ui, |ui| {
                        for item in [Source::Vk, Source::Musixmatch, Source::Genius] {
                            ui.selectable_value(&mut source, item, item.name());
                        }
                    });
            },
        );
        if source != status.source {
            self.lyrics.status = Some(Status { source, ..status });
            self.configure_lyrics(&ctx, ConfigAction::Source(source));
        }
        if let Some(other) = source.other() {
            let mut fallback_other = status.fallback_to_other;
            let label = match other {
                Source::Musixmatch => t(
                    "Искать в Musixmatch, если текст недоступен",
                    "Fallback to Musixmatch",
                ),
                _ => t("Искать в VK, если текст недоступен", "Fallback to VK"),
            };
            if ui
                .add_enabled_ui(
                    !self.lyrics.settings_busy && self.lyrics.status.is_some(),
                    |ui| theme::switch(ui, &mut fallback_other, label),
                )
                .inner
                .changed()
            {
                self.configure_lyrics(&ctx, ConfigAction::FallbackOther(fallback_other));
            }
        }
        let mut fallback = status.fallback_to_genius;
        if ui
            .add_enabled_ui(
                !self.lyrics.settings_busy
                    && self.lyrics.status.is_some()
                    && source != Source::Genius,
                |ui| {
                    theme::switch(
                        ui,
                        &mut fallback,
                        t(
                            "Искать в Genius, если текст недоступен",
                            "Fallback to Genius",
                        ),
                    )
                },
            )
            .inner
            .changed()
        {
            self.configure_lyrics(&ctx, ConfigAction::Fallback(fallback));
        }
        ui.add_space(8.0);
        if source == Source::Musixmatch {
            ui.label(
                RichText::new(if status.token_rejected {
                    t(
                        "Токен Musixmatch отклонён. Обновите его.",
                        "The Musixmatch token was rejected. Renew it.",
                    )
                } else if status.token_ready {
                    t("Токен Musixmatch готов.", "The Musixmatch token is ready.")
                } else {
                    t(
                        "Токен Musixmatch создастся при первом открытии текста.",
                        "A Musixmatch token will be created when you first open lyrics.",
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
                    egui::Button::new(t("Обновить токен Musixmatch", "Renew Musixmatch token")),
                )
                .clicked()
            {
                self.configure_lyrics(&ctx, ConfigAction::Generate);
            }
            if status.token_retry_after > unix_now() {
                ui.label(
                    RichText::new(t(
                        "Повторное обновление будет доступно позже.",
                        "The token can be renewed again later.",
                    ))
                    .small()
                    .color(MUTED),
                );
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
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
