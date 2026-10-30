use std::collections::HashMap;
use std::convert::From;
use std::convert::TryFrom;
use std::convert::TryInto;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use zbus::connection;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zvariant::{ObjectPath, Value};

use crate::{
    MediaControlEvent, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection,
};

use super::Error;

/// A handle to OS media controls.
pub struct MediaControls {
    thread: Option<ServiceThreadHandle>,
    dbus_name: String,
    friendly_name: String,
}

struct ServiceThreadHandle {
    event_channel: mpsc::Sender<InternalEvent>,
    thread: JoinHandle<()>,
}

#[derive(Clone, PartialEq, Debug)]
enum InternalEvent {
    ChangeMetadata(OwnedMetadata),
    ChangePlayback(MediaPlayback),
    SeekPlayback(MediaPlayback),
    ChangeVolume(f64),
    Kill,
}

#[derive(Clone, Debug)]
struct ServiceState {
    metadata: OwnedMetadata,
    playback_status: MediaPlayback,
    volume: f64,
    playback_updated_at: Instant,
    track_changed: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct OwnedMetadata {
    pub track_id: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub cover_url: Option<String>,
    pub duration: Option<i64>,
}

impl From<MediaMetadata<'_>> for OwnedMetadata {
    fn from(other: MediaMetadata) -> Self {
        OwnedMetadata {
            track_id: other.track_id.map(str::to_owned),
            title: other.title.map(|s| s.to_string()),
            artist: other.artist.map(|s| s.to_string()),
            album: other.album.map(|s| s.to_string()),
            cover_url: other.cover_url.map(|s| s.to_string()),
            duration: other.duration.map(|d| d.as_micros().try_into().unwrap()),
        }
    }
}

impl MediaControls {
    /// Create media controls with the specified config.
    pub fn new(config: PlatformConfig) -> Result<Self, Error> {
        let PlatformConfig {
            dbus_name,
            display_name,
            ..
        } = config;

        Ok(Self {
            thread: None,
            dbus_name: dbus_name.to_string(),
            friendly_name: display_name.to_string(),
        })
    }

    /// Attach the media control events to a handler.
    pub fn attach<F>(&mut self, event_handler: F) -> Result<(), Error>
    where
        F: Fn(MediaControlEvent) + Send + 'static,
    {
        self.detach()?;

        let dbus_name = self.dbus_name.clone();
        let friendly_name = self.friendly_name.clone();
        let event_handler = Arc::new(Mutex::new(event_handler));
        let (event_channel, rx) = mpsc::channel();

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            // Surface initialization failures without a detached panic.
            let _ = pollster::block_on(run_service(
                dbus_name,
                friendly_name,
                event_handler,
                rx,
                ready_tx,
            ));
        });
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => {
                self.thread = Some(ServiceThreadHandle {
                    event_channel,
                    thread,
                });
                Ok(())
            }
            result => {
                // If connecting finishes later, the queued Kill releases the
                // service. Never block application startup on an absent bus.
                let _ = event_channel.send(InternalEvent::Kill);
                drop(thread);
                let message = match result {
                    Ok(Err(message)) => message,
                    Err(error) => error.to_string(),
                    Ok(Ok(())) => unreachable!(),
                };
                Err(Error::Startup(message))
            }
        }
    }
    /// Detach the event handler.
    pub fn detach(&mut self) -> Result<(), Error> {
        if let Some(ServiceThreadHandle {
            event_channel,
            thread,
        }) = self.thread.take()
        {
            event_channel.send(InternalEvent::Kill).ok();
            thread.join().map_err(|_| Error::ThreadPanicked)?;
        }
        Ok(())
    }

    /// Set the current playback status.
    pub fn set_playback(&mut self, playback: MediaPlayback) -> Result<(), Error> {
        self.send_internal_event(InternalEvent::ChangePlayback(playback))?;
        Ok(())
    }

    /// Explicitly acknowledge a committed seek, including sub-second seeks.
    pub fn set_playback_seeked(&mut self, playback: MediaPlayback) -> Result<(), Error> {
        self.send_internal_event(InternalEvent::SeekPlayback(playback))
    }

    /// Set the metadata of the currently playing media item.
    pub fn set_metadata(&mut self, metadata: MediaMetadata) -> Result<(), Error> {
        self.send_internal_event(InternalEvent::ChangeMetadata(metadata.into()))?;
        Ok(())
    }

    /// Set the volume level (0.0 - 1.0) (Only available on MPRIS)
    pub fn set_volume(&mut self, volume: f64) -> Result<(), Error> {
        self.send_internal_event(InternalEvent::ChangeVolume(volume))?;
        Ok(())
    }

    fn send_internal_event(&mut self, event: InternalEvent) -> Result<(), Error> {
        let channel = &self
            .thread
            .as_ref()
            .ok_or(Error::ThreadNotRunning)?
            .event_channel;
        channel.send(event).map_err(|_| Error::ThreadPanicked)
    }
}

