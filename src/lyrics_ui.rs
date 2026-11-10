//! Shared lyric rendering for the right drawer and the immersive player view.
use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    lyrics::{Error, Lyrics},
    model::{ArtistRef, Track},
    theme,
};
use eframe::egui::{self, Align, Color32, FontId, Rect, Sense};
use std::sync::Arc;

#[derive(Debug, PartialEq)]
pub enum Action {
    Close,
    Fullscreen,
    Retry,
    /// Load the lyrics again from their sources.
    Reload,
    Settings,
    Seek(f64),
    /// Open the playing track's album or single.
    Album,
    Artist(ArtistRef),
}
pub struct View<'a> {
    pub language: Language,
    pub track: Option<&'a Track>,
    pub lyrics: Option<&'a Lyrics>,
    pub error: Option<Error>,
    pub loading: bool,
    pub position: f64,
    pub playing: bool,
    pub fullscreen: bool,
    pub retry_allowed: bool,
    /// The cover, title and artists above the lines. Hidden under the
    /// queue, which already shows the playing track.
    pub show_track: bool,
}
struct Row {
    galley: Arc<egui::Galley>,
    top: f32,
    height: f32,
    gap: bool,
}
pub struct State {
    scroll_epoch: u64,
    scroll_ids: [Option<egui::Id>; 2],
    stamp: Option<(usize, u32, u32)>,
    rows: Vec<Row>,
    height: f32,
    follow: bool,
    last_active: Option<usize>,
    last_position: Option<f64>,
    gap_clock: GapClock,
}
impl Default for State {
    fn default() -> Self {
        Self {
            scroll_epoch: 0,
            scroll_ids: [None; 2],
            stamp: None,
            rows: Vec::new(),
            height: 0.0,
            follow: true,
            last_active: None,
            last_position: None,
            gap_clock: GapClock::default(),
        }
    }
}
impl State {
    pub fn reset(&mut self) {
        *self = Self {
            scroll_epoch: self.scroll_epoch.wrapping_add(1),
            scroll_ids: self.scroll_ids,
            ..Self::default()
        };
    }
}

#[derive(Default)]
struct GapClock {
    identity: Option<(usize, usize)>,
    elapsed: f64,
    last_frame: Option<f64>,
}
impl GapClock {
    fn advance(&mut self, identity: Option<(usize, usize)>, playing: bool, now: f64) -> f64 {
        if self.identity != identity {
            self.identity = identity;
            self.elapsed = 0.0;
            self.last_frame = None;
        }
        if identity.is_some()
            && playing
            && let Some(last) = self.last_frame
        {
            self.elapsed += (now - last).clamp(0.0, 0.1);
        }
        self.last_frame = Some(now);
        self.elapsed
    }
}

/// How much the current lyric line grows.
const ACTIVE_SCALE: f32 = 1.06;

fn gap_marker(text: &str) -> bool {
    matches!(text.trim(), "" | "..." | "…" | "•••")
}

fn dot_radius(size: f32, dot: usize, phase: Option<f64>) -> f32 {
    let pulse = phase.map_or(1.0, |seconds| {
        let angle = (seconds / 2.4 - dot as f64 / 3.0) * std::f64::consts::TAU;
        (0.85 + (angle.cos() + 1.0).powi(2) * 0.13) as f32
    });
    size * 0.13 * pulse
}

