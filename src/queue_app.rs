//! The right-hand queue drawer and explicit, local queue actions.
use super::*;
use crate::play_order::{TrackKey, Upcoming};

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct PanelState {
    pub(super) open: bool,
    stamp: Option<(u64, Option<TrackKey>, bool)>,
    upcoming: Vec<Upcoming>,
    source_stamp: Option<(usize, usize, u64)>,
    source_indices: HashMap<TrackKey, usize>,
}

enum Action {
    Play(usize),
    ToggleCurrent,
    Remove(u64),
    Clear,
    Track(Box<TrackAction>),
}

impl MusicApp {
    pub(super) fn add_to_queue(&mut self, ctx: &egui::Context, track: Track) {
        if self.session.is_none() || self.demo {
            return;
        }
        if track.unavailable == Some(true) {
            self.track_unavailable_notice(ctx);
            return;
        }
        self.synchronize_order();
        let added = self.play_order.enqueue(track.key());
        if added {
            self.queued_tracks.insert(track.key(), track);
            self.prune_queued_tracks();
        }
        self.track_menu_notice = Some((
            self.language
                .text(
                    if added {
                        "Трек добавлен в очередь"
                    } else {
                        "Очередь заполнена"
                    },
                    if added {
                        "Added to queue"
                    } else {
                        "Queue is full"
                    },
                )
                .into(),
            Instant::now() + Duration::from_secs(3),
        ));
        ctx.request_repaint();
    }

    fn prune_queued_tracks(&mut self) {
        let keep: HashSet<_> = self.play_order.queued_keys_to_keep().collect();
        self.queued_tracks.retain(|key, _| keep.contains(key));
    }

    fn queue_track(&self, key: TrackKey, index: Option<usize>) -> Option<&Track> {
        index
            .and_then(|index| {
                self.play_queue
                    .as_deref()
                    .unwrap_or(&self.library.tracks)
                    .get(index)
            })
            .filter(|track| track.key() == key)
            .or_else(|| self.queued_tracks.get(&key))
            .or_else(|| self.playing.as_ref().filter(|track| track.key() == key))
    }

    fn play_queue_position(&mut self, ctx: &egui::Context, position: usize) {
        self.unavailable_playback.clear();
        self.cancel_delayed_advance();
        let current = self.playing.as_ref().map(Track::key).or(self.selected);
        if let Some(key) = self
            .play_order
            .jump_to(position, current, self.allow_queue_wrap())
        {
            self.play_track(ctx, key);
        }
    }

