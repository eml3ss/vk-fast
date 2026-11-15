//! "Add tracks" for the account's own playlist: a window over the playlist
//! page with the account's music and VK search, each track added with its
//! plus. Additions go to VK one at a time on a worker; the page and the
//! sidebar are reloaded once the window closes. Queries never reach logs.
use super::*;

#[cfg(test)]
#[path = "playlist_add_app_tests.rs"]
mod tests;

/// Library matches shown under a query.
const LIBRARY_MATCHES: usize = 50;
const ROW: f32 = 52.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Added {
    Pending,
    Done,
    Failed,
}

type Reply = ((i64, i64), Result<(i64, i64), WebAudioError>);

pub(super) struct AddTracks {
    playlist: AlbumRef,
    query: String,
    /// The query VK was asked for, and when to ask for the typed one.
    searched: String,
    search_due: Option<Instant>,
    search: Option<SearchRequest<crate::web_audio::SearchResults>>,
    found: Vec<Track>,
    search_error: Option<WebAudioError>,
    /// Tracks already in the playlist when the window opened.
    present: HashSet<(i64, i64)>,
    states: HashMap<(i64, i64), Added>,
    tracks: HashMap<(i64, i64), Track>,
    jobs: Option<mpsc::Sender<Track>>,
    replies: mpsc::Receiver<Reply>,
    canceled: Arc<AtomicBool>,
    added: usize,
    /// Closed while additions were still on their way: hidden, waiting for
    /// them before the page is reloaded.
    closing: bool,
}

#[cfg(test)]
impl AddTracks {
    pub(super) fn send_for_test(&mut self, track: &Track) {
        MusicApp::add_from_dialog(self, track);
    }
    pub(super) fn added_count(&self) -> usize {
        self.added
    }
    pub(super) fn set_query(&mut self, query: &str) {
        self.query = query.into();
    }
}

impl AddTracks {
    /// The window's state, with the worker's ends: where pressed tracks go,
    /// and where its answers come back.
    pub(super) fn new(
        playlist: AlbumRef,
        present: HashSet<(i64, i64)>,
    ) -> (Self, mpsc::Receiver<Track>, mpsc::Sender<Reply>) {
        let (jobs, work) = mpsc::channel();
        let (reply, replies) = mpsc::channel();
        (
            Self {
                playlist,
                query: String::new(),
                searched: String::new(),
                search_due: None,
                search: None,
                found: Vec::new(),
                search_error: None,
                present,
                states: HashMap::new(),
                tracks: HashMap::new(),
                jobs: Some(jobs),
                replies,
                canceled: Arc::new(AtomicBool::new(false)),
                added: 0,
                closing: false,
            },
            work,
            reply,
        )
    }
}

enum Entry {
    Heading(&'static str),
    Note(String),
    Track(Box<Track>),
}

impl MusicApp {
    pub(super) fn open_add_tracks(&mut self, ctx: &egui::Context, playlist: AlbumRef) {
        if self.demo || !self.search_ready() || self.add_tracks.is_some() {
            return;
        }
        let Some(session) = &self.session else { return };
        let Some(client) = session.music.clone() else {
            return;
        };
        let owner = session.profile.id;
        let present = self
            .details
            .get(
                &MusicPage::Playlist(playlist.clone())
                    .detail_key()
                    .unwrap_or_default(),
            )
            .and_then(|page| match page.as_ref() {
                DetailPage::Playlist(page) | DetailPage::Album(page) => {
                    Some(page.tracks.iter().map(Track::key).collect())
                }
                _ => None,
            })
            .unwrap_or_default();
        let (dialog, work, reply) = AddTracks::new(playlist.clone(), present);
        let worker_cancel = dialog.canceled.clone();
        let target = playlist.clone();
        let repaint = self.repaint_target.with_context(ctx);
        let started = thread::Builder::new()
            .name("vk-playlist-add-many".into())
            .spawn(move || {
                // One at a time, in the order they were pressed; closing the
                // window drops the sender and ends the loop.
                while let Ok(track) = work.recv() {
                    if worker_cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let key = track.key();
                    let result = client.add_to_playlist(owner, &target, key, &worker_cancel);
                    let _ = reply.send((key, result));
                    repaint.request_repaint();
                }
            });
        let Ok(worker) = started else {
            self.track_menu_error = Some(WebAudioError::Network);
            return;
        };
        self.workers.push(worker);
        crate::diagnostics::event("playlist", "add tracks: opened");
        self.add_tracks = Some(dialog);
    }