/// `backdrop` is the animated fullscreen background; without it, the
/// fullscreen view keeps its plain gradient.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    covers: &mut CoverCache,
    backdrop: Option<&mut crate::backdrop::Backdrop>,
    view: View<'_>,
) -> Option<Action> {
    let t = |ru, en| view.language.text(ru, en);
    let mut action = None;
    let rect = ui.available_rect_before_wrap();
    let inset = if view.fullscreen { 32.0 } else { 0.0 };
    let header = Rect::from_min_max(
        rect.min + egui::vec2(inset, if view.fullscreen { 20.0 } else { 0.0 }),
        egui::pos2(
            rect.right() - inset,
            rect.top() + if view.fullscreen { 54.0 } else { 32.0 },
        ),
    );
    let regular_cover = view.track.and_then(|track| track.cover_url.as_deref());
    let cover_url = if view.fullscreen {
        view.track
            .and_then(|track| track.album.as_ref()?.cover_url.as_deref())
            .or(regular_cover)
    } else {
        regular_cover
    };
    if view.fullscreen {
        let color = covers.header_color(ui.ctx(), cover_url, 512.0);
        background(ui.painter(), rect, color);
        if let Some(backdrop) = backdrop {
            let image = covers.ready_image(ui.ctx(), cover_url, 512.0);
            backdrop.paint(ui, rect, cover_url, image, view.playing);
        }
    }
    caption(
        ui,
        Rect::from_min_size(
            header.min,
            egui::vec2(
                (header.width() - if view.fullscreen { 72.0 } else { 104.0 }).max(0.0),
                32.0,
            ),
        ),
        t("Текст", "Lyrics"),
        FontId::new(if view.fullscreen { 18.0 } else { 20.0 }, theme::semibold()),
        ui.visuals().text_color(),
    );
    let end = Rect::from_center_size(
        header.right_center() - egui::vec2(14.0, 0.0),
        egui::vec2(28.0, 28.0),
    );
    // Reload sits left of the close (or exit fullscreen) button.
    let reload = end.translate(egui::vec2(-34.0, 0.0));
    let can_reload = view.track.is_some() && !view.loading && view.retry_allowed;
    if ui
        .add_enabled_ui(can_reload, |ui| {
            icons::panel_button(
                ui,
                reload,
                Icon::Refresh,
                t("Обновить текст", "Reload lyrics"),
            )
        })
        .inner
        .clicked()
    {
        action = Some(Action::Reload);
    }
    if view.fullscreen {
        if icons::panel_button(
            ui,
            end,
            Icon::Collapse,
            t("Выйти из полноэкранного режима", "Exit fullscreen"),
        )
        .clicked()
        {
            action = Some(Action::Fullscreen);
        }
    } else {
        if icons::panel_button(ui, end, Icon::Close, t("Закрыть текст", "Close lyrics")).clicked()
        {
            action = Some(Action::Close);
        }
        if icons::panel_button(
            ui,
            end.translate(egui::vec2(-68.0, 0.0)),
            Icon::Expand,
            t("На весь экран", "Fullscreen lyrics"),
        )
        .clicked()
        {
            action = Some(Action::Fullscreen);
        }
    }
    let mut content = Rect::from_min_max(egui::pos2(rect.left(), header.bottom() + 12.0), rect.max);
    // With no lines to show, the fullscreen cover moves to the middle and
    // says why below the artists.
    let missing = view.fullscreen
        && view.track.is_some()
        && match view.lyrics {
            Some(lyrics) => lyrics.instrumental && lyrics.lines.is_empty(),
            None => !view.loading && view.error == Some(Error::NotFound),
        };
    if view.fullscreen {
        content = content.shrink2(egui::vec2(32.0, 12.0));
        let width = content.width().min(1280.0);
        content = Rect::from_center_size(content.center(), egui::vec2(width, content.height()));
        let gap = (width * 0.05).clamp(24.0, 64.0);
        let art_size = (width * 0.42)
            .min(512.0)
            .min((content.height() - 80.0).max(64.0));
        let artwork = Rect::from_min_size(
            egui::pos2(
                if missing {
                    content.center().x - art_size * 0.5
                } else {
                    content.left()
                },
                content.center().y - (art_size + 64.0) * 0.5,
            ),
            egui::vec2(art_size, art_size),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(artwork), |ui| {
            covers.show_high_resolution(ui, cover_url, regular_cover, art_size, 14);
        });
        let title = view.track.map_or_else(
            || t("Выберите трек", "Choose a track").to_owned(),
            |track| track.display_title().into_owned(),
        );
        let title_rect = caption(
            ui,
            Rect::from_min_size(
                artwork.left_bottom() + egui::vec2(0.0, 16.0),
                egui::vec2(art_size, 28.0),
            ),
            &title,
            FontId::new(21.0, theme::semibold()),
            ui.visuals().text_color(),
        );
        let artist_rect = Rect::from_min_size(
            artwork.left_bottom() + egui::vec2(0.0, 47.0),
            egui::vec2(art_size, 22.0),
        );
        if let Some(track) = view.track {
            if album_link(ui, artwork, "artwork", view.language)
                || album_link(ui, title_rect, "title", view.language)
            {
                action = Some(Action::Album);
            }
            if let Some(artist) = artist_links(ui, artist_rect, track, FontId::proportional(14.0)) {
                action = Some(Action::Artist(artist));
            }
        }
        if missing {
            let reason = if view.lyrics.is_some() {
                t("Инструментальный трек", "Instrumental track")
            } else {
                Error::NotFound.message(view.language)
            };
            caption(
                ui,
                Rect::from_min_size(
                    artwork.left_bottom() + egui::vec2(0.0, 78.0),
                    egui::vec2(art_size, 22.0),
                ),
                reason,
                FontId::proportional(14.0),
                theme::MUTED,
            );
        }
        content.min.x = artwork.right() + gap;
    } else if let Some(track) = view.track.filter(|_| view.show_track) {
        let metadata = Rect::from_min_size(content.min, egui::vec2(content.width(), 52.0));
        let cover = Rect::from_min_size(metadata.min, egui::vec2(40.0, 40.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(cover), |ui| {
            covers.show(ui, cover_url, 40.0);
        });
        let title_rect = caption_job(
            ui,
            Rect::from_min_size(
                metadata.min + egui::vec2(50.0, 0.0),
                egui::vec2((metadata.width() - 50.0).max(0.0), 22.0),
            ),
            theme::track_title(
                track,
                ui.visuals().text_color(),
                FontId::new(14.0, theme::semibold()),
            ),
        );
        if album_link(ui, cover, "cover", view.language)
            || album_link(ui, title_rect, "title", view.language)
        {
            action = Some(Action::Album);
        }
        let artists_line = Rect::from_min_size(
            metadata.min + egui::vec2(50.0, 23.0),
            egui::vec2((metadata.width() - 50.0).max(0.0), 20.0),
        );
        let badge = theme::explicit_badge(ui.ctx(), Some(track), 12.0);
        let artists_line = badge.map_or(artists_line, |size| {
            theme::paint_explicit_badge(
                &ui.painter()
                    .with_clip_rect(ui.clip_rect().intersect(artists_line)),
                Rect::from_center_size(
                    egui::pos2(artists_line.left() + size * 0.5, artists_line.center().y),
                    egui::vec2(size, size),
                ),
            );
            artists_line.with_min_x(
                (artists_line.left() + size + theme::EXPLICIT_GAP).min(artists_line.right()),
            )
        });
        if let Some(artist) = artist_links(ui, artists_line, track, FontId::proportional(12.0)) {
            action = Some(Action::Artist(artist));
        }
        content.min.y += 62.0;
    }
    if missing {
        // Said under the centred cover.
    } else if let Some(lyrics) = view.lyrics {
        if lyrics.instrumental && lyrics.lines.is_empty() {
            centered(
                ui,
                content,
                t("Инструментальный трек", "Instrumental track"),
            );
        } else {
            let size = if view.fullscreen {
                (content.width() * 0.06).clamp(24.0, 34.0)
            } else {
                18.0
            };
            if let Some(at) = lines(
                ui,
                state,
                content,
                lyrics,
                view.position,
                size,
                view.fullscreen,
                view.playing,
            ) {
                action = Some(Action::Seek(at));
            }
            if !state.follow && lyrics.synced {
                let follow = Rect::from_center_size(
                    content.center_bottom() - egui::vec2(0.0, 20.0),
                    egui::vec2(content.width().min(220.0), 32.0),
                );
                let response = ui.put(
                    follow,
                    egui::Button::new(t("К текущей строке", "Follow current line"))
                        .fill(theme::PRIMARY)
                        .corner_radius(8),
                );
                if response.clicked() {
                    state.follow = true;
                    state.last_active = None;
                    response.surrender_focus();
                    action = None;
                }
            }
        }
    } else if view.loading {
        let center = content.center();
        ui.put(
            Rect::from_center_size(center - egui::vec2(0.0, 22.0), egui::vec2(24.0, 24.0)),
            egui::Spinner::new().color(theme::ACCENT),
        );
        ui.put(
            Rect::from_center_size(
                center + egui::vec2(0.0, 18.0),
                egui::vec2(content.width(), 40.0),
            ),
            theme::wrapped_body(
                ui,
                t("Загружаем текст…", "Loading lyrics…"),
                content.width(),
                Align::Center,
                theme::MUTED,
            ),
        );
    } else if let Some(error) = view.error {
        let center = content.center();
        ui.put(
            Rect::from_center_size(
                center - egui::vec2(0.0, 58.0),
                egui::vec2(content.width(), 88.0),
            ),
            theme::wrapped_body(
                ui,
                error.message(view.language),
                content.width(),
                Align::Center,
                theme::MUTED,
            ),
        );
        if !matches!(error, Error::Auth | Error::Storage)
            && ui
                .add_enabled_ui(view.retry_allowed, |ui| {
                    ui.put(
                        Rect::from_center_size(
                            center + egui::vec2(0.0, 18.0),
                            egui::vec2(content.width().min(220.0), 36.0),
                        ),
                        egui::Button::new(t("Повторить", "Retry"))
                            .fill(theme::PRIMARY)
                            .corner_radius(8),
                    )
                })
                .inner
                .clicked()
        {
            action = Some(Action::Retry);
        }
        if ui
            .put(
                Rect::from_center_size(
                    center + egui::vec2(0.0, 64.0),
                    egui::vec2(content.width().min(220.0), 34.0),
                ),
                egui::Button::new(t("Настройки текстов", "Lyrics settings")).frame(false),
            )
            .clicked()
        {
            action = Some(Action::Settings);
        }
    } else {
        centered(
            ui,
            content,
            t(
                "Включите трек, чтобы увидеть текст.",
                "Play a track to see its lyrics.",
            ),
        );
    }
    action
}

fn centered(ui: &mut egui::Ui, rect: Rect, text: &str) {
    ui.put(
        Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 72.0)),
        theme::wrapped_body(ui, text, rect.width(), Align::Center, theme::MUTED),
    );
}

