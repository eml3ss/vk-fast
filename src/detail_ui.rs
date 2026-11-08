//! Artist and album presentation. Networking, history and playback stay with the
//! application; these widgets only return the action the user chose.
use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense, Stroke};

use crate::{
    artist_releases::{Kind as ReleaseKind, Release},
    cover_art::CoverCache,
    cover_play::{self, Playback},
    i18n::Language,
    icons::{self, Icon},
    model::{AlbumRef, ArtistRef},
    theme,
};

pub struct ArtistView<'a> {
    pub language: Language,
    pub name: &'a str,
    pub image_url: Option<&'a str>,
    pub track_count: usize,
    pub loading: bool,
    pub playing: bool,
    pub follow: Option<FollowView>,
}

pub struct AlbumView<'a> {
    pub language: Language,
    pub title: &'a str,
    pub artists: &'a [ArtistRef],
    pub creator: Option<&'a crate::web_audio::PlaylistCreator>,
    pub image_url: Option<&'a str>,
    pub subtitle: &'a str,
    pub track_count: usize,
    pub loading: bool,
    pub playing: bool,
    pub can_edit_playlist: bool,
    pub follow: Option<FollowView>,
}

#[derive(Clone, Copy)]
pub struct FollowView {
    pub saved: bool,
    pub busy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Play,
    Shuffle,
    OpenAlbum(AlbumRef),
    /// Start a release from its card without opening it.
    PlayAlbum(AlbumRef),
    OpenArtist(ArtistRef),
    OpenCreator(crate::web_audio::PlaylistCreator),
    EditPlaylist,
    ToggleLibraryLike,
    CopyLink,
    ReloadReleases,
}

/// The names remain separate click targets even when VK supplies one combined
/// display string. Legacy metadata is resolved by its full name by the caller;
/// commas and ampersands inside an artist name are never guessed separators.
pub fn artist_links(ui: &mut egui::Ui, artists: &[ArtistRef], fallback: &str) -> Option<ArtistRef> {
    artist_links_with_font(
        ui,
        artists,
        fallback,
        egui::TextStyle::Small.resolve(ui.style()),
    )
}

/// Uses the same canonical links at a caller-selected caption size.
pub fn artist_links_with_font(
    ui: &mut egui::Ui,
    artists: &[ArtistRef],
    fallback: &str,
    font: FontId,
) -> Option<ArtistRef> {
    let fallback_artist;
    let artists = if artists.is_empty() && !fallback.trim().is_empty() {
        fallback_artist = [ArtistRef {
            id: String::new(),
            name: fallback.to_owned(),
        }];
        &fallback_artist[..]
    } else {
        artists
    };
    let height = ui.fonts_mut(|fonts| fonts.row_height(&font)).max(16.0);
    let (line, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(0.0), height),
        Sense::hover(),
    );
    let mut left = line.left();
    let mut action = None;
    for (index, artist) in artists.iter().enumerate() {
        if left >= line.right() - 1.0 {
            break;
        }
        if index != 0 {
            let separator = ui
                .painter()
                .layout_no_wrap(", ".into(), font.clone(), theme::MUTED);
            if left + separator.size().x >= line.right() {
                break;
            }
            ui.painter().galley(
                egui::pos2(left, line.center().y - separator.size().y * 0.5),
                separator.clone(),
                theme::MUTED,
            );
            left += separator.size().x;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(
            artist.name.clone(),
            font.clone(),
            theme::MUTED,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(line.right() - left);
        let galley = ui.painter().layout_job(job);
        let rect = egui::Rect::from_min_size(
            egui::pos2(left, line.top()),
            egui::vec2(galley.size().x.min(line.right() - left), line.height()),
        );
        let response = ui.interact(
            rect,
            ui.id()
                .with(("artist-link", index, &artist.id, &artist.name)),
            Sense::click(),
        );
        if response.clicked() {
            response.request_focus();
            action = Some(artist.clone());
        }
        let color = if response.hovered() || response.has_focus() {
            ui.visuals().text_color()
        } else {
            theme::MUTED
        };
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(line))
            .galley_with_override_text_color(
                egui::pos2(left, line.center().y - galley.size().y * 0.5),
                galley.clone(),
                color,
            );
        if response.hovered() || response.has_focus() {
            ui.painter().line_segment(
                [
                    egui::pos2(rect.left(), rect.bottom() - 1.0),
                    egui::pos2(rect.right(), rect.bottom() - 1.0),
                ],
                Stroke::new(0.7, color),
            );
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), &artist.name)
        });
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
        left += galley.size().x;
        if galley.elided {
            break;
        }
    }
    action
}

pub fn artist_header(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: ArtistView<'_>,
) -> Option<Action> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 238.0), Sense::hover());
    let portrait_size = 214.0_f32.min((width * 0.43).max(0.0));
    let color = if ui.is_rect_visible(rect) {
        covers.header_color(ui.ctx(), view.image_url, portrait_size)
    } else {
        None
    };
    background(ui.painter(), rect, color);
    if width >= 380.0 {
        let portrait = egui::Rect::from_min_size(
            egui::pos2(rect.right() - portrait_size - 12.0, rect.top() + 12.0),
            egui::Vec2::splat(portrait_size),
        );
        artwork(ui, covers, portrait, view.image_url, "artist-portrait");
        if let Some(url) = view.image_url.filter(|url| !url.is_empty()) {
            cover_viewer(ui, covers, portrait, url, view.language);
        }
        // The portrait recedes into the page's dark blue surface rather than
        // placing text over a bright face or introducing an unreadable crop.
        fade(ui.painter(), rect, color);
    }
    let inner = rect.shrink(22.0);
    let text_right = if width >= 540.0 {
        rect.right() - portrait_size * 0.65
    } else {
        inner.right()
    };
    let mut text = child(
        ui,
        "artist-heading",
        egui::Rect::from_min_max(inner.min, egui::pos2(text_right, inner.bottom() - 48.0)),
    );
    kicker(&mut text, view.language.text("ИСПОЛНИТЕЛЬ", "ARTIST"));
    text.add_space(20.0);
    text.add(
        egui::Label::new(
            RichText::new(view.name)
                .size(if width < 500.0 { 28.0 } else { 36.0 })
                .family(theme::semibold()),
        )
        .truncate()
        .selectable(false),
    );
    text.add_space(4.0);
    text.add(
        egui::Label::new(
            RichText::new(
                view.language
                    .text("Популярные треки и релизы", "Popular tracks and releases"),
            )
            .color(theme::MUTED),
        )
        .truncate()
        .selectable(false),
    );
    let mut buttons = child(
        ui,
        "artist-controls",
        egui::Rect::from_min_max(egui::pos2(inner.left(), inner.bottom() - 38.0), inner.max),
    );
    controls(
        &mut buttons,
        view.language,
        view.track_count,
        view.loading,
        view.playing,
        view.follow,
    )
}

pub fn album_header(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: AlbumView<'_>,
) -> Option<Action> {
    let label = view.language.text("АЛЬБОМ", "ALBUM");
    collection_header(ui, covers, view, label, false)
}

pub fn playlist_header(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: AlbumView<'_>,
) -> Option<Action> {
    let label = view.language.text("ПЛЕЙЛИСТ", "PLAYLIST");
    collection_header(ui, covers, view, label, true)
}

