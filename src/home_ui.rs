//! The home page renders data supplied by the application and returns intents.
//! It never invents VK collections or starts authentication or playback itself.
use eframe::egui::{self, Align, Color32, Layout, Response, RichText, Sense, Stroke};
use std::collections::{HashMap, HashSet};

use crate::{
    carousel::carousel,
    cover_art::CoverCache,
    cover_play::{self, PlayState, state_icon},
    i18n::Language,
    icons::{self, Icon},
    model::{AlbumRef, Track},
    theme,
    web_audio::{ArtistMix, CatalogMix, FriendPlaylist, HomeCatalog},
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
    /// The collection whose queue is current, and one loading to autoplay.
    pub active_collection: Option<&'a str>,
    pub pending_collection: Option<&'a str>,
    pub catalog: &'a HomeCatalog,
    /// The Home catalog mix that is current, and whether it is audible.
    pub active_mix: Option<&'a CatalogMix>,
    pub active_mix_playing: bool,
    pub pending_mix: Option<&'a CatalogMix>,
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
    PlayCatalogMix(CatalogMix),
    OpenPlaylist(AlbumRef),
    PlayFriendTrack((i64, i64), (i64, i64)),
    SignIn,
    /// Open VK Mix settings: mood, familiarity and language.
    ConfigureMix,
}

/// One vertical rhythm for Home: the gap above a section heading equals the
/// gap between that heading and its content (plus egui's item spacing).
const SECTION_GAP: f32 = 10.0;

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
        // VK Mix's colours keep their white text in a light theme too.
        action = theme::dark_scene(ui, |ui| hero(ui, &view)).or(action.take());
        ui.add_space(SECTION_GAP);
        action = shortcuts(ui, covers, &view).or(action.take());
        if let Some(error) = view.error {
            ui.add_space(SECTION_GAP);
            egui::Frame::new()
                .fill(Color32::from_rgb(54, 35, 39))
                .corner_radius(12)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(error).color(theme::error()))
                            .wrap()
                            .selectable(false),
                    );
                });
        }

        if !view.recent.is_empty() {
            ui.add_space(SECTION_GAP);
            if section_heading(
                ui,
                Icon::Clock,
                view.language.text("Недавно слушали", "Recently played"),
                Some((Icon::Play, view.language.text("Слушать", "Play"))),
                true,
            ) {
                action = Some(Action::PlayRecent);
            }
            ui.add_space(SECTION_GAP);
            let columns = ((ui.available_width() + 16.0) / 216.0)
                .floor()
                .clamp(1.0, 5.0) as usize;
            let width = cell_width(ui.available_width(), columns);
            let recent = recent_preview(view.recent, columns * 2);
            for (row, tracks) in recent.chunks(columns).enumerate() {
                if row > 0 {
                    ui.add_space(4.0);
                }
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
            }
        }

        ui.add_space(SECTION_GAP);
        section_heading(
            ui,
            Icon::Sparkles,
            view.language.text("Подборки для вас", "Made for you"),
            None,
            true,
        );
        ui.add_space(SECTION_GAP);
        if view.collections.is_empty() {
            empty_collections(ui, &view);
        } else {
            // Two full rows that slide together: the first half on top.
            let top = view.collections.len().div_ceil(2);
            // Few collections stretch over the full width instead of
            // leaving empty columns; an odd count leaves the second row short.
            let columns = columns(ui.available_width()).min(top.max(1));
            let width = cell_width(ui.available_width(), columns);
            let spans = fixed_spans(top, width);
            carousel(
                ui,
                "home-collections",
                &spans,
                COLLECTION_HEIGHT * 2.0 + 8.0,
                view.language,
                |ui, column| {
                    for index in [column, column + top] {
                        let Some(collection) = view.collections.get(index) else {
                            continue;
                        };
                        ui.push_id(("home-collection", &collection.id), |ui| {
                            let state = collection_state(&view, &collection.id);
                            let (body, play_clicked) = collection_card(
                                ui,
                                covers,
                                collection,
                                width,
                                view.language,
                                state,
                            );
                            if play_clicked {
                                action = Some(Action::PlayCollection(collection.id.clone()));
                            } else if body.clicked() {
                                action = Some(Action::OpenCollection(collection.id.clone()));
                            }
                        });
                    }
                },
            );
        }
        action = catalog_sections(ui, covers, &view).or(action.take());
        ui.add_space(8.0);
    });
    action
}

const CARD_GAP: f32 = 16.0;
const COLLECTION_HEIGHT: f32 = 194.0;

fn fixed_spans(count: usize, width: f32) -> Vec<(f32, f32)> {
    (0..count)
        .map(|index| (index as f32 * (width + CARD_GAP), width))
        .collect()
}

fn mix_state(view: &HomeView<'_>, mix: &CatalogMix) -> PlayState {
    PlayState::new(
        view.pending_mix == Some(mix),
        view.active_mix == Some(mix),
        view.active_mix_playing,
    )
}

fn collection_state(view: &HomeView<'_>, id: &str) -> PlayState {
    PlayState::new(
        view.pending_collection == Some(id),
        view.active_collection == Some(id),
        view.active_mix_playing,
    )
}

