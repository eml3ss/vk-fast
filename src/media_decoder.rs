//! Native Rust decoding, transport and sample conversion. PCM remains 48 kHz
//! stereo f32 so complete existing audio caches remain directly reusable.
use std::{
    io::{self, Cursor, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use symphonia::core::{
    codecs::audio::{AudioDecoder, AudioDecoderOptions},
    errors::Error as DecodeError,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType, probe::Hint},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    units::Time,
};
use zeroize::Zeroizing;
mod convert;
mod hls;
mod net;
mod source;
mod transport;
use net::{canceled, invalid};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}
impl Cancellation {
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
pub struct Stream {
    pub reader: Box<dyn Read + Send>,
    pub cancellation: Cancellation,
}

pub fn open(url: &str, offset: f64) -> io::Result<Stream> {
    open_stored(Some(url), offset, None)
}
/// Decode a track whose original file the cache keeps (or fills): a file
/// stored whole plays from disk without `url`, otherwise chunks come from
/// disk where stored and from VK where not, and are stored as they arrive.
pub fn open_stored(
    url: Option<&str>,
    offset: f64,
    store: Option<crate::audio_cache::MediaStore>,
) -> io::Result<Stream> {
    if !offset.is_finite() || offset < 0.0 {
        return Err(invalid());
    }
    if let Some(url) = url
        && !net::valid_url(&reqwest::Url::parse(url).map_err(|_| invalid())?)
    {
        return Err(invalid());
    }
    if url.is_none() && store.is_none() {
        return Err(invalid());
    }
    let cancellation = Cancellation::default();
    Ok(Stream {
        reader: Box::new(Lazy {
            url: url.map(|url| Zeroizing::new(url.into())),
            offset,
            canceled: cancellation.0.clone(),
            pipeline: None,
            store,
        }),
        cancellation,
    })
}
/// Fetch the chunks of `url`'s file that the store lacks, on this thread,
/// until done, canceled, or the cache has no room or was changed.
pub fn download(
    url: &str,
    store: &crate::audio_cache::MediaStore,
    cancellation: &Cancellation,
) -> io::Result<()> {
    if !net::valid_url(&reqwest::Url::parse(url).map_err(|_| invalid())?) {
        return Err(invalid());
    }
    source::download(net::Net::new(cancellation.0.clone())?, url, store)
}
struct Lazy {
    url: Option<Zeroizing<String>>,
    offset: f64,
    canceled: Arc<AtomicBool>,
    pipeline: Option<Pipeline>,
    store: Option<crate::audio_cache::MediaStore>,
}
impl Read for Lazy {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.canceled.load(Ordering::Acquire) {
            return Err(canceled());
        }
        if self.pipeline.is_none() {
            let stored = self
                .store
                .clone()
                .and_then(|store| source::stored(store, self.offset));
            let (source, origin) = match (stored, &self.url) {
                (Some(stored), _) => (stored, 0.0),
                (None, Some(url)) => {
                    source::open(url, self.offset, self.canceled.clone(), self.store.clone())?
                }
                (None, None) => {
                    crate::diagnostics::event("decoder", "cache_incomplete_without_url");
                    return Err(invalid());
                }
            };
            let pipeline = Pipeline::new(source, origin, self.offset, self.canceled.clone())?;
            self.pipeline = Some(pipeline);
        }
        let pipeline = self.pipeline.as_mut().ok_or_else(|| invalid())?;
        pipeline.read(out)
    }
}

