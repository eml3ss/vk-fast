//! Native PipeWire (Linux) and WASAPI (Windows) output. Backend objects stay on their owning thread;
//! callers only hold a stop flag and the thread's join handle.
//! API references: pipewire-rs 0.10.1 stream/main-loop documentation and the
//! PipeWire raw-audio stream format contract. No ALSA/Pulse compatibility path.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

pub struct OutputStream {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for OutputStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Starts one native 48 kHz stereo F32 stream. The callback must fill its slice
/// without blocking. Success means the platform stream has started.
pub fn open(
    render: impl FnMut(&mut [f32]) + Send + 'static,
    on_error: impl Fn() + Send + 'static,
) -> Result<OutputStream, ()> {
    #[cfg(target_os = "linux")]
    {
        linux::open(render, on_error)
    }
    #[cfg(target_os = "windows")]
    {
        windows::open(render, on_error)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (render, on_error);
        Err(())
    }
}

#[cfg(any(target_os = "windows", test))]
mod windows_channels;

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        sync::mpsc::{self, SyncSender},
        thread,
        time::Duration,
    };

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::{windows_channels::ChannelMapper, *};

    const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
    const ACTIVATE_TIMEOUT: Duration = Duration::from_secs(4);
    const STOP_INTERVAL: Duration = Duration::from_millis(20);

    pub(super) fn open(
        render: impl FnMut(&mut [f32]) + Send + 'static,
        on_error: impl Fn() + Send + 'static,
    ) -> Result<OutputStream, ()> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("vk-wasapi".into())
            .spawn(move || {
                if run(render, on_error, worker_stop, &sender).is_err() {
                    let _ = sender.try_send(Err(()));
                }
            })
            .map_err(|_| ())?;
        match receiver.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(OutputStream {
                stop,
                worker: Some(worker),
            }),
            _ => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(())
            }
        }
    }

    fn run(
        mut render: impl FnMut(&mut [f32]) + Send + 'static,
        on_error: impl Fn() + Send + 'static,
        stop: Arc<AtomicBool>,
        ready: &SyncSender<Result<(), ()>>,
    ) -> Result<(), ()> {
        let host = cpal::host_from_id(cpal::HostId::Wasapi).map_err(|_| ())?;
        let device = host.default_output_device().ok_or(())?;
        let channels = device.default_output_config().map_err(|_| ())?.channels();
        let mut mapper = ChannelMapper::new(channels).ok_or(())?;
        let config = cpal::StreamConfig {
            channels,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };
        // CPAL 0.18's WASAPI output initializes shared mode with
        // AUTOCONVERTPCM | SRC_DEFAULT_QUALITY. Windows converts this 48 kHz
        // F32 stream to the device's mix rate/format, including Bluetooth.
        // Keep its native channel count; mono is downmixed and surround uses
        // front L/R. No custom resampler or integer conversion is needed.
        let failed = Arc::new(AtomicBool::new(false));
        let render_stop = stop.clone();
        let render_failed = failed.clone();
        let error_stop = stop.clone();
        let error_failed = failed.clone();
        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if render_stop.load(Ordering::Acquire) || render_failed.load(Ordering::Acquire)
                    {
                        data.fill(0.0);
                    } else {
                        mapper.render(data, &mut render);
                    }
                },
                move |_| {
                    if !error_stop.load(Ordering::Acquire)
                        && !error_failed.swap(true, Ordering::AcqRel)
                    {
                        on_error();
                    }
                },
                Some(ACTIVATE_TIMEOUT),
            )
            .map_err(|_| ())?;
        if stop.load(Ordering::Acquire) {
            return Err(());
        }
        stream.play().map_err(|_| ())?;
        if failed.load(Ordering::Acquire) {
            return Err(());
        }
        let _ = ready.try_send(Ok(()));
        while !stop.load(Ordering::Acquire) && !failed.load(Ordering::Acquire) {
            thread::sleep(STOP_INTERVAL);
        }
        let _ = stream.pause();
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        cell::{Cell, RefCell},
        io::Cursor,
        rc::Rc,
        sync::mpsc::{self, SyncSender},
        thread,
        time::{Duration, Instant},
    };

    use pipewire::{self as pw, properties::properties, spa};

    use super::*;

    const STOP_INTERVAL: Duration = Duration::from_millis(20);
    const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
    const STRIDE: usize = CHANNELS as usize * std::mem::size_of::<f32>();
    const SCRATCH_SAMPLES: usize = 65_536;

    struct Status {
        ready: RefCell<Option<SyncSender<Result<(), ()>>>>,
        failed: Cell<bool>,
        connected: Cell<bool>,
    }

    impl Status {
        fn ready(&self) {
            if self.failed.get() {
                return;
            }
            self.connected.set(true);
            if let Some(sender) = self.ready.borrow_mut().take() {
                let _ = sender.try_send(Ok(()));
            }
        }

        fn fail(&self, on_error: &dyn Fn()) {
            if !self.failed.replace(true) {
                if let Some(sender) = self.ready.borrow_mut().take() {
                    let _ = sender.try_send(Err(()));
                }
                on_error();
            }
        }
    }

    pub(super) fn open(
        render: impl FnMut(&mut [f32]) + Send + 'static,
        on_error: impl Fn() + Send + 'static,
    ) -> Result<OutputStream, ()> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("vk-pipewire".into())
            .spawn(move || {
                let on_error: Rc<dyn Fn()> = Rc::new(on_error);
                let status = Rc::new(Status {
                    ready: RefCell::new(Some(sender)),
                    failed: Cell::new(false),
                    connected: Cell::new(false),
                });
                if run(
                    render,
                    on_error.clone(),
                    status.clone(),
                    worker_stop.clone(),
                )
                .is_err()
                    && !worker_stop.load(Ordering::Acquire)
                {
                    status.fail(on_error.as_ref());
                }
            })
            .map_err(|_| ())?;
        match receiver.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(OutputStream {
                stop,
                worker: Some(worker),
            }),
            _ => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(())
            }
        }
    }

    fn run(
        mut render: impl FnMut(&mut [f32]) + Send + 'static,
        on_error: Rc<dyn Fn()>,
        status: Rc<Status>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), ()> {
        let mainloop = Rc::new(pw::main_loop::MainLoopBox::new(None).map_err(|_| ())?);
        let context = pw::context::ContextBox::new(
            mainloop.loop_(),
            Some(properties! {
                *pw::keys::APP_NAME => "VK Fast",
                *pw::keys::APP_ID => "vk-fast",
            }),
        )
        .map_err(|_| ())?;
        let core = context.connect(None).map_err(|_| ())?;

        let errors_loop = mainloop.clone();
        let errors_status = status.clone();
        let errors_callback = on_error.clone();
        let errors_stop = stop.clone();
        let _core_listener = core
            .add_listener_local()
            .error(move |_, _, _, _| {
                if !errors_stop.load(Ordering::Acquire) {
                    errors_status.fail(errors_callback.as_ref());
                }
                errors_loop.quit();
            })
            .register();

        let stream = pw::stream::StreamBox::new(
            &core,
            "vk-fast-player",
            properties! {
                *pw::keys::APP_NAME => "VK Fast",
                *pw::keys::NODE_NAME => "vk-fast-player",
                *pw::keys::NODE_DESCRIPTION => "VK Fast",
                *pw::keys::MEDIA_NAME => "VK Fast music",
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::MEDIA_ROLE => "Music",
                *pw::keys::AUDIO_CHANNELS => "2",
                *pw::keys::AUDIO_RATE => "48000",
                *pw::keys::NODE_LATENCY => "1024/48000",
            },
        )
        .map_err(|_| ())?;

        let state_loop = mainloop.clone();
        let state_status = status.clone();
        let state_error = on_error.clone();
        let state_stop = stop.clone();
        let format_loop = mainloop.clone();
        let format_status = status.clone();
        let format_error = on_error.clone();
        let process_stop = stop.clone();
        // Allocate once on the owner thread. The process callback performs no
        // allocation, float-to-byte cast, blocking I/O, or mutex acquisition.
        let mut scratch = vec![0.0_f32; SCRATCH_SAMPLES];
        let _stream_listener = stream
            .add_local_listener::<()>()
            .state_changed(move |_, _, _, new| match new {
                pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming => {
                    state_status.ready()
                }
                pw::stream::StreamState::Error(_) => {
                    if !state_stop.load(Ordering::Acquire) {
                        state_status.fail(state_error.as_ref());
                    }
                    state_loop.quit();
                }
                pw::stream::StreamState::Unconnected if state_status.connected.get() => {
                    if !state_stop.load(Ordering::Acquire) {
                        state_status.fail(state_error.as_ref());
                    }
                    state_loop.quit();
                }
                _ => (),
            })
            .param_changed(move |_, _, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    return;
                };
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err()
                    || info.format() != spa::param::audio::AudioFormat::F32LE
                    || info.rate() != SAMPLE_RATE
                    || info.channels() != u32::from(CHANNELS)
                {
                    format_status.fail(format_error.as_ref());
                    format_loop.quit();
                }
            })
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let requested = usize::try_from(buffer.requested()).unwrap_or(usize::MAX);
                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let frames = if let Some(bytes) = data.data() {
                    let frames = frame_count(bytes.len(), requested, scratch.len());
                    let samples = &mut scratch[..frames * usize::from(CHANNELS)];
                    samples.fill(0.0);
                    if !process_stop.load(Ordering::Acquire) {
                        render(samples);
                    }
                    write_samples(&mut bytes[..frames * STRIDE], samples);
                    frames
                } else {
                    0
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = STRIDE as i32;
                *chunk.size_mut() = (frames * STRIDE) as u32;
            })
            .register()
            .map_err(|_| ())?;

        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_format(spa::param::audio::AudioFormat::F32LE);
        info.set_rate(SAMPLE_RATE);
        info.set_channels(u32::from(CHANNELS));
        let mut positions = [0; spa::param::audio::MAX_CHANNELS];
        positions[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        positions[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
        info.set_position(positions);
        let format = spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        });
        let encoded =
            spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &format)
                .map_err(|_| ())?
                .0
                .into_inner();
        let format = spa::pod::Pod::from_bytes(&encoded).ok_or(())?;
        stream
            .connect(
                spa::utils::Direction::Output,
                None,
                pw::stream::StreamFlags::AUTOCONNECT
                    | pw::stream::StreamFlags::MAP_BUFFERS
                    | pw::stream::StreamFlags::RT_PROCESS,
                &mut [format],
            )
            .map_err(|_| ())?;

        let timer_loop = mainloop.clone();
        let timer_stop = stop.clone();
        let timer_status = status.clone();
        let timer_error = on_error.clone();
        let deadline = Instant::now() + OPEN_TIMEOUT - Duration::from_millis(100);
        let timer = mainloop.loop_().add_timer(move |_| {
            if timer_stop.load(Ordering::Acquire) {
                timer_loop.quit();
            } else if !timer_status.connected.get() && Instant::now() >= deadline {
                timer_status.fail(timer_error.as_ref());
                timer_loop.quit();
            }
        });
        timer
            .update_timer(Some(STOP_INTERVAL), Some(STOP_INTERVAL))
            .into_result()
            .map_err(|_| ())?;
        mainloop.run();
        let _ = stream.disconnect();
        Ok(())
    }

    fn frame_count(bytes: usize, requested: usize, scratch_samples: usize) -> usize {
        let available = (bytes / STRIDE).min(scratch_samples / usize::from(CHANNELS));
        if requested == 0 {
            available
        } else {
            available.min(requested)
        }
    }

    fn write_samples(bytes: &mut [u8], samples: &[f32]) {
        for (target, sample) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(samples) {
            *target = sample.to_le_bytes();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn native_buffers_respect_quantum_channels_and_scratch_capacity() {
            assert_eq!(frame_count(8192, 128, 2048), 128);
            assert_eq!(frame_count(8192, 0, 2048), 1024);
            assert_eq!(frame_count(8192, 2048, 256), 128);
            assert_eq!(frame_count(7, 128, 2048), 0);
            let mut bytes = [0; 8];
            write_samples(&mut bytes, &[0.25, -0.5]);
            assert_eq!(f32::from_le_bytes(bytes[..4].try_into().unwrap()), 0.25);
            assert_eq!(f32::from_le_bytes(bytes[4..].try_into().unwrap()), -0.5);
        }
    }
}