fn catalog_sections(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: &HomeView<'_>,
) -> Option<Action> {
    let mut action = None;
    let language = view.language;
    for (key, icon, title, tiles) in [
        (
            "home-genres",
            Icon::Genres,
            language.text("Жанры", "Genres"),
            &view.catalog.genres,
        ),
        (
            "home-moods",
            Icon::Mood,
            language.text("Настроения и занятия", "Moods and activities"),
            &view.catalog.moods,
        ),
    ] {
        if tiles.is_empty() {
            continue;
        }
        ui.add_space(SECTION_GAP);
        section_heading(ui, icon, title, None, true);
        ui.add_space(SECTION_GAP);
        let mut x = 0.0;
        let spans: Vec<(f32, f32)> = tiles
            .iter()
            .map(|tile| {
                let width = chip_width(ui, &tile.title);
                let span = (x, width);
                x += width + CHIP_GAP;
                span
            })
            .collect();
        carousel(ui, key, &spans, 36.0, language, |ui, index| {
            let tile = &tiles[index];
            ui.push_id(("home-mix-tile", &tile.id), |ui| {
                if mix_chip(ui, &tile.title, mix_state(view, &tile.mix)).clicked() {
                    action = Some(Action::PlayCatalogMix(tile.mix.clone()));
                }
            });
        });
    }

    if !view.catalog.friends.is_empty() {
        ui.add_space(SECTION_GAP);
        section_heading(
            ui,
            Icon::Friends,
            language.text("Слушайте друг друга", "Listen with friends"),
            None,
            true,
        );
        ui.add_space(SECTION_GAP);
        let friends = &view.catalog.friends;
        let width = cell_width(ui.available_width(), columns(ui.available_width()));
        let height = friends.iter().map(friend_card_height).fold(0.0, f32::max);
        carousel(
            ui,
            "home-friends",
            &fixed_spans(friends.len(), width),
            height,
            language,
            |ui, index| {
                let friend = &friends[index];
                let key = (friend.playlist.owner_id, friend.playlist.id);
                ui.push_id(("home-friend", key), |ui| {
                    if let Some(intent) = friend_card(ui, covers, friend, width, language) {
                        action = Some(intent);
                    }
                });
            },
        );
    }

    if !view.catalog.artist_mixes.is_empty() {
        ui.add_space(SECTION_GAP);
        section_heading(
            ui,
            Icon::Radio,
            language.text("Микс по исполнителям", "Artist mixes"),
            None,
            true,
        );
        ui.add_space(SECTION_GAP);
        let mixes = &view.catalog.artist_mixes;
        let width = cell_width(ui.available_width(), columns(ui.available_width()));
        carousel(
            ui,
            "home-artist-mixes",
            &fixed_spans(mixes.len(), width),
            COLLECTION_HEIGHT,
            language,
            |ui, index| {
                let mix = &mixes[index];
                ui.push_id(("home-artist-mix", index), |ui| {
                    let state = mix_state(view, &mix.mix);
                    if artist_mix_card(ui, covers, mix, width, state) {
                        action = Some(Action::PlayCatalogMix(mix.mix.clone()));
                    }
                });
            },
        );
    }
    action
}

const CHIP_GAP: f32 = 8.0;
const CHIP_DISC: f32 = 11.0;

fn chip_galley(ui: &egui::Ui, title: &str) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::new(14.0, theme::semibold()),
        ui.visuals().text_color(),
    )
}

fn chip_width(ui: &egui::Ui, title: &str) -> f32 {
    7.0 + CHIP_DISC * 2.0 + 8.0 + chip_galley(ui, title).size().x + 16.0
}

/// A compact genre or mood pill: the app's surface, a blue play disc and the
/// accent outline while its mix is the current one.
fn mix_chip(ui: &mut egui::Ui, title: &str, state: PlayState) -> Response {
    let text_color = ui.visuals().text_color();
    let galley = chip_galley(ui, title);
    let disc = CHIP_DISC;
    let size = egui::vec2(chip_width(ui, title), 36.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        response.request_focus();
    }
    let hovered = response.hovered() || response.has_focus();
    let active = state != PlayState::Idle;
    let fill = if active {
        theme::selected()
    } else if hovered {
        theme::tone(Color32::from_rgb(31, 38, 49))
    } else {
        theme::surface()
    };
    let radius = rect.height() * 0.5;
    ui.painter().rect_filled(rect, radius, fill);
    if active || hovered {
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(
                1.0,
                if active {
                    theme::accent()
                } else {
                    theme::primary()
                },
            ),
            egui::StrokeKind::Inside,
        );
    }
    let center = egui::pos2(rect.left() + 7.0 + disc, rect.center().y);
    ui.painter().circle_filled(
        center,
        disc,
        if active {
            theme::accent()
        } else {
            theme::primary()
        },
    );
    state_icon(
        ui,
        egui::Rect::from_center_size(center, egui::vec2(12.0, 12.0)),
        state,
        Color32::WHITE,
    );
    ui.painter().galley(
        egui::pos2(
            center.x + disc + 8.0,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        text_color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), title)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

const FRIEND_HEADER: f32 = 84.0;
const FRIEND_ROW: f32 = 44.0;

