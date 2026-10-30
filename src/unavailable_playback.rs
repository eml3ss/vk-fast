//! Bounded recovery from unavailable audio without replacing its source queue.
use super::*;

#[cfg(test)]
#[path = "unavailable_playback_tests.rs"]
mod tests;

type Key = (i64, i64);
const MAX_RECOVERY_STEPS: usize = 32;

#[derive(Default)]
pub(super) struct State {
    rejected: HashSet<Key>,
    steps: usize,
    halted: bool,
}

impl State {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(super) fn active(&self) -> bool {
        !self.rejected.is_empty()
    }
    pub(super) fn halted(&self) -> bool {
        self.halted
    }

    fn reject(&mut self, key: Key) -> bool {
        if self.halted || self.steps >= MAX_RECOVERY_STEPS || !self.rejected.insert(key) {
            self.halted = true;
            return false;
        }
        self.steps += 1;
        true
    }

    /// Root also uses this before fetching another all-restricted radio/library
    /// batch. Such a batch may not require another resolver invocation at all.
    pub(super) fn allow_continuation(&mut self) -> bool {
        if !self.active() {
            return true;
        }
        if self.halted || self.steps >= MAX_RECOVERY_STEPS {
            self.halted = true;
            return false;
        }
        self.steps += 1;
        true
    }
}

fn next_candidate(
    order: &mut PlaybackOrder,
    tracks: &[Track],
    anchor: Key,
    allow_wrap: bool,
    rejected: &HashSet<Key>,
) -> Option<Key> {
    // The failed anchor must stay until Next has consumed its position. Every
    // other known restriction is removed from the order, not from the queue.
    let keys: Vec<_> = tracks
        .iter()
        .filter(|track| {
            track.key() == anchor
                || (track.unavailable != Some(true) && !rejected.contains(&track.key()))
        })
        .map(Track::key)
        .collect();
    if keys.contains(&anchor) {
        order.sync(&keys, Some(anchor));
        order
            .advance_next(anchor, allow_wrap, false)
            .filter(|next| *next != anchor && !rejected.contains(next))
    } else {
        order.sync(&keys, None);
        order.first().filter(|next| !rejected.contains(next))
    }
}

impl MusicApp {
    pub(super) fn reject_unavailable_track(
        &mut self,
        ctx: &egui::Context,
        key: Key,
        learned: bool,
    ) {
        let newer_source_pending = self.home_autoplay.is_some()
            || self.playlist_controls.loading_play()
            || self
                .mix_request
                .as_ref()
                .is_some_and(|request| request.start);
        let failed = self
            .play_queue
            .iter()
            .flatten()
            .chain(&self.library.tracks)
            .chain(&self.recent)
            .chain(self.playing.iter())
            .find(|track| track.key() == key)
            .cloned();

        if learned {
            let mut identities = HashSet::from([key]);
            identities.extend(failed.as_ref().and_then(|track| track.release_key));
            // Saved copies and original-source keys are exact identities; names
            // and durations never stand in for an alias relationship.
            for (alias, owned) in &self.like_aliases {
                if identities.contains(alias) {
                    identities.insert(*owned);
                }
            }
            for (alias, owned) in &self.like_aliases {
                if identities.contains(owned) {
                    identities.insert(*alias);
                }
            }
            self.update_known_tracks(|track| {
                if identities.contains(&track.key())
                    || track
                        .release_key
                        .is_some_and(|key| identities.contains(&key))
                {
                    track.unavailable = Some(true);
                }
            });
            self.save_library(ctx);
            self.save_recent(ctx);
        }

        self.track_unavailable_notice(ctx);
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.playback_pending = None;
        self.player.stop();
        self.playback_error = None;
        self.seek_position = None;
        self.resume_position = None;
        self.time_slots = None;
        self.finished_handled = true;
        // A newer explicit source selection wins over an older resolver reply.
        if newer_source_pending {
            ctx.request_repaint();
            return;
        }
        self.advance_on_load = false;
        self.mix_advance = false;
        self.home_autoplay = None;
        self.selected = Some(key);
        if let Some(mut failed) = failed {
            if learned {
                failed.unavailable = Some(true);
            }
            self.playing = Some(failed);
        }

        if !self.unavailable_playback.reject(key) {
            self.stop_unavailable_recovery();
            ctx.request_repaint();
            return;
        }
        self.continue_unavailable_recovery(ctx);
    }

    /// Continue after a library/radio page arrives, without counting the same
    /// failed anchor as another resolver failure.
    pub(super) fn continue_unavailable_recovery(&mut self, ctx: &egui::Context) {
        if self.unavailable_playback.halted() {
            self.stop_unavailable_recovery();
            return;
        }
        if self.home_autoplay.is_some()
            || self.playlist_controls.loading_play()
            || self
                .mix_request
                .as_ref()
                .is_some_and(|request| request.start)
        {
            return;
        }
        let Some(key) = self.playing.as_ref().map(Track::key).or(self.selected) else {
            self.stop_unavailable_recovery();
            return;
        };
        let allow_wrap = self.allow_queue_wrap();
        let tracks = self.play_queue.as_deref().unwrap_or(&self.library.tracks);
        let next = next_candidate(
            &mut self.play_order,
            tracks,
            key,
            allow_wrap,
            &self.unavailable_playback.rejected,
        );
        let more_mix = self.mix_active
            && self.mix_has_more
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.music.is_some());
        let more_library = self.library_queue_active()
            && !self.library.exhausted
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.music.is_some() || session.client.is_some());
        if let Some(next) = next {
            self.play_track(ctx, next);
        } else if more_mix || more_library {
            if self.unavailable_playback.allow_continuation() {
                if more_mix {
                    self.mix_advance = true;
                    self.load_mix(ctx, false);
                } else {
                    self.advance_on_load = true;
                    if self.pending.is_none() {
                        self.load_page(ctx, false);
                    }
                }
            } else {
                self.stop_unavailable_recovery();
            }
        } else {
            self.stop_unavailable_recovery();
        }
        ctx.request_repaint();
    }

    pub(super) fn stop_unavailable_recovery(&mut self) {
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.playback_pending = None;
        self.player.stop();
        self.playback_error = None;
        self.seek_position = None;
        self.resume_position = None;
        self.time_slots = None;
        self.finished_handled = true;
        self.home_autoplay = None;
        self.advance_on_load = false;
        self.mix_advance = false;
        self.mix_request = None;
        self.unavailable_playback.halted = true;
    }
}
