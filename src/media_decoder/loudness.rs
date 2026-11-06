//! File loudness tags are read before decoded samples enter the PCM pipeline.
use symphonia::core::meta::{MetadataRevision, RawValue, StandardTag};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    pub gain_db: f32,
    pub reference_lufs: f32,
    pub peak: Option<f32>,
}

fn number(text: &str) -> Option<f32> {
    let text = text.trim();
    let text = if text
        .get(text.len().saturating_sub(2)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case("db"))
    {
        text.get(..text.len() - 2)?.trim()
    } else {
        text
    };
    text.parse::<f32>().ok().filter(|value| value.is_finite())
}

fn raw_number(value: &RawValue) -> Option<f32> {
    match value {
        RawValue::String(value) => number(value),
        RawValue::Float(value) => Some(*value as f32).filter(|value| value.is_finite()),
        RawValue::SignedInt(value) => Some(*value as f32),
        RawValue::UnsignedInt(value) => Some(*value as f32),
        _ => None,
    }
}

pub(super) fn from_metadata(revision: &MetadataRevision, track: u64) -> Option<Loudness> {
    let mut track_gain = None;
    let mut track_peak = None;
    let mut album_gain = None;
    let mut album_peak = None;
    let mut r128_track = None;
    let mut r128_album = None;
    let tags = revision
        .per_track
        .iter()
        .filter(|item| item.track_id == track)
        .flat_map(|item| &item.metadata.tags)
        .chain(&revision.media.tags);
    for tag in tags {
        let (key, value) = match &tag.std {
            Some(StandardTag::ReplayGainTrackGain(value)) => {
                ("REPLAYGAIN_TRACK_GAIN".into(), number(value))
            }
            Some(StandardTag::ReplayGainTrackPeak(value)) => {
                ("REPLAYGAIN_TRACK_PEAK".into(), number(value))
            }
            Some(StandardTag::ReplayGainAlbumGain(value)) => {
                ("REPLAYGAIN_ALBUM_GAIN".into(), number(value))
            }
            Some(StandardTag::ReplayGainAlbumPeak(value)) => {
                ("REPLAYGAIN_ALBUM_PEAK".into(), number(value))
            }
            _ => (tag.raw.key.to_ascii_uppercase(), raw_number(&tag.raw.value)),
        };
        let Some(value) = value else { continue };
        let slot = match key.as_str() {
            "REPLAYGAIN_TRACK_GAIN" if value.abs() <= 128.0 => &mut track_gain,
            "REPLAYGAIN_ALBUM_GAIN" if value.abs() <= 128.0 => &mut album_gain,
            "REPLAYGAIN_TRACK_PEAK" if value > 0.0 && value <= 32.0 => &mut track_peak,
            "REPLAYGAIN_ALBUM_PEAK" if value > 0.0 && value <= 32.0 => &mut album_peak,
            "R128_TRACK_GAIN" if value.fract() == 0.0 && (-32768.0..=32767.0).contains(&value) => {
                &mut r128_track
            }
            "R128_ALBUM_GAIN" if value.fract() == 0.0 && (-32768.0..=32767.0).contains(&value) => {
                &mut r128_album
            }
            _ => continue,
        };
        slot.get_or_insert(value);
    }
    if let Some(gain_db) = track_gain {
        Some(Loudness {
            gain_db,
            reference_lufs: -18.0,
            peak: track_peak,
        })
    } else if let Some(gain) = r128_track {
        Some(Loudness {
            gain_db: gain / 256.0,
            reference_lufs: -23.0,
            peak: track_peak,
        })
    } else if let Some(gain_db) = album_gain {
        Some(Loudness {
            gain_db,
            reference_lufs: -18.0,
            peak: album_peak,
        })
    } else {
        r128_album.map(|gain| Loudness {
            gain_db: gain / 256.0,
            reference_lufs: -23.0,
            peak: album_peak,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::meta::{METADATA_ID_NULL, MetadataBuilder, MetadataInfo, RawTag, Tag};

    fn metadata(tags: &[(&str, &str)]) -> MetadataRevision {
        let mut builder = MetadataBuilder::new(MetadataInfo {
            metadata: METADATA_ID_NULL,
            short_name: "synthetic",
            long_name: "Synthetic tags",
        });
        for (key, value) in tags {
            builder.add_tag(Tag::new(RawTag::new(*key, *value)));
        }
        builder.build()
    }

    #[test]
    fn replaygain_track_metadata_takes_priority_and_retains_peak() {
        let value = metadata(&[
            ("REPLAYGAIN_ALBUM_GAIN", "-2 dB"),
            ("replaygain_track_gain", "-8.5 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.98"),
        ]);
        assert_eq!(
            from_metadata(&value, 0),
            Some(Loudness {
                gain_db: -8.5,
                reference_lufs: -18.0,
                peak: Some(0.98)
            })
        );
        let mut value = metadata(&[]);
        value.media.tags.push(Tag::new_std(
            RawTag::new("TXXX", "unused"),
            StandardTag::ReplayGainTrackGain("-6 dB".to_owned().into()),
        ));
        assert_eq!(from_metadata(&value, 0).unwrap().gain_db, -6.0);
    }

    #[test]
    fn r128_q78_and_album_fallback_validate_numeric_values() {
        assert_eq!(
            from_metadata(&metadata(&[("R128_TRACK_GAIN", "-512")]), 0),
            Some(Loudness {
                gain_db: -2.0,
                reference_lufs: -23.0,
                peak: None
            })
        );
        assert_eq!(
            from_metadata(&metadata(&[("REPLAYGAIN_ALBUM_GAIN", "-3 DB")]), 0)
                .unwrap()
                .gain_db,
            -3.0
        );
        for (key, value) in [
            ("R128_TRACK_GAIN", "0.5"),
            ("R128_TRACK_GAIN", "32768"),
            ("REPLAYGAIN_TRACK_GAIN", "NaN"),
            ("REPLAYGAIN_TRACK_GAIN", "inf"),
            ("REPLAYGAIN_TRACK_GAIN", "-9999 dB"),
        ] {
            assert!(from_metadata(&metadata(&[(key, value)]), 0).is_none());
        }
    }
}
