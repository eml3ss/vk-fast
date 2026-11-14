//! A track that fails to load or breaks off gets one more try with a fresh
//! link, and an automatic advance moves on past a track that keeps failing:
//! VK's servers fail a request now and then (a 500, a cut stream, a refused
//! link), and when the queue moves on by itself nobody is at the window to
//! press Play again.
use super::*;

#[cfg(test)]
#[path = "playback_retry_tests.rs"]
mod tests;

/// How long after a failure the track is tried again.
pub(super) const RETRY_DELAY: Duration = Duration::from_millis(1500);
/// Failing tracks an automatic advance skips in a row before it stops.
pub(super) const MAX_SKIPS: u8 = 3;

#[derive(Default)]
pub(super) struct State {
    /// The current track was reached by the queue itself, not by the user.
    automatic: bool,
    /// Set right before an automatic advance; taken by the track it starts.
    next_automatic: bool,
    /// The current track already had its one more try.
    retried: bool,
    /// Set right before that try starts; taken by it.
    retrying: bool,
    /// The current track's failure is dealt with: retried, skipped or shown.
    settled: bool,
    /// When to try again: the track and where to resume it.
    due: Option<((i64, i64), f64, Instant)>,
    /// Failing tracks skipped in a row; a track that plays resets it.
    skipped: u8,
}

impl State {
    /// A new track starts: what the flags set for it are taken here.
    pub(super) fn starting(&mut self) {
        let retrying = std::mem::take(&mut self.retrying);
        self.automatic = std::mem::take(&mut self.next_automatic);
        self.retried = retrying;
        self.settled = false;
        self.due = None;
        if !self.automatic && !retrying {
            self.skipped = 0;
        }
    }
    /// The queue moves on by itself: the track it starts is automatic.
    pub(super) fn advancing(&mut self, automatic: bool) {
        self.next_automatic = automatic;
    }
}

/// A resolve failure that may pass on its own: the network or VK's server.
fn transient(error: &UiError) -> bool {
    matches!(
        error,
        UiError::Music(
            WebAudioError::Network
                | WebAudioError::Timeout
                | WebAudioError::Temporary
                | WebAudioError::Http(500..=599)
        )
    )
}

impl MusicApp {
    /// Retries or skips the current track once it failed. Called with
    /// `poll_playback`, so it works without the window.
    pub(super) fn recover_failed_playback(&mut self, ctx: &egui::Context) {
        let snapshot = self.player.snapshot();
        if snapshot.state == PlaybackState::Playing {
            self.playback_retry.skipped = 0;
        }
        if let Some((key, position, at)) = self.playback_retry.due {
            let now = Instant::now();
            if now < at {
                ctx.request_repaint_after(at - now);
                return;
            }
            self.playback_retry.due = None;
            if self.playing.as_ref().map(Track::key) == Some(key) {
                crate::diagnostics::event("playback", "retry: fresh_link");
                self.playback_retry.next_automatic = self.playback_retry.automatic;
                self.playback_retry.retrying = true;
                self.play_track_at(ctx, key, position);
            }
            return;
        }
        if self.playback_retry.settled || self.playback_pending.is_some() {
            return;
        }
        let Some(key) = self.playing.as_ref().map(Track::key) else {
            return;
        };
        let position = match (&snapshot.state, &self.playback_error) {
            (PlaybackState::Failed(PlaybackError::Decoder), _) => snapshot.position,
            (_, Some(error)) if transient(error) => self.resume_position.unwrap_or(0.0),
            _ => return,
        };
        self.playback_retry.settled = true;
        if !self.playback_retry.retried {
            self.playback_retry.due = Some((key, position.max(0.0), Instant::now() + RETRY_DELAY));
            ctx.request_repaint_after(RETRY_DELAY);
        } else if self.playback_retry.automatic && self.playback_retry.skipped < MAX_SKIPS {
            self.playback_retry.skipped += 1;
            crate::diagnostics::event("playback", "retry_failed; skipping");
            self.playback_retry.advancing(true);
            self.advance_track(ctx, true);
            self.playback_retry.advancing(false);
        } else {
            crate::diagnostics::event("playback", "retry_failed; stopped");
        }
    }
}
