//! "Configure recommendations": pick the artists VK builds the account's
//! recommendations ("Made for you") from, like vk.ru/music/profile. Picked
//! artists are the account's followed artists; nothing is sent until Save.
use eframe::egui::{self, Color32, FontId, RichText, Sense, Stroke};

use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    theme,
    web_audio::FollowedArtist,
};

/// VK asks for at least this many artists before recommendations are tuned.
pub const MINIMUM: usize = 5;

pub struct View<'a> {
    pub language: Language,
    /// The artists offered: VK's suggestions, or search results for `query`.
    pub picks: &'a [FollowedArtist],
    /// The artists picked so far, newest first.
    pub picked: &'a [FollowedArtist],
    pub loading: bool,
    pub more: bool,
    pub searching: bool,
    pub saving: bool,
    pub changed: bool,
    pub error: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    None,
    Close,
    /// Pick or drop an artist (by its entry in `picks` or `picked`).
    Toggle(String),
    LoadMore,
    Save,
}

const PHOTO: f32 = 84.0;
/// How far the ring around a picked photo reaches past it.
const RING: f32 = 4.0;
const CELL: f32 = 104.0;
const SIDE: f32 = 270.0;

pub fn show(
    ctx: &egui::Context,
    covers: &mut CoverCache,
    view: View<'_>,
    query: &mut String,
) -> Output {
    let t = |ru, en| view.language.text(ru, en);
    let content = ctx.content_rect();
    // The frame adds 24 on each side; keep 40 more around it.
    let width = (content.width() - 128.0).clamp(520.0, 960.0);
    let height = (content.height() - 220.0).clamp(360.0, 700.0);
    let mut output = Output::None;
    let modal = egui::Modal::new(egui::Id::new("vk-recommendation-settings"))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .fill(theme::surface())
                .inner_margin(24)
                .corner_radius(20)
                .stroke(Stroke::new(1.0, theme::tone(Color32::from_rgb(44, 53, 67)))),
        )
        .backdrop_color(Color32::from_black_alpha(180))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("Кого вы слушаете?", "Who do you listen to?"))
                            .size(20.0)
                            .family(theme::semibold())
                            .color(theme::tone(Color32::from_rgb(230, 235, 244))),
                    )
                    .truncate()
                    .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if close_button(ui, t("Закрыть", "Close")) {
                        output = Output::Close;
                    }
                });
            });
            ui.label(
                RichText::new(t(
                    "Отметьте хотя бы пятерых исполнителей — по ним VK соберёт «Подборки для вас». Чем больше, тем точнее.",
                    "Pick at least five artists: VK builds \u{201c}Made for you\u{201d} from them. The more, the better.",
                ))
                .color(theme::muted()),
            );
            ui.add_space(12.0);
            let grid_width = width - SIDE - 24.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(grid_width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(grid_width);
                        search_field(ui, query, t("Поиск исполнителей", "Search artists"));
                        ui.add_space(10.0);
                        if let Some(next) = grid(ui, covers, &view, height - 54.0) {
                            output = next;
                        }
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(SIDE, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(SIDE);
                        if let Some(next) = side(ui, covers, &view, height) {
                            output = next;
                        }
                    },
                );
            });
        });
    if modal.should_close() && output == Output::None {
        output = Output::Close;
    }
    output
}

fn close_button(ui: &mut egui::Ui, label: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, 8.0, theme::selected());
    }
    icons::paint(ui.painter(), rect.shrink(7.0), Icon::Close, theme::muted());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
        .clicked()
}

fn search_field(ui: &mut egui::Ui, query: &mut String, hint: &str) {
    egui::Frame::new()
        .fill(theme::background())
        .corner_radius(8)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = 22.0;
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Search, theme::muted(), 16.0);
                ui.add(
                    egui::TextEdit::singleline(query)
                        .id_salt("recommendation-artist-search")
                        .frame(egui::Frame::NONE)
                        .desired_width(ui.available_width())
                        .hint_text(hint),
                );
            });
        });
}

