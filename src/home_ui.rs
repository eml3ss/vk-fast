//! The home page renders data supplied by the application and returns intents.
//! It never invents VK collections or starts authentication or playback itself.
use eframe::egui::{self, Align, Color32, Layout, Response, RichText, Sense, Stroke};
use std::collections::{HashMap, HashSet};

use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    model::Track,
    theme,
};

#[derive(Clone)]
pub struct Collection {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub cover_url: Option<String>,
    pub tracks: Vec<Track>,
}

pub struct HomeView<'a> {
    pub language: Language,
    pub library: &'a [Track],
    pub library_total: Option<usize>,
    pub recent: &'a [Track],
    pub playing: Option<&'a Track>,
    pub connected: bool,
    pub loading: bool,
    pub mix_loading: bool,
    pub mix_playing: bool,
    pub collections: &'a [Collection],
    pub error: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    OpenLibrary,
    OpenRecent,
    PlayMix,
    PlayRecent,
    PlayTrack((i64, i64)),
    OpenCollection(String),
    PlayCollection(String),
    Refresh,
    SignIn,
}

pub fn show(ui: &mut egui::Ui, covers: &mut CoverCache, view: HomeView<'_>) -> Option<Action> {
    let mut action = None;
    let scroll = egui::ScrollArea::vertical()
        .id_salt("home-content")
        .auto_shrink([false, false]);
    #[cfg(feature = "screenshot")]
    let scroll = if ui.ctx().data(|data| {
        data.get_temp::<bool>(egui::Id::new("home-screenshot"))
            .unwrap_or(false)
    }) {
        scroll
            .vertical_scroll_offset(0.0)
            .scroll_source(egui::scroll_area::ScrollSource::NONE)
    } else {
        scroll
    };
    theme::scroll_area(ui, scroll, |ui| {
        action = hero(ui, &view).or(action.take());
        ui.add_space(22.0);
        action = shortcuts(ui, covers, &view).or(action.take());
        if let Some(error) = view.error {
            ui.add_space(18.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(54, 35, 39))
                .corner_radius(12)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(error).color(Color32::from_rgb(255, 174, 169)),
                        )
                        .wrap()
                        .selectable(false),
                    );
                });
        }

        if !view.recent.is_empty() {
            ui.add_space(26.0);
            if section_heading(
                ui,
                Icon::Clock,
                view.language.text("Недавно слушали", "Recently played"),
                Some((Icon::Play, view.language.text("Слушать", "Play"))),
                true,
            ) {
                action = Some(Action::PlayRecent);
            }
            ui.add_space(10.0);
            let columns = ((ui.available_width() + 16.0) / 216.0)
                .floor()
                .clamp(1.0, 5.0) as usize;
            let width = cell_width(ui.available_width(), columns);
            let recent = recent_preview(view.recent, columns * 2);
            for (row, tracks) in recent.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (column, track) in tracks.iter().enumerate() {
                        ui.push_id(("home-recent", row, column, track.key()), |ui| {
                            if recent_card(ui, covers, track, width).clicked() {
                                action = Some(Action::PlayTrack(track.key()));
                            }
                        });
                    }
                });
                ui.add_space(4.0);
            }
        }

        ui.add_space(26.0);
        if section_heading(
            ui,
            Icon::Sparkles,
            view.language.text("Подборки для вас", "Made for you"),
            Some((Icon::Refresh, view.language.text("Обновить", "Refresh"))),
            view.connected && !view.loading,
        ) {
            action = Some(Action::Refresh);
        }
        ui.add_space(10.0);
        if view.collections.is_empty() {
            empty_collections(ui, &view);
        } else {
            let columns = columns(ui.available_width());
            let width = cell_width(ui.available_width(), columns);
            for collections in view.collections.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for collection in collections {
                        ui.push_id(("home-collection", &collection.id), |ui| {
                            let (body, play_clicked) =
                                collection_card(ui, covers, collection, width, view.language);
                            if play_clicked {
                                action = Some(Action::PlayCollection(collection.id.clone()));
                            } else if body.clicked() {
                                action = Some(Action::OpenCollection(collection.id.clone()));
                            }
                        });
                    }
                });
                ui.add_space(8.0);
            }
        }
        ui.add_space(8.0);
    });
    action
}

