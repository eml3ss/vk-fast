//! Sidebar playlist presentation. The application owns loading, navigation and
//! playback; this module returns intents without touching account data.
use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Sense};

use crate::{
    cover_art::CoverCache,
    i18n::Language,
    icons::{self, Icon},
    model::AlbumRef,
    theme,
};

#[derive(Clone)]
pub struct PlaylistItem {
    pub reference: AlbumRef,
    pub subtitle: String,
    pub can_edit: bool,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open(AlbumRef),
    Refresh,
    Create,
    Play(AlbumRef),
    Shuffle(AlbumRef),
    TogglePin(AlbumRef),
    Edit(AlbumRef),
    Delete(AlbumRef),
    Mix(AlbumRef),
    CopyLink(AlbumRef),
    Reorder {
        playlist: AlbumRef,
        before: Option<AlbumRef>,
    },
}

pub fn show_sidebar(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    items: &[PlaylistItem],
    selected: Option<(i64, i64)>,
    loading: bool,
    error: Option<&str>,
) -> Option<Action> {
    let mut action = None;
    let drag_id = ui.id().with("sidebar-playlist-drag");
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            icons::show(ui, Icon::Library, theme::MUTED, 16.0);
            ui.label(
                RichText::new(language.text("Плейлисты", "Playlists"))
                    .family(theme::semibold())
                    .color(theme::MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if header_button(
                    ui,
                    Icon::Refresh,
                    language.text("Обновить плейлисты", "Refresh playlists"),
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
            });
        });
    });
    ui.add_space(4.0);
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
    if items.is_empty() {
        ui.ctx().data_mut(|data| data.remove::<DragMemory>(drag_id));
        if loading {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(
                    RichText::new(language.text("Загружаем плейлисты…", "Loading playlists…"))
                        .small()
                        .color(theme::MUTED),
                );
            });
        } else if error.is_none() {
            ui.add(
                egui::Label::new(
                    RichText::new(language.text("Плейлистов пока нет", "No playlists yet"))
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
                .id_salt("sidebar-playlists")
                .max_height(height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false]),
            48.0,
            items.len(),
            |ui, range| {
                if let Some(intent) = playlist_rows(
                    ui,
                    covers,
                    RowView {
                        language,
                        items,
                        selected,
                        drag_id,
                    },
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
}

struct RowView<'a> {
    language: Language,
    items: &'a [PlaylistItem],
    selected: Option<PlaylistKey>,
    drag_id: egui::Id,
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
    let pointer = ui.input(|input| input.pointer.interact_pos());
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
                update_drag_target(drag, view.items, pointer.y, origin_y);
                // Keep the source animation at the pointer while dragging so
                // release smoothly settles it into its final slot.
                ctx.animate_value_with_time(
                    view.drag_id.with(("row-y", drag.key)),
                    pointer.y - drag.grab_y - origin_y,
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
        && pointer.is_some_and(|point| clip.expand2(egui::vec2(8.0, 20.0)).contains(point))
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
            Sense::click_and_drag(),
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
            };
            if let Some(pointer) = pointer {
                update_drag_target(&mut drag, view.items, pointer.y, origin_y);
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
        if memory.active.as_ref().is_none_or(|drag| drag.key != key) {
            paint_playlist_row(
                ui,
                covers,
                view.language,
                item,
                rect,
                RowPaint {
                    selected: view.selected == Some(key),
                    hovered: response.hovered() && !dragging,
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
            action = Some(Action::Open(item.reference.clone()));
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
            egui::pos2(left, pointer.y - drag.grab_y),
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
        if edge > 0.0 && clip.x_range().contains(pointer.x) {
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
                language.text("Убрать из библиотеки", "Remove from Your Library")
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
            language.text("Микс по плейлисту", "Go to playlist mix"),
            Action::Mix(reference.clone()),
        ),
        (
            Icon::CopyLink,
            language.text("Копировать ссылку", "Copy link"),
            Action::CopyLink(reference.clone()),
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

fn paint_playlist_row(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    language: Language,
    item: &PlaylistItem,
    rect: egui::Rect,
    state: RowPaint,
) {
    let title = if item.reference.title.is_empty() {
        language.text("Плейлист", "Playlist")
    } else {
        &item.reference.title
    };
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
    if ui.is_rect_visible(rect) {
        let art =
            egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 8.0), egui::vec2(32.0, 32.0));
        let mut image = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("playlist-art", item.reference.owner_id, item.reference.id))
                .max_rect(art)
                .layout(Layout::left_to_right(Align::Center)),
        );
        image.set_clip_rect(ui.clip_rect().intersect(art));
        covers.show(&mut image, item.reference.cover_url.as_deref(), 32.0);
        let text = egui::Rect::from_min_max(
            rect.min + egui::vec2(44.0, 6.0),
            rect.max - egui::vec2(6.0, 6.0),
        );
        let title_color = if state.selected {
            theme::ACCENT
        } else {
            ui.visuals().text_color()
        };
        if item.subtitle.is_empty() {
            paint_line(
                ui,
                text,
                title,
                FontId::new(14.0, theme::semibold()),
                title_color,
            );
        } else {
            paint_line(
                ui,
                egui::Rect::from_min_max(text.min, egui::pos2(text.right(), rect.center().y)),
                title,
                FontId::new(14.0, theme::semibold()),
                title_color,
            );
            paint_line(
                ui,
                egui::Rect::from_min_max(egui::pos2(text.left(), rect.center().y + 2.0), text.max),
                &item.subtitle,
                egui::TextStyle::Small.resolve(ui.style()),
                theme::MUTED,
            );
        }
    }
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

    fn item(id: i64) -> PlaylistItem {
        PlaylistItem {
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
                    language,
                    view.items,
                    view.selected,
                    view.loading,
                    view.error,
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
                        self.language,
                        &self.items,
                        None,
                        false,
                        None,
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
            ("Copy link", Action::CopyLink(item(2).reference)),
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
                language.text("Убрать из библиотеки", "Remove from Your Library"),
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
                        language.text("Убрать из библиотеки", "Remove from Your Library")
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
                    language.text("Загружаем плейлисты…", "Loading playlists…")
                )
                .is_some()
            );
            assert!(
                text_rect(
                    &loading,
                    language.text("Плейлистов пока нет", "No playlists yet")
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
                    language.text("Плейлистов пока нет", "No playlists yet")
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
                    language.text("Плейлистов пока нет", "No playlists yet")
                )
                .is_none()
            );
        }
    }
}
