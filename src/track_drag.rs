//! A track carried out of a track list onto a drop target: one of the
//! account's own playlists in the sidebar, or the open queue. While it is
//! over its list a reorderable row moves as before; out of the list it
//! shrinks into a small card with its cover, title and artists that
//! follows the pointer.
use eframe::egui::{self, Color32};

use crate::{cover_art::CoverCache, model::Track, theme};

/// The card's size away from the list.
const CHIP: egui::Vec2 = egui::vec2(248.0, 48.0);
/// How long the row takes to shrink into the card.
const SHRINK_SECONDS: f32 = 0.14;

#[derive(Clone)]
pub struct Carried {
    pub track: Track,
    /// The list it came from: over it, a reorderable row is not a card.
    pub list: egui::Rect,
    /// Where the row was last drawn whole; the card shrinks from there.
    pub row: egui::Rect,
    /// Carried by a reorderable list, which draws the row itself over the
    /// list and ends the drag itself.
    pub reorder: bool,
}

fn id() -> egui::Id {
    egui::Id::new("carried-track")
}

pub fn carry(ctx: &egui::Context, carried: Carried) {
    ctx.data_mut(|data| data.insert_temp(id(), carried));
}

pub fn carried(ctx: &egui::Context) -> Option<Carried> {
    ctx.data(|data| data.get_temp::<Carried>(id()))
}

pub fn clear(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<Carried>(id()));
}

/// Another list took the track over and drags it from now on: the list it
/// came from lets go of it this frame.
pub fn hand_over(ctx: &egui::Context) {
    clear(ctx);
    ctx.data_mut(|data| data.insert_temp(id().with("handed-over"), true));
}

/// Whether another list took the track over this frame.
pub fn handed_over(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(id().with("handed-over")))
        .unwrap_or(false)
}

/// Away from the list it came from, or anywhere for a list that cannot be
/// reordered: shown as the small card.
fn away(ctx: &egui::Context, carried: &Carried) -> Option<egui::Pos2> {
    let pointer = ctx.input(|input| input.pointer.interact_pos())?;
    let over_list =
        pointer.x >= carried.list.left() - 8.0 && pointer.x <= carried.list.right() + 8.0;
    (!carried.reorder || !over_list).then_some(pointer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    None,
    /// A track is carried over the target.
    Hovered,
    /// A track was let go over the target this frame.
    Dropped,
}

/// Whether a carried track is over `rect`, or was let go there.
pub fn target(ui: &egui::Ui, rect: egui::Rect) -> Target {
    let Some(carried) = carried(ui.ctx()) else {
        return Target::None;
    };
    let Some(pointer) = away(ui.ctx(), &carried) else {
        return Target::None;
    };
    if !rect.contains(pointer) || !ui.clip_rect().contains(pointer) {
        return Target::None;
    }
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
        Target::Dropped
    } else {
        Target::Hovered
    }
}

/// The outline of a target a carried track is over.
pub fn paint_target(ctx: &egui::Context, rect: egui::Rect, radius: f32) {
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("track-drop-target"),
    ))
    .rect_stroke(
        rect,
        radius,
        egui::Stroke::new(2.0, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
}

/// Ends a drag that is over: a released or lost button, or Escape for a
/// list that cannot end it itself. Call it before the lists.
pub fn begin_frame(ctx: &egui::Context) {
    let Some(carried) = carried(ctx) else {
        return;
    };
    let escape = !carried.reorder
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if escape || !ctx.input(|input| input.pointer.primary_down() || input.pointer.any_released()) {
        clear(ctx);
    }
}

/// Paints the card at the pointer and ends the drag once the button is
/// let go, after the drop targets had their turn. Call it after the lists.
pub fn end_frame(ctx: &egui::Context, covers: &mut CoverCache) {
    ctx.data_mut(|data| data.remove::<bool>(id().with("handed-over")));
    let Some(carried) = carried(ctx) else {
        return;
    };
    if !ctx.input(|input| input.pointer.primary_down()) {
        clear(ctx);
        return;
    }
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    ctx.request_repaint();
    let shrink = id().with("shrink");
    let Some(pointer) = away(ctx, &carried) else {
        ctx.animate_value_with_time(shrink, 0.0, 0.0);
        return;
    };
    let t = ctx.animate_value_with_time(shrink, 1.0, SHRINK_SECONDS);
    let chip = egui::Rect::from_min_size(pointer - egui::vec2(24.0, CHIP.y * 0.5), CHIP);
    let rect = egui::Rect::from_min_max(
        carried.row.min.lerp(chip.min, t),
        carried.row.max.lerp(chip.max, t),
    );
    egui::Area::new(id().with("card"))
        .order(egui::Order::Tooltip)
        .fixed_pos(rect.min)
        .constrain(false)
        .interactable(false)
        .show(ctx, |ui| {
            paint_card(ui, covers, &carried.track, rect);
        });
}

fn paint_card(ui: &mut egui::Ui, covers: &mut CoverCache, track: &Track, rect: egui::Rect) {
    let painter = ui.painter().clone();
    painter.rect_filled(
        rect.translate(egui::vec2(0.0, 3.0)).expand(2.0),
        12.0,
        Color32::from_black_alpha(110),
    );
    painter.rect(
        rect,
        10.0,
        Color32::from_rgb(31, 38, 49),
        egui::Stroke::new(1.0, theme::PRIMARY),
        egui::StrokeKind::Inside,
    );
    let size = (rect.height() - 12.0).clamp(24.0, 40.0);
    let cover = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 6.0, rect.center().y - size * 0.5),
        egui::vec2(size, size),
    );
    let mut art = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(cover)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    covers.show(&mut art, track.cover_url.as_deref(), size);
    let left = cover.right() + 10.0;
    let width = (rect.right() - 10.0 - left).max(0.0);
    let line = |text: &str, font: egui::FontId, color: Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        painter.layout_job(job)
    };
    let title = line(
        &track.display_title(),
        egui::FontId::new(14.0, theme::semibold()),
        ui.visuals().text_color(),
    );
    let artist = line(
        &track.artist,
        egui::FontId::proportional(12.0),
        theme::MUTED,
    );
    let height = title.size().y + 2.0 + artist.size().y;
    let top = rect.center().y - height * 0.5;
    let clip = painter.with_clip_rect(rect);
    clip.galley(egui::pos2(left, top), title, Color32::WHITE);
    clip.galley(
        egui::pos2(left, top + height - artist.size().y),
        artist,
        Color32::WHITE,
    );
}