    pub(super) fn add_tracks_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.add_tracks.take() else {
            return;
        };
        self.poll_add_tracks(ctx, &mut dialog);
        if !dialog.closing && self.draw_add_tracks(ctx, &mut dialog) {
            // Nothing new is sent; what was pressed is still added.
            dialog.jobs = None;
            dialog.closing = true;
        }
        let pending = dialog.states.values().any(|state| *state == Added::Pending);
        if dialog.closing && !pending {
            self.close_add_tracks(ctx, dialog);
        } else {
            self.add_tracks = Some(dialog);
        }
    }

    fn poll_add_tracks(&mut self, ctx: &egui::Context, dialog: &mut AddTracks) {
        while let Ok((key, result)) = dialog.replies.try_recv() {
            match result {
                Ok(owned) => {
                    dialog.states.insert(key, Added::Done);
                    dialog.added += 1;
                    if let Some(track) = dialog.tracks.get(&key).cloned()
                        && self
                            .session
                            .as_ref()
                            .is_some_and(|session| session.profile.id == owned.0)
                    {
                        self.apply_library_change(ctx, &track, LibraryChange::Added(owned));
                    }
                }
                Err(_) => {
                    dialog.states.insert(key, Added::Failed);
                }
            }
        }
        // VK search for the typed query, a moment after typing stops.
        let query = dialog.query.trim().to_owned();
        if query != dialog.searched {
            if query.is_empty() {
                dialog.searched.clear();
                dialog.search = None;
                dialog.search_due = None;
                dialog.found.clear();
                dialog.search_error = None;
            } else if dialog.search_due.is_none() {
                dialog.search_due = Some(Instant::now() + Duration::from_millis(350));
            }
        }
        if let Some(due) = dialog.search_due {
            let now = Instant::now();
            if now < due {
                ctx.request_repaint_after(due - now);
            } else {
                dialog.search_due = None;
                dialog.searched = query.clone();
                dialog.search_error = None;
                let worker_query = query.clone();
                match self.spawn_search(ctx, query, 0, move |client, owner, cancel| {
                    client.search(owner, &worker_query, 0, cancel)
                }) {
                    Ok(request) => dialog.search = Some(request),
                    Err(error) => dialog.search_error = Some(error),
                }
            }
        }
        if let Some((request, result)) = search_reply(&mut dialog.search)
            && request.query == dialog.searched
        {
            match result {
                Ok(results) => dialog.found = results.tracks,
                Err(WebAudioError::Canceled) => {}
                Err(error) => {
                    dialog.found.clear();
                    dialog.search_error = Some(error);
                }
            }
        }
    }

    fn add_from_dialog(dialog: &mut AddTracks, track: &Track) {
        let key = track.key();
        if dialog.present.contains(&key)
            || matches!(dialog.states.get(&key), Some(Added::Pending | Added::Done))
        {
            return;
        }
        let Some(jobs) = &dialog.jobs else { return };
        if jobs.send(track.clone()).is_ok() {
            dialog.states.insert(key, Added::Pending);
            dialog.tracks.insert(key, track.clone());
        }
    }

    fn close_add_tracks(&mut self, ctx: &egui::Context, dialog: AddTracks) {
        dialog.canceled.store(true, Ordering::Release);
        crate::diagnostics::event(
            "playlist",
            &format!("add tracks: closed added={}", dialog.added),
        );
        if dialog.added == 0 {
            return;
        }
        let playlist_key = MusicPage::Playlist(dialog.playlist.clone()).detail_key();
        let active = self.page.detail_key();
        if let Some(key) = playlist_key {
            if active.as_deref() == Some(key.as_str()) {
                self.ensure_detail(ctx, true);
            } else {
                self.details.remove(&key);
            }
        }
        self.load_playlists(ctx, true);
        self.track_menu_notice = Some((
            format!(
                "{} {}",
                self.language.text("Добавлено треков:", "Tracks added:"),
                dialog.added
            ),
            Instant::now() + Duration::from_secs(3),
        ));
    }

