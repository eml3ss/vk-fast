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

/// The emblem fills more of the tray's circle than of the window icon's: on
/// a dark taskbar the navy circle all but disappears, and the emblem alone
/// looked far smaller than other tray icons.
#[cfg(any(windows, test))]
const TRAY_EMBLEM_SCALE: f32 = 1.3;

/// The tray's image at `size` pixels: the notification area's own small-icon
/// size, so the shell shows it as large as other tray icons instead of
/// shrinking a bigger image. Tightly framed, unlike the padded window icon.
#[cfg(any(windows, test))]
pub fn tray_icon(size: u32) -> Arc<IconData> {
    static ICONS: std::sync::Mutex<Vec<(u32, Arc<IconData>)>> = std::sync::Mutex::new(Vec::new());
    let size = size.clamp(8, 256);
    let mut icons = ICONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, icon)) = icons.iter().find(|(known, _)| *known == size) {
        return icon.clone();
    }
    let icon = {
        let source = native_icon();
        let pixels = image::RgbaImage::from_raw(source.width, source.height, source.rgba.clone())
            .expect("Embedded VK Fast icon must contain RGBA pixels");
        let (mut left, mut top, mut right, mut bottom) = (source.width, source.height, 0, 0);
        for (x, y, pixel) in pixels.enumerate_pixels() {
            if pixel[3] > 16 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
        let center = ((left + right) as f32 * 0.5, (top + bottom) as f32 * 0.5);
        // The circle's own colour, read inside its left edge.
        let base = f32::from(pixels.get_pixel(left + 12, center.1 as u32)[0]);
        // How much of the white emblem covers a point of the original.
        let emblem = |x: i64, y: i64| -> f32 {
            if x < 0 || y < 0 || x >= i64::from(source.width) || y >= i64::from(source.height) {
                return 0.0;
            }
            let pixel = pixels.get_pixel(x as u32, y as u32);
            let white = ((f32::from(pixel[0]) - base) / (255.0 - base)).clamp(0.0, 1.0);
            white * f32::from(pixel[3]) / 255.0
        };
        let enlarged = image::RgbaImage::from_fn(source.width, source.height, |x, y| {
            let circle = pixels.get_pixel(left + 12, center.1 as u32);
            let alpha = pixels.get_pixel(x, y)[3];
            let sx = center.0 + (x as f32 + 0.5 - center.0) / TRAY_EMBLEM_SCALE - 0.5;
            let sy = center.1 + (y as f32 + 0.5 - center.1) / TRAY_EMBLEM_SCALE - 0.5;
            let (x0, y0) = (sx.floor() as i64, sy.floor() as i64);
            let (fx, fy) = (sx - sx.floor(), sy - sy.floor());
            let cover = emblem(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + emblem(x0 + 1, y0) * fx * (1.0 - fy)
                + emblem(x0, y0 + 1) * (1.0 - fx) * fy
                + emblem(x0 + 1, y0 + 1) * fx * fy;
            let channel =
                |navy: u8| (f32::from(navy) + (255.0 - f32::from(navy)) * cover).round() as u8;
            image::Rgba([
                channel(circle[0]),
                channel(circle[1]),
                channel(circle[2]),
                alpha,
            ])
        });
        let cropped = image::imageops::crop_imm(&enlarged, left, top, right - left, bottom - top);
        let pixels = image::imageops::resize(
            &cropped.to_image(),
            size,
            size,
            image::imageops::FilterType::Lanczos3,
        );
        Arc::new(IconData {
            width: pixels.width(),
            height: pixels.height(),
            rgba: pixels.into_raw(),
        })
    };
    icons.push((size, icon.clone()));
    icon
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
    fn tray_badge_fills_the_small_canvas_without_changing_the_window_icon() {
        let window = native_icon();
        let tray = tray_icon(32);
        assert_eq!((tray.width, tray.height), (32, 32));
        assert_eq!(tray.rgba.len(), 32 * 32 * 4);
        let alpha = |x: usize, y: usize| tray.rgba[(y * 32 + x) * 4 + 3];
        for (x, y) in [(0, 16), (31, 16), (16, 0), (16, 31)] {
            assert!(
                alpha(x, y) > 128,
                "Tray badge has an oversized margin at {x},{y}"
            );
        }
        for (x, y) in [(0, 0), (31, 0), (0, 31), (31, 31)] {
            assert_eq!(alpha(x, y), 0);
        }
        // The same navy circle as the window icon.
        let pixel = |x: usize, y: usize| &tray.rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        let edge = pixel(16, 2);
        assert!(edge[0] < 20 && edge[2] < 120 && edge[3] > 200, "{edge:?}");
        // An emblem filling more of it than the window icon's does.
        let white = |rgba: &[u8], side: usize| {
            rgba.as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200 && p[3] > 200)
                .count() as f32
                / (side * side) as f32
        };
        let tray_white = white(&tray.rgba, 32);
        let window_white = white(&window.rgba, 256);
        assert!(
            tray_white > window_white * 1.3,
            "{tray_white} vs {window_white}"
        );
        assert_eq!((window.width, window.height), (256, 256));
        assert!(Arc::ptr_eq(&window, &native_icon()));
        assert!(Arc::ptr_eq(&tray, &tray_icon(32)));
        // Every notification-area size is drawn exactly, edge to edge.
        for size in [16, 20, 24, 32, 40] {
            let icon = tray_icon(size);
            assert_eq!((icon.width, icon.height), (size, size));
            let side = size as usize;
            let middle = side / 2;
            let alpha = |x: usize, y: usize| icon.rgba[(y * side + x) * 4 + 3];
            assert!(
                alpha(0, middle) > 100 && alpha(side - 1, middle) > 100,
                "{size}"
            );
            assert!(
                alpha(middle, 0) > 100 && alpha(middle, side - 1) > 100,
                "{size}"
            );
        }
    }

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
        let build = include_str!("../build.rs");
        assert!(build.contains("assets/vk-fast-icon.ico"));
    }
}