/// VK descriptions stay plain text, with their paragraph breaks intact. The
/// caller places this below the playlist header and before its track list.
pub fn playlist_description(ui: &mut egui::Ui, description: &str) {
    if description.trim().is_empty() {
        return;
    }
    ui.add_space(12.0);
    let mut rect = ui.available_rect_before_wrap();
    rect.min.x = (rect.left() + 8.0).min(rect.right());
    rect.max.x = (rect.right() - 8.0).max(rect.left());
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("playlist-description")
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.add(
                egui::Label::new(RichText::new(description).size(14.0).color(theme::MUTED))
                    .wrap()
                    .selectable(false),
            );
        },
    );
}

fn playlist_creator(
    ui: &mut egui::Ui,
    language: Language,
    creator: &crate::web_audio::PlaylistCreator,
) -> Option<Action> {
    if creator.owner_id == 0 || creator.name.trim().is_empty() {
        return None;
    }
    let width = ui.available_width().max(0.0);
    let chevron = ui
        .painter()
        .layout_no_wrap("›".into(), FontId::proportional(18.0), theme::MUTED);
    let gap = 5.0;
    let name_width = (width - chevron.size().x - gap).max(0.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        creator.name.clone(),
        FontId::proportional(14.0),
        theme::MUTED,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(name_width);
    let name = ui.painter().layout_job(job);
    let size = egui::vec2((name.size().x + gap + chevron.size().x).min(width), 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let color = if response.hovered() || response.has_focus() {
        ui.visuals().text_color()
    } else {
        theme::MUTED
    };
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.galley_with_override_text_color(
        egui::pos2(rect.left(), rect.center().y - name.size().y * 0.5),
        name,
        color,
    );
    painter.galley_with_override_text_color(
        egui::pos2(
            rect.right() - chevron.size().x,
            rect.center().y - chevron.size().y * 0.5,
        ),
        chevron,
        color,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &creator.name)
    });
    let clicked = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(language.text("Открыть страницу владельца", "Open owner page"))
        .clicked();
    clicked.then_some(Action::OpenCreator(creator.clone()))
}

fn collection_header(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: AlbumView<'_>,
    kind: &str,
    playlist: bool,
) -> Option<Action> {
    let width = ui.available_width();
    let wide = width >= 520.0;
    let height = if wide { 226.0 } else { 320.0 };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let inner = rect.shrink(20.0);
    let art_size = if wide {
        180.0
    } else {
        108.0_f32.min(inner.width())
    };
    let color = if ui.is_rect_visible(rect) {
        covers.header_color(ui.ctx(), view.image_url, art_size)
    } else {
        None
    };
    background(ui.painter(), rect, color);
    let art = egui::Rect::from_min_size(inner.min, egui::Vec2::splat(art_size));
    artwork(ui, covers, art, view.image_url, "album-cover");
    if let Some(url) = view.image_url.filter(|url| !url.is_empty()) {
        cover_viewer(ui, covers, art, url, view.language);
    }
    let text_origin = if wide {
        egui::pos2(art.right() + 24.0, inner.top() + 4.0)
    } else {
        egui::pos2(inner.left(), art.bottom() + 14.0)
    };
    let mut text = child(
        ui,
        "album-heading",
        egui::Rect::from_min_max(
            text_origin,
            egui::pos2(
                inner.right()
                    - if playlist && view.can_edit_playlist && wide {
                        38.0
                    } else {
                        0.0
                    },
                inner.bottom() - 45.0,
            ),
        ),
    );
    kicker(&mut text, kind);
    text.add_space(if wide { 12.0 } else { 2.0 });
    text.add(
        egui::Label::new(
            RichText::new(view.title)
                .size(if wide { 30.0 } else { 24.0 })
                .family(theme::semibold()),
        )
        .truncate()
        .selectable(false),
    );
    text.add_space(3.0);
    let mut action = if playlist {
        view.creator
            .and_then(|creator| playlist_creator(&mut text, view.language, creator))
    } else {
        artist_links(&mut text, view.artists, "").map(Action::OpenArtist)
    };
    if !view.subtitle.is_empty() {
        text.add(
            egui::Label::new(RichText::new(view.subtitle).small().color(theme::MUTED))
                .truncate()
                .selectable(false),
        );
    }
    let mut buttons = child(
        ui,
        "album-controls",
        egui::Rect::from_min_max(egui::pos2(text_origin.x, inner.bottom() - 38.0), inner.max),
    );
    action = controls(
        &mut buttons,
        view.language,
        view.track_count,
        view.loading,
        view.playing,
        if !playlist || !view.can_edit_playlist {
            view.follow
        } else {
            None
        },
    )
    .or(action);
    if playlist && view.can_edit_playlist {
        let edit_rect = egui::Rect::from_min_size(
            egui::pos2(inner.right() - 30.0, inner.top()),
            egui::vec2(30.0, 30.0),
        );
        let response = ui
            .add_enabled_ui(!view.loading, |ui| {
                let response = ui.interact(
                    edit_rect,
                    ui.id().with("playlist-hero-edit"),
                    Sense::click(),
                );
                if response.hovered() || response.has_focus() {
                    ui.painter().rect_filled(edit_rect, 8.0, theme::SELECTED);
                }
                icons::paint(
                    ui.painter(),
                    edit_rect.shrink(5.0),
                    Icon::Edit,
                    if response.hovered() || response.has_focus() {
                        ui.visuals().text_color()
                    } else {
                        theme::MUTED
                    },
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        view.language
                            .text("Редактировать плейлист", "Edit playlist"),
                    )
                });
                response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(
                        view.language
                            .text("Редактировать плейлист", "Edit playlist"),
                    )
            })
            .inner;
        if response.clicked() {
            action = Some(Action::EditPlaylist);
        }
    }
    action
}

pub struct ReleasesView<'a> {
    pub language: Language,
    pub artist_id: &'a str,
    pub latest: &'a [Release],
    pub discography: &'a [Release],
    pub incomplete: bool,
    pub loading: bool,
    /// The full discography is still loading behind the latest releases.
    pub pending: bool,
    pub playback: Playback,
    pub related: &'a [crate::web_audio::RelatedArtist],
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ReleaseTab {
    #[default]
    Albums,
    Singles,
}