/// How far below a cover's top a title line and the small artists line
/// under it start, so that the two sit centred on the cover.
fn centred_text_top(ui: &egui::Ui, cover: f32, title: &egui::FontId, gap: f32) -> f32 {
    let small = egui::TextStyle::Small.resolve(ui.style());
    let height = ui.fonts_mut(|fonts| fonts.row_height(title) + gap + fonts.row_height(&small));
    ((cover - height) * 0.5).max(0.0)
}

fn friend_card_height(friend: &FriendPlaylist) -> f32 {
    FRIEND_HEADER + 6.0 + FRIEND_ROW * friend.tracks.len().min(3) as f32 + 6.0
}

fn friend_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    friend: &FriendPlaylist,
    width: f32,
    language: Language,
) -> Option<Action> {
    let rows = friend.tracks.len().min(3);
    let height = friend_card_height(friend);
    let (rect, response) = card(ui, egui::vec2(width, height), &friend.playlist.title);
    let header = egui::Rect::from_min_size(rect.min, egui::vec2(width, FRIEND_HEADER));
    if let Some([red, green, blue]) = friend.color {
        ui.painter().rect_filled(
            header,
            egui::CornerRadius {
                nw: 12,
                ne: 12,
                sw: 0,
                se: 0,
            },
            Color32::from_rgba_unmultiplied(red, green, blue, 46),
        );
    }
    let mut text = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-friend-text")
            .max_rect(header.shrink2(egui::vec2(14.0, 12.0)))
            .layout(Layout::top_down(Align::Min)),
    );
    text.set_clip_rect(ui.clip_rect().intersect(header));
    text.spacing_mut().item_spacing.y = 3.0;
    if let Some(percent) = friend.match_percent {
        text.add(
            egui::Label::new(
                RichText::new(format!(
                    "{percent}% · {}",
                    language.text("совпадение с вашим вкусом", "match with your taste")
                ))
                .small()
                .color(theme::tone(Color32::from_rgb(196, 210, 230))),
            )
            .truncate()
            .selectable(false),
        );
    }
    text.add(
        egui::Label::new(
            RichText::new(&friend.playlist.title)
                .size(16.0)
                .family(theme::semibold()),
        )
        .truncate()
        .selectable(false),
    );
    text.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if friend.owner_photo.is_some() {
            covers.show_rounded(ui, friend.owner_photo.as_deref(), 16.0, 8);
        }
        ui.add(
            egui::Label::new(
                RichText::new(&friend.owner_name)
                    .small()
                    .color(theme::muted()),
            )
            .truncate()
            .selectable(false),
        );
    });

    let mut intent = None;
    for (index, track) in friend.tracks.iter().take(rows).enumerate() {
        let row = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + 6.0,
                header.bottom() + 6.0 + FRIEND_ROW * index as f32,
            ),
            egui::vec2(width - 12.0, FRIEND_ROW),
        );
        let hit = ui.interact(row, ui.id().with(("friend-track", index)), Sense::click());
        if hit.hovered() {
            ui.painter()
                .rect_filled(row, 8.0, theme::tone(Color32::from_rgb(36, 44, 57)));
        }
        let art = egui::Rect::from_min_size(row.min + egui::vec2(6.0, 6.0), egui::vec2(32.0, 32.0));
        image(ui, covers, art, track.cover_url.as_deref(), index + 1);
        if hit.hovered() {
            hover_play(ui.painter(), art);
        }
        let top = centred_text_top(ui, art.height(), &egui::FontId::proportional(14.0), 1.0);
        let mut line = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("home-friend-track", index))
                .max_rect(egui::Rect::from_min_max(
                    egui::pos2(row.left() + 48.0, art.top() + top),
                    row.max - egui::vec2(8.0, 0.0),
                ))
                .layout(Layout::top_down(Align::Min)),
        );
        line.set_clip_rect(ui.clip_rect().intersect(row));
        line.spacing_mut().item_spacing.y = 1.0;
        line.add(
            egui::Label::new(theme::track_title(
                track,
                line.visuals().text_color(),
                egui::FontId::proportional(14.0),
            ))
            .truncate()
            .selectable(false),
        );
        let badge = theme::explicit_badge(
            line.ctx(),
            Some(track),
            egui::TextStyle::Small.resolve(line.style()).size,
        );
        theme::badged_line(&mut line, badge, |ui| {
            ui.add(
                egui::Label::new(RichText::new(&track.artist).small().color(theme::muted()))
                    .truncate()
                    .selectable(false),
            )
        });
        hit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &track.title));
        if hit
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            intent = Some(Action::PlayFriendTrack(
                (friend.playlist.owner_id, friend.playlist.id),
                track.key(),
            ));
        }
    }
    intent.or_else(|| {
        response
            .clicked()
            .then(|| Action::OpenPlaylist(friend.playlist.clone()))
    })
}