/// Paint one truncated line and return the rect its text occupies.
fn caption(ui: &mut egui::Ui, rect: Rect, text: &str, font: FontId, color: Color32) -> Rect {
    caption_job(
        ui,
        rect,
        egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color),
    )
}
fn caption_job(ui: &mut egui::Ui, rect: Rect, mut job: egui::text::LayoutJob) -> Rect {
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(1.0));
    let galley = ui.painter().layout_job(job);
    let at = egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
    let text = Rect::from_min_size(at, galley.size()).intersect(rect);
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .galley(at, galley, Color32::WHITE);
    text
}

/// The cover and the title open the track's album or single, like the player.
fn album_link(ui: &mut egui::Ui, rect: Rect, salt: &str, language: Language) -> bool {
    if !rect.is_positive() {
        return false;
    }
    let label = language.text("Открыть альбом / сингл", "Open album / single");
    let response = ui.interact(rect, ui.id().with(("lyrics-album", salt)), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, label));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
        .clicked()
}

/// Each artist name is its own link, vertically centred in `rect`.
fn artist_links(ui: &mut egui::Ui, rect: Rect, track: &Track, font: FontId) -> Option<ArtistRef> {
    let height = ui.fonts_mut(|fonts| fonts.row_height(&font)).max(16.0);
    let line = Rect::from_center_size(rect.center(), egui::vec2(rect.width(), height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(line), |ui| {
        crate::detail_ui::artist_links_with_font(ui, &track.artists, &track.artist, font)
    })
    .inner
}

#[allow(clippy::too_many_arguments)]
fn lines(
    ui: &mut egui::Ui,
    state: &mut State,
    rect: Rect,
    data: &Lyrics,
    position: f64,
    size: f32,
    fullscreen: bool,
    playing: bool,
) -> Option<f64> {
    let mut action = None;
    if state
        .last_position
        .is_some_and(|old| (position - old).abs() > 2.5)
    {
        state.follow = true;
        state.last_active = None;
    }
    state.last_position = Some(position);
    let manually_scrolling = ui.input(|input| {
        input
            .pointer
            .hover_pos()
            .is_some_and(|point| rect.contains(point))
            && (input.events.iter().any(|event|matches!(event,egui::Event::MouseWheel{delta,..} if *delta!=egui::Vec2::ZERO))
                || (input.pointer.primary_down() && input.pointer.interact_pos().is_some_and(|point|point.x>rect.right()-16.0
                    || input.pointer.press_origin().is_some_and(|start|point.distance(start)>4.0))))
    });
    if manually_scrolling {
        state.follow = false;
    }
    let active = data.active_at(position);
    let gap = active.filter(|index| {
        data.lines
            .get(*index)
            .is_some_and(|line| gap_marker(&line.text))
    });
    let phase = state.gap_clock.advance(
        gap.map(|index| (data as *const Lyrics as usize, index)),
        playing,
        ui.input(|input| input.time),
    );
    let should_follow = state.follow && active != state.last_active;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(rect));
        if fullscreen {
            // Over the backdrop the bar's track shows only while the pointer
            // is on the bar; its handle and the room it takes stay the same.
            let solid = ui.spacing().scroll;
            ui.spacing_mut().scroll = egui::style::ScrollStyle {
                floating: true,
                floating_width: solid.bar_width,
                floating_allocated_width: solid.bar_width
                    + solid.bar_inner_margin
                    + solid.bar_outer_margin,
                dormant_background_opacity: 0.0,
                active_background_opacity: 0.0,
                interact_background_opacity: 1.0,
                dormant_handle_opacity: 1.0,
                active_handle_opacity: 1.0,
                interact_handle_opacity: 1.0,
                ..solid
            };
        }
        // A fresh identity also discards the previous track's animated target
        // and kinetic scrolling, which setting only the offset would retain.
        let shown = theme::scroll_viewport(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("lyric-lines", fullscreen, state.scroll_epoch))
                .auto_shrink([false, false]),
            |ui, viewport| {
                let width = ui.available_width().max(1.0);
                let stamp = (
                    data as *const Lyrics as usize,
                    width.to_bits(),
                    size.to_bits(),
                );
                let changed = state.stamp != Some(stamp);
                if changed {
                    state.rows.clear();
                    state.height = 12.0;
                    let font = FontId::proportional(size);
                    for line in &data.lines {
                        let gap = gap_marker(&line.text);
                        let text = if gap { "•••" } else { line.text.as_str() };
                        // Narrower by the active line's growth, so it keeps
                        // the same line breaks when it grows.
                        let galley = ui.painter().layout(
                            text.into(),
                            font.clone(),
                            Color32::WHITE,
                            ((width - 8.0) / ACTIVE_SCALE).max(1.0),
                        );
                        let height = galley.size().y + if fullscreen { 24.0 } else { 16.0 };
                        state.rows.push(Row {
                            galley,
                            top: state.height,
                            height,
                            gap,
                        });
                        state.height += height;
                    }
                    state.height += rect.height() * 0.4;
                    state.stamp = Some(stamp);
                }
                let (whole, _) = ui.allocate_exact_size(
                    egui::vec2(width, state.height.max(rect.height())),
                    Sense::hover(),
                );
                if (should_follow || (changed && state.follow))
                    && let Some(index) = active
                    && let Some(row) = state.rows.get(index)
                {
                    let target = Rect::from_min_size(
                        whole.min + egui::vec2(0.0, row.top),
                        egui::vec2(width, row.height),
                    );
                    ui.scroll_to_rect(target, Some(Align::Center));
                    state.last_active = active;
                }
                let start = state
                    .rows
                    .partition_point(|row| row.top + row.height < viewport.top());
                let end = state
                    .rows
                    .partition_point(|row| row.top < viewport.bottom());
                for index in start..end {
                    let row = &state.rows[index];
                    let line = Rect::from_min_size(
                        whole.min + egui::vec2(0.0, row.top),
                        egui::vec2(width, row.height),
                    );
                    let response = ui.interact(
                        line,
                        ui.id().with((stamp.0, index)),
                        if data.synced {
                            Sense::click()
                        } else {
                            Sense::hover()
                        },
                    );
                    // The current line grows a little and turns accent blue,
                    // easing in and out.
                    let current = ui.ctx().animate_bool_with_time(
                        ui.id().with(("lyric-active", stamp.0, index)),
                        data.synced && active == Some(index),
                        0.25,
                    );
                    let resting = if !data.synced || response.hovered() {
                        Color32::from_rgb(238, 241, 247)
                    } else {
                        Color32::from_gray(124)
                    };
                    let color = resting.lerp_to_gamma(theme::ACCENT, current);
                    let scale = 1.0 + (ACTIVE_SCALE - 1.0) * current;
                    if row.gap {
                        let animated = data.synced && active == Some(index);
                        let center = line.min
                            + egui::vec2(
                                4.0 + size * scale * 0.18,
                                row.galley.size().y * 0.5 + 2.0,
                            );
                        for dot in 0..3 {
                            ui.painter().circle_filled(
                                center + egui::vec2(dot as f32 * size * scale * 0.6, 0.0),
                                dot_radius(size * scale, dot, animated.then_some(phase)),
                                color,
                            );
                        }
                        if animated && playing {
                            ui.ctx().request_repaint();
                        }
                    } else if scale > 1.001 {
                        // The laid-out line is scaled as a shape, from its
                        // left edge and middle: laying it out again at each
                        // size would move its glyphs and could rewrap it.
                        let origin = line.min + egui::vec2(4.0, 2.0);
                        let anchor = origin + egui::vec2(0.0, row.galley.size().y * 0.5);
                        let mut shape =
                            egui::epaint::TextShape::new(origin, row.galley.clone(), color)
                                .with_override_text_color(color);
                        shape.transform(egui::emath::TSTransform::new(
                            anchor.to_vec2() * (1.0 - scale),
                            scale,
                        ));
                        ui.painter().add(shape);
                    } else {
                        ui.painter().galley_with_override_text_color(
                            line.min + egui::vec2(4.0, 2.0),
                            row.galley.clone(),
                            color,
                        );
                    }
                    if data.synced {
                        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() {
                            action = data.lines[index].at;
                            state.follow = true;
                            state.last_active = None;
                            response.surrender_focus();
                        }
                    }
                }
            },
        );
        if let Some(previous) = state.scroll_ids[usize::from(fullscreen)].replace(shown.id)
            && previous != shown.id
        {
            ui.ctx()
                .data_mut(|data| data.remove::<egui::scroll_area::State>(previous));
        }
    });
    action
}