pub fn artist_releases(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: ReleasesView<'_>,
) -> Option<Action> {
    let mut action = releases(ui, covers, view.language, view.latest, view.playback);
    ui.add_space(22.0);
    let id = ui.make_persistent_id(("artist-release-tab", view.artist_id));
    let mut tab = ui
        .ctx()
        .data(|data| data.get_temp::<ReleaseTab>(id))
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut tab,
            ReleaseTab::Albums,
            view.language.text("Альбомы", "Albums"),
        );
        ui.selectable_value(
            &mut tab,
            ReleaseTab::Singles,
            view.language.text("Синглы и EP", "Singles & EPs"),
        );
        if view.pending {
            ui.add_space(4.0);
            ui.spinner();
        }
    });
    ui.ctx().data_mut(|data| data.insert_temp(id, tab));
    ui.add_space(10.0);
    if view.incomplete {
        theme::inset_heading(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(view.language.text(
                        "Не удалось загрузить всю дискографию.",
                        "Could not load the full discography.",
                    ))
                    .small()
                    .color(theme::MUTED),
                );
                if ui
                    .add_enabled(
                        !view.loading,
                        egui::Button::new(view.language.text("Повторить", "Retry")).frame(false),
                    )
                    .clicked()
                {
                    action = Some(Action::ReloadReleases);
                }
            })
        });
        ui.add_space(6.0);
    }
    let filtered: Vec<_> = view
        .discography
        .iter()
        .filter(|release| match tab {
            ReleaseTab::Albums => matches!(release.kind, ReleaseKind::Album | ReleaseKind::Unknown),
            ReleaseTab::Singles => matches!(release.kind, ReleaseKind::Single | ReleaseKind::Ep),
        })
        .collect();
    // The space under the last element before "You may also like".
    let mut tail_gap = SECTION_GAP;
    if filtered.is_empty() && !view.incomplete {
        // Same space under the note as between the tabs and the note: the
        // note's scope already adds one item spacing below it.
        tail_gap = 10.0 - ui.spacing().item_spacing.y;
        theme::inset_heading(ui, |ui| {
            ui.label(
                RichText::new(match tab {
                    ReleaseTab::Albums => view.language.text("Альбомов пока нет", "No albums yet"),
                    ReleaseTab::Singles => view
                        .language
                        .text("Синглов и EP пока нет", "No singles or EPs yet"),
                })
                .color(theme::MUTED),
            )
        });
    }
    // Two rows first; the rest behind "Show all", which then offers "Hide".
    let limit = release_columns(ui.available_width()) * 2;
    let expanded_id = ui.make_persistent_id(("artist-discography-all", view.artist_id, tab as u8));
    let expanded = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(expanded_id))
        .unwrap_or(false);
    let shown = if expanded {
        &filtered[..]
    } else {
        &filtered[..filtered.len().min(limit)]
    };
    ui.push_id(("artist-discography", view.artist_id, tab as u8), |ui| {
        if let Some(intent) = release_grid(ui, covers, view.language, shown, view.playback) {
            action = Some(intent);
        }
    });
    if filtered.len() > limit {
        tail_gap = LINK_GAP;
        if expand_button(ui, view.language, expanded) {
            ui.ctx()
                .data_mut(|data| data.insert_temp(expanded_id, !expanded));
            // Re-lay out this frame: the page height changes at once, and the
            // half-drawn frame (old grid, new scroll range) would flicker.
            ui.ctx().request_discard("discography expanded or hidden");
        }
    }
    if let Some(intent) = related_artists(
        ui,
        covers,
        view.language,
        view.artist_id,
        view.related,
        tail_gap,
    ) {
        action = Some(intent);
    }
    // The page ends with the same space that separates its blocks.
    ui.add_space(SECTION_GAP);
    action
}

const SECTION_GAP: f32 = 22.0;
/// Extra space on each side of "Show all", added to the item spacing.
const LINK_GAP: f32 = 8.0;

/// "Show all" / "Hide" under a shortened grid: a compact link with equal
/// space above and below. Returns true when clicked.
pub fn expand_button(ui: &mut egui::Ui, language: Language, expanded: bool) -> bool {
    ui.add_space(LINK_GAP);
    theme::inset_heading(ui, |ui| {
        let label = if expanded {
            language.text("Скрыть", "Hide")
        } else {
            language.text("Показать все", "Show all")
        };
        let response = ui.add(
            egui::Label::new(RichText::new(label).color(theme::ACCENT))
                .sense(Sense::click())
                .selectable(false),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    })
}

/// VK's "You may also like" artists at the bottom of an artist page.
fn related_artists(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    artist_id: &str,
    related: &[crate::web_audio::RelatedArtist],
    top_gap: f32,
) -> Option<Action> {
    if related.is_empty() {
        return None;
    }
    ui.add_space(top_gap);
    theme::inset_heading(ui, |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(language.text("Вам могут понравиться", "You may also like"))
                    .size(20.0)
                    .family(theme::semibold()),
            )
            .selectable(false),
        )
    });
    ui.add_space(10.0);
    // One sliding row with edge arrows on hover, like Home's sections.
    let columns = ((ui.available_width() + 12.0) / 212.0)
        .floor()
        .clamp(1.0, 6.0) as usize;
    let width = ((ui.available_width() - 12.0 * columns.saturating_sub(1) as f32) / columns as f32)
        .max(1.0);
    let spans: Vec<(f32, f32)> = (0..related.len())
        .map(|index| (index as f32 * (width + 12.0), width))
        .collect();
    let mut action = None;
    crate::carousel::carousel(
        ui,
        &format!("artist-related:{artist_id}"),
        &spans,
        64.0,
        language,
        |ui, index| {
            let item = &related[index];
            ui.push_id(("related-artist", &item.artist.id), |ui| {
                if related_card(ui, covers, item, width) {
                    action = Some(Action::OpenArtist(item.artist.clone()));
                }
            });
        },
    );
    action
}

fn related_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    item: &crate::web_audio::RelatedArtist,
    width: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 64.0), Sense::click());
    let hovered = response.hovered() || response.has_focus();
    ui.painter().rect_filled(
        rect,
        12.0,
        if hovered {
            Color32::from_rgb(31, 38, 49)
        } else {
            theme::SURFACE
        },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let avatar =
        egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::Vec2::splat(44.0));
    let mut image = child(ui, "related-avatar", avatar);
    covers.show_rounded(&mut image, item.image_url.as_deref(), 44.0, 22);
    let mut text = child(
        ui,
        "related-name",
        egui::Rect::from_min_max(
            egui::pos2(avatar.right() + 12.0, rect.top()),
            rect.max - egui::vec2(10.0, 0.0),
        ),
    );
    text.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.add(
            egui::Label::new(RichText::new(&item.artist.name).family(theme::semibold()))
                .truncate()
                .selectable(false),
        );
    });
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &item.artist.name)
    });
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// The catalog supplies latest releases in display order. Fit one row to the window.
pub fn releases(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    releases: &[Release],
    playback: Playback,
) -> Option<Action> {
    if releases.is_empty() {
        return None;
    }
    ui.add_space(22.0);
    theme::inset_heading(ui, |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(language.text("Последние релизы", "Latest releases"))
                    .size(20.0)
                    .family(theme::semibold()),
            )
            .selectable(false),
        )
    });
    ui.add_space(8.0);
    let visible: Vec<_> = releases
        .iter()
        .take(release_columns(ui.available_width()))
        .collect();
    ui.push_id("latest-releases", |ui| {
        release_grid(ui, covers, language, &visible, playback)
    })
    .inner
}

fn release_columns(width: f32) -> usize {
    ((width + 14.0) / 174.0).floor().clamp(1.0, 16.0) as usize
}

fn release_grid(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    releases: &[&Release],
    playback: Playback,
) -> Option<Action> {
    if releases.is_empty() {
        return None;
    }
    let columns = release_columns(ui.available_width());
    let width = ((ui.available_width() - 14.0 * columns.saturating_sub(1) as f32) / columns as f32)
        .max(1.0);
    let card_height = (width - 24.0).clamp(1.0, 200.0) + 65.0;
    let row_height = card_height + 10.0;
    let rows = releases.len().div_ceil(columns);
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), rows as f32 * row_height - 10.0),
        Sense::hover(),
    );
    let start = (((ui.clip_rect().top() - area.top()) / row_height)
        .floor()
        .max(0.0) as usize)
        .min(rows);
    let end = (((ui.clip_rect().bottom() - area.top()) / row_height)
        .ceil()
        .max(0.0) as usize)
        .min(rows);
    let mut action = None;
    // Cards paint into a child bounded by the grid, so their inner widgets
    // can never stretch the page below the allocated rows.
    let mut cards = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("release-cards")
            .max_rect(area),
    );
    cards.set_clip_rect(ui.clip_rect());
    let ui = &mut cards;
    for row in start..end {
        for column in 0..columns {
            let Some(release) = releases.get(row * columns + column) else {
                break;
            };
            let rect = egui::Rect::from_min_size(
                area.min + egui::vec2(column as f32 * (width + 14.0), row as f32 * row_height),
                egui::vec2(width, card_height),
            );
            if let Some(intent) = release_card(ui, covers, language, release, rect, playback) {
                action = Some(intent);
            }
        }
    }
    action
}

