//! Loudness is measured on producer/scanner threads. The output callback only
//! reads a gain atomically and runs a bounded, allocation-free stereo limiter.
use super::*;
use ebur128::{EbuR128, Mode};

const TARGET_LUFS: f64 = -14.0;
const CEILING: f32 = 0.891_250_9; // -1 dBFS; the full-track gain also respects true peak.
const MAX_GAIN: f32 = 3.981_071_7; // +12 dB, never magnify near-silence without limit.
const MIN_GAIN: f32 = 0.063_095_73; // -24 dB.

struct Measurement {
    meter: EbuR128,
    frames: u64,
    channels: usize,
}

impl Measurement {
    fn new() -> Self {
        Self {
            meter: EbuR128::new(
                u32::from(CHANNELS),
                SAMPLE_RATE,
                Mode::I | Mode::TRUE_PEAK | Mode::HISTOGRAM,
            )
            .expect("Native stereo loudness format is supported"),
            frames: 0,
            channels: usize::from(CHANNELS),
        }
    }

    fn add(&mut self, bytes: &[u8]) {
        // Decoded/cache chunks are bounded. This scratch space is on a worker,
        // never the audio stack, and keeps each meter call frame-aligned.
        let mut samples = [0.0_f32; 8192];
        let chunk_bytes = samples.len() * size_of::<f32>();
        for chunk in bytes.chunks(chunk_bytes) {
            let count = chunk.len() / FRAME_BYTES * self.channels;
            for (sample, raw) in samples[..count].iter_mut().zip(chunk.as_chunks::<4>().0) {
                let value = f32::from_le_bytes(*raw);
                *sample = if value.is_finite() {
                    value.clamp(-16.0, 16.0)
                } else {
                    0.0
                };
            }
            if self.meter.add_frames_f32(&samples[..count]).is_ok() {
                self.frames += (count / self.channels) as u64;
            }
        }
    }

    fn gain(&self) -> Option<f32> {
        if self.frames < u64::from(SAMPLE_RATE) * 2 / 5 {
            return None;
        }
        let loudness = self.meter.loudness_global().ok()?;
        // The EBU gate excludes silence. Do not turn silence/noise into a boost.
        if !loudness.is_finite() || loudness < -60.0 {
            return None;
        }
        let mut gain = 10.0_f64.powf((TARGET_LUFS - loudness) / 20.0) as f32;
        let peak = (0..self.channels)
            .filter_map(|channel| self.meter.true_peak(channel as u32).ok())
            .fold(0.0_f64, f64::max);
        if peak.is_finite() && peak > 0.0 {
            gain = gain.min(CEILING / peak as f32);
        }
        gain.is_finite().then(|| gain.clamp(MIN_GAIN, MAX_GAIN))
    }
}

struct Progressive {
    meter: Measurement,
    origin: u64,
    next: u64,
}

struct Core {
    gain: AtomicU32,
    final_gain: AtomicBool,
    progressive: Mutex<Option<Progressive>>,
}

pub(super) struct Profile {
    core: Arc<Core>,
    enabled: Arc<AtomicBool>,
    canceled: Arc<AtomicBool>,
    scanner: Mutex<Option<JoinHandle<()>>>,
}

