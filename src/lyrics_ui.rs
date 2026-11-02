//! Shared lyric rendering for the right drawer and the immersive player view.
use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    lyrics::{Error, Lyrics},
    model::Track,
    theme,
};
use eframe::egui::{self, Align, Color32, FontId, Rect, RichText, Sense};
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
    pub fullscreen: bool,
    pub retry_allowed: bool,
}
struct Row {
    galley: Arc<egui::Galley>,
    top: f32,
    height: f32,
}
pub struct State {
    stamp: Option<(usize, u32, u32)>,
    rows: Vec<Row>,
    height: f32,
    follow: bool,
    last_active: Option<usize>,
    last_position: Option<f64>,
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
        }
    }
}
impl State {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn icon_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, label: &str) -> egui::Response {
    let response = ui
        .put(rect, egui::Button::new("").frame(false))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label);
    icons::paint(
        ui.painter(),
        rect.shrink(6.0),
        icon,
        if response.hovered() {
            Color32::WHITE
        } else {
            theme::MUTED
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
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
    let cover_url = view.track.and_then(|track| track.cover_url.as_deref());
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
        if icon_button(
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
        if icon_button(ui, end, Icon::Close, t("Закрыть текст", "Close lyrics")).clicked()
        {
            action = Some(Action::Close);
        }
        if icon_button(
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
            covers.show_rounded(ui, cover_url, art_size, 14);
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
        let copyright_height = if lyrics.copyright.is_empty() {
            24.0
        } else {
            55.0
        };
        let footer = Rect::from_min_max(
            egui::pos2(
                content.left(),
                (content.bottom() - copyright_height).max(content.top()),
            ),
            content.right_bottom(),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(footer));
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to(
                    RichText::new("Musixmatch").size(11.0).color(theme::MUTED),
                    lyrics
                        .source_url
                        .as_deref()
                        .unwrap_or("https://www.musixmatch.com/"),
                );
                if !lyrics.synced && !lyrics.instrumental {
                    ui.label(
                        RichText::new(t("Без синхронизации", "Not synchronized"))
                            .size(11.0)
                            .color(theme::MUTED),
                    );
                }
            });
            if !lyrics.copyright.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(&lyrics.copyright)
                            .size(10.0)
                            .color(theme::MUTED),
                    )
                    .wrap()
                    .selectable(false),
                )
                .on_hover_text(&lyrics.copyright);
            }
        });
        content.max.y = footer.top() - 8.0;
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
            egui::Label::new(
                RichText::new(t("Загружаем текст…", "Loading lyrics…")).color(theme::MUTED),
            )
            .wrap()
            .halign(Align::Center),
        );
    } else if let Some(error) = view.error {
        let center = content.center();
        ui.put(
            Rect::from_center_size(
                center - egui::vec2(0.0, 58.0),
                egui::vec2(content.width(), 88.0),
            ),
            egui::Label::new(RichText::new(error.message(view.language)).color(theme::MUTED))
                .wrap()
                .halign(Align::Center)
                .selectable(false),
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
                egui::Button::new(t("Настройки Musixmatch", "Musixmatch settings")).frame(false),
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
        egui::Label::new(RichText::new(text).color(theme::MUTED))
            .wrap()
            .halign(Align::Center)
            .selectable(false),
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

fn lines(
    ui: &mut egui::Ui,
    state: &mut State,
    rect: Rect,
    data: &Lyrics,
    position: f64,
    size: f32,
    fullscreen: bool,
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
                        let text = if line.text.trim().is_empty() {
                            "•••"
                        } else {
                            line.text.as_str()
                        };
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
                        Color32::from_gray(154)
                    };
                    ui.painter().galley_with_override_text_color(
                        line.min + egui::vec2(4.0, 2.0),
                        row.galley.clone(),
                        color,
                    );
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
