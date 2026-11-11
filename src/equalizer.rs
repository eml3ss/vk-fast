//! Ten-band equalizer for playback on this computer.
//!
//! The UI thread publishes band gains through atomics; the audio callback
//! notices a new revision and recomputes its filters there, without locks,
//! waits or allocations. Boosts lower the overall level by the curve's peak so
//! the result does not clip.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::{audio_output::SAMPLE_RATE, i18n::Language};

/// Band centres in Hz.
pub const BANDS: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];
pub const MAX_DB: f32 = 12.0;
/// Gains move in half-decibel steps.
pub const STEP_DB: f32 = 0.5;
/// One octave per band.
const Q: f64 = std::f64::consts::SQRT_2;
/// Channels the callback filters; any further ones pass unchanged.
const MAX_CHANNELS: usize = 8;

pub type Gains = [f32; BANDS.len()];

/// A gain the equalizer accepts: finite, within ±12 dB, on a half-decibel step.
pub fn clean_gain(gain: f32) -> f32 {
    if !gain.is_finite() {
        return 0.0;
    }
    let gain = (gain.clamp(-MAX_DB, MAX_DB) / STEP_DB).round() * STEP_DB;
    // No negative zero in labels or saved files.
    if gain == 0.0 { 0.0 } else { gain }
}

fn clean_gains(gains: Gains) -> Gains {
    gains.map(clean_gain)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Flat,
    Bass,
    LessBass,
    Treble,
    Voice,
    Warm,
    Bright,
    Electronic,
    HipHop,
    Rock,
    Acoustic,
    Night,
}

impl Builtin {
    pub const ALL: [Self; 12] = [
        Self::Flat,
        Self::Bass,
        Self::LessBass,
        Self::Treble,
        Self::Voice,
        Self::Warm,
        Self::Bright,
        Self::Electronic,
        Self::HipHop,
        Self::Rock,
        Self::Acoustic,
        Self::Night,
    ];

    pub fn name(self, language: Language) -> &'static str {
        match self {
            Self::Flat => language.text("Без изменений", "Flat"),
            Self::Bass => language.text("Больше баса", "More bass"),
            Self::LessBass => language.text("Меньше баса", "Less bass"),
            Self::Treble => language.text("Больше высоких", "More treble"),
            Self::Voice => language.text("Чёткий голос", "Clear vocals"),
            Self::Warm => language.text("Тёплый", "Warm"),
            Self::Bright => language.text("Яркий", "Bright"),
            Self::Electronic => language.text("Электроника", "Electronic"),
            Self::HipHop => language.text("Хип-хоп", "Hip-hop"),
            Self::Rock => language.text("Рок", "Rock"),
            Self::Acoustic => language.text("Акустика", "Acoustic"),
            Self::Night => language.text("Тихо ночью", "Late night"),
        }
    }

    /// Short and stable, for logs.
    pub fn id(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Bass => "bass",
            Self::LessBass => "less_bass",
            Self::Treble => "treble",
            Self::Voice => "voice",
            Self::Warm => "warm",
            Self::Bright => "bright",
            Self::Electronic => "electronic",
            Self::HipHop => "hip_hop",
            Self::Rock => "rock",
            Self::Acoustic => "acoustic",
            Self::Night => "night",
        }
    }

    pub fn gains(self) -> Gains {
        match self {
            Self::Flat => [0.0; 10],
            Self::Bass => [6.0, 5.5, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
            Self::LessBass => [-6.0, -5.0, -3.5, -1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            Self::Treble => [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.5, 3.5, 5.0, 6.0],
            Self::Voice => [-2.0, -1.5, -1.0, 0.0, 1.5, 3.0, 3.5, 2.5, 1.0, 0.0],
            Self::Warm => [2.0, 2.5, 2.0, 1.0, 0.0, -0.5, -1.0, -1.5, -2.0, -2.5],
            Self::Bright => [-1.0, -0.5, 0.0, 0.0, 0.0, 0.5, 1.5, 3.0, 4.0, 4.5],
            Self::Electronic => [5.0, 4.0, 1.5, 0.0, -1.5, -0.5, 0.5, 2.0, 3.5, 4.0],
            Self::HipHop => [5.5, 5.0, 2.5, 1.0, -0.5, -1.0, 0.5, 1.0, 2.0, 2.5],
            Self::Rock => [4.0, 3.0, 1.5, -0.5, -1.5, -0.5, 1.5, 3.0, 3.5, 3.5],
            Self::Acoustic => [1.0, 1.5, 1.5, 1.0, 0.5, 1.0, 1.5, 2.0, 1.5, 1.0],
            Self::Night => [-4.0, -3.0, -1.5, 0.0, 1.0, 1.5, 1.0, -1.0, -2.5, -4.0],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub gains: Gains,
}

/// Longest preset name, in characters.
pub const MAX_NAME: usize = 40;
pub const MAX_PRESETS: usize = 30;

/// A preset name as it is kept: one line, trimmed, at most `MAX_NAME` chars.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub gains: Gains,
    pub presets: Vec<Preset>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            gains: [0.0; 10],
            presets: Vec::new(),
        }
    }
}

