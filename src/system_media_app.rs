//! System transport commands use the same playback state as the UI, without
//! borrowing its keyboard focus or replacing the currently selected source.
use super::*;
use crate::system_media::{Command, Snapshot, Status, SystemMedia, TrackMetadata};
use sha2::{Digest, Sha256};

#[cfg(test)]
#[path = "system_media_app_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct State {
    service: Option<SystemMedia>,
    attempted: bool,
    hwnd: Option<isize>,
    window_dirty: bool,
}

impl State {
    pub(super) fn detach(&mut self) {
        self.service = None;
    }
    pub(super) fn window_recreated(&mut self) {
        self.window_dirty = true;
    }
    pub(super) fn position_changed(&mut self) {
        if let Some(service) = &mut self.service {
            service.position_changed();
        }
    }
}

fn native_hwnd(frame: &eframe::Frame) -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match frame.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = frame;
        None
    }
}

impl MusicApp {
    pub(super) fn poll_system_media(&mut self, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        if self.demo
            || (self.system_media.service.is_none()
                && ctx.input(|input| input.viewport().close_requested()))
        {
            return;
        }
        let hwnd = frame.and_then(native_hwnd);
        #[cfg(target_os = "windows")]
        if hwnd.is_none() {
            return;
        }
        let state = &mut self.system_media;
        let changed = cfg!(target_os = "windows") && state.hwnd != hwnd;
        if !state.attempted || state.window_dirty || changed {
            let result = if let Some(service) = &mut state.service {
                service.rebind_window(hwnd)
            } else {
                SystemMedia::new(hwnd, self.repaint_target.with_context(ctx))
                    .map(|service| state.service = Some(service))
            };
            state.attempted = true;
            state.hwnd = hwnd;
            state.window_dirty = false;
            if let Err(error) = result {
                eprintln!("VK Fast system media controls: {error}");
                state.service = None;
            }
        }
        let commands = self
            .system_media
            .service
            .as_mut()
            .map(SystemMedia::drain_commands)
            .unwrap_or_default();
        for command in commands {
            self.handle_system_media_command(ctx, command);
        }
    }

    pub(super) fn publish_system_media(&mut self) {
        if self.system_media.service.is_none() {
            return;
        }
        let snapshot = self.system_media_snapshot();
        if let Some(service) = &mut self.system_media.service
            && let Err(error) = service.publish(&snapshot)
        {
            eprintln!("VK Fast system media controls: {error}");
            self.system_media.detach();
        }
    }

    fn system_media_track(&self) -> Option<&Track> {
        self.playing.as_ref().or_else(|| {
            self.selected.and_then(|key| {
                self.play_queue
                    .iter()
                    .flatten()
                    .chain(&self.library.tracks)
                    .chain(&self.recent)
                    .find(|track| track.key() == key)
            })
        })
    }