fn columns(width: f32) -> usize {
    ((width + 16.0) / 216.0).floor().clamp(1.0, 4.0) as usize
}

fn cell_width(width: f32, columns: usize) -> f32 {
    ((width - 16.0 * columns.saturating_sub(1) as f32) / columns as f32).max(1.0)
}

fn gradient(painter: &egui::Painter, rect: egui::Rect) {
    // Each strip paints the same rounded silhouette. Its corners keep egui's
    // anti-aliasing, and no gradient image or external asset is necessary.
    for step in 0..32 {
        let fraction = step as f32 / 31.0;
        let channel = |left: u8, right: u8| {
            (f32::from(left) + (f32::from(right) - f32::from(left)) * fraction).round() as u8
        };
        let color = Color32::from_rgb(channel(26, 22), channel(51, 29), channel(84, 42));
        let strip = egui::Rect::from_min_max(
            egui::pos2(rect.left() + rect.width() * step as f32 / 32.0, rect.top()),
            egui::pos2(
                rect.left() + rect.width() * (step + 1) as f32 / 32.0,
                rect.bottom(),
            ),
        );
        painter
            .with_clip_rect(painter.clip_rect().intersect(strip))
            .rect_filled(rect, 16.0, color);
    }
    painter.rect_stroke(
        rect,
        16.0,
        Stroke::new(1.0, Color32::from_rgb(44, 61, 83)),
        egui::StrokeKind::Inside,
    );
}

