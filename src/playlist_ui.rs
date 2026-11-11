//! Sidebar playlist presentation. The application owns loading, navigation and
//! playback; this module returns intents without touching account data.
use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense};
use std::hash::{Hash, Hasher};

use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    model::{AlbumRef, ArtistRef},
    theme,
    web_audio::{CollectionKind, FollowedArtist},
};

#[derive(Clone)]
pub struct PlaylistItem {
    pub reference: AlbumRef,
    pub kind: CollectionKind,
    pub subtitle: String,
    pub can_edit: bool,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open(AlbumRef),
    OpenAlbum(AlbumRef),
    OpenArtist(ArtistRef),
    UnfollowArtist(ArtistRef),
    Refresh,
    Create,
    Play(AlbumRef),
    Shuffle(AlbumRef),
    TogglePin(AlbumRef),
    Edit(AlbumRef),
    Delete(AlbumRef),
    Mix(AlbumRef),
    /// The reference and whether VK calls it an album or single.
    CopyLink(AlbumRef, bool),
    Reorder {
        playlist: AlbumRef,
        before: Option<AlbumRef>,
    },
    /// A track carried from a list was let go over this own playlist.
    AddCarriedTrack(AlbumRef),
}

/// Library tags under the header, like VK's own sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LibraryFilter {
    Playlists,
    Albums,
    Artists,
}

#[derive(Default)]
pub struct SearchState {
    open: bool,
    query: String,
    filter: Option<LibraryFilter>,
    /// Within playlists: only the ones this account created.
    own: bool,
    stamp: Option<(String, u64)>,
    items: Vec<PlaylistItem>,
    artists: Vec<FollowedArtist>,
    input_id: Option<egui::Id>,
}

impl SearchState {
    fn filter(&mut self, items: &[PlaylistItem], artists: &[FollowedArtist]) {
        let query = self.query.trim().to_lowercase();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for item in items {
            (
                item.reference.owner_id,
                item.reference.id,
                &item.reference.title,
                &item.subtitle,
                &item.reference.cover_url,
                item.kind,
                item.pinned,
                item.can_edit,
            )
                .hash(&mut hasher);
        }
        for item in artists {
            (&item.artist.id, &item.artist.name, &item.image_url).hash(&mut hasher);
        }
        (self.filter, self.own).hash(&mut hasher);
        let stamp = (query.clone(), hasher.finish());
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let (filter, own) = (self.filter, self.own);
        self.items = items
            .iter()
            .filter(|item| match filter {
                None => true,
                Some(LibraryFilter::Playlists) => {
                    item.kind == CollectionKind::Playlist && (!own || item.can_edit)
                }
                Some(LibraryFilter::Albums) => item.kind == CollectionKind::Album,
                Some(LibraryFilter::Artists) => false,
            })
            .filter(|item| {
                item.reference.title.to_lowercase().contains(&query)
                    || item.subtitle.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        self.artists = artists
            .iter()
            .filter(|_| matches!(filter, None | Some(LibraryFilter::Artists)))
            .filter(|item| item.artist.name.to_lowercase().contains(&query))
            .cloned()
            .collect();
        self.stamp = Some(stamp);
    }
}

pub struct SidebarView<'a> {
    pub language: Language,
    pub items: &'a [PlaylistItem],
    pub artists: &'a [FollowedArtist],
    pub selected: Option<(i64, i64)>,
    pub selected_artist: Option<&'a str>,
    pub loading: bool,
    pub error: Option<&'a str>,
}

pub fn show_sidebar(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    search: &mut SearchState,
    view: SidebarView<'_>,
) -> Option<Action> {
    let SidebarView {
        language,
        items,
        artists,
        selected,
        selected_artist,
        loading,
        error,
    } = view;
    let mut action = None;
    let mut focus_search = false;
    let drag_id = ui.id().with("sidebar-playlist-drag");
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(4.0);
            icons::show(ui, Icon::Library, theme::MUTED, 16.0);
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    RichText::new(language.text("Библиотека", "Library"))
                        .family(theme::semibold())
                        .color(theme::MUTED),
                )
                .truncate()
                .selectable(false),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if header_button(
                    ui,
                    Icon::Refresh,
                    language.text("Обновить библиотеку", "Refresh library"),
                    !loading,
                )
                .clicked()
                {
                    action = Some(Action::Refresh);
                }
                if header_button(
                    ui,
                    Icon::Plus,
                    language.text("Создать плейлист", "Create playlist"),
                    true,
                )
                .clicked()
                {
                    action = Some(Action::Create);
                }
                if header_button(
                    ui,
                    Icon::Search,
                    language.text("Поиск в библиотеке", "Search in library"),
                    true,
                )
                .clicked()
                {
                    search.open = !search.open;
                    if search.open {
                        focus_search = true;
                    } else {
                        search.query.clear();
                        if let Some(id) = search.input_id {
                            ui.ctx().memory_mut(|memory| memory.surrender_focus(id));
                        }
                    }
                }
            });
        });
    });
    ui.add_space(4.0);
    if search.open {
        egui::Frame::new()
            .fill(Color32::from_rgb(37, 42, 50))
            .corner_radius(8)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.spacing_mut().interact_size.y = 22.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    icons::show(ui, Icon::Search, theme::MUTED, 16.0);
                    let input = ui.add(
                        egui::TextEdit::singleline(&mut search.query)
                            .id_salt("library-search-input")
                            .frame(egui::Frame::NONE)
                            .desired_width(ui.available_width())
                            .char_limit(256)
                            .hint_text(language.text("Поиск в библиотеке", "Search in library")),
                    );
                    search.input_id = Some(input.id);
                    if focus_search {
                        input.request_focus();
                    }
                });
            });
        ui.add_space(4.0);
    }
    filter_chips(ui, language, search, items, artists);
    let searching = search.open && !search.query.trim().is_empty();
    let filtering = searching || search.filter.is_some();
    let query = if searching {
        search.query.trim().to_lowercase()
    } else {
        String::new()
    };
    let (items, artists) = if filtering {
        if !searching {
            search.query.clear();
        }
        search.filter(items, artists);
        ui.ctx().data_mut(|data| data.remove::<DragMemory>(drag_id));
        (search.items.as_slice(), search.artists.as_slice())
    } else {
        (items, artists)
    };
    if let Some(error) = error {
        ui.add(
            egui::Label::new(
                RichText::new(error)
                    .small()
                    .color(Color32::from_rgb(255, 158, 153)),
            )
            .wrap()
            .selectable(false),
        );
        ui.add_space(4.0);
    }
    if items.is_empty() && artists.is_empty() {
        ui.ctx().data_mut(|data| data.remove::<DragMemory>(drag_id));
        if filtering {
            ui.label(
                RichText::new(language.text("Ничего не найдено", "No results"))
                    .small()
                    .color(theme::MUTED),
            );
        } else if loading {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(
                    RichText::new(language.text("Загружаем библиотеку…", "Loading library…"))
                        .small()
                        .color(theme::MUTED),
                );
            });
        } else if error.is_none() {
            ui.add(
                egui::Label::new(
                    RichText::new(language.text("Библиотека пока пуста", "Your library is empty"))
                        .small()
                        .color(theme::MUTED),
                )
                .wrap()
                .selectable(false),
            );
        }
        return action;
    }
    let height = ui.available_height().max(0.0);
    if height < 1.0 {
        return action;
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        // Keep the whole row width. Reveal the overlay handle only when the
        // pointer reaches it, rather than reserving an empty scrollbar gutter.
        ui.spacing_mut().scroll = egui::style::ScrollStyle {
            floating: true,
            floating_width: 8.0,
            floating_allocated_width: 0.0,
            dormant_handle_opacity: 0.0,
            active_handle_opacity: 0.0,
            interact_handle_opacity: 1.0,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            ..ui.spacing().scroll
        };
        theme::scroll_rows(
            ui,
            egui::ScrollArea::vertical()
                .id_salt(("sidebar-playlists", &query, search.filter, search.own))
                .max_height(height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false]),
            48.0,
            items.len() + artists.len(),
            |ui, range| {
                if let Some(intent) = playlist_rows(
                    ui,
                    covers,
                    RowView {
                        language,
                        items,
                        selected,
                        drag_id: if filtering {
                            drag_id.with(("query", &query, search.filter, search.own))
                        } else {
                            drag_id
                        },
                        allow_reorder: !filtering,
                    },
                    range.clone(),
                ) {
                    action = Some(intent);
                }
                if let Some(intent) = artist_rows(
                    ui,
                    covers,
                    language,
                    artists,
                    items.len(),
                    selected_artist,
                    range,
                ) {
                    action = Some(intent);
                }
            },
        );
    });
    action
}