impl Profile {
    pub(super) fn new(enabled: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            core: Arc::new(Core {
                gain: AtomicU32::new(1.0_f32.to_bits()),
                final_gain: AtomicBool::new(false),
                progressive: Mutex::new(None),
            }),
            enabled,
            canceled: Arc::new(AtomicBool::new(false)),
            scanner: Mutex::new(None),
        })
    }

    pub(super) fn gain(&self) -> f32 {
        f32::from_bits(self.core.gain.load(Ordering::Relaxed))
    }
    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Duplicated cache/decoder ranges and backward seeks do not count twice.
    pub(super) fn feed(&self, frame: u64, bytes: &[u8]) {
        if !self.enabled() || self.core.final_gain.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut guard) = self.core.progressive.lock() else {
            return;
        };
        if self.core.final_gain.load(Ordering::Acquire) {
            return;
        }
        let state = guard.get_or_insert_with(|| Progressive {
            meter: Measurement::new(),
            origin: frame,
            next: frame,
        });
        if frame == 0 && state.origin > 0 {
            *state = Progressive {
                meter: Measurement::new(),
                origin: 0,
                next: 0,
            };
        }
        if frame > state.next {
            return;
        }
        let frames = (bytes.len() / FRAME_BYTES) as u64;
        let skipped = state.next.saturating_sub(frame).min(frames);
        if skipped == frames {
            return;
        }
        let start = skipped as usize * FRAME_BYTES;
        state.meter.add(&bytes[start..]);
        state.next = frame + frames;
        if let Some(gain) = state.meter.gain() {
            self.core.gain.store(gain.to_bits(), Ordering::Relaxed);
        }
    }

    pub(super) fn finish_stream(&self, end: u64) {
        let Ok(mut guard) = self.core.progressive.lock() else {
            return;
        };
        if self.core.final_gain.load(Ordering::Acquire) {
            return;
        }
        if let Some(state) = guard.as_ref()
            && state.origin == 0
            && state.next >= end
        {
            if let Some(gain) = state.meter.gain() {
                self.core.gain.store(gain.to_bits(), Ordering::Relaxed);
            }
            self.core.final_gain.store(true, Ordering::Release);
            *guard = None;
        }
    }

    /// A completed raw cache permits whole-track measurement, including a
    /// prefix skipped by a resume/seek. This never blocks starting playback.
    pub(super) fn scan_cache(&self, cache: &CacheBinding, control: Arc<Control>) {
        if !self.enabled() || self.core.final_gain.load(Ordering::Acquire) {
            return;
        }
        let Some(end) = cache.complete() else { return };
        let Ok(mut worker) = self.scanner.lock() else {
            return;
        };
        if self.core.final_gain.load(Ordering::Acquire) {
            return;
        }
        if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return;
        }
        if let Some(previous) = worker.take() {
            let _ = previous.join();
        }
        let cache = cache.clone();
        let core = self.core.clone();
        let enabled = self.enabled.clone();
        let canceled = self.canceled.clone();
        let source = control.source.load(Ordering::Acquire);
        *worker = thread::Builder::new()
            .name("vk-loudness-analysis".into())
            .spawn(move || {
                let mut meter = Measurement::new();
                let mut frame = 0;
                while frame < end {
                    if canceled.load(Ordering::Acquire)
                        || !enabled.load(Ordering::Relaxed)
                        || control.source.load(Ordering::Acquire) != source
                    {
                        return;
                    }
                    let bytes = cache
                        .cache
                        .read_contiguous(&cache.key, frame, 16_384)
                        .unwrap_or_default();
                    if bytes.is_empty() {
                        return;
                    }
                    let take =
                        ((end - frame) as usize).min(bytes.len() / FRAME_BYTES) * FRAME_BYTES;
                    meter.add(&bytes[..take]);
                    frame += (take / FRAME_BYTES) as u64;
                    thread::yield_now();
                }
                if !canceled.load(Ordering::Acquire)
                    && control.source.load(Ordering::Acquire) == source
                    && let Ok(mut progressive) = core.progressive.lock()
                {
                    if let Some(gain) = meter.gain() {
                        core.gain.store(gain.to_bits(), Ordering::Relaxed);
                    }
                    core.final_gain.store(true, Ordering::Release);
                    *progressive = None;
                }
            })
            .ok();
    }

    pub(super) fn analyze_window(&self, pcm: &Pcm, offset: f64) {
        if !self.enabled() || self.core.final_gain.load(Ordering::Acquire) {
            return;
        }
        // Only the producer/controller locks the window. The callback can keep
        // consuming while this finite snapshot is copied and then measured.
        let (frame, bytes) = {
            let Ok(window) = pcm.window.lock() else {
                return;
            };
            let read =
                (pcm.cursor.load(Ordering::Acquire) & PCM_FRAME_MASK).max(window.start_frame);
            let end = window.end_frame;
            let capacity_frames = pcm.capacity / pcm.channels;
            let mut bytes = Vec::with_capacity(end.saturating_sub(read) as usize * FRAME_BYTES);
            for frame in read..end {
                let slot = frame as usize % capacity_frames * pcm.channels;
                for channel in 0..pcm.channels {
                    bytes.extend_from_slice(
                        &pcm.samples[slot + channel]
                            .load(Ordering::Relaxed)
                            .to_le_bytes(),
                    );
                }
            }
            (frame_at(offset).saturating_add(read), bytes)
        };
        self.feed(frame, &bytes);
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
        if let Ok(worker) = self.scanner.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

/// State owned exclusively by one output callback, so no atomics are needed
/// for smoothing and limiting. Gain is linked across all channels in a frame.
pub(super) struct OutputNormalizer {
    gain: f64,
    mix: f64,
    limiter: f64,
    smoothing: f64,
    blend_smoothing: f64,
    release: f64,
    heard: bool,
}

impl OutputNormalizer {
    pub(super) fn new(rate: u32) -> Self {
        Self {
            gain: 1.0,
            mix: 0.0,
            limiter: 1.0,
            smoothing: 1.0 - (-1.0 / (f64::from(rate.max(1)) * 0.35)).exp(),
            blend_smoothing: 1.0 - (-1.0 / (f64::from(rate.max(1)) * 0.08)).exp(),
            release: 1.0 - (-1.0 / (f64::from(rate.max(1)) * 0.05)).exp(),
            heard: false,
        }
    }

    pub(super) fn process(
        &mut self,
        data: &mut [f32],
        channels: usize,
        enabled: bool,
        target: f32,
        volume: f32,
    ) {
        if data.is_empty() {
            return;
        }
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            DEFAULT_VOLUME
        };
        if !enabled && self.mix == 0.0 {
            self.heard = true;
            self.gain = 1.0;
            self.limiter = 1.0;
            for sample in data {
                *sample = if sample.is_finite() {
                    (*sample * volume).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
            }
            return;
        }
        let target = f64::from(if enabled && target.is_finite() {
            target.clamp(MIN_GAIN, MAX_GAIN)
        } else {
            1.0
        });
        if !self.heard {
            self.gain = target;
            self.mix = if enabled { 1.0 } else { 0.0 };
            self.heard = true;
        }
        let target_mix = if enabled { 1.0 } else { 0.0 };
        for frame in data.chunks_exact_mut(channels.max(1)) {
            self.gain += (target - self.gain) * self.smoothing;
            self.mix += (target_mix - self.mix) * self.blend_smoothing;
            if (self.gain - target).abs() < 1e-9 {
                self.gain = target;
            }
            if (self.mix - target_mix).abs() < 1e-9 {
                self.mix = target_mix;
            }
            let gain = self.gain as f32;
            let mix = self.mix as f32;
            let normalized = |sample: f32| {
                if !sample.is_finite() {
                    return 0.0;
                }
                sample * gain
            };
            let peak = frame
                .iter()
                .map(|sample| normalized(*sample).abs())
                .fold(0.0_f32, f32::max);
            let ceiling = CEILING;
            let required = f64::from(if peak > ceiling { ceiling / peak } else { 1.0 });
            if required < self.limiter {
                self.limiter = required;
            } else {
                self.limiter = (self.limiter + (1.0 - self.limiter) * self.release).min(required);
            }
            if (1.0 - self.limiter).abs() < 1e-9 {
                self.limiter = 1.0;
            }
            let limiter = self.limiter as f32;
            for sample in frame {
                // Both paths apply the user's volume exactly once. Their
                // crossfade removes a clipped transfer smoothly, while the
                // normalized path attenuates float overs before any limiting.
                let legacy = if sample.is_finite() {
                    (*sample * volume).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                let controlled = (normalized(*sample) * limiter).clamp(-1.0, 1.0) * volume;
                *sample = (legacy * (1.0 - mix) + controlled * mix).clamp(-1.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{f32::consts::TAU, fs, time::Instant};

    fn tone(amplitude: f32, seconds: usize) -> Vec<f32> {
        (0..SAMPLE_RATE as usize * seconds)
            .flat_map(|frame| {
                let sample = amplitude * (TAU * 1000.0 * frame as f32 / SAMPLE_RATE as f32).sin();
                [sample, sample]
            })
            .collect()
    }
    fn bytes(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }
    fn measured(samples: &[f32]) -> f64 {
        let mut meter = Measurement::new();
        meter.add(&bytes(samples));
        meter.meter.loudness_global().unwrap()
    }

    #[test]
    fn tracks_at_different_levels_reach_the_same_perceived_loudness() {
        let mut output_levels = Vec::new();
        for amplitude in [0.08, 0.6, 1.2] {
            let mut samples = tone(amplitude, 3);
            let profile = Profile::new(Arc::new(AtomicBool::new(true)));
            let raw = bytes(&samples);
            profile.feed(0, &raw);
            profile.finish_stream(3 * u64::from(SAMPLE_RATE));
            let mut normalizer = OutputNormalizer::new(SAMPLE_RATE);
            normalizer.process(&mut samples, 2, true, profile.gain(), 1.0);
            let loudness = measured(&samples);
            assert!(
                (loudness - TARGET_LUFS).abs() < 0.15,
                "{amplitude}: {loudness} LUFS, gain {}",
                profile.gain()
            );
            assert!(
                samples
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= CEILING + 0.000_001)
            );
            assert_eq!(
                raw,
                bytes(&tone(amplitude, 3)),
                "Analysis changed the source PCM"
            );
            output_levels.push(loudness);
        }
        assert!((output_levels[0] - output_levels[1]).abs() < 0.1);
    }

    #[test]
    fn float_overs_are_attenuated_before_clipping_and_master_volume_is_applied_once() {
        let enabled = Arc::new(AtomicBool::new(true));
        let profile = Profile::new(enabled.clone());
        profile
            .core
            .gain
            .store(0.3_f32.to_bits(), Ordering::Relaxed);
        let pcm = Pcm {
            loudness: Some(profile),
            ..Pcm::new(
                CHANNELS,
                SAMPLE_RATE,
                Arc::new(AtomicU32::new(1.0_f32.to_bits())),
                false,
            )
        };
        let raw = bytes(&[1.2, -0.6]);
        prefill(&pcm, &raw, false);
        pcm.eof.store(true, Ordering::Release);
        let mut output = [0.0; 2];
        render_normalized(&mut output, &pcm, &mut OutputNormalizer::new(SAMPLE_RATE));
        assert!(
            (output[0] - 0.36).abs() < 0.000_001 && (output[1] + 0.18).abs() < 0.000_001,
            "Early clipping distorted normalized float PCM: {output:?}"
        );
        assert_eq!(f32::from_bits(pcm.samples[0].load(Ordering::Relaxed)), 1.2);

        pcm.volume.store(0.5_f32.to_bits(), Ordering::Relaxed);
        assert!(pcm.seek_cached(0.0, 0.0, 1));
        render_normalized(&mut output, &pcm, &mut OutputNormalizer::new(SAMPLE_RATE));
        assert!((output[0] - 0.18).abs() < 0.000_001 && (output[1] + 0.09).abs() < 0.000_001);

        enabled.store(false, Ordering::Relaxed);
        assert!(pcm.seek_cached(0.0, 0.0, 1));
        render_normalized(&mut output, &pcm, &mut OutputNormalizer::new(SAMPLE_RATE));
        assert!(
            (output[0] - 0.6).abs() < 0.000_001 && (output[1] + 0.3).abs() < 0.000_001,
            "Disabled normalization must clip after the master fader: {output:?}"
        );

        let mut meter = Measurement::new();
        meter.add(&bytes(&tone(1.2, 1)));
        assert!(
            meter.meter.true_peak(0).unwrap() > 1.19,
            "The loudness meter received clipped source samples"
        );
    }

    #[test]
    fn native_callback_normalizes_output_while_its_pcm_window_remains_raw() {
        let enabled = Arc::new(AtomicBool::new(true));
        let profile = Profile::new(enabled.clone());
        let samples = tone(0.08, 1);
        let raw = bytes(&samples);
        profile.feed(0, &raw);
        let pcm = Pcm {
            loudness: Some(profile),
            ..Pcm::new(
                CHANNELS,
                SAMPLE_RATE,
                Arc::new(AtomicU32::new(1.0_f32.to_bits())),
                false,
            )
        };
        prefill(&pcm, &raw, false);
        pcm.eof.store(true, Ordering::Release);
        let mut output = vec![0.0; samples.len()];
        render_normalized(&mut output, &pcm, &mut OutputNormalizer::new(SAMPLE_RATE));
        assert!((measured(&output) - TARGET_LUFS).abs() < 0.15);
        for (index, sample) in samples.iter().enumerate() {
            assert_eq!(
                pcm.samples[index].load(Ordering::Relaxed),
                sample.to_bits(),
                "Output gain contaminated the seekable PCM window"
            );
        }
        enabled.store(false, Ordering::Relaxed);
        assert!(pcm.seek_cached(0.0, 0.0, 1));
        render_normalized(&mut output, &pcm, &mut OutputNormalizer::new(SAMPLE_RATE));
        assert_eq!(output, samples);
    }

    #[test]
    fn silence_non_finite_input_and_extreme_peaks_stay_bounded_and_stereo_linked() {
        let profile = Profile::new(Arc::new(AtomicBool::new(true)));
        profile.feed(0, &bytes(&vec![0.0; SAMPLE_RATE as usize * 2]));
        assert_eq!(profile.gain(), 1.0);
        let mut normalizer = OutputNormalizer::new(SAMPLE_RATE);
        let mut output = vec![0.9, -0.45];
        normalizer.process(&mut output, 2, true, MAX_GAIN, 1.0);
        assert!((output[0] / output[1] + 2.0).abs() < 0.0001);
        assert!(output[0].abs() <= CEILING + 0.000_001);
        let mut hostile = [f32::NAN, f32::INFINITY, -1.0, 0.5, 0.1, -0.05].repeat(2000);
        normalizer.process(&mut hostile, 2, true, MAX_GAIN, 1.0);
        assert!(
            hostile
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= CEILING + 0.000_001)
        );
    }

    #[test]
    fn live_toggle_crossfades_gain_and_off_returns_to_unmodified_audio() {
        let mut normalizer = OutputNormalizer::new(SAMPLE_RATE);
        let mut before = [0.1, -0.05];
        normalizer.process(&mut before, 2, false, 2.0, 1.0);
        assert_eq!(before, [0.1, -0.05]);
        let mut enabled = vec![0.1; SAMPLE_RATE as usize * 2];
        normalizer.process(&mut enabled, 2, true, 2.0, 1.0);
        assert!(
            (enabled[0] - 0.1).abs() < 0.0001,
            "Toggle jumped rather than smoothing"
        );
        assert!(enabled[enabled.len() - 1] > 0.19);
        assert!(
            enabled
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 0.0001)
        );
        let mut disabled = vec![0.1; SAMPLE_RATE as usize * 10];
        normalizer.process(&mut disabled, 2, false, 2.0, 1.0);
        assert!((disabled[disabled.len() - 1] - 0.1).abs() < 0.0001);
    }

    #[test]
    fn repeats_and_backwards_ranges_do_not_remeasure_or_change_a_final_gain() {
        let samples = bytes(&tone(0.08, 2));
        let profile = Profile::new(Arc::new(AtomicBool::new(true)));
        profile.feed(0, &samples);
        let initial = profile.gain();
        profile.feed(0, &samples);
        assert_eq!(
            profile
                .core
                .progressive
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .meter
                .frames,
            2 * u64::from(SAMPLE_RATE)
        );
        assert_eq!(profile.gain(), initial);
        profile.finish_stream(2 * u64::from(SAMPLE_RATE));
        profile.feed(0, &bytes(&tone(0.6, 2)));
        assert_eq!(profile.gain(), initial);
        assert!(profile.core.final_gain.load(Ordering::Acquire));
    }

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let mut random = [0_u8; 12];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("vk-fast-loudness-{suffix}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn completed_cache_analysis_recovers_a_skipped_prefix_without_rewriting_audio() {
        let directory = Directory::new();
        let cache = AudioCache::test_at(directory.0.join("audio"), 8 * 1024 * 1024).unwrap();
        let key = CacheKey {
            account_id: 42,
            owner_id: -7,
            track_id: 1,
            duration: 3,
        };
        let mut samples = tone(0.6, 2);
        samples.extend(tone(0.08, 1));
        let raw = bytes(&samples);
        cache.write_range(&key, 0, &raw).unwrap();
        cache
            .mark_complete(&key, 3 * u64::from(SAMPLE_RATE))
            .unwrap();
        let expected = {
            let mut meter = Measurement::new();
            meter.add(&raw);
            meter.gain().unwrap()
        };
        let enabled = Arc::new(AtomicBool::new(true));
        let profile = Profile::new(enabled.clone());
        profile.feed(2 * u64::from(SAMPLE_RATE), &bytes(&tone(0.08, 1)));
        assert!(profile.gain() > expected * 2.0);
        profile.scan_cache(
            &CacheBinding {
                cache: cache.clone(),
                key,
            },
            Arc::new(Control::default()),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !profile.core.final_gain.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(profile.core.final_gain.load(Ordering::Acquire));
        assert!((profile.gain() - expected).abs() < 0.0001);
        assert_eq!(
            cache
                .read_contiguous(&key, 0, SAMPLE_RATE as usize * 3)
                .unwrap(),
            raw
        );
        enabled.store(false, Ordering::Relaxed);
        profile.feed(0, &bytes(&tone(0.2, 1)));
        assert_eq!(profile.gain(), expected);
    }
}