fn hero(ui: &mut egui::Ui, view: &HomeView<'_>) -> Option<Action> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 208.0), Sense::hover());
    gradient(ui.painter(), rect);
    let show_art = rect.width() >= 620.0;
    let inner = rect.shrink(24.0);
    let text_right = if show_art {
        inner.right() - 172.0
    } else {
        inner.right()
    };
    let mut text = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-hero-text")
            .max_rect(egui::Rect::from_min_max(
                inner.min,
                egui::pos2(text_right, inner.bottom() - 48.0),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    text.set_clip_rect(rect.intersect(ui.clip_rect()));
    text.add(
        egui::Label::new(
            RichText::new("VK MIX")
                .small()
                .color(Color32::from_rgb(151, 181, 220)),
        )
        .selectable(false),
    );
    text.add_space(6.0);
    text.add(
        egui::Label::new(
            RichText::new(view.language.text("Слушать VK Микс", "Listen to VK Mix"))
                .size(30.0)
                .family(theme::semibold()),
        )
        .wrap()
        .selectable(false),
    );
    text.add_space(6.0);
    let subtitle = view.language.text(
        "Музыкальные рекомендации для вас",
        "Personal music recommendations for you",
    );
    text.add(
        egui::Label::new(RichText::new(subtitle).color(Color32::from_rgb(174, 187, 205)))
            .wrap()
            .selectable(false),
    );

    if show_art {
        let art = egui::Rect::from_center_size(
            egui::pos2(inner.right() - 68.0, rect.center().y),
            egui::vec2(136.0, 136.0),
        );
        mix_symbol(ui.painter(), art);
    }

    let mut buttons = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-hero-action")
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(inner.left(), inner.bottom() - 36.0),
                egui::vec2(text_right - inner.left(), 36.0),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    buttons.style_mut().visuals.widgets.inactive.bg_fill = theme::PRIMARY;
    buttons.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    buttons.style_mut().visuals.widgets.hovered.bg_fill = theme::ACCENT;
    buttons.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    let (icon, label, action) = if !view.connected {
        (
            Icon::ArrowRight,
            view.language.text("Войти в VK", "Sign in with VK"),
            Action::SignIn,
        )
    } else if view.mix_loading {
        (
            Icon::Clock,
            view.language.text("Подключаем…", "Connecting…"),
            Action::PlayMix,
        )
    } else if view.mix_playing {
        (
            Icon::Pause,
            view.language.text("Пауза", "Pause"),
            Action::PlayMix,
        )
    } else {
        (
            Icon::Play,
            view.language.text("Слушать", "Listen"),
            Action::PlayMix,
        )
    };
    icons::button(
        &mut buttons,
        icon,
        label,
        !view.connected || !view.mix_loading,
    )
    .clicked()
    .then_some(action)
}

fn mix_symbol(painter: &egui::Painter, rect: egui::Rect) {
    let center = rect.center();
    painter.circle_filled(center, rect.width() * 0.5, Color32::from_rgb(24, 43, 70));
    painter.circle_stroke(
        center,
        rect.width() * 0.5 - 0.5,
        Stroke::new(1.0, Color32::from_rgb(48, 79, 122)),
    );
    for (ribbon, color) in [
        Color32::from_rgb(47, 87, 144),
        Color32::from_rgb(68, 120, 191),
        Color32::from_rgb(110, 158, 222),
    ]
    .into_iter()
    .enumerate()
    {
        let points = (0..=40)
            .map(|step| {
                let fraction = step as f32 / 40.0;
                let phase = fraction * std::f32::consts::TAU - ribbon as f32 * 0.8;
                egui::pos2(
                    rect.left() + rect.width() * (0.15 + fraction * 0.7),
                    center.y + (ribbon as f32 - 1.0) * 15.0 + phase.sin() * 16.0,
                )
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(5.0, color)));
    }
}

fn collage(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    rect: egui::Rect,
    tracks: &[Track],
    fallback: Icon,
) {
    if tracks.is_empty() {
        ui.painter().rect_filled(rect, 10.0, theme::SELECTED);
        icons::paint(
            ui.painter(),
            rect.shrink(rect.width() * 0.27),
            fallback,
            Color32::from_rgb(119, 159, 214),
        );
        return;
    }
    if tracks.len() < 4 {
        image(ui, covers, rect, tracks[0].cover_url.as_deref(), 0);
        return;
    }
    let size = (rect.width() - 2.0) * 0.5;
    for (index, track) in tracks.iter().take(4).enumerate() {
        let origin = rect.min
            + egui::vec2(
                (index % 2) as f32 * (size + 2.0),
                (index / 2) as f32 * (size + 2.0),
            );
        image(
            ui,
            covers,
            egui::Rect::from_min_size(origin, egui::vec2(size, size)),
            track.cover_url.as_deref(),
            index,
        );
    }
}

fn image(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    rect: egui::Rect,
    url: Option<&str>,
    image_id: usize,
) {
    let mut image = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("home-art", image_id))
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    image.set_clip_rect(ui.clip_rect().intersect(rect));
    covers.show(&mut image, url, rect.width());
}

fn card(ui: &mut egui::Ui, size: egui::Vec2, label: &str) -> (egui::Rect, Response) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        response.request_focus();
    }
    let fill = if response.hovered() || response.has_focus() {
        Color32::from_rgb(31, 38, 49)
    } else {
        theme::SURFACE
    };
    ui.painter().rect_filled(rect, 12.0, fill);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, theme::PRIMARY),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    (
        rect,
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
    )
}

fn card_text(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    title: impl Into<egui::WidgetText>,
    subtitle: &str,
) {
    let mut text = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-card-text")
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    text.set_clip_rect(ui.clip_rect().intersect(rect));
    text.spacing_mut().item_spacing.y = 3.0;
    text.add(egui::Label::new(title).truncate().selectable(false));
    text.add(
        egui::Label::new(RichText::new(subtitle).small().color(theme::MUTED))
            .truncate()
            .selectable(false),
    );
}

