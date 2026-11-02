//! Coalesced preference writes; closing the app flushes the last chosen modes.
use crate::{diagnostics, play_order::RepeatMode, preferences};
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};

type Modes = (bool, RepeatMode);
#[derive(Default)]
pub struct Writer {
    latest: Arc<Mutex<Option<Modes>>>,
    wake: Option<SyncSender<()>>,
    worker: Option<JoinHandle<()>>,
}
impl Writer {
    fn with_sink(save: impl Fn(Modes) + Send + 'static) -> std::io::Result<Self> {
        let latest = Arc::new(Mutex::new(None));
        let pending = latest.clone();
        let (wake, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("vk-play-modes".into())
            .spawn(move || {
                while receive.recv().is_ok() {
                    let modes = pending.lock().ok().and_then(|mut modes| modes.take());
                    if let Some(modes) = modes {
                        save(modes);
                    }
                }
            })?;
        Ok(Self {
            latest,
            wake: Some(wake),
            worker: Some(worker),
        })
    }

    pub fn save(&mut self, shuffle: bool, repeat: RepeatMode) {
        if self.wake.is_none() {
            if cfg!(test) {
                return;
            }
            match Self::with_sink(|(shuffle, repeat)| {
                if preferences::save_playback_modes(shuffle, repeat).is_err() {
                    diagnostics::event("preferences", "Could not save playback modes");
                }
            }) {
                Ok(writer) => *self = writer,
                Err(_) => {
                    diagnostics::event("preferences", "Playback preference writer unavailable");
                    return;
                }
            }
        }
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some((shuffle, repeat));
        }
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.wake.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_flushes_the_latest_modes_after_rapid_changes() {
        let saved = Arc::new(Mutex::new(None));
        let output = saved.clone();
        let mut writer =
            Writer::with_sink(move |modes| *output.lock().unwrap() = Some(modes)).unwrap();
        for index in 0..100 {
            writer.save(index % 2 == 0, RepeatMode::All);
        }
        writer.save(true, RepeatMode::One);
        drop(writer);
        assert_eq!(*saved.lock().unwrap(), Some((true, RepeatMode::One)));
    }
}