    /// The window; returns whether it closes.
    fn draw_add_tracks(&mut self, ctx: &egui::Context, dialog: &mut AddTracks) -> bool {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let content = ctx.content_rect();
        let width = (content.width() - 128.0).clamp(420.0, 640.0);
        let height = (content.height() - 220.0).clamp(320.0, 620.0);
        let query = dialog.query.trim().to_lowercase();
        let mut entries = Vec::new();
        entries.push(Entry::Heading(t("Моя музыка", "My music")));
        let mine: Vec<Track> = if query.is_empty() {
            self.library.tracks.clone()
        } else {
            self.library
                .tracks
                .iter()
                .filter(|track| {
                    track.title.to_lowercase().contains(&query)
                        || track.artist.to_lowercase().contains(&query)
                })
                .take(LIBRARY_MATCHES)
                .cloned()
                .collect()
        };
        if mine.is_empty() {
            entries.push(Entry::Note(
                t("Ничего не найдено.", "Nothing found.").to_owned(),
            ));
        }
        entries.extend(mine.into_iter().map(|track| Entry::Track(Box::new(track))));
        if !query.is_empty() {
            entries.push(Entry::Heading(t("Поиск VK", "VK search")));
            if let Some(error) = &dialog.search_error {
                entries.push(Entry::Note(error.localized(language)));
            } else if dialog.search.is_some() || dialog.search_due.is_some() {
                entries.push(Entry::Note(t("Ищем…", "Searching…").to_owned()));
            } else if dialog.found.is_empty() {
                entries.push(Entry::Note(
                    t("Ничего не найдено.", "Nothing found.").to_owned(),
                ));
            }
            entries.extend(
                dialog
                    .found
                    .iter()
                    .map(|track| Entry::Track(Box::new(track.clone()))),
            );
        }
        let mut close = false;
        let mut chosen = None;
        let modal = egui::Modal::new(egui::Id::new("add-tracks-to-playlist"))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .fill(theme::surface())
                    .inner_margin(24)
                    .corner_radius(20)
                    .stroke(egui::Stroke::new(
                        1.0,
                        theme::tone(Color32::from_rgb(44, 53, 67)),
                    )),
            )
            .backdrop_color(Color32::from_black_alpha(180))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("Добавить треки", "Add tracks"))
                                .size(20.0)
                                .family(theme::semibold()),
                        )
                        .selectable(false),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close_label = t("Закрыть", "Close");
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
                        if response.hovered() || response.has_focus() {
                            ui.painter().rect_filled(rect, 8.0, theme::selected());
                        }
                        icons::paint(ui.painter(), rect.shrink(7.0), Icon::Close, theme::muted());
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, close_label)
                        });
                        close |= response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(close_label)
                            .clicked();
                    });
                });
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} «{}»",
                            t("В плейлист", "To the playlist"),
                            dialog.playlist.title
                        ))
                        .color(theme::muted()),
                    )
                    .truncate()
                    .selectable(false),
                );
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(theme::background())
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.spacing_mut().interact_size.y = 22.0;
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.horizontal(|ui| {
                            icons::show(ui, Icon::Search, theme::muted(), 16.0);
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut dialog.query)
                                    .id_salt("add-tracks-search")
                                    .char_limit(512)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(ui.available_width())
                                    .hint_text(t(
                                        "Поиск в моей музыке и VK",
                                        "Search My music and VK",
                                    )),
                            );
                            if ui.memory(|memory| memory.focused().is_none()) {
                                edit.request_focus();
                            }
                        });
                    });
                ui.add_space(10.0);
                theme::scroll_area(
                    ui,
                    egui::ScrollArea::vertical()
                        .id_salt("add-tracks-list")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, false]),
                    |ui| {
                        let width = ui.available_width();
                        let (whole, _) = ui.allocate_exact_size(
                            egui::vec2(width, ROW * entries.len() as f32),
                            Sense::hover(),
                        );
                        let visible = ui.clip_rect();
                        for (index, entry) in entries.iter().enumerate() {
                            let rect = egui::Rect::from_min_size(
                                whole.min + egui::vec2(0.0, ROW * index as f32),
                                egui::vec2(width, ROW),
                            );
                            if !visible.intersects(rect) {
                                continue;
                            }
                            match entry {
                                Entry::Heading(text) => {
                                    ui.painter().text(
                                        rect.left_bottom() + egui::vec2(4.0, -12.0),
                                        egui::Align2::LEFT_BOTTOM,
                                        *text,
                                        egui::FontId::new(15.0, theme::semibold()),
                                        ui.visuals().text_color(),
                                    );
                                }
                                Entry::Note(text) => {
                                    ui.painter().text(
                                        rect.left_center() + egui::vec2(4.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        text,
                                        egui::TextStyle::Body.resolve(ui.style()),
                                        theme::muted(),
                                    );
                                }
                                Entry::Track(track) => {
                                    let state = if dialog.present.contains(&track.key()) {
                                        Some(None)
                                    } else {
                                        dialog.states.get(&track.key()).copied().map(Some)
                                    };
                                    if self.add_tracks_row(ui, rect, track, state, index) {
                                        chosen = Some(track.as_ref().clone());
                                    }
                                }
                            }
                        }
                    },
                );
            });
        if let Some(track) = chosen {
            Self::add_from_dialog(dialog, &track);
        }
        close || modal.should_close()
    }

    /// One track with its plus; `state` is `Some(None)` for a track the
    /// playlist already has. Returns whether its plus was clicked.
    fn add_tracks_row(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        track: &Track,
        state: Option<Option<Added>>,
        index: usize,
    ) -> bool {
        let language = self.language;
        let t = |ru, en| language.text(ru, en);
        let inner = rect.shrink2(egui::vec2(4.0, 6.0));
        let hovered = ui.rect_contains_pointer(inner);
        if hovered {
            ui.painter().rect_filled(inner, 8.0, theme::selected());
        }
        let mut clicked = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(("add-track-row", index, track.key()))
                .max_rect(inner.shrink2(egui::vec2(6.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                self.covers.show(ui, track.cover_url.as_deref(), 36.0);
                let text_width = (ui.available_width() - 90.0).max(40.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, 36.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(text_width);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(
                            egui::Label::new(
                                RichText::new(track.display_title())
                                    .family(theme::semibold())
                                    .size(14.0),
                            )
                            .truncate()
                            .selectable(false),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(&track.artist).small().color(theme::muted()),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match state {
                        Some(None) | Some(Some(Added::Done)) => {
                            let done = state == Some(Some(Added::Done));
                            let (rect, response) =
                                ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                            icons::paint(
                                ui.painter(),
                                rect.shrink(3.0),
                                Icon::Check,
                                if done {
                                    theme::accent()
                                } else {
                                    theme::muted()
                                },
                            );
                            response.on_hover_text(if done {
                                t("Добавлен", "Added")
                            } else {
                                t("Уже в плейлисте", "Already in the playlist")
                            });
                        }
                        Some(Some(Added::Pending)) => {
                            ui.add(egui::Spinner::new().size(18.0));
                        }
                        Some(Some(Added::Failed)) | None => {
                            let failed = state == Some(Some(Added::Failed));
                            let label = if failed {
                                t(
                                    "Не удалось добавить. Попробовать ещё раз",
                                    "Could not add. Try again",
                                )
                            } else {
                                t("Добавить в плейлист", "Add to playlist")
                            };
                            let response =
                                icons::track_button(ui, Icon::Plus, label, true, failed, false);
                            clicked = response.clicked();
                        }
                    }
                    ui.label(
                        RichText::new(track.duration_label())
                            .small()
                            .color(theme::muted()),
                    );
                });
            },
        );
        clicked
    }
}