fn shortcuts(ui: &mut egui::Ui, covers: &mut CoverCache, view: &HomeView<'_>) -> Option<Action> {
    let library_subtitle = view.library_total.map_or_else(
        || "—".to_owned(),
        |total| {
            let noun = match view.language {
                Language::Russian => match (total % 100, total % 10) {
                    (11..=14, _) => "треков",
                    (_, 1) => "трек",
                    (_, 2..=4) => "трека",
                    _ => "треков",
                },
                Language::English => {
                    if total == 1 {
                        "track"
                    } else {
                        "tracks"
                    }
                }
            };
            format!("{total} {noun}")
        },
    );
    let recent_subtitle = if view.recent.is_empty() {
        view.language
            .text("История ваших прослушиваний", "Your listening history")
            .to_owned()
    } else {
        format!(
            "{} {}",
            view.recent.len(),
            view.language.text("недавних треков", "recent tracks")
        )
    };
    let mut entries = vec![
        (
            view.language.text("Моя музыка", "My music"),
            library_subtitle,
            view.library,
            Icon::Library,
            Action::OpenLibrary,
        ),
        (
            view.language.text("Недавнее", "Recent"),
            recent_subtitle,
            view.recent,
            Icon::Clock,
            Action::OpenRecent,
        ),
    ];
    if let Some(track) = view.playing {
        entries.push((
            view.language.text("Продолжить", "Continue"),
            format!("{} · {}", track.artist, track.display_title()),
            std::slice::from_ref(track),
            Icon::Play,
            Action::PlayTrack(track.key()),
        ));
    }
    let columns = columns(ui.available_width()).min(entries.len());
    let width = cell_width(ui.available_width(), columns);
    let mut action = None;
    for (row, entries) in entries.chunks(columns).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (column, (title, subtitle, tracks, icon, intent)) in entries.iter().enumerate() {
                ui.push_id(("home-shortcut", row * columns + column), |ui| {
                    let (rect, response) = card(ui, egui::vec2(width, 152.0), title);
                    let art = egui::Rect::from_min_size(
                        rect.min + egui::vec2(14.0, 14.0),
                        egui::vec2(64.0, 64.0),
                    );
                    collage(ui, covers, art, tracks, *icon);
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_min_size(
                            egui::pos2(rect.right() - 34.0, rect.top() + 18.0),
                            egui::vec2(18.0, 18.0),
                        ),
                        Icon::ArrowRight,
                        theme::MUTED,
                    );
                    card_text(
                        ui,
                        egui::Rect::from_min_max(
                            rect.min + egui::vec2(14.0, 92.0),
                            rect.max - egui::vec2(14.0, 12.0),
                        ),
                        RichText::new(*title).size(16.0).family(theme::semibold()),
                        subtitle,
                    );
                    if response.clicked() {
                        action = Some(intent.clone());
                    }
                });
            }
        });
        ui.add_space(8.0);
    }
    action
}

fn recent_card(ui: &mut egui::Ui, covers: &mut CoverCache, track: &Track, width: f32) -> Response {
    let (rect, response) = card(ui, egui::vec2(width, 72.0), &track.title);
    image(
        ui,
        covers,
        egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 12.0), egui::vec2(48.0, 48.0)),
        track.cover_url.as_deref(),
        0,
    );
    card_text(
        ui,
        egui::Rect::from_min_max(
            rect.min + egui::vec2(72.0, 15.0),
            rect.max - egui::vec2(12.0, 12.0),
        ),
        theme::track_title(
            track,
            ui.visuals().text_color(),
            egui::FontId::new(16.0, theme::semibold()),
        ),
        &track.artist,
    );
    response
}