const ROW_HEIGHT: f32 = 48.0;
const ROW_STRIDE: f32 = 52.0;
const REORDER_SECONDS: f32 = 0.16;
type PlaylistKey = (i64, i64);

fn drag_top(pointer_y: f32, grab_y: f32, viewport: egui::Rect) -> f32 {
    (pointer_y - grab_y).clamp(
        viewport.top(),
        (viewport.bottom() - ROW_HEIGHT).max(viewport.top()),
    )
}

#[derive(Clone, Default)]
struct DragMemory {
    active: Option<PlaylistDrag>,
    suppress_release: bool,
}

#[derive(Clone)]
struct PlaylistDrag {
    key: PlaylistKey,
    from: usize,
    to: usize,
    grab_y: f32,
    pointer: egui::Pos2,
}

struct RowView<'a> {
    language: Language,
    items: &'a [PlaylistItem],
    selected: Option<PlaylistKey>,
    drag_id: egui::Id,
    allow_reorder: bool,
}

#[derive(Default)]
struct RowPaint {
    selected: bool,
    hovered: bool,
    focused: bool,
    ghost: bool,
}

fn playlist_key(item: &PlaylistItem) -> PlaylistKey {
    (item.reference.owner_id, item.reference.id)
}

fn visual_slot(index: usize, drag: Option<&PlaylistDrag>) -> usize {
    let Some(drag) = drag else { return index };
    if index == drag.from {
        drag.to
    } else if drag.from < drag.to && index > drag.from && index <= drag.to {
        index - 1
    } else if drag.to < drag.from && index >= drag.to && index < drag.from {
        index + 1
    } else {
        index
    }
}

fn update_drag_target(
    drag: &mut PlaylistDrag,
    items: &[PlaylistItem],
    pointer_y: f32,
    origin_y: f32,
) {
    let pinned = items[drag.from].pinned;
    let group: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.pinned == pinned)
        .map(|(index, _)| index)
        .collect();
    let first = *group.first().unwrap_or(&drag.from);
    let last = *group.last().unwrap_or(&drag.from);
    let slot = ((pointer_y - drag.grab_y - origin_y) / ROW_STRIDE)
        .round()
        .max(0.0) as usize;
    drag.to = slot.clamp(first, last);
}

fn reorder_intent(items: &[PlaylistItem], drag: &PlaylistDrag) -> Option<Action> {
    if drag.from == drag.to {
        return None;
    }
    let source = &items[drag.from];
    let mut order: Vec<_> = (0..items.len())
        .filter(|index| *index != drag.from)
        .collect();
    order.insert(drag.to, drag.from);
    let before = order
        .get(drag.to + 1)
        .map(|index| &items[*index])
        .filter(|item| item.pinned == source.pinned)
        .map(|item| item.reference.clone());
    Some(Action::Reorder {
        playlist: source.reference.clone(),
        before,
    })
}

fn playlist_rows(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    view: RowView<'_>,
    range: std::ops::Range<usize>,
) -> Option<Action> {
    let ctx = ui.ctx().clone();
    let mut memory = ctx
        .data_mut(|data| data.get_temp::<DragMemory>(view.drag_id))
        .unwrap_or_default();
    let was_dragging = memory.active.is_some() || memory.suppress_release;
    let origin_y = ui.max_rect().top() - range.start as f32 * ROW_STRIDE;
    let left = ui.max_rect().left();
    let width = ui.available_width();
    let clip = ui.clip_rect().intersect(egui::Rect::from_min_max(
        egui::pos2(left, ui.clip_rect().top()),
        egui::pos2(left + width, ui.clip_rect().bottom()),
    ));
    let pointer = ui
        .input(|input| input.pointer.interact_pos())
        .or_else(|| memory.active.as_ref().map(|drag| drag.pointer));
    let inside_column =
        pointer.is_some_and(|point| point.x >= clip.left() - 8.0 && point.x <= clip.right() + 8.0);
    let (released, held) = ui.input(|input| {
        (
            input.pointer.button_released(egui::PointerButton::Primary),
            input.pointer.primary_down(),
        )
    });
    let escape = memory.active.is_some()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut action = None;
    let mut settled_drag = None;
    if let Some(drag) = &mut memory.active {
        if let Some(from) = view
            .items
            .iter()
            .position(|item| playlist_key(item) == drag.key)
        {
            drag.from = from;
            if let Some(pointer) = pointer {
                drag.pointer = pointer;
                let top = drag_top(pointer.y, drag.grab_y, clip);
                let target_y = top + drag.grab_y;
                update_drag_target(drag, view.items, target_y, origin_y);
                // Keep the source animation at the pointer while dragging so
                // release smoothly settles it into its final slot.
                ctx.animate_value_with_time(
                    view.drag_id.with(("row-y", drag.key)),
                    top - origin_y,
                    0.0,
                );
            }
        } else {
            memory.active = None;
        }
    }
    if (escape || !held)
        && let Some(drag) = memory.active.take()
        && released
        && !escape
        && inside_column
    {
        action = reorder_intent(view.items, &drag);
        if action.is_some() {
            settled_drag = Some(drag);
        }
    }
    let mut responses = Vec::new();
    // One overscan row covers neighbours sliding into view at either edge.
    for index in range.start.saturating_sub(1)..(range.end + 1).min(view.items.len()) {
        let item = &view.items[index];
        let key = playlist_key(item);
        let slot = visual_slot(index, memory.active.as_ref().or(settled_drag.as_ref()));
        let y = ctx.animate_value_with_time(
            view.drag_id.with(("row-y", key)),
            slot as f32 * ROW_STRIDE,
            REORDER_SECONDS,
        );
        let rect = egui::Rect::from_min_size(
            egui::pos2(left, origin_y + y),
            egui::vec2(width, ROW_HEIGHT),
        );
        let response = ui.interact(
            rect,
            view.drag_id.with(("playlist-row", key)),
            if view.allow_reorder {
                Sense::click_and_drag()
            } else {
                Sense::click()
            },
        );
        if response.drag_started_by(egui::PointerButton::Primary)
            && !escape
            && !memory.suppress_release
        {
            let grab_y = ui
                .input(|input| input.pointer.press_origin())
                .map_or(ROW_HEIGHT * 0.5, |point| {
                    (point.y - rect.top()).clamp(0.0, ROW_HEIGHT)
                });
            let mut drag = PlaylistDrag {
                key,
                from: index,
                to: index,
                grab_y,
                pointer: pointer.unwrap_or(rect.center()),
            };
            if let Some(pointer) = pointer {
                let top = drag_top(pointer.y, drag.grab_y, clip);
                let target_y = top + drag.grab_y;
                update_drag_target(&mut drag, view.items, target_y, origin_y);
            }
            memory.active = Some(drag);
            memory.suppress_release = true;
        }
        responses.push((index, rect, response));
    }
    let dragging = memory.active.is_some();
    for (index, rect, response) in responses {
        let item = &view.items[index];
        let key = playlist_key(item);
        // A track carried from a list goes into the account's own playlists.
        let drop = if item.can_edit && item.kind != CollectionKind::Album {
            crate::track_drag::target(ui, rect)
        } else {
            crate::track_drag::Target::None
        };
        match drop {
            crate::track_drag::Target::Hovered => {
                crate::track_drag::paint_target(ui.ctx(), rect, 8.0);
            }
            crate::track_drag::Target::Dropped => {
                action = Some(Action::AddCarriedTrack(item.reference.clone()));
            }
            crate::track_drag::Target::None => {}
        }
        if memory.active.as_ref().is_none_or(|drag| drag.key != key) {
            paint_playlist_row(
                ui,
                covers,
                view.language,
                item,
                rect,
                RowPaint {
                    selected: view.selected == Some(key),
                    hovered: (response.hovered() && !dragging)
                        || drop == crate::track_drag::Target::Hovered,
                    focused: response.has_focus() && !dragging,
                    ghost: false,
                },
            );
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                view.selected == Some(key),
                &item.reference.title,
            )
        });
        if !was_dragging && !dragging && response.clicked() {
            response.request_focus();
            action = Some(if item.kind == CollectionKind::Album {
                Action::OpenAlbum(item.reference.clone())
            } else {
                Action::Open(item.reference.clone())
            });
        }
        if !was_dragging
            && !dragging
            && let Some(popup) = egui::Popup::context_menu(&response)
                .id(response.id.with(("playlist-context", key)))
                .show(|ui| playlist_menu(ui, view.language, item))
            && let Some(intent) = popup.inner
        {
            action = Some(intent);
        }
        if !dragging {
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(&item.reference.title);
        }
    }
    if let Some(drag) = &memory.active
        && let Some(pointer) = pointer
    {
        let item = &view.items[drag.from];
        let target = origin_y + drag.to as f32 * ROW_STRIDE;
        let marker = egui::Rect::from_min_size(
            egui::pos2(left + 6.0, target - 2.0),
            egui::vec2((width - 12.0).max(0.0), 2.0),
        );
        ui.painter().rect_filled(marker, 1.0, theme::ACCENT);
        let ghost = egui::Rect::from_min_size(
            egui::pos2(left, drag_top(pointer.y, drag.grab_y, clip)),
            egui::vec2(width, ROW_HEIGHT),
        );
        ctx.animate_value_with_time(
            view.drag_id.with(("row-y", drag.key)),
            ghost.top() - origin_y,
            0.0,
        );
        ui.painter().rect_filled(
            ghost.translate(egui::vec2(0.0, 2.0)).expand(1.0),
            9.0,
            Color32::from_black_alpha(90),
        );
        paint_playlist_row(
            ui,
            covers,
            view.language,
            item,
            ghost,
            RowPaint {
                selected: view.selected == Some(drag.key),
                ghost: true,
                ..Default::default()
            },
        );
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let edge = 32.0_f32.min(clip.height() * 0.25);
        if edge > 0.0 && inside_column {
            let speed = if pointer.y < clip.top() + edge {
                ((clip.top() + edge - pointer.y) / edge).clamp(0.0, 1.0) * 360.0
            } else if pointer.y > clip.bottom() - edge {
                -((pointer.y - clip.bottom() + edge) / edge).clamp(0.0, 1.0) * 360.0
            } else {
                0.0
            };
            if speed != 0.0 {
                let dt = ui.input(|input| input.stable_dt).min(0.05);
                ui.scroll_with_delta_animation(
                    egui::vec2(0.0, speed * dt),
                    egui::style::ScrollAnimation::none(),
                );
            }
        }
        ctx.request_repaint();
    }
    if !held {
        memory.suppress_release = false;
    }
    ctx.data_mut(|data| data.insert_temp(view.drag_id, memory));
    action
}