fn release_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    release: &Release,
    rect: egui::Rect,
    playback: Playback,
) -> Option<Action> {
    let album = &release.album;
    ui.push_id(("release", album.owner_id, album.id), |ui| {
        let response = ui.interact(rect, ui.id().with("card"), Sense::click());
        let fill = if response.hovered() || response.has_focus() {
            Color32::from_rgb(31, 38, 49)
        } else {
            theme::SURFACE
        };
        ui.painter().rect_filled(rect, 12.0, fill);
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                12.0,
                Stroke::new(1.0, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let art_size = (rect.width() - 24.0).clamp(1.0, 200.0);
        let art = egui::Rect::from_min_size(
            rect.min + egui::vec2((rect.width() - art_size) * 0.5, 12.0),
            egui::Vec2::splat(art_size),
        );
        artwork(ui, covers, art, album.cover_url.as_deref(), "release-art");
        // Like VK: the blue play button appears inside the cover only while
        // the pointer is over the card, even for the release that is playing.
        let state = playback.state((album.owner_id, album.id));
        let play = ui
            .rect_contains_pointer(rect)
            .then(|| {
                cover_play::button(
                    ui,
                    cover_play::corner(art),
                    cover_play::radius(art),
                    ui.id().with("play"),
                    state,
                    language.text("Слушать", "Play"),
                )
            })
            .is_some_and(|play| play.clicked());
        let mut text = child(
            ui,
            "release-title",
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + 12.0, art.bottom() + 11.0),
                rect.max - egui::vec2(12.0, 6.0),
            ),
        );
        text.spacing_mut().item_spacing.y = 2.0;
        text.add(
            egui::Label::new(RichText::new(&album.title).family(theme::semibold()))
                .truncate()
                .selectable(false),
        );
        text.add(
            egui::Label::new(
                RichText::new(release.caption(language))
                    .small()
                    .color(theme::MUTED),
            )
            .truncate()
            .selectable(false),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &album.title)
        });
        let clicked = response.clicked();
        if clicked {
            response.surrender_focus();
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if play {
            Some(Action::PlayAlbum(album.clone()))
        } else {
            clicked.then(|| Action::OpenAlbum(album.clone()))
        }
    })
    .inner
}

fn child(ui: &mut egui::Ui, id: &'static str, rect: egui::Rect) -> egui::Ui {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(ui.clip_rect().intersect(rect));
    child
}

fn artwork(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    rect: egui::Rect,
    url: Option<&str>,
    id: &'static str,
) {
    let mut image = child(ui, id, rect);
    covers.show(&mut image, url, rect.width());
}

/// A release or playlist cover, or an artist photo, opens full size over a
/// dimmed window. Any click, or Escape, closes it.
fn cover_viewer(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    art: egui::Rect,
    url: &str,
    language: Language,
) {
    let label = language.text("Открыть изображение", "Open image");
    let id = egui::Id::new(("release-cover-viewer", url));
    let open = ui.interact(art, id.with("open"), Sense::click());
    open.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let frame = ui.ctx().cumulative_frame_nr();
    if open.on_hover_cursor(egui::CursorIcon::ZoomIn).clicked() {
        ui.data_mut(|data| data.insert_temp(id, frame));
    }
    // The viewer belongs to the page that opened it: leaving the page for a
    // frame closes it instead of reopening it on a later visit.
    let shown = ui.data(|data| data.get_temp::<u64>(id));
    if !shown.is_some_and(|last| last + 1 >= frame) {
        ui.data_mut(|data| data.remove::<u64>(id));
        return;
    }
    ui.data_mut(|data| data.insert_temp(id, frame));
    let screen = ui.ctx().content_rect();
    let size = (screen.width().min(screen.height()) * 0.82).clamp(120.0, 512.0);
    let modal = egui::Modal::new(id.with("modal"))
        .frame(egui::Frame::NONE)
        .backdrop_color(Color32::from_black_alpha(205))
        .show(ui.ctx(), |ui| {
            let image = covers.show_high_resolution(ui, Some(url), Some(url), size, 10);
            ui.interact(image.rect, id.with("image"), Sense::click())
                .clicked()
        });
    if modal.inner || modal.should_close() {
        ui.data_mut(|data| data.remove::<u64>(id));
    }
}

fn kicker(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .small()
                .color(Color32::from_rgb(151, 181, 220)),
        )
        .selectable(false),
    );
}

fn backdrop(color: Option<Color32>) -> (Color32, Color32) {
    let Some(color) = color else {
        return (Color32::from_rgb(26, 49, 79), Color32::from_rgb(20, 29, 43));
    };
    let tint = |weight: f32| {
        let channel = |base, accent| {
            (f32::from(base) * (1.0 - weight) + f32::from(accent) * weight).round() as u8
        };
        Color32::from_rgb(
            channel(theme::BACKGROUND.r(), color.r()),
            channel(theme::BACKGROUND.g(), color.g()),
            channel(theme::BACKGROUND.b(), color.b()),
        )
    };
    (tint(0.23), tint(0.07))
}

pub(crate) fn background(painter: &egui::Painter, rect: egui::Rect, color: Option<Color32>) {
    let (left, right) = backdrop(color);
    theme::horizontal_gradient(painter, rect, 16.0, left, right);
}

fn fade(painter: &egui::Painter, rect: egui::Rect, color: Option<Color32>) {
    let tint = color.map_or(Color32::from_rgb(22, 36, 57), |color| {
        backdrop(Some(color)).0
    });
    theme::horizontal_gradient(
        painter,
        rect,
        16.0,
        Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 245),
        Color32::TRANSPARENT,
    );
}