fn collection_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    collection: &Collection,
    width: f32,
    language: Language,
) -> (Response, bool) {
    let (rect, response) = card(ui, egui::vec2(width, 194.0), &collection.title);
    let art =
        egui::Rect::from_min_size(rect.min + egui::vec2(14.0, 14.0), egui::vec2(112.0, 112.0));
    if collection.cover_url.is_some() {
        image(ui, covers, art, collection.cover_url.as_deref(), 0);
    } else {
        collage(ui, covers, art, &collection.tracks, Icon::Headphones);
    }
    card_text(
        ui,
        egui::Rect::from_min_max(
            rect.min + egui::vec2(14.0, 142.0),
            rect.max - egui::vec2(14.0, 12.0),
        ),
        RichText::new(&collection.title)
            .size(16.0)
            .family(theme::semibold()),
        &collection.subtitle,
    );
    let play_rect = egui::Rect::from_center_size(
        art.right_bottom() - egui::vec2(16.0, 16.0),
        egui::vec2(40.0, 40.0),
    );
    let play = ui.interact(play_rect, ui.id().with("collection-play"), Sense::click());
    if play.clicked() {
        play.request_focus();
    }
    let fill = if play.is_pointer_button_down_on() {
        Color32::from_rgb(218, 228, 241)
    } else {
        Color32::WHITE
    };
    ui.painter().circle_filled(play_rect.center(), 20.0, fill);
    icons::paint(
        ui.painter(),
        play_rect.shrink(10.0),
        Icon::Play,
        theme::BACKGROUND,
    );
    if play.has_focus() {
        ui.painter()
            .circle_stroke(play_rect.center(), 22.0, Stroke::new(1.5, theme::ACCENT));
    }
    let label = language.text("Слушать подборку", "Play playlist");
    play.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let play = play
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label);
    (response, play.clicked())
}

fn section_heading(
    ui: &mut egui::Ui,
    icon: Icon,
    title: &str,
    action: Option<(Icon, &str)>,
    enabled: bool,
) -> bool {
    let mut clicked = false;
    theme::inset_heading(ui, |ui| {
        ui.horizontal(|ui| {
            icons::show(ui, icon, theme::MUTED, 20.0);
            ui.add(
                egui::Label::new(RichText::new(title).size(20.0).family(theme::semibold()))
                    .selectable(false),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some((icon, label)) = action {
                    clicked = if matches!(icon, Icon::Refresh) {
                        icons::compact_button(ui, icon, label, enabled, false).clicked()
                    } else {
                        icons::button(ui, icon, label, enabled).clicked()
                    };
                }
            });
        })
    });
    clicked
}

/// Presentation-only: keep the full history intact. Only exact IDs and an
/// acknowledged library-copy relation identify the same recording.
fn recent_preview(recent: &[Track], limit: usize) -> Vec<&Track> {
    type Key = (i64, i64);
    fn root(parents: &HashMap<Key, Key>, mut key: Key) -> Key {
        while let Some(parent) = parents.get(&key).filter(|parent| **parent != key) {
            key = *parent;
        }
        key
    }
    let mut parents = HashMap::new();
    for track in recent {
        if let Some(source) = track.library_source_key {
            let a = root(&parents, track.key());
            let b = root(&parents, source);
            if a != b {
                parents.insert(a.max(b), a.min(b));
            }
        }
    }
    let mut seen = HashSet::new();
    recent
        .iter()
        .filter(|track| seen.insert(root(&parents, track.key())))
        .take(limit.min(10))
        .collect()
}

