//! MPRIS / Windows SMTC. Native callbacks only enqueue intents and wake egui;
//! the resident application remains the only owner of playback and account data.
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use crate::repaint_target::RepaintTarget;

const POSITION_INTERVAL: Duration = Duration::from_secs(1);
const SEEK_DISCONTINUITY: f64 = 0.75;
const MAX_DURATION: Duration = Duration::from_secs(u32::MAX as u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Stopped,
    Paused,
    Playing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackMetadata {
    /// Opaque, stable D-Bus object path; never a media URL or access key.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub track: Option<TrackMetadata>,
    pub status: Status,
    pub position: Duration,
    pub volume: f64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            track: None,
            status: Status::Stopped,
            position: Duration::ZERO,
            volume: 0.7,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    SeekBy(f64),
    SetPosition {
        position: Duration,
        track_id: Option<String>,
    },
    SetVolume(f64),
    Raise,
    Quit,
}

#[derive(Debug)]
pub struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<souvlaki::Error> for Error {
    fn from(error: souvlaki::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Default)]
struct PublishCache {
    track: Option<Option<TrackMetadata>>,
    status: Option<Status>,
    position: Duration,
    position_at: Option<Instant>,
    volume: Option<f64>,
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    metadata: bool,
    playback: bool,
    volume: bool,
}

impl PublishCache {
    fn plan(&self, snapshot: &Snapshot, now: Instant) -> Plan {
        let metadata = self.track.as_ref() != Some(&snapshot.track);
        let elapsed = self
            .position_at
            .map_or(Duration::MAX, |then| now.saturating_duration_since(then));
        let expected = self.position.as_secs_f64()
            + if self.status == Some(Status::Playing) {
                elapsed.as_secs_f64()
            } else {
                0.0
            };
        let position_changed = if snapshot.status == Status::Playing {
            elapsed >= POSITION_INTERVAL
                || (snapshot.position.as_secs_f64() - expected).abs() > SEEK_DISCONTINUITY
        } else {
            snapshot.position != self.position
        };
        Plan {
            metadata,
            playback: metadata || self.status != Some(snapshot.status) || position_changed,
            volume: self
                .volume
                .is_none_or(|volume| (volume - snapshot.volume).abs() > 0.000_001),
        }
    }
}

pub struct SystemMedia {
    controls: MediaControls,
    receiver: Receiver<Command>,
    sender: SyncSender<Command>,
    repaint: RepaintTarget,
    published_id: Arc<Mutex<Option<String>>>,
    cache: PublishCache,
    force_seek: bool,
}

impl SystemMedia {
    /// Must be called on the application's UI thread with its current HWND.
    /// Linux ignores the handle and registers org.mpris.MediaPlayer2.vk-fast.
    pub fn new(hwnd: Option<isize>, repaint: RepaintTarget) -> Result<Self, Error> {
        let (sender, receiver) = mpsc::sync_channel(128);
        let published_id = Arc::new(Mutex::new(None));
        let controls = create_controls(hwnd, &sender, &repaint, &published_id)?;
        Ok(Self {
            controls,
            receiver,
            sender,
            repaint,
            published_id,
            cache: PublishCache::default(),
            force_seek: false,
        })
    }

    pub fn drain_commands(&mut self) -> Vec<Command> {
        self.receiver.try_iter().collect()
    }

    /// Call after an actual committed UI/system seek, not while previewing a drag.
    pub fn position_changed(&mut self) {
        self.force_seek = true;
    }

    /// The MPRIS service survives renderer recreation; SMTC belongs to HWND.
    pub fn rebind_window(&mut self, hwnd: Option<isize>) -> Result<(), Error> {
        #[cfg(target_os = "windows")]
        {
            // HWND values can be recycled after native window recreation.
            // The former HWND may already be destroyed. Failure to disable
            // that retired controller must not prevent binding the new one.
            let _ = self.controls.detach();
            self.controls = create_controls(hwnd, &self.sender, &self.repaint, &self.published_id)?;
            self.cache = PublishCache::default();
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (hwnd, &self.sender, &self.repaint);
        Ok(())
    }

    /// Call from app logic. Unchanged paused states publish nothing; normal
    /// progress is capped at 1Hz while explicit seeks publish immediately.
    pub fn publish(&mut self, snapshot: &Snapshot) -> Result<(), Error> {
        let snapshot = normalize(snapshot)?;
        let now = Instant::now();
        let plan = self.cache.plan(&snapshot, now);
        if plan.metadata {
            let track = snapshot.track.as_ref();
            self.controls.set_metadata(MediaMetadata {
                track_id: track.map(|track| track.id.as_str()),
                title: Some(track.map_or("", |track| track.title.as_str())),
                artist: Some(track.map_or("", |track| track.artist.as_str())),
                album: Some(track.and_then(|track| track.album.as_deref()).unwrap_or("")),
                cover_url: track.and_then(|track| track.cover_url.as_deref()),
                duration: track.and_then(|track| track.duration),
            })?;
            if let Ok(mut id) = self.published_id.lock() {
                *id = track.map(|track| track.id.clone());
            }
            self.cache.track = Some(snapshot.track.clone());
        }
        if plan.playback || self.force_seek {
            let progress = Some(MediaPosition(snapshot.position));
            let playback = match snapshot.status {
                Status::Stopped => MediaPlayback::Stopped,
                Status::Paused => MediaPlayback::Paused { progress },
                Status::Playing => MediaPlayback::Playing { progress },
            };
            #[cfg(target_os = "linux")]
            if self.force_seek {
                self.controls.set_playback_seeked(playback)?;
            } else {
                self.controls.set_playback(playback)?;
            }
            #[cfg(not(target_os = "linux"))]
            self.controls.set_playback(playback)?;
            self.force_seek = false;
            self.cache.status = Some(snapshot.status);
            self.cache.position = snapshot.position;
            self.cache.position_at = Some(now);
        }
        if plan.volume {
            #[cfg(target_os = "linux")]
            self.controls.set_volume(snapshot.volume)?;
            self.cache.volume = Some(snapshot.volume);
        }
        Ok(())
    }
}

fn create_controls(
    hwnd: Option<isize>,
    sender: &SyncSender<Command>,
    repaint: &RepaintTarget,
    published_id: &Arc<Mutex<Option<String>>>,
) -> Result<MediaControls, Error> {
    #[cfg(target_os = "windows")]
    if hwnd.is_none_or(|hwnd| hwnd == 0) {
        return Err(Error(
            "Windows media controls need the current window handle".into(),
        ));
    }
    let mut controls = MediaControls::new(PlatformConfig {
        dbus_name: "vk-fast",
        display_name: "VK Fast",
        hwnd: hwnd
            .filter(|hwnd| *hwnd != 0)
            .map(|hwnd| hwnd as *mut std::ffi::c_void),
    })?;
    let sender = sender.clone();
    let repaint = repaint.clone();
    let published_id = published_id.clone();
    controls.attach(move |event| {
        let current_id = published_id.lock().ok().and_then(|id| id.clone());
        if let Some(command) = command_from_event(event, current_id) {
            let _ = sender.try_send(command);
            repaint.request_repaint();
        }
    })?;
    Ok(controls)
}

fn command_from_event(event: MediaControlEvent, current_id: Option<String>) -> Option<Command> {
    Some(match event {
        MediaControlEvent::Play => Command::Play,
        MediaControlEvent::Pause => Command::Pause,
        MediaControlEvent::Toggle => Command::Toggle,
        MediaControlEvent::Next => Command::Next,
        MediaControlEvent::Previous => Command::Previous,
        MediaControlEvent::Stop => Command::Stop,
        MediaControlEvent::Seek(direction) => Command::SeekBy(direction_sign(direction) * 5.0),
        MediaControlEvent::SeekBy(direction, duration) => {
            Command::SeekBy(direction_sign(direction) * duration.min(MAX_DURATION).as_secs_f64())
        }
        MediaControlEvent::SetPosition(position) => Command::SetPosition {
            position: position.0.min(MAX_DURATION),
            track_id: current_id,
        },
        MediaControlEvent::SetPositionForTrack { position, track_id } => Command::SetPosition {
            position: position.0.min(MAX_DURATION),
            track_id: Some(track_id),
        },
        MediaControlEvent::SetVolume(volume) if volume.is_finite() => {
            Command::SetVolume(volume.clamp(0.0, 1.0))
        }
        MediaControlEvent::Raise => Command::Raise,
        MediaControlEvent::Quit => Command::Quit,
        _ => return None,
    })
}

fn direction_sign(direction: SeekDirection) -> f64 {
    match direction {
        SeekDirection::Forward => 1.0,
        SeekDirection::Backward => -1.0,
    }
}

fn normalize(snapshot: &Snapshot) -> Result<Snapshot, Error> {
    let mut snapshot = snapshot.clone();
    if let Some(track) = &mut snapshot.track {
        if !valid_track_id(&track.id) {
            return Err(Error("Invalid system media track identity".into()));
        }
        track.cover_url = track
            .cover_url
            .as_deref()
            .and_then(crate::model::valid_cover_url);
        track.duration = track.duration.map(|duration| duration.min(MAX_DURATION));
        snapshot.position = track.duration.map_or(snapshot.position, |duration| {
            snapshot.position.min(duration)
        });
    }
    snapshot.position = snapshot.position.min(MAX_DURATION);
    snapshot.volume = if snapshot.volume.is_finite() {
        snapshot.volume.clamp(0.0, 1.0)
    } else {
        0.7
    };
    Ok(snapshot)
}

fn valid_track_id(id: &str) -> bool {
    id.len() <= 512
        && id.starts_with('/')
        && id != "/"
        && id[1..].split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            track: Some(TrackMetadata {
                id: "/org/vkfast/track/a1".into(),
                title: "Song - Slowed".into(),
                artist: "A, B".into(),
                album: Some("Release".into()),
                cover_url: Some("https://example.invalid/cover.jpg".into()),
                duration: Some(Duration::from_secs(200)),
            }),
            status: Status::Playing,
            position: Duration::from_secs(10),
            volume: 0.7,
        }
    }

    #[test]
    fn commands_preserve_seek_targets_and_clamp_only_finite_volume() {
        assert_eq!(
            command_from_event(
                MediaControlEvent::SetPositionForTrack {
                    track_id: "/old/track".into(),
                    position: MediaPosition(Duration::from_secs(3))
                },
                None
            ),
            Some(Command::SetPosition {
                position: Duration::from_secs(3),
                track_id: Some("/old/track".into())
            })
        );
        assert_eq!(
            command_from_event(
                MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(4))),
                Some("/current/track".into())
            ),
            Some(Command::SetPosition {
                position: Duration::from_secs(4),
                track_id: Some("/current/track".into())
            })
        );
        assert_eq!(
            command_from_event(
                MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(5)),
                None
            ),
            Some(Command::SeekBy(-5.0))
        );
        assert_eq!(
            command_from_event(MediaControlEvent::SetVolume(1.5), None),
            Some(Command::SetVolume(1.0))
        );
        assert_eq!(
            command_from_event(MediaControlEvent::SetVolume(f64::NAN), None),
            None
        );
        assert_eq!(
            command_from_event(
                MediaControlEvent::OpenUri("https://example.invalid/".into()),
                None
            ),
            None
        );
    }

    #[test]
    fn progress_is_throttled_but_track_switches_seeks_and_pause_publish_immediately() {
        let now = Instant::now();
        let snapshot = snapshot();
        let cache = PublishCache {
            track: Some(snapshot.track.clone()),
            status: Some(snapshot.status),
            position: snapshot.position,
            position_at: Some(now),
            volume: Some(0.7),
        };
        let mut updated = snapshot.clone();
        updated.position += Duration::from_millis(100);
        assert_eq!(
            cache.plan(&updated, now + Duration::from_millis(100)),
            Plan {
                metadata: false,
                playback: false,
                volume: false
            }
        );
        updated.position += Duration::from_secs(1);
        assert!(
            cache
                .plan(&updated, now + Duration::from_millis(1100))
                .playback
        );
        updated.position = Duration::from_secs(50);
        assert!(
            cache
                .plan(&updated, now + Duration::from_millis(200))
                .playback
        );
        updated.status = Status::Paused;
        assert!(cache.plan(&updated, now).playback);
        updated = snapshot.clone();
        updated.track.as_mut().unwrap().id = "/org/vkfast/track/b2".into();
        assert!(cache.plan(&updated, now).metadata && cache.plan(&updated, now).playback);
    }

    #[test]
    fn unchanged_paused_state_does_not_generate_dbus_traffic() {
        let now = Instant::now();
        let mut snapshot = snapshot();
        snapshot.status = Status::Paused;
        let cache = PublishCache {
            track: Some(snapshot.track.clone()),
            status: Some(snapshot.status),
            position: snapshot.position,
            position_at: Some(now),
            volume: Some(0.7),
        };
        assert_eq!(
            cache.plan(&snapshot, now + Duration::from_secs(60)),
            Plan {
                metadata: false,
                playback: false,
                volume: false
            }
        );
        snapshot.volume = 0.25;
        assert_eq!(
            cache.plan(&snapshot, now + Duration::from_secs(60)),
            Plan {
                metadata: false,
                playback: false,
                volume: true
            }
        );
    }

    #[test]
    fn protocol_metadata_has_bounded_positions_public_art_and_valid_opaque_paths() {
        let mut value = snapshot();
        value.position = Duration::from_secs(999);
        value.volume = -2.0;
        value.track.as_mut().unwrap().cover_url = Some("file:///private/token.txt".into());
        let normalized = normalize(&value).unwrap();
        assert_eq!(normalized.position, Duration::from_secs(200));
        assert_eq!(normalized.volume, 0.0);
        assert!(normalized.track.unwrap().cover_url.is_none());
        for id in ["audio-42_1", "/", "/track/id-with-hyphen", "/track//id"] {
            assert!(!valid_track_id(id));
        }
        assert!(valid_track_id("/org/vkfast/track/42_1"));
        value.track.as_mut().unwrap().id = "/bad/id?secret".into();
        assert!(normalize(&value).is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "system_media_dbus_tests.rs"]
mod dbus_tests;