fn controls(
    ui: &mut egui::Ui,
    language: Language,
    track_count: usize,
    loading: bool,
    playing: bool,
    follow: Option<FollowView>,
) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let label = if loading && track_count == 0 {
            language.text("Загружаем…", "Loading…")
        } else if playing {
            language.text("Пауза", "Pause")
        } else {
            language.text("Слушать", "Listen")
        };
        let icon = if playing { Icon::Pause } else { Icon::Play };
        let play = ui
            .scope(|ui| {
                ui.style_mut().visuals.widgets.inactive.bg_fill = theme::PRIMARY;
                ui.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
                ui.style_mut().visuals.widgets.hovered.bg_fill = theme::ACCENT;
                ui.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::NONE;
                icons::button(ui, icon, label, track_count > 0)
            })
            .inner;
        if play.clicked() {
            action = Some(Action::Play);
        }
        let shuffle_label = language.text("Перемешать", "Shuffle");
        let font = FontId::new(14.0, theme::semibold());
        let galley =
            ui.painter()
                .layout_no_wrap(shuffle_label.into(), font, ui.visuals().text_color());
        let full_width = galley.size().x + 50.0;
        // The heart and the copy-link button each take 36px plus spacing.
        let follow_room = if follow.is_some() { 92.0 } else { 46.0 };
        let width = if ui.available_width() >= full_width + follow_room {
            full_width
        } else {
            36.0
        };
        if ui.available_width() >= width + follow_room {
            let shuffle = ui
                .add_enabled_ui(track_count > 1, |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(width, 36.0), Sense::click());
                    let visual = ui.style().interact(&response);
                    ui.painter().rect(
                        rect,
                        8.0,
                        visual.bg_fill,
                        visual.bg_stroke,
                        egui::StrokeKind::Inside,
                    );
                    shuffle_icon(
                        ui.painter(),
                        egui::Rect::from_min_size(
                            rect.min + egui::vec2(if width == 36.0 { 9.0 } else { 12.0 }, 9.0),
                            egui::vec2(18.0, 18.0),
                        ),
                        visual.fg_stroke.color,
                    );
                    if width == full_width {
                        ui.painter().galley_with_override_text_color(
                            rect.min + egui::vec2(36.0, (36.0 - galley.size().y) * 0.5),
                            galley,
                            visual.fg_stroke.color,
                        );
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            shuffle_label,
                        )
                    });
                    response
                })
                .inner
                .on_hover_text(shuffle_label);
            if shuffle.clicked() {
                shuffle.request_focus();
                action = Some(Action::Shuffle);
            }
        }
        if let Some(follow) = follow {
            let label = if follow.saved {
                language.text("Убрать из библиотеки", "Remove from Your Library")
            } else {
                language.text("Добавить в библиотеку", "Add to Your Library")
            };
            let heart = ui
                .add_enabled_ui(!follow.busy, |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::click());
                    let visual = ui.style().interact(&response);
                    ui.painter().rect(
                        rect,
                        8.0,
                        visual.bg_fill,
                        visual.bg_stroke,
                        egui::StrokeKind::Inside,
                    );
                    icons::paint(
                        ui.painter(),
                        rect.shrink(8.0),
                        if follow.saved {
                            Icon::HeartFilled
                        } else {
                            Icon::Heart
                        },
                        if follow.saved {
                            theme::ACCENT
                        } else {
                            visual.fg_stroke.color
                        },
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            follow.saved,
                            label,
                        )
                    });
                    response
                })
                .inner
                .on_hover_text(label);
            if heart.clicked() {
                action = Some(Action::ToggleLibraryLike);
            }
        }
        let label = language.text("Копировать ссылку", "Copy link");
        let (rect, copy) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::click());
        let visual = ui.style().interact(&copy);
        ui.painter().rect(
            rect,
            8.0,
            visual.bg_fill,
            visual.bg_stroke,
            egui::StrokeKind::Inside,
        );
        icons::paint(
            ui.painter(),
            rect.shrink(8.0),
            Icon::CopyLink,
            visual.fg_stroke.color,
        );
        copy.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        if copy.on_hover_text(label).clicked() {
            action = Some(Action::CopyLink);
        }
    });
    action
}

