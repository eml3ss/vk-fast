//! Rust media decoding feeds a bounded, seekable PCM window. The native
//! output callback never waits for network, decoding or a mutex.
#[path = "playback_normalization.rs"]
mod normalization;
pub(crate) use crate::audio_cache::completed_duration_plausible;
use crate::{
    audio_cache::{AudioCache, CacheKey, CachePin},
    audio_output::{self, CHANNELS, OutputStream, SAMPLE_RATE},
    i18n::Language,
};
use std::{
    collections::VecDeque,
    io::Read,
    mem::size_of,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use zeroize::Zeroizing;
const TICK: Duration = Duration::from_millis(20);
const DEFAULT_VOLUME: f32 = 0.7;
const BUFFER_SECONDS: usize = 15;
const HISTORY_SECONDS: usize = 15;
const MAX_PCM_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackError {
    Worker,
    Device,
    Decoder,
    InvalidUrl,
    CacheMiss,
}
impl PlaybackError {
    pub fn localized(&self, language: Language) -> String {
        let message = match self {
            Self::Worker => language.text(
                "Не удалось запустить аудиоплеер.",
                "Could not start the audio player.",
            ),
            Self::Device => {
                #[cfg(windows)]
                {
                    language.text(
                        "Не удалось открыть аудиовыход WASAPI.",
                        "Could not open the WASAPI audio output.",
                    )
                }
                #[cfg(not(windows))]
                {
                    language.text(
                        "Не удалось открыть аудиовыход PipeWire.",
                        "Could not open the PipeWire audio output.",
                    )
                }
            }
            Self::Decoder => language.text(
                "Не удалось декодировать этот трек. Попробуйте ещё раз или выберите другой.",
                "Could not decode this track. Try again or choose another track.",
            ),
            Self::InvalidUrl => language.text(
                "VK не вернул подходящую HTTPS-ссылку на аудио.",
                "VK did not return a suitable HTTPS audio URL.",
            ),
            Self::CacheMiss => language.text(
                "Этот участок трека ещё не сохранён. Подключитесь к VK для загрузки.",
                "This part of the track is not cached yet. Connect to VK to load it.",
            ),
        };
        message.into()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed(PlaybackError),
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: PlaybackState,
    pub position: f64,
    /// Absolute media time up to which complete PCM frames are ready to play.
    pub buffered_until: f64,
    pub volume: f32,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            position: 0.0,
            buffered_until: 0.0,
            volume: DEFAULT_VOLUME,
        }
    }
}
#[derive(Clone, Copy, Default)]
struct Epoch {
    source: u64,
    revision: u64,
}
#[derive(Default)]
struct Control {
    source: AtomicU64,
    revision: AtomicU64,
    decoder_starts: AtomicU64,
    underruns: Arc<AtomicU64>,
}
impl Control {
    fn new_source(&self) -> Epoch {
        Epoch {
            source: self.source.fetch_add(1, Ordering::SeqCst) + 1,
            revision: self.revision.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
    fn seek(&self) -> Epoch {
        Epoch {
            source: self.source.load(Ordering::SeqCst),
            revision: self.revision.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
    fn current_source(&self, epoch: Epoch) -> bool {
        self.source.load(Ordering::SeqCst) == epoch.source
    }
    fn current(&self, epoch: Epoch) -> bool {
        self.current_source(epoch) && self.revision.load(Ordering::SeqCst) == epoch.revision
    }
}
enum Command {
    Play {
        epoch: Epoch,
        source: Source,
        offset: f64,
    },
    Pause,
    Resume,
    Seek {
        epoch: Epoch,
        offset: f64,
    },
    Stop(Epoch),
    NormalizationChanged,
    Shutdown,
}
pub struct Player {
    commands: Sender<Command>,
    state: Arc<Mutex<Snapshot>>,
    volume: Arc<AtomicU32>,
    normalize: Arc<AtomicBool>,
    normalization_profiles: VecDeque<(CacheKey, Arc<normalization::Profile>)>,
    control: Arc<Control>,
    source_duration: Option<u32>,
    worker: Option<JoinHandle<()>>,
}
impl Player {
    #[cfg(test)]
    pub(crate) fn test_playing(duration: u32, position: f64) -> (Self, impl Send) {
        let (player, control) = Self::test_paused(duration, position);
        player.state.lock().unwrap().state = PlaybackState::Playing;
        (player, control)
    }

    #[cfg(test)]
    pub(crate) fn test_paused(duration: u32, position: f64) -> (Self, impl Send) {
        let (commands, receiver) = mpsc::channel();
        let control = Arc::new(Control::default());
        control.new_source();
        (
            Self {
                commands,
                state: Arc::new(Mutex::new(Snapshot {
                    state: PlaybackState::Paused,
                    position: seek_position(position, duration),
                    buffered_until: f64::from(duration),
                    volume: DEFAULT_VOLUME,
                })),
                volume: Arc::new(AtomicU32::new(DEFAULT_VOLUME.to_bits())),
                normalize: Arc::new(AtomicBool::new(false)),
                normalization_profiles: VecDeque::new(),
                control,
                source_duration: Some(duration),
                worker: None,
            },
            receiver,
        )
    }

    /// Creating a player does not open an audio output or start decoding.
    pub fn new() -> Self {
        Self::create(None)
    }
    pub fn with_cache(cache: AudioCache) -> Self {
        Self::create(Some(cache))
    }
    fn create(cache: Option<AudioCache>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let volume = Arc::new(AtomicU32::new(DEFAULT_VOLUME.to_bits()));
        let normalize = Arc::new(AtomicBool::new(false));
        let control = Arc::new(Control::default());
        let (ws, wv, wc) = (state.clone(), volume.clone(), control.clone());
        let worker = thread::Builder::new()
            .name("vk-audio".into())
            .spawn(move || controller(receiver, ws, wv, wc, cache))
            .ok();
        let player = Self {
            commands,
            state,
            volume,
            normalize,
            normalization_profiles: VecDeque::new(),
            control,
            source_duration: None,
            worker,
        };
        if player.worker.is_none() {
            player.update_failure(PlaybackError::Worker);
        }
        player
    }
    fn update_failure(&self, error: PlaybackError) {
        if let Ok(mut s) = self.state.lock() {
            s.state = PlaybackState::Failed(error);
        }
    }
    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            self.update_failure(PlaybackError::Worker);
        }
    }
    pub fn play(&mut self, url: String, duration: u32, offset: f64) {
        self.play_source(
            Source {
                url: Some(Zeroizing::new(url)),
                duration,
                key: None,
                loudness: normalization::Profile::new(self.normalize.clone()),
            },
            offset,
        );
    }
    pub fn play_cached(&mut self, key: CacheKey, url: Option<String>, offset: f64) {
        let profile = if let Some(index) = self
            .normalization_profiles
            .iter()
            .position(|(stored, _)| *stored == key)
        {
            self.normalization_profiles.remove(index).unwrap().1
        } else {
            normalization::Profile::new(self.normalize.clone())
        };
        self.normalization_profiles
            .push_front((key, profile.clone()));
        self.normalization_profiles.truncate(32);
        self.play_source(
            Source {
                url: url.map(Zeroizing::new),
                duration: key.duration,
                key: Some(key),
                loudness: profile,
            },
            offset,
        );
    }
    fn play_source(&mut self, source: Source, offset: f64) {
        self.source_duration = Some(source.duration);
        let epoch = self.control.new_source();
        if let Ok(mut s) = self.state.lock() {
            s.state = PlaybackState::Loading;
            s.position = seek_position(offset, source.duration);
            s.buffered_until = s.position;
        }
        self.send(Command::Play {
            epoch,
            source,
            offset,
        });
    }
    pub fn pause(&self) {
        self.send(Command::Pause);
    }
    pub fn resume(&self) {
        self.send(Command::Resume);
    }
    pub fn seek(&mut self, offset: f64) {
        let Some(duration) = self.source_duration else {
            return;
        };
        let offset = seek_position(offset, duration);
        // Suppress stale Ended while queued, without invalidating the decoder.
        let epoch = self.control.seek();
        if let Ok(mut s) = self.state.lock() {
            // A seek outside the PCM/disk window may have to stop its old pump
            // before the controller can publish again. Keep the requested
            // position visible during that interval, just as play_source does.
            // publish() rejects older revisions under this same mutex.
            s.position = offset;
            s.buffered_until = s.buffered_until.max(offset);
            if matches!(
                s.state,
                PlaybackState::Ended | PlaybackState::Stopped | PlaybackState::Failed(_)
            ) {
                s.state = PlaybackState::Loading;
            }
        }
        self.send(Command::Seek { epoch, offset });
    }
    pub fn stop(&mut self) {
        self.source_duration = None;
        let epoch = self.control.new_source();
        if let Ok(mut s) = self.state.lock() {
            s.state = PlaybackState::Stopped;
            s.position = 0.0;
            s.buffered_until = 0.0;
        }
        self.send(Command::Stop(epoch));
    }
    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(normal_volume(volume).to_bits(), Ordering::Relaxed);
    }
    pub fn set_normalize_volume(&self, enabled: bool) {
        if self.normalize.swap(enabled, Ordering::Relaxed) != enabled {
            self.send(Command::NormalizationChanged);
        }
    }
    pub fn normalize_volume(&self) -> bool {
        self.normalize.load(Ordering::Relaxed)
    }
    /// Diagnostic counter: cached seeks never spawn another decoder.
    pub fn decoder_starts(&self) -> u64 {
        self.control.decoder_starts.load(Ordering::Relaxed)
    }
    /// Cumulative starvation bursts after playback is primed. Startup, pause,
    /// requested seeks and finite EOF tails do not count as underruns.
    pub fn underruns(&self) -> u64 {
        self.control.underruns.load(Ordering::Relaxed)
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.state.lock().map(|s| s.clone()).unwrap_or(Snapshot {
            state: PlaybackState::Failed(PlaybackError::Worker),
            ..Snapshot::default()
        });
        s.volume = f32::from_bits(self.volume.load(Ordering::Relaxed));
        s
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.control.new_source();
        let _ = self.commands.send(Command::Shutdown);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
fn normal_volume(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        DEFAULT_VOLUME
    }
}
fn seek_position(v: f64, duration: u32) -> f64 {
    if !v.is_finite() {
        0.0
    } else {
        v.max(0.0).min(if duration > 0 {
            f64::from(duration)
        } else {
            86_400.0
        })
    }
}
fn valid_url(v: &str) -> bool {
    v.len() <= 16_384
        && !v.chars().any(char::is_control)
        && reqwest::Url::parse(v).is_ok_and(|u| {
            u.scheme() == "https"
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
                && u.port_or_known_default() == Some(443)
        })
}
#[derive(Clone)]
struct Source {
    url: Option<Zeroizing<String>>,
    duration: u32,
    key: Option<CacheKey>,
    loudness: Arc<normalization::Profile>,
}
// Relative frames use 48 bits (more than every u32-second 48 kHz track);
// the remaining bits version refills so an in-flight callback cannot overwrite
// the position of a seek, including a seek to the same sample position.
const PCM_FRAME_MASK: u64 = (1_u64 << 48) - 1;
const PCM_EPOCH_STEP: u64 = 1_u64 << 48;

/// Producer-only bookkeeping. The audio callback never locks this mutex.
struct PcmWindow {
    start_frame: u64,
    end_frame: u64,
}
impl PcmWindow {
    fn sample_len(&self, pcm: &Pcm) -> usize {
        self.end_frame.saturating_sub(self.start_frame) as usize * pcm.channels
    }
    fn writable(&mut self, pcm: &Pcm) -> usize {
        let read = pcm.cursor.load(Ordering::Acquire) & PCM_FRAME_MASK;
        self.start_frame = self
            .start_frame
            .max(read.saturating_sub(pcm.history_frames as u64));
        pcm.start_frame.store(self.start_frame, Ordering::Release);
        let ahead = self.end_frame.saturating_sub(read) as usize * pcm.channels;
        pcm.capacity
            .saturating_sub(self.sample_len(pcm))
            .min(pcm.ahead_capacity.saturating_sub(ahead))
    }
    fn append(&mut self, pcm: &Pcm, bytes: &[u8]) {
        let frame_bytes = pcm.channels * size_of::<f32>();
        let frames = bytes.len() / frame_bytes;
        let capacity_frames = pcm.capacity / pcm.channels;
        for (index, frame) in bytes[..frames * frame_bytes]
            .chunks_exact(frame_bytes)
            .enumerate()
        {
            let slot =
                ((self.end_frame + index as u64) % capacity_frames as u64) as usize * pcm.channels;
            for (channel, raw) in frame.as_chunks::<4>().0.iter().enumerate() {
                let value = f32::from_le_bytes(*raw);
                let value = if value.is_finite() {
                    // Floating-point decoders can produce valid inter-sample
                    // overs. Keep them until normalization can attenuate them.
                    let limit = if pcm.loudness.is_some() { 16.0 } else { 1.0 };
                    value.clamp(-limit, limit)
                } else {
                    0.0
                };
                pcm.samples[slot + channel].store(value.to_bits(), Ordering::Relaxed);
            }
        }
        self.end_frame += frames as u64;
        // Publishing the complete frames after their samples makes them visible
        // to the callback as one ordered append, without a callback-side lock.
        pcm.end_frame.store(self.end_frame, Ordering::Release);
    }
}
struct Pcm {
    window: Mutex<PcmWindow>,
    samples: Box<[AtomicU32]>,
    cursor: AtomicU64,
    start_frame: AtomicU64,
    end_frame: AtomicU64,
    primed: AtomicU64,
    underruns: Arc<AtomicU64>,
    space: Condvar,
    capacity: usize,
    ahead_capacity: usize,
    history_frames: usize,
    channels: usize,
    sample_rate: u32,
    canceled: AtomicBool,
    paused: AtomicBool,
    eof: AtomicBool,
    read_failed: AtomicBool,
    device_failed: AtomicBool,
    started: AtomicBool,
    volume: Arc<AtomicU32>,
    loudness: Option<Arc<normalization::Profile>>,
}
impl Pcm {
    fn new(channels: u16, sample_rate: u32, volume: Arc<AtomicU32>, paused: bool) -> Self {
        let channels = usize::from(channels).max(1);
        let max_frames = MAX_PCM_BYTES / size_of::<f32>() / channels;
        let ahead_frames = (sample_rate as usize * BUFFER_SECONDS).min(max_frames / 2);
        let history_frames =
            (sample_rate as usize * HISTORY_SECONDS).min(max_frames - ahead_frames);
        let capacity = (ahead_frames + history_frames).max(1) * channels;
        Self {
            window: Mutex::new(PcmWindow {
                start_frame: 0,
                end_frame: 0,
            }),
            samples: (0..capacity)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            cursor: AtomicU64::new(0),
            start_frame: AtomicU64::new(0),
            end_frame: AtomicU64::new(0),
            primed: AtomicU64::new(0),
            underruns: Arc::new(AtomicU64::new(0)),
            space: Condvar::new(),
            capacity,
            ahead_capacity: ahead_frames * channels,
            history_frames,
            channels,
            sample_rate,
            canceled: AtomicBool::new(false),
            paused: AtomicBool::new(paused),
            eof: AtomicBool::new(false),
            read_failed: AtomicBool::new(false),
            device_failed: AtomicBool::new(false),
            started: AtomicBool::new(false),
            volume,
            loudness: None,
        }
    }
    fn is_primed(&self) -> bool {
        let epoch = self.cursor.load(Ordering::Acquire) & !PCM_FRAME_MASK;
        self.primed.load(Ordering::Acquire) == (epoch | 1)
    }
    fn mark_primed(&self, cursor: u64) {
        let previous = self.primed.load(Ordering::Acquire);
        let epoch = cursor & !PCM_FRAME_MASK;
        if self.cursor.load(Ordering::Acquire) & !PCM_FRAME_MASK == epoch {
            // A controller seek may publish its own priming token between these
            // reads. Never overwrite that new token with an old callback's one.
            let _ = self.primed.compare_exchange(
                previous,
                epoch | 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
    fn buffered_range(&self, offset: f64, duration: u32) -> (f64, f64, bool) {
        let cursor = self.cursor.load(Ordering::Acquire);
        let read = cursor & PCM_FRAME_MASK;
        let end = self.end_frame.load(Ordering::Acquire);
        if (self.cursor.load(Ordering::Acquire) ^ cursor) & !PCM_FRAME_MASK != 0 {
            return (offset, offset, false);
        }
        let limit = |time: f64| {
            if duration > 0 {
                time.min(f64::from(duration))
            } else {
                time
            }
        };
        let position = limit(offset + read as f64 / f64::from(self.sample_rate));
        let end_time = limit(offset + end as f64 / f64::from(self.sample_rate));
        (position, end_time.max(position), end > read)
    }
    fn seek_cached(&self, target: f64, offset: f64, duration: u32) -> bool {
        let frame = (seek_position(target, duration) - offset) * f64::from(self.sample_rate);
        if !frame.is_finite() || frame < 0.0 || frame >= PCM_FRAME_MASK as f64 {
            return false;
        }
        let frame = (frame + 1e-6).floor() as u64;
        // Serialize with producer eviction, never with the callback.
        let Ok(window) = self.window.lock() else {
            return false;
        };
        if frame < window.start_frame || frame > window.end_frame {
            return false;
        }
        let mut cursor = self.cursor.load(Ordering::Acquire);
        loop {
            let next = (cursor.wrapping_add(PCM_EPOCH_STEP) & !PCM_FRAME_MASK) | frame;
            match self.cursor.compare_exchange_weak(
                cursor,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => cursor = current,
            }
        }
        let epoch = self.cursor.load(Ordering::Acquire) & !PCM_FRAME_MASK;
        self.primed.store(
            if self.started.load(Ordering::Acquire) && frame < window.end_frame {
                epoch | 1
            } else {
                0
            },
            Ordering::Release,
        );
        drop(window);
        self.space.notify_one();
        true
    }
}
#[cfg(test)]
fn render(output: &mut [f32], pcm: &Pcm) {
    render_normalized(
        output,
        pcm,
        &mut normalization::OutputNormalizer::new(pcm.sample_rate),
    );
}
fn render_normalized(
    output: &mut [f32],
    pcm: &Pcm,
    normalizer: &mut normalization::OutputNormalizer,
) {
    output.fill(0.0);
    if pcm.canceled.load(Ordering::Acquire) || pcm.paused.load(Ordering::Acquire) {
        return;
    }
    let cursor = pcm.cursor.load(Ordering::Acquire);
    let read = cursor & PCM_FRAME_MASK;
    let start = pcm.start_frame.load(Ordering::Acquire);
    let end = pcm.end_frame.load(Ordering::Acquire);
    if read < start || read > end {
        return;
    }
    let available = end.saturating_sub(read) as usize;
    let finished = pcm.eof.load(Ordering::Acquire) || pcm.read_failed.load(Ordering::Acquire);
    let startup = (pcm.sample_rate as usize / 2).max(1);
    let primed = (cursor & !PCM_FRAME_MASK) | 1;
    if pcm.primed.load(Ordering::Acquire) != primed && !finished && available < startup {
        return;
    }
    let requested = output.len() / pcm.channels;
    let frames = requested.min(available);
    let volume = f32::from_bits(pcm.volume.load(Ordering::Relaxed));
    let capacity_frames = pcm.capacity / pcm.channels;
    for (index, output) in output[..frames * pcm.channels]
        .chunks_exact_mut(pcm.channels)
        .enumerate()
    {
        let slot = ((read + index as u64) % capacity_frames as u64) as usize * pcm.channels;
        for (channel, sample) in output.iter_mut().enumerate() {
            let raw = f32::from_bits(pcm.samples[slot + channel].load(Ordering::Relaxed));
            *sample = if pcm.loudness.is_some() {
                raw
            } else {
                (raw * volume).clamp(-1.0, 1.0)
            };
        }
    }
    let next = (cursor & !PCM_FRAME_MASK) | ((read + frames as u64) & PCM_FRAME_MASK);
    if pcm
        .cursor
        .compare_exchange(cursor, next, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        // Only an explicit seek/refill changes the cursor outside this callback.
        // Discard this obsolete quantum instead of advancing the new timeline.
        output.fill(0.0);
        return;
    }
    if let Some(profile) = &pcm.loudness {
        normalizer.process(
            &mut output[..frames * pcm.channels],
            pcm.channels,
            profile.enabled(),
            profile.gain(),
            volume,
        );
    }
    if frames > 0 {
        pcm.started.store(true, Ordering::Release);
        pcm.mark_primed(cursor);
    }
    if frames < requested
        && !finished
        && pcm.cursor.load(Ordering::Acquire) & !PCM_FRAME_MASK == cursor & !PCM_FRAME_MASK
        && !pcm.paused.load(Ordering::Acquire)
        && !pcm.canceled.load(Ordering::Acquire)
    {
        pcm.underruns.fetch_add(1, Ordering::Relaxed);
        let _ = pcm
            .primed
            .compare_exchange(primed, 0, Ordering::AcqRel, Ordering::Acquire);
    }
    // No mutex, allocation, I/O, or futex wake occurs on the RT thread. A
    // bounded producer wait rechecks the atomic cursor when space becomes free.
}
#[cfg(test)]
fn feed(mut input: impl Read, pcm: &Pcm) {
    let mut bytes = [0_u8; 16_387];
    let mut carry = 0;
    let frame_bytes = pcm.channels * size_of::<f32>();
    loop {
        if pcm.canceled.load(Ordering::Acquire) {
            break;
        }
        let count = match input.read(&mut bytes[carry..16_384]) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                pcm.read_failed.store(true, Ordering::Release);
                break;
            }
        };
        let available = carry + count;
        let complete = available / frame_bytes * frame_bytes;
        if !push_pcm(&bytes[..complete], pcm, &AtomicBool::new(false)) {
            break;
        }
        carry = available - complete;
        bytes.copy_within(complete..available, 0);
    }
    if carry != 0 && !pcm.canceled.load(Ordering::Acquire) {
        pcm.read_failed.store(true, Ordering::Release);
    }
    pcm.eof.store(true, Ordering::Release);
}
const FRAME_BYTES: usize = CHANNELS as usize * size_of::<f32>();
const DISK_READ_FRAMES: usize = 4_096;

#[derive(Clone)]
struct CacheBinding {
    cache: AudioCache,
    key: CacheKey,
}
impl CacheBinding {
    fn read(&self, frame: u64) -> Vec<u8> {
        self.cache
            .read_contiguous(&self.key, frame, DISK_READ_FRAMES)
            .unwrap_or_default()
    }
    fn complete(&self) -> Option<u64> {
        self.cache
            .complete_frames(&self.key)
            .ok()
            .flatten()
            .filter(|end| completed_duration_plausible(*end, self.key.duration))
    }
    fn next_range(&self, frame: u64) -> Option<u64> {
        self.cache
            .ranges(&self.key)
            .ok()?
            .into_iter()
            .filter_map(|(start, _)| (start > frame).then_some(start))
            .min()
    }
}
fn binding(source: &Source, cache: Option<&AudioCache>) -> Option<CacheBinding> {
    Some(CacheBinding {
        cache: cache?.clone(),
        key: source.key?,
    })
}
// Commit sizeable contiguous writes rather than fsyncing each decoded block.
// Drop flushes the tail on pause/seek/stop so played audio remains reusable.
struct CacheWriter {
    cache: Option<CacheBinding>,
    start: u64,
    bytes: Vec<u8>,
    revision: Option<u64>,
}
impl CacheWriter {
    fn new(cache: Option<&CacheBinding>) -> Self {
        Self {
            cache: cache.cloned(),
            start: 0,
            bytes: Vec::with_capacity(256 * 1024),
            revision: None,
        }
    }
    fn append(&mut self, frame: u64, bytes: &[u8]) -> bool {
        if self.cache.is_none() {
            return true;
        }
        let mut saved = true;
        if self.bytes.is_empty() {
            self.start = frame;
        } else if self.start + (self.bytes.len() / FRAME_BYTES) as u64 != frame {
            saved = self.flush();
            self.start = frame;
        }
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() >= 256 * 1024 {
            saved &= self.flush();
        }
        saved
    }
    fn flush(&mut self) -> bool {
        let mut saved = true;
        if !self.bytes.is_empty() {
            if let Some(cache) = &self.cache {
                let result = match self.revision {
                    Some(revision) => {
                        cache
                            .cache
                            .write_at_revision(&cache.key, self.start, &self.bytes, revision)
                    }
                    None => cache.cache.write_range(&cache.key, self.start, &self.bytes),
                };
                saved = result.is_ok_and(|frames| frames == self.bytes.len() / FRAME_BYTES);
            }
            self.bytes.clear();
        }
        saved
    }
}
impl Drop for CacheWriter {
    fn drop(&mut self) {
        self.flush();
    }
}
fn frame_at(time: f64) -> u64 {
    (time.max(0.0) * f64::from(SAMPLE_RATE)).floor() as u64
}
fn frame_time(frame: u64) -> f64 {
    frame as f64 / f64::from(SAMPLE_RATE)
}
fn push_pcm(bytes: &[u8], pcm: &Pcm, canceled: &AtomicBool) -> bool {
    let frame_bytes = pcm.channels * size_of::<f32>();
    if !bytes.len().is_multiple_of(frame_bytes) {
        return false;
    }
    let mut cursor = 0;
    while cursor < bytes.len()
        && !canceled.load(Ordering::Acquire)
        && !pcm.canceled.load(Ordering::Acquire)
    {
        let Ok(mut window) = pcm.window.lock() else {
            return false;
        };
        let mut writable = window.writable(pcm);
        while writable < pcm.channels
            && !canceled.load(Ordering::Acquire)
            && !pcm.canceled.load(Ordering::Acquire)
        {
            let Ok((guard, _)) = pcm.space.wait_timeout(window, Duration::from_millis(20)) else {
                return false;
            };
            window = guard;
            writable = window.writable(pcm);
        }
        if canceled.load(Ordering::Acquire) || pcm.canceled.load(Ordering::Acquire) {
            return false;
        }
        let frames = ((bytes.len() - cursor) / frame_bytes).min(writable / pcm.channels);
        let count = frames * frame_bytes;
        window.append(pcm, &bytes[cursor..cursor + count]);
        cursor += count;
    }
    cursor == bytes.len()
}
fn prefill(pcm: &Pcm, bytes: &[u8], was_started: bool) {
    if let Ok(mut window) = pcm.window.lock() {
        window.start_frame = 0;
        window.end_frame = 0;
        pcm.end_frame.store(0, Ordering::Release);
        pcm.start_frame.store(0, Ordering::Release);
        pcm.primed.store(0, Ordering::Release);
        pcm.eof.store(false, Ordering::Release);
        pcm.read_failed.store(false, Ordering::Release);
        pcm.started
            .store(was_started && !bytes.is_empty(), Ordering::Release);
        let previous = pcm.cursor.load(Ordering::Acquire);
        let epoch = previous.wrapping_add(PCM_EPOCH_STEP) & !PCM_FRAME_MASK;
        pcm.cursor.store(epoch, Ordering::Release);
        let maximum = bytes.len().min(pcm.ahead_capacity * size_of::<f32>());
        window.append(pcm, &bytes[..maximum]);
        pcm.primed.store(
            if was_started && maximum > 0 {
                epoch | 1
            } else {
                0
            },
            Ordering::Release,
        );
    }
}
type DecoderFactory = fn(&str, f64) -> std::io::Result<crate::media_decoder::Stream>;
type DecoderSlot = Arc<Mutex<Option<crate::media_decoder::Cancellation>>>;
fn spawn_decoder(url: &str, offset: f64) -> std::io::Result<crate::media_decoder::Stream> {
    crate::media_decoder::open(url, offset)
}
fn log_decoder_io_error(stage: &str, error: &std::io::Error) {
    crate::diagnostics::event("decoder", &format!("{stage} kind={:?}", error.kind()));
}
fn finish_decoder(
    slot: &DecoderSlot,
    canceled: &AtomicBool,
    force: bool,
) -> Result<(), PlaybackError> {
    if let Some(handle) = slot.lock().map_err(|_| PlaybackError::Worker)?.take()
        && (force || canceled.load(Ordering::Acquire))
    {
        handle.cancel();
    }
    Ok(())
}
fn pump(
    source: &Source,
    cache: Option<&CacheBinding>,
    frame: u64,
    pcm: &Pcm,
    canceled: &AtomicBool,
    decoder_slot: &DecoderSlot,
    control: &Control,
) -> Result<(), PlaybackError> {
    pump_mode(
        source,
        cache,
        frame,
        Some(pcm),
        canceled,
        decoder_slot,
        control,
        None,
        None,
        None,
        spawn_decoder,
    )
}

#[expect(clippy::too_many_arguments)]
fn pump_mode(
    source: &Source,
    cache: Option<&CacheBinding>,
    mut frame: u64,
    pcm: Option<&Pcm>,
    canceled: &AtomicBool,
    decoder_slot: &DecoderSlot,
    control: &Control,
    revision: Option<u64>,
    stop_at: Option<u64>,
    eof_frame: Option<&AtomicU64>,
    decoder: DecoderFactory,
) -> Result<(), PlaybackError> {
    let first_frame = frame;
    let mut writer = CacheWriter::new(cache);
    writer.revision = revision;
    loop {
        if canceled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if revision.is_some_and(|revision| {
            cache
                .and_then(|cache| cache.cache.policy().ok())
                .is_none_or(|(_, current)| current != revision)
        }) {
            return Err(PlaybackError::CacheMiss);
        }
        if stop_at.is_some_and(|end| frame >= end) {
            return Ok(());
        }
        if let Some(cache) = cache {
            let bytes = cache.read(frame);
            if !bytes.is_empty() {
                source.loudness.feed(frame, &bytes);
                if pcm.is_some_and(|pcm| !push_pcm(&bytes, pcm, canceled)) {
                    return Ok(());
                }
                frame += (bytes.len() / FRAME_BYTES) as u64;
                continue;
            }
            if let Some(end) = cache.complete()
                && frame >= end
            {
                if let Some(eof_frame) = eof_frame {
                    eof_frame.store(end, Ordering::Release);
                }
                return Ok(());
            }
        }
        let url = source.url.as_ref().ok_or(PlaybackError::CacheMiss)?;
        if !valid_url(url) {
            return Err(PlaybackError::InvalidUrl);
        }
        let next_cached = cache
            .and_then(|cache| cache.next_range(frame))
            .into_iter()
            .chain(stop_at)
            .min();
        let stream = decoder(url, frame_time(frame)).map_err(|error| {
            log_decoder_io_error("open_failed", &error);
            PlaybackError::Decoder
        })?;
        control.decoder_starts.fetch_add(1, Ordering::Relaxed);
        *decoder_slot.lock().map_err(|_| PlaybackError::Worker)? = Some(stream.cancellation);
        let mut input = stream.reader;
        if canceled.load(Ordering::Relaxed) {
            let _ = finish_decoder(decoder_slot, canceled, true);
            return Ok(());
        }
        let mut bytes = [0_u8; 32_775];
        let mut carry = 0;
        let reached_eof = loop {
            if canceled.load(Ordering::Relaxed) {
                let _ = finish_decoder(decoder_slot, canceled, true);
                return Ok(());
            }
            let n = match input.read(&mut bytes[carry..32_768]) {
                Ok(0) => break true,
                Ok(n) => n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    log_decoder_io_error("stream_read_failed", &error);
                    let _ = finish_decoder(decoder_slot, canceled, true);
                    return Err(PlaybackError::Decoder);
                }
            };
            let available = carry + n;
            let complete = available / FRAME_BYTES * FRAME_BYTES;
            let frames = (complete / FRAME_BYTES) as u64;
            let take_frames =
                next_cached.map_or(frames, |end| frames.min(end.saturating_sub(frame)));
            let take_bytes = take_frames as usize * FRAME_BYTES;
            if take_bytes > 0 {
                source.loudness.feed(frame, &bytes[..take_bytes]);
                let mut saved = writer.append(frame, &bytes[..take_bytes]);
                if revision.is_some() && frame == first_frame {
                    // Make the initial seek position audible immediately; later
                    // commits stay batched for background throughput.
                    saved &= writer.flush();
                }
                if !saved && revision.is_some() {
                    let _ = finish_decoder(decoder_slot, canceled, true);
                    return Err(PlaybackError::CacheMiss);
                }
                if pcm.is_some_and(|pcm| !push_pcm(&bytes[..take_bytes], pcm, canceled)) {
                    let _ = finish_decoder(decoder_slot, canceled, true);
                    return Ok(());
                }
                frame += take_frames;
            }
            if next_cached.is_some_and(|end| frame >= end) {
                break false;
            }
            carry = available - complete;
            bytes.copy_within(complete..available, 0);
        };
        if reached_eof {
            if carry != 0 {
                crate::diagnostics::event("decoder", "output_incomplete_frame");
                let _ = finish_decoder(decoder_slot, canceled, true);
                return Err(PlaybackError::Decoder);
            }
            finish_decoder(decoder_slot, canceled, false)?;
            if canceled.load(Ordering::Relaxed) {
                return Ok(());
            }
            if !writer.flush() && revision.is_some() {
                return Err(PlaybackError::CacheMiss);
            }
            if !completed_duration_plausible(frame, source.duration) {
                crate::diagnostics::event("decoder", "output_duration_incomplete");
                return Err(PlaybackError::Decoder);
            }
            source.loudness.finish_stream(frame);
            if let Some(eof_frame) = eof_frame {
                eof_frame.store(frame, Ordering::Release);
            } else if let Some(cache) = cache {
                let _ = cache.cache.mark_complete(&cache.key, frame);
            }
            return Ok(());
        }
        // This missing interval is filled. Continue directly from the next
        // existing PCM range instead of fetching/decoding that range again.
        let _ = finish_decoder(decoder_slot, canceled, true);
        if !writer.flush() && revision.is_some() {
            return Err(PlaybackError::CacheMiss);
        }
    }
}
/// The source downloader is independent of the bounded audible PCM window.
/// Cached seeks replace only their disk-to-RAM feeder, not this downloader.
struct Prefetch {
    canceled: Arc<AtomicBool>,
    disabled: AtomicBool,
    decoder_slot: DecoderSlot,
    reader: Mutex<Option<JoinHandle<()>>>,
    result: Arc<Mutex<Option<Result<(), PlaybackError>>>>,
    changed: Arc<Condvar>,
    eof: Arc<AtomicU64>,
    revision: u64,
}
impl Prefetch {
    fn new(
        source: &Source,
        cache: &CacheBinding,
        first_frame: u64,
        control: Arc<Control>,
    ) -> Result<Option<Arc<Self>>, PlaybackError> {
        Self::with_decoder(source, cache, first_frame, control, spawn_decoder)
    }
    fn with_decoder(
        source: &Source,
        cache: &CacheBinding,
        first_frame: u64,
        control: Arc<Control>,
        decoder: DecoderFactory,
    ) -> Result<Option<Arc<Self>>, PlaybackError> {
        let (limit, revision) = cache.cache.policy().map_err(|_| PlaybackError::CacheMiss)?;
        // Metadata may round the duration; leave two seconds of headroom. The
        // actual write count remains authoritative if the budget later changes.
        let required = u64::from(source.duration)
            .saturating_add(2)
            .saturating_mul(u64::from(SAMPLE_RATE))
            .saturating_mul(FRAME_BYTES as u64);
        if source.duration == 0
            || source.url.is_none()
            || limit < required
            || cache.complete().is_some()
        {
            return Ok(None);
        }
        let canceled = Arc::new(AtomicBool::new(false));
        let decoder_slot = Arc::new(Mutex::new(None));
        let result = Arc::new(Mutex::new(None));
        let changed = Arc::new(Condvar::new());
        let eof = Arc::new(AtomicU64::new(0));
        let worker_eof = eof.clone();
        let (worker_cancel, worker_decoder, worker_result, worker_changed) = (
            canceled.clone(),
            decoder_slot.clone(),
            result.clone(),
            changed.clone(),
        );
        let source = source.clone();
        let cache = cache.clone();
        let reader = thread::Builder::new()
            .name("vk-track-prefetch".into())
            .spawn(move || {
                // Prioritize the requested seek position, then fill its prefix.
                let status = pump_mode(
                    &source,
                    Some(&cache),
                    first_frame,
                    None,
                    &worker_cancel,
                    &worker_decoder,
                    &control,
                    Some(revision),
                    None,
                    Some(&worker_eof),
                    decoder,
                )
                .and_then(|()| {
                    if worker_cancel.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    let end = worker_eof.load(Ordering::Acquire);
                    if !completed_duration_plausible(end, source.duration) {
                        return Err(PlaybackError::Decoder);
                    }
                    if first_frame > 0 {
                        pump_mode(
                            &source,
                            Some(&cache),
                            0,
                            None,
                            &worker_cancel,
                            &worker_decoder,
                            &control,
                            Some(revision),
                            Some(first_frame),
                            Some(&AtomicU64::new(0)),
                            decoder,
                        )?;
                    }
                    if worker_cancel.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    cache
                        .cache
                        .mark_complete_at_revision(&cache.key, end, revision)
                        .map_err(|_| PlaybackError::CacheMiss)?;
                    source.loudness.scan_cache(&cache, control.clone());
                    Ok(())
                });
                if let Ok(mut result) = worker_result.lock() {
                    *result = Some(status);
                }
                worker_changed.notify_all();
            })
            .map_err(|_| PlaybackError::Worker)?;
        Ok(Some(Arc::new(Self {
            canceled,
            disabled: AtomicBool::new(false),
            decoder_slot,
            reader: Mutex::new(Some(reader)),
            result,
            changed,
            eof,
            revision,
        })))
    }
    fn stop(&self) {
        self.disabled.store(true, Ordering::Release);
        self.canceled.store(true, Ordering::Release);
        self.changed.notify_all();
        if let Ok(mut guard) = self.decoder_slot.lock()
            && let Some(process) = guard.as_mut()
        {
            process.cancel();
        }
        let reader = self.reader.lock().ok().and_then(|mut reader| reader.take());
        if let Some(reader) = reader {
            let _ = reader.join();
        }
        if let Ok(mut guard) = self.decoder_slot.lock()
            && let Some(process) = guard.take()
        {
            process.cancel();
        }
    }
}
impl Drop for Prefetch {
    fn drop(&mut self) {
        self.stop();
    }
}

#[expect(clippy::too_many_arguments)]
fn pump_from_prefetch(
    source: &Source,
    cache: &CacheBinding,
    mut frame: u64,
    pcm: &Pcm,
    canceled: &AtomicBool,
    decoder_slot: &DecoderSlot,
    control: &Control,
    prefetch: &Prefetch,
) -> Result<(), PlaybackError> {
    loop {
        if canceled.load(Ordering::Acquire) || pcm.canceled.load(Ordering::Acquire) {
            return Ok(());
        }
        let bytes = cache.read(frame);
        if !bytes.is_empty() {
            source.loudness.feed(frame, &bytes);
            if !push_pcm(&bytes, pcm, canceled) {
                return Ok(());
            }
            frame += (bytes.len() / FRAME_BYTES) as u64;
            continue;
        }
        // A requested tail can finish before the downloader backfills the
        // missing prefix. This validated natural EOF belongs to audible media,
        // independently of whether the entire track is ready for offline reuse.
        let end = prefetch.eof.load(Ordering::Acquire);
        if (end > 0 && frame >= end) || cache.complete().is_some_and(|end| frame >= end) {
            return Ok(());
        }
        let policy_changed = !cache
            .cache
            .policy()
            .is_ok_and(|(_, revision)| revision == prefetch.revision);
        let result = prefetch
            .result
            .lock()
            .map_err(|_| PlaybackError::Worker)?
            .clone();
        if policy_changed
            || prefetch.disabled.load(Ordering::Acquire)
            || matches!(result, Some(Ok(())) | Some(Err(PlaybackError::CacheMiss)))
        {
            // Explicit cache changes and insufficient writes downgrade to the
            // bounded streaming path. Stop the downloader before a new decode stream.
            prefetch.stop();
            return pump(
                source,
                Some(cache),
                frame,
                pcm,
                canceled,
                decoder_slot,
                control,
            );
        }
        if let Some(Err(error)) = result {
            return Err(error);
        }
        let result = prefetch.result.lock().map_err(|_| PlaybackError::Worker)?;
        let _ = prefetch.changed.wait_timeout(result, TICK);
    }
}

struct Pump {
    canceled: Arc<AtomicBool>,
    decoder_slot: DecoderSlot,
    reader: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<PlaybackError>>>,
    pcm: Arc<Pcm>,
}
impl Pump {
    fn new(
        source: Source,
        cache: Option<CacheBinding>,
        frame: u64,
        pcm: Arc<Pcm>,
        control: Arc<Control>,
        prefetch: Option<Arc<Prefetch>>,
    ) -> Result<Self, PlaybackError> {
        let canceled = Arc::new(AtomicBool::new(false));
        let decoder_slot = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let (reader_canceled, reader_decoder, reader_error, reader_pcm) = (
            canceled.clone(),
            decoder_slot.clone(),
            error.clone(),
            pcm.clone(),
        );
        let reader = thread::Builder::new()
            .name("vk-pcm-source".into())
            .spawn(move || {
                let result = match (prefetch.as_deref(), cache.as_ref()) {
                    (Some(prefetch), Some(cache)) => pump_from_prefetch(
                        &source,
                        cache,
                        frame,
                        &reader_pcm,
                        &reader_canceled,
                        &reader_decoder,
                        &control,
                        prefetch,
                    ),
                    _ => pump(
                        &source,
                        cache.as_ref(),
                        frame,
                        &reader_pcm,
                        &reader_canceled,
                        &reader_decoder,
                        &control,
                    ),
                };
                if !reader_canceled.load(Ordering::Relaxed) {
                    match result {
                        Ok(()) => reader_pcm.eof.store(true, Ordering::Release),
                        Err(failure) => {
                            if let Ok(mut error) = reader_error.lock() {
                                *error = Some(failure);
                            }
                            reader_pcm.read_failed.store(true, Ordering::Relaxed);
                        }
                    }
                }
            })
            .map_err(|_| PlaybackError::Worker)?;
        Ok(Self {
            canceled,
            decoder_slot,
            reader: Some(reader),
            error,
            pcm,
        })
    }
    fn failure(&self) -> Option<PlaybackError> {
        self.error.lock().ok()?.clone()
    }
}
impl Drop for Pump {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
        self.pcm.space.notify_all();
        if let Ok(mut guard) = self.decoder_slot.lock()
            && let Some(decoder_slot) = guard.as_mut()
        {
            decoder_slot.cancel();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Ok(mut guard) = self.decoder_slot.lock()
            && let Some(decoder_slot) = guard.take()
        {
            decoder_slot.cancel();
        }
    }
}
struct Running {
    pump: Option<Pump>,
    _stream: OutputStream,
    pcm: Arc<Pcm>,
    offset: f64,
    duration: u32,
    cache: Option<CacheBinding>,
    prefetch: Option<Arc<Prefetch>>,
    _pin: Option<CachePin>,
}
impl Running {
    fn seek_disk(
        &mut self,
        source: &Source,
        offset: f64,
        paused: bool,
        control: Arc<Control>,
    ) -> Result<bool, PlaybackError> {
        let Some(cache) = &self.cache else {
            return Ok(false);
        };
        let frame = frame_at(offset);
        let bytes = cache.read(frame);
        if bytes.is_empty() && cache.complete().is_none_or(|end| frame < end) {
            return Ok(false);
        }
        // Keep PipeWire's stream, stop only the old source pump, refill from
        // disk off the audio callback and then continue from the local range.
        self.pcm.paused.store(true, Ordering::Relaxed);
        self.pump = None;
        let was_started = self.pcm.started.load(Ordering::Relaxed);
        prefill(&self.pcm, &bytes, was_started);
        self.offset = frame_time(frame);
        self.pump = Some(Pump::new(
            source.clone(),
            self.cache.clone(),
            frame + (bytes.len() / FRAME_BYTES) as u64,
            self.pcm.clone(),
            control,
            self.prefetch.clone(),
        )?);
        self.pcm.paused.store(paused, Ordering::Relaxed);
        Ok(true)
    }
    fn progress(&self) -> (f64, f64, bool) {
        let (position, mut end, ready) = self.pcm.buffered_range(self.offset, self.duration);
        if let Some(cache) = &self.cache
            && let Ok(cached) = cache.cache.contiguous_end(&cache.key, frame_at(position))
        {
            let disk_end = frame_time(cached);
            end = end.max(if self.duration > 0 {
                disk_end.min(f64::from(self.duration))
            } else {
                disk_end
            });
        }
        (position, end, ready)
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.pcm.canceled.store(true, Ordering::Release);
        self.pcm.space.notify_all();
        self.pump = None;
    }
}
fn start(
    source: &Source,
    offset: f64,
    volume: Arc<AtomicU32>,
    paused: bool,
    control: Arc<Control>,
    cache: Option<&AudioCache>,
) -> Result<Running, PlaybackError> {
    let cache = binding(source, cache);
    let pin = cache.as_ref().map(|cache| cache.cache.pin(&cache.key));
    let frame = frame_at(offset);
    let bytes = cache
        .as_ref()
        .map_or_else(Vec::new, |cache| cache.read(frame));
    if bytes.is_empty()
        && source.url.is_none()
        && cache
            .as_ref()
            .and_then(CacheBinding::complete)
            .is_none_or(|end| frame < end)
    {
        return Err(PlaybackError::CacheMiss);
    }
    let pcm = Arc::new(Pcm {
        underruns: control.underruns.clone(),
        loudness: Some(source.loudness.clone()),
        ..Pcm::new(CHANNELS, SAMPLE_RATE, volume, paused)
    });
    source.loudness.feed(frame, &bytes);
    if let Some(cache) = &cache {
        source.loudness.scan_cache(cache, control.clone());
    }
    prefill(&pcm, &bytes, false);
    let render_pcm = pcm.clone();
    let errors = pcm.clone();
    let mut normalizer = normalization::OutputNormalizer::new(SAMPLE_RATE);
    let stream = audio_output::open(
        move |data| render_normalized(data, &render_pcm, &mut normalizer),
        move || {
            errors.device_failed.store(true, Ordering::Relaxed);
        },
    )
    .map_err(|_| PlaybackError::Device)?;
    let prefetch = match &cache {
        Some(cache) => Prefetch::new(source, cache, frame, control.clone())?,
        None => None,
    };
    let pump = Pump::new(
        source.clone(),
        cache.clone(),
        frame + (bytes.len() / FRAME_BYTES) as u64,
        pcm.clone(),
        control,
        prefetch.clone(),
    )?;
    Ok(Running {
        pump: Some(pump),
        _stream: stream,
        pcm,
        offset: frame_time(frame),
        duration: source.duration,
        cache,
        prefetch,
        _pin: pin,
    })
}
fn publish(
    state: &Mutex<Snapshot>,
    control: &Control,
    epoch: Epoch,
    playback: PlaybackState,
    range: (f64, f64),
) {
    if let Ok(mut s) = state.lock() {
        if !control.current(epoch) {
            return;
        }
        s.state = playback;
        s.position = range.0;
        s.buffered_until = range.1.max(range.0);
    }
}

fn completion_state(
    started: bool,
    paused: bool,
    offset: f64,
    position: f64,
    duration: u32,
) -> PlaybackState {
    // A user can seek straight to decoded EOF while still paused, before any
    // sample has reached the device. That is a completed seek, not a decoder
    // failure. A genuinely empty decode before the requested end still fails.
    if started || position > offset || (duration > 0 && offset >= f64::from(duration)) {
        if paused {
            PlaybackState::Paused
        } else {
            PlaybackState::Ended
        }
    } else {
        PlaybackState::Failed(PlaybackError::Decoder)
    }
}
fn controller(
    commands: Receiver<Command>,
    state: Arc<Mutex<Snapshot>>,
    volume: Arc<AtomicU32>,
    control: Arc<Control>,
    cache: Option<AudioCache>,
) {
    let mut source: Option<Source> = None;
    let mut running: Option<Running> = None;
    let mut active = Epoch::default();
    let mut pause_intent = false;
    loop {
        match commands.recv_timeout(TICK) {
            Ok(Command::Play {
                epoch,
                source: selected,
                offset,
            }) => {
                if !control.current_source(epoch) {
                    continue;
                }
                let duration = selected.duration;
                source = Some(selected);
                running = None;
                active = epoch;
                pause_intent = false;
                if !control.current(epoch) {
                    continue;
                }
                let offset = seek_position(offset, duration);
                publish(
                    &state,
                    &control,
                    active,
                    PlaybackState::Loading,
                    (offset, offset),
                );
                match start(
                    source.as_ref().unwrap(),
                    offset,
                    volume.clone(),
                    false,
                    control.clone(),
                    cache.as_ref(),
                ) {
                    Ok(new) => running = Some(new),
                    Err(e) => publish(
                        &state,
                        &control,
                        active,
                        PlaybackState::Failed(e),
                        (offset, offset),
                    ),
                }
            }
            Ok(Command::Pause) => {
                pause_intent = true;
                if let Some(r) = &running {
                    r.pcm.paused.store(true, Ordering::Relaxed);
                }
            }
            Ok(Command::Resume) => {
                pause_intent = false;
                if let Some(r) = &running {
                    r.pcm.paused.store(false, Ordering::Relaxed);
                }
            }
            Ok(Command::Seek { epoch, offset }) => {
                if !control.current(epoch) {
                    continue;
                }
                active = epoch;
                let Some(source) = &source else {
                    publish(&state, &control, active, PlaybackState::Stopped, (0.0, 0.0));
                    continue;
                };
                let offset = seek_position(offset, source.duration);
                let paused = running
                    .as_ref()
                    .map_or(pause_intent, |r| r.pcm.paused.load(Ordering::Relaxed));
                let cached = running
                    .as_ref()
                    .is_some_and(|r| r.pcm.seek_cached(offset, r.offset, r.duration));
                let disk_cached = if !cached {
                    if let Some(running) = &mut running {
                        match running.seek_disk(source, offset, paused, control.clone()) {
                            Ok(cached) => cached,
                            Err(error) => {
                                running.pcm.read_failed.store(true, Ordering::Relaxed);
                                publish(
                                    &state,
                                    &control,
                                    active,
                                    PlaybackState::Failed(error),
                                    (offset, offset),
                                );
                                continue;
                            }
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !cached && !disk_cached {
                    running = None;
                    publish(
                        &state,
                        &control,
                        active,
                        if paused {
                            PlaybackState::Paused
                        } else {
                            PlaybackState::Loading
                        },
                        (offset, offset),
                    );
                    match start(
                        source,
                        offset,
                        volume.clone(),
                        paused,
                        control.clone(),
                        cache.as_ref(),
                    ) {
                        Ok(new) => running = Some(new),
                        Err(e) => publish(
                            &state,
                            &control,
                            active,
                            PlaybackState::Failed(e),
                            (offset, offset),
                        ),
                    }
                }
            }
            Ok(Command::Stop(epoch)) => {
                if !control.current_source(epoch) {
                    continue;
                }
                active = epoch;
                running = None;
                source = None;
                pause_intent = false;
                publish(&state, &control, active, PlaybackState::Stopped, (0.0, 0.0));
            }
            Ok(Command::NormalizationChanged) => {
                if let Some(source) = &source
                    && let Some(running) = &running
                {
                    source.loudness.analyze_window(&running.pcm, running.offset);
                    if let Some(cache) = &running.cache {
                        source.loudness.scan_cache(cache, control.clone());
                    }
                }
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => (),
        }
        if !control.current_source(active) {
            running = None;
            continue;
        }
        let Some(r) = &mut running else {
            continue;
        };
        let (position, buffered_until, ready) = r.progress();
        let failure = if r.pcm.device_failed.load(Ordering::Relaxed) {
            Some(PlaybackError::Device)
        } else if r.pcm.read_failed.load(Ordering::Relaxed) {
            Some(
                r.pump
                    .as_ref()
                    .and_then(Pump::failure)
                    .unwrap_or(PlaybackError::Decoder),
            )
        } else {
            None
        };
        if let Some(e) = failure {
            running = None;
            publish(
                &state,
                &control,
                active,
                PlaybackState::Failed(e),
                (position, position),
            );
            continue;
        }
        let finished = r.pcm.eof.load(Ordering::Acquire) && !ready;
        let started = r.pcm.started.load(Ordering::Relaxed);
        if finished {
            // Keep EOF history and stream so backward seek can reuse decoded PCM.
            publish(
                &state,
                &control,
                active,
                completion_state(
                    started,
                    r.pcm.paused.load(Ordering::Relaxed),
                    r.offset,
                    position,
                    r.duration,
                ),
                (position, position),
            );
        } else {
            let s = if r.pcm.paused.load(Ordering::Relaxed) {
                PlaybackState::Paused
            } else if started && ready && r.pcm.is_primed() {
                PlaybackState::Playing
            } else {
                PlaybackState::Loading
            };
            publish(&state, &control, active, s, (position, buffered_until));
        }
    }
    drop(running);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn seed_pcm(pcm: &Pcm, start: u64, samples: impl IntoIterator<Item = f32>) {
        let bytes: Vec<_> = samples.into_iter().flat_map(f32::to_le_bytes).collect();
        let mut window = pcm.window.lock().unwrap();
        window.start_frame = start;
        window.end_frame = start;
        pcm.start_frame.store(start, Ordering::Release);
        pcm.end_frame.store(start, Ordering::Release);
        let epoch = pcm.cursor.load(Ordering::Acquire) & !PCM_FRAME_MASK;
        pcm.cursor.store(epoch | start, Ordering::Release);
        window.append(pcm, &bytes);
        pcm.primed.store(epoch | 1, Ordering::Release);
    }
    fn retained_samples(pcm: &Pcm) -> Vec<f32> {
        let window = pcm.window.lock().unwrap();
        let frames = pcm.capacity / pcm.channels;
        (window.start_frame..window.end_frame)
            .flat_map(|frame| {
                let slot = frame as usize % frames * pcm.channels;
                (0..pcm.channels).map(move |channel| {
                    f32::from_bits(pcm.samples[slot + channel].load(Ordering::Relaxed))
                })
            })
            .collect()
    }
    fn read_frame(pcm: &Pcm) -> u64 {
        pcm.cursor.load(Ordering::Acquire) & PCM_FRAME_MASK
    }
    fn pcm(channels: u16) -> Pcm {
        Pcm::new(
            channels,
            48_000,
            Arc::new(AtomicU32::new(0.5_f32.to_bits())),
            false,
        )
    }
    fn numbered_pcm() -> Pcm {
        let p = Pcm::new(2, 10, Arc::new(AtomicU32::new(1.0_f32.to_bits())), false);
        seed_pcm(
            &p,
            0,
            (0..20).flat_map(|i| [i as f32 / 20.0, -(i as f32) / 20.0]),
        );
        p
    }
    #[test]
    fn callback_scales_frames_and_freezes_when_paused_or_empty() {
        let p = pcm(2);
        seed_pcm(&p, 0, [0.8, -0.4, 0.2]);
        let mut output = [9.0; 6];
        render(&mut output, &p);
        assert_eq!(output, [0.4, -0.2, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(read_frame(&p), 1);
        p.paused.store(true, Ordering::Relaxed);
        render(&mut output, &p);
        assert_eq!(output, [0.0; 6]);
        assert_eq!(read_frame(&p), 1);
        assert!((p.buffered_range(12.0, 60).0 - (12.0 + 1.0 / 48_000.0)).abs() < 1e-9);
    }
    #[test]
    fn callback_does_not_drop_ready_audio_when_producer_metadata_is_locked() {
        let p = pcm(2);
        seed_pcm(&p, 0, [0.8, -0.4, 0.2, -0.2]);
        let _guard = p.window.lock().unwrap();
        let mut output = [1.0; 4];
        render(&mut output, &p);
        assert_eq!(output, [0.4, -0.2, 0.1, -0.1]);
        assert_eq!(read_frame(&p), 2);
        assert_eq!(p.underruns.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn startup_and_rebuffer_wait_for_half_a_second_but_short_eof_plays_immediately() {
        let p = Pcm::new(2, 100, Arc::new(AtomicU32::new(1.0_f32.to_bits())), false);
        let bytes = |frames: usize| {
            vec![0.25_f32; frames * 2]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
        };
        assert!(push_pcm(&bytes(10), &p, &AtomicBool::new(false)));
        let mut output = [1.0; 20];
        render(&mut output, &p);
        assert_eq!(output, [0.0; 20]);
        assert_eq!(read_frame(&p), 0);
        assert_eq!(p.underruns.load(Ordering::Relaxed), 0);
        assert!(push_pcm(&bytes(40), &p, &AtomicBool::new(false)));
        for _ in 0..5 {
            render(&mut output, &p);
            assert_eq!(output, [0.25; 20]);
        }
        render(&mut output, &p);
        assert_eq!(output, [0.0; 20]);
        assert_eq!(p.underruns.load(Ordering::Relaxed), 1);
        assert!(push_pcm(&bytes(10), &p, &AtomicBool::new(false)));
        render(&mut output, &p);
        assert_eq!(output, [0.0; 20]);
        p.eof.store(true, Ordering::Release);
        render(&mut output, &p);
        assert_eq!(output, [0.25; 20]);
        assert_eq!(read_frame(&p), 60);
        assert_eq!(p.underruns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cached_same_position_seek_and_refill_reject_old_callback_cursor_commits() {
        let p = numbered_pcm();
        let old = p.cursor.load(Ordering::Acquire);
        assert!(p.seek_cached(10.0, 10.0, 60));
        let seek = p.cursor.load(Ordering::Acquire);
        assert_ne!(seek & !PCM_FRAME_MASK, old & !PCM_FRAME_MASK);
        assert_eq!(seek & PCM_FRAME_MASK, old & PCM_FRAME_MASK);
        assert!(
            p.cursor
                .compare_exchange(old, old + 2, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        );
        let bytes: Vec<_> = [0.3_f32; 8]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        prefill(&p, &bytes, true);
        p.mark_primed(old);
        assert!(
            p.is_primed(),
            "old callbacks must not erase the new seek's priming token"
        );
        assert!(
            p.cursor
                .compare_exchange(seek, seek + 2, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        );
        let mut output = [0.0; 4];
        render(&mut output, &p);
        assert_eq!(output, [0.3; 4]);
        assert_eq!(read_frame(&p), 2);
    }

    #[test]
    fn pipewire_quantum_stays_continuous_during_producer_and_controller_contention() {
        use std::sync::{Barrier, atomic::AtomicUsize};
        let p = Arc::new(Pcm::new(
            2,
            48_000,
            Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            false,
        ));
        let initial: Vec<_> = vec![0.25_f32; 48_000 * 2 * 8]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        prefill(&p, &initial, false);
        let barrier = Arc::new(Barrier::new(3));
        let done = Arc::new(AtomicBool::new(false));
        let holds = Arc::new(AtomicUsize::new(0));
        let writer_pcm = p.clone();
        let writer_barrier = barrier.clone();
        let writer = thread::spawn(move || {
            let bytes: Vec<_> = vec![0.25_f32; 48_000 * 2 * 8]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect();
            writer_barrier.wait();
            assert!(push_pcm(&bytes, &writer_pcm, &AtomicBool::new(false)));
        });
        let observer_pcm = p.clone();
        let observer_barrier = barrier.clone();
        let observer_done = done.clone();
        let observer_holds = holds.clone();
        let observer = thread::spawn(move || {
            observer_barrier.wait();
            while !observer_done.load(Ordering::Acquire) {
                let guard = observer_pcm.window.lock().unwrap();
                observer_holds.fetch_add(1, Ordering::Relaxed);
                thread::sleep(Duration::from_micros(100));
                drop(guard);
                let _ = observer_pcm.buffered_range(0.0, 30);
                thread::yield_now();
            }
        });
        barrier.wait();
        wait_until(|| holds.load(Ordering::Relaxed) > 0);
        let mut output = [0.0; 1024 * 2];
        for quantum in 0..300 {
            render(&mut output, &p);
            assert!(
                output.iter().all(|sample| *sample == 0.25),
                "artificial gap at quantum {quantum}"
            );
            if quantum % 7 == 0 {
                thread::yield_now();
            }
        }
        done.store(true, Ordering::Release);
        observer.join().unwrap();
        writer.join().unwrap();
        assert_eq!(read_frame(&p), 300 * 1024);
        assert_eq!(p.underruns.load(Ordering::Relaxed), 0);
        assert!(holds.load(Ordering::Relaxed) > 0);
        assert!(p.samples.len() * size_of::<AtomicU32>() <= MAX_PCM_BYTES);
    }

    #[test]
    fn atomic_history_wrap_keeps_exact_frame_order_across_a_cached_backward_seek() {
        let p = Arc::new(Pcm::new(
            2,
            100,
            Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            false,
        ));
        let value = |frame: u64| 0.125 + (frame % 8) as f32 / 16.0;
        let bytes = |start: u64, end: u64| {
            (start..end)
                .flat_map(|frame| [value(frame), -value(frame)])
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
        };
        prefill(&p, &bytes(0, 1000), false);
        let writer_pcm = p.clone();
        let writer = thread::spawn(move || {
            let bytes: Vec<_> = (1000_u64..10_000)
                .flat_map(|frame| {
                    let value = 0.125 + (frame % 8) as f32 / 16.0;
                    [value, -value]
                })
                .flat_map(f32::to_le_bytes)
                .collect();
            assert!(push_pcm(&bytes, &writer_pcm, &AtomicBool::new(false)));
            writer_pcm.eof.store(true, Ordering::Release);
        });
        let mut sought = false;
        loop {
            let read = read_frame(&p);
            if read == 10_000 {
                break;
            }
            let count = (10_000 - read).min(32) as usize;
            wait_until(|| p.end_frame.load(Ordering::Acquire).saturating_sub(read) >= count as u64);
            let mut output = vec![0.0; count * 2];
            render(&mut output, &p);
            for (index, frame) in output.as_chunks::<2>().0.iter().enumerate() {
                assert_eq!(
                    *frame,
                    [value(read + index as u64), -value(read + index as u64)]
                );
            }
            if !sought && read_frame(&p) >= 5_000 {
                let target = read_frame(&p) - 100;
                assert!(p.seek_cached(target as f64 / 100.0, 0.0, 100));
                sought = true;
            }
        }
        writer.join().unwrap();
        assert!(sought);
        assert_eq!(p.underruns.load(Ordering::Relaxed), 0);
        assert!(p.start_frame.load(Ordering::Acquire) > p.capacity as u64 / p.channels as u64);
    }

    #[test]
    fn cached_forward_and_backward_seeks_reuse_exact_pcm_samples() {
        let p = numbered_pcm();
        let mut output = [0.0; 4];
        render(&mut output, &p);
        assert_eq!(output, [0.0, 0.0, 0.05, -0.05]);
        assert!(p.seek_cached(11.0, 10.0, 60));
        render(&mut output, &p);
        assert_eq!(output, [0.5, -0.5, 0.55, -0.55]);
        assert_eq!(p.buffered_range(10.0, 60), (11.2, 12.0, true));
        assert!(p.seek_cached(10.1, 10.0, 60));
        render(&mut output, &p);
        assert_eq!(output, [0.05, -0.05, 0.1, -0.1]);
        assert_eq!(retained_samples(&p).len(), 40);
        assert!(p.started.load(Ordering::Relaxed));
    }
    #[test]
    fn cached_seek_preserves_pause_and_finite_eof_boundary() {
        let p = numbered_pcm();
        p.paused.store(true, Ordering::Relaxed);
        p.eof.store(true, Ordering::Release);
        assert!(p.seek_cached(11.5, 10.0, 12));
        let mut output = [1.0; 4];
        render(&mut output, &p);
        assert_eq!(output, [0.0; 4]);
        assert_eq!(p.buffered_range(10.0, 12), (11.5, 12.0, true));
        assert!(p.eof.load(Ordering::Acquire));
        assert!(p.seek_cached(12.0, 10.0, 12));
        assert_eq!(p.buffered_range(10.0, 12), (12.0, 12.0, false));
        assert!(p.seek_cached(10.0, 10.0, 12));
        assert!(!p.seek_cached(9.9, 10.0, 12));
        assert!(!p.seek_cached(12.2, 10.0, 60));
        assert_eq!(
            completion_state(false, false, 10.0, 12.0, 12),
            PlaybackState::Ended
        );
        assert_eq!(
            completion_state(false, false, 10.0, 10.0, 60),
            PlaybackState::Failed(PlaybackError::Decoder)
        );
        assert_eq!(
            completion_state(false, true, 10.0, 12.0, 12),
            PlaybackState::Paused
        );
    }
    #[test]
    fn eviction_preserves_cursor_and_future_after_backward_seek() {
        let p = Pcm {
            history_frames: 5,
            ahead_capacity: 30,
            capacity: 40,
            ..numbered_pcm()
        };
        {
            let mut window = p.window.lock().unwrap();
            p.cursor.store(18, Ordering::Release);
            assert_eq!(window.writable(&p), 26);
            assert_eq!(
                (window.start_frame, read_frame(&p), window.end_frame),
                (13, 18, 20)
            );
            p.cursor.store(13, Ordering::Release);
            assert_eq!(window.writable(&p), 16);
            assert_eq!(window.start_frame, 13);
        }
        assert!(!p.seek_cached(11.2, 10.0, 60));
        assert!(p.seek_cached(11.3, 10.0, 60));
    }
    #[test]
    fn buffered_endpoint_counts_complete_frames_and_retains_history() {
        let p = pcm(2);
        seed_pcm(&p, 48_000, std::iter::repeat_n(0.2, 9_601));
        assert_eq!(p.buffered_range(10.0, 60), (11.0, 11.1, true));
        let mut output = [0.0; 960];
        render(&mut output, &p);
        assert_eq!(p.buffered_range(10.0, 60), (11.01, 11.1, true));
        p.paused.store(true, Ordering::Relaxed);
        render(&mut output, &p);
        assert_eq!(p.buffered_range(10.0, 60), (11.01, 11.1, true));
        assert_eq!(p.buffered_range(10.0, 11), (11.0, 11.0, true));
        assert_eq!(retained_samples(&p).len(), 9_600);
    }
    #[test]
    fn capacity_targets_fifteen_seconds_each_way_with_absolute_limit() {
        let p = pcm(2);
        assert_eq!(p.ahead_capacity, 48_000 * 2 * 15);
        assert_eq!(p.history_frames, 48_000 * 15);
        assert_eq!(p.capacity, 48_000 * 2 * 30);
        let surround = Pcm::new(8, 192_000, p.volume.clone(), false);
        assert!(surround.capacity * size_of::<f32>() <= MAX_PCM_BYTES);
        assert_eq!(surround.capacity % 8, 0);
        assert_eq!(surround.buffered_range(45.0, 120), (45.0, 45.0, false));
    }
    #[test]
    fn seek_revision_suppresses_old_end_without_invalidating_source() {
        let c = Control::default();
        let source = c.new_source();
        let seek = c.seek();
        assert!(c.current_source(source));
        assert!(!c.current(source));
        assert!(c.current(seek));
        let state = Mutex::new(Snapshot {
            state: PlaybackState::Paused,
            position: 5.0,
            buffered_until: 20.0,
            ..Snapshot::default()
        });
        publish(&state, &c, source, PlaybackState::Ended, (10.0, 10.0));
        assert_eq!(state.lock().unwrap().state, PlaybackState::Paused);
        publish(&state, &c, seek, PlaybackState::Paused, (7.0, 20.0));
        assert_eq!(state.lock().unwrap().position, 7.0);
        c.new_source();
        assert!(!c.current_source(seek));
    }
    fn queued_player(snapshot: Snapshot, duration: Option<u32>) -> (Player, Receiver<Command>) {
        let (commands, receiver) = mpsc::channel();
        let player = Player {
            commands,
            state: Arc::new(Mutex::new(snapshot)),
            volume: Arc::new(AtomicU32::new(DEFAULT_VOLUME.to_bits())),
            normalize: Arc::new(AtomicBool::new(false)),
            normalization_profiles: VecDeque::new(),
            control: Arc::new(Control::default()),
            source_duration: duration,
            worker: None,
        };
        (player, receiver)
    }
    #[test]
    fn normalization_toggle_is_live_and_profiles_are_bounded_and_account_bound() {
        let (mut player, receiver) = queued_player(Snapshot::default(), None);
        assert!(!player.normalize_volume());
        player.set_normalize_volume(true);
        assert!(player.normalize_volume());
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::NormalizationChanged)
        ));
        player.set_normalize_volume(true);
        assert!(receiver.try_recv().is_err());
        let key = cache_key(2);
        player.play_cached(key, None, 0.0);
        let first = match receiver.try_recv().unwrap() {
            Command::Play { source, .. } => source.loudness,
            _ => panic!("Expected play"),
        };
        player.play_cached(key, None, 0.5);
        let repeated = match receiver.try_recv().unwrap() {
            Command::Play { source, .. } => source.loudness,
            _ => panic!("Expected play"),
        };
        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(first.enabled());
        player.set_normalize_volume(false);
        assert!(!first.enabled());
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::NormalizationChanged)
        ));
        let other = CacheKey {
            account_id: key.account_id + 1,
            ..key
        };
        player.play_cached(other, None, 0.0);
        let distinct = match receiver.try_recv().unwrap() {
            Command::Play { source, .. } => source.loudness,
            _ => panic!("Expected play"),
        };
        assert!(!Arc::ptr_eq(&first, &distinct));
        for id in 1..40 {
            player.play_cached(
                CacheKey {
                    track_id: id,
                    ..key
                },
                None,
                0.0,
            );
            assert!(matches!(receiver.try_recv(), Ok(Command::Play { .. })));
        }
        assert_eq!(player.normalization_profiles.len(), 32);
        assert_eq!(player.control.decoder_starts.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn seek_publishes_the_requested_position_and_keeps_ready_buffer_metadata() {
        let (mut p, _receiver) = queued_player(
            Snapshot {
                state: PlaybackState::Paused,
                position: 5.0,
                buffered_until: 20.0,
                volume: DEFAULT_VOLUME,
            },
            Some(60),
        );
        p.seek(12.0);
        let s = p.snapshot();
        assert_eq!(s.state, PlaybackState::Paused);
        assert_eq!((s.position, s.buffered_until), (12.0, 20.0));
        p.stop();
        let s = p.snapshot();
        assert_eq!(s.state, PlaybackState::Stopped);
        assert_eq!((s.position, s.buffered_until), (0.0, 0.0));
    }
    #[test]
    fn queued_uncached_seeks_never_roll_the_requested_position_back_to_an_old_snapshot() {
        let (mut player, receiver) = queued_player(
            Snapshot {
                state: PlaybackState::Playing,
                position: 5.0,
                buffered_until: 20.0,
                ..Snapshot::default()
            },
            Some(60),
        );
        let old_epoch = Epoch::default();
        player.seek(45.0);
        let Command::Seek {
            epoch: forward_epoch,
            offset,
        } = receiver.try_recv().unwrap()
        else {
            panic!("Expected queued seek")
        };
        assert_eq!(offset, 45.0);
        assert_eq!(
            (player.snapshot().position, player.snapshot().buffered_until),
            (45.0, 45.0)
        );
        publish(
            &player.state,
            &player.control,
            old_epoch,
            PlaybackState::Playing,
            (5.1, 20.0),
        );
        assert_eq!(player.snapshot().position, 45.0);

        player.seek(3.0);
        let Command::Seek {
            epoch: backward_epoch,
            offset,
        } = receiver.try_recv().unwrap()
        else {
            panic!("Expected queued seek")
        };
        assert_eq!(offset, 3.0);
        publish(
            &player.state,
            &player.control,
            forward_epoch,
            PlaybackState::Loading,
            (45.0, 45.0),
        );
        assert_eq!(player.snapshot().position, 3.0);
        // The controller can acknowledge the latest target normally. It never
        // needs a UI timeout, and older completion packets cannot auto-advance.
        publish(
            &player.state,
            &player.control,
            backward_epoch,
            PlaybackState::Playing,
            (3.25, 18.0),
        );
        publish(
            &player.state,
            &player.control,
            forward_epoch,
            PlaybackState::Ended,
            (60.0, 60.0),
        );
        assert_eq!(player.snapshot().position, 3.25);
        assert_eq!(player.snapshot().state, PlaybackState::Playing);
    }
    #[test]
    fn immediate_seek_clamps_to_the_selected_source_and_stop_disables_late_seeks() {
        let (mut player, receiver) = queued_player(Snapshot::default(), None);
        player.seek(25.0);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(player.snapshot().position, 0.0);

        player.play("https://audio.example.test/first.mp3".into(), 60, 0.0);
        let Command::Play {
            epoch: first_source,
            ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("Expected queued play")
        };
        player.seek(100.0);
        let Command::Seek {
            epoch: first_seek,
            offset,
        } = receiver.try_recv().unwrap()
        else {
            panic!("Expected queued seek")
        };
        assert_eq!(offset, 60.0);
        publish(
            &player.state,
            &player.control,
            first_source,
            PlaybackState::Loading,
            (0.0, 0.0),
        );
        assert_eq!(player.snapshot().position, 60.0);

        player.play("https://audio.example.test/second.mp3".into(), 10, 0.0);
        let Command::Play { .. } = receiver.try_recv().unwrap() else {
            panic!("Expected queued play")
        };
        publish(
            &player.state,
            &player.control,
            first_seek,
            PlaybackState::Playing,
            (60.0, 60.0),
        );
        assert_eq!(player.snapshot().position, 0.0);
        for (requested, expected) in [(100.0, 10.0), (-1.0, 0.0), (f64::NAN, 0.0)] {
            player.seek(requested);
            let Command::Seek { offset, .. } = receiver.try_recv().unwrap() else {
                panic!("Expected queued seek")
            };
            assert_eq!(offset, expected);
            assert_eq!(player.snapshot().position, expected);
        }
        player.stop();
        assert!(matches!(receiver.try_recv(), Ok(Command::Stop(_))));
        player.seek(4.0);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(player.snapshot().position, 0.0);
        assert_eq!(player.snapshot().state, PlaybackState::Stopped);
    }
    #[test]
    fn feeder_handles_partial_reads_and_invalid_samples() {
        struct ByteReader {
            bytes: VecDeque<u8>,
        }
        impl Read for ByteReader {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                match self.bytes.pop_front() {
                    Some(b) => {
                        out[0] = b;
                        Ok(1)
                    }
                    None => Ok(0),
                }
            }
        }
        let input = [0.25_f32, f32::NAN, -2.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let p = pcm(1);
        feed(ByteReader { bytes: input }, &p);
        assert_eq!(retained_samples(&p), [0.25, 0.0, -1.0]);
        assert!(p.eof.load(Ordering::Acquire));
        assert!(!p.read_failed.load(Ordering::Relaxed));
    }
    #[test]
    fn persistent_pcm_reopens_and_plays_without_url_or_decoder() {
        let directory = CacheDirectory::new();
        let key = cache_key(1);
        let bytes: Vec<_> = std::iter::repeat_n(0.25_f32, 48_000 * 2)
            .flat_map(f32::to_le_bytes)
            .collect();
        {
            let cache = AudioCache::test_at(directory.0.join("audio"), 4 * 1024 * 1024).unwrap();
            assert_eq!(cache.write_range(&key, 0, &bytes).unwrap(), 48_000);
            cache.mark_complete(&key, 48_000).unwrap();
        }
        let cache = AudioCache::test_at(directory.0.join("audio"), 4 * 1024 * 1024).unwrap();
        let source = Source {
            loudness: normalization::Profile::new(Arc::new(AtomicBool::new(false))),
            url: None,
            duration: key.duration,
            key: Some(key),
        };
        let pcm = pcm(2);
        let control = Control::default();
        let decoder_slot = Arc::new(Mutex::new(None));
        pump(
            &source,
            Some(&CacheBinding { cache, key }),
            0,
            &pcm,
            &AtomicBool::new(false),
            &decoder_slot,
            &control,
        )
        .unwrap();
        assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 0);
        assert!(decoder_slot.lock().unwrap().is_none());
        assert_eq!(retained_samples(&pcm).len(), 96_000);
        assert_eq!(pcm.buffered_range(0.0, 1), (0.0, 1.0, true));
        let mut output = [0.0; 4];
        render(&mut output, &pcm);
        assert_eq!(output, [0.125; 4]);
    }

    #[test]
    fn local_cache_gap_is_not_reported_as_complete_audio() {
        let directory = CacheDirectory::new();
        let cache = AudioCache::test_at(directory.0.join("audio"), 4 * 1024 * 1024).unwrap();
        let key = cache_key(5);
        let bytes: Vec<_> = std::iter::repeat_n(0.25_f32, 200)
            .flat_map(f32::to_le_bytes)
            .collect();
        cache.write_range(&key, 0, &bytes).unwrap();
        let source = Source {
            loudness: normalization::Profile::new(Arc::new(AtomicBool::new(false))),
            url: None,
            duration: key.duration,
            key: Some(key),
        };
        let pcm = pcm(2);
        let control = Control::default();
        let result = pump(
            &source,
            Some(&CacheBinding { cache, key }),
            0,
            &pcm,
            &AtomicBool::new(false),
            &Arc::new(Mutex::new(None)),
            &control,
        );
        assert_eq!(result, Err(PlaybackError::CacheMiss));
        assert!(!pcm.eof.load(Ordering::Acquire));
        assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 0);
        assert_eq!(retained_samples(&pcm).len(), 200);
    }

    #[test]
    fn disabled_disk_cache_does_not_prevent_decoded_pcm_from_playing() {
        let directory = CacheDirectory::new();
        let cache = AudioCache::test_at(directory.0.join("audio"), 0).unwrap();
        let key = cache_key(1);
        let bytes: Vec<_> = [0.2_f32, -0.2]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let binding = CacheBinding {
            cache: cache.clone(),
            key,
        };
        let mut writer = CacheWriter::new(Some(&binding));
        writer.append(0, &bytes);
        writer.flush();
        assert_eq!(cache.contiguous_end(&key, 0).unwrap(), 0);
        let pcm = pcm(2);
        assert!(push_pcm(&bytes, &pcm, &AtomicBool::new(false)));
        pcm.eof.store(true, Ordering::Release); // one-frame finite fixture
        let mut output = [0.0; 2];
        render(&mut output, &pcm);
        assert_eq!(output, [0.1, -0.1]);
    }

    struct SyntheticReader {
        remaining: usize,
        realtime: bool,
        cancellation: crate::media_decoder::Cancellation,
    }
    impl Read for SyntheticReader {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.cancellation.is_canceled() {
                return Err(std::io::Error::other("Canceled test source"));
            }
            let count = output.len().min(self.remaining);
            if self.realtime {
                let deadline =
                    std::time::Instant::now() + Duration::from_secs_f64(count as f64 / 384000.0);
                while std::time::Instant::now() < deadline {
                    if self.cancellation.is_canceled() {
                        return Err(std::io::Error::other("Canceled test source"));
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            }
            output[..count].fill(0);
            self.remaining -= count;
            Ok(count)
        }
    }
    fn synthetic_stream(
        seconds: f64,
        realtime: bool,
    ) -> std::io::Result<crate::media_decoder::Stream> {
        let cancellation = crate::media_decoder::Cancellation::default();
        Ok(crate::media_decoder::Stream {
            reader: Box::new(SyntheticReader {
                remaining: (frame_at(seconds) as usize) * FRAME_BYTES,
                realtime,
                cancellation: cancellation.clone(),
            }),
            cancellation,
        })
    }
    fn synthetic_decoder(_url: &str, offset: f64) -> std::io::Result<crate::media_decoder::Stream> {
        synthetic_stream((24.0 - offset).max(0.0), false)
    }
    fn realtime_synthetic_decoder(
        _url: &str,
        _offset: f64,
    ) -> std::io::Result<crate::media_decoder::Stream> {
        synthetic_stream(24.0, true)
    }
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "synthetic decoder did not finish"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn preload_fixture(directory: &CacheDirectory) -> (Source, CacheBinding, Arc<Control>) {
        let key = cache_key(24);
        let cache = AudioCache::test_at(directory.0.join("audio"), 16 * 1024 * 1024).unwrap();
        let source = Source {
            loudness: normalization::Profile::new(Arc::new(AtomicBool::new(false))),
            url: Some(Zeroizing::new("https://example.test/synthetic".into())),
            duration: 24,
            key: Some(key),
        };
        (
            source,
            CacheBinding { cache, key },
            Arc::new(Control::default()),
        )
    }

    #[test]
    fn full_track_prefetch_finishes_while_paused_without_growing_pcm_or_restarting_cached_seek() {
        let directory = CacheDirectory::new();
        let (source, cache, control) = preload_fixture(&directory);
        let prefetch =
            Prefetch::with_decoder(&source, &cache, 0, control.clone(), synthetic_decoder)
                .unwrap()
                .unwrap();
        let pcm = Arc::new(Pcm::new(
            2,
            48_000,
            Arc::new(AtomicU32::new(0.7_f32.to_bits())),
            true,
        ));
        let pump = Pump::new(
            source.clone(),
            Some(cache.clone()),
            0,
            pcm.clone(),
            control.clone(),
            Some(prefetch.clone()),
        )
        .unwrap();
        let end = 24 * u64::from(SAMPLE_RATE);
        wait_until(|| cache.complete() == Some(end));
        assert!(cache.cache.contains_range(&cache.key, 0, end).unwrap());
        let window = pcm.window.lock().unwrap();
        assert_eq!(read_frame(&pcm), 0);
        assert!(window.sample_len(&pcm) <= pcm.ahead_capacity);
        assert!(pcm.samples.len() * size_of::<AtomicU32>() <= MAX_PCM_BYTES);
        drop(window);
        assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 1);
        drop(pump);
        // This is the same disk-to-RAM pump replacement used by a far cached seek.
        let offset = 18.0;
        let bytes = cache.read(frame_at(offset));
        prefill(&pcm, &bytes, false);
        let pump = Pump::new(
            source,
            Some(cache.clone()),
            frame_at(offset) + (bytes.len() / FRAME_BYTES) as u64,
            pcm.clone(),
            control.clone(),
            Some(prefetch.clone()),
        )
        .unwrap();
        wait_until(|| pcm.eof.load(Ordering::Acquire));
        assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 1);
        assert_eq!(pcm.buffered_range(offset, 24), (18.0, 24.0, true));
        drop(pump);
        drop(prefetch);
    }

    #[test]
    fn seek_prefetch_prioritizes_the_requested_tail_then_backfills_a_complete_prefix() {
        let directory = CacheDirectory::new();
        let (source, cache, control) = preload_fixture(&directory);
        let prefetch = Prefetch::with_decoder(
            &source,
            &cache,
            frame_at(18.0),
            control.clone(),
            synthetic_decoder,
        )
        .unwrap()
        .unwrap();
        wait_until(|| cache.complete() == Some(24 * u64::from(SAMPLE_RATE)));
        assert!(
            cache
                .cache
                .contains_range(&cache.key, 0, frame_at(24.0))
                .unwrap()
        );
        // Tail and prefix use sequential decoders, never one per playback window.
        assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 2);
        assert!(prefetch.decoder_slot.lock().unwrap().is_none());
    }

    fn tail_then_slow_prefix_decoder(
        url: &str,
        offset: f64,
    ) -> std::io::Result<crate::media_decoder::Stream> {
        if offset > 0.0 {
            synthetic_decoder(url, offset)
        } else {
            realtime_synthetic_decoder(url, offset)
        }
    }
    fn tail_then_failed_prefix_decoder(
        url: &str,
        offset: f64,
    ) -> std::io::Result<crate::media_decoder::Stream> {
        if offset > 0.0 {
            synthetic_decoder(url, offset)
        } else {
            Err(std::io::Error::other("Synthetic decoding failure"))
        }
    }

    #[test]
    fn decoded_tail_finishes_before_a_slow_or_failed_prefix_backfill() {
        for decoder in [
            tail_then_slow_prefix_decoder as DecoderFactory,
            tail_then_failed_prefix_decoder,
        ] {
            let directory = CacheDirectory::new();
            let (source, cache, control) = preload_fixture(&directory);
            let prefetch =
                Prefetch::with_decoder(&source, &cache, frame_at(18.0), control.clone(), decoder)
                    .unwrap()
                    .unwrap();
            wait_until(|| prefetch.eof.load(Ordering::Acquire) == frame_at(24.0));
            let pcm = Arc::new(Pcm::new(
                2,
                48_000,
                Arc::new(AtomicU32::new(0.7_f32.to_bits())),
                true,
            ));
            let pump = Pump::new(
                source,
                Some(cache.clone()),
                frame_at(18.0),
                pcm.clone(),
                control,
                Some(prefetch.clone()),
            )
            .unwrap();
            wait_until(|| pcm.eof.load(Ordering::Acquire));
            assert!(!pcm.read_failed.load(Ordering::Acquire));
            assert!(pump.failure().is_none());
            assert!(cache.complete().is_none());
            assert_eq!(pcm.buffered_range(18.0, 24), (18.0, 24.0, true));
            prefetch.stop();
            assert!(pump.failure().is_none());
            assert!(cache.complete().is_none());
            assert!(prefetch.decoder_slot.lock().unwrap().is_none());
        }
    }

    #[test]
    fn canceled_prefetch_stops_native_decoding_and_never_marks_partial_audio_complete() {
        let directory = CacheDirectory::new();
        let (source, cache, control) = preload_fixture(&directory);
        let prefetch =
            Prefetch::with_decoder(&source, &cache, 0, control, realtime_synthetic_decoder)
                .unwrap()
                .unwrap();
        wait_until(|| cache.cache.contiguous_end(&cache.key, 0).unwrap() > 0);
        let stopped = std::time::Instant::now();
        prefetch.stop();
        assert!(stopped.elapsed() < Duration::from_secs(2));
        assert!(prefetch.decoder_slot.lock().unwrap().is_none());
        assert!(prefetch.reader.lock().unwrap().is_none());
        assert!(cache.complete().is_none());
        assert_eq!(prefetch.eof.load(Ordering::Acquire), 0);
        assert!(cache.cache.contiguous_end(&cache.key, 0).unwrap() < frame_at(24.0));
    }

    #[test]
    fn cache_policy_changes_reject_stale_prefetch_commits_and_small_budgets_stay_streaming() {
        for limit in [0, 64] {
            let directory = CacheDirectory::new();
            let (source, cache, control) = preload_fixture(&directory);
            cache.cache.set_limit(limit).unwrap();
            assert!(
                Prefetch::with_decoder(&source, &cache, 0, control.clone(), synthetic_decoder)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 0);
        }
        let directory = CacheDirectory::new();
        let (_, cache, _) = preload_fixture(&directory);
        let (_, revision) = cache.cache.policy().unwrap();
        let mut writer = CacheWriter::new(Some(&cache));
        writer.revision = Some(revision);
        assert!(writer.append(0, &[0; 64]));
        assert!(writer.flush());
        cache.cache.clear().unwrap();
        assert!(writer.append(8, &[0; 64]));
        assert!(!writer.flush());
        assert_eq!(cache.cache.contiguous_end(&cache.key, 0).unwrap(), 0);
        assert!(
            cache
                .cache
                .mark_complete_at_revision(&cache.key, 16, revision)
                .is_err()
        );
        assert!(cache.complete().is_none());
    }

    #[test]
    fn cleared_offline_cache_never_waits_for_a_finished_downloader() {
        let directory = CacheDirectory::new();
        let (mut source, cache, control) = preload_fixture(&directory);
        source.url = None;
        let (_, revision) = cache.cache.policy().unwrap();
        let prefetch = Arc::new(Prefetch {
            canceled: Arc::new(AtomicBool::new(false)),
            disabled: AtomicBool::new(false),
            decoder_slot: Arc::new(Mutex::new(None)),
            reader: Mutex::new(None),
            result: Arc::new(Mutex::new(Some(Ok(())))),
            changed: Arc::new(Condvar::new()),
            eof: Arc::new(AtomicU64::new(0)),
            revision,
        });
        cache.cache.clear().unwrap();
        let pcm = Arc::new(pcm(2));
        let pump = Pump::new(
            source,
            Some(cache),
            0,
            pcm.clone(),
            control.clone(),
            Some(prefetch.clone()),
        )
        .unwrap();
        wait_until(|| pcm.read_failed.load(Ordering::Acquire));
        assert_eq!(pump.failure(), Some(PlaybackError::CacheMiss));
        assert!(prefetch.disabled.load(Ordering::Acquire));
        assert_eq!(control.decoder_starts.load(Ordering::Relaxed), 0);
    }

    fn short_synthetic_decoder(
        _url: &str,
        _offset: f64,
    ) -> std::io::Result<crate::media_decoder::Stream> {
        synthetic_stream(1.0, false)
    }
    #[test]
    fn premature_successful_eof_keeps_partial_audio_without_poisoning_full_cache() {
        let directory = CacheDirectory::new();
        let (source, cache, control) = preload_fixture(&directory);
        let prefetch =
            Prefetch::with_decoder(&source, &cache, 0, control.clone(), short_synthetic_decoder)
                .unwrap()
                .unwrap();
        wait_until(|| prefetch.result.lock().unwrap().is_some());
        assert_eq!(
            prefetch.result.lock().unwrap().clone(),
            Some(Err(PlaybackError::Decoder))
        );
        assert!(cache.cache.contiguous_end(&cache.key, 0).unwrap() > 0);
        assert!(cache.cache.complete_frames(&cache.key).unwrap().is_none());
        assert_eq!(prefetch.eof.load(Ordering::Acquire), 0);
        drop(prefetch);
        // The ordinary bounded-streaming path applies the same validation.
        let pcm = pcm(2);
        let status = pump_mode(
            &source,
            Some(&cache),
            0,
            Some(&pcm),
            &AtomicBool::new(false),
            &Arc::new(Mutex::new(None)),
            &control,
            None,
            None,
            None,
            short_synthetic_decoder,
        );
        assert_eq!(status, Err(PlaybackError::Decoder));
        assert!(cache.cache.complete_frames(&cache.key).unwrap().is_none());
        assert!(!completed_duration_plausible(frame_at(1.0), 24));
        assert!(completed_duration_plausible(frame_at(23.0), 24));
        assert!(completed_duration_plausible(frame_at(1.0), 0));
        assert!(!completed_duration_plausible(0, 0));
    }

    struct CacheDirectory(std::path::PathBuf);
    impl CacheDirectory {
        fn new() -> Self {
            let mut suffix = [0_u8; 12];
            getrandom::fill(&mut suffix).unwrap();
            let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("vk-fast-playback-disk-{suffix}"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for CacheDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn cache_key(duration: u32) -> CacheKey {
        CacheKey {
            account_id: 42,
            owner_id: 42,
            track_id: 7,
            duration,
        }
    }

    #[test]
    fn playback_url_and_seek_limits_are_enforced() {
        assert!(!valid_url("file:///etc/passwd"));
        assert!(!valid_url("https://user:password@example.com/a"));
        assert!(!valid_url("https://example.com:8443/a"));
        assert!(valid_url("https://example.com/a.m3u8"));
        assert_eq!(seek_position(f64::NAN, 60), 0.0);
        assert_eq!(seek_position(75.0, 60), 60.0);
        assert_eq!(seek_position(-5.0, 60), 0.0);
    }
}