impl Settings {
    /// Drops what an older or edited file could hold that the equalizer
    /// cannot use.
    pub fn cleaned(mut self) -> Self {
        self.gains = clean_gains(self.gains);
        let mut presets: Vec<Preset> = Vec::new();
        for preset in self.presets {
            let name = clean_name(&preset.name);
            if name.is_empty() || presets.iter().any(|kept| kept.name == name) {
                continue;
            }
            presets.push(Preset {
                name,
                gains: clean_gains(preset.gains),
            });
        }
        presets.truncate(MAX_PRESETS);
        self.presets = presets;
        self
    }

    /// The built-in preset the current curve is, if any.
    pub fn builtin(&self) -> Option<Builtin> {
        Builtin::ALL
            .into_iter()
            .find(|preset| preset.gains() == self.gains)
    }

    /// The saved preset the current curve is, if any (and not a built-in one).
    pub fn custom(&self) -> Option<usize> {
        if self.builtin().is_some() {
            return None;
        }
        self.presets
            .iter()
            .position(|preset| preset.gains == self.gains)
    }

    /// Saves the current curve under `name`, replacing a preset of that name.
    pub fn save_preset(&mut self, name: &str) -> bool {
        let name = clean_name(name);
        if name.is_empty() {
            return false;
        }
        if let Some(existing) = self.presets.iter_mut().find(|preset| preset.name == name) {
            existing.gains = self.gains;
            return true;
        }
        if self.presets.len() >= MAX_PRESETS {
            return false;
        }
        self.presets.push(Preset {
            name,
            gains: self.gains,
        });
        true
    }
}

/// Gain of one band's filter at `freq`, in dB.
fn band_db(band: usize, gain: f32, freq: f32) -> f32 {
    if gain == 0.0 {
        return 0.0;
    }
    let coefficients = Biquad::band(band, gain);
    let w = std::f64::consts::TAU * f64::from(freq) / f64::from(SAMPLE_RATE);
    let (cos1, sin1, cos2, sin2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let magnitude = |c0: f64, c1: f64, c2: f64| {
        let re = c0 + c1 * cos1 + c2 * cos2;
        let im = -(c1 * sin1 + c2 * sin2);
        re.hypot(im)
    };
    let [b0, b1, b2, a1, a2] = coefficients.coefficients;
    let response = magnitude(b0, b1, b2) / magnitude(1.0, a1, a2);
    (20.0 * response.log10()) as f32
}

/// The whole curve's gain at `freq`, in dB.
pub fn response_db(gains: &Gains, freq: f32) -> f32 {
    gains
        .iter()
        .enumerate()
        .map(|(band, gain)| band_db(band, *gain, freq))
        .sum()
}

/// Level the output is lowered by so the loudest point of the curve stays at
/// or below 0 dB.
pub fn headroom_db(gains: &Gains) -> f32 {
    const POINTS: usize = 240;
    let (low, high) = (20.0_f32.ln(), 20_000.0_f32.ln());
    (0..=POINTS)
        .map(|step| {
            let freq = (low + (high - low) * step as f32 / POINTS as f32).exp();
            response_db(gains, freq)
        })
        .fold(0.0_f32, f32::max)
}

/// What the UI thread hands the audio callback.
pub struct Shared {
    revision: AtomicU64,
    enabled: AtomicBool,
    gains: [AtomicU32; BANDS.len()],
    /// Linear output gain that undoes the curve's peak.
    output: AtomicU32,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            revision: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            gains: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            output: AtomicU32::new(1.0_f32.to_bits()),
        }
    }
}

impl Shared {
    #[cfg(test)]
    pub fn current(&self) -> (bool, Gains) {
        (
            self.enabled.load(Ordering::Relaxed),
            std::array::from_fn(|band| f32::from_bits(self.gains[band].load(Ordering::Relaxed))),
        )
    }