    pub(super) fn system_media_snapshot(&self) -> Snapshot {
        let playback = self.playback_snapshot();
        let track = self.system_media_track();
        let position = if playback.position.is_finite() {
            playback.position.max(0.0)
        } else {
            0.0
        };
        let status = match playback.state {
            PlaybackState::Playing => Status::Playing,
            PlaybackState::Paused => Status::Paused,
            PlaybackState::Loading if track.is_some() => Status::Playing,
            _ => Status::Stopped,
        };
        Snapshot {
            track: track.map(|track| {
                let account = self
                    .session
                    .as_ref()
                    .map_or(0, |session| session.profile.id);
                let hash =
                    Sha256::digest(format!("{account}:{}:{}", track.owner_id, track.id).as_bytes());
                let id = format!("/org/vkfast/track/t{:x}", hash);
                let artist = if track.artists.is_empty() {
                    track.artist.clone()
                } else {
                    track
                        .artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                TrackMetadata {
                    id,
                    title: track.display_title().into_owned(),
                    artist,
                    album: track
                        .album
                        .as_ref()
                        .map(|album| album.title.clone())
                        .filter(|title| !title.is_empty()),
                    cover_url: track
                        .cover_url
                        .as_deref()
                        .and_then(crate::model::valid_cover_url),
                    duration: (track.duration > 0)
                        .then(|| Duration::from_secs(u64::from(track.duration))),
                }
            }),
            status,
            position: Duration::from_secs_f64(track.map_or(0.0, |track| {
                bounded_track_position(position, track.duration).min(f64::from(u32::MAX))
            })),
            volume: f64::from(playback.volume),
        }
    }

    pub(super) fn handle_system_media_command(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::Raise => self.show_application(ctx),
            Command::Quit => self.request_quit(ctx),
            Command::SetVolume(volume) => {
                if volume.is_finite() {
                    let previous = self.player.snapshot().volume;
                    let volume = volume.clamp(0.0, 1.0) as f32;
                    if previous != volume {
                        self.player.set_volume(volume);
                        self.volume_save_at = Some(Instant::now() + Duration::from_millis(500));
                    }
                }
            }
            _ if self.demo || self.session.is_none() => return,
            Command::Play => {
                if !matches!(
                    self.playback_snapshot().state,
                    PlaybackState::Playing | PlaybackState::Loading
                ) {
                    self.toggle_playback(ctx);
                }
            }
            Command::Pause => self.system_media_pause(),
            Command::Toggle => {
                if matches!(
                    self.playback_snapshot().state,
                    PlaybackState::Playing | PlaybackState::Loading
                ) {
                    self.system_media_pause();
                } else {
                    self.toggle_playback(ctx);
                }
            }
            Command::Stop => self.system_media_stop(),
            Command::Next => self.system_media_adjacent(ctx, true),
            Command::Previous => self.system_media_adjacent(ctx, false),
            Command::SeekBy(seconds) => {
                if seconds.is_finite() {
                    let snapshot = self.playback_snapshot();
                    let target = snapshot.position + seconds;
                    if target.is_finite() {
                        if let Some(track) = self.system_media_track()
                            && track.duration > 0
                            && target > f64::from(track.duration)
                        {
                            self.system_media_adjacent(ctx, true);
                        } else {
                            self.system_media_seek(target.max(0.0));
                        }
                    }
                }
            }
            Command::SetPosition { position, track_id } => {
                let snapshot = self.system_media_snapshot();
                if let Some(track) = snapshot.track
                    && track_id.as_ref().is_none_or(|id| *id == track.id)
                    && track.duration.is_none_or(|duration| position <= duration)
                {
                    self.system_media_seek(position.as_secs_f64());
                }
            }
        }
        ctx.request_repaint();
    }

    fn system_media_pause(&mut self) {
        let snapshot = self.playback_snapshot();
        if !matches!(
            snapshot.state,
            PlaybackState::Playing | PlaybackState::Loading
        ) {
            return;
        }
        self.cancel_delayed_advance();
        self.seek_position = None;
        if self.playback_pending.is_some() {
            if let Some(cancel) = self.playback_cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            self.playback_pending = None;
            self.player.stop();
            self.resume_position = Some(snapshot.position);
            self.finished_handled = false;
        } else {
            self.player.pause();
        }
    }

    fn system_media_stop(&mut self) {
        self.cancel_delayed_advance();
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.playback_pending = None;
        self.player.stop();
        self.playback_error = None;
        self.resume_position = None;
        self.seek_position = None;
        self.time_slots = None;
        self.finished_handled = true;
    }

    fn system_media_seek(&mut self, position: f64) {
        if !position.is_finite()
            || self
                .playing
                .as_ref()
                .is_none_or(|track| track.unavailable == Some(true))
        {
            return;
        }
        self.cancel_delayed_advance();
        self.seek_position = None;
        self.finished_handled = false;
        self.commit_playback_seek(position);
    }

    fn system_media_adjacent(&mut self, ctx: &egui::Context, next: bool) {
        let state = self.playback_snapshot().state;
        if matches!(state, PlaybackState::Playing | PlaybackState::Loading) {
            if next {
                self.next_track(ctx);
            } else {
                self.previous_track(ctx, false);
            }
            return;
        }
        self.cancel_delayed_advance();
        self.synchronize_order();
        let Some(current) = self.playing.as_ref().map(Track::key).or(self.selected) else {
            return;
        };
        let wrap = self.allow_queue_wrap();
        let target = if next {
            self.play_order.advance_next(current, wrap, false)
        } else {
            self.play_order.advance_previous(current, wrap)
        };
        let Some(track) = target.and_then(|key| {
            self.play_queue
                .iter()
                .flatten()
                .chain(&self.library.tracks)
                .chain(&self.recent)
                .find(|track| track.key() == key)
                .cloned()
        }) else {
            return;
        };
        self.system_media_stop();
        self.selected = Some(track.key());
        self.playing = Some(track);
        if matches!(state, PlaybackState::Paused) {
            self.resume_position = Some(0.0);
        }
    }
}