fn playlist_menu(ui: &mut egui::Ui, language: Language, item: &PlaylistItem) -> Option<Action> {
    ui.set_width(252.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.style_mut().interaction.selectable_labels = false;
    let reference = &item.reference;
    let mut action = None;
    for (icon, label, visible, intent) in [
        (
            Icon::Play,
            language.text("Воспроизвести", "Play"),
            true,
            Action::Play(reference.clone()),
        ),
        (
            Icon::Shuffle,
            language.text("Перемешать", "Shuffle play"),
            true,
            Action::Shuffle(reference.clone()),
        ),
        (
            Icon::Pin,
            if item.pinned {
                language.text("Открепить", "Unpin")
            } else {
                language.text("Закрепить", "Pin")
            },
            true,
            Action::TogglePin(reference.clone()),
        ),
        (
            Icon::Edit,
            language.text("Редактировать", "Edit details"),
            item.can_edit,
            Action::Edit(reference.clone()),
        ),
        (
            if item.can_edit {
                Icon::Trash
            } else {
                Icon::RemoveFromLibrary
            },
            if item.can_edit {
                language.text("Удалить", "Delete")
            } else {
                language.text("Удалить из библиотеки", "Remove from Your Library")
            },
            true,
            Action::Delete(reference.clone()),
        ),
    ] {
        if !visible {
            continue;
        }
        if menu_row(ui, icon, label, true).clicked() {
            action = Some(intent);
        }
    }
    ui.add_space(5.0);
    ui.separator();
    ui.add_space(5.0);
    for (icon, label, intent) in [
        (
            Icon::Radio,
            if item.kind == CollectionKind::Album {
                language.text("Микс по альбому", "Go to album mix")
            } else {
                language.text("Микс по плейлисту", "Go to playlist mix")
            },
            Action::Mix(reference.clone()),
        ),
        (
            Icon::CopyLink,
            language.text("Копировать ссылку", "Copy link"),
            Action::CopyLink(reference.clone(), item.kind == CollectionKind::Album),
        ),
    ] {
        if menu_row(ui, icon, label, true).clicked() {
            action = Some(intent);
        }
    }
    if action.is_some() {
        ui.close();
    }
    action
}

fn menu_row(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::click());
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 6.0, theme::SELECTED);
        }
        let color = if ui.is_enabled() {
            ui.visuals().text_color()
        } else {
            theme::MUTED
        };
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                rect.left_center() + egui::vec2(15.0, 0.0),
                egui::vec2(17.0, 17.0),
            ),
            icon,
            if response.hovered() || response.has_focus() {
                color
            } else {
                theme::MUTED
            },
        );
        let text = ui.painter().layout_no_wrap(
            label.into(),
            egui::TextStyle::Body.resolve(ui.style()),
            color,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 34.0, rect.center().y - text.size().y * 0.5),
            text,
            color,
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response
    })
    .inner
}