    pub fn set(&self, enabled: bool, gains: &Gains) {
        let gains = clean_gains(*gains);
        for (slot, gain) in self.gains.iter().zip(gains) {
            slot.store(gain.to_bits(), Ordering::Relaxed);
        }
        let output = 10.0_f32.powf(-headroom_db(&gains) / 20.0);
        self.output.store(output.to_bits(), Ordering::Relaxed);
        self.enabled.store(enabled, Ordering::Relaxed);
        self.revision.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    /// b0, b1, b2, a1, a2 with a0 normalized to 1.
    coefficients: [f64; 5],
}

impl Biquad {
    const IDENTITY: Self = Self {
        coefficients: [1.0, 0.0, 0.0, 0.0, 0.0],
    };

    /// A peaking filter an octave wide (RBJ audio EQ cookbook), so every
    /// knob sits on the curve the way it sounds.
    fn band(band: usize, gain: f32) -> Self {
        if gain == 0.0 {
            return Self::IDENTITY;
        }
        let a = 10.0_f64.powf(f64::from(gain) / 40.0);
        let w0 = std::f64::consts::TAU * f64::from(BANDS[band]) / f64::from(SAMPLE_RATE);
        let (cos, sin) = (w0.cos(), w0.sin());
        let alpha = sin / (2.0 * Q);
        let [b0, b1, b2, a0, a1, a2] = [
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        ];
        Self {
            coefficients: [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0],
        }
    }
}

/// Lives in the audio callback: the filters and their per-channel state.
pub struct Processor {
    revision: u64,
    enabled: bool,
    filters: [Biquad; BANDS.len()],
    /// Transposed direct form II state per channel and band.
    state: [[[f64; 2]; BANDS.len()]; MAX_CHANNELS],
    output: f32,
    target: f32,
}

impl Processor {
    pub fn new() -> Self {
        Self {
            revision: 0,
            enabled: false,
            filters: [Biquad::IDENTITY; BANDS.len()],
            state: [[[0.0; 2]; BANDS.len()]; MAX_CHANNELS],
            output: 1.0,
            target: 1.0,
        }
    }