struct AppInterface {
    friendly_name: String,
    desktop_entry: String,
    event_handler: Arc<Mutex<dyn Fn(MediaControlEvent) + Send + 'static>>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl AppInterface {
    fn raise(&self) {
        self.send_event(MediaControlEvent::Raise);
    }
    fn quit(&self) {
        self.send_event(MediaControlEvent::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_tracklist(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        &self.friendly_name
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        &self.desktop_entry
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> &[&str] {
        &[]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> &[&str] {
        &[]
    }
}

impl AppInterface {
    fn send_event(&self, event: MediaControlEvent) {
        (self.event_handler.lock().unwrap())(event);
    }
}

struct PlayerInterface {
    state: ServiceState,
    event_handler: Arc<Mutex<dyn Fn(MediaControlEvent) + Send + 'static>>,
}

impl PlayerInterface {
    fn send_event(&self, event: MediaControlEvent) {
        (self.event_handler.lock().unwrap())(event);
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerInterface {
    fn next(&self) {
        self.send_event(MediaControlEvent::Next);
    }
    fn previous(&self) {
        self.send_event(MediaControlEvent::Previous);
    }
    fn pause(&self) {
        self.send_event(MediaControlEvent::Pause);
    }
    fn play_pause(&self) {
        self.send_event(MediaControlEvent::Toggle);
    }
    fn stop(&self) {
        self.send_event(MediaControlEvent::Stop);
    }
    fn play(&self) {
        self.send_event(MediaControlEvent::Play);
    }

    fn seek(&self, offset: i64) {
        let abs_offset = offset.unsigned_abs();
        let direction = if offset > 0 {
            SeekDirection::Forward
        } else {
            SeekDirection::Backward
        };

        self.send_event(MediaControlEvent::SeekBy(
            direction,
            Duration::from_micros(abs_offset),
        ));

        // NOTE: Should the `Seeked` signal be called when calling this method?
    }

    fn set_position(&self, track_id: zvariant::ObjectPath, position: i64) {
        if self.state.metadata.track_id.as_deref() != Some(track_id.as_str()) {
            return;
        }
        if let Ok(micros) = position.try_into() {
            if let Some(duration) = self.state.metadata.duration {
                // If the Position argument is greater than the track length, do nothing.
                if position > duration {
                    return;
                }
            }

            let position = Duration::from_micros(micros);
            self.send_event(MediaControlEvent::SetPositionForTrack {
                track_id: track_id.to_string(),
                position: MediaPosition(position),
            });
        }
    }

    fn open_uri(&self, uri: String) {
        // NOTE: we should check if the URI is in the `SupportedUriSchemes` list.
        self.send_event(MediaControlEvent::OpenUri(uri));
    }

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        match self.state.playback_status {
            MediaPlayback::Playing { .. } => "Playing",
            MediaPlayback::Paused { .. } => "Paused",
            MediaPlayback::Stopped => "Stopped",
        }
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<&str, Value<'_>> {
        // TODO: this should be stored in a cache inside the state.
        let mut dict = HashMap::<&str, Value>::new();

        let OwnedMetadata {
            ref track_id,
            ref title,
            ref album,
            ref artist,
            ref cover_url,
            ref duration,
        } = self.state.metadata;

        // MPRIS
        dict.insert(
            "mpris:trackid",
            Value::new(
                ObjectPath::try_from(
                    track_id
                        .as_deref()
                        .unwrap_or("/org/mpris/MediaPlayer2/TrackList/NoTrack"),
                )
                .unwrap_or_else(|_| {
                    ObjectPath::try_from("/org/mpris/MediaPlayer2/TrackList/NoTrack").unwrap()
                }),
            ),
        );

        if let Some(length) = duration {
            dict.insert("mpris:length", Value::new(*length));
        }

        if let Some(cover_url) = cover_url {
            dict.insert("mpris:artUrl", Value::new(cover_url.clone()));
        }

        // Xesam
        if let Some(title) = title {
            dict.insert("xesam:title", Value::new(title.clone()));
        }
        if let Some(artist) = artist {
            dict.insert("xesam:artist", Value::new(vec![artist.clone()]));
        }
        if let Some(album) = album {
            dict.insert("xesam:album", Value::new(album.clone()));
        }
        dict
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.state.volume
    }

    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        if !volume.is_finite() {
            return;
        }
        self.state.volume = volume.clamp(0.0, 1.0);
        self.send_event(MediaControlEvent::SetVolume(self.state.volume));
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        let position = match self.state.playback_status {
            MediaPlayback::Playing {
                progress: Some(pos),
            } => pos
                .0
                .saturating_add(self.state.playback_updated_at.elapsed()),
            MediaPlayback::Paused {
                progress: Some(pos),
            } => pos.0,
            _ => Duration::ZERO,
        };
        let position = self
            .state
            .metadata
            .duration
            .map_or(position.as_micros(), |length| {
                position.as_micros().min(length.max(0) as u128)
            });
        position.try_into().unwrap_or(i64::MAX)
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.state.metadata.track_id.is_some()
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.state.metadata.track_id.is_some()
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.state.metadata.track_id.is_some()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.state.metadata.track_id.is_some()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.state.metadata.track_id.is_some()
            && self
                .state
                .metadata
                .duration
                .is_some_and(|length| length > 0)
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

async fn run_service(
    dbus_name: String,
    friendly_name: String,
    event_handler: Arc<Mutex<dyn Fn(MediaControlEvent) + Send + 'static>>,
    event_channel: mpsc::Receiver<InternalEvent>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> zbus::Result<()> {
    let app = AppInterface {
        friendly_name,
        desktop_entry: dbus_name.clone(),
        event_handler: event_handler.clone(),
    };

    let player = PlayerInterface {
        state: ServiceState {
            metadata: OwnedMetadata::default(),
            playback_status: MediaPlayback::Stopped,
            volume: 1.0,
            playback_updated_at: Instant::now(),
            track_changed: false,
        },
        event_handler,
    };

    let name = format!("org.mpris.MediaPlayer2.{dbus_name}");
    let path = ObjectPath::try_from("/org/mpris/MediaPlayer2")?;
    let connection = async {
        connection::Builder::session()?
            .serve_at(&path, app)?
            .serve_at(&path, player)?
            .name(name.as_str())?
            .build()
            .await
    }
    .await;
    let connection = match connection {
        Ok(connection) => {
            let _ = ready.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return Err(error);
        }
    };

    loop {
        if let Ok(event) = event_channel.recv_timeout(Duration::from_millis(10)) {
            if event == InternalEvent::Kill {
                break;
            }

            let interface_ref = connection
                .object_server()
                .interface::<_, PlayerInterface>(&path)
                .await?;
            let mut interface = interface_ref.get_mut().await;
            let ctxt = SignalEmitter::new(&connection, &path)?;

            let explicit_seek = matches!(&event, InternalEvent::SeekPlayback(_));
            match event {
                InternalEvent::ChangeMetadata(metadata) => {
                    interface.state.track_changed =
                        interface.state.metadata.track_id != metadata.track_id;
                    interface.state.metadata = metadata;
                    interface.metadata_changed(&ctxt).await?;
                }
                InternalEvent::ChangePlayback(playback) | InternalEvent::SeekPlayback(playback) => {
                    let old_status = interface.playback_status();
                    let old_position = interface.position();
                    interface.state.playback_status = playback;
                    interface.state.playback_updated_at = Instant::now();
                    let new_status = interface.playback_status();
                    let new_position = interface.position();
                    if old_status != new_status {
                        interface.playback_status_changed(&ctxt).await?;
                    }
                    if explicit_seek
                        || (old_status == new_status
                            && !interface.state.track_changed
                            && old_position.abs_diff(new_position) > 750_000)
                    {
                        PlayerInterface::seeked(&ctxt, new_position).await?;
                    }
                    interface.state.track_changed = false;
                }
                InternalEvent::ChangeVolume(volume) => {
                    interface.state.volume = volume;
                    interface.volume_changed(&ctxt).await?;
                }
                InternalEvent::Kill => (),
            }
        }
    }

    Ok(())
}
