//! Shared lyric rendering for the right drawer and the immersive player view.
use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    lyrics::{Error, Lyrics},
    model::Track,
    theme,
};
use eframe::egui::{self, Align, Color32, FontId, Rect, Sense};
use std::sync::Arc;

pub enum Action {
    Close,
    Fullscreen,
    Retry,
    Settings,
    Seek(f64),
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
}
struct Row {
    galley: Arc<egui::Galley>,
    top: f32,
    height: f32,
    gap: bool,
}
pub struct State {
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
        *self = Self::default();
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

pub fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    covers: &mut CoverCache,
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
    }
    caption(
        ui,
        Rect::from_min_size(
            header.min,
            egui::vec2(
                (header.width() - if view.fullscreen { 38.0 } else { 70.0 }).max(0.0),
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
            end.translate(egui::vec2(-34.0, 0.0)),
            Icon::Expand,
            t("На весь экран", "Fullscreen lyrics"),
        )
        .clicked()
        {
            action = Some(Action::Fullscreen);
        }
    }
    let mut content = Rect::from_min_max(egui::pos2(rect.left(), header.bottom() + 12.0), rect.max);
    if view.fullscreen {
        content = content.shrink2(egui::vec2(32.0, 12.0));
        let width = content.width().min(1280.0);
        content = Rect::from_center_size(content.center(), egui::vec2(width, content.height()));
        let gap = (width * 0.05).clamp(24.0, 64.0);
        let art_size = (width * 0.42)
            .min(512.0)
            .min((content.height() - 80.0).max(64.0));
        let artwork = Rect::from_min_size(
            egui::pos2(content.left(), content.center().y - (art_size + 64.0) * 0.5),
            egui::vec2(art_size, art_size),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(artwork), |ui| {
            covers.show_high_resolution(ui, cover_url, regular_cover, art_size, 14);
        });
        let title = view.track.map_or_else(
            || t("Выберите трек", "Choose a track").to_owned(),
            |track| track.display_title().into_owned(),
        );
        let artist = view.track.map_or("", |track| track.artist.as_str());
        caption(
            ui,
            Rect::from_min_size(
                artwork.left_bottom() + egui::vec2(0.0, 16.0),
                egui::vec2(art_size, 28.0),
            ),
            &title,
            FontId::new(21.0, theme::semibold()),
            ui.visuals().text_color(),
        );
        caption(
            ui,
            Rect::from_min_size(
                artwork.left_bottom() + egui::vec2(0.0, 47.0),
                egui::vec2(art_size, 22.0),
            ),
            artist,
            FontId::proportional(14.0),
            theme::MUTED,
        );
        content.min.x = artwork.right() + gap;
    } else if let Some(track) = view.track {
        let metadata = Rect::from_min_size(content.min, egui::vec2(content.width(), 52.0));
        let cover = Rect::from_min_size(metadata.min, egui::vec2(40.0, 40.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(cover), |ui| {
            covers.show(ui, cover_url, 40.0);
        });
        caption_job(
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
        caption(
            ui,
            Rect::from_min_size(
                metadata.min + egui::vec2(50.0, 23.0),
                egui::vec2((metadata.width() - 50.0).max(0.0), 20.0),
            ),
            &track.artist,
            FontId::proportional(12.0),
            theme::MUTED,
        );
        content.min.y += 62.0;
    }
    if let Some(lyrics) = view.lyrics {
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
        if !matches!(error, Error::MissingKey | Error::Auth | Error::Storage)
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

fn caption(ui: &mut egui::Ui, rect: Rect, text: &str, font: FontId, color: Color32) {
    caption_job(
        ui,
        rect,
        egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color),
    );
}
fn caption_job(ui: &mut egui::Ui, rect: Rect, mut job: egui::text::LayoutJob) {
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(1.0));
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            Color32::WHITE,
        );
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
        theme::scroll_viewport(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("lyric-lines", fullscreen))
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
                        let galley = ui.painter().layout(
                            text.into(),
                            font.clone(),
                            Color32::WHITE,
                            (width - 8.0).max(1.0),
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
                    let color = if !data.synced || active == Some(index) || response.hovered() {
                        Color32::from_rgb(238, 241, 247)
                    } else {
                        Color32::from_gray(124)
                    };
                    if row.gap {
                        let animated = data.synced && active == Some(index);
                        let center = line.min
                            + egui::vec2(4.0 + size * 0.18, row.galley.size().y * 0.5 + 2.0);
                        for dot in 0..3 {
                            ui.painter().circle_filled(
                                center + egui::vec2(dot as f32 * size * 0.6, 0.0),
                                dot_radius(size, dot, animated.then_some(phase)),
                                color,
                            );
                        }
                        if animated && playing {
                            ui.ctx().request_repaint();
                        }
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
            let colors: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.galley.text().starts_with("Synthetic lyric line") =>
                    {
                        Some(text.override_text_color)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(colors.len(), 3);
            assert_eq!(colors[0], Some(Color32::from_rgb(238, 241, 247)));
            assert_eq!(
                colors[1],
                Some(if synced {
                    Color32::from_gray(124)
                } else {
                    Color32::from_rgb(238, 241, 247)
                })
            );
        }
    }
}
