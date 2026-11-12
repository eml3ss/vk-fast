//! Worker-side channel conversion and band-limited resampling to the device
//! format. Neither allocation nor filter work enters the native audio callback.
use super::net::invalid;
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters};
use std::io;
use symphonia::core::audio::{AudioSpec, Channels, Position};
const CHUNK: usize = 1024;

pub(super) struct Converter {
    spec: AudioSpec,
    matrix: Vec<[f32; 2]>,
    resampler: Option<Async<f32>>,
    input: Vec<f32>,
    delay: usize,
    received: u64,
    produced: u64,
    output_rate: u32,
}
impl Converter {
    pub fn new(spec: &AudioSpec, output_rate: u32) -> io::Result<Self> {
        let count = spec.channels().count();
        if !(1..=32).contains(&count) || !(8000..=384000).contains(&spec.rate()) {
            return Err(invalid());
        }
        let mut matrix = vec![[0.0; 2]; count];
        if count == 1 {
            matrix[0] = [1.0, 1.0];
        } else if count == 2 {
            matrix[0] = [1.0, 0.0];
            matrix[1] = [0.0, 1.0];
        } else if let Channels::Positioned(mask) = spec.channels() {
            for (position, weights) in [
                (Position::FRONT_LEFT, [1.0, 0.0]),
                (Position::FRONT_RIGHT, [0.0, 1.0]),
                (Position::FRONT_CENTER, [std::f32::consts::FRAC_1_SQRT_2; 2]),
                (Position::REAR_LEFT, [std::f32::consts::FRAC_1_SQRT_2, 0.0]),
                (Position::REAR_RIGHT, [0.0, std::f32::consts::FRAC_1_SQRT_2]),
                (Position::SIDE_LEFT, [std::f32::consts::FRAC_1_SQRT_2, 0.0]),
                (Position::SIDE_RIGHT, [0.0, std::f32::consts::FRAC_1_SQRT_2]),
                (Position::REAR_CENTER, [0.5, 0.5]),
            ] {
                if mask.contains(position)
                    && let Some(index) = spec
                        .channels()
                        .get_canonical_index_for_positioned_channel(position)
                    && let Some(row) = matrix.get_mut(index)
                {
                    *row = weights;
                }
            }
            for side in 0..2 {
                let total = matrix.iter().map(|m| m[side]).sum::<f32>().max(1.0);
                for row in &mut matrix {
                    row[side] /= total;
                }
            }
        } else {
            for (i, row) in matrix.iter_mut().enumerate() {
                row[i % 2] = 2.0 / count as f32;
            }
        }
        let resampler = if spec.rate() != output_rate {
            Some(
                Async::<f32>::new_sinc(
                    f64::from(output_rate) / f64::from(spec.rate()),
                    1.0,
                    &SincInterpolationParameters {
                        sinc_len: 256,
                        ..Default::default()
                    },
                    CHUNK,
                    2,
                    FixedAsync::Input,
                )
                .map_err(|_| invalid())?,
            )
        } else {
            None
        };
        let delay = resampler.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            spec: spec.clone(),
            matrix,
            resampler,
            input: Vec::new(),
            delay,
            received: 0,
            produced: 0,
            output_rate,
        })
    }
    pub fn spec(&self) -> &AudioSpec {
        &self.spec
    }
    pub fn push(&mut self, samples: &[f32]) -> io::Result<Vec<f32>> {
        let channels = self.matrix.len();
        if !samples.len().is_multiple_of(channels) {
            return Err(invalid());
        }
        self.received += (samples.len() / channels) as u64;
        let mut stereo = Vec::with_capacity(samples.len() / channels * 2);
        for frame in samples.chunks_exact(channels) {
            let mut mixed = [0.0; 2];
            for (&value, weights) in frame.iter().zip(&self.matrix) {
                let value = if value.is_finite() {
                    value.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
                mixed[0] += value * weights[0];
                mixed[1] += value * weights[1];
            }
            stereo.extend_from_slice(&mixed);
        }
        if self.resampler.is_none() {
            self.produced += stereo.len() as u64 / 2;
            return Ok(stereo);
        }
        self.input.extend(stereo);
        self.process(false)
    }
    pub fn finish(&mut self) -> io::Result<Vec<f32>> {
        if self.resampler.is_none() {
            return Ok(Vec::new());
        }
        self.process(true)
    }
    fn process(&mut self, finish: bool) -> io::Result<Vec<f32>> {
        let mut out = Vec::new();
        let target = (self.received as f64 * f64::from(self.output_rate)
            / f64::from(self.spec.rate()))
        .round() as u64;
        let resampler = self.resampler.as_mut().ok_or_else(|| invalid())?;
        let mut offset = 0;
        loop {
            let needed = resampler.input_frames_next();
            let available = self.input.len() / 2 - offset;
            if available < needed && (!finish || self.produced >= target) {
                break;
            }
            let used = available.min(needed);
            let mut block = vec![0.0; needed * 2];
            block[..used * 2].copy_from_slice(&self.input[offset * 2..(offset + used) * 2]);
            let input = InterleavedSlice::new(&block, 2, needed).map_err(|_| invalid())?;
            let indexing = Indexing::new().partial_len(used);
            let block = resampler
                .process(&input, Some(&indexing))
                .map_err(|_| invalid())?
                .take_data();
            offset += used;
            let skip = self.delay.min(block.len() / 2);
            self.delay -= skip;
            let mut frames = block.len() / 2 - skip;
            if finish {
                frames = frames.min(target.saturating_sub(self.produced) as usize);
            }
            out.extend_from_slice(&block[skip * 2..(skip + frames) * 2]);
            self.produced += frames as u64;
            if finish && self.produced >= target {
                break;
            }
        }
        self.input.drain(..offset * 2);
        Ok(out)
    }
}