    fn refresh(&mut self, shared: &Shared) {
        let revision = shared.revision.load(Ordering::Acquire);
        if revision == self.revision {
            return;
        }
        self.revision = revision;
        let enabled = shared.enabled.load(Ordering::Relaxed);
        for (band, filter) in self.filters.iter_mut().enumerate() {
            let gain = f32::from_bits(shared.gains[band].load(Ordering::Relaxed));
            *filter = if enabled {
                Biquad::band(band, clean_gain(gain))
            } else {
                Biquad::IDENTITY
            };
        }
        if enabled && !self.enabled {
            self.state = [[[0.0; 2]; BANDS.len()]; MAX_CHANNELS];
        }
        self.enabled = enabled;
        let output = f32::from_bits(shared.output.load(Ordering::Relaxed));
        self.target = if enabled && output.is_finite() {
            output.clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    /// Filters interleaved `samples` in place. No locks, waits or allocations.
    pub fn process(&mut self, shared: &Shared, samples: &mut [f32], channels: usize) {
        self.refresh(shared);
        if channels == 0 || samples.is_empty() {
            return;
        }
        let frames = samples.len() / channels;
        if !self.enabled && self.output == 1.0 {
            return;
        }
        // The level glides over the quantum, so a new curve does not click.
        let (start, end) = (self.output, self.target);
        for (index, frame) in samples.chunks_exact_mut(channels).enumerate() {
            let level = start + (end - start) * (index + 1) as f32 / frames as f32;
            for (channel, sample) in frame.iter_mut().enumerate() {
                let mut value = f64::from(*sample);
                if self.enabled && channel < MAX_CHANNELS {
                    for (filter, state) in self.filters.iter().zip(&mut self.state[channel]) {
                        let [b0, b1, b2, a1, a2] = filter.coefficients;
                        let out = b0 * value + state[0];
                        state[0] = b1 * value - a1 * out + state[1];
                        state[1] = b2 * value - a2 * out;
                        value = out;
                    }
                }
                *sample = value as f32 * level;
            }
        }
        self.output = end;
        // Denormals and runaway state never outlive a quantum.
        for channel in &mut self.state {
            for state in channel {
                for value in state {
                    if !value.is_finite() || value.abs() < 1e-25 {
                        *value = 0.0;
                    }
                }
            }
        }
    }
}

impl Default for Processor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|index| {
                let value =
                    (std::f32::consts::TAU * freq * index as f32 / SAMPLE_RATE as f32).sin() * 0.25;
                [value, value]
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn played(shared: &Shared, freq: f32) -> f32 {
        let mut processor = Processor::new();
        let mut samples = sine(freq, SAMPLE_RATE as usize);
        for chunk in samples.chunks_mut(960) {
            processor.process(shared, chunk, 2);
        }
        // The second half, once the filters have settled.
        rms(&samples[samples.len() / 2..])
    }

    #[test]
    fn off_and_flat_leave_samples_untouched() {
        let shared = Shared::default();
        let original = sine(440.0, 4800);
        let mut samples = original.clone();
        Processor::new().process(&shared, &mut samples, 2);
        assert_eq!(samples, original);
        shared.set(true, &[0.0; 10]);
        let mut samples = original.clone();
        Processor::new().process(&shared, &mut samples, 2);
        for (a, b) in samples.iter().zip(&original) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn a_band_changes_its_frequency_and_the_peak_is_taken_off_the_level() {
        let shared = Shared::default();
        let flat = played(&shared, 1_000.0);
        let mut gains = [0.0; 10];
        gains[5] = -6.0;
        shared.set(true, &gains);
        let cut = played(&shared, 1_000.0);
        let ratio = 20.0 * (cut / flat).log10();
        assert!((ratio + 6.0).abs() < 0.3, "1 kHz cut by {ratio} dB");
        // Far from the band nothing changes.
        let far = played(&shared, 62.0) / played(&Shared::default(), 62.0);
        assert!((20.0 * far.log10()).abs() < 0.3);
        // A boost is lowered back to 0 dB at its peak, so it never clips.
        gains[5] = 6.0;
        shared.set(true, &gains);
        assert!((headroom_db(&gains) - 6.0).abs() < 0.2);
        let boosted = played(&shared, 1_000.0);
        assert!((20.0 * (boosted / flat).log10()).abs() < 0.3);
        let lowered = played(&shared, 62.0) / played(&Shared::default(), 62.0);
        assert!((20.0 * lowered.log10() + 6.0).abs() < 0.3);
    }

    #[test]
    fn the_drawn_curve_matches_what_is_played() {
        for preset in Builtin::ALL {
            let shared = Shared::default();
            shared.set(true, &preset.gains());
            let headroom = headroom_db(&preset.gains());
            for freq in [62.0, 250.0, 1_000.0, 4_000.0] {
                let heard =
                    20.0 * (played(&shared, freq) / played(&Shared::default(), freq)).log10();
                let drawn = response_db(&preset.gains(), freq) - headroom;
                assert!(
                    (heard - drawn).abs() < 0.3,
                    "{preset:?} {freq} Hz: {heard} vs {drawn}"
                );
            }
        }
    }

    #[test]
    fn settings_from_a_file_are_cleaned() {
        let settings = Settings {
            enabled: true,
            gains: [f32::NAN, 30.0, -30.0, 0.26, -0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
            presets: vec![
                Preset {
                    name: "  Мой\n  пресет ".into(),
                    gains: [1.0; 10],
                },
                Preset {
                    name: "Мой пресет".into(),
                    gains: [2.0; 10],
                },
                Preset {
                    name: " ".into(),
                    gains: [2.0; 10],
                },
            ],
        }
        .cleaned();
        assert_eq!(
            settings.gains,
            [0.0, 12.0, -12.0, 0.5, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0]
        );
        assert!(settings.gains[4].is_sign_positive());
        assert_eq!(settings.presets.len(), 1);
        assert_eq!(settings.presets[0].name, "Мой пресет");
        assert_eq!(clean_name(&"я".repeat(100)).chars().count(), MAX_NAME);
    }

    #[test]
    fn presets_are_recognized_saved_and_replaced_by_name() {
        let mut settings = Settings::default();
        assert_eq!(settings.builtin(), Some(Builtin::Flat));
        settings.gains = Builtin::Rock.gains();
        assert_eq!(settings.builtin(), Some(Builtin::Rock));
        settings.gains[3] = 2.0;
        assert_eq!(settings.builtin(), None);
        assert!(settings.save_preset(" Мой "));
        assert_eq!(settings.custom(), Some(0));
        settings.gains[3] = 3.0;
        assert_eq!(settings.custom(), None);
        assert!(settings.save_preset("Мой"));
        assert_eq!(settings.presets.len(), 1);
        assert_eq!(settings.custom(), Some(0));
        assert!(!settings.save_preset("   "));
        for index in 1..MAX_PRESETS {
            assert!(settings.save_preset(&format!("P{index}")));
        }
        assert!(!settings.save_preset("one too many"));
    }

    #[test]
    fn builtin_presets_are_distinct_and_within_range() {
        for (index, preset) in Builtin::ALL.into_iter().enumerate() {
            assert_eq!(clean_gains(preset.gains()), preset.gains());
            for other in &Builtin::ALL[index + 1..] {
                assert_ne!(preset.gains(), other.gains(), "{preset:?} {other:?}");
            }
            assert!(!preset.name(Language::Russian).is_empty());
        }
    }
}