    pub(super) fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let mut panel = std::mem::take(&mut self.queue_ui);
        let source = self.play_queue.as_deref().unwrap_or(&self.library.tracks);
        let source_stamp = (
            source.as_ptr() as usize,
            source.len(),
            self.track_view_cache.revision(),
        );
        if panel.open && panel.source_stamp != Some(source_stamp) {
            panel.source_indices = source
                .iter()
                .enumerate()
                .map(|(index, track)| (track.key(), index))
                .collect();
            panel.source_stamp = Some(source_stamp);
        }
        let current = self.playing.as_ref().map(Track::key).or(self.selected);
        let stamp = (self.play_order.revision(), current, self.allow_queue_wrap());
        if panel.open && panel.stamp != Some(stamp) {
            panel.upcoming = if current.is_none() && self.play_order.queued().is_empty() {
                Vec::new()
            } else {
                self.play_order.upcoming(current, stamp.2)
            };
            panel.stamp = Some(stamp);
        }
        let mut open = panel.open;
        let mut close = false;
        let mut action = None;
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let shown = self
            .right_side_panel(ui)
            .show_collapsible(ui, &mut open, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("Очередь", "Queue"))
                                .size(20.0)
                                .family(theme::semibold()),
                        )
                        .selectable(false),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::Vec2::splat(28.0), Sense::hover());
                        let response = icons::panel_button(
                            ui,
                            rect,
                            Icon::Close,
                            t("Закрыть очередь", "Close queue"),
                        );
                        release_pointer_focus(&response);
                        close = response.clicked();
                    });
                });
                ui.add_space(12.0);
                if let Some(track) = self.playing.clone() {
                    ui.label(
                        RichText::new(t("Сейчас играет", "Now playing"))
                            .small()
                            .color(MUTED),
                    );
                    ui.add_space(4.0);
                    if let Some(row_action) = self.queue_row(ui, &track, None, true) {
                        action = Some(match row_action {
                            Action::Play(_) => Action::ToggleCurrent,
                            other => other,
                        });
                    }
                    ui.add_space(16.0);
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Далее", "Up next")).family(theme::semibold()));
                    if !self.play_order.queued().is_empty() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(t("Очистить добавленные", "Clear added"))
                                            .small()
                                            .color(MUTED),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text(t(
                                    "Убрать добавленные вами треки",
                                    "Clear the tracks you added",
                                ))
                                .clicked()
                            {
                                action = Some(Action::Clear);
                            }
                        });
                    }
                });
                ui.add_space(8.0);
                if panel.upcoming.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("Очередь пуста", "The queue is empty")).color(MUTED),
                        )
                        .wrap()
                        .selectable(false),
                    );
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(t(
                                "Добавляйте треки через контекстное меню.",
                                "Add tracks from their context menu.",
                            ))
                            .small()
                            .color(MUTED),
                        )
                        .wrap()
                        .selectable(false),
                    );
                } else {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    theme::scroll_rows(
                        ui,
                        egui::ScrollArea::vertical()
                            .id_salt("playback-queue-list")
                            .auto_shrink([false, false]),
                        50.0,
                        panel.upcoming.len(),
                        |ui, range| {
                            for index in range {
                                let entry = panel.upcoming[index];
                                let Some(track) = self
                                    .queue_track(
                                        entry.key,
                                        panel.source_indices.get(&entry.key).copied(),
                                    )
                                    .cloned()
                                else {
                                    ui.allocate_space(egui::vec2(ui.available_width(), 50.0));
                                    continue;
                                };
                                ui.push_id((stamp.0, index, entry.queued_id, entry.key), |ui| {
                                    if let Some(row_action) =
                                        self.queue_row(ui, &track, entry.queued_id, false)
                                    {
                                        action = Some(match row_action {
                                            Action::Play(_) => Action::Play(index),
                                            other => other,
                                        });
                                    }
                                });
                            }
                        },
                    );
                }
            });
        // The edge only resizes: a drag or double click must not close it.
        panel.open &= !close;
        self.queue_ui = panel;
        if let Some(shown) = shown {
            self.remember_panel_width(
                ui.ctx(),
                1,
                super::RIGHT_PANEL_ID,
                shown.response.rect.width(),
            );
        }
        let ctx = ui.ctx();
        match action {
            Some(Action::Play(index))
                if self.queue_ui.stamp == Some(stamp) && self.play_order.revision() == stamp.0 =>
            {
                self.play_queue_position(ctx, index)
            }
            Some(Action::ToggleCurrent) => self.toggle_playback(ctx),
            Some(Action::Remove(id)) => {
                self.play_order.remove_queued(id);
                self.prune_queued_tracks();
            }
            Some(Action::Clear) => {
                self.play_order.clear_queued();
                self.prune_queued_tracks();
            }
            Some(Action::Track(action)) => self.handle_track_action(ctx, *action),
            _ => {}
        }
    }

    fn queue_row(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        queued_id: Option<u64>,
        current: bool,
    ) -> Option<Action> {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), Sense::click());
        let hovered = response.contains_pointer();
        if hovered {
            ui.painter().rect_filled(rect, 8.0, theme::SELECTED);
        }
        let cover = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(22.0, 0.0),
            egui::vec2(36.0, 36.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(cover), |ui| {
            self.covers.show(ui, track.cover_url.as_deref(), 36.0);
        });
        let play = ui.interact(cover, ui.id().with("queue-row-play"), Sense::click());
        if hovered || play.hovered() {
            ui.painter()
                .rect_filled(cover, 6.0, Color32::from_black_alpha(165));
            icons::paint(
                ui.painter(),
                cover.shrink(9.0),
                if current
                    && matches!(
                        self.playback_snapshot().state,
                        PlaybackState::Playing | PlaybackState::Loading
                    )
                {
                    Icon::Pause
                } else {
                    Icon::Play
                },
                Color32::WHITE,
            );
        }
        let mut action = None;
        let remove = egui::Rect::from_center_size(
            rect.right_center() - egui::vec2(15.0, 0.0),
            egui::vec2(26.0, 28.0),
        );
        let mut remove_hovered = false;
        if let Some(id) = queued_id {
            let response = ui.interact(remove, ui.id().with("queue-row-remove"), Sense::click());
            remove_hovered = response.contains_pointer();
            if hovered || response.hovered() || response.has_focus() {
                icons::paint(ui.painter(), remove.shrink(7.0), Icon::Close, MUTED);
            }
            if response
                .on_hover_text(self.language.text("Убрать из очереди", "Remove from queue"))
                .clicked()
            {
                action = Some(Action::Remove(id));
            }
        }
        let right = if !current {
            remove.left() - 4.0
        } else {
            rect.right() - 5.0
        };
        let duration = egui::Rect::from_min_max(
            egui::pos2(right - 35.0, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        ui.put(
            duration,
            egui::Label::new(RichText::new(track.duration_label()).small().color(MUTED))
                .truncate()
                .selectable(false),
        );
        let metadata = egui::Rect::from_min_max(
            egui::pos2(cover.right() + 8.0, rect.top() + 7.0),
            egui::pos2(duration.left() - 4.0, rect.bottom() - 7.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(metadata)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(metadata));
                ui.set_width(metadata.width().max(0.0));
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add(
                    egui::Label::new(theme::track_title(
                        track,
                        if track.unavailable == Some(true) {
                            MUTED
                        } else if current {
                            ACCENT
                        } else {
                            ui.visuals().text_color()
                        },
                        egui::FontId::new(13.0, theme::semibold()),
                    ))
                    .truncate()
                    .selectable(false),
                );
                if let Some(artist) = detail_ui::artist_links_with_font(
                    ui,
                    &track.artists,
                    &track.artist,
                    egui::FontId::proportional(12.0),
                ) {
                    action = Some(Action::Track(Box::new(TrackAction::Artist(artist))));
                }
            },
        );
        if let Some(menu) = self.track_popup(ui, &response, rect, track) {
            action = Some(Action::Track(Box::new(menu)));
        }
        if action.is_none()
            && !remove_hovered
            && (play.clicked()
                || if self.double_click_to_play {
                    response.double_clicked()
                } else {
                    response.clicked()
                })
        {
            action = Some(Action::Play(0));
        }
        release_pointer_focus(&play);
        release_pointer_focus(&response);
        action
    }
}
