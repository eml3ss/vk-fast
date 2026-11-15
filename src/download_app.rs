//! Downloading tracks into the chosen folder, the folder setting, and the
//! choice of buttons beside a track. The file comes from the audio cache
//! when it is stored whole, otherwise from VK (filling the cache), on a
//! worker; names never reach the log.
use super::*;
use crate::track_buttons::{Button, Set};

#[cfg(test)]
#[path = "download_app_tests.rs"]
mod tests;
use std::{
    io::{Read, Seek, Write},
    path::PathBuf,
};

pub(super) struct DownloadJob {
    key: (i64, i64),
    receiver: mpsc::Receiver<Result<PathBuf, ()>>,
}

impl MusicApp {
    /// The folder downloads go to: the chosen one or the system's.
    pub(super) fn download_dir(&self) -> PathBuf {
        if self.download_dir.is_empty() {
            crate::downloads::default_dir()
        } else {
            PathBuf::from(&self.download_dir)
        }
    }

    pub(super) fn downloading(&self, key: (i64, i64)) -> bool {
        self.downloads.iter().any(|job| job.key == key)
    }

    pub(super) fn download_track(&mut self, ctx: &egui::Context, track: &Track) {
        let key = track.key();
        if self.demo || self.downloading(key) || track.unavailable == Some(true) {
            return;
        }
        let Some(session) = &self.session else { return };
        let music = session.music.clone();
        let cache_key = CacheKey {
            account_id: session.profile.id,
            owner_id: key.0,
            track_id: key.1,
            duration: track.duration,
        };
        let store = self
            .audio_cache
            .clone()
            .map(|cache| crate::audio_cache::MediaStore::new(cache, cache_key));
        let own_url = track.url.clone().filter(|url| url.starts_with("https://"));
        let dir = self.download_dir();
        let artist = track.artist.clone();
        let title = track.display_title().into_owned();
        let album = self
            .album_titles
            .named(track)
            .map(|album| album.title)
            .filter(|title| !title.trim().is_empty());
        let cover_url = track.cover_url.clone();
        let disk = self.covers.disk();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        crate::diagnostics::event("download", "started");
        let started = thread::Builder::new()
            .name("vk-download".into())
            .spawn(move || {
                let canceled = AtomicBool::new(false);
                let result = (|| {
                    let complete = store
                        .as_ref()
                        .is_some_and(|store| store.complete().is_some());
                    let url = if complete {
                        None
                    } else {
                        match music.map(|music| music.resolve_cached(key, &canceled)) {
                            Some(Ok(url)) => Some(url),
                            _ => Some(own_url.ok_or(())?),
                        }
                    };
                    std::fs::create_dir_all(&dir).map_err(|_| ())?;
                    let mut audio = tempfile::NamedTempFile::new_in(&dir).map_err(|_| ())?;
                    let extension = crate::media_decoder::save_whole(
                        url.as_deref(),
                        store,
                        &crate::media_decoder::Cancellation::default(),
                        audio.as_file_mut(),
                    )
                    .map_err(|_| ())?;
                    let mut file = tempfile::NamedTempFile::new_in(&dir).map_err(|_| ())?;
                    let source = audio.as_file_mut();
                    source.seek(std::io::SeekFrom::Start(0)).map_err(|_| ())?;
                    let mut head = [0; 3];
                    let tagged = source.read_exact(&mut head).is_ok() && &head == b"ID3";
                    source.seek(std::io::SeekFrom::Start(0)).map_err(|_| ())?;
                    if extension == "mp3" && !tagged {
                        let cover = cover_url.as_deref().and_then(|url| {
                            crate::cover_art::stored(&disk, url)
                                .or_else(|| crate::cover_art::original(&disk, url).ok())
                        });
                        let tag = crate::downloads::id3(&crate::downloads::Tags {
                            title: &title,
                            artist: &artist,
                            album: album.as_deref(),
                            cover: cover.as_deref(),
                        });
                        file.write_all(&tag).map_err(|_| ())?;
                    }
                    std::io::copy(source, file.as_file_mut()).map_err(|_| ())?;
                    file.as_file_mut().sync_all().map_err(|_| ())?;
                    let name = crate::downloads::file_name(&artist, &title, extension);
                    crate::downloads::place(file, &dir, &name).map_err(|_| ())
                })();
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        match started {
            Ok(worker) => {
                self.workers.push(worker);
                self.downloads.push(DownloadJob { key, receiver });
                self.track_menu_notice = Some((
                    self.language
                        .text("Скачиваем трек…", "Downloading the track…")
                        .into(),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            Err(_) => self.download_failed(),
        }
    }

    fn download_failed(&mut self) {
        crate::diagnostics::event("download", "failed");
        self.track_menu_notice = Some((
            self.language
                .text("Не удалось скачать трек", "Could not download the track")
                .into(),
            Instant::now() + Duration::from_secs(4),
        ));
    }

    pub(super) fn poll_downloads(&mut self) {
        let mut finished = Vec::new();
        self.downloads.retain(|job| match job.receiver.try_recv() {
            Ok(result) => {
                finished.push(result);
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                finished.push(Err(()));
                false
            }
        });
        for result in finished {
            match result {
                Ok(path) => {
                    crate::diagnostics::event("download", "saved");
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.track_menu_notice = Some((
                        format!("{} {name}", self.language.text("Сохранено:", "Saved:")),
                        Instant::now() + Duration::from_secs(4),
                    ));
                }
                Err(()) => self.download_failed(),
            }
        }
    }

    /// Opens the system's folder picker on a worker; the answer arrives in
    /// `poll_download_dir`.
    fn choose_download_dir(&mut self, ctx: &egui::Context) {
        if self.download_dir_pick.is_some() {
            return;
        }
        let start = self.download_dir();
        let title = self
            .language
            .text("Папка для скачанных треков", "Folder for downloaded tracks");
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        if let Ok(worker) = thread::Builder::new()
            .name("vk-download-folder".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .set_title(title)
                    .set_directory(start)
                    .pick_folder();
                let _ = sender.send(picked);
                repaint.request_repaint();
            })
        {
            self.workers.push(worker);
            self.download_dir_pick = Some(receiver);
        }
    }

    pub(super) fn poll_download_dir(&mut self) {
        let Some(receiver) = &self.download_dir_pick else {
            return;
        };
        match receiver.try_recv() {
            Ok(picked) => {
                self.download_dir_pick = None;
                if let Some(path) = picked.filter(|path| path.is_absolute()) {
                    self.set_download_dir(path.to_string_lossy().into_owned());
                }
            }
            Err(TryRecvError::Disconnected) => self.download_dir_pick = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    fn set_download_dir(&mut self, path: String) {
        self.download_dir = path;
        self.download_dir_save_failed =
            !cfg!(test) && preferences::save_download_dir(&self.download_dir).is_err();
    }

    pub(super) fn download_folder_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let language = self.language;
        ui.label(RichText::new(language.text("Папка сохранения", "Download folder")).strong());
        ui.label(
            RichText::new(language.text(
                "Куда сохраняются скачанные треки.",
                "Where downloaded tracks are saved.",
            ))
            .small()
            .color(theme::muted()),
        );
        ui.add_space(4.0);
        let shown = self.download_dir();
        ui.add(
            egui::Label::new(RichText::new(shown.to_string_lossy()).monospace())
                .truncate()
                .selectable(false),
        )
        .on_hover_text(shown.to_string_lossy());
        ui.horizontal(|ui| {
            let busy = self.download_dir_pick.is_some();
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(language.text("Изменить…", "Change…")),
                )
                .clicked()
            {
                self.choose_download_dir(ctx);
            }
            if ui
                .add_enabled(
                    !self.download_dir.is_empty(),
                    egui::Button::new(language.text("По умолчанию", "Default")),
                )
                .on_hover_text(language.text(
                    "Системная папка «Загрузки»",
                    "The system's Downloads folder",
                ))
                .clicked()
            {
                self.set_download_dir(String::new());
            }
        });
        if self.download_dir_save_failed {
            ui.colored_label(
                theme::error(),
                language.text("Не удалось сохранить папку.", "Could not save the folder."),
            );
        }
    }

    pub(super) fn track_buttons_settings(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        ui.label(RichText::new(language.text("Кнопки у трека", "Track buttons")).strong());
        ui.label(
            RichText::new(language.text(
                "Какие кнопки показывать справа от названия трека.",
                "Which buttons to show beside a track's title.",
            ))
            .small()
            .color(theme::muted()),
        );
        ui.add_space(6.0);
        let (mut player, mut lists) = (self.track_buttons_player, self.track_buttons_lists);
        // Laid out by hand: each place is a fixed column, its heading and
        // its boxes centred on the same line.
        const PLACE: f32 = 100.0;
        const ROW: f32 = 32.0;
        let body = egui::TextStyle::Body.resolve(ui.style());
        let small = egui::TextStyle::Small.resolve(ui.style());
        let names = ui.fonts_mut(|fonts| {
            Button::ALL
                .iter()
                .map(|button| {
                    fonts
                        .layout_no_wrap(button.label(language).into(), body.clone(), Color32::WHITE)
                        .size()
                        .x
                })
                .fold(0.0, f32::max)
        });
        let names = 16.0 + 10.0 + names + 20.0;
        let places = [
            (
                language.text("В плеере", "In the player"),
                language.text("в плеере", "in the player"),
            ),
            (
                language.text("В списках", "In lists"),
                language.text("в списках", "in lists"),
            ),
        ];
        let (header, _) =
            ui.allocate_exact_size(egui::vec2(names + PLACE * 2.0, 18.0), egui::Sense::hover());
        for (index, (heading, _)) in places.iter().enumerate() {
            ui.painter().text(
                egui::pos2(
                    header.left() + names + PLACE * (index as f32 + 0.5),
                    header.center().y,
                ),
                egui::Align2::CENTER_CENTER,
                *heading,
                small.clone(),
                theme::muted(),
            );
        }
        for button in Button::ALL {
            let (row, _) =
                ui.allocate_exact_size(egui::vec2(names + PLACE * 2.0, ROW), egui::Sense::hover());
            crate::icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    row.left_center() + egui::vec2(8.0, 0.0),
                    egui::vec2(16.0, 16.0),
                ),
                button.icon(),
                theme::muted(),
            );
            ui.painter().text(
                row.left_center() + egui::vec2(26.0, 0.0),
                egui::Align2::LEFT_CENTER,
                button.label(language),
                body.clone(),
                ui.visuals().text_color(),
            );
            for (index, (_, place)) in places.iter().enumerate() {
                let set = if index == 0 { &mut player } else { &mut lists };
                let mut shown = set.has(button);
                let label = format!("{} {place}", button.label(language));
                let cell = egui::Rect::from_center_size(
                    egui::pos2(
                        row.left() + names + PLACE * (index as f32 + 0.5),
                        row.center().y,
                    ),
                    egui::vec2(22.0, 22.0),
                );
                let changed = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(cell), |ui| {
                        theme::check(ui, &mut shown, &label).changed()
                    })
                    .inner;
                if changed {
                    *set = set.with(button, shown);
                }
            }
        }
        if (player, lists) != (self.track_buttons_player, self.track_buttons_lists) {
            self.track_buttons_player = player;
            self.track_buttons_lists = lists;
            self.track_buttons_save_failed =
                !cfg!(test) && preferences::save_track_buttons(player, lists).is_err();
        }
        if self.track_buttons_save_failed {
            ui.colored_label(
                theme::error(),
                language.text(
                    "Не удалось сохранить настройку.",
                    "Could not save the setting.",
                ),
            );
        }
    }

    /// One of the small buttons beside a track, as the player and rows draw it.
    pub(super) fn track_button(
        &self,
        ui: &mut egui::Ui,
        track: &Track,
        button: Button,
    ) -> egui::Response {
        match button {
            Button::Like => self.like_button(ui, track),
            Button::Dislike => self.dislike_button(ui, track),
            Button::Mix => self.track_mix_button(ui),
            Button::Download => {
                let response = crate::icons::track_button(
                    ui,
                    Icon::Save,
                    Button::Download.label(self.language),
                    !self.demo && self.session.is_some() && track.unavailable != Some(true),
                    false,
                    self.downloading(track.key()),
                );
                release_pointer_focus(&response);
                response
            }
        }
    }

    /// What a click on `button` beside `track` does.
    pub(super) fn track_button_action(track: &Track, button: Button) -> TrackAction {
        match button {
            Button::Like => TrackAction::ToggleLike(track.clone()),
            Button::Dislike => TrackAction::Menu(track.clone(), track_menu::Action::ToggleDislike),
            Button::Mix => TrackAction::Menu(track.clone(), track_menu::Action::TrackMix),
            Button::Download => TrackAction::Menu(track.clone(), track_menu::Action::Download),
        }
    }
}

/// The width a list row keeps on the right for `buttons` and the duration.
pub(super) fn controls_width(buttons: Set) -> f32 {
    58.0 + 28.0 * buttons.count() as f32
}
