//! Allocation-free channel mapping for the Windows shared mixer. Sample-rate
//! conversion remains in WASAPI; the decoder/render contract stays 48 kHz stereo.

use super::CHANNELS;

const CHUNK_FRAMES: usize = 512;
const SOURCE_SAMPLES: usize = CHUNK_FRAMES * CHANNELS as usize;

pub(super) struct ChannelMapper {
    channels: usize,
    scratch: Box<[f32; SOURCE_SAMPLES]>,
}

impl ChannelMapper {
    pub(super) fn new(channels: u16) -> Option<Self> {
        (1..=32).contains(&channels).then(|| Self {
            channels: usize::from(channels),
            scratch: Box::new([0.0; SOURCE_SAMPLES]),
        })
    }

    pub(super) fn render(&mut self, output: &mut [f32], render: &mut impl FnMut(&mut [f32])) {
        output.fill(0.0);
        let frames = output.len() / self.channels;
        if self.channels == usize::from(CHANNELS) {
            render(&mut output[..frames * self.channels]);
            return;
        }
        for destination in output[..frames * self.channels].chunks_mut(CHUNK_FRAMES * self.channels)
        {
            let frames = destination.len() / self.channels;
            let source = &mut self.scratch[..frames * usize::from(CHANNELS)];
            source.fill(0.0);
            render(source);
            for (source, destination) in source
                .as_chunks::<2>()
                .0
                .iter()
                .zip(destination.chunks_exact_mut(self.channels))
            {
                if self.channels == 1 {
                    destination[0] = source[0] * 0.5 + source[1] * 0.5;
                } else {
                    destination[0] = source[0];
                    destination[1] = source[1];
                    // Remaining surround/LFE channels were filled with silence.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_is_direct_and_incomplete_frames_stay_silent() {
        let mut mapper = ChannelMapper::new(2).unwrap();
        let mut output = [9.0; 7];
        let mut requested = Vec::new();
        mapper.render(&mut output, &mut |source| {
            requested.push(source.len());
            source.copy_from_slice(&[0.1, -0.1, 0.2, -0.2, 0.3, -0.3]);
        });
        assert_eq!(requested, vec![6]);
        assert_eq!(output, [0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.0]);
    }

    #[test]
    fn mono_downmix_and_surround_front_channels_preserve_source_time() {
        for channels in [1, 6, 8] {
            let mut mapper = ChannelMapper::new(channels).unwrap();
            let mut output = vec![9.0; usize::from(channels) * 3];
            let mut requested = 0;
            mapper.render(&mut output, &mut |source| {
                requested += source.len();
                source.copy_from_slice(&[0.2, 0.6, -0.4, 0.8, 0.9, -0.9]);
            });
            assert_eq!(requested, 6, "Mapping must not advance the track too far");
            if channels == 1 {
                assert_eq!(output, vec![0.4, 0.2, 0.0]);
            } else {
                for (frame, expected) in output.chunks_exact(usize::from(channels)).zip([
                    [0.2, 0.6],
                    [-0.4, 0.8],
                    [0.9, -0.9],
                ]) {
                    assert_eq!(&frame[..2], &expected);
                    assert!(frame[2..].iter().all(|sample| *sample == 0.0));
                }
            }
        }
    }

    #[test]
    fn large_device_buffers_are_chunked_without_allocating_or_losing_frames() {
        let mut mapper = ChannelMapper::new(6).unwrap();
        let original_scratch = mapper.scratch.as_ptr();
        let mut output = vec![7.0; 6 * (CHUNK_FRAMES * 3 + 17) + 1];
        let mut requested = Vec::new();
        let mut source_frame = 0;
        mapper.render(&mut output, &mut |source| {
            requested.push(source.len());
            for frame in source.as_chunks_mut::<2>().0 {
                frame[0] = source_frame as f32;
                frame[1] = -(source_frame as f32);
                source_frame += 1;
            }
        });
        assert_eq!(
            requested,
            vec![SOURCE_SAMPLES, SOURCE_SAMPLES, SOURCE_SAMPLES, 34]
        );
        assert_eq!(mapper.scratch.as_ptr(), original_scratch);
        for (index, frame) in output.as_chunks::<6>().0.iter().enumerate() {
            assert_eq!(frame[0], index as f32);
            assert_eq!(frame[1], -(index as f32));
            assert!(frame[2..].iter().all(|sample| *sample == 0.0));
        }
        assert_eq!(output.last(), Some(&0.0));
        assert!(ChannelMapper::new(0).is_none());
        assert!(ChannelMapper::new(33).is_none());
    }
}
