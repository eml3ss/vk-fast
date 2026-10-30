//! Playback order is separate from the displayed library. Probes never consume
//! a shuffle candidate; explicit advances commit navigation before playback.
use std::collections::{HashSet, VecDeque};

pub type TrackKey = (i64, i64);
const HISTORY_LIMIT: usize = 50_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

#[derive(Default)]
pub struct PlaybackOrder {
    shuffle: bool,
    repeat: RepeatMode,
    keys: Vec<TrackKey>,
    remaining: VecDeque<TrackKey>,
    visited: HashSet<TrackKey>,
    history: Vec<TrackKey>,
    cursor: usize,
    wrap: Vec<TrackKey>,
    random: u64,
}

impl PlaybackOrder {
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn remaining_after(&self, current: TrackKey) -> usize {
        if !self.shuffle {
            return self
                .keys
                .iter()
                .position(|key| *key == current)
                .map_or(0, |index| self.keys.len() - index - 1);
        }
        let forward = if self.history.get(self.cursor) == Some(&current) {
            self.history.len() - self.cursor - 1
        } else {
            0
        };
        forward + self.remaining.iter().filter(|key| **key != current).count()
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
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.repeat
    }

    pub fn set_shuffle(&mut self, enabled: bool, keys: &[TrackKey], current: Option<TrackKey>) {
        if enabled != self.shuffle {
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
        self.keys = keys;
        self.visited.retain(|key| available.contains(key));
        self.remaining.retain(|key| available.contains(key));
        if changed {
            let mut cursor = 0;
            let mut history = Vec::with_capacity(self.history.len());
            for (index, key) in self.history.iter().copied().enumerate() {
                if available.contains(&key) {
                    if index <= self.cursor {
                        cursor = history.len();
                    }
                    history.push(key);
                }
            }
            self.history = history;
            self.cursor = cursor.min(self.history.len().saturating_sub(1));
            self.wrap.clear();
        }
        if let Some(current) = current.filter(|key| available.contains(key)) {
            self.record_played(current);
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
        let index = self.keys.iter().position(|key| *key == current)?;
        if !self.shuffle {
            return self.keys.get(index + 1).copied().or_else(|| {
                (allow_wrap && self.repeat == RepeatMode::All)
                    .then(|| self.keys.first().copied())
                    .flatten()
            });
        }
        if self.history.get(self.cursor) == Some(&current)
            && let Some(key) = self.history.get(self.cursor + 1)
        {
            return Some(*key);
        }
        self.remaining
            .iter()
            .copied()
            .find(|key| *key != current)
            .or_else(|| {
                if !allow_wrap || self.repeat != RepeatMode::All {
                    return None;
                }
                self.wrap
                    .iter()
                    .copied()
                    .find(|key| self.keys.len() == 1 || *key != current)
            })
    }

    pub fn next_on_end(&self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        if self.repeat == RepeatMode::One && self.keys.contains(&current) {
            Some(current)
        } else {
            self.next(current, allow_wrap)
        }
    }

    pub fn previous(&self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        let index = self.keys.iter().position(|key| *key == current)?;
        if self.shuffle {
            return (self.history.get(self.cursor) == Some(&current) && self.cursor > 0)
                .then(|| self.history[self.cursor - 1]);
        }
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
        let key = if natural_end {
            self.next_on_end(current, allow_wrap)
        } else {
            self.next(current, allow_wrap)
        }?;
        if key == current {
            return Some(key);
        }
        if self.shuffle
            && self.history.get(self.cursor) == Some(&current)
            && self.history.get(self.cursor + 1) == Some(&key)
        {
            self.cursor += 1;
            self.mark_visited(key);
        } else {
            self.commit_new(key);
        }
        Some(key)
    }

    pub fn advance_previous(&mut self, current: TrackKey, allow_wrap: bool) -> Option<TrackKey> {
        let key = self.previous(current, allow_wrap)?;
        if self.shuffle {
            self.cursor -= 1;
            self.mark_visited(key);
        } else if key != current {
            self.commit_new(key);
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
        self.history.truncate(if self.history.is_empty() {
            0
        } else {
            self.cursor + 1
        });
        self.history.push(key);
        if self.history.len() > HISTORY_LIMIT {
            self.history.drain(..self.history.len() - HISTORY_LIMIT);
        }
        self.cursor = self.history.len() - 1;
        self.mark_visited(key);
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
}