struct Pipeline {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    time_base: Option<symphonia::core::units::TimeBase>,
    origin: f64,
    target: u64,
    position: Option<i64>,
    converter: Option<convert::Converter>,
    bytes: Cursor<Vec<u8>>,
    canceled: Arc<AtomicBool>,
    eof: bool,
    failures: usize,
    packets: u64,
    skipped: u64,
}
impl Pipeline {
    fn new(
        input: Box<dyn MediaSource>,
        origin: f64,
        offset: f64,
        canceled: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let seekable = input.is_seekable();
        let mss = MediaSourceStream::new(input, Default::default());
        let options = FormatOptions::default();
        let mut format = symphonia::default::get_probe()
            .probe(&Hint::new(), mss, options, MetadataOptions::default())
            .map_err(map_error)?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| invalid())?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| invalid())?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(map_error)?;
        let id = track.id;
        let time_base = track.time_base;
        let length = time_base
            .zip(track.duration)
            .and_then(|(base, duration)| base.calc_duration(duration))
            .map_or_else(|| "unknown".into(), |t| format!("{:.2}", t.as_secs_f64()));
        crate::diagnostics::event(
            "decoder",
            &format!(
                "format {} codec={} rate_khz={} channels={} length_s={length} delay={} padding={} \
                 seekable={seekable} origin_s={origin:.2} offset_s={offset:.2}",
                format.format_info().short_name,
                decoder.codec_info().short_name,
                params.sample_rate.map_or_else(
                    || "unknown".into(),
                    |rate| format!("{:.3}", f64::from(rate) / 1000.0)
                ),
                params.channels.as_ref().map_or(0, |c| c.count()),
                track.delay.unwrap_or(0),
                track.padding.unwrap_or(0),
            ),
        );
        if seekable && offset > 2.0 {
            let seeked = format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time: Time::try_from_secs_f64(offset - 2.0).ok_or_else(|| invalid())?,
                        track_id: Some(id),
                    },
                )
                .map_err(map_error)
                .inspect_err(|error| {
                    crate::diagnostics::event("decoder", &format!("seek_failed reason={error}"));
                })?;
            let landed = time_base
                .and_then(|base| base.calc_time(seeked.actual_ts))
                .map_or(-1.0, |t| t.as_secs_f64());
            crate::diagnostics::event(
                "decoder",
                &format!("seek to_s={:.2} landed_s={landed:.2}", offset - 2.0),
            );
        }
        Ok(Self {
            format,
            decoder,
            track: id,
            time_base,
            origin,
            target: (offset * 48000.0).floor() as u64,
            position: None,
            converter: None,
            bytes: Cursor::new(Vec::new()),
            canceled,
            eof: false,
            failures: 0,
            packets: 0,
            skipped: 0,
        })
    }
    fn chunk(&mut self) -> io::Result<bool> {
        loop {
            if self.canceled.load(Ordering::Acquire) {
                return Err(canceled());
            }
            if self.eof {
                return Ok(false);
            }
            let packet = self
                .format
                .next_packet()
                .map_err(map_error)
                .inspect_err(|error| self.log_failure("demux", error))?;
            let Some(packet) = packet else {
                self.eof = true;
                crate::diagnostics::event(
                    "decoder",
                    &format!(
                        "stream_end at_s={:.2} packets={} skipped={}",
                        self.seconds(),
                        self.packets,
                        self.skipped
                    ),
                );
                let tail = self
                    .converter
                    .as_mut()
                    .map(|c| c.finish())
                    .transpose()?
                    .unwrap_or_default();
                return self.publish(tail);
            };
            if packet.track_id != self.track {
                continue;
            }
            self.packets += 1;
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.failures = 0;
                    decoded
                }
                Err(DecodeError::DecodeError(reason)) if self.failures < 32 => {
                    self.failures += 1;
                    self.skipped += 1;
                    // The first few name the reason; a long broken run is
                    // summed up at the end of the stream instead.
                    if self.skipped <= 5 {
                        crate::diagnostics::event(
                            "decoder",
                            &format!(
                                "packet_skipped at_s={:.2} reason={reason} in_a_row={}",
                                self.seconds(),
                                self.failures
                            ),
                        );
                    }
                    continue;
                }
                Err(error) => {
                    let error = map_error(error);
                    self.log_failure("decode", &error);
                    return Err(error);
                }
            };
            if decoded.frames() > 1024 * 1024 || decoded.spec().channels().count() > 32 {
                let at = self.position.unwrap_or(0) as f64 / 48000.0;
                crate::diagnostics::event(
                    "decoder",
                    &format!(
                        "packet_too_large at_s={at:.2} frames={} channels={}",
                        decoded.frames(),
                        decoded.spec().channels().count()
                    ),
                );
                return Err(invalid());
            }
            let spec = decoded.spec().clone();
            let mut samples = vec![0.0f32; decoded.samples_interleaved()];
            decoded.copy_to_slice_interleaved(&mut samples);
            if self.converter.as_ref().is_none_or(|c| c.spec() != &spec) {
                if let Some(converter) = &self.converter {
                    crate::diagnostics::event(
                        "decoder",
                        &format!(
                            "format_changed at_s={:.2} rate_khz={:.3}->{:.3} channels={}->{}",
                            self.seconds(),
                            f64::from(converter.spec().rate()) / 1000.0,
                            f64::from(spec.rate()) / 1000.0,
                            converter.spec().channels().count(),
                            spec.channels().count()
                        ),
                    );
                    return Err(invalid());
                }
                self.converter = Some(convert::Converter::new(&spec, 48000)?);
            }
            if self.position.is_none() {
                let time = self
                    .time_base
                    .and_then(|tb| tb.calc_time(packet.pts))
                    .map_or(0.0, |t| t.as_secs_f64());
                // The codec has already removed priming samples. Packet PTS
                // still includes them, so the valid buffer starts after trim.
                let time =
                    self.origin + time + packet.trim_start.get() as f64 / f64::from(spec.rate());
                let frame = (time * f64::from(spec.rate())).round() as i64;
                let mut a = spec.rate();
                let mut b = 48000;
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                let period = spec.rate() / a;
                let padding = frame.rem_euclid(i64::from(period)) as usize;
                self.position = Some(
                    ((i128::from(frame) - padding as i128) * 48000 / i128::from(spec.rate()))
                        as i64,
                );
                if padding > 0 {
                    let prefix = self
                        .converter
                        .as_mut()
                        .ok_or_else(|| invalid())?
                        .push(&vec![0.0; padding * spec.channels().count()])?;
                    self.publish(prefix)?;
                }
            }
            let converted = self
                .converter
                .as_mut()
                .ok_or_else(|| invalid())?
                .push(&samples)?;
            if self.publish(converted)? {
                return Ok(true);
            }
        }
    }
    /// Decoded position on the track timeline, for the log.
    fn seconds(&self) -> f64 {
        self.position.unwrap_or(0) as f64 / 48000.0
    }
    fn log_failure(&self, stage: &str, error: &io::Error) {
        crate::diagnostics::event(
            "decoder",
            &format!(
                "{stage}_failed at_s={:.2} packets={} skipped={} kind={:?} reason={error}",
                self.seconds(),
                self.packets,
                self.skipped,
                error.kind()
            ),
        );
    }
    fn publish(&mut self, samples: Vec<f32>) -> io::Result<bool> {
        let position = self.position.unwrap_or(0);
        let frames = samples.len() / 2;
        self.position = Some(position.saturating_add(frames as i64));
        let skip = (self.target as i128 - i128::from(position)).clamp(0, frames as i128) as usize;
        if skip == frames {
            return Ok(false);
        }
        let mut bytes = Vec::with_capacity((frames - skip) * 8);
        for sample in &samples[skip * 2..] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.bytes = Cursor::new(bytes);
        Ok(true)
    }
}
impl Read for Pipeline {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.bytes.read(out)?;
            if count > 0 {
                return Ok(count);
            }
            if !self.chunk()? && self.eof {
                return Ok(0);
            }
        }
    }
}
/// Symphonia's own reasons are static strings from the library, never stream
/// data, so they go into the error text that the playback log records.
fn map_error(error: DecodeError) -> io::Error {
    match error {
        DecodeError::IoError(error) => {
            let reason = error.to_string();
            io::Error::new(error.kind(), format!("media read failed: {reason}"))
        }
        DecodeError::Unsupported(reason) => io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported audio: {reason}"),
        ),
        DecodeError::DecodeError(reason) => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("malformed audio: {reason}"),
        ),
        DecodeError::SeekError(kind) => {
            io::Error::new(io::ErrorKind::InvalidData, format!("seek failed: {kind:?}"))
        }
        DecodeError::LimitError(reason) => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decoder limit: {reason}"),
        ),
        DecodeError::ResetRequired => io::Error::new(
            io::ErrorKind::InvalidData,
            "decoder reset required (stream parameters changed)",
        ),
        error => io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decoder failed: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests;