fn empty_collections(ui: &mut egui::Ui, view: &HomeView<'_>) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(12)
        .inner_margin(18)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let message = if !view.connected {
                view.language.text(
                    "Подключите VK, чтобы увидеть свои подборки.",
                    "Connect VK to see your collections.",
                )
            } else if view.loading {
                view.language
                    .text("Загружаем подборки…", "Loading collections…")
            } else {
                view.language.text(
                    "Подборок пока нет. Попробуйте обновить страницу.",
                    "No collections yet. Try refreshing the page.",
                )
            };
            ui.add(
                egui::Label::new(RichText::new(message).color(theme::MUTED))
                    .wrap()
                    .selectable(false),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct MixState {
        connected: bool,
        loading: bool,
        playing: bool,
    }

    fn hero_frame(
        context: &egui::Context,
        state: MixState,
        language: Language,
        tick: u32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Action>) {
        let mut action = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(740.0, 240.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| {
                action = hero(
                    ui,
                    &HomeView {
                        language,
                        library: &[],
                        library_total: None,
                        recent: &[],
                        playing: None,
                        connected: state.connected,
                        loading: true,
                        mix_loading: state.loading,
                        mix_playing: state.playing,
                        collections: &[],
                        error: None,
                    },
                );
            },
        );
        output.textures_delta.clear();
        (output, action)
    }

    #[test]
    fn hero_controls_vk_mix_independently_of_library_and_collection_loading() {
        for language in [Language::Russian, Language::English] {
            for (state, label, expected) in [
                (
                    MixState {
                        connected: false,
                        loading: true,
                        playing: false,
                    },
                    language.text("Войти в VK", "Sign in with VK"),
                    Some(Action::SignIn),
                ),
                (
                    MixState {
                        connected: true,
                        loading: false,
                        playing: false,
                    },
                    language.text("Слушать", "Listen"),
                    Some(Action::PlayMix),
                ),
                (
                    MixState {
                        connected: true,
                        loading: false,
                        playing: true,
                    },
                    language.text("Пауза", "Pause"),
                    Some(Action::PlayMix),
                ),
                (
                    MixState {
                        connected: true,
                        loading: true,
                        playing: false,
                    },
                    language.text("Подключаем…", "Connecting…"),
                    None,
                ),
            ] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut output = hero_frame(&context, state, language, 1, Vec::new()).0;
                for tick in 2..4 {
                    output = hero_frame(&context, state, language, tick, Vec::new()).0;
                }
                let text_position = |label: &str| {
                    output.shapes.iter().find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => {
                            Some(text.visual_bounding_rect().center())
                        }
                        _ => None,
                    })
                };
                assert!(text_position("VK MIX").is_some());
                assert!(
                    text_position(language.text("Слушать VK Микс", "Listen to VK Mix")).is_some()
                );
                assert!(
                    text_position(language.text(
                        "Музыкальные рекомендации для вас",
                        "Personal music recommendations for you"
                    ))
                    .is_some()
                );
                let target = text_position(label).expect("Missing VK Mix state label");
                hero_frame(&context, state, language, 4, pointer(target, true));
                let (_, action) = hero_frame(&context, state, language, 5, pointer(target, false));
                assert_eq!(action, expected);
            }
        }
    }

    fn collection_frame(
        context: &egui::Context,
        covers: &mut CoverCache,
        collection: &Collection,
        language: Language,
        tick: u32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Action>) {
        let mut action = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 1100.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| {
                action = show(
                    ui,
                    covers,
                    HomeView {
                        language,
                        library: &[],
                        library_total: None,
                        recent: &[],
                        playing: None,
                        connected: true,
                        loading: true,
                        mix_loading: false,
                        mix_playing: false,
                        collections: std::slice::from_ref(collection),
                        error: None,
                    },
                );
            },
        );
        output.textures_delta.clear();
        (output, action)
    }

    fn pointer(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn recent_cards_fill_two_responsive_rows_and_keep_each_tracks_play_action() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let template = crate::model::demo_tracks().remove(0);
        let recent: Vec<_> = (1..=32)
            .map(|id| Track {
                id,
                owner_id: 42,
                title: format!("R{id:02}"),
                artist: "Artist".into(),
                ..template.clone()
            })
            .collect();
        let mut tick = 0;
        let mut frame = |width: f32, tracks: &[Track], events: Vec<egui::Event>| {
            tick += 1;
            let mut action = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 2000.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = show(
                        ui,
                        &mut covers,
                        HomeView {
                            language: Language::English,
                            library: &[],
                            library_total: None,
                            recent: tracks,
                            playing: None,
                            connected: true,
                            loading: false,
                            mix_loading: false,
                            mix_playing: false,
                            collections: &[],
                            error: None,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            (output, action)
        };

        for (width, columns) in [
            (330.0, 1),
            (700.0, 3),
            (1040.0, 4),
            (1480.0, 5),
            (2200.0, 5),
        ] {
            frame(width, &recent, Vec::new());
            frame(width, &recent, Vec::new());
            let output = frame(width, &recent, Vec::new()).0;
            let mut cards: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if (rect.rect.height() - 72.0).abs() < 0.1 => {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect();
            cards.sort_by(|a, b| {
                a.top()
                    .total_cmp(&b.top())
                    .then(a.left().total_cmp(&b.left()))
            });
            assert_eq!(
                cards.len(),
                columns * 2,
                "Wrong card count at width {width}"
            );
            let (first, second) = cards.split_at(columns);
            assert!(
                first
                    .iter()
                    .all(|card| (card.top() - first[0].top()).abs() < 0.1)
            );
            assert!(
                second
                    .iter()
                    .all(|card| (card.top() - second[0].top()).abs() < 0.1)
            );
            assert!(second[0].top() > first[0].bottom());
            for (left, right) in first.iter().zip(second) {
                assert!((left.left() - right.left()).abs() < 0.1);
                assert!((left.right() - right.right()).abs() < 0.1);
                assert!(
                    left.right() <= width + 0.1,
                    "Card overflows width {width}: {left:?}"
                );
            }
        }

        frame(1040.0, &recent, Vec::new());
        let output = frame(1040.0, &recent, Vec::new()).0;
        let eighth = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "R08" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("The eighth recent listen must fill the second row");
        frame(1040.0, &recent, pointer(eighth, true));
        assert_eq!(
            frame(1040.0, &recent, pointer(eighth, false)).1,
            Some(Action::PlayTrack((42, 8)))
        );

        let output = frame(1040.0, &recent[..3], Vec::new()).0;
        let count = output.shapes.iter().filter(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if (rect.rect.height() - 72.0).abs() < 0.1)).count();
        assert_eq!(count, 3, "A short history must not invent extra cards");
    }

    fn enter(pressed: bool) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    #[test]
    fn library_shortcut_shows_the_total_not_the_loaded_preview_and_keeps_unknown_distinct_from_empty()
     {
        for language in [Language::Russian, Language::English] {
            for (total, expected) in [
                (Some(638), language.text("638 треков", "638 tracks")),
                (Some(0), language.text("0 треков", "0 tracks")),
                (Some(1), language.text("1 трек", "1 track")),
                (None, "—"),
            ] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut covers = CoverCache::new();
                let tracks = crate::model::demo_tracks();
                let mut latest = None;
                for tick in 1..=3 {
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(740.0, 200.0),
                            )),
                            time: Some(f64::from(tick) * 0.1),
                            ..Default::default()
                        },
                        |ui| {
                            shortcuts(
                                ui,
                                &mut covers,
                                &HomeView {
                                    language,
                                    library: &tracks[..2],
                                    library_total: total,
                                    recent: &[],
                                    playing: None,
                                    connected: true,
                                    loading: false,
                                    mix_loading: false,
                                    mix_playing: false,
                                    collections: &[],
                                    error: None,
                                },
                            );
                        },
                    );
                    output.textures_delta.clear();
                    latest = Some(output);
                }
                let output = latest.unwrap();
                let labels: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                assert!(
                    labels.contains(&expected),
                    "Missing total {expected}: {labels:?}"
                );
                assert!(!labels.contains(&language.text("2 трека", "2 tracks")));
                assert!(!labels.contains(&language.text("2 треков", "2 tracks")));
                assert!(labels.contains(
                    &language.text("История ваших прослушиваний", "Your listening history")
                ));
            }
        }
    }

    #[test]
    fn playlist_play_button_and_card_body_produce_separate_click_and_keyboard_intents() {
        for language in [Language::Russian, Language::English] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let collection = Collection {
                id: "synthetic-playlist".into(),
                title: "Synthetic playlist".into(),
                subtitle: "Supplied test data".into(),
                cover_url: None,
                tracks: Vec::new(),
            };
            let mut output =
                collection_frame(&context, &mut covers, &collection, language, 1, Vec::new()).0;
            for tick in 2..4 {
                output = collection_frame(
                    &context,
                    &mut covers,
                    &collection,
                    language,
                    tick,
                    Vec::new(),
                )
                .0;
            }
            let play = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle)
                        if circle.fill == Color32::WHITE && circle.radius == 20.0 =>
                    {
                        Some(circle.center)
                    }
                    _ => None,
                })
                .expect("Missing playlist play button");
            let body = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == collection.title => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .expect("Missing playlist title");

            collection_frame(
                &context,
                &mut covers,
                &collection,
                language,
                4,
                pointer(play, true),
            );
            let (output, action) = collection_frame(
                &context,
                &mut covers,
                &collection,
                language,
                5,
                pointer(play, false),
            );
            assert_eq!(action, Some(Action::PlayCollection(collection.id.clone())));
            let label = language.text("Слушать подборку", "Play playlist");
            assert!(output.platform_output.events.iter().any(|event| matches!(
                event, egui::output::OutputEvent::Clicked(info)
                    if info.label.as_deref() == Some(label) && info.typ == egui::WidgetType::Button
            )));
            let (_, action) =
                collection_frame(&context, &mut covers, &collection, language, 6, enter(true));
            assert_eq!(action, Some(Action::PlayCollection(collection.id.clone())));
            collection_frame(
                &context,
                &mut covers,
                &collection,
                language,
                7,
                enter(false),
            );

            collection_frame(
                &context,
                &mut covers,
                &collection,
                language,
                8,
                pointer(body, true),
            );
            let (_, action) = collection_frame(
                &context,
                &mut covers,
                &collection,
                language,
                9,
                pointer(body, false),
            );
            assert_eq!(action, Some(Action::OpenCollection(collection.id.clone())));
            let (_, action) = collection_frame(
                &context,
                &mut covers,
                &collection,
                language,
                10,
                enter(true),
            );
            assert_eq!(action, Some(Action::OpenCollection(collection.id.clone())));
        }
    }
}

