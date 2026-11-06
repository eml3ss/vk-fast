use super::*;
use crate::{
    audio_upload::{self, Error, Source},
    audio_upload_ui::{self, Action},
};
use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub(super) struct State {
    owner: Option<i64>,
    form: Option<Form>,
    picker: Option<Receiver<Result<Option<Arc<Source>>, Error>>>,
    request: Option<Request>,
}
#[derive(Default)]
struct Form {
    source: Option<Arc<Source>>,
    title: String,
    artist: String,
    error: Option<Error>,
}
struct Request {
    owner: i64,
    canceled: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    receiver: Receiver<Result<Track, Error>>,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}
impl State {
    pub fn has_modal(&self) -> bool {
        self.form.is_some()
    }
}

impl MusicApp {
    pub(super) fn open_audio_upload(&mut self) {
        let Some(owner) = self
            .session
            .as_ref()
            .filter(|session| session.music.is_some() && !self.demo)
            .map(|s| s.profile.id)
        else {
            return;
        };
        if self.audio_upload.has_modal() {
            return;
        }
        self.audio_upload = State {
            owner: Some(owner),
            form: Some(Form::default()),
            ..Default::default()
        };
    }

    fn choose_audio_upload(&mut self, ctx: &egui::Context) {
        if self.audio_upload.picker.is_some() || self.audio_upload.request.is_some() {
            return;
        }
        if cfg!(test) {
            return;
        }
        let language = self.language;
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        match thread::Builder::new()
            .name("choose-audio-upload".into())
            .spawn(move || {
                let _ = sender.send(audio_upload::choose(language));
                repaint.request_repaint();
            }) {
            Ok(_) => self.audio_upload.picker = Some(receiver),
            Err(_) => {
                if let Some(form) = &mut self.audio_upload.form {
                    form.error = Some(Error::Read)
                }
            }
        }
    }

    fn start_audio_upload(&mut self, ctx: &egui::Context) {
        if self.audio_upload.request.is_some() || self.audio_upload.picker.is_some() {
            return;
        }
        let Some(form) = &mut self.audio_upload.form else {
            return;
        };
        if form.error == Some(Error::Uncertain) {
            return;
        }
        let Some(source) = form.source.clone() else {
            return;
        };
        if !audio_upload::valid_text(&form.title) || !audio_upload::valid_text(&form.artist) {
            form.error = Some(Error::Metadata);
            return;
        }
        let Some((owner, client)) = self
            .session
            .as_ref()
            .filter(|session| Some(session.profile.id) == self.audio_upload.owner)
            .and_then(|session| {
                session
                    .music
                    .clone()
                    .map(|client| (session.profile.id, client))
            })
        else {
            return;
        };
        let title = form.title.trim().to_owned();
        let artist = form.artist.trim().to_owned();
        form.error = None;
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let progress = Arc::new(AtomicU64::new(0));
        let worker_progress = progress.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        let progress_repaint = repaint.clone();
        if cfg!(test) {
            return;
        }
        match thread::Builder::new()
            .name("vk-audio-upload".into())
            .spawn(move || {
                let result = client.upload_audio(
                    owner,
                    &source,
                    &title,
                    &artist,
                    &worker_cancel,
                    move |done, total| {
                        worker_progress
                            .store(done.saturating_mul(10000) / total.max(1), Ordering::Release);
                        progress_repaint.request_repaint();
                    },
                );
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.audio_upload.request = Some(Request {
                    owner,
                    canceled,
                    progress,
                    receiver,
                })
            }
            Err(_) => {
                if let Some(form) = &mut self.audio_upload.form {
                    form.error = Some(Error::Read)
                }
            }
        }
    }

    pub(super) fn poll_audio_upload(&mut self, ctx: &egui::Context) {
        if self
            .audio_upload
            .owner
            .is_some_and(|owner| self.session.as_ref().is_none_or(|s| s.profile.id != owner))
        {
            self.audio_upload = State::default();
            return;
        }
        if let Some(picker) = &self.audio_upload.picker {
            let result = match picker.try_recv() {
                Ok(value) => Some(value),
                Err(TryRecvError::Disconnected) => Some(Err(Error::Read)),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.audio_upload.picker = None;
                if let Some(form) = &mut self.audio_upload.form {
                    match result {
                        Ok(Some(source)) => {
                            form.title = source.title.clone();
                            form.artist = source.artist.clone();
                            form.source = Some(source);
                            form.error = None;
                        }
                        Ok(None) => {}
                        Err(error) => form.error = Some(error),
                    }
                }
            }
        }
        let Some(request) = &self.audio_upload.request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(Error::Uncertain),
        };
        let request = self
            .audio_upload
            .request
            .take()
            .expect("active audio upload");
        if request.canceled.load(Ordering::Acquire)
            || self
                .session
                .as_ref()
                .is_none_or(|s| s.profile.id != request.owner)
        {
            return;
        }
        match result {
            Ok(track) => {
                self.audio_upload = State::default();
                self.apply_library_change(ctx, &track, LibraryChange::Added(track.key()));
                self.track_menu_notice = Some((
                    self.language
                        .text("Трек загружен в VK", "Track uploaded to VK")
                        .into(),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            Err(error) => {
                if let Some(form) = &mut self.audio_upload.form {
                    form.error = Some(error);
                }
            }
        }
    }

    pub(super) fn audio_upload_window(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.audio_upload.form else {
            return;
        };
        let error = form.error.as_ref().map(|e| e.message(self.language));
        let action = audio_upload_ui::show(
            ctx,
            audio_upload_ui::View {
                title: &mut form.title,
                artist: &mut form.artist,
                source: form.source.as_deref(),
                language: self.language,
                picking: self.audio_upload.picker.is_some(),
                progress: self
                    .audio_upload
                    .request
                    .as_ref()
                    .map(|r| r.progress.load(Ordering::Acquire).min(10000) as f32 / 10000.0),
                error: error.as_deref(),
                uncertain: form.error == Some(Error::Uncertain),
            },
        );
        match action {
            Some(Action::Cancel) => self.audio_upload = State::default(),
            Some(Action::Choose) => self.choose_audio_upload(ctx),
            Some(Action::Upload) => self.start_audio_upload(ctx),
            Some(Action::Refresh) => {
                self.audio_upload = State::default();
                self.library_refresh_due = true;
            }
            None => {}
        }
    }
}

#[cfg(test)]
#[path = "audio_upload_app_tests.rs"]
mod tests;
