//! The blue round play button drawn inside a cover's corner, shared by Home
//! cards and release/playlist grids. It only reports clicks; playback stays
//! with the application.
use eframe::egui::{self, Color32, Response, Sense, Stroke};

use crate::{
    icons::{self, Icon},
    theme,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Idle,
    Pending,
    Playing,
    Paused,
}

impl PlayState {
    pub fn new(pending: bool, active: bool, playing: bool) -> Self {
        match (pending, active, playing) {
            (true, _, _) => Self::Pending,
            (false, false, _) => Self::Idle,
            (false, true, true) => Self::Playing,
            (false, true, false) => Self::Paused,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Playing => Icon::Pause,
            Self::Idle | Self::Paused | Self::Pending => Icon::Play,
        }
    }
}

/// Which collection is current, and which one is loading to start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Playback {
    pub pending: Option<(i64, i64)>,
    pub active: Option<(i64, i64)>,
    pub audible: bool,
}

impl Playback {
    pub fn state(&self, key: (i64, i64)) -> PlayState {
        PlayState::new(
            self.pending == Some(key),
            self.active == Some(key),
            self.audible,
        )
    }
}

/// Play or pause, or a spinning arc while the mix or playlist is loading.
pub fn state_icon(ui: &egui::Ui, rect: egui::Rect, state: PlayState, color: Color32) {
    if state != PlayState::Pending {
        icons::paint(ui.painter(), rect, state.icon(), color);
        return;
    }
    let radius = rect.width().min(rect.height()) * 0.42;
    let start = ui.input(|input| input.time) as f32 * 6.0;
    let points = (0..=28)
        .map(|step| {
            let angle = start + 4.4 * step as f32 / 28.0;
            rect.center() + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        Stroke::new((radius * 0.32).max(1.6), color),
    ));
    ui.ctx().request_repaint();
}

/// The button's radius for a cover of this size.
pub fn radius(art: egui::Rect) -> f32 {
    (art.width() * 0.13).clamp(14.0, 20.0)
}

/// Center of the button inside the cover's bottom-right corner.
pub fn corner(art: egui::Rect) -> egui::Pos2 {
    let radius = radius(art);
    let inset = (art.width() * 0.06).clamp(6.0, 10.0);
    art.max - egui::vec2(radius + inset, radius + inset)
}

pub fn button(
    ui: &mut egui::Ui,
    center: egui::Pos2,
    radius: f32,
    id: egui::Id,
    state: PlayState,
    label: &str,
) -> Response {
    let rect = egui::Rect::from_center_size(center, egui::Vec2::splat(radius * 2.0));
    let response = ui.interact(rect, id, Sense::click());
    if response.clicked() {
        response.request_focus();
    }
    let fill = if response.is_pointer_button_down_on() {
        theme::PRIMARY
    } else if response.hovered() || response.has_focus() {
        Color32::from_rgb(72, 132, 216)
    } else {
        theme::ACCENT
    };
    let painter = ui.painter();
    painter.circle_filled(
        center + egui::vec2(0.0, 1.5),
        radius + 1.0,
        Color32::from_black_alpha(90),
    );
    painter.circle_filled(center, radius, fill);
    let glyph = radius * 0.95;
    state_icon(
        ui,
        egui::Rect::from_center_size(center, egui::Vec2::splat(glyph)),
        state,
        Color32::WHITE,
    );
    if response.has_focus() {
        ui.painter().circle_stroke(
            center,
            radius + 2.0,
            Stroke::new(1.5, Color32::from_rgb(170, 200, 240)),
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}