fn artist_mix_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    mix: &ArtistMix,
    width: f32,
    state: PlayState,
) -> bool {
    let (rect, response) = card(ui, egui::vec2(width, 194.0), &mix.title);
    if state != PlayState::Idle {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    let art =
        egui::Rect::from_min_size(rect.min + egui::vec2(14.0, 14.0), egui::vec2(112.0, 112.0));
    let mut photo = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-artist-photo")
            .max_rect(art)
            .layout(Layout::top_down(Align::Min)),
    );
    photo.set_clip_rect(ui.clip_rect().intersect(art));
    covers.show_rounded(&mut photo, mix.photo_url.as_deref(), art.width(), 56);
    let play = cover_play::button(
        ui,
        art.right_bottom() - egui::vec2(16.0, 16.0),
        20.0,
        ui.id().with("artist-mix-play"),
        state,
        &mix.title,
    );
    card_text(
        ui,
        egui::Rect::from_min_max(
            rect.min + egui::vec2(14.0, 142.0),
            rect.max - egui::vec2(14.0, 12.0),
        ),
        RichText::new(&mix.title)
            .size(16.0)
            .family(theme::semibold()),
        &mix.description,
    );
    response.clicked() || play.clicked()
}

fn columns(width: f32) -> usize {
    ((width + 16.0) / 216.0).floor().clamp(1.0, 4.0) as usize
}

fn cell_width(width: f32, columns: usize) -> f32 {
    ((width - 16.0 * columns.saturating_sub(1) as f32) / columns as f32).max(1.0)
}

/// VK Mix's banner: a fixed blend of the "For you" mix cover colors (blue,
/// magenta, cyan, purple), dimmed slightly so white text stays readable.
const MIX_COLORS: [Color32; 4] = [
    Color32::from_rgb(41, 135, 197),
    Color32::from_rgb(158, 48, 180),
    Color32::from_rgb(13, 150, 161),
    Color32::from_rgb(144, 51, 175),
];

/// VK Mix's colors with the left shade laid over them, sampled finely
/// enough (on every original stop too) to keep both gradients' shape.
fn shaded_mix_colors() -> Vec<Color32> {
    const SAMPLES: usize = 16 * (MIX_COLORS.len() - 1) + 1;
    let shade = crate::detail_ui::fade_color(Some(MIX_COLORS[0])).to_array();
    let lerp = |a: [u8; 4], b: [u8; 4], t: f32| -> [f32; 4] {
        std::array::from_fn(|i| egui::lerp(f32::from(a[i])..=f32::from(b[i]), t))
    };
    (0..SAMPLES)
        .map(|sample| {
            let x = sample as f32 / (SAMPLES - 1) as f32;
            let position = x * (MIX_COLORS.len() - 1) as f32;
            let index = (position.floor() as usize).min(MIX_COLORS.len() - 2);
            let base = lerp(
                MIX_COLORS[index].to_array(),
                MIX_COLORS[index + 1].to_array(),
                position - index as f32,
            );
            // Premultiplied "over", as the shade drawn on top would blend.
            let over = lerp(shade, [0; 4], x);
            let keep = 1.0 - over[3] / 255.0;
            let [r, g, b, a] = std::array::from_fn(|i| (over[i] + base[i] * keep).round() as u8);
            Color32::from_rgba_premultiplied(r, g, b, a)
        })
        .collect()
}

fn hero(ui: &mut egui::Ui, view: &HomeView<'_>) -> Option<Action> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 208.0), Sense::hover());
    // The artist header's shade from the left, in the card's own blue,
    // mixed into the card's colors: one shape keeps the rounded corners
    // clean, where a second one on top leaves a light rim around them.
    theme::gradient_stops(ui.painter(), rect, 16.0, &shaded_mix_colors());
    let inner = rect.shrink(24.0);
    let text_right = inner.right();
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
                .color(Color32::from_rgba_unmultiplied(255, 255, 255, 200)),
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
        egui::Label::new(
            RichText::new(subtitle).color(Color32::from_rgba_unmultiplied(255, 255, 255, 215)),
        )
        .wrap()
        .selectable(false),
    );

    let mut buttons = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-hero-action")
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(inner.left(), inner.bottom() - 36.0),
                egui::vec2(text_right - inner.left(), 36.0),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    buttons.style_mut().visuals.widgets.inactive.bg_fill = theme::primary();
    buttons.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    buttons.style_mut().visuals.widgets.hovered.bg_fill = theme::accent();
    buttons.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    theme::on_primary_widgets(buttons.style_mut());
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
    let mut clicked = icons::button(
        &mut buttons,
        icon,
        label,
        !view.connected || !view.mix_loading,
    )
    .clicked()
    .then_some(action);
    if view.connected {
        buttons.add_space(8.0);
        let widgets = &mut buttons.style_mut().visuals.widgets;
        widgets.inactive.bg_fill = Color32::from_black_alpha(72);
        widgets.hovered.bg_fill = Color32::from_black_alpha(110);
        widgets.active.bg_fill = Color32::from_black_alpha(140);
        widgets.active.bg_stroke = Stroke::NONE;
        if icons::button(
            &mut buttons,
            Icon::Sliders,
            view.language.text("Настроить", "Configure"),
            true,
        )
        .clicked()
        {
            clicked = Some(Action::ConfigureMix);
        }
    }
    clicked
}

fn collage(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    rect: egui::Rect,
    tracks: &[Track],
    fallback: Icon,
) {
    if tracks.is_empty() {
        ui.painter().rect_filled(rect, 10.0, theme::selected());
        icons::paint(
            ui.painter(),
            rect.shrink(rect.width() * 0.27),
            fallback,
            theme::accent_light(Color32::from_rgb(119, 159, 214)),
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
        theme::tone(Color32::from_rgb(31, 38, 49))
    } else {
        theme::surface()
    };
    ui.painter().rect_filled(rect, 12.0, fill);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, theme::primary()),
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
    card_text_badged(ui, rect, title, subtitle, None);
}