/// Tags under the header, in one row that slides like Home's carousels when
/// it does not fit. Picking one hides the others behind a reset button;
/// playlists also offer "Created by you".
fn filter_chips(
    ui: &mut egui::Ui,
    language: Language,
    search: &mut SearchState,
    items: &[PlaylistItem],
    artists: &[FollowedArtist],
) {
    #[derive(Clone, Copy)]
    enum Tag {
        Reset,
        Kind(LibraryFilter),
        Own,
    }
    let available = |filter| match filter {
        LibraryFilter::Playlists => items
            .iter()
            .any(|item| item.kind == CollectionKind::Playlist),
        LibraryFilter::Albums => items.iter().any(|item| item.kind == CollectionKind::Album),
        LibraryFilter::Artists => !artists.is_empty(),
    };
    let label = |filter| match filter {
        LibraryFilter::Playlists => language.text("Плейлисты", "Playlists"),
        LibraryFilter::Albums => language.text("Альбомы", "Albums"),
        LibraryFilter::Artists => language.text("Исполнители", "Artists"),
    };
    let all = [
        LibraryFilter::Playlists,
        LibraryFilter::Albums,
        LibraryFilter::Artists,
    ];
    let tags: Vec<Tag> = match search.filter {
        None => all
            .into_iter()
            .filter(|filter| available(*filter))
            .map(Tag::Kind)
            .collect(),
        Some(filter) => {
            let mut tags = vec![Tag::Reset, Tag::Kind(filter)];
            if filter == LibraryFilter::Playlists
                && items
                    .iter()
                    .any(|item| item.kind == CollectionKind::Playlist && item.can_edit)
            {
                tags.push(Tag::Own);
            }
            tags
        }
    };
    if search.filter.is_none() && tags.len() < 2 {
        return;
    }
    let own = language.text("Созданные вами", "By you");
    let text = |tag| match tag {
        Tag::Reset => "",
        Tag::Kind(filter) => label(filter),
        Tag::Own => own,
    };
    let mut spans = Vec::with_capacity(tags.len());
    let mut x = 0.0;
    for tag in &tags {
        let width = match tag {
            Tag::Reset => CHIP_HEIGHT,
            _ => chip_width(ui, text(*tag)),
        };
        spans.push((x, width));
        x += width + 6.0;
    }
    let (filter, own_only) = (search.filter, search.own);
    let mut clicked = None;
    crate::carousel::carousel(
        ui,
        "library-tags",
        &spans,
        CHIP_HEIGHT,
        language,
        |ui, index| {
            let tag = tags[index];
            let response = match tag {
                Tag::Reset => reset_chip(ui, language.text("Сбросить фильтр", "Clear filter")),
                Tag::Kind(kind) => chip(ui, text(tag), filter == Some(kind)),
                Tag::Own => chip(ui, text(tag), own_only),
            };
            if response.clicked() {
                clicked = Some(tag);
            }
        },
    );
    match clicked {
        // The selected tag also clears the filter, like the reset button.
        Some(Tag::Reset | Tag::Kind(_)) if search.filter.is_some() => {
            search.filter = None;
            search.own = false;
        }
        Some(Tag::Kind(kind)) => {
            search.filter = Some(kind);
            search.own = false;
        }
        Some(Tag::Own) => search.own = !search.own,
        _ => {}
    }
    ui.add_space(8.0);
}

const CHIP_HEIGHT: f32 = 28.0;

fn chip_paint(ui: &egui::Ui, rect: egui::Rect, response: &egui::Response, selected: bool) {
    let hovered = response.hovered() || response.has_focus();
    let fill = match (selected, hovered) {
        (true, false) => theme::ACCENT,
        (true, true) => Color32::from_rgb(72, 132, 216),
        (false, false) => Color32::from_rgb(35, 41, 50),
        (false, true) => Color32::from_rgb(45, 53, 65),
    };
    ui.painter().rect_filled(rect, rect.height() * 0.5, fill);
}

fn chip_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), Color32::WHITE)
        .size()
        .x
        + 24.0
}

fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), Color32::WHITE);
    let size = egui::vec2(chip_width(ui, text), CHIP_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    chip_paint(ui, rect, &response, selected);
    let color = if selected {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, color);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, selected, text)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn reset_chip(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(CHIP_HEIGHT), Sense::click());
    chip_paint(ui, rect, &response, false);
    icons::paint(
        ui.painter(),
        rect.shrink(8.0),
        Icon::Close,
        ui.visuals().text_color(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}

fn header_button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 6.0, theme::SELECTED);
        }
        icons::paint(ui.painter(), rect.shrink(4.0), icon, theme::MUTED);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
    })
    .inner
}

struct Entry<'a> {
    id: egui::Id,
    title: &'a str,
    subtitle: &'a str,
    image: Option<&'a str>,
    artist: bool,
    /// Pinned rows show a pin before the subtitle.
    pinned: bool,
}

fn paint_playlist_row(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    item: &PlaylistItem,
    rect: egui::Rect,
    state: RowPaint,
) {
    let kind = match item.kind {
        CollectionKind::Playlist => language.text("Плейлист", "Playlist"),
        CollectionKind::Album => language.text("Альбом", "Album"),
    };
    let subtitle = if item.subtitle.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind} · {}", item.subtitle)
    };
    paint_entry(
        ui,
        covers,
        rect,
        state,
        Entry {
            id: ui.id().with((item.reference.owner_id, item.reference.id)),
            title: if item.reference.title.is_empty() {
                kind
            } else {
                &item.reference.title
            },
            subtitle: &subtitle,
            image: item.reference.cover_url.as_deref(),
            artist: false,
            pinned: item.pinned,
        },
    );
}

fn paint_entry(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    rect: egui::Rect,
    state: RowPaint,
    entry: Entry<'_>,
) {
    if state.selected || state.hovered || state.focused || state.ghost {
        ui.painter().rect_filled(
            rect,
            8.0,
            if state.selected {
                theme::SELECTED
            } else {
                Color32::from_rgb(31, 38, 49)
            },
        );
    }
    if !ui.is_rect_visible(rect) {
        return;
    }
    let art = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 8.0), egui::vec2(32.0, 32.0));
    if entry.artist && entry.image.is_none() {
        ui.painter()
            .circle_filled(art.center(), 16.0, theme::SELECTED);
        icons::paint(ui.painter(), art.shrink(7.0), Icon::Account, theme::MUTED);
    } else {
        let mut image = ui.new_child(
            egui::UiBuilder::new()
                .id(entry.id.with("cover"))
                .max_rect(art)
                .layout(Layout::left_to_right(Align::Center)),
        );
        image.set_clip_rect(ui.clip_rect().intersect(art));
        covers.show_rounded(
            &mut image,
            entry.image,
            32.0,
            if entry.artist { 16 } else { 6 },
        );
    }
    let text = egui::Rect::from_min_max(
        rect.min + egui::vec2(44.0, 6.0),
        rect.max - egui::vec2(6.0, 6.0),
    );
    paint_line(
        ui,
        egui::Rect::from_min_max(text.min, egui::pos2(text.right(), rect.center().y)),
        entry.title,
        FontId::new(14.0, theme::semibold()),
        if state.selected {
            theme::ACCENT
        } else {
            ui.visuals().text_color()
        },
    );
    let mut subtitle =
        egui::Rect::from_min_max(egui::pos2(text.left(), rect.center().y + 2.0), text.max);
    if entry.pinned {
        // paint_line centers the text in its rect; the pin shares that line.
        let pin = egui::Rect::from_center_size(
            egui::pos2(subtitle.left() + 6.0, subtitle.center().y),
            egui::vec2(12.0, 12.0),
        );
        icons::paint(ui.painter(), pin, Icon::Pin, theme::MUTED);
        subtitle.min.x = pin.right() + 4.0;
    }
    paint_line(
        ui,
        subtitle,
        entry.subtitle,
        egui::TextStyle::Small.resolve(ui.style()),
        theme::MUTED,
    );
}

fn artist_rows(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    artists: &[FollowedArtist],
    offset: usize,
    selected: Option<&str>,
    range: std::ops::Range<usize>,
) -> Option<Action> {
    let origin_y = ui.max_rect().top() - range.start as f32 * ROW_STRIDE;
    let mut action = None;
    for (index, item) in artists
        .iter()
        .enumerate()
        .take(range.end.saturating_sub(offset))
        .skip(range.start.saturating_sub(offset))
    {
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                ui.max_rect().left(),
                origin_y + (index + offset) as f32 * ROW_STRIDE,
            ),
            egui::vec2(ui.available_width(), ROW_HEIGHT),
        );
        let response = ui.interact(
            rect,
            ui.id().with(("library-artist", &item.artist.id)),
            Sense::click(),
        );
        paint_entry(
            ui,
            covers,
            rect,
            RowPaint {
                selected: selected == Some(item.artist.id.as_str()),
                hovered: response.hovered(),
                focused: response.has_focus(),
                ghost: false,
            },
            Entry {
                id: response.id,
                title: &item.artist.name,
                subtitle: language.text("Исполнитель", "Artist"),
                image: item.image_url.as_deref(),
                artist: true,
                pinned: false,
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                selected == Some(item.artist.id.as_str()),
                &item.artist.name,
            )
        });
        if response.clicked() {
            action = Some(Action::OpenArtist(item.artist.clone()));
        }
        if let Some(popup) = egui::Popup::context_menu(&response).show(|ui| {
            ui.set_width(230.0);
            if menu_row(
                ui,
                Icon::RemoveFromLibrary,
                language.text("Отписаться", "Unfollow"),
                true,
            )
            .clicked()
            {
                ui.close();
                return Some(Action::UnfollowArtist(item.artist.clone()));
            }
            None
        }) && let Some(intent) = popup.inner
        {
            action = Some(intent);
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(&item.artist.name);
    }
    action
}