fn background(painter: &egui::Painter, rect: Rect, color: Option<Color32>) {
    let tint = color.unwrap_or(Color32::from_rgb(61, 91, 132));
    let mix = |base: Color32, ratio: f32| {
        Color32::from_rgb(
            (f32::from(base.r()) * (1.0 - ratio) + f32::from(tint.r()) * ratio) as u8,
            (f32::from(base.g()) * (1.0 - ratio) + f32::from(tint.g()) * ratio) as u8,
            (f32::from(base.b()) * (1.0 - ratio) + f32::from(tint.b()) * ratio) as u8,
        )
    };
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), mix(Color32::from_rgb(43, 49, 61), 0.22)),
        (rect.right_top(), mix(theme::BACKGROUND, 0.24)),
        (rect.right_bottom(), mix(theme::BACKGROUND, 0.07)),
        (rect.left_bottom(), mix(theme::BACKGROUND, 0.18)),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(
        ctx: &egui::Context,
        state: &mut State,
        data: &Lyrics,
        time: f64,
        playing: bool,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 260.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                lines(
                    ui,
                    state,
                    ui.available_rect_before_wrap(),
                    data,
                    1.0,
                    18.0,
                    false,
                    playing,
                );
            },
        );
        output.textures_delta.clear();
        output
    }

    fn texts(output: &egui::FullOutput) -> Vec<(String, Rect, Color32, f32)> {
        fn collect(shape: &egui::Shape, found: &mut Vec<(String, Rect, Color32, f32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let section = text.galley.job.sections.first();
                    found.push((
                        text.galley.text().to_owned(),
                        text.visual_bounding_rect(),
                        text.override_text_color
                            .or_else(|| section.map(|s| s.format.color))
                            .unwrap_or(text.fallback_color),
                        section.map_or(0.0, |s| s.format.font_id.size),
                    ));
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        output
            .shapes
            .iter()
            .for_each(|shape| collect(&shape.shape, &mut found));
        found
    }

    #[test]
    fn the_current_line_grows_a_little_and_turns_accent_blue() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let data = crate::lyrics::test_data(12, true);
        // Line 3 is current at 35 s; the change eases in over a quarter second.
        let frame = |state: &mut State, time: f64| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 400.0),
                    )),
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    lines(
                        ui,
                        state,
                        ui.available_rect_before_wrap(),
                        &data,
                        35.0,
                        18.0,
                        false,
                        true,
                    );
                },
            );
            output.textures_delta.clear();
            output
        };
        let mut output = frame(&mut state, 1.0);
        // While it grows, the line stays put on the left and keeps its
        // wrapping; only its size changes, a little more each frame.
        let mut sizes = Vec::new();
        for tick in 1..10 {
            output = frame(&mut state, 1.0 + f64::from(tick) * 0.03);
            if let Some((_, rect, ..)) = texts(&output)
                .into_iter()
                .find(|(text, ..)| text == "Synthetic lyric line 3")
            {
                sizes.push(rect);
            }
        }
        let lefts: Vec<_> = sizes.iter().map(|rect| rect.left()).collect();
        assert!(
            lefts
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 0.01),
            "{lefts:?}"
        );
        assert!(
            sizes
                .windows(2)
                .all(|pair| pair[1].height() >= pair[0].height() - 0.01),
            "{sizes:?}"
        );
        for _ in 0..10 {
            output = frame(&mut state, 3.0);
        }
        let shown = texts(&output);
        let find = |line: &str| {
            shown
                .iter()
                .find(|(text, ..)| text == line)
                .cloned()
                .unwrap_or_else(|| panic!("missing {line}"))
        };
        let (_, current_rect, current_color, _) = find("Synthetic lyric line 3");
        let (_, other_rect, other_color, _) = find("Synthetic lyric line 5");
        assert_eq!(current_color, theme::ACCENT);
        assert_ne!(other_color, theme::ACCENT);
        let grown = current_rect.width() / other_rect.width();
        assert!((grown - ACTIVE_SCALE).abs() < 0.01, "{grown}");
        assert!(
            (current_rect.left() - other_rect.left()).abs() < 1.0,
            "grows from the left"
        );
    }

    #[test]
    fn fullscreen_scrollbar_track_shows_only_under_the_pointer() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut state = State::default();
        let data = crate::lyrics::test_data(80, true);
        let size = egui::vec2(600.0, 400.0);
        let frame = |state: &mut State, time: f64, pointer: Option<egui::Pos2>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(time),
                    events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
                    ..Default::default()
                },
                |ui| {
                    lines(ui, state, ui.max_rect(), &data, 35.0, 18.0, true, false);
                },
            );
            output.textures_delta.clear();
            output
        };
        // Visible fills along the bar's column, as tall boxes.
        let bars = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.rect.left() > size.x - 20.0
                            && rect.fill.a() > 0
                            && rect.rect.height() > 20.0 =>
                    {
                        Some((rect.rect, rect.fill))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mut idle = egui::FullOutput::default();
        for tick in 0..20 {
            idle = frame(
                &mut state,
                f64::from(tick) * 0.1,
                Some(egui::pos2(200.0, 200.0)),
            );
        }
        let mut hovered = egui::FullOutput::default();
        for tick in 20..40 {
            hovered = frame(
                &mut state,
                f64::from(tick) * 0.1,
                Some(egui::pos2(size.x - 7.0, 200.0)),
            );
        }
        let (idle, hovered) = (bars(&idle), bars(&hovered));
        // The handle shows either way; the full-height track only on hover.
        let track =
            |found: &[(Rect, Color32)]| found.iter().any(|(rect, _)| rect.height() > size.y * 0.9);
        assert!(!idle.is_empty(), "the handle stays visible");
        assert!(!track(&idle), "no track away from the bar: {idle:?}");
        assert!(track(&hovered), "the track shows on the bar: {hovered:?}");
    }

    #[test]
    fn fullscreen_cover_moves_to_the_middle_when_there_are_no_lines() {
        let track = Track {
            id: 7,
            owner_id: 1,
            artist: "Synthetic artist".into(),
            title: "Centred song".into(),
            subtitle: None,
            duration: 180,
            url: None,
            cover_url: None,
            artists: Vec::new(),
            album: None,
            library_liked: None,
            release_key: None,
            library_source_key: None,
            unavailable: None,
            added: None,
            explicit: false,
        };
        let lines = crate::lyrics::test_data(12, true);
        let mut instrumental = crate::lyrics::test_data(0, false);
        instrumental.instrumental = true;
        let width = 1000.0;
        let draw = |lyrics: Option<&Lyrics>, error: Option<Error>| {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut covers = CoverCache::new();
            let mut state = State::default();
            let mut output = egui::FullOutput::default();
            for tick in 0..3 {
                output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 600.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        show(
                            ui,
                            &mut state,
                            &mut covers,
                            None,
                            View {
                                language: Language::English,
                                track: Some(&track),
                                lyrics,
                                error,
                                loading: false,
                                position: 0.0,
                                playing: false,
                                fullscreen: true,
                                retry_allowed: true,
                                show_track: true,
                            },
                        );
                    },
                );
                output.textures_delta.clear();
            }
            texts(&output)
        };
        let title_left = |shown: &[(String, Rect, Color32, f32)]| {
            shown
                .iter()
                .find(|(text, ..)| text == "Centred song")
                .map(|(_, rect, ..)| rect.left())
                .expect("the title")
        };
        let with_lines = draw(Some(&lines), None);
        let beside = title_left(&with_lines);
        assert!(beside < width * 0.25, "beside the lines: {beside}");
        for (shown, reason) in [
            (
                draw(None, Some(Error::NotFound)),
                Error::NotFound.message(Language::English),
            ),
            (draw(Some(&instrumental), None), "Instrumental track"),
        ] {
            let left = title_left(&shown);
            assert!(left > width * 0.3, "the cover moves to the middle: {left}");
            let (_, said, ..) = shown
                .iter()
                .find(|(text, ..)| text == reason)
                .unwrap_or_else(|| panic!("missing {reason}"));
            assert!((said.left() - left).abs() < 1.0, "said under the cover");
            assert!(!shown.iter().any(|(text, ..)| text == "Retry"));
        }
    }

    #[test]
    fn reset_clears_previous_timed_scroll_before_showing_plain_lyrics() {
        for fullscreen in [false, true] {
            let ctx = egui::Context::default();
            let mut state = State::default();
            let synced = crate::lyrics::test_data(40, true);
            let plain = crate::lyrics::test_data(40, false);
            let frame = |state: &mut State, data: &Lyrics, position: f64, time: f64| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(300.0, 260.0),
                        )),
                        time: Some(time),
                        ..Default::default()
                    },
                    |ui| {
                        lines(
                            ui,
                            state,
                            ui.available_rect_before_wrap(),
                            data,
                            position,
                            18.0,
                            fullscreen,
                            false,
                        );
                    },
                );
                output.textures_delta.clear();
            };
            for tick in 0..12 {
                frame(&mut state, &synced, 150.0, f64::from(tick) * 0.1);
            }
            let previous = state.scroll_ids[usize::from(fullscreen)].unwrap();
            assert!(
                egui::scroll_area::State::load(&ctx, previous)
                    .unwrap()
                    .offset
                    .y
                    > 200.0
            );

            state.reset();
            frame(&mut state, &plain, 0.0, 2.0);
            let current = state.scroll_ids[usize::from(fullscreen)].unwrap();
            assert_ne!(current, previous);
            assert_eq!(
                egui::scroll_area::State::load(&ctx, current)
                    .unwrap()
                    .offset
                    .y,
                0.0
            );
            assert!(egui::scroll_area::State::load(&ctx, previous).is_none());
        }
    }

    #[test]
    fn cover_title_and_artists_open_the_album_and_each_artist_in_both_layouts() {
        let track = Track {
            explicit: false,
            added: None,
            id: 5,
            owner_id: 4,
            artist: "First, Second".into(),
            title: "Linked song".into(),
            subtitle: None,
            duration: 180,
            url: None,
            cover_url: None,
            artists: vec![
                ArtistRef {
                    id: "first".into(),
                    name: "First".into(),
                },
                ArtistRef {
                    id: "second".into(),
                    name: "Second".into(),
                },
            ],
            album: None,
            library_liked: None,
            release_key: None,
            library_source_key: None,
            unavailable: None,
        };
        for fullscreen in [false, true] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut covers = CoverCache::new();
            let mut state = State::default();
            let mut time = 0.0;
            let mut frame = |events: Vec<egui::Event>| {
                time += 0.1;
                let mut action = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(if fullscreen { 1000.0 } else { 320.0 }, 600.0),
                        )),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        action = show(
                            ui,
                            &mut state,
                            &mut covers,
                            None,
                            View {
                                language: Language::English,
                                track: Some(&track),
                                lyrics: None,
                                error: None,
                                loading: false,
                                position: 0.0,
                                playing: false,
                                fullscreen,
                                retry_allowed: true,
                                show_track: true,
                            },
                        );
                    },
                );
                output.textures_delta.clear();
                (output, action)
            };
            let click = |point: egui::Pos2| {
                [true, false].map(|pressed| {
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                })
            };
            let (output, _) = frame(Vec::new());
            let text = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => {
                            Some(text.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing {label}"))
            };
            let title = text("Linked song");
            let second = text("Second");
            let cover = if fullscreen {
                title.left_top() - egui::vec2(-20.0, 40.0)
            } else {
                title.left_center() - egui::vec2(30.0, 0.0)
            };
            for (point, expected) in [
                (title.center(), Action::Album),
                (cover, Action::Album),
                (second.center(), Action::Artist(track.artists[1].clone())),
            ] {
                let [press, release] = click(point);
                frame(press);
                let (_, action) = frame(release);
                assert_eq!(
                    action,
                    Some(expected),
                    "fullscreen={fullscreen} at {point:?}"
                );
            }
        }
    }

    fn radii(output: &egui::FullOutput) -> Vec<f32> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Circle(circle) => Some(circle.radius),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn synced_gap_breathes_in_sequence_and_freezes_on_pause_but_plain_lyrics_are_static() {
        for synced in [true, false] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut state = State::default();
            let mut data = crate::lyrics::test_data(3, synced);
            data.lines[0].text.clear();
            let start = draw(&ctx, &mut state, &data, 0.0, true);
            let mut later = start.clone();
            for frame in 1..=8 {
                later = draw(&ctx, &mut state, &data, f64::from(frame) * 0.1, true);
            }
            assert_eq!(radii(&start).len(), 3);
            if synced {
                assert_ne!(radii(&start), radii(&later));
                assert_ne!(radii(&later)[0], radii(&later)[1]);
                let paused = draw(&ctx, &mut state, &data, 5.0, false);
                let still = draw(&ctx, &mut state, &data, 10.0, false);
                assert_eq!(radii(&paused), radii(&still));
                assert_eq!(radii(&later), radii(&paused));
            } else {
                assert_eq!(radii(&start), radii(&later));
                assert!(radii(&later).windows(2).all(|pair| pair[0] == pair[1]));
                assert_eq!(state.gap_clock.elapsed, 0.0);
            }
        }
        for text in ["", "...", "…", "•••", "  ...  "] {
            assert!(gap_marker(text));
        }
        assert!(!gap_marker("A line..."));
    }

    #[test]
    fn only_inactive_synced_lines_use_the_darker_color() {
        for synced in [true, false] {
            let ctx = egui::Context::default();
            let mut state = State::default();
            let data = crate::lyrics::test_data(3, synced);
            let output = draw(&ctx, &mut state, &data, 0.0, false);
            let colors: Vec<_> = texts(&output)
                .into_iter()
                .filter(|(text, ..)| text.starts_with("Synthetic lyric line"))
                .map(|(_, _, color, _)| color)
                .collect();
            assert_eq!(colors.len(), 3);
            // The current synced line is accent blue; plain lyrics have none.
            assert_eq!(
                colors[0],
                if synced {
                    theme::ACCENT
                } else {
                    Color32::from_rgb(238, 241, 247)
                }
            );
            assert_eq!(
                colors[1],
                if synced {
                    Color32::from_gray(124)
                } else {
                    Color32::from_rgb(238, 241, 247)
                }
            );
        }
    }
}
