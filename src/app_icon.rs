//! One embedded brand image for the native window, taskbar and About page.
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, IconData, TextureHandle};

pub const APP_ID: &str = "vk-fast";
const PNG: &[u8] = include_bytes!("../assets/vk-fast-icon.png");
const TEXTURE_KEY: &str = "vk-fast-brand-icon";

pub fn native_icon() -> Arc<IconData> {
    static ICON: OnceLock<Arc<IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        let pixels = image::load_from_memory(PNG)
            .expect("Embedded VK Fast icon must be a valid PNG")
            .into_rgba8();
        Arc::new(IconData {
            width: pixels.width(),
            height: pixels.height(),
            rgba: pixels.into_raw(),
        })
    })
    .clone()
}

/// Context memory is copied during window recreation; its old GPU handle is
/// explicitly discarded before a new renderer uses this texture.
pub fn rebind(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<TextureHandle>(egui::Id::new(TEXTURE_KEY)));
}

pub fn show(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let id = egui::Id::new(TEXTURE_KEY);
    let existing = ui.ctx().data_mut(|data| data.get_temp::<TextureHandle>(id));
    let texture = existing.unwrap_or_else(|| {
        let icon = native_icon();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let texture = ui
            .ctx()
            .load_texture(TEXTURE_KEY, image, egui::TextureOptions::LINEAR);
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    ui.image((texture.id(), egui::vec2(size, size)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_icon_is_square_rgba_with_transparent_exterior_and_a_visible_round_badge() {
        let icon = native_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        let pixel = |x: usize, y: usize| &icon.rgba[(y * 256 + x) * 4..(y * 256 + x + 1) * 4];
        for (x, y) in [
            (0, 0),
            (255, 0),
            (0, 255),
            (255, 255),
            (12, 12),
            (243, 12),
            (12, 243),
            (243, 243),
        ] {
            assert_eq!(pixel(x, y)[3], 0, "Opaque square corner at {x},{y}");
        }
        for (x, y) in [(128, 24), (24, 128), (232, 128), (128, 232)] {
            assert!(pixel(x, y)[3] > 240, "Missing circle at {x},{y}");
        }
        assert!(
            icon.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] > 230 && p[1] > 230 && p[2] > 230 && p[3] > 240)
                .count()
                > 4000
        );
        let white: Vec<_> = icon
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, pixel)| {
                pixel[0] > 230 && pixel[1] > 230 && pixel[2] > 230 && pixel[3] > 240
            })
            .map(|(index, _)| ((index % 256) as f64, (index / 256) as f64))
            .collect();
        let center = white
            .iter()
            .fold((0.0, 0.0), |sum, point| (sum.0 + point.0, sum.1 + point.1));
        let center = (center.0 / white.len() as f64, center.1 / white.len() as f64);
        assert!(
            (center.0 - 127.5).abs() < 256.0 * 0.02 && (center.1 - 127.5).abs() < 256.0 * 0.02,
            "The emblem is visibly off-center: {center:?}"
        );
        assert!(Arc::ptr_eq(&icon, &native_icon()));
    }

    #[test]
    fn about_texture_is_recreated_after_window_context_memory_is_copied() {
        fn frame(ctx: &egui::Context) -> egui::FullOutput {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, 56.0);
            })
        }
        let original = egui::Context::default();
        let mut output = frame(&original);
        assert!(
            output
                .textures_delta
                .set
                .iter()
                .any(|(_, deltas)| deltas.iter().any(|delta| delta.image.size() == [256, 256]))
        );
        output.textures_delta.clear();
        let replacement = egui::Context::default();
        let memory = original.memory(Clone::clone);
        replacement.memory_mut(|target| *target = memory);
        rebind(&replacement);
        let mut output = frame(&replacement);
        assert!(
            output
                .textures_delta
                .set
                .iter()
                .any(|(_, deltas)| deltas.iter().any(|delta| delta.image.size() == [256, 256]))
        );
        output.textures_delta.clear();
        rebind(&original);
        rebind(&replacement);
        original.tex_manager().write().take_delta().clear();
        replacement.tex_manager().write().take_delta().clear();
    }

    #[test]
    fn windows_resource_has_every_required_size_from_the_same_round_png() {
        let ico = include_bytes!("../assets/vk-fast-icon.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        let count = usize::from(u16::from_le_bytes(ico[4..6].try_into().unwrap()));
        assert_eq!(count, 10);
        let mut sizes = Vec::new();
        for entry in ico[6..6 + count * 16].as_chunks::<16>().0 {
            let width = if entry[0] == 0 {
                256
            } else {
                usize::from(entry[0])
            };
            let height = if entry[1] == 0 {
                256
            } else {
                usize::from(entry[1])
            };
            assert_eq!(width, height);
            let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            assert!(offset >= 6 + count * 16 && length > 0 && offset + length <= ico.len());
            sizes.push(width);
        }
        sizes.sort_unstable();
        assert_eq!(sizes, [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]);
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["bundle"]["icon"][0], "assets/vk-fast-icon.ico");
        assert_eq!(config["bundle"]["icon"][1], "assets/vk-fast-icon.png");
    }
}
