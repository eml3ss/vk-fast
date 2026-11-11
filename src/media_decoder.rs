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
#[cfg(test)]
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
    if !offset.is_finite()
        || offset < 0.0
        || !net::valid_url(&reqwest::Url::parse(url).map_err(|_| invalid())?)
    {
        return Err(invalid());
    }
    let cancellation = Cancellation::default();
    Ok(Stream {
        reader: Box::new(Lazy {
            url: Zeroizing::new(url.into()),
            offset,
            canceled: cancellation.0.clone(),
            pipeline: None,
        }),
        cancellation,
    })
}
struct Lazy {
    url: Zeroizing<String>,
    offset: f64,
    canceled: Arc<AtomicBool>,
    pipeline: Option<Pipeline>,
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
            let (source, origin) = source::open(&self.url, self.offset, self.canceled.clone())?;
            let pipeline = Pipeline::new(source, origin, self.offset, self.canceled.clone())?;
            self.pipeline = Some(pipeline);
        }
        let pipeline = self.pipeline.as_mut().ok_or_else(invalid)?;
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
        let track = format.default_track(TrackType::Audio).ok_or_else(invalid)?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(invalid)?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(map_error)?;
        let id = track.id;
        let time_base = track.time_base;
        if seekable && offset > 2.0 {
            format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time: Time::try_from_secs_f64(offset - 2.0).ok_or_else(invalid)?,
                        track_id: Some(id),
                    },
                )
                .map_err(map_error)?;
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
            let packet = self.format.next_packet().map_err(map_error)?;
            let Some(packet) = packet else {
                self.eof = true;
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
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.failures = 0;
                    decoded
                }
                Err(DecodeError::DecodeError(_)) if self.failures < 32 => {
                    self.failures += 1;
                    continue;
                }
                Err(error) => return Err(map_error(error)),
            };
            if decoded.frames() > 1024 * 1024 || decoded.spec().channels().count() > 32 {
                return Err(invalid());
            }
            let spec = decoded.spec().clone();
            let mut samples = vec![0.0f32; decoded.samples_interleaved()];
            decoded.copy_to_slice_interleaved(&mut samples);
            if self.converter.as_ref().is_none_or(|c| c.spec() != &spec) {
                if self.converter.is_some() {
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
                        .ok_or_else(invalid)?
                        .push(&vec![0.0; padding * spec.channels().count()])?;
                    self.publish(prefix)?;
                }
            }
            let converted = self
                .converter
                .as_mut()
                .ok_or_else(invalid)?
                .push(&samples)?;
            if self.publish(converted)? {
                return Ok(true);
            }
        }
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
fn map_error(error: DecodeError) -> io::Error {
    if let DecodeError::Unsupported(reason) | DecodeError::DecodeError(reason) = &error {
        crate::diagnostics::event("decoder", reason);
    }
    match error {
        DecodeError::IoError(error) => io::Error::new(error.kind(), "Media read failed"),
        DecodeError::Unsupported(_) => {
            io::Error::new(io::ErrorKind::Unsupported, "Unsupported audio format")
        }
        _ => invalid(),
    }
}

#[cfg(test)]
mod tests;
