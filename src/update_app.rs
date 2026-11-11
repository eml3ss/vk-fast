//! Release checks and downloads wake the current window, including from tray.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    i18n::Language,
    icons::{self, Icon},
    repaint_target::RepaintTarget,
    theme,
    updater::{self, Error, Release, StagedUpdate},
};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};

#[derive(Default, Debug, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Current,
    Available,
    Downloading {
        received: u64,
        total: u64,
    },
    Ready,
    Starting,
    Installing,
    Failed(Error),
}

enum Event {
    Checked(Result<Option<Release>, Error>),
    Progress(u64, u64),
    Downloaded(Result<StagedUpdate, Error>),
    Handoff(Result<(), Error>),
}

#[derive(Default)]
pub(super) struct State {
    phase: Phase,
    release: Option<Release>,
    prepared: Option<StagedUpdate>,
    receiver: Option<Receiver<Event>>,
    canceled: Option<Arc<AtomicBool>>,
    next_check: Option<Instant>,
    automatic: bool,
    save_failed: bool,
    pub open: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel_download();
    }
}

impl State {
    pub fn new(automatic: bool) -> Self {
        Self {
            phase: Phase::Idle,
            release: None,
            prepared: None,
            receiver: None,
            canceled: None,
            automatic,
            // The first check runs as soon as the app opens.
            next_check: automatic.then(Instant::now),
            save_failed: false,
            open: false,
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking | Phase::Downloading { .. } | Phase::Starting | Phase::Installing
        )
    }

    pub fn has_notification(&self) -> bool {
        self.release.is_some()
    }

    pub fn notification_width(&self) -> f32 {
        if self.has_notification() { 112.0 } else { 0.0 }
    }