fn grid(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: &View<'_>,
    height: f32,
) -> Option<Output> {
    let t = |ru, en| view.language.text(ru, en);
    let mut output = None;
    egui::ScrollArea::vertical()
        .id_salt("recommendation-artists")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let columns = ((ui.available_width() + 8.0) / CELL).floor().max(1.0) as usize;
            let gap = (ui.available_width() - columns as f32 * CELL) / (columns - 1).max(1) as f32;
            for (row, artists) in view.picks.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap.max(0.0);
                    for artist in artists {
                        ui.push_id(("recommendation-pick", row, &artist.artist.id), |ui| {
                            let picked = view
                                .picked
                                .iter()
                                .any(|item| item.artist.id == artist.artist.id);
                            if artist_cell(ui, covers, artist, picked).clicked() {
                                output = Some(Output::Toggle(artist.artist.id.clone()));
                            }
                        });
                    }
                });
                ui.add_space(6.0);
            }
            if view.loading {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(t("Загружаем исполнителей…", "Loading artists…"))
                            .color(theme::muted()),
                    );
                });
            } else if view.picks.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(if view.searching {
                        t("Никого не нашли.", "No artists found.")
                    } else {
                        t("VK не предложил исполнителей.", "VK offered no artists.")
                    })
                    .color(theme::muted()),
                );
            } else if view.more {
                // Reaching the end of the list asks VK for the next page.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
                if ui.is_rect_visible(rect) {
                    output = output.take().or(Some(Output::LoadMore));
                }
            }
        });
    output
}

fn photo(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    rect: egui::Rect,
    artist: &FollowedArtist,
    id: egui::Id,
) {
    if artist.image_url.is_none() {
        ui.painter()
            .circle_filled(rect.center(), rect.width() * 0.5, theme::selected());
        icons::paint(
            ui.painter(),
            rect.shrink(rect.width() * 0.28),
            Icon::Account,
            theme::muted(),
        );
        return;
    }
    let mut image = ui.new_child(
        egui::UiBuilder::new()
            .id(id)
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    image.set_clip_rect(ui.clip_rect().intersect(rect));
    covers.show_rounded(
        &mut image,
        artist.image_url.as_deref(),
        rect.width(),
        (rect.width() * 0.5) as u8,
    );
}

fn artist_cell(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    artist: &FollowedArtist,
    picked: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(CELL, PHOTO + RING + 44.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            picked,
            &artist.artist.name,
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let circle = egui::Rect::from_center_size(
        // The ring of a picked artist reaches past the photo: leave it room
        // so the first row is not cut by the top of the list.
        egui::pos2(rect.center().x, rect.top() + RING + PHOTO * 0.5),
        egui::vec2(PHOTO, PHOTO),
    );
    photo(ui, covers, circle, artist, ui.id().with("photo"));
    if response.hovered() && !picked {
        ui.painter()
            .circle_filled(circle.center(), PHOTO * 0.5, Color32::from_black_alpha(60));
    }
    if picked {
        ui.painter().circle_stroke(
            circle.center(),
            PHOTO * 0.5 + 1.0,
            Stroke::new(3.0, theme::accent()),
        );
        let badge = egui::Rect::from_center_size(
            circle.right_bottom() - egui::vec2(12.0, 12.0),
            egui::vec2(26.0, 26.0),
        );
        ui.painter()
            .circle_filled(badge.center(), 13.0, theme::accent());
        // The heart's wide top makes it look high when centred exactly.
        icons::paint(
            ui.painter(),
            badge.shrink(6.0).translate(egui::vec2(0.0, 1.0)),
            Icon::HeartFilled,
            theme::on_fill(theme::accent(), Color32::WHITE),
        );
    }
    let name = egui::Rect::from_min_max(egui::pos2(rect.left(), circle.bottom() + 6.0), rect.max);
    let mut job = egui::text::LayoutJob::single_section(
        artist.artist.name.clone(),
        egui::TextFormat::simple(
            FontId::proportional(13.0),
            if picked {
                theme::accent()
            } else {
                ui.visuals().text_color()
            },
        ),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: name.width(),
        max_rows: 2,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job.halign = egui::Align::Center;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(name.center().x, name.top()),
        galley,
        Color32::WHITE,
    );
    response
}

fn side(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: &View<'_>,
    height: f32,
) -> Option<Output> {
    let t = |ru, en| view.language.text(ru, en);
    let mut output = None;
    let count = view.picked.len();
    egui::Frame::new()
        .fill(theme::background())
        .corner_radius(12)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t("Любимые исполнители", "Favourite artists"))
                        .family(theme::semibold()),
                );
                ui.label(RichText::new(count.to_string()).color(theme::muted()));
            });
            ui.add_space(6.0);
            if view.picked.is_empty() {
                ui.label(
                    RichText::new(t(
                        "Пока никого. Нажмите на исполнителя слева.",
                        "Nobody yet. Click an artist on the left.",
                    ))
                    .color(theme::muted()),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("recommendation-favourites")
                .max_height((height - 170.0).max(120.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for artist in view.picked {
                        ui.push_id(("recommendation-favourite", &artist.artist.id), |ui| {
                            if favourite_row(ui, covers, artist, view.language) {
                                output = Some(Output::Toggle(artist.artist.id.clone()));
                            }
                        });
                    }
                });
        });
    ui.add_space(10.0);
    progress(ui, count, view.language);
    ui.add_space(10.0);
    let label = if view.saving {
        t("Сохраняем…", "Saving…")
    } else {
        t("Сохранить", "Save")
    };
    if ui
        .add_enabled(
            view.changed && !view.saving,
            egui::Button::new(theme::on_primary_text(label))
                .fill(theme::primary())
                .corner_radius(10)
                .min_size(egui::vec2(ui.available_width(), 36.0)),
        )
        .clicked()
    {
        output = Some(Output::Save);
    }
    if let Some(error) = view.error {
        ui.add_space(8.0);
        ui.label(RichText::new(error).color(theme::error()));
    }
    output
}

