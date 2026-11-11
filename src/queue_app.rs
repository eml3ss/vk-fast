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
    /// The queue's share of the height when lyrics are open below it.
    pub(super) split: f32,
    /// A track carried in from a list: already queued, dragged on here.
    adopted: Option<Adopted>,
}

struct Adopted {
    /// Its entry among the added tracks.
    id: u64,
    track: Track,
    /// The queue's own drag has taken it over.
    started: bool,
}

/// What becomes of a carried-in track this frame.
enum Adoption {
    /// Escape, or let go away from the queue: it leaves the queue again.
    Remove(u64),
    /// Carried back out: it leaves the queue and is a card again.
    Revert(u64, Box<Track>, egui::Rect),
}

enum Action {
    Play(usize),
    ToggleCurrent,
    Remove(u64),
    /// An added track dragged to another place among the added tracks.
    Move {
        from: usize,
        to: usize,
    },
    /// A source track dragged next to another source track.
    MoveSource {
        key: TrackKey,
        neighbour: row_drag::Neighbour<TrackKey>,
    },
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

    /// Queues a track carried in from a list, without a notice: it shows up
    /// right under the pointer. Returns its entry.
    fn enqueue_carried(&mut self, track: &Track) -> Option<u64> {
        if self.session.is_none() || self.demo || track.unavailable == Some(true) {
            return None;
        }
        self.synchronize_order();
        if !self.play_order.enqueue(track.key()) {
            return None;
        }
        self.queued_tracks.insert(track.key(), track.clone());
        self.prune_queued_tracks();
        self.play_order.queued().back().map(|entry| entry.id)
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
        let mut open = self.queue_ui.open;
        let mut close = false;
        let shown = self
            .right_side_panel(ui)
            .show_collapsible(ui, &mut open, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                close = self.queue_contents(ui);
            });
        // The edge only resizes: a drag or double click must not close it.
        self.queue_ui.open &= !close;
        if let Some(shown) = shown {
            self.remember_panel_width(
                ui.ctx(),
                1,
                super::RIGHT_PANEL_ID,
                shown.response.rect.width(),
            );
        }
    }

    /// The queue itself, in its own panel or above the lyrics. Returns
    /// whether its close button was clicked.
    pub(super) fn queue_contents(&mut self, ui: &mut egui::Ui) -> bool {
        // A track carried from a list joins the queue as soon as it is over
        // it, and the queue's own drag carries it on to its place.
        let area = ui.max_rect().expand(6.0);
        if self.queue_ui.adopted.is_some() && !ui.input(|input| input.pointer.primary_down()) {
            self.queue_ui.adopted = None;
        }
        let target = crate::track_drag::target(ui, area);
        if target != crate::track_drag::Target::None
            && let Some(carried) = crate::track_drag::carried(ui.ctx())
        {
            if let Some(id) = self.enqueue_carried(&carried.track) {
                crate::track_drag::hand_over(ui.ctx());
                if target == crate::track_drag::Target::Hovered {
                    self.queue_ui.adopted = Some(Adopted {
                        id,
                        track: carried.track,
                        started: false,
                    });
                }
            } else if target == crate::track_drag::Target::Dropped {
                // Refused: say why, as the menu does.
                crate::track_drag::clear(ui.ctx());
                self.add_to_queue(ui.ctx(), carried.track);
            } else {
                crate::track_drag::paint_target(ui.ctx(), area, 10.0);
            }
        }
        let mut adoption = None;
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
        let mut close = false;
        let mut action = None;
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        {
            {
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
                                "Добавляйте треки через контекстное меню или перетаскивайте их сюда из списка.",
                                "Add tracks from their context menu, or drag them here from a list.",
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
                            // "Up next": rows retracing history, then the added
                            // tracks, then the source. Added tracks and the
                            // source each reorder within their own block.
                            let len = panel.upcoming.len();
                            let queued_start = self.play_order.history_ahead(current).min(len);
                            let added = panel.upcoming[queued_start..]
                                .iter()
                                .take_while(|entry| entry.queued_id.is_some())
                                .count();
                            let source_start = queued_start + added;
                            let source_movable = self.play_order.source_reorderable(current)
                                && len > source_start + 1;
                            let group = |index: usize| {
                                if (queued_start..source_start).contains(&index) && added > 1 {
                                    Some(queued_start..=source_start - 1)
                                } else if index >= source_start && source_movable {
                                    Some(source_start..=len - 1)
                                } else {
                                    None
                                }
                            };
                            let stride = 50.0 + ui.spacing().item_spacing.y;
                            // A track can be listed twice, say once played
                            // ahead and once in the source: each row gets its
                            // own identity from its block and its repeat.
                            let row_keys: Vec<u64> = {
                                let mut seen: HashMap<(u8, TrackKey), u64> = HashMap::new();
                                panel
                                    .upcoming
                                    .iter()
                                    .enumerate()
                                    .map(|(index, entry)| {
                                        if let Some(id) = entry.queued_id {
                                            return row_drag::track_key((1, id as i64));
                                        }
                                        let block = u8::from(index >= source_start);
                                        let repeat = seen.entry((block, entry.key)).or_insert(0);
                                        *repeat += 1;
                                        row_drag::track_key((
                                            i64::from(block) + 2,
                                            row_drag::track_key(entry.key) as i64
                                                ^ (*repeat as i64),
                                        ))
                                    })
                                    .collect()
                            };
                            let row_key = |index: usize| row_keys[index];
                            let mut lane = row_drag::Lane::begin(
                                ui,
                                egui::Id::new("queue-row-drag"),
                                ui.max_rect().top() - range.start as f32 * stride,
                                stride,
                                50.0,
                                // A new source is a new list: its rows start in
                                // place even where the old list had them too.
                                row_drag::scope_key(&format!(
                                    "{}:{:x}",
                                    self.queue_origin.as_deref().unwrap_or("library"),
                                    self.play_queue
                                        .as_ref()
                                        .map_or(0, |tracks| tracks.as_ptr() as usize),
                                )),
                                |key| (0..len).find(|index| row_key(*index) == key),
                            );
                            // A carried-in track: picked up here, or done.
                            if let Some(adopted) = &mut panel.adopted {
                                if !adopted.started {
                                    if let Some(index) = panel
                                        .upcoming
                                        .iter()
                                        .position(|entry| entry.queued_id == Some(adopted.id))
                                    {
                                        let slots = group(index).unwrap_or(index..=index);
                                        lane.adopt(index, row_key(index), slots);
                                        adopted.started = true;
                                    }
                                } else if let Some(ended) = lane.ended() {
                                    if ended == row_drag::Ended::Canceled {
                                        adoption = Some(Adoption::Remove(adopted.id));
                                    }
                                    panel.adopted = None;
                                } else if lane.away() {
                                    let pointer = ui
                                        .input(|input| input.pointer.interact_pos())
                                        .unwrap_or_default();
                                    adoption = Some(Adoption::Revert(
                                        adopted.id,
                                        Box::new(adopted.track.clone()),
                                        egui::Rect::from_center_size(
                                            pointer,
                                            egui::vec2(lane.clip().width(), 50.0),
                                        ),
                                    ));
                                    lane.cancel();
                                    panel.adopted = None;
                                } else if !lane.dragging() {
                                    panel.adopted = None;
                                }
                            }
                            let lifted = lane.dragging() || lane.suppressing();
                            let rows = range.start.saturating_sub(1)..(range.end + 1).min(len);
                            for index in rows {
                                let entry = panel.upcoming[index];
                                let key = row_key(index);
                                if lane.hides(key) {
                                    continue;
                                }
                                let rect = lane.rect(ui.ctx(), index, key);
                                let Some(track) = self
                                    .queue_track(
                                        entry.key,
                                        panel.source_indices.get(&entry.key).copied(),
                                    )
                                    .cloned()
                                else {
                                    continue;
                                };
                                self.row_drag_enabled = group(index).is_some() && !lane.dragging();
                                let row_action = ui
                                    .scope_builder(
                                        egui::UiBuilder::new().max_rect(rect).id_salt((
                                            stamp.0,
                                            index,
                                            entry.queued_id,
                                            entry.key,
                                        )),
                                        |ui| self.queue_row(ui, &track, entry.queued_id, false),
                                    )
                                    .inner;
                                self.row_drag_enabled = false;
                                if let Some(started) = self.row_drag_started.take()
                                    && let Some(slots) = group(index)
                                {
                                    lane.start(ui, index, key, started, slots);
                                }
                                if !lifted
                                    && !lane.dragging()
                                    && let Some(row_action) = row_action
                                {
                                    action = Some(match row_action {
                                        Action::Play(_) => Action::Play(index),
                                        other => other,
                                    });
                                }
                            }
                            if let Some(moved) = lane.moved {
                                if moved.from < source_start {
                                    action = Some(Action::Move {
                                        from: moved.from - queued_start,
                                        to: moved.to - queued_start,
                                    });
                                } else if let Some(neighbour) = (row_drag::Move {
                                    from: moved.from - source_start,
                                    to: moved.to - source_start,
                                })
                                .neighbour(|index| panel.upcoming[source_start + index].key)
                                {
                                    action = Some(Action::MoveSource {
                                        key: panel.upcoming[moved.from].key,
                                        neighbour,
                                    });
                                }
                            }
                            if let Some((from, rect)) = lane.finish(ui) {
                                let entry = panel.upcoming[from];
                                if let Some(track) = self
                                    .queue_track(
                                        entry.key,
                                        panel.source_indices.get(&entry.key).copied(),
                                    )
                                    .cloned()
                                {
                                    ui.scope_builder(
                                        egui::UiBuilder::new()
                                            .max_rect(rect)
                                            .id_salt("queue-drag-ghost"),
                                        |ui| {
                                            self.queue_row(ui, &track, entry.queued_id, false);
                                        },
                                    );
                                }
                            }
                        },
                    );
                }
            }
        }
        self.queue_ui = panel;
        let ctx = ui.ctx();
        match adoption {
            Some(Adoption::Remove(id)) => {
                self.play_order.remove_queued(id);
                self.prune_queued_tracks();
            }
            Some(Adoption::Revert(id, track, row)) => {
                self.play_order.remove_queued(id);
                self.prune_queued_tracks();
                crate::track_drag::carry(
                    ctx,
                    crate::track_drag::Carried {
                        track: *track,
                        list: row,
                        row,
                        reorder: false,
                    },
                );
            }
            None => {}
        }
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
            Some(Action::Move { from, to })
                if self.queue_ui.stamp == Some(stamp) && self.play_order.revision() == stamp.0 =>
            {
                self.play_order.move_queued(from, to);
            }
            Some(Action::MoveSource { key, neighbour })
                if self.queue_ui.stamp == Some(stamp) && self.play_order.revision() == stamp.0 =>
            {
                self.move_upcoming_source(key, neighbour);
            }
            Some(Action::Clear) => {
                self.play_order.clear_queued();
                self.prune_queued_tracks();
            }
            Some(Action::Track(action)) => self.handle_track_action(ctx, *action),
            _ => {}
        }
        close
    }

    /// Reorders what plays next from the source: the shuffle's picks, or
    /// this session's copy of the source list. The library and the open
    /// playlist themselves keep their order.
    fn move_upcoming_source(&mut self, key: TrackKey, neighbour: row_drag::Neighbour<TrackKey>) {
        if self.play_order.shuffle() {
            let (other, after) = match neighbour {
                row_drag::Neighbour::After(other) => (other, true),
                row_drag::Neighbour::Before(other) => (other, false),
            };
            self.play_order.move_remaining(key, other, after);
            return;
        }
        if self.play_queue.is_none() {
            self.play_queue = Some(self.library.tracks.clone());
            self.queue_origin.get_or_insert_with(|| "library".into());
        }
        if let Some(queue) = &mut self.play_queue
            && row_drag::apply(queue, Track::key, key, neighbour)
        {
            // The drawer's row lookup follows the view revision.
            self.track_view_cache.invalidate();
            self.synchronize_order();
        }
    }

    fn queue_row(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        queued_id: Option<u64>,
        current: bool,
    ) -> Option<Action> {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 50.0),
            if self.row_drag_enabled {
                Sense::click_and_drag()
            } else {
                Sense::click()
            },
        );
        if self.row_drag_enabled && response.drag_started_by(egui::PointerButton::Primary) {
            self.row_drag_started = Some(rect);
        }
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
            // 3 px from the overlay scrollbar; the cross icon starts 7 px
            // inside its area, so the text ends where the icon begins.
            remove.left() + 7.0
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
                let badge = theme::explicit_badge(ui.ctx(), Some(track), 12.0);
                if let Some(artist) = theme::badged_line(ui, badge, |ui| {
                    detail_ui::artist_links_with_font(
                        ui,
                        &track.artists,
                        &track.artist,
                        egui::FontId::proportional(12.0),
                    )
                }) {
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
