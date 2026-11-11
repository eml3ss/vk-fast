//! Drag-and-drop reordering of track rows, with the library sidebar's
//! motion: neighbours slide aside, the row follows the pointer above the
//! list, a blue line marks the drop slot and the list scrolls at its edges.
use super::*;

/// The sidebar's slide time, so both lists move alike.
const REORDER_SECONDS: f32 = 0.16;

#[derive(Clone)]
struct Drag {
    key: u64,
    from: usize,
    to: usize,
    /// The rows this drag may reach: its own group.
    slots: std::ops::RangeInclusive<usize>,
    grab_y: f32,
    pointer: egui::Pos2,
}

#[derive(Clone, Default)]
struct Memory {
    active: Option<Drag>,
    /// The press that started a drag must not also click on release.
    suppress_release: bool,
    /// Rows drawn in the frame below; only they keep moving. A row that
    /// was hidden for a while starts in place instead of flying in from
    /// wherever it was last seen.
    drawn: HashSet<u64>,
    frame: u64,
    /// The list those rows belonged to: a track in two playlists does not
    /// fly from one list's place to the other's.
    scope: u64,
}

/// A finished drag: the row at `from` goes to `to`, both in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Move {
    pub from: usize,
    pub to: usize,
}

impl Move {
    /// The neighbour VK needs: behind the row now before it, or in front of
    /// the first row when it becomes first.
    pub(super) fn neighbour<K>(self, key_at: impl Fn(usize) -> K) -> Option<Neighbour<K>> {
        if self.to > self.from {
            Some(Neighbour::After(key_at(self.to)))
        } else if self.to < self.from {
            Some(if self.to == 0 {
                Neighbour::Before(key_at(0))
            } else {
                Neighbour::After(key_at(self.to - 1))
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Neighbour<K> {
    After(K),
    Before(K),
}

/// Moves the item `key` next to its neighbour; rows that are hidden from
/// the view keep their places.
pub(super) fn apply<T, K: PartialEq + Copy>(
    items: &mut Vec<T>,
    key: impl Fn(&T) -> K,
    moved: K,
    neighbour: Neighbour<K>,
) -> bool {
    let Some(from) = items.iter().position(|item| key(item) == moved) else {
        return false;
    };
    let other = match neighbour {
        Neighbour::After(other) | Neighbour::Before(other) => other,
    };
    if other == moved || !items.iter().any(|item| key(item) == other) {
        return false;
    }
    let item = items.remove(from);
    let at = items.iter().position(|item| key(item) == other).unwrap();
    items.insert(
        match neighbour {
            Neighbour::After(_) => at + 1,
            Neighbour::Before(_) => at,
        },
        item,
    );
    true
}

/// Where an item sits now, for undoing a move later.
pub(super) fn current_neighbour<T, K: Copy>(
    items: &[T],
    key: impl Fn(&T) -> K,
    index: usize,
) -> Option<Neighbour<K>> {
    if index > 0 {
        Some(Neighbour::After(key(&items[index - 1])))
    } else {
        items.get(1).map(|item| Neighbour::Before(key(item)))
    }
}

/// One frame of a reorderable list of equally tall rows.
pub(super) struct Lane {
    id: egui::Id,
    origin_y: f32,
    stride: f32,
    height: f32,
    left: f32,
    width: f32,
    clip: egui::Rect,
    memory: Memory,
    pointer: Option<egui::Pos2>,
    held: bool,
    escape: bool,
    /// The dragged row is carried away from the list, to a playlist or the
    /// queue: it waits in its own place meanwhile.
    away: bool,
    settled: Option<Drag>,
    pub(super) moved: Option<Move>,
    previous: HashSet<u64>,
    drawn: HashSet<u64>,
    /// How far the row above still is from its place, so a row entering
    /// below it moves along instead of waiting in its final place.
    carry: f32,
}

impl Lane {
    /// `origin_y` is the top of row 0, `scope` names the list shown (rows of
    /// another list start in place) and `position` finds a dragged row
    /// again after the list changed.
    pub(super) fn begin(
        ui: &mut egui::Ui,
        id: egui::Id,
        origin_y: f32,
        stride: f32,
        height: f32,
        scope: u64,
        position: impl Fn(u64) -> Option<usize>,
    ) -> Self {
        let ctx = ui.ctx().clone();
        let mut memory = ctx
            .data_mut(|data| data.get_temp::<Memory>(id))
            .unwrap_or_default();
        let left = ui.max_rect().left();
        let width = ui.available_width();
        let clip = ui.clip_rect().intersect(egui::Rect::from_min_max(
            egui::pos2(left, ui.clip_rect().top()),
            egui::pos2(left + width, ui.clip_rect().bottom()),
        ));
        let pointer = ui
            .input(|input| input.pointer.interact_pos())
            .or_else(|| memory.active.as_ref().map(|drag| drag.pointer));
        let (released, held) = ui.input(|input| {
            (
                input.pointer.button_released(egui::PointerButton::Primary),
                input.pointer.primary_down(),
            )
        });
        let escape = memory.active.is_some()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut lane = Self {
            id,
            origin_y,
            stride,
            height,
            left,
            width,
            clip,
            memory: Memory::default(),
            pointer,
            held,
            escape,
            away: false,
            settled: None,
            moved: None,
            previous: HashSet::new(),
            drawn: HashSet::new(),
            carry: 0.0,
        };
        let frame = ctx.cumulative_frame_nr();
        if frame.saturating_sub(memory.frame) <= 1 && memory.scope == scope {
            lane.previous = std::mem::take(&mut memory.drawn);
        } else {
            // Another list: no row carries its old place, and no drag
            // started in the old list goes on.
            memory.active = None;
        }
        memory.scope = scope;
        let inside = pointer
            .is_some_and(|point| point.x >= clip.left() - 8.0 && point.x <= clip.right() + 8.0);
        if let Some(drag) = &mut memory.active {
            match position(drag.key) {
                Some(from) if drag.slots.contains(&from) => {
                    drag.from = from;
                    if !inside {
                        drag.to = from;
                        lane.away = true;
                    } else if let Some(pointer) = pointer {
                        drag.pointer = pointer;
                        lane.target(drag);
                        // The source follows the pointer, so release
                        // settles it smoothly into its slot.
                        ctx.animate_value_with_time(
                            id.with(("row-y", drag.key)),
                            lane.ghost_top(drag) - origin_y,
                            0.0,
                        );
                    }
                }
                _ => memory.active = None,
            }
        }
        if (escape || !held)
            && let Some(drag) = memory.active.take()
            && released
            && !escape
            && inside
            && drag.from != drag.to
        {
            lane.moved = Some(Move {
                from: drag.from,
                to: drag.to,
            });
            lane.settled = Some(drag);
        }
        lane.memory = memory;
        lane
    }

    fn ghost_top(&self, drag: &Drag) -> f32 {
        (drag.pointer.y - drag.grab_y).clamp(
            self.clip.top(),
            (self.clip.bottom() - self.height).max(self.clip.top()),
        )
    }

    fn target(&self, drag: &mut Drag) {
        let top = self.ghost_top(drag);
        let slot = ((top - self.origin_y) / self.stride).round().max(0.0) as usize;
        drag.to = slot.clamp(*drag.slots.start(), *drag.slots.end());
    }

    pub(super) fn dragging(&self) -> bool {
        self.memory.active.is_some()
    }

    /// The display row being dragged, if any.
    pub(super) fn dragged(&self) -> Option<usize> {
        self.memory.active.as_ref().map(|drag| drag.from)
    }

    /// The list's visible area.
    pub(super) fn clip(&self) -> egui::Rect {
        self.clip
    }

    /// The press of a finished drag is still held or just released.
    pub(super) fn suppressing(&self) -> bool {
        self.memory.suppress_release
    }

    /// The dragged row is painted by `finish`, not in the list.
    pub(super) fn hides(&self, key: u64) -> bool {
        !self.away
            && self
                .memory
                .active
                .as_ref()
                .is_some_and(|drag| drag.key == key)
    }

    /// Where row `index` is drawn now, sliding to its slot. Call it for the
    /// drawn rows from top to bottom.
    pub(super) fn rect(&mut self, ctx: &egui::Context, index: usize, key: u64) -> egui::Rect {
        let slot = match self.memory.active.as_ref().or(self.settled.as_ref()) {
            Some(drag) if index == drag.from => drag.to,
            Some(drag) if drag.from < drag.to && index > drag.from && index <= drag.to => index - 1,
            Some(drag) if drag.to < drag.from && index >= drag.to && index < drag.from => index + 1,
            _ => index,
        };
        let id = self.id.with(("row-y", key));
        let target = slot as f32 * self.stride;
        if !self.previous.contains(&key) {
            ctx.animate_value_with_time(id, target + self.carry, 0.0);
        }
        let y = ctx.animate_value_with_time(id, target, REORDER_SECONDS);
        self.carry = y - target;
        self.drawn.insert(key);
        egui::Rect::from_min_size(
            egui::pos2(self.left, self.origin_y + y),
            egui::vec2(self.width, self.height),
        )
    }

    /// A row's own response reports that a drag began on it; `slots` are
    /// the rows of its group.
    pub(super) fn start(
        &mut self,
        ui: &egui::Ui,
        index: usize,
        key: u64,
        rect: egui::Rect,
        slots: std::ops::RangeInclusive<usize>,
    ) {
        if self.memory.active.is_some()
            || self.memory.suppress_release
            || self.escape
            || !slots.contains(&index)
        {
            return;
        }
        let grab_y = ui
            .input(|input| input.pointer.press_origin())
            .map_or(self.height * 0.5, |point| {
                (point.y - rect.top()).clamp(0.0, self.height)
            });
        let mut drag = Drag {
            key,
            from: index,
            to: index,
            slots,
            grab_y,
            pointer: self.pointer.unwrap_or(rect.center()),
        };
        self.target(&mut drag);
        self.memory.active = Some(drag);
        self.memory.suppress_release = true;
    }

    /// Paints the drop line and the lifted row's shadow, scrolls at the
    /// edges and returns the dragged row's index and rect for its content.
    pub(super) fn finish(mut self, ui: &mut egui::Ui) -> Option<(usize, egui::Rect)> {
        let ctx = ui.ctx().clone();
        let mut ghost = None;
        if let Some(drag) = &self.memory.active
            && let Some(pointer) = self.pointer
            && !self.away
        {
            let target = self.origin_y + drag.to as f32 * self.stride;
            let gap = (self.stride - self.height) * 0.5;
            let marker = egui::Rect::from_min_size(
                egui::pos2(self.left + 6.0, target - gap - 1.0),
                egui::vec2((self.width - 12.0).max(0.0), 2.0),
            );
            ui.painter().rect_filled(marker, 1.0, theme::ACCENT);
            let rect = egui::Rect::from_min_size(
                egui::pos2(self.left, self.ghost_top(drag)),
                egui::vec2(self.width, self.height),
            );
            ctx.animate_value_with_time(
                self.id.with(("row-y", drag.key)),
                rect.top() - self.origin_y,
                0.0,
            );
            ui.painter().rect_filled(
                rect.translate(egui::vec2(0.0, 2.0)).expand(1.0),
                9.0,
                Color32::from_black_alpha(90),
            );
            ui.painter()
                .rect_filled(rect, 8.0, Color32::from_rgb(31, 38, 49));
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let clip = self.clip;
            let edge = 32.0_f32.min(clip.height() * 0.25);
            let inside = pointer.x >= clip.left() - 8.0 && pointer.x <= clip.right() + 8.0;
            if edge > 0.0 && inside {
                let speed = if pointer.y < clip.top() + edge {
                    ((clip.top() + edge - pointer.y) / edge).clamp(0.0, 1.0) * 360.0
                } else if pointer.y > clip.bottom() - edge {
                    -((pointer.y - clip.bottom() + edge) / edge).clamp(0.0, 1.0) * 360.0
                } else {
                    0.0
                };
                if speed != 0.0 {
                    let dt = ui.input(|input| input.stable_dt).min(0.05);
                    ui.scroll_with_delta_animation(
                        egui::vec2(0.0, speed * dt),
                        egui::style::ScrollAnimation::none(),
                    );
                }
            }
            ctx.request_repaint();
            ghost = Some((drag.from, rect));
        }
        if !self.held {
            self.memory.suppress_release = false;
        }
        if let Some(drag) = &self.memory.active {
            self.drawn.insert(drag.key);
        }
        self.memory.drawn = std::mem::take(&mut self.drawn);
        self.memory.frame = ctx.cumulative_frame_nr();
        ctx.data_mut(|data| data.insert_temp(self.id, self.memory));
        ghost
    }
}

/// A list's identity for `Lane::begin`.
pub(super) fn scope_key(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

/// A stable row identity for animation, from a track key.
pub(super) fn track_key(key: (i64, i64)) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_name_the_neighbour_and_apply_keeps_hidden_rows_in_place() {
        let keys = [1, 2, 3, 4];
        let at = |index: usize| keys[index];
        assert_eq!(
            Move { from: 0, to: 2 }.neighbour(at),
            Some(Neighbour::After(3))
        );
        assert_eq!(
            Move { from: 3, to: 1 }.neighbour(at),
            Some(Neighbour::After(1))
        );
        assert_eq!(
            Move { from: 2, to: 0 }.neighbour(at),
            Some(Neighbour::Before(1))
        );
        assert_eq!(Move { from: 1, to: 1 }.neighbour(at), None);
        // 9 is hidden from the view between 2 and 3.
        let mut items = vec![1, 2, 9, 3, 4];
        assert!(apply(&mut items, |item| *item, 1, Neighbour::After(3)));
        assert_eq!(items, [2, 9, 3, 1, 4]);
        let back = current_neighbour(&[1, 2, 9, 3, 4], |item| *item, 0).unwrap();
        assert_eq!(back, Neighbour::Before(2));
        assert!(apply(&mut items, |item| *item, 1, back));
        assert_eq!(items, [1, 2, 9, 3, 4]);
        assert!(!apply(&mut items, |item| *item, 7, Neighbour::After(3)));
        assert!(!apply(&mut items, |item| *item, 1, Neighbour::After(1)));
    }
}
