//! Playback order is separate from the displayed library. Probes never consume
//! a shuffle candidate; explicit advances commit navigation before playback.
use std::collections::{HashSet, VecDeque};

pub type TrackKey = (i64, i64);
const HISTORY_LIMIT: usize = 50_000;
pub const QUEUE_LIMIT: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Queued {
    pub id: u64,
    pub key: TrackKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upcoming {
    pub key: TrackKey,
    /// Only unplayed, explicitly added entries can be removed from the queue.
    pub queued_id: Option<u64>,
}

#[derive(Clone, Copy, Default)]
enum Visit {
    #[default]
    Source,
    Queued {
        anchor: Option<TrackKey>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

#[derive(Clone, Default)]
pub struct PlaybackOrder {
    shuffle: bool,
    repeat: RepeatMode,
    keys: Vec<TrackKey>,
    remaining: VecDeque<TrackKey>,
    visited: HashSet<TrackKey>,
    history: Vec<TrackKey>,
    visits: Vec<Visit>,
    cursor: usize,
    wrap: Vec<TrackKey>,
    random: u64,
    queued: VecDeque<Queued>,
    next_id: u64,
    revision: u64,
}

impl PlaybackOrder {
    pub fn with_modes(shuffle: bool, repeat: RepeatMode) -> Self {
        Self {
            shuffle,
            repeat,
            ..Self::default()
        }
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn queued(&self) -> &VecDeque<Queued> {
        &self.queued
    }

    pub fn enqueue(&mut self, key: TrackKey) -> bool {
        if key.0 == 0
            || key.0.checked_abs().is_none()
            || key.1 <= 0
            || self.queued.len() >= QUEUE_LIMIT
        {
            return false;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.queued.push_back(Queued {
            id: self.next_id,
            key,
        });
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn remove_queued(&mut self, id: u64) {
        let before = self.queued.len();
        self.queued.retain(|entry| entry.id != id);
        if before != self.queued.len() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn clear_queued(&mut self) {
        if !self.queued.is_empty() {
            self.queued.clear();
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn queued_keys_to_keep(&self) -> impl Iterator<Item = TrackKey> + '_ {
        self.queued.iter().map(|entry| entry.key).chain(
            self.history
                .iter()
                .copied()
                .zip(&self.visits)
                .filter_map(|(key, visit)| matches!(visit, Visit::Queued { .. }).then_some(key)),
        )
    }

    pub fn is_queued_current(&self, current: TrackKey) -> bool {
        self.history.get(self.cursor) == Some(&current)
            && matches!(self.visits.get(self.cursor), Some(Visit::Queued { .. }))
    }

    fn anchor(&self, current: TrackKey) -> Option<TrackKey> {
        if self.history.get(self.cursor) == Some(&current)
            && let Some(Visit::Queued { anchor }) = self.visits.get(self.cursor)
        {
            return *anchor;
        }
        Some(current)
    }

    /// Explicitly selecting a source row leaves an injected track's old anchor.
    pub fn select_source(&mut self, key: TrackKey) {
        if self.is_queued_current(key) && self.keys.contains(&key) {
            self.commit_new(key);
        } else {
            self.record_played(key);
        }
    }

    pub fn start_queued(&mut self, current: Option<TrackKey>) -> Option<TrackKey> {
        let entry = self.queued.pop_front()?;
        let anchor = current
            .and_then(|key| self.anchor(key))
            .filter(|key| self.keys.contains(key));
        self.push_history(entry.key, Visit::Queued { anchor });
        Some(entry.key)
    }

    /// Jump to a displayed entry without recording skipped tracks as played.
    pub fn jump_to(
        &mut self,
        position: usize,
        current: Option<TrackKey>,
        allow_wrap: bool,
    ) -> Option<TrackKey> {
        let mut position = position;
        if let Some(key) = self.repeats(current) {
            // The first row is the repeated current track: play it again.
            if position == 0 {
                return Some(key);
            }
            position -= 1;
        }
        let target = *self.upcoming_after(current, allow_wrap).get(position)?;
        let future = if current.is_some_and(|key| self.history.get(self.cursor) == Some(&key)) {
            self.history.len() - self.cursor - 1
        } else {
            0
        };
        if position < future {
            self.cursor += position + 1;
            self.revision = self.revision.wrapping_add(1);
            if !self.is_queued_current(target.key) {
                self.mark_visited(target.key);
            }
            return Some(target.key);
        }
        if let Some(id) = target.queued_id {
            let index = self.queued.iter().position(|entry| entry.id == id)?;
            let anchor = if future > 0 {
                match self.visits.last() {
                    Some(Visit::Queued { anchor }) => *anchor,
                    _ => self.history.last().copied(),
                }
            } else {
                current.and_then(|key| self.anchor(key))
            };
            self.queued.drain(..=index);
            self.push_history(target.key, Visit::Queued { anchor });
        } else {
            self.clear_queued();
            if self.shuffle {
                if self.remaining.is_empty() && allow_wrap && self.repeat == RepeatMode::All {
                    self.remaining = std::mem::take(&mut self.wrap).into();
                    self.visited.clear();
                }
                while self.remaining.front().is_some_and(|key| *key != target.key) {
                    if let Some(key) = self.remaining.pop_front() {
                        self.visited.insert(key);
                    }
                }
            }
            self.commit_new(target.key);
        }
        Some(target.key)
    }

    /// A read-only snapshot for the menu. It never draws a new shuffle order.
    /// Repeat one replays the current track, so it is also the next one.
    pub fn repeats(&self, current: Option<TrackKey>) -> Option<TrackKey> {
        current.filter(|key| {
            self.repeat == RepeatMode::One
                && (self.keys.contains(key) || self.is_queued_current(*key))
        })
    }

    pub fn upcoming(&self, current: Option<TrackKey>, allow_wrap: bool) -> Vec<Upcoming> {
        let mut result = self.upcoming_after(current, allow_wrap);
        if let Some(key) = self.repeats(current) {
            result.insert(
                0,
                Upcoming {
                    key,
                    queued_id: None,
                },
            );
        }
        result
    }

    fn upcoming_after(&self, current: Option<TrackKey>, allow_wrap: bool) -> Vec<Upcoming> {
        let mut result = Vec::new();
        let mut anchor = current.and_then(|key| self.anchor(key));
        if current.is_some_and(|key| self.history.get(self.cursor) == Some(&key)) {
            for (key, visit) in self.history[self.cursor + 1..]
                .iter()
                .copied()
                .zip(&self.visits[self.cursor + 1..])
            {
                result.push(Upcoming {
                    key,
                    queued_id: None,
                });
                anchor = match visit {
                    Visit::Source => Some(key),
                    Visit::Queued { anchor } => *anchor,
                };
            }
        }
        result.extend(self.queued.iter().map(|entry| Upcoming {
            key: entry.key,
            queued_id: Some(entry.id),
        }));
        if self.shuffle {
            let remaining: Vec<_> = self
                .remaining
                .iter()
                .copied()
                .filter(|key| Some(*key) != anchor)
                .collect();
            if remaining.is_empty() && allow_wrap && self.repeat == RepeatMode::All {
                result.extend(
                    self.wrap
                        .iter()
                        .copied()
                        .filter(|key| self.keys.len() == 1 || Some(*key) != anchor)
                        .map(|key| Upcoming {
                            key,
                            queued_id: None,
                        }),
                );
            } else {
                result.extend(remaining.into_iter().map(|key| Upcoming {
                    key,
                    queued_id: None,
                }));
            }
        } else {
            let start = anchor
                .and_then(|key| self.keys.iter().position(|candidate| *candidate == key))
                .map_or(0, |index| index + 1);
            let start = if start == self.keys.len() && allow_wrap && self.repeat == RepeatMode::All
            {
                0
            } else {
                start
            };
            result.extend(self.keys[start..].iter().copied().map(|key| Upcoming {
                key,
                queued_id: None,
            }));
        }
        result
    }

    pub fn exclude_queued(&mut self, rejected: &HashSet<TrackKey>) {
        let before = self.queued.len();
        self.queued.retain(|entry| !rejected.contains(&entry.key));
        if !self
            .history
            .iter()
            .zip(&self.visits)
            .enumerate()
            .any(|(index, (key, visit))| {
                index != self.cursor
                    && matches!(visit, Visit::Queued { .. })
                    && rejected.contains(key)
            })
        {
            if before != self.queued.len() {
                self.revision = self.revision.wrapping_add(1);
            }
            return;
        }
        let mut cursor = self.cursor;
        let mut history = Vec::with_capacity(self.history.len());
        let mut visits = Vec::with_capacity(self.visits.len());
        for (index, (key, visit)) in self
            .history
            .iter()
            .copied()
            .zip(self.visits.iter().copied())
            .enumerate()
        {
            if index != self.cursor
                && matches!(visit, Visit::Queued { .. })
                && rejected.contains(&key)
            {
                if index < self.cursor {
                    cursor -= 1;
                }
                continue;
            }
            history.push(key);
            visits.push(visit);
        }
        if before != self.queued.len() || history.len() != self.history.len() {
            self.revision = self.revision.wrapping_add(1);
        }
        self.history = history;
        self.visits = visits;
        self.cursor = cursor;
    }

    pub fn remaining_after(&self, current: TrackKey) -> usize {
        let future = if self.history.get(self.cursor) == Some(&current) {
            self.history.len() - self.cursor - 1
        } else {
            0
        };
        let anchor = if future > 0 {
            match self.visits.last() {
                Some(Visit::Queued { anchor }) => *anchor,
                _ => self.history.last().copied(),
            }
        } else {
            self.anchor(current)
        };
        if !self.shuffle {
            return future
                + self.queued.len()
                + self
                    .keys
                    .iter()
                    .position(|key| Some(*key) == anchor)
                    .map_or(
                        if anchor.is_none() { self.keys.len() } else { 0 },
                        |index| self.keys.len() - index - 1,
                    );
        }
        future
            + self.queued.len()
            + self
                .remaining
                .iter()
                .filter(|key| Some(**key) != anchor)
                .count()
    }

    pub fn first(&self) -> Option<TrackKey> {
        if self.shuffle {
            self.remaining
                .front()
                .copied()
                .or_else(|| self.wrap.first().copied())
        } else {
            self.keys.first().copied()
        }
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.revision = self.revision.wrapping_add(1);
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.repeat
    }

    pub fn set_shuffle(&mut self, enabled: bool, keys: &[TrackKey], current: Option<TrackKey>) {
        if enabled != self.shuffle {
            self.revision = self.revision.wrapping_add(1);
            self.shuffle = enabled;
            self.remaining.clear();
            self.visited.clear();
            self.wrap.clear();
        }
        self.sync(keys, current);
    }

    /// Call when choosing a different source, preserving the user's modes.
    pub fn reset_source(&mut self, keys: &[TrackKey], current: Option<TrackKey>) {
        self.keys.clear();
        self.remaining.clear();
        self.visited.clear();
        self.history.clear();
        self.visits.clear();
        self.revision = self.revision.wrapping_add(1);
        self.cursor = 0;
        self.wrap.clear();
        self.sync(keys, current);
    }

    /// Appending a page only adds its new identities to the remaining bag.
    pub fn sync(&mut self, keys: &[TrackKey], current: Option<TrackKey>) {
        if keys == self.keys
            && (!self.shuffle
                || !self.remaining.is_empty()
                || !self.wrap.is_empty()
                || keys.is_empty())
        {
            if let Some(key) = current
                && self.history.get(self.cursor) != Some(&key)
            {
                self.record_played(key);
            }
            return;
        }
        let mut available = HashSet::new();
        let keys: Vec<_> = keys
            .iter()
            .copied()
            .filter(|key| available.insert(*key))
            .collect();
        let changed = keys != self.keys;
        let previous_keys = std::mem::replace(&mut self.keys, keys);
        self.visited.retain(|key| available.contains(key));
        self.remaining.retain(|key| available.contains(key));
        if changed {
            self.revision = self.revision.wrapping_add(1);
            let mut cursor = 0;
            let mut history = Vec::with_capacity(self.history.len());
            let mut visits = Vec::with_capacity(self.visits.len());
            for (index, (key, mut visit)) in self
                .history
                .iter()
                .copied()
                .zip(self.visits.iter().copied())
                .enumerate()
            {
                if available.contains(&key) || matches!(visit, Visit::Queued { .. }) {
                    if index <= self.cursor {
                        cursor = history.len();
                    }
                    history.push(key);
                    if let Visit::Queued { anchor } = &mut visit
                        && let Some(old) = *anchor
                        && !available.contains(&old)
                    {
                        *anchor = if self.shuffle {
                            None
                        } else {
                            previous_keys
                                .iter()
                                .position(|key| *key == old)
                                .and_then(|index| {
                                    previous_keys[..index]
                                        .iter()
                                        .rev()
                                        .find(|key| available.contains(key))
                                        .copied()
                                })
                        };
                    }
                    visits.push(visit);
                }
            }
            self.history = history;
            self.visits = visits;
            self.cursor = cursor.min(self.history.len().saturating_sub(1));
            self.wrap.clear();
        }
        if let Some(current) = current {
            if self.is_queued_current(current) {
                if let Some(anchor) = self.anchor(current).filter(|key| available.contains(key)) {
                    self.visited.insert(anchor);
                    self.remaining.retain(|key| *key != anchor);
                }
            } else if available.contains(&current) {
                self.record_played(current);
            }
        }
        if self.shuffle {
            let queued: HashSet<_> = self.remaining.iter().copied().collect();
            let mut added: Vec<_> = self
                .keys
                .iter()
                .copied()
                .filter(|key| !self.visited.contains(key) && !queued.contains(key))
                .collect();
            self.randomize(&mut added);
            self.remaining.extend(added);
            self.prepare_wrap(current);
        }
    }

    /// Manual row selection is a new history branch. Calling this after an
    /// advance is harmless because that target is already the current entry.
    pub fn record_played(&mut self, key: TrackKey) {
        if self.is_queued_current(key) {
            return;
        }
        if !self.keys.contains(&key) {
            return;
        }
        if self.history.get(self.cursor) == Some(&key) {
            self.visited.insert(key);
            self.remaining.retain(|candidate| *candidate != key);
            self.prepare_wrap(Some(key));
            return;
        }
        self.commit_new(key);
    }

    pub fn next(&self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        if self.history.get(self.cursor) == Some(&current)
            && let Some(key) = self.history.get(self.cursor + 1)
        {
            return Some(*key);
        }
        if let Some(entry) = self.queued.front() {
            return Some(entry.key);
        }
        let anchor = self.anchor(current);
        if !self.shuffle {
            let Some(anchor) = anchor else {
                return self.keys.first().copied();
            };
            let index = self.keys.iter().position(|key| *key == anchor)?;
            return self.keys.get(index + 1).copied().or_else(|| {
                (allow_wrap && self.repeat == RepeatMode::All)
                    .then(|| self.keys.first().copied())
                    .flatten()
            });
        }
        self.remaining
            .iter()
            .copied()
            .find(|key| Some(*key) != anchor)
            .or_else(|| {
                if !allow_wrap || self.repeat != RepeatMode::All {
                    return None;
                }
                self.wrap
                    .iter()
                    .copied()
                    .find(|key| self.keys.len() == 1 || Some(*key) != anchor)
            })
    }

    pub fn next_on_end(&self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        if self.repeat == RepeatMode::One
            && (self.keys.contains(&current) || self.is_queued_current(current))
        {
            Some(current)
        } else {
            self.next(current, allow_wrap)
        }
    }

    pub fn previous(&self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        if self.history.get(self.cursor) == Some(&current)
            && self.cursor > 0
            && (self.shuffle
                || self.is_queued_current(current)
                || matches!(self.visits[self.cursor - 1], Visit::Queued { .. }))
        {
            return Some(self.history[self.cursor - 1]);
        }
        if self.shuffle {
            return None;
        }
        if self.is_queued_current(current) {
            return self.anchor(current);
        }
        let index = self.keys.iter().position(|key| *key == current)?;
        index
            .checked_sub(1)
            .and_then(|index| self.keys.get(index))
            .copied()
            .or_else(|| {
                (allow_wrap && self.repeat == RepeatMode::All)
                    .then(|| self.keys.last().copied())
                    .flatten()
            })
    }

    pub fn advance_next(
        &mut self,
        current: TrackKey,
        allow_wrap: bool,
        natural_end: bool,
    ) -> Option<TrackKey> {
        if natural_end && self.repeat == RepeatMode::One {
            return self.next_on_end(current, allow_wrap);
        }
        if self.history.get(self.cursor) == Some(&current)
            && let Some(&key) = self.history.get(self.cursor + 1)
        {
            self.cursor += 1;
            self.revision = self.revision.wrapping_add(1);
            if !self.is_queued_current(key) {
                self.mark_visited(key);
            }
            return Some(key);
        }
        if !self.queued.is_empty() {
            return self.start_queued(Some(current));
        }
        let key = self.next(current, allow_wrap)?;
        if key != current || self.is_queued_current(current) {
            self.commit_new(key);
        }
        Some(key)
    }

    pub fn advance_previous(&mut self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        let key = self.previous(current, allow_wrap)?;
        if self.history.get(self.cursor) == Some(&current)
            && self.cursor > 0
            && self.history.get(self.cursor - 1) == Some(&key)
        {
            self.cursor -= 1;
            self.revision = self.revision.wrapping_add(1);
            if !self.is_queued_current(key) {
                self.mark_visited(key);
            }
        } else if key != current {
            // A previous source row may not have been played yet. Insert it
            // before the cursor so another Previous keeps moving backwards.
            if self.history.get(self.cursor) != Some(&current) {
                self.record_played(current);
            }
            self.history.insert(self.cursor, key);
            self.visits.insert(self.cursor, Visit::Source);
            self.history.truncate(HISTORY_LIMIT);
            self.visits.truncate(HISTORY_LIMIT);
            self.revision = self.revision.wrapping_add(1);
            self.mark_visited(key);
        }
        Some(key)
    }

    fn commit_new(&mut self, key: TrackKey) {
        if self.shuffle && self.remaining.is_empty() && self.visited.contains(&key) {
            self.remaining = if self.wrap.is_empty() {
                let mut cycle = self.keys.clone();
                self.randomize(&mut cycle);
                cycle.into()
            } else {
                std::mem::take(&mut self.wrap).into()
            };
            self.visited.clear();
        }
        self.push_history(key, Visit::Source);
        self.mark_visited(key);
    }

    fn push_history(&mut self, key: TrackKey, visit: Visit) {
        let keep = if self.history.is_empty() {
            0
        } else {
            self.cursor + 1
        };
        self.history.truncate(keep);
        self.visits.truncate(keep);
        self.history.push(key);
        self.visits.push(visit);
        if self.history.len() > HISTORY_LIMIT {
            let excess = self.history.len() - HISTORY_LIMIT;
            self.history.drain(..excess);
            self.visits.drain(..excess);
        }
        self.cursor = self.history.len() - 1;
        self.revision = self.revision.wrapping_add(1);
    }

    fn mark_visited(&mut self, key: TrackKey) {
        self.visited.insert(key);
        self.remaining.retain(|candidate| *candidate != key);
        self.prepare_wrap(Some(key));
    }

    fn prepare_wrap(&mut self, current: Option<TrackKey>) {
        if !self.shuffle
            || !self.remaining.is_empty()
            || !self.wrap.is_empty()
            || self.keys.is_empty()
        {
            return;
        }
        let mut wrap = self.keys.clone();
        self.randomize(&mut wrap);
        if wrap.len() > 1 && wrap.first().copied() == current {
            wrap.swap(0, 1);
        }
        self.wrap = wrap;
    }

    fn randomize(&mut self, keys: &mut [TrackKey]) {
        if self.random == 0 {
            let mut seed = [0_u8; 8];
            let _ = getrandom::fill(&mut seed);
            self.random = match u64::from_le_bytes(seed) {
                0 => 0x9e37_79b9_7f4a_7c15,
                seed => seed,
            };
        }
        for end in (1..keys.len()).rev() {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            keys.swap(end, (self.random % (end as u64 + 1)) as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys(n: i64) -> Vec<TrackKey> {
        (1..=n).map(|id| (42, id)).collect()
    }
    fn shuffled(keys: &[TrackKey], current: Option<TrackKey>) -> PlaybackOrder {
        let mut order = PlaybackOrder {
            random: 7,
            ..Default::default()
        };
        order.set_shuffle(true, keys, current);
        order
    }

    #[test]
    fn shuffle_visits_every_track_once_and_peeking_does_not_consume_a_candidate() {
        let keys = keys(40);
        let untouched = keys.clone();
        let mut order = shuffled(&keys, Some(keys[0]));
        let mut current = keys[0];
        let mut played = HashSet::from([current]);
        while let Some(next) = order.next(current, false) {
            assert_eq!(order.next(current, false), Some(next));
            assert_eq!(order.advance_next(current, false, false), Some(next));
            order.record_played(next);
            assert!(played.insert(next));
            current = next;
        }
        assert_eq!(played.len(), keys.len());
        assert_eq!(keys, untouched);
    }

    #[test]
    fn shuffled_previous_reverses_actual_history_and_next_retraces_it() {
        let keys = keys(8);
        let mut order = shuffled(&keys, Some(keys[0]));
        let first = keys[0];
        let second = order.advance_next(first, false, false).unwrap();
        let third = order.advance_next(second, false, false).unwrap();
        assert_eq!(order.advance_previous(third, false), Some(second));
        order.record_played(second);
        assert_eq!(order.previous(second, false), Some(first));
        assert_eq!(order.advance_next(second, false, false), Some(third));
    }

    #[test]
    fn first_is_a_stable_shuffle_candidate_without_consuming_or_reordering_source() {
        let keys = keys(12);
        let mut order = shuffled(&keys, None);
        let first = order.first().unwrap();
        let queued = order.remaining.clone();
        assert_eq!(order.first(), Some(first));
        assert_eq!(order.remaining, queued);
        order.sync(&keys, Some(first));
        order.record_played(first);
        assert_eq!(order.history, [first]);
        assert!(!order.remaining.contains(&first));
        let next = order.next(first, false);
        order.set_shuffle(true, &keys, Some(first));
        assert_eq!(order.next(first, false), next);
        order.set_shuffle(false, &keys, Some(first));
        assert_eq!(order.first(), Some(keys[0]));
    }

    #[test]
    fn new_pages_add_unplayed_tracks_without_losing_history_or_resetting_the_bag() {
        let initial = keys(3);
        let mut order = shuffled(&initial, Some(initial[0]));
        let second = order.advance_next(initial[0], false, false).unwrap();
        let third = order.next(second, false).unwrap();
        let complete = keys(5);
        order.sync(&complete, Some(second));
        assert_eq!(order.next(second, false), Some(third));
        assert_eq!(order.previous(second, false), Some(initial[0]));
        let mut played = HashSet::from([initial[0], second]);
        let mut current = second;
        while let Some(next) = order.advance_next(current, false, false) {
            assert!(played.insert(next));
            current = next;
        }
        assert_eq!(played.len(), 5);
    }

    #[test]
    fn repeat_one_only_applies_to_natural_end_and_wrap_waits_for_pending_pages() {
        let keys = keys(2);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        assert_eq!(order.cycle_repeat(), RepeatMode::All);
        assert_eq!(order.next(keys[1], false), None);
        assert_eq!(order.next(keys[1], true), Some(keys[0]));
        assert_eq!(order.cycle_repeat(), RepeatMode::One);
        assert_eq!(order.next_on_end(keys[0], false), Some(keys[0]));
        assert_eq!(order.next(keys[0], false), Some(keys[1]));
        assert_eq!(order.next(keys[1], true), None);
        assert_eq!(order.cycle_repeat(), RepeatMode::Off);
    }

    #[test]
    fn repeat_all_wrap_collision_with_previous_history_is_still_a_forward_step() {
        let keys = keys(3);
        let mut order = shuffled(&keys, Some(keys[0]));
        let second = order.advance_next(keys[0], false, false).unwrap();
        let last = order.advance_next(second, false, false).unwrap();
        order.cycle_repeat();
        order.wrap = vec![second, last, keys[0]];
        assert_eq!(order.advance_next(last, true, false), Some(second));
        order.record_played(second);
        assert_eq!(order.previous(second, false), Some(last));
        assert_eq!(order.remaining.len(), 2);
    }

    #[test]
    fn empty_single_removed_tracks_and_mode_toggles_stay_bounded() {
        let keys = keys(4);
        let mut order = shuffled(&keys, Some(keys[0]));
        order.record_played(keys[1]);
        order.set_shuffle(false, &keys, Some(keys[1]));
        assert!(!order.shuffle());
        assert_eq!(order.next(keys[1], false), Some(keys[2]));
        order.set_shuffle(true, &keys, Some(keys[1]));
        assert_eq!(order.history[order.cursor], keys[1]);
        assert!(order.next(keys[1], false).is_some());
        order.sync(&[keys[1]], Some(keys[1]));
        assert_eq!(order.next(keys[1], false), None);
        order.cycle_repeat();
        assert_eq!(order.next(keys[1], true), Some(keys[1]));
        order.reset_source(&[], None);
        assert!(order.shuffle());
        assert_eq!(order.repeat(), RepeatMode::All);
        assert!(order.next(keys[1], true).is_none());
        assert!(order.history.is_empty() && order.remaining.is_empty());
    }

    #[test]
    fn explicit_queue_preserves_duplicates_and_returns_to_its_source_anchor() {
        let keys = keys(3);
        let external = (99, 7);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        for key in [external, external, keys[2]] {
            assert!(order.enqueue(key));
        }
        let expected = [external, external, keys[2], keys[1], keys[2]];
        assert_eq!(
            order
                .upcoming(Some(keys[0]), false)
                .iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            expected
        );
        assert_ne!(order.queued()[0].id, order.queued()[1].id);
        let mut current = keys[0];
        for key in expected {
            assert_eq!(order.next(current, false), Some(key));
            assert_eq!(order.advance_next(current, false, false), Some(key));
            order.sync(&keys, Some(key));
            order.record_played(key);
            current = key;
        }
        assert_eq!(order.next(current, false), None);
        assert!(order.queued().is_empty());
        assert_eq!(order.advance_previous(current, false), Some(keys[1]));
        assert_eq!(order.advance_previous(keys[1], false), Some(keys[2]));
        assert!(order.is_queued_current(keys[2]));
        assert_eq!(order.advance_previous(keys[2], false), Some(external));
        assert_eq!(order.advance_next(external, false, false), Some(keys[2]));
        assert_eq!(order.advance_next(keys[2], false, false), Some(keys[1]));
    }

    #[test]
    fn previous_retraces_played_entries_before_consuming_new_queued_tracks() {
        let keys = keys(3);
        let external = (99, 7);
        let later = (99, 8);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        order.enqueue(external);
        order.enqueue(later);
        assert_eq!(order.advance_next(keys[0], false, false), Some(external));
        assert_eq!(order.advance_previous(external, false), Some(keys[0]));
        assert_eq!(
            order
                .upcoming(Some(keys[0]), false)
                .iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            [external, later, keys[1], keys[2]]
        );
        assert_eq!(order.advance_next(keys[0], false, false), Some(external));
        assert_eq!(order.queued().len(), 1);
        assert_eq!(order.advance_next(external, false, false), Some(later));
    }

    #[test]
    fn queued_tracks_do_not_consume_shuffle_candidates_and_previews_never_reorder_them() {
        let keys = keys(20);
        let mut order = shuffled(&keys, Some(keys[0]));
        let original = order.upcoming(Some(keys[0]), false);
        let next = original[0].key;
        order.enqueue((99, 7));
        order.enqueue(next);
        let preview = order.upcoming(Some(keys[0]), false);
        assert_eq!(&preview[2..], original.as_slice());
        let bag = order.remaining.clone();
        assert_eq!(order.upcoming(Some(keys[0]), false), preview);
        assert_eq!(order.remaining, bag);
        assert_eq!(order.advance_next(keys[0], false, false), Some((99, 7)));
        order.sync(&keys, Some((99, 7)));
        assert_eq!(order.advance_next((99, 7), false, false), Some(next));
        order.sync(&keys, Some(next));
        order.record_played(next);
        assert_eq!(order.remaining, bag);
        assert_eq!(order.advance_next(next, false, false), Some(next));
        assert!(!order.is_queued_current(next));
        assert!(!order.remaining.contains(&next));
    }

    #[test]
    fn repeat_one_lists_the_current_track_next_and_its_row_replays_it() {
        let keys = keys(3);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        let before: Vec<_> = order
            .upcoming(Some(keys[0]), true)
            .iter()
            .map(|row| row.key)
            .collect();
        order.cycle_repeat();
        order.cycle_repeat();
        let upcoming: Vec<_> = order
            .upcoming(Some(keys[0]), true)
            .iter()
            .map(|row| row.key)
            .collect();
        assert_eq!(upcoming[0], keys[0]);
        assert_eq!(&upcoming[1..], &before[..]);
        assert_eq!(order.repeats(Some(keys[0])), Some(keys[0]));
        assert_eq!(order.jump_to(0, Some(keys[0]), true), Some(keys[0]));
        assert_eq!(order.jump_to(2, Some(keys[0]), true), Some(keys[2]));
    }

    #[test]
    fn repeat_one_keeps_the_queue_and_repeat_all_resumes_the_source_cycle() {
        let keys = keys(2);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        order.enqueue((99, 7));
        order.cycle_repeat();
        order.cycle_repeat();
        assert_eq!(order.advance_next(keys[0], true, true), Some(keys[0]));
        assert_eq!(order.queued().len(), 1);
        assert_eq!(order.advance_next(keys[0], true, false), Some((99, 7)));
        assert_eq!(order.advance_next((99, 7), true, true), Some((99, 7)));
        assert_eq!(order.advance_next((99, 7), true, false), Some(keys[1]));
        order.cycle_repeat();
        order.cycle_repeat();
        assert_eq!(order.advance_next(keys[1], true, true), Some(keys[0]));
    }

    #[test]
    fn queue_removal_source_changes_and_unavailable_recovery_keep_exact_identities() {
        let keys = keys(3);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        order.enqueue((99, 7));
        order.enqueue((99, 7));
        order.enqueue((99, 8));
        let id = order.queued()[1].id;
        order.remove_queued(id);
        assert_eq!(
            order
                .queued()
                .iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            [(99, 7), (99, 8)]
        );
        order.reset_source(&keys[1..], None);
        order.select_source(keys[1]);
        assert_eq!(order.advance_next(keys[1], false, false), Some((99, 7)));
        order.enqueue((99, 7));
        order.exclude_queued(&HashSet::from([(99, 7)]));
        order.sync(&keys[1..], Some((99, 7)));
        assert_eq!(order.advance_next((99, 7), false, false), Some((99, 8)));
        assert_eq!(order.advance_next((99, 8), false, false), Some(keys[2]));
        order.enqueue((99, 9));
        order.clear_queued();
        assert!(order.queued().is_empty());
        assert_eq!(order.next(keys[2], false), None);
    }

    #[test]
    fn queue_can_start_before_a_source_and_previous_source_navigation_does_not_bounce() {
        let keys = keys(4);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, None);
        order.enqueue((99, 7));
        assert_eq!(order.start_queued(None), Some((99, 7)));
        assert_eq!(order.advance_next((99, 7), false, false), Some(keys[0]));
        order.select_source(keys[3]);
        assert_eq!(order.advance_previous(keys[3], false), Some(keys[2]));
        assert_eq!(order.advance_previous(keys[2], false), Some(keys[1]));
        assert_eq!(order.advance_next(keys[1], false, false), Some(keys[2]));
    }

    #[test]
    fn jumping_in_the_drawer_consumes_skipped_entries_without_inventing_history() {
        let keys = keys(8);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[0]));
        order.enqueue((99, 7));
        order.enqueue((99, 8));
        order.enqueue((99, 9));
        assert_eq!(order.jump_to(1, Some(keys[0]), false), Some((99, 8)));
        assert_eq!(order.history, [keys[0], (99, 8)]);
        assert_eq!(order.queued().len(), 1);
        assert_eq!(order.advance_previous((99, 8), false), Some(keys[0]));
        assert_eq!(order.advance_next(keys[0], false, false), Some((99, 8)));
        assert_eq!(order.next((99, 8), false), Some((99, 9)));
        let mut order = shuffled(&keys, Some(keys[0]));
        order.enqueue((99, 7));
        let preview = order.upcoming(Some(keys[0]), false);
        let target = preview[4].key;
        assert_eq!(order.jump_to(4, Some(keys[0]), false), Some(target));
        assert_eq!(order.history, [keys[0], target]);
        assert!(order.queued().is_empty());
        assert_eq!(order.upcoming(Some(target), false), preview[5..]);
    }

    #[test]
    fn source_refresh_and_shuffle_toggle_during_a_queued_track_preserve_continuation() {
        let keys = keys(4);
        let mut order = PlaybackOrder::default();
        order.sync(&keys, Some(keys[1]));
        order.enqueue((99, 7));
        order.advance_next(keys[1], false, false).unwrap();
        order.sync(&[keys[0], keys[2], keys[3]], Some((99, 7)));
        assert_eq!(order.next((99, 7), false), Some(keys[2]));
        order.set_shuffle(true, &[keys[0], keys[2], keys[3]], Some((99, 7)));
        assert!(!order.remaining.contains(&keys[0]));
        assert_eq!(order.remaining.len(), 2);
    }
}