/// The card's text with the track's explicit badge before the subtitle.
fn card_text_badged(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    title: impl Into<egui::WidgetText>,
    subtitle: &str,
    badge: Option<f32>,
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
    theme::badged_line(&mut text, badge, |ui| {
        ui.add(
            egui::Label::new(RichText::new(subtitle).small().color(theme::muted()))
                .truncate()
                .selectable(false),
        )
    });
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
        if row > 0 {
            ui.add_space(8.0);
        }
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
                    if response.hovered() && matches!(intent, Action::PlayTrack(_)) {
                        // "Continue" plays its track, as a recent card does.
                        hover_play(ui.painter(), art);
                    }
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_min_size(
                            egui::pos2(rect.right() - 34.0, rect.top() + 18.0),
                            egui::vec2(18.0, 18.0),
                        ),
                        Icon::ArrowRight,
                        theme::muted(),
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
    }
    action
}

/// A hovered track's cover, as in a queue row: dimmed under a play sign
/// half its size.
fn hover_play(painter: &egui::Painter, art: egui::Rect) {
    painter.rect_filled(art, 6.0, Color32::from_black_alpha(165));
    icons::paint(
        painter,
        art.shrink(art.width() * 0.25),
        Icon::Play,
        Color32::WHITE,
    );
}

fn recent_card(ui: &mut egui::Ui, covers: &mut CoverCache, track: &Track, width: f32) -> Response {
    let (rect, response) = card(ui, egui::vec2(width, 72.0), &track.title);
    let art = egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 12.0), egui::vec2(48.0, 48.0));
    image(ui, covers, art, track.cover_url.as_deref(), 0);
    if response.hovered() {
        hover_play(ui.painter(), art);
    }
    let badge = theme::explicit_badge(
        ui.ctx(),
        Some(track),
        egui::TextStyle::Small.resolve(ui.style()).size,
    );
    let title_font = egui::FontId::new(16.0, theme::semibold());
    let top = centred_text_top(ui, art.height(), &title_font, 3.0);
    card_text_badged(
        ui,
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + 72.0, art.top() + top),
            rect.max - egui::vec2(12.0, 0.0),
        ),
        theme::track_title(track, ui.visuals().text_color(), title_font),
        &track.artist,
        badge,
    );
    response
}

