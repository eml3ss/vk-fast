//! The mix that follows the queue, made ready ahead: while the last queued
//! track plays (repeat off) and "Start a mix when the queue ends" is on, its
//! track mix is fetched at once, so the queue shows what plays next. At the
//! end of the track, on Next, or on a click on one of its rows in the queue,
//! the mix starts from those very tracks without asking VK again.
use super::*;

#[cfg(test)]
#[path = "mix_prepare_tests.rs"]
mod tests;

type MixReceiver = Receiver<Result<crate::web_audio::MixPage, WebAudioError>>;

pub(super) struct PreparedMix {
    owner: i64,
    /// The last queued track the mix is made from.
    seed: (i64, i64),
    request: Option<(Arc<AtomicBool>, MixReceiver)>,
    /// The mix's tracks; none while loading or after a failure, which is
    /// not asked again for the same track.
    tracks: Vec<Track>,
    has_more: bool,
}

impl Drop for PreparedMix {
    fn drop(&mut self) {
        if let Some((canceled, _)) = &self.request {
            canceled.store(true, Ordering::Release);
        }
    }
}

impl MusicApp {
    /// The track whose mix follows the queue: the playing last track, when
    /// nothing else would play after it and the setting asks for a mix.
    fn queue_end_seed(&self) -> Option<(i64, i64)> {
        if !self.mix_after_queue
            || self.mix_active
            || self.demo
            || self.session.is_none()
            || self.adjacent_track(true).is_some()
            || (self.library_queue_active() && !self.library.exhausted)
        {
            return None;
        }
        self.playing
            .as_ref()
            .map(Track::key)
            .filter(|key| key.0 != 0 && key.1 > 0)
    }

    /// Fetches the queue-end mix once per last track and collects it. Runs
    /// with the background tick, also without UI frames.
    pub(super) fn prepare_queue_end_mix(&mut self, ctx: &egui::Context) {
        let seed = self.queue_end_seed();
        if self.prepared_mix.as_ref().map(|prepared| prepared.seed) != seed {
            self.prepared_mix = None;
        }
        if let Some(prepared) = &mut self.prepared_mix {
            let Some((_, receiver)) = &prepared.request else {
                return;
            };
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
            };
            prepared.request = None;
            let owner = self.session.as_ref().map(|session| session.profile.id);
            if owner != Some(prepared.owner) {
                self.prepared_mix = None;
                return;
            }
            if let Ok(page) = result {
                prepared.tracks = page.tracks;
                prepared.has_more = page.has_more;
                ctx.request_repaint();
            }
            return;
        }
        let Some(seed) = seed else { return };
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        let request = if cfg!(test) {
            // Tests fill the mix themselves and never ask VK.
            None
        } else {
            self.workers.retain(|worker| !worker.is_finished());
            thread::Builder::new()
                .name("vk-mix-ahead".into())
                .spawn(move || {
                    let _ = sender.send(client.track_mix(owner, seed, &worker_cancel));
                    repaint.request_repaint();
                })
                .ok()
                .map(|worker| {
                    self.workers.push(worker);
                    (canceled, receiver)
                })
        };
        self.prepared_mix = Some(PreparedMix {
            owner,
            seed,
            request,
            tracks: Vec::new(),
            has_more: false,
        });
    }

    /// The mix shown after the queue, if it is ready for the playing track.
    pub(super) fn prepared_mix_tracks(&self) -> &[Track] {
        match &self.prepared_mix {
            Some(prepared) if Some(prepared.seed) == self.queue_end_seed() => &prepared.tracks,
            _ => &[],
        }
    }

    /// Starts the ready queue-end mix, from `first` or its first track.
    /// Returns false when none is ready, for the caller to fetch one.
    pub(super) fn start_prepared_mix(
        &mut self,
        ctx: &egui::Context,
        first: Option<(i64, i64)>,
    ) -> bool {
        if self.prepared_mix_tracks().is_empty() {
            return false;
        }
        let Some(prepared) = self.prepared_mix.take() else {
            return false;
        };
        let seed = Some(MixSeed::Track(prepared.seed));
        self.mix_request = None;
        self.track_menu_error = None;
        self.mix_error = None;
        self.begin_mix(ctx, seed, prepared.tracks.clone(), prepared.has_more, first);
        true
    }
}

#[cfg(test)]
impl MusicApp {
    pub(super) fn set_prepared_mix_for_test(&mut self, tracks: Vec<Track>) {
        let seed = self.queue_end_seed().expect("a queue-end seed");
        self.prepared_mix = Some(PreparedMix {
            owner: self.session.as_ref().unwrap().profile.id,
            seed,
            request: None,
            tracks,
            has_more: true,
        });
    }
}