#[cfg(test)]
mod recent_preview_tests {
    use super::*;

    #[test]
    fn preview_limits_only_home_and_retains_distinct_versions_with_matching_titles() {
        let template = crate::model::demo_tracks().remove(0);
        let recent: Vec<_> = (1..=32)
            .map(|id| Track {
                id,
                owner_id: 42,
                title: "Same title".into(),
                release_key: Some((-9, 999)),
                library_source_key: None,
                ..template.clone()
            })
            .collect();
        let preview = recent_preview(&recent, 100);
        assert_eq!(preview.len(), 10);
        assert_eq!(
            preview.iter().map(|track| track.id).collect::<Vec<_>>(),
            (1..=10).collect::<Vec<_>>()
        );
        assert_eq!(recent.len(), 32);
    }

    #[test]
    fn repeated_ids_and_confirmed_library_copies_share_one_preview_card() {
        let original = Track {
            id: 7,
            owner_id: -9,
            library_source_key: None,
            ..crate::model::demo_tracks().remove(0)
        };
        let owned = Track {
            id: 8,
            owner_id: 42,
            library_source_key: Some(original.key()),
            ..original.clone()
        };
        let older = Track {
            library_source_key: None,
            ..owned.clone()
        };
        let other = Track {
            id: 9,
            ..original.clone()
        };
        let recent = vec![older, original.clone(), original, owned, other];
        let preview = recent_preview(&recent, 10);
        assert_eq!(
            preview.iter().map(|track| track.key()).collect::<Vec<_>>(),
            vec![(42, 8), (-9, 9)]
        );
        assert_eq!(recent.len(), 5);
    }
}