fn paint_line(ui: &egui::Ui, rect: egui::Rect, label: &str, font: FontId, color: Color32) {
    if !rect.is_positive() {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(rect))
        .galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_search_toggles_filters_collections_and_artists_and_restores_every_row_when_hidden() {
        struct Harness {
            ctx: egui::Context,
            covers: CoverCache,
            search: SearchState,
            language: Language,
            items: Vec<PlaylistItem>,
            artists: Vec<FollowedArtist>,
            tick: u32,
        }
        impl Harness {
            fn draw(&mut self, events: Vec<egui::Event>) -> (egui::FullOutput, Option<Action>) {
                self.tick += 1;
                let mut action = None;
                let mut output = self.ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(232.0, 400.0),
                        )),
                        time: Some(f64::from(self.tick) * 0.1),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        action = show_sidebar(
                            ui,
                            &mut self.covers,
                            &mut self.search,
                            SidebarView {
                                language: self.language,
                                items: &self.items,
                                artists: &self.artists,
                                selected: None,
                                selected_artist: None,
                                loading: false,
                                error: None,
                            },
                        );
                    },
                );
                output.textures_delta.clear();
                (output, action)
            }
            fn click(&mut self, point: egui::Pos2) -> (egui::FullOutput, Option<Action>) {
                self.draw(vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                self.draw(vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }])
            }
        }
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut album = item(7);
            album.reference.title = "Saved release".into();
            album.kind = CollectionKind::Album;
            album.subtitle = "Creator".into();
            let mut h = Harness {
                ctx,
                covers: CoverCache::new(),
                search: SearchState::default(),
                language,
                items: vec![item(1), item(2), album.clone()],
                artists: vec![FollowedArtist {
                    artist: ArtistRef {
                        id: "subscribed".into(),
                        name: "Subscribed".into(),
                    },
                    image_url: None,
                }],
                tick: 0,
            };
            h.draw(Vec::new());
            let (initial, _) = h.draw(Vec::new());
            let caption = language.text("Поиск в библиотеке", "Search in library");
            assert!(text_rect(&initial, caption).is_none());
            assert!(text_rect(&initial, language.text("Библиотека", "Library")).is_some());
            assert!(
                text_rect(
                    &initial,
                    language.text("Альбом · Creator", "Album · Creator")
                )
                .is_some()
            );
            let search_button = initial
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle)
                        if circle.center.y < 35.0 && (3.0..6.0).contains(&circle.radius) =>
                    {
                        Some(circle.center)
                    }
                    _ => None,
                })
                .expect("Library search icon");
            let (opened, _) = h.click(search_button);
            assert!(h.search.open && text_rect(&opened, caption).is_some());
            assert!(opened.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Rect(rect) if rect.fill == Color32::from_rgb(37, 42, 50) && rect.rect.width() > 200.0 && (rect.rect.height() - 38.0).abs() < 0.5)));
            let (filtered, _) = h.draw(vec![egui::Event::Text("Subscribed".into())]);
            assert_eq!(h.search.query, "Subscribed");
            assert!(text_rect(&filtered, "Saved release").is_none());
            assert!(text_rect(&filtered, "Subscribed").is_some());
            h.search.query = "Saved".into();
            let (filtered_album, _) = h.draw(Vec::new());
            assert!(
                // Below the header, the search field and one row of tags.
                text_rect(&filtered_album, "Saved release").unwrap().top() < 166.0,
                "Filtered rows must appear immediately at the start of the list"
            );
            let (closed, _) = h.click(search_button);
            assert!(!h.search.open && h.search.query.is_empty());
            assert!(text_rect(&closed, caption).is_none());
            assert!(text_rect(&closed, "Saved release").is_some());
            assert!(!h.ctx.text_edit_focused());
            let point = text_rect(&closed, "Saved release").unwrap().center();
            let (_, action) = h.click(point);
            assert_eq!(action, Some(Action::OpenAlbum(album.reference)));
        }
    }

    #[test]
    fn library_tags_show_one_kind_and_reset_to_the_whole_library() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut covers = CoverCache::new();
        let mut search = SearchState::default();
        let mut own = item(1);
        own.reference.title = "My playlist".into();
        own.can_edit = true;
        let mut saved = item(2);
        saved.reference.title = "Saved playlist".into();
        let mut album = item(3);
        album.reference.title = "Saved release".into();
        album.kind = CollectionKind::Album;
        let items = vec![own, saved, album];
        let artists = vec![FollowedArtist {
            artist: ArtistRef {
                id: "subscribed".into(),
                name: "Subscribed".into(),
            },
            image_url: None,
        }];
        let mut tick = 0;
        let mut draw = |search: &mut SearchState, events: Vec<egui::Event>| {
            tick += 1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(232.0, 400.0),
                    )),
                    time: Some(f64::from(tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    show_sidebar(
                        ui,
                        &mut covers,
                        search,
                        SidebarView {
                            language: Language::Russian,
                            items: &items,
                            artists: &artists,
                            selected: None,
                            selected_artist: None,
                            loading: false,
                            error: None,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            output
        };
        let click = |point: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        draw(&mut search, Vec::new());
        let all = draw(&mut search, Vec::new());
        let row = text_rect(&all, "Плейлисты").unwrap().center().y;
        for tag in ["Плейлисты", "Альбомы", "Исполнители"] {
            let rect = text_rect(&all, tag).unwrap_or_else(|| panic!("{tag}"));
            // Glyph bounds differ slightly (descenders); a second row would
            // be a whole tag height lower.
            assert!(
                (rect.center().y - row).abs() < 5.0,
                "Tags share one row: {tag}"
            );
        }
        let albums = text_rect(&all, "Альбомы").unwrap().center();
        draw(&mut search, click(albums));
        let only_albums = draw(&mut search, Vec::new());
        assert_eq!(search.filter, Some(LibraryFilter::Albums));
        assert!(text_rect(&only_albums, "Saved release").is_some());
        assert!(text_rect(&only_albums, "My playlist").is_none());
        assert!(text_rect(&only_albums, "Subscribed").is_none());
        assert!(text_rect(&only_albums, "Плейлисты").is_none());
        let reset = only_albums
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - CHIP_HEIGHT).abs() < 0.5
                        && (rect.rect.height() - CHIP_HEIGHT).abs() < 0.5 =>
                {
                    Some(rect.rect.center())
                }
                _ => None,
            })
            .expect("Reset tag");
        draw(&mut search, click(reset));
        let restored = draw(&mut search, Vec::new());
        assert_eq!(search.filter, None);
        assert!(text_rect(&restored, "My playlist").is_some());
        assert!(text_rect(&restored, "Subscribed").is_some());
        let playlists = text_rect(&restored, "Плейлисты").unwrap().center();
        draw(&mut search, click(playlists));
        let only_playlists = draw(&mut search, Vec::new());
        assert!(text_rect(&only_playlists, "Saved playlist").is_some());
        assert!(text_rect(&only_playlists, "Saved release").is_none());
        // The row overflows the narrow sidebar: click the tag's visible start,
        // left of the carousel's edge arrow.
        let by_you = text_rect(&only_playlists, "Созданные вами")
            .unwrap()
            .left_center()
            + egui::vec2(4.0, 0.0);
        assert!(by_you.x < 232.0 - 40.0, "{by_you:?}");
        draw(&mut search, click(by_you));
        let own_only = draw(&mut search, Vec::new());
        assert!(search.own);
        assert!(text_rect(&own_only, "My playlist").is_some());
        assert!(text_rect(&own_only, "Saved playlist").is_none());
        let selected = text_rect(&own_only, "Плейлисты").unwrap().center();
        draw(&mut search, click(selected));
        assert_eq!((search.filter, search.own), (None, false));
    }

    fn item(id: i64) -> PlaylistItem {
        PlaylistItem {
            kind: CollectionKind::Playlist,
            reference: AlbumRef {
                id,
                owner_id: -42,
                title: format!("Playlist {id}"),
                access_key: Some("synthetic-access-key".into()),
                cover_url: None,
            },
            subtitle: "3 tracks".into(),
            can_edit: false,
            pinned: false,
        }
    }

    struct View<'a> {
        items: &'a [PlaylistItem],
        selected: Option<(i64, i64)>,
        loading: bool,
        error: Option<&'a str>,
    }

    fn frame(
        ctx: &egui::Context,
        covers: &mut CoverCache,
        view: View<'_>,
        language: Language,
        tick: u32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Action>) {
        let mut action = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(216.0, 320.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| {
                action = show_sidebar(
                    ui,
                    covers,
                    &mut SearchState::default(),
                    SidebarView {
                        language,
                        items: view.items,
                        artists: &[],
                        selected: view.selected,
                        selected_artist: None,
                        loading: view.loading,
                        error: view.error,
                    },
                );
            },
        );
        output.textures_delta.clear();
        (output, action)
    }

    fn text_rect(output: &egui::FullOutput, text: &str) -> Option<egui::Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(label) if label.galley.text() == text => {
                Some(label.visual_bounding_rect())
            }
            _ => None,
        })
    }

    #[test]
    fn pinned_collections_show_a_pin_before_their_subtitle() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut covers = CoverCache::new();
        let mut items = vec![item(1), item(2)];
        items[0].pinned = true;
        let view = |items| View {
            items,
            selected: None,
            loading: false,
            error: None,
        };
        frame(
            &ctx,
            &mut covers,
            view(&items),
            Language::English,
            1,
            Vec::new(),
        );
        let (output, _) = frame(
            &ctx,
            &mut covers,
            view(&items),
            Language::English,
            2,
            Vec::new(),
        );
        let subtitles: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Playlist · 3 tracks" => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(subtitles.len(), 2);
        let (pinned, plain) = if subtitles[0].top() < subtitles[1].top() {
            (subtitles[0], subtitles[1])
        } else {
            (subtitles[1], subtitles[0])
        };
        let shift = pinned.left() - plain.left();
        assert!((15.0..=17.0).contains(&shift), "Pin gap {shift}");
    }

    #[test]
    fn russian_library_heading_clears_its_buttons_at_the_narrowest_panel() {
        // The navigation panel's inner width: its minimum minus 18 px margins.
        let width = crate::app::LEFT_PANEL_WIDTHS.0 - 36.0;
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut covers = CoverCache::new();
        let mut search = SearchState::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 400.0),
                )),
                ..Default::default()
            },
            |ui| {
                show_sidebar(
                    ui,
                    &mut covers,
                    &mut search,
                    SidebarView {
                        language: Language::Russian,
                        items: &[item(1)],
                        artists: &[],
                        selected: None,
                        selected_artist: None,
                        loading: false,
                        error: None,
                    },
                );
            },
        );
        output.textures_delta.clear();
        let heading = text_rect(&output, "Библиотека").expect("the heading must not be cut");
        // Three 24 px header buttons with 4 px gaps sit at the right edge.
        let search_button = width - 3.0 * 24.0 - 2.0 * 4.0;
        assert!(heading.right() + 4.0 <= search_button, "{heading:?}");
    }

    struct MenuHarness {
        context: egui::Context,
        covers: CoverCache,
        language: Language,
        items: Vec<PlaylistItem>,
        tick: u32,
    }
    impl MenuHarness {
        fn new(language: Language) -> Self {
            let context = egui::Context::default();
            theme::install(&context);
            let mut items = vec![item(1), item(2)];
            items[1].can_edit = true;
            Self {
                context,
                covers: CoverCache::new(),
                language,
                items,
                tick: 0,
            }
        }
        fn frame(&mut self, events: Vec<egui::Event>) -> (egui::FullOutput, Option<Action>) {
            self.tick += 1;
            let mut action = None;
            let mut output = self.context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 540.0),
                    )),
                    time: Some(f64::from(self.tick) * 0.1),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(216.0);
                    action = show_sidebar(
                        ui,
                        &mut self.covers,
                        &mut SearchState::default(),
                        SidebarView {
                            language: self.language,
                            items: &self.items,
                            artists: &[],
                            selected: None,
                            selected_artist: None,
                            loading: false,
                            error: None,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            (output, action)
        }
        fn open(&mut self) -> egui::FullOutput {
            self.frame(Vec::new());
            self.frame(Vec::new());
            let output = self.frame(Vec::new()).0;
            let position = text_rect(&output, "Playlist 2").unwrap().center();
            for pressed in [true, false] {
                let (_, action) = self.frame(vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                assert!(action.is_none(), "Right click opened the playlist page");
            }
            self.frame(Vec::new()).0
        }
        fn click(&mut self, position: egui::Pos2) -> Option<Action> {
            let mut action = None;
            for pressed in [true, false] {
                action = self
                    .frame(vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ])
                    .1;
            }
            action
        }
    }

    #[test]
    fn sidebar_context_menu_routes_every_action_to_the_right_clicked_playlist() {
        for (caption, expected) in [
            ("Play", Action::Play(item(2).reference)),
            ("Shuffle play", Action::Shuffle(item(2).reference)),
            ("Pin", Action::TogglePin(item(2).reference)),
            ("Edit details", Action::Edit(item(2).reference)),
            ("Delete", Action::Delete(item(2).reference)),
            ("Go to playlist mix", Action::Mix(item(2).reference)),
            ("Copy link", Action::CopyLink(item(2).reference, false)),
        ] {
            let mut h = MenuHarness::new(Language::English);
            let output = h.open();
            assert_eq!(
                h.click(text_rect(&output, caption).unwrap().center()),
                Some(expected)
            );
        }
    }

    fn primary(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
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

    fn warm_drag_list(h: &mut MenuHarness) -> egui::FullOutput {
        h.frame(Vec::new());
        h.frame(Vec::new());
        h.frame(Vec::new()).0
    }

    #[test]
    fn dragging_slides_neighbours_and_emits_one_exact_reorder_without_opening_a_playlist() {
        let mut h = MenuHarness::new(Language::English);
        h.items = (1..=6).map(item).collect();
        let initial = warm_drag_list(&mut h);
        let from = text_rect(&initial, "Playlist 1").unwrap().center();
        let to = text_rect(&initial, "Playlist 4").unwrap().center();
        let old_second = text_rect(&initial, "Playlist 2").unwrap().top();
        assert!(h.frame(primary(from, true)).1.is_none());
        assert!(h.frame(vec![egui::Event::PointerMoved(to)]).1.is_none());
        let mut intermediate = false;
        for _ in 0..4 {
            let (output, action) = h.frame(Vec::new());
            assert!(action.is_none());
            let current = text_rect(&output, "Playlist 2").unwrap().top();
            intermediate |= current > old_second - ROW_STRIDE + 0.5 && current < old_second - 0.5;
        }
        assert!(intermediate, "Neighbour jumped straight to its new slot");
        let (released, action) = h.frame(primary(to, false));
        assert_eq!(
            action,
            Some(Action::Reorder {
                playlist: item(1).reference,
                before: Some(item(5).reference)
            })
        );
        let released_y = text_rect(&released, "Playlist 2").unwrap().top();
        assert!(
            (released_y - (old_second - ROW_STRIDE)).abs() < 1.0,
            "Release jumped back to the old order"
        );
        let moved = h.items.remove(0);
        h.items.insert(3, moved);
        for _ in 0..3 {
            assert!(h.frame(Vec::new()).1.is_none());
        }
        let output = h.frame(Vec::new()).0;
        let moved = text_rect(&output, "Playlist 1").unwrap().center();
        assert!((moved.y - to.y).abs() < 1.0);
        assert_eq!(h.click(moved), Some(Action::Open(item(1).reference)));
    }

    #[test]
    fn dragging_respects_pinned_group_boundaries_in_both_directions() {
        for pinned_source in [false, true] {
            let mut h = MenuHarness::new(Language::English);
            h.items = (1..=6).map(item).collect();
            h.items[0].pinned = true;
            h.items[1].pinned = true;
            let output = warm_drag_list(&mut h);
            let (source, destination, before) = if pinned_source {
                (1, 6, None)
            } else {
                (4, 1, Some(item(3).reference))
            };
            let from = text_rect(&output, &format!("Playlist {source}"))
                .unwrap()
                .center();
            let to = text_rect(&output, &format!("Playlist {destination}"))
                .unwrap()
                .center();
            h.frame(primary(from, true));
            assert!(h.frame(vec![egui::Event::PointerMoved(to)]).1.is_none());
            h.frame(Vec::new());
            assert_eq!(
                h.frame(primary(to, false)).1,
                Some(Action::Reorder {
                    playlist: item(source).reference,
                    before
                })
            );
        }
    }

    #[test]
    fn escape_and_drops_outside_the_sidebar_cancel_without_playing_or_reordering() {
        for escape in [false, true] {
            let mut h = MenuHarness::new(Language::English);
            h.items = (1..=6).map(item).collect();
            let output = warm_drag_list(&mut h);
            let from = text_rect(&output, "Playlist 1").unwrap().center();
            let to = text_rect(&output, "Playlist 4").unwrap().center();
            h.frame(primary(from, true));
            h.frame(vec![egui::Event::PointerMoved(to)]);
            if escape {
                assert!(
                    h.frame(vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE
                    }])
                    .1
                    .is_none()
                );
            }
            let drop = if escape { to } else { egui::pos2(700.0, to.y) };
            let release = h.frame(primary(drop, false)).1;
            assert!(release.is_none(), "escape={escape}: {release:?}");
            for _ in 0..3 {
                assert!(h.frame(Vec::new()).1.is_none());
            }
            let output = h.frame(Vec::new()).0;
            assert!((text_rect(&output, "Playlist 1").unwrap().center().y - from.y).abs() < 1.0);
        }
    }

    #[test]
    fn dragging_at_the_viewport_edge_scrolls_without_losing_the_offscreen_source() {
        let mut h = MenuHarness::new(Language::English);
        h.items = (1..=40).map(item).collect();
        let output = warm_drag_list(&mut h);
        let from = text_rect(&output, "Playlist 1").unwrap().center();
        let edge = egui::pos2(from.x, 528.0);
        h.frame(primary(from, true));
        h.frame(vec![egui::Event::PointerMoved(edge)]);
        let mut output = h.frame(Vec::new()).0;
        for _ in 0..18 {
            let frame = h.frame(vec![egui::Event::PointerMoved(edge)]);
            assert!(frame.1.is_none());
            output = frame.0;
        }
        assert!(
            text_rect(&output, "Playlist 2").is_none(),
            "The list did not scroll at its lower edge"
        );
        assert!(
            text_rect(&output, "Playlist 1").is_some(),
            "Virtualization discarded the drag ghost"
        );
        assert!(matches!(h.frame(primary(edge, false)).1,
            Some(Action::Reorder { playlist, before: Some(before) }) if playlist.id == 1 && before.id > 10
        ));
    }

    #[test]
    fn vertical_outside_drops_commit_the_clamped_order_after_pointer_leaves_the_window() {
        for (source, pointer_y, before) in [(6, -400.0, Some(item(1).reference)), (1, 1000.0, None)]
        {
            let mut h = MenuHarness::new(Language::English);
            h.items = (1..=6).map(item).collect();
            let initial = warm_drag_list(&mut h);
            let from = text_rect(&initial, &format!("Playlist {source}"))
                .unwrap()
                .center();
            let destination = text_rect(
                &initial,
                if pointer_y < 0.0 {
                    "Playlist 1"
                } else {
                    "Playlist 6"
                },
            )
            .unwrap()
            .center();
            let outside = egui::pos2(from.x, pointer_y);
            h.frame(primary(from, true));
            assert!(
                h.frame(vec![egui::Event::PointerMoved(outside)])
                    .1
                    .is_none()
            );
            assert!(h.frame(vec![egui::Event::PointerGone]).1.is_none());
            let (held, action) = h.frame(Vec::new());
            assert!(action.is_none());
            assert!(
                text_rect(&held, &format!("Playlist {source}")).is_some(),
                "The last pointer position must keep the clamped card visible"
            );
            assert_eq!(
                h.frame(primary(outside, false)).1,
                Some(Action::Reorder {
                    playlist: item(source).reference,
                    before
                })
            );
            let moved = h.items.remove(source as usize - 1);
            let target = if pointer_y < 0.0 { 0 } else { h.items.len() };
            h.items.insert(target, moved);
            for _ in 0..3 {
                assert!(h.frame(Vec::new()).1.is_none());
            }
            let final_frame = h.frame(Vec::new()).0;
            let actual = text_rect(&final_frame, &format!("Playlist {source}"))
                .unwrap()
                .center();
            assert!(
                (actual.y - destination.y).abs() < 1.0,
                "The dropped item returned to its old position"
            );
        }
    }

    #[test]
    fn dragging_beyond_the_window_keeps_scrolling_with_the_last_pointer_position() {
        let mut h = MenuHarness::new(Language::English);
        h.items = (1..=40).map(item).collect();
        let initial = warm_drag_list(&mut h);
        let from = text_rect(&initial, "Playlist 1").unwrap().center();
        let outside = egui::pos2(from.x, 1000.0);
        h.frame(primary(from, true));
        h.frame(vec![egui::Event::PointerMoved(outside)]);
        h.frame(vec![egui::Event::PointerGone]);
        let mut output = h.frame(Vec::new()).0;
        for _ in 0..18 {
            let frame = h.frame(Vec::new());
            assert!(frame.1.is_none());
            output = frame.0;
        }
        assert!(
            text_rect(&output, "Playlist 2").is_none(),
            "Outside dragging did not continue autoscrolling"
        );
        assert!(text_rect(&output, "Playlist 1").is_some());
        assert!(matches!(h.frame(primary(outside, false)).1,
            Some(Action::Reorder { playlist, before: Some(before) }) if playlist.id == 1 && before.id > 10
        ));
    }

    #[test]
    fn dragged_card_stays_inside_the_library_when_the_pointer_leaves_the_viewport() {
        let mut h = MenuHarness::new(Language::English);
        h.items = (1..=40).map(item).collect();
        let initial = warm_drag_list(&mut h);
        let from = text_rect(&initial, "Playlist 1").unwrap().center();
        h.frame(primary(from, true));
        for pointer in [
            egui::pos2(from.x, -400.0),
            egui::pos2(from.x, 1000.0),
            egui::pos2(700.0, 300.0),
        ] {
            let (output, action) = h.frame(vec![egui::Event::PointerMoved(pointer)]);
            assert!(action.is_none());
            let card = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == Color32::from_rgb(31, 38, 49)
                            && (rect.rect.height() - ROW_HEIGHT).abs() < 0.1 =>
                    {
                        Some((rect.rect, shape.clip_rect))
                    }
                    _ => None,
                })
                .expect("Visible drag card");
            assert!(
                card.1.expand(0.2).contains_rect(card.0),
                "Drag escaped its viewport: {card:?}"
            );
            assert!(text_rect(&output, "Playlist 1").is_some());
        }
        assert!(
            h.frame(primary(egui::pos2(700.0, 300.0), false))
                .1
                .is_none()
        );
    }

    #[test]
    fn sidebar_menu_hides_editing_and_offers_removal_for_saved_readonly_playlists() {
        for language in [Language::Russian, Language::English] {
            let mut h = MenuHarness::new(language);
            h.items[1].pinned = true;
            h.items[1].can_edit = false;
            let output = h.open();
            for caption in [
                language.text("Воспроизвести", "Play"),
                language.text("Перемешать", "Shuffle play"),
                language.text("Открепить", "Unpin"),
                language.text("Удалить из библиотеки", "Remove from Your Library"),
                language.text("Микс по плейлисту", "Go to playlist mix"),
                language.text("Копировать ссылку", "Copy link"),
            ] {
                assert!(text_rect(&output, caption).is_some(), "Missing {caption}");
            }
            assert!(text_rect(&output, language.text("Редактировать", "Edit details")).is_none());
            assert!(text_rect(&output, language.text("Удалить", "Delete")).is_none());
            assert_eq!(
                h.click(
                    text_rect(
                        &output,
                        language.text("Удалить из библиотеки", "Remove from Your Library")
                    )
                    .unwrap()
                    .center()
                ),
                Some(Action::Delete(item(2).reference))
            );
            let output = h.open();
            assert_eq!(
                h.click(
                    text_rect(&output, language.text("Открепить", "Unpin"))
                        .unwrap()
                        .center()
                ),
                Some(Action::TogglePin(item(2).reference))
            );
            h.items[1].kind = CollectionKind::Album;
            let output = h.open();
            assert!(
                text_rect(&output, language.text("Микс по альбому", "Go to album mix")).is_some()
            );
            assert!(
                text_rect(
                    &output,
                    language.text("Микс по плейлисту", "Go to playlist mix")
                )
                .is_none()
            );
        }
    }

    #[test]
    fn create_button_is_immediately_left_of_refresh_and_does_not_open_a_playlist() {
        for language in [Language::Russian, Language::English] {
            for (loading, expected) in [(false, Some(Action::Refresh)), (true, None)] {
                let ctx = egui::Context::default();
                theme::install(&ctx);
                let mut covers = CoverCache::new();
                let mut tick = 0;
                let mut render = |events| {
                    tick += 1;
                    frame(
                        &ctx,
                        &mut covers,
                        View {
                            items: &[],
                            selected: None,
                            loading,
                            error: None,
                        },
                        language,
                        tick,
                        events,
                    )
                };
                render(Vec::new());
                render(Vec::new());
                let pointer = |pos, pressed| {
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                };
                // Both controls use 24px hitboxes with the standard 10px gap.
                let plus = egui::pos2(170.0, 12.0);
                render(pointer(plus, true));
                let (output, action) = render(pointer(plus, false));
                assert_eq!(action, Some(Action::Create));
                let hitbox = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == theme::SELECTED && rect.rect.contains(plus) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("Create hover background");
                assert_eq!(hitbox.size(), egui::vec2(24.0, 24.0));
                let refresh = egui::pos2(204.0, 12.0);
                assert!(refresh.x - hitbox.right() <= 22.0);
                render(pointer(refresh, true));
                let (_, action) = render(pointer(refresh, false));
                assert_eq!(action, expected);
            }
        }
    }

    #[test]
    fn playlist_rows_keep_canonical_id_and_compact_fixed_artwork() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut covers = CoverCache::new();
            let items = vec![item(1), item(2)];
            let mut output = frame(
                &ctx,
                &mut covers,
                View {
                    items: &items,
                    selected: Some((-42, 2)),
                    loading: false,
                    error: None,
                },
                language,
                1,
                Vec::new(),
            )
            .0;
            for tick in 2..4 {
                output = frame(
                    &ctx,
                    &mut covers,
                    View {
                        items: &items,
                        selected: Some((-42, 2)),
                        loading: false,
                        error: None,
                    },
                    language,
                    tick,
                    Vec::new(),
                )
                .0;
            }
            let first = text_rect(&output, "Playlist 1").unwrap();
            let second = text_rect(&output, "Playlist 2").unwrap();
            assert!((first.left() - second.left()).abs() < 0.5);
            assert!((second.top() - first.top() - 52.0).abs() < 0.5);
            let artwork: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.rect.width() == 32.0 && rect.rect.height() == 32.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(artwork.len(), 2);
            assert_eq!(artwork[0].left(), artwork[1].left());
            let pointer = |pressed| {
                vec![
                    egui::Event::PointerMoved(second.center()),
                    egui::Event::PointerButton {
                        pos: second.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            frame(
                &ctx,
                &mut covers,
                View {
                    items: &items,
                    selected: Some((-42, 2)),
                    loading: false,
                    error: None,
                },
                language,
                4,
                pointer(true),
            );
            let (_, action) = frame(
                &ctx,
                &mut covers,
                View {
                    items: &items,
                    selected: Some((-42, 2)),
                    loading: false,
                    error: None,
                },
                language,
                5,
                pointer(false),
            );
            assert_eq!(action, Some(Action::Open(items[1].reference.clone())));
        }
    }

    #[test]
    fn sidebar_uses_full_row_width_and_reveals_scroll_handle_only_at_its_edge() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut covers = CoverCache::new();
        let items: Vec<_> = (1..=32).map(item).collect();
        let render = |covers: &mut CoverCache, tick, position| {
            frame(
                &ctx,
                covers,
                View {
                    items: &items,
                    selected: Some((-42, 1)),
                    loading: false,
                    error: None,
                },
                Language::English,
                tick,
                vec![egui::Event::PointerMoved(position)],
            )
            .0
        };
        let visible_handle = |output: &egui::FullOutput| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape,egui::Shape::Rect(rect) if (rect.rect.width()-8.0).abs()<0.5
                && rect.rect.height()>=28.0 && rect.rect.left()>200.0 && rect.fill.a()>0)
            })
        };
        let mut output = render(&mut covers, 1, egui::pos2(20.0, 200.0));
        for tick in 2..5 {
            output = render(&mut covers, tick, egui::pos2(20.0, 200.0));
        }
        let row = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == theme::SELECTED && (rect.rect.height() - 48.0).abs() < 0.5 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            row.width() >= 215.0,
            "An invisible scrollbar must not steal the row width: {row:?}"
        );
        assert!(!visible_handle(&output));
        for tick in 5..8 {
            output = render(&mut covers, tick, egui::pos2(212.0, row.center().y));
        }
        assert!(
            visible_handle(&output),
            "The scrollbar edge must reveal a usable handle"
        );
        output = render(&mut covers, 8, egui::pos2(20.0, 200.0));
        assert!(!visible_handle(&output));
    }

    #[test]
    fn loading_empty_and_errors_are_distinct_without_fabricating_playlists() {
        for language in [Language::Russian, Language::English] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut covers = CoverCache::new();
            let (loading, _) = frame(
                &ctx,
                &mut covers,
                View {
                    items: &[],
                    selected: None,
                    loading: true,
                    error: None,
                },
                language,
                1,
                Vec::new(),
            );
            assert!(
                text_rect(
                    &loading,
                    language.text("Загружаем библиотеку…", "Loading library…")
                )
                .is_some()
            );
            assert!(
                text_rect(
                    &loading,
                    language.text("Библиотека пока пуста", "Your library is empty")
                )
                .is_none()
            );
            let (empty, _) = frame(
                &ctx,
                &mut covers,
                View {
                    items: &[],
                    selected: None,
                    loading: false,
                    error: None,
                },
                language,
                2,
                Vec::new(),
            );
            assert!(
                text_rect(
                    &empty,
                    language.text("Библиотека пока пуста", "Your library is empty")
                )
                .is_some()
            );
            let (error, _) = frame(
                &ctx,
                &mut covers,
                View {
                    items: &[],
                    selected: None,
                    loading: false,
                    error: Some("Synthetic error"),
                },
                language,
                3,
                Vec::new(),
            );
            assert!(text_rect(&error, "Synthetic error").is_some());
            assert!(
                text_rect(
                    &error,
                    language.text("Библиотека пока пуста", "Your library is empty")
                )
                .is_none()
            );
        }
    }
}