fn shuffle_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    icons::paint(painter, rect, Icon::Shuffle, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_heart_is_right_of_shuffle_and_aligned_with_play_even_in_a_narrow_row() {
        for language in [Language::Russian, Language::English] {
            for width in [280.0, 340.0, 700.0] {
                let ctx = egui::Context::default();
                theme::install(&ctx);
                let mut tick = 0;
                let mut action = None;
                let mut draw = |events| {
                    tick += 1;
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 100.0),
                            )),
                            time: Some(f64::from(tick) * 0.1),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            action = controls(
                                ui,
                                language,
                                3,
                                false,
                                false,
                                Some(FollowView {
                                    saved: false,
                                    busy: false,
                                }),
                            );
                        },
                    );
                    output.textures_delta.clear();
                    output
                };
                draw(Vec::new());
                let output = draw(Vec::new());
                let mut buttons: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect) if (rect.rect.height() - 36.0).abs() < 0.5 => {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .collect();
                buttons.sort_by(|a, b| a.left().total_cmp(&b.left()));
                assert_eq!(buttons.len(), 4, "Four controls must fit at {width}px");
                assert_eq!(buttons[2].size(), egui::vec2(36.0, 36.0));
                assert!((buttons[2].center().y - buttons[0].center().y).abs() < 0.5);
                assert!((buttons[2].left() - buttons[1].right() - 10.0).abs() < 0.5);
                // The copy-link button follows the heart.
                assert_eq!(buttons[3].size(), egui::vec2(36.0, 36.0));
                assert!((buttons[3].left() - buttons[2].right() - 10.0).abs() < 0.5);
                assert!(buttons[3].right() <= width);
                let point = buttons[2].center();
                for pressed in [true, false] {
                    draw(vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]);
                }
                assert_eq!(action, Some(Action::ToggleLibraryLike));
            }
        }
    }

    #[test]
    fn playlist_description_keeps_paragraphs_literal_text_and_wraps_inside_both_gutters() {
        let description = "Первая строка описания.\n\nSecond paragraph with <b>literal markup</b> and a very long reference: https://example.test/abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz\nLast line.";
        for width in [180.0, 320.0, 760.0] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut result = None;
            for tick in 1..=3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1100.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        ..Default::default()
                    },
                    |ui| {
                        playlist_description(ui, description);
                    },
                );
                output.textures_delta.clear();
                result = Some(output);
            }
            let output = result.unwrap();
            let text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == description => Some(text),
                    _ => None,
                })
                .expect("Description is rendered as unchanged plain text");
            assert!(
                text.galley.rows.len() >= 5,
                "Paragraph and explicit line breaks must remain"
            );
            let bounds = text.visual_bounding_rect();
            assert!(bounds.left() >= 7.0, "Left gutter lost: {bounds:?}");
            assert!(
                bounds.right() <= width - 7.0,
                "Description escaped the right gutter at {width}: {bounds:?}"
            );
            assert_eq!(text.galley.job.sections[0].format.font_id.size, 14.0);
            assert_eq!(text.galley.job.sections[0].format.color, theme::MUTED);
            assert!(output.viewport_output.values().all(|viewport| {
                viewport
                    .commands
                    .iter()
                    .all(|command| matches!(command, egui::ViewportCommand::SetTheme(_)))
            }));
        }
    }

    #[test]
    fn missing_playlist_description_has_no_text_or_spacing() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for text in ["", " ", "\n\t\r\n"] {
                let before = ui.cursor();
                playlist_description(ui, text);
                assert_eq!(ui.cursor(), before);
            }
        });
        output.textures_delta.clear();
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::Shape::Text(_)))
        );
    }

    #[test]
    fn playlist_creator_is_below_the_title_and_name_and_chevron_open_the_exact_owner() {
        for language in [Language::Russian, Language::English] {
            for owner in [42, -77] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut covers = CoverCache::new();
                let creator = crate::web_audio::PlaylistCreator {
                    owner_id: owner,
                    name: "Cynthoni".into(),
                };
                let mut render = |tick, events| {
                    let mut action = None;
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(700.0, 360.0),
                            )),
                            time: Some(f64::from(tick) * 0.1),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            action = playlist_header(
                                ui,
                                &mut covers,
                                AlbumView {
                                    language,
                                    title: "Creator playlist",
                                    artists: &[],
                                    creator: Some(&creator),
                                    image_url: None,
                                    subtitle: "",
                                    track_count: 2,
                                    loading: false,
                                    playing: false,
                                    can_edit_playlist: false,
                                    follow: None,
                                },
                            );
                        },
                    );
                    output.textures_delta.clear();
                    (output, action)
                };
                let mut output = render(1, Vec::new()).0;
                for tick in 2..4 {
                    output = render(tick, Vec::new()).0;
                }
                let find = |label: &str| {
                    output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == label => {
                                Some(text.visual_bounding_rect())
                            }
                            _ => None,
                        })
                        .unwrap()
                };
                let title = find("Creator playlist");
                let name = find("Cynthoni");
                let arrow = find("›");
                assert!(name.top() > title.bottom());
                assert!(arrow.left() > name.right());
                assert!((arrow.center().y - name.center().y).abs() < 4.0);
                let creator_color = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == "Cynthoni" => {
                            Some(text.galley.job.sections[0].format.color)
                        }
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(creator_color, theme::MUTED);
                for (tick, target) in [(4, name.center()), (8, arrow.center())] {
                    render(tick, pointer(target, true));
                    let (_, action) = render(tick + 1, pointer(target, false));
                    assert_eq!(
                        action,
                        Some(Action::OpenCreator(crate::web_audio::PlaylistCreator {
                            owner_id: owner,
                            name: "Cynthoni".into()
                        }))
                    );
                }
            }
        }
    }

    #[test]
    fn a_playlist_without_verified_creator_does_not_display_artists_or_a_fake_creator_link() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let artist = ArtistRef {
            id: "unrelated-artist".into(),
            name: "Music artist is not the playlist creator".into(),
        };
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            playlist_header(
                ui,
                &mut covers,
                AlbumView {
                    language: Language::English,
                    title: "Playlist",
                    artists: std::slice::from_ref(&artist),
                    creator: None,
                    image_url: None,
                    subtitle: "",
                    track_count: 1,
                    loading: false,
                    playing: false,
                    can_edit_playlist: false,
                    follow: None,
                },
            );
        });
        output.textures_delta.clear();
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| matches!(&shape.shape,egui::Shape::Text(text)
            if text.galley.text()=="›" || text.galley.text()==artist.name))
        );
    }

    #[test]
    fn release_playlist_and_artist_images_open_full_size_and_close() {
        for (kind, point) in [
            ("album", egui::pos2(110.0, 110.0)),
            ("playlist", egui::pos2(110.0, 110.0)),
            ("artist", egui::pos2(840.0, 120.0)),
        ] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let mut tick = 0;
            let mut frame = |events: Vec<egui::Event>| {
                tick += 1;
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 700.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let view = AlbumView {
                            language: Language::Russian,
                            title: "Release",
                            artists: &[],
                            creator: None,
                            image_url: Some("https://images.example.test/cover.jpg"),
                            subtitle: "",
                            track_count: 1,
                            loading: false,
                            playing: false,
                            can_edit_playlist: false,
                            follow: None,
                        };
                        match kind {
                            "playlist" => {
                                playlist_header(ui, &mut covers, view);
                            }
                            "artist" => {
                                artist_header(
                                    ui,
                                    &mut covers,
                                    ArtistView {
                                        language: Language::Russian,
                                        name: "Artist",
                                        image_url: view.image_url,
                                        track_count: 1,
                                        loading: false,
                                        playing: false,
                                        follow: None,
                                    },
                                );
                            }
                            _ => {
                                album_header(ui, &mut covers, view);
                            }
                        }
                    },
                );
                output.textures_delta.clear();
                output
            };
            let dimmed = |output: &egui::FullOutput| {
                output.shapes.iter().any(|shape| {
                    // The backdrop fades in, so its alpha is still rising.
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.rect.width() >= 900.0
                            && rect.fill.a() > 0
                            && rect.fill.r() == 0
                            && rect.fill.g() == 0
                            && rect.fill.b() == 0)
                })
            };
            frame(Vec::new());
            frame(vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            frame(vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            let opened = frame(Vec::new());
            assert!(dimmed(&opened), "{kind}");
            frame(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            assert!(!dimmed(&frame(Vec::new())));
        }
    }

    #[test]
    fn playlist_header_has_its_own_kind_and_keeps_the_blue_play_action() {
        for language in [Language::Russian, Language::English] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 360.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    playlist_header(
                        ui,
                        &mut covers,
                        AlbumView {
                            language,
                            title: "Synthetic playlist",
                            artists: &[],
                            creator: None,
                            image_url: None,
                            subtitle: "3 tracks",
                            track_count: 3,
                            loading: false,
                            playing: false,
                            can_edit_playlist: false,
                            follow: None,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            let contains = |label: &str| {
                output.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text()==label))
            };
            assert!(contains(language.text("ПЛЕЙЛИСТ", "PLAYLIST")));
            assert!(!contains(language.text("АЛЬБОМ", "ALBUM")));
            assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Rect(rect) if rect.fill==theme::PRIMARY && rect.rect.height()>30.0 && rect.rect.height()<45.0 && rect.rect.width()>70.0)));
        }
    }

    #[test]
    fn playlist_edit_pencil_is_inside_the_hero_and_never_appears_on_album_or_readonly_pages() {
        for width in [330.0, 700.0] {
            for (playlist, can_edit) in [(true, true), (true, false), (false, true)] {
                let ctx = egui::Context::default();
                theme::install(&ctx);
                let mut covers = CoverCache::new();
                let mut tick = 0;
                let mut action = None;
                let mut render = |events| {
                    tick += 1;
                    let mut output = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 380.0))),
                        time: Some(f64::from(tick) * 0.1), events, ..Default::default()
                    }, |ui| {
                        let view = AlbumView { language: Language::English, title: "A long playlist title that still must not cover its edit control", artists: &[], creator: None, image_url: None, subtitle: "", track_count: 2, loading: false, playing: false, can_edit_playlist: can_edit, follow: None };
                        action = if playlist { playlist_header(ui, &mut covers, view) } else { album_header(ui, &mut covers, view) };
                    });
                    output.textures_delta.clear();
                    output
                };
                render(Vec::new());
                render(Vec::new());
                render(Vec::new());
                let point = egui::pos2(width - 35.0, 35.0);
                let hovered = render(vec![egui::Event::PointerMoved(point)]);
                let edit_rect = hovered.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == theme::SELECTED
                            && rect.rect.size() == egui::vec2(30.0, 30.0)
                            && rect.rect.contains(point) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                });
                assert_eq!(edit_rect.is_some(), playlist && can_edit);
                if let Some(rect) = edit_rect {
                    assert!(
                        (rect.top() - 20.0).abs() < 0.5
                            && (width - rect.right() - 20.0).abs() < 0.5
                    );
                }
                render(pointer(point, true));
                render(pointer(point, false));
                assert_eq!(
                    action,
                    (playlist && can_edit).then_some(Action::EditPlaylist)
                );
            }
        }
    }

    #[test]
    fn artwork_tints_both_headers_but_play_buttons_stay_blue() {
        for color in [
            Color32::from_rgb(220, 30, 30),
            Color32::from_rgb(30, 220, 30),
            Color32::from_gray(150),
        ] {
            for artist in [false, true] {
                let context = egui::Context::default();
                theme::install(&context);
                let mut covers = CoverCache::new();
                let url = "https://images.example.test/synthetic-header.jpg";
                covers.seed_image_for_test(
                    &context,
                    url,
                    180.0,
                    egui::ColorImage::new([32, 32], vec![color; 32 * 32]),
                );
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(720.0, 420.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        if artist {
                            artist_header(
                                ui,
                                &mut covers,
                                ArtistView {
                                    follow: None,
                                    language: Language::English,
                                    name: "Artist",
                                    image_url: Some(url),
                                    track_count: 3,
                                    loading: false,
                                    playing: false,
                                },
                            );
                        } else {
                            album_header(
                                ui,
                                &mut covers,
                                AlbumView {
                                    language: Language::English,
                                    title: "Album",
                                    artists: &[],
                                    creator: None,
                                    image_url: Some(url),
                                    subtitle: "2026",
                                    track_count: 3,
                                    loading: false,
                                    playing: false,
                                    can_edit_playlist: false,
                                    follow: None,
                                },
                            );
                        }
                    },
                );
                output.textures_delta.clear();
                let backdrop = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Mesh(mesh)
                            if mesh.calc_bounds().width() > 600.0
                                && mesh.calc_bounds().height() > 220.0
                                && mesh.vertices.len() < 100 =>
                        {
                            Some(
                                mesh.vertices
                                    .iter()
                                    .filter(|v| v.color.a() == 255)
                                    .max_by_key(|v| v.color.r().max(v.color.g()).max(v.color.b()))
                                    .unwrap()
                                    .color,
                            )
                        }
                        _ => None,
                    })
                    .unwrap();
                if color.r() > color.g() {
                    assert!(backdrop.r() > backdrop.g() + 20);
                } else if color.g() > color.r() {
                    assert!(backdrop.g() > backdrop.r() + 20);
                } else {
                    assert!(backdrop.b().abs_diff(backdrop.r()) <= 5);
                }
                assert!(
                    backdrop.r().max(backdrop.g()).max(backdrop.b()) <= 75,
                    "Header must stay dark enough for its text"
                );
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Rect(rect) if rect.fill==theme::PRIMARY && rect.rect.height()>30.0 && rect.rect.height()<45.0 && rect.rect.width()>70.0)));
            }
        }
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

    fn links_frame(
        context: &egui::Context,
        artists: &[ArtistRef],
        fallback: &str,
        width: f32,
        tick: u32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<ArtistRef>, f32) {
        let mut action = None;
        let mut height = 0.0;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 100.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| {
                let top = ui.cursor().top();
                action = artist_links(ui, artists, fallback);
                height = ui.cursor().top() - top;
            },
        );
        output.textures_delta.clear();
        (output, action, height)
    }

    #[test]
    fn each_canonical_artist_is_an_independent_target_and_names_are_not_split() {
        let artists = [
            ArtistRef {
                id: "artist-1".into(),
                name: "Name, With Comma".into(),
            },
            ArtistRef {
                id: "artist-2".into(),
                name: "Second & Artist".into(),
            },
        ];
        let context = egui::Context::default();
        theme::install(&context);
        let mut output = links_frame(&context, &artists, "Raw display", 600.0, 1, vec![]).0;
        for tick in 2..4 {
            output = links_frame(&context, &artists, "Raw display", 600.0, tick, vec![]).0;
        }
        for (index, artist) in artists.iter().enumerate() {
            let target = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == artist.name => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .expect("Artist name must be a separate label");
            let tick = 4 + index as u32 * 2;
            links_frame(
                &context,
                &artists,
                "Raw display",
                600.0,
                tick,
                pointer(target, true),
            );
            let (_, action, _) = links_frame(
                &context,
                &artists,
                "Raw display",
                600.0,
                tick + 1,
                pointer(target, false),
            );
            assert_eq!(action, Some(artist.clone()));
        }
    }

    #[test]
    fn larger_artist_caption_preserves_each_link_and_fits_its_measured_height() {
        let artists = [
            ArtistRef {
                id: "first".into(),
                name: "First artist".into(),
            },
            ArtistRef {
                id: "second".into(),
                name: "Second & Artist".into(),
            },
        ];
        let context = egui::Context::default();
        theme::install(&context);
        let font = FontId::new(14.0, egui::FontFamily::Proportional);
        let mut tick = 0;
        let mut render = |events| {
            tick += 1;
            let mut action = None;
            let mut measured = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 100.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let top = ui.cursor().top();
                    action = artist_links_with_font(ui, &artists, "Raw display", font.clone());
                    measured = Some((
                        top,
                        ui.cursor().top() - top,
                        ui.fonts_mut(|fonts| fonts.row_height(&font)).max(16.0),
                    ));
                },
            );
            output.textures_delta.clear();
            (output, action, measured.unwrap())
        };
        let mut output = render(Vec::new()).0;
        for _ in 0..3 {
            output = render(Vec::new()).0;
        }
        for artist in &artists {
            let label = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == artist.name => Some(text),
                    _ => None,
                })
                .expect("Each canonical artist keeps a separate larger label");
            assert!(
                label
                    .galley
                    .job
                    .sections
                    .iter()
                    .all(|section| section.format.font_id.size == 14.0)
            );
            let target = label.visual_bounding_rect().center();
            render(pointer(target, true));
            let (next, action, (top, height, line_height)) = render(pointer(target, false));
            assert_eq!(action, Some(artist.clone()));
            assert_eq!(height, line_height + 8.0);
            for shape in &next.shapes {
                if let egui::Shape::Text(text) = &shape.shape
                    && artists
                        .iter()
                        .any(|artist| artist.name == text.galley.text())
                {
                    let bounds = text.visual_bounding_rect();
                    assert!(
                        bounds.top() >= top - 0.5 && bounds.bottom() <= top + line_height + 0.5
                    );
                }
            }
            output = next;
        }
    }

    #[test]
    fn narrow_artist_caption_stays_one_line_without_changing_row_height() {
        let artists = [ArtistRef {
            id: "long-name".into(),
            name: "Очень длинное имя исполнителя / A very long artist name ".repeat(6),
        }];
        let context = egui::Context::default();
        theme::install(&context);
        for width in [28.0, 90.0, 270.0] {
            let (_, _, height) = links_frame(&context, &artists, "", width, 1, vec![]);
            assert_eq!(height, 24.0, "16px caption plus ordinary 8px item spacing");
        }
    }

    #[test]
    fn legacy_artist_fallback_keeps_the_entire_display_name() {
        let context = egui::Context::default();
        theme::install(&context);
        let fallback = "Artist, With Comma & Ampersand";
        let mut output = links_frame(&context, &[], fallback, 600.0, 1, vec![]).0;
        for tick in 2..4 {
            output = links_frame(&context, &[], fallback, 600.0, tick, vec![]).0;
        }
        let target = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == fallback => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap();
        links_frame(&context, &[], fallback, 600.0, 4, pointer(target, true));
        let (_, action, _) = links_frame(&context, &[], fallback, 600.0, 5, pointer(target, false));
        assert_eq!(
            action,
            Some(ArtistRef {
                id: String::new(),
                name: fallback.into()
            })
        );
    }

    #[test]
    fn detail_headers_keep_long_titles_and_controls_within_their_surfaces() {
        let context = egui::Context::default();
        theme::install(&context);
        let artists = [ArtistRef {
            id: "id".into(),
            name: "Artist".into(),
        }];
        let mut covers = CoverCache::new();
        for language in [Language::Russian, Language::English] {
            for width in [320.0, 520.0, 940.0] {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1200.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        artist_header(
                            ui,
                            &mut covers,
                            ArtistView {
                                follow: None,
                                language,
                                name: "An artist with a very long displayed name "
                                    .repeat(4)
                                    .as_str(),
                                image_url: None,
                                track_count: 10,
                                loading: false,
                                playing: false,
                            },
                        );
                        album_header(
                            ui,
                            &mut covers,
                            AlbumView {
                                language,
                                title: "An album with a very long title ".repeat(4).as_str(),
                                artists: &artists,
                                creator: None,
                                image_url: None,
                                subtitle: "2026 · 10 tracks",
                                track_count: 10,
                                loading: false,
                                playing: false,
                                can_edit_playlist: false,
                                follow: None,
                            },
                        );
                    },
                );
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        let bounds = text.visual_bounding_rect();
                        assert!(
                            bounds.left() >= -1.0 && bounds.right() <= width + 1.0,
                            "Text exceeds width {width}: {:?}: {bounds:?}",
                            text.galley.text()
                        );
                        assert!(
                            shape.clip_rect.contains_rect(bounds),
                            "Text clipped at width {width}: {:?}: {bounds:?} in {:?}",
                            text.galley.text(),
                            shape.clip_rect
                        );
                    }
                }
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn artist_latest_stays_one_row_and_discography_tabs_filter_their_cards() {
        let release = |id, title: String, kind| Release {
            album: AlbumRef {
                id,
                owner_id: -42,
                title,
                access_key: None,
                cover_url: None,
            },
            kind,
            year: Some(2026),
        };
        let latest: Vec<_> = (1..=12)
            .map(|id| release(id, format!("Latest {id}"), ReleaseKind::Single))
            .collect();
        let discography = vec![
            release(20, "Album only".into(), ReleaseKind::Album),
            release(21, "Single only".into(), ReleaseKind::Single),
            release(22, "EP only".into(), ReleaseKind::Ep),
        ];
        for width in [600.0, 1040.0] {
            let context = egui::Context::default();
            theme::install(&context);
            let mut covers = CoverCache::new();
            let mut frame = |tick, events| {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1000.0),
                        )),
                        time: Some(f64::from(tick) * 0.1),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        artist_releases(
                            ui,
                            &mut covers,
                            ReleasesView {
                                language: Language::English,
                                artist_id: "synthetic",
                                latest: &latest,
                                discography: &discography,
                                incomplete: false,
                                loading: false,
                                pending: false,
                                playback: Playback::default(),
                                related: &[],
                            },
                        );
                    },
                );
                output.textures_delta.clear();
                output
            };
            let find = |output: &egui::FullOutput, title: &str| {
                output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == title => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
            };
            frame(1, vec![]);
            let output = frame(2, vec![]);
            assert!(find(&output, "Latest releases").is_some());
            let top = find(&output, "Latest 1").unwrap().top();
            for id in 1..=release_columns(width) {
                assert!((find(&output, &format!("Latest {id}")).unwrap().top() - top).abs() < 1.0);
            }
            assert!(find(&output, &format!("Latest {}", release_columns(width) + 1)).is_none());
            assert!(find(&output, "Album only").is_some());
            assert!(find(&output, "Single only").is_none());
            let tab = find(&output, "Singles & EPs").unwrap().center();
            frame(3, pointer(tab, true));
            frame(4, pointer(tab, false));
            let output = frame(5, vec![]);
            assert!(find(&output, "Album only").is_none());
            assert!(find(&output, "Single only").is_some());
            assert!(find(&output, "EP only").is_some());
        }
    }

    #[test]
    fn discography_shows_two_rows_then_expands_with_hover_play_and_related_artists() {
        let release = |id: i64| Release {
            album: AlbumRef {
                id,
                owner_id: -42,
                title: format!("Album {id}"),
                access_key: None,
                cover_url: None,
            },
            kind: ReleaseKind::Album,
            year: Some(2026),
        };
        let discography: Vec<_> = (1..=12).map(release).collect();
        let related = [crate::web_audio::RelatedArtist {
            artist: ArtistRef {
                id: "related_one".into(),
                name: "Related One".into(),
            },
            image_url: None,
        }];
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        // 600 px fits three columns: two rows are six albums.
        let mut frame = |tick: u32, events: Vec<egui::Event>| {
            let mut action = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 3000.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = artist_releases(
                        ui,
                        &mut covers,
                        ReleasesView {
                            language: Language::Russian,
                            artist_id: "synthetic",
                            latest: &[],
                            discography: &discography,
                            incomplete: false,
                            loading: false,
                            pending: false,
                            playback: Playback::default(),
                            related: &related,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            (output, action)
        };
        let text = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
        };
        let blue = |output: &egui::FullOutput| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Circle(circle) if circle.fill == theme::ACCENT => Some(circle.center),
                _ => None,
            })
        };
        let (output, _) = frame(1, Vec::new());
        assert!(text(&output, "Album 6").is_some());
        assert!(text(&output, "Album 7").is_none(), "only two rows at first");
        assert!(blue(&output).is_none(), "play buttons wait for hover");

        let card = text(&output, "Album 2").unwrap();
        let (output, _) = frame(2, vec![egui::Event::PointerMoved(card)]);
        let play = blue(&output).expect("hover shows the blue play button");
        frame(3, pointer(play, true));
        let (_, action) = frame(4, pointer(play, false));
        assert_eq!(action, Some(Action::PlayAlbum(release(2).album)));

        let (output, _) = frame(5, Vec::new());
        let rect_of = |label: &str| {
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .next_back()
                .unwrap()
        };
        let link = rect_of("Показать все");
        let cards_bottom = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == theme::SURFACE && rect.rect.bottom() <= link.top() =>
                {
                    Some(rect.rect.bottom())
                }
                _ => None,
            })
            .fold(0.0, f32::max);
        let heading = rect_of("Вам могут понравиться");
        let above = link.top() - cards_bottom;
        let below = heading.top() - link.bottom();
        assert!(
            (above - below).abs() <= 1.0 && (14.0..=20.0).contains(&above),
            "Show all spacing: {above} above, {below} below"
        );
        let show_all = text(&output, "Показать все").unwrap();
        frame(6, pointer(show_all, true));
        frame(7, pointer(show_all, false));
        let (output, _) = frame(8, Vec::new());
        assert!(text(&output, "Album 12").is_some(), "show all expands");
        assert!(text(&output, "Скрыть").is_some());

        let artist = text(&output, "Related One").unwrap();
        frame(9, pointer(artist, true));
        let (_, action) = frame(10, pointer(artist, false));
        assert_eq!(action, Some(Action::OpenArtist(related[0].artist.clone())));
    }

    #[test]
    fn an_empty_tab_note_has_equal_space_above_and_below() {
        let single = Release {
            album: AlbumRef {
                id: 1,
                owner_id: -42,
                title: "Only single".into(),
                access_key: None,
                cover_url: None,
            },
            kind: ReleaseKind::Single,
            year: Some(2026),
        };
        let related = [crate::web_audio::RelatedArtist {
            artist: ArtistRef {
                id: "related_one".into(),
                name: "Related One".into(),
            },
            image_url: None,
        }];
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 1200.0),
                )),
                ..Default::default()
            },
            |ui| {
                artist_releases(
                    ui,
                    &mut covers,
                    ReleasesView {
                        language: Language::English,
                        artist_id: "synthetic-empty",
                        latest: &[],
                        discography: std::slice::from_ref(&single),
                        incomplete: false,
                        loading: false,
                        pending: false,
                        playback: Playback::default(),
                        related: &related,
                    },
                );
            },
        );
        output.textures_delta.clear();
        let text_rect = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        let tab_text = text_rect("Albums");
        let tab = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.rect.contains(tab_text.center()) => Some(rect.rect),
                _ => None,
            })
            .next()
            .expect("selected tab background");
        let note = text_rect("No albums yet");
        let heading = text_rect("You may also like");
        let above = note.top() - tab.bottom();
        let below = heading.top() - note.bottom();
        assert!(
            (above - below).abs() <= 1.0,
            "empty note spacing: {above} above, {below} below"
        );
    }

    #[test]
    fn release_card_opens_the_supplied_album_identity() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut covers = CoverCache::new();
        let album = AlbumRef {
            id: -41,
            owner_id: 17,
            title: "A supplied release".into(),
            access_key: Some("opaque-access".into()),
            cover_url: None,
        };
        let mut frame = |tick: u32, events: Vec<egui::Event>| {
            let mut action = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    action = releases(
                        ui,
                        &mut covers,
                        Language::Russian,
                        &[Release {
                            album: album.clone(),
                            kind: ReleaseKind::Album,
                            year: None,
                        }],
                        Playback::default(),
                    );
                },
            );
            output.textures_delta.clear();
            (output, action)
        };
        let mut output = frame(1, vec![]).0;
        for tick in 2..4 {
            output = frame(tick, vec![]).0;
        }
        let target = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == album.title => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap();
        frame(4, pointer(target, true));
        let (_, action) = frame(5, pointer(target, false));
        assert_eq!(action, Some(Action::OpenAlbum(album)));
    }
}