    pub fn notification_button(&mut self, ui: &mut egui::Ui, language: Language) {
        let Some(release) = &self.release else {
            return;
        };
        let version = release.version.to_string();
        let (rect, response) = ui.allocate_exact_size(egui::vec2(104.0, 38.0), Sense::click());
        ui.painter().rect_filled(
            rect,
            8,
            if response.hovered() {
                theme::accent()
            } else {
                theme::primary()
            },
        );
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 20.0, rect.center().y),
                egui::vec2(18.0, 18.0),
            ),
            Icon::Refresh,
            Color32::WHITE,
        );
        let text_rect = egui::Rect::from_min_max(
            rect.min + egui::vec2(34.0, 0.0),
            rect.max - egui::vec2(8.0, 0.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
            |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&version).color(Color32::WHITE).strong())
                        .truncate()
                        .selectable(false),
                );
            },
        );
        let label = format!(
            "{} {version}",
            language.text("Доступна версия", "Version available:")
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
            .clicked()
        {
            self.open = true;
        }
    }

    pub fn tick(&mut self, target: &RepaintTarget) -> bool {
        let mut quit = false;
        while let Some(receiver) = self.receiver.as_ref() {
            let event = match receiver.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.phase = Phase::Failed(Error::Helper);
                    break;
                }
            };
            match event {
                Event::Progress(received, total) => {
                    self.phase = Phase::Downloading { received, total };
                }
                Event::Checked(result) => {
                    self.receiver = None;
                    self.next_check = self.automatic.then(|| {
                        Instant::now()
                            + if result.is_ok() {
                                Duration::from_secs(6 * 3600)
                            } else {
                                Duration::from_secs(1800)
                            }
                    });
                    match result {
                        Ok(release) => {
                            self.phase = if release.is_some() {
                                Phase::Available
                            } else {
                                Phase::Current
                            };
                            self.release = release;
                        }
                        Err(error) => self.phase = Phase::Failed(error),
                    }
                }
                Event::Downloaded(result) => {
                    self.receiver = None;
                    self.canceled = None;
                    match result {
                        Ok(prepared) => {
                            self.prepared = Some(prepared);
                            self.phase = Phase::Ready;
                        }
                        Err(error) => self.phase = Phase::Failed(error),
                    }
                }
                Event::Handoff(result) => {
                    self.receiver = None;
                    match result {
                        Ok(()) => {
                            self.phase = Phase::Installing;
                            quit = true;
                        }
                        Err(error) => self.phase = Phase::Failed(error),
                    }
                }
            }
        }
        if let Some(deadline) = self.next_check
            && Instant::now() >= deadline
            && !self.busy()
            && self.prepared.is_none()
        {
            self.check(target);
        }
        quit
    }

    pub fn next_check_in(&self) -> Option<Duration> {
        self.next_check
            .filter(|_| !self.busy() && self.prepared.is_none())
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn check(&mut self, target: &RepaintTarget) {
        if self.busy() || self.prepared.is_some() {
            return;
        }
        self.phase = Phase::Checking;
        self.next_check = None;
        if cfg!(test) {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let target = target.clone();
        if thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let _ = sender.send(Event::Checked(updater::check()));
                target.request_repaint();
            })
            .is_ok()
        {
            self.receiver = Some(receiver);
        } else {
            self.phase = Phase::Failed(Error::Helper);
        }
    }

    fn download(&mut self, target: &RepaintTarget) {
        if self.busy() {
            return;
        }
        let Some(release) = self.release.clone() else {
            return;
        };
        self.phase = Phase::Downloading {
            received: 0,
            total: 1,
        };
        if cfg!(test) {
            return;
        }
        let canceled = Arc::new(AtomicBool::new(false));
        self.canceled = Some(canceled.clone());
        let (sender, receiver) = mpsc::sync_channel(8);
        let target = target.clone();
        if thread::Builder::new()
            .name("update-download".into())
            .spawn(move || {
                let mut last_percent = 0;
                let result = updater::download(&release, &canceled, |received, total| {
                    let percent = received.saturating_mul(100) / total.max(1);
                    if percent != last_percent {
                        last_percent = percent;
                        let _ = sender.try_send(Event::Progress(received, total));
                        target.request_repaint();
                    }
                });
                let _ = sender.send(Event::Downloaded(result));
                target.request_repaint();
            })
            .is_ok()
        {
            self.receiver = Some(receiver);
        } else {
            self.phase = Phase::Failed(Error::Helper);
        }
    }

    fn install(&mut self, target: &RepaintTarget, language: Language) {
        if self.busy() {
            return;
        }
        let Some(prepared) = self.prepared.take() else {
            return;
        };
        self.phase = Phase::Starting;
        if cfg!(test) {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let target = target.clone();
        if thread::Builder::new()
            .name("update-handoff".into())
            .spawn(move || {
                let result = updater::install::launch(prepared, language == Language::English);
                let _ = sender.send(Event::Handoff(result));
                target.request_repaint();
            })
            .is_ok()
        {
            self.receiver = Some(receiver);
        } else {
            self.phase = Phase::Failed(Error::Helper);
        }
    }

    fn cancel_download(&mut self) {
        if let Some(canceled) = self.canceled.take() {
            canceled.store(true, Ordering::Release);
        }
        if matches!(self.phase, Phase::Downloading { .. }) {
            self.receiver = None;
            self.phase = Phase::Available;
        }
    }

    pub fn settings(&mut self, ui: &mut egui::Ui, language: Language, target: &RepaintTarget) {
        ui.add_space(16.0);
        ui.label(RichText::new(language.text("Обновления", "Updates")).strong());
        if theme::switch(
            ui,
            &mut self.automatic,
            language.text(
                "Автоматически проверять обновления",
                "Automatically check for updates",
            ),
        )
        .changed()
        {
            self.save_failed = if cfg!(test) {
                false
            } else {
                crate::preferences::save_auto_updates(self.automatic).is_err()
            };
            self.next_check = self
                .automatic
                .then(|| Instant::now() + Duration::from_secs(10));
        }
        if self.save_failed {
            ui.colored_label(
                theme::error(),
                language.text(
                    "Не удалось сохранить настройку.",
                    "Could not save the setting.",
                ),
            );
        }
        self.status(ui, language);
        if self.release.is_some() || self.prepared.is_some() {
            if ui
                .button(language.text("Открыть обновление", "Open update"))
                .clicked()
            {
                self.open = true;
            }
        } else if ui
            .add_enabled(
                !self.busy(),
                egui::Button::new(language.text("Проверить обновления", "Check for updates")),
            )
            .clicked()
        {
            self.check(target);
        }
    }

    fn status(&self, ui: &mut egui::Ui, language: Language) {
        match self.phase {
            Phase::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(language.text("Проверяем обновления…", "Checking for updates…"));
                });
            }
            Phase::Current => {
                ui.label(
                    RichText::new(
                        language.text("Установлена последняя версия.", "You are up to date."),
                    )
                    .color(theme::muted()),
                );
            }
            Phase::Available | Phase::Ready => {
                if let Some(release) = &self.release {
                    ui.label(format!(
                        "{} {}",
                        language.text("Доступна версия", "Version available:"),
                        release.version
                    ));
                }
            }
            Phase::Downloading { received, total } => {
                let fraction = (received as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                ui.label(language.text("Загружаем обновление…", "Downloading update…"));
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .desired_width(ui.available_width()),
                );
            }
            Phase::Starting | Phase::Installing => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(language.text("Готовим перезапуск…", "Preparing to restart…"));
                });
            }
            Phase::Failed(error) => {
                ui.colored_label(theme::error(), error.message(language));
            }
            Phase::Idle => {}
        }
    }

    pub fn dialog(&mut self, ctx: &egui::Context, language: Language, target: &RepaintTarget) {
        if !self.open {
            return;
        }
        let width = (ctx.content_rect().width() - 64.0).clamp(240.0, 440.0);
        let modal = egui::Modal::new(egui::Id::new("application-update-dialog"))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .fill(theme::surface())
                    .inner_margin(20)
                    .corner_radius(14),
            )
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.style_mut().interaction.selectable_labels = false;
                ui.horizontal(|ui| {
                    ui.heading(language.text("Обновление VK Fast", "VK Fast update"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::Vec2::splat(24.0), Sense::hover());
                        let closable = !matches!(self.phase, Phase::Starting | Phase::Installing);
                        if ui
                            .add_enabled_ui(closable, |ui| {
                                icons::panel_button(
                                    ui,
                                    rect,
                                    Icon::Close,
                                    language.text("Закрыть", "Close"),
                                )
                            })
                            .inner
                            .clicked()
                        {
                            self.open = false;
                        }
                    });
                });
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        language.text("Текущая версия:", "Current version:"),
                        env!("CARGO_PKG_VERSION")
                    ))
                    .small()
                    .color(theme::muted()),
                );
                ui.add_space(12.0);
                self.status(ui, language);
                if let Some(release) = &self.release {
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 330.0).clamp(60.0, 200.0))
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(&release.notes).wrap().selectable(false));
                        });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(language.text(
                            "Вход, настройки и кэш сохранятся.",
                            "Your sign-in, settings and cache will be retained.",
                        ))
                        .small()
                        .color(theme::muted()),
                    );
                }
                ui.add_space(14.0);
                let label = match self.phase {
                    Phase::Ready => {
                        language.text("Установить и перезапустить", "Install and restart")
                    }
                    _ if self.release.is_some() => {
                        language.text("Загрузить обновление", "Download update")
                    }
                    _ => language.text("Проверить обновления", "Check for updates"),
                };
                if ui
                    .add_enabled_ui(!self.busy(), |ui| {
                        ui.add_sized(
                            egui::vec2(width, 38.0),
                            egui::Button::new(label)
                                .fill(theme::primary())
                                .corner_radius(8),
                        )
                    })
                    .inner
                    .clicked()
                {
                    if self.prepared.is_some() {
                        self.install(target, language);
                    } else if self.release.is_some() {
                        self.download(target);
                    } else {
                        self.check(target);
                    }
                }
                ui.add_space(4.0);
                if matches!(self.phase, Phase::Downloading { .. }) {
                    if ui
                        .add_sized(
                            egui::vec2(width, 34.0),
                            egui::Button::new(
                                language.text("Отменить загрузку", "Cancel download"),
                            )
                            .corner_radius(8),
                        )
                        .clicked()
                    {
                        self.cancel_download();
                    }
                } else if ui
                    .add_enabled_ui(
                        !matches!(self.phase, Phase::Starting | Phase::Installing),
                        |ui| {
                            ui.add_sized(
                                egui::vec2(width, 34.0),
                                egui::Button::new(language.text("Позже", "Later")).corner_radius(8),
                            )
                        },
                    )
                    .inner
                    .clicked()
                {
                    self.open = false;
                }
            });
        if modal.should_close() && !matches!(self.phase, Phase::Starting | Phase::Installing) {
            self.open = false;
        }
    }
}

#[cfg(test)]
#[path = "update_app/tests.rs"]
mod tests;