fn collection_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    collection: &Collection,
    width: f32,
    language: Language,
    state: PlayState,
) -> (Response, bool) {
    let (rect, response) = card(ui, egui::vec2(width, 194.0), &collection.title);
    if state != PlayState::Idle {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
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
    let label = language.text("Слушать подборку", "Play playlist");
    // Home keeps its original corner position; only the colour is shared.
    let play = cover_play::button(
        ui,
        art.right_bottom() - egui::vec2(16.0, 16.0),
        20.0,
        ui.id().with("collection-play"),
        state,
        label,
    );
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
            icons::show(ui, icon, theme::muted(), 20.0);
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
        .fill(theme::surface())
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
                egui::Label::new(RichText::new(message).color(theme::muted()))
                    .wrap()
                    .selectable(false),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carousel::{next_stop, previous_stop};

    #[test]
    fn the_mix_card_is_one_opaque_shape_shaded_from_the_left() {
        let colors = shaded_mix_colors();
        assert!(
            colors.iter().all(|color| color.a() == 255),
            "no see-through rim"
        );
        assert_eq!(colors.last(), MIX_COLORS.last(), "no shade on the right");
        let brightness =
            |color: Color32| u32::from(color.r()) + u32::from(color.g()) + u32::from(color.b());
        assert!(
            brightness(colors[0]) * 2 < brightness(MIX_COLORS[0]),
            "dark on the left"
        );
        // Every original stop is sampled, so the card keeps its colors.
        let step = (colors.len() - 1) / (MIX_COLORS.len() - 1);
        assert!(
            brightness(colors[step]) < brightness(MIX_COLORS[1]),
            "the shade thins out"
        );
        assert!(brightness(colors[2 * step]) > brightness(colors[step]) / 2);
    }

    #[derive(Clone, Copy)]
    struct PlayState {
        connected: bool,
        loading: bool,
        playing: bool,
    }

    fn hero_frame(
        context: &egui::Context,
        state: PlayState,
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
                        active_collection: None,
                        pending_collection: None,
                        catalog: &HomeCatalog::default(),
                        active_mix: None,
                        active_mix_playing: false,
                        pending_mix: None,
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
                    PlayState {
                        connected: false,
                        loading: true,
                        playing: false,
                    },
                    language.text("Войти в VK", "Sign in with VK"),
                    Some(Action::SignIn),
                ),
                (
                    PlayState {
                        connected: true,
                        loading: false,
                        playing: false,
                    },
                    language.text("Слушать", "Listen"),
                    Some(Action::PlayMix),
                ),
                (
                    PlayState {
                        connected: true,
                        loading: false,
                        playing: true,
                    },
                    language.text("Пауза", "Pause"),
                    Some(Action::PlayMix),
                ),
                (
                    PlayState {
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
                // Settings sit next to Listen once VK is connected, even while
                // the mix is loading.
                let configure = text_position(language.text("Настроить", "Configure"));
                assert_eq!(configure.is_some(), state.connected);
                if let Some(target) = configure {
                    assert!(target.x > text_position(label).unwrap().x);
                    hero_frame(&context, state, language, 6, pointer(target, true));
                    let (_, action) =
                        hero_frame(&context, state, language, 7, pointer(target, false));
                    assert_eq!(action, Some(Action::ConfigureMix));
                }
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
                        active_collection: None,
                        pending_collection: None,
                        catalog: &HomeCatalog::default(),
                        active_mix: None,
                        active_mix_playing: false,
                        pending_mix: None,
                        error: None,
                    },
                );
            },
        );
        output.textures_delta.clear();
        (output, action)
    }

    fn catalog_frame(
        context: &egui::Context,
        covers: &mut CoverCache,
        catalog: &HomeCatalog,
        tick: u32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Action>) {
        let mut action = None;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 2000.0),
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
                        language: Language::Russian,
                        library: &[],
                        library_total: None,
                        recent: &[],
                        playing: None,
                        connected: true,
                        loading: false,
                        mix_loading: false,
                        mix_playing: false,
                        collections: &[],
                        active_collection: None,
                        pending_collection: None,
                        catalog,
                        active_mix: catalog.genres.first().map(|tile| &tile.mix),
                        active_mix_playing: true,
                        pending_mix: None,
                        error: None,
                    },
                );
            },
        );
        output.textures_delta.clear();
        (output, action)
    }

    fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing {label}"))
    }

    #[test]
    fn recent_and_friend_track_text_sits_centred_on_the_cover() {
        let template = crate::model::demo_tracks().remove(0);
        let track = Track {
            title: "Centred title".into(),
            artist: "Centred artist".into(),
            cover_url: None,
            ..template
        };
        let friend = FriendPlaylist {
            playlist: AlbumRef {
                id: 36,
                owner_id: 7,
                title: "Friend list".into(),
                access_key: None,
                cover_url: None,
            },
            owner_name: "Synthetic Friend".into(),
            owner_photo: None,
            match_percent: None,
            color: None,
            tracks: vec![track.clone()],
        };
        for (friend_row, cover) in [(false, 48.0), (true, 32.0)] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let mut output = egui::FullOutput::default();
            for tick in 0..3 {
                output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(600.0, 400.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        if friend_row {
                            friend_card(ui, &mut covers, &friend, 300.0, Language::English);
                        } else {
                            recent_card(ui, &mut covers, &track, 300.0);
                        }
                    },
                );
                output.textures_delta.clear();
            }
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
            let (title, artist) = (text("Centred title"), text("Centred artist"));
            let art = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) => Some(rect.rect),
                    egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds()),
                    _ => None,
                })
                .find(|rect| {
                    (rect.width() - cover).abs() < 0.6
                        && (rect.height() - cover).abs() < 0.6
                        && rect.right() < title.left()
                })
                .expect("the cover");
            // From the title's capitals to the artists' baseline, the same
            // space above and below inside the cover.
            let above = title.top() - art.top();
            let below = art.bottom() - artist.bottom();
            assert!(
                (above - below).abs() <= 1.0,
                "friend row {friend_row}: above {above}, below {below}"
            );
        }
    }

    #[test]
    fn catalog_chips_friend_rows_and_artist_mixes_produce_their_own_intents() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let template = crate::model::demo_tracks().remove(0);
        let track = |id: i64, title: &str| Track {
            id,
            owner_id: 7,
            title: title.into(),
            artist: "Synthetic Artist".into(),
            cover_url: None,
            ..template.clone()
        };
        let genre = CatalogMix::Common {
            options: vec![("additionals".into(), "pop".into())],
        };
        let mood = CatalogMix::Common {
            options: vec![("vibes".into(), "happy".into())],
        };
        let artist = CatalogMix::Artist {
            entity_id: "3424231346320955355".into(),
        };
        let playlist = AlbumRef {
            id: 36,
            owner_id: 7,
            title: "Friend list".into(),
            access_key: None,
            cover_url: None,
        };
        let catalog = HomeCatalog {
            genres: vec![crate::web_audio::MixTile {
                id: "pop_genre".into(),
                title: "Поп".into(),
                mix: genre.clone(),
            }],
            moods: vec![crate::web_audio::MixTile {
                id: "happy_vibe".into(),
                title: "Радостно".into(),
                mix: mood.clone(),
            }],
            friends: vec![FriendPlaylist {
                playlist: playlist.clone(),
                owner_name: "Synthetic Friend".into(),
                owner_photo: None,
                match_percent: Some(96),
                color: Some([0x36, 0x81, 0xFF]),
                tracks: vec![track(1, "Friend track one"), track(2, "Friend track two")],
            }],
            artist_mixes: vec![ArtistMix {
                title: "Synthetic Mix Artist".into(),
                description: "A, B, C".into(),
                photo_url: None,
                mix: artist.clone(),
            }],
        };
        let mut output = catalog_frame(&context, &mut covers, &catalog, 1, Vec::new()).0;
        for tick in 2..4 {
            output = catalog_frame(&context, &mut covers, &catalog, tick, Vec::new()).0;
        }
        for heading in [
            "Жанры",
            "Настроения и занятия",
            "Слушайте друг друга",
            "Микс по исполнителям",
        ] {
            text_center(&output, heading);
        }
        text_center(&output, "96% · совпадение с вашим вкусом");
        let mut tick = 4;
        let mut click = |label: &str| {
            let position = text_center(&output, label);
            catalog_frame(
                &context,
                &mut covers,
                &catalog,
                tick,
                pointer(position, true),
            );
            let (_, action) = catalog_frame(
                &context,
                &mut covers,
                &catalog,
                tick + 1,
                pointer(position, false),
            );
            tick += 2;
            action
        };
        assert_eq!(click("Поп"), Some(Action::PlayCatalogMix(genre)));
        assert_eq!(click("Радостно"), Some(Action::PlayCatalogMix(mood)));
        assert_eq!(
            click("Friend track two"),
            Some(Action::PlayFriendTrack((7, 36), (7, 2)))
        );
        assert_eq!(click("Friend list"), Some(Action::OpenPlaylist(playlist)));
        assert_eq!(
            click("Synthetic Mix Artist"),
            Some(Action::PlayCatalogMix(artist))
        );
        // A friend's track dims its cover under a play sign on hover.
        let dimmed = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.fill == Color32::from_black_alpha(165)
                            && (rect.rect.width() - 32.0).abs() < 0.1)
                })
                .count()
        };
        let row = text_center(&output, "Friend track one");
        catalog_frame(
            &context,
            &mut covers,
            &catalog,
            40,
            vec![egui::Event::PointerMoved(row)],
        );
        let hovered = catalog_frame(
            &context,
            &mut covers,
            &catalog,
            41,
            vec![egui::Event::PointerMoved(row)],
        )
        .0;
        assert_eq!(dimmed(&hovered), 1);
    }

    #[test]
    fn carousel_stops_page_by_whole_items_and_keep_the_last_page_full() {
        // Four cards of 200 with 16 gaps; three are visible in 632.
        let spans = fixed_spans(8, 200.0);
        let visible = 632.0;
        let max = spans.last().map(|(x, w)| x + w).unwrap() - visible;
        assert_eq!(next_stop(&spans, 0.0, visible, max), 648.0);
        assert_eq!(next_stop(&spans, 648.0, visible, max), max);
        assert_eq!(next_stop(&spans, max, visible, max), max);
        assert_eq!(previous_stop(&spans, max, visible), 432.0);
        assert_eq!(previous_stop(&spans, 648.0, visible), 0.0);
        assert_eq!(previous_stop(&spans, 0.0, visible), 0.0);

        // Variable chip widths stop at the first chip cut by the edge.
        let chips = [(0.0, 90.0), (98.0, 120.0), (226.0, 70.0), (304.0, 140.0)];
        assert_eq!(next_stop(&chips, 0.0, 300.0, 144.0), 144.0_f32.min(226.0));
        assert_eq!(next_stop(&chips, 0.0, 400.0, 44.0), 44.0);
    }

    fn collections_frame(
        context: &egui::Context,
        covers: &mut CoverCache,
        collections: &[Collection],
        tick: u32,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(660.0, 1100.0),
                )),
                time: Some(f64::from(tick) / 60.0),
                events,
                ..Default::default()
            },
            |ui| {
                show(
                    ui,
                    covers,
                    HomeView {
                        language: Language::Russian,
                        library: &[],
                        library_total: None,
                        recent: &[],
                        playing: None,
                        connected: true,
                        loading: false,
                        mix_loading: false,
                        mix_playing: false,
                        collections,
                        active_collection: None,
                        pending_collection: None,
                        catalog: &HomeCatalog::default(),
                        active_mix: None,
                        active_mix_playing: false,
                        pending_mix: None,
                        error: None,
                    },
                );
            },
        );
        output.textures_delta.clear();
        output
    }

    fn eight_collections() -> Vec<Collection> {
        (0..8)
            .map(|index| Collection {
                id: format!("c{index}"),
                title: format!("Collection {index}"),
                subtitle: String::new(),
                cover_url: None,
                tracks: Vec::new(),
            })
            .collect()
    }

    fn fully_visible(output: &egui::FullOutput, label: &str) -> bool {
        output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text)
                if text.galley.text() == label
                    && text.visual_bounding_rect().left() >= 0.0
                    && text.visual_bounding_rect().right() <= 660.0)
        })
    }

    #[test]
    fn shift_wheel_scrolls_only_the_hovered_row_sideways() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let collections = eight_collections();
        let output = collections_frame(&context, &mut covers, &collections, 1, Vec::new());
        let first = text_center(&output, "Collection 0");
        assert!(!fully_visible(&output, "Collection 3"));
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            modifiers: egui::Modifiers::SHIFT,
            phase: egui::TouchPhase::Move,
        };
        collections_frame(
            &context,
            &mut covers,
            &collections,
            2,
            vec![egui::Event::PointerMoved(first), wheel],
        );
        let mut output = collections_frame(&context, &mut covers, &collections, 3, Vec::new());
        for tick in 4..90 {
            output = collections_frame(&context, &mut covers, &collections, tick, Vec::new());
        }
        assert!(
            fully_visible(&output, "Collection 3"),
            "the row moved sideways"
        );
    }

    #[test]
    fn hovering_a_row_shows_an_edge_arrow_that_slides_to_the_next_cards() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let collections = eight_collections();
        let mut frame = |tick: u32, events: Vec<egui::Event>| {
            collections_frame(&context, &mut covers, &collections, tick, events)
        };
        let arrows = |output: &egui::FullOutput| -> Vec<egui::Pos2> {
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle)
                        if circle.radius == 13.0 && circle.fill != Color32::TRANSPARENT =>
                    {
                        Some(circle.center)
                    }
                    _ => None,
                })
                .collect()
        };
        let output = frame(1, Vec::new());
        assert!(
            arrows(&output).is_empty(),
            "arrows stay hidden without hover"
        );
        let first = text_center(&output, "Collection 0");
        let hidden = |output: &egui::FullOutput| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text)
                    if text.galley.text() == "Collection 3"
                        && text.visual_bounding_rect().right() <= 660.0)
            })
        };
        assert!(!hidden(&output), "the fourth column starts off screen");
        let output = frame(2, vec![egui::Event::PointerMoved(first)]);
        let shown = arrows(&output);
        assert_eq!(shown.len(), 1, "only the next arrow on the first page");
        let next = shown[0];
        assert!(next.x > 600.0);
        frame(3, pointer(next, true));
        frame(4, pointer(next, false));
        let mut output = frame(5, Vec::new());
        for tick in 6..20 {
            output = frame(tick, Vec::new());
        }
        assert!(hidden(&output), "the row slid to the next cards");
        let output = frame(
            20,
            vec![egui::Event::PointerMoved(text_center(
                &output,
                "Collection 3",
            ))],
        );
        assert_eq!(arrows(&output).len(), 1, "only the back arrow at the end");
        assert!(arrows(&output)[0].x < 300.0);
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
    fn the_continue_card_dims_its_cover_on_hover_but_the_recent_card_does_not() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let track = crate::model::demo_tracks().remove(0);
        let recent = vec![track.clone()];
        let mut tick = 0;
        let mut frame = |events: Vec<egui::Event>| {
            tick += 1;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1040.0, 1400.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    show(
                        ui,
                        &mut covers,
                        HomeView {
                            language: Language::English,
                            library: &[],
                            library_total: None,
                            recent: &recent,
                            playing: Some(&track),
                            connected: true,
                            loading: false,
                            mix_loading: false,
                            mix_playing: false,
                            collections: &[],
                            active_collection: None,
                            pending_collection: None,
                            catalog: &HomeCatalog::default(),
                            active_mix: None,
                            active_mix_playing: false,
                            pending_mix: None,
                            error: None,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            output
        };
        let at = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap()
        };
        let dimmed = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.fill == Color32::from_black_alpha(165)
                            && (rect.rect.width() - 64.0).abs() < 0.1)
                })
                .count()
        };
        frame(Vec::new());
        let output = frame(Vec::new());
        assert_eq!(dimmed(&output), 0);
        let (resume, recent_card) = (at(&output, "Continue"), at(&output, "Recent"));
        frame(vec![egui::Event::PointerMoved(resume)]);
        assert_eq!(dimmed(&frame(vec![egui::Event::PointerMoved(resume)])), 1);
        frame(vec![egui::Event::PointerMoved(recent_card)]);
        assert_eq!(
            dimmed(&frame(vec![egui::Event::PointerMoved(recent_card)])),
            0,
            "the Recent card opens a page; it plays nothing"
        );
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
                            active_collection: None,
                            pending_collection: None,
                            catalog: &HomeCatalog::default(),
                            active_mix: None,
                            active_mix_playing: false,
                            pending_mix: None,
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

        // Hovering a card dims its cover under a play sign, as in the queue.
        let dimmed = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.fill == Color32::from_black_alpha(165)
                            && (rect.rect.width() - 48.0).abs() < 0.1)
                })
                .count()
        };
        frame(1040.0, &recent, vec![egui::Event::PointerGone]);
        let still = frame(1040.0, &recent, Vec::new()).0;
        assert_eq!(dimmed(&still), 0);
        frame(1040.0, &recent, vec![egui::Event::PointerMoved(eighth)]);
        let hovered = frame(1040.0, &recent, vec![egui::Event::PointerMoved(eighth)]).0;
        assert_eq!(dimmed(&hovered), 1, "only the hovered card dims");

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
                                    active_collection: None,
                                    pending_collection: None,
                                    catalog: &HomeCatalog::default(),
                                    active_mix: None,
                                    active_mix_playing: false,
                                    pending_mix: None,
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
    fn playlist_play_button_and_card_body_produce_separate_intents_without_a_click_focus_ring() {
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
                    egui::Shape::Circle(circle) if circle.fill == theme::accent() => {
                        Some(circle.center)
                    }
                    _ => None,
                })
                .expect("Missing blue playlist play button");
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
            // A mouse click leaves no focus ring, so Enter does not replay it.
            let (_, action) =
                collection_frame(&context, &mut covers, &collection, language, 6, enter(true));
            assert_eq!(action, None);
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