fn favourite_row(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    artist: &FollowedArtist,
    language: Language,
) -> bool {
    let (rect, row) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), Sense::hover());
    let photo_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 6.0), egui::vec2(36.0, 36.0));
    photo(ui, covers, photo_rect, artist, ui.id().with("photo"));
    let heart = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 16.0, rect.center().y),
        egui::vec2(32.0, 32.0),
    );
    let name = egui::Rect::from_min_max(
        egui::pos2(photo_rect.right() + 10.0, rect.top()),
        egui::pos2(heart.left() - 6.0, rect.bottom()),
    );
    let mut job = egui::text::LayoutJob::single_section(
        artist.artist.name.clone(),
        egui::TextFormat::simple(FontId::proportional(14.0), ui.visuals().text_color()),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(name.width().max(0.0));
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(name.left(), name.center().y - galley.size().y * 0.5),
        galley,
        Color32::WHITE,
    );
    let _ = row;
    let response = ui
        .interact(heart, ui.id().with("heart"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let label = language.text("Убрать из любимых", "Remove from favourites");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if response.hovered() {
        ui.painter().rect_filled(heart, 8.0, theme::selected());
    }
    icons::paint(
        ui.painter(),
        heart.shrink(8.0),
        Icon::HeartFilled,
        theme::accent(),
    );
    response.on_hover_text(label).clicked()
}

fn progress(ui: &mut egui::Ui, count: usize, language: Language) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme::background());
    let target = (count.min(MINIMUM) as f32 / MINIMUM as f32).clamp(0.0, 1.0);
    // The bar slides to its new length as artists are picked or dropped.
    let share =
        ui.ctx()
            .animate_value_with_time(egui::Id::new("recommendation-progress"), target, 0.25);
    if share > 0.0 {
        let mut fill = rect;
        fill.set_width(rect.width() * share);
        ui.painter().rect_filled(fill, 8.0, theme::primary());
    }
    let text = if count >= MINIMUM {
        language
            .text(
                "Рекомендации подстроятся под вас",
                "Recommendations will follow you",
            )
            .to_owned()
    } else {
        let left = MINIMUM - count;
        match language {
            Language::Russian => format!("Выберите ещё {left}"),
            Language::English => format!("Pick {left} more"),
        }
    };
    // Text sits over both the filled and the empty part of the bar.
    let color = if share >= 0.5 {
        theme::on_fill(theme::primary(), ui.visuals().text_color())
    } else {
        ui.visuals().text_color()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(13.0),
        color,
    );
}
