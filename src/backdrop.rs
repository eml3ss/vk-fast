//! The fullscreen player's animated backdrop: the cover, heavily blurred, as
//! a few large layers that slowly drift and turn over one another. Blurring
//! runs on a worker thread; the UI only draws a tiny texture stretched wide.
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::Duration,
};

use eframe::egui::{self, Color32, ColorImage, Rect, TextureHandle, TextureOptions};

/// The blurred image's side: tiny, as only patches of its colours remain.
const SIDE: u32 = 32;
/// How long a new cover takes to show through the previous one.
const FADE_SECONDS: f64 = 1.2;
/// How much the backdrop is darkened under the lyrics, out of 255.
const DIMMING: u8 = 150;
/// The animation's frame pace.
const FRAME: Duration = Duration::from_millis(33);

/// Each layer: size relative to the screen's diagonal, how far its centre
/// circles from the middle, the circling and turning speeds (radians a
/// second), the starting angle, and its opacity.
const LAYERS: [(f32, f32, f32, f32, f32, f32); 4] = [
    (1.45, 0.00, 0.00, 0.11, 0.0, 1.0),
    (1.00, 0.30, 0.16, -0.19, 1.7, 0.85),
    (0.85, 0.36, -0.13, 0.24, 3.9, 0.8),
    (0.70, 0.40, 0.10, -0.30, 5.3, 0.75),
];

/// Two blurred copies of a cover: opaque for the bottom layer, whose edges
/// stay off screen, and with soft round edges for the drifting ones, so no
/// straight edge ever shows.
struct Blurred {
    base: ColorImage,
    soft: ColorImage,
}

struct Layer {
    url: String,
    base: TextureHandle,
    soft: TextureHandle,
    shown_at: f64,
}

#[derive(Default)]
pub struct Backdrop {
    current: Option<Layer>,
    previous: Option<Layer>,
    request: Option<(String, Receiver<Option<Blurred>>)>,
    failed: Option<String>,
    /// Animation time: it advances only while music plays.
    clock: f64,
    last_frame: Option<f64>,
}

impl Backdrop {
    /// Draws the backdrop of `url`'s cover over `rect`, once its blurred copy
    /// is ready; until then the caller's own background shows. `image` is the
    /// decoded cover, when the cover cache has it.
    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        url: Option<&str>,
        image: Option<Arc<ColorImage>>,
        playing: bool,
    ) {
        let ctx = ui.ctx();
        let now = ui.input(|input| input.time);
        if playing && let Some(last) = self.last_frame {
            self.clock += (now - last).clamp(0.0, 0.1);
        }
        self.last_frame = Some(now);
        self.poll(ctx, now);
        if let Some(url) = url
            && self.current.as_ref().is_none_or(|layer| layer.url != url)
            && self.request.as_ref().is_none_or(|(asked, _)| asked != url)
            && self.failed.as_deref() != Some(url)
            && let Some(image) = image
        {
            let (sender, receiver) = mpsc::sync_channel(1);
            let repaint = ctx.clone();
            let spawned = thread::Builder::new()
                .name("vk-backdrop-blur".into())
                .spawn(move || {
                    let _ = sender.send(blurred(&image));
                    repaint.request_repaint();
                });
            if spawned.is_ok() {
                self.request = Some((url.to_owned(), receiver));
            }
        }
        if self.current.is_none() {
            return;
        }
        let painter = ui.painter().with_clip_rect(rect);
        for layer in [&self.previous, &self.current].into_iter().flatten() {
            let fade = ((now - layer.shown_at) / FADE_SECONDS).clamp(0.0, 1.0) as f32;
            draw(&painter, rect, layer, self.clock as f32, fade);
        }
        // Darkened so the lines stay easy to read on any cover.
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(DIMMING));
        if self
            .current
            .as_ref()
            .is_some_and(|layer| now - layer.shown_at >= FADE_SECONDS)
        {
            self.previous = None;
        }
        if playing || self.previous.is_some() {
            ctx.request_repaint_after(FRAME);
        }
    }

    /// The cover whose blurred copy is shown or being made.
    #[cfg(test)]
    pub fn cover(&self) -> Option<&str> {
        self.current
            .as_ref()
            .map(|layer| layer.url.as_str())
            .or_else(|| self.request.as_ref().map(|(url, _)| url.as_str()))
    }

    fn poll(&mut self, ctx: &egui::Context, now: f64) {
        let Some((url, receiver)) = &self.request else {
            return;
        };
        let image = match receiver.try_recv() {
            Ok(image) => image,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let url = url.clone();
        self.request = None;
        let Some(images) = image else {
            self.failed = Some(url);
            return;
        };
        let base = ctx.load_texture("vk-fast-backdrop", images.base, TextureOptions::LINEAR);
        let soft = ctx.load_texture("vk-fast-backdrop-soft", images.soft, TextureOptions::LINEAR);
        self.previous = self.current.take();
        self.current = Some(Layer {
            url,
            base,
            soft,
            shown_at: now,
        });
    }
}

/// The cover shrunk, blurred and made a little more vivid.
fn blurred(cover: &ColorImage) -> Option<Blurred> {
    let [width, height] = cover.size;
    let rgba: Vec<u8> = cover
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect();
    let source = image::RgbaImage::from_raw(width as u32, height as u32, rgba)?;
    let small = image::imageops::resize(&source, SIDE, SIDE, image::imageops::FilterType::Triangle);
    // Light enough that each layer keeps distinct patches of colour: those
    // are what show the layers drifting and turning.
    let soft = image::imageops::blur(&small, 1.6);
    let pixels = soft
        .pixels()
        .map(|pixel| {
            let [r, g, b, _] = pixel.0.map(f32::from);
            let grey = (r + g + b) / 3.0;
            let vivid = |channel: f32| (grey + (channel - grey) * 1.4).clamp(0.0, 255.0) as u8;
            Color32::from_rgb(vivid(r), vivid(g), vivid(b))
        })
        .collect::<Vec<_>>();
    let half = SIDE as f32 * 0.5;
    let soft = pixels
        .iter()
        .enumerate()
        .map(|(index, pixel)| {
            let x = (index % SIDE as usize) as f32 + 0.5;
            let y = (index / SIDE as usize) as f32 + 0.5;
            // The outermost pixel centres sit exactly on the clear edge.
            let distance = ((x - half).powi(2) + (y - half).powi(2)).sqrt() / (half - 0.5);
            // Smoothly from opaque in the middle to clear at the round edge.
            let t = ((1.0 - distance) / 0.55).clamp(0.0, 1.0);
            let alpha = t * t * (3.0 - 2.0 * t);
            Color32::from_rgba_unmultiplied(
                pixel.r(),
                pixel.g(),
                pixel.b(),
                (alpha * 255.0).round() as u8,
            )
        })
        .collect();
    let size = [SIDE as usize, SIDE as usize];
    Some(Blurred {
        base: ColorImage::new(size, pixels),
        soft: ColorImage::new(size, soft),
    })
}

/// Paints the layers of one blurred cover at animation time `time`.
fn draw(painter: &egui::Painter, rect: Rect, layer: &Layer, time: f32, fade: f32) {
    let diagonal = rect.size().length();
    for (index, (scale, orbit, circling, turning, phase, opacity)) in LAYERS.into_iter().enumerate()
    {
        let angle = phase + time * circling;
        let center = rect.center() + egui::vec2(angle.cos(), angle.sin()) * orbit * diagonal * 0.5;
        let half = diagonal * scale * 0.5;
        let turn = phase + time * turning;
        let (sin, cos) = turn.sin_cos();
        let corner =
            |x: f32, y: f32| center + egui::vec2(x * cos - y * sin, x * sin + y * cos) * half;
        // The bottom layer is opaque so nothing behind shows through.
        let alpha = if index == 0 { fade } else { fade * opacity };
        let tint = Color32::from_white_alpha((alpha * 255.0).round() as u8);
        let texture = if index == 0 { &layer.base } else { &layer.soft };
        let mut mesh = egui::Mesh::with_texture(texture.id());
        for (x, y, u, v) in [
            (-1.0, -1.0, 0.0, 0.0),
            (1.0, -1.0, 1.0, 0.0),
            (1.0, 1.0, 1.0, 1.0),
            (-1.0, 1.0, 0.0, 1.0),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: corner(x, y),
                uv: egui::pos2(u, v),
                color: tint,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_blurred_small_and_the_layers_always_cover_the_screen() {
        let mut cover =
            ColorImage::new([256, 256], vec![Color32::from_rgb(20, 40, 200); 256 * 256]);
        for pixel in &mut cover.pixels[..256 * 128] {
            *pixel = Color32::from_rgb(220, 60, 20);
        }
        let images = blurred(&cover).unwrap();
        let soft = &images.base;
        assert_eq!(soft.size, [SIDE as usize, SIDE as usize]);
        assert!(
            soft.pixels.iter().all(|pixel| pixel.a() == 255),
            "the base is opaque"
        );
        // The drifting copy fades to nothing at its edges, so they never show.
        let side = SIDE as usize;
        let edge = |x: usize, y: usize| images.soft.pixels[y * side + x].a();
        assert_eq!(edge(side / 2, side / 2), 255);
        for (x, y) in [(0, 0), (side - 1, 0), (0, side / 2), (side / 2, side - 1)] {
            assert_eq!(edge(x, y), 0, "{x},{y}");
        }
        // The hard line between the halves is gone: the middle rows blend.
        let at = |y: usize| soft.pixels[y * SIDE as usize + SIDE as usize / 2];
        let (top, middle, bottom) = (at(2), at(SIDE as usize / 2), at(SIDE as usize - 3));
        assert!(
            top.r() > middle.r() && middle.r() > bottom.r(),
            "{top:?} {middle:?} {bottom:?}"
        );
        assert!(middle.b() > top.b() && middle.b() < bottom.b());

        // The opaque bottom layer covers the whole screen whatever its turn.
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
        let (scale, ..) = LAYERS[0];
        let half = rect.size().length() * scale * 0.5;
        // Its inscribed circle, around the unmoving centre, holds the screen.
        assert!(half >= rect.size().length() * 0.5);
        // Motion is visible within seconds: every layer turns at least a
        // fifth of a radian in 3 s, and the drifting ones move their centre
        // by over 4% of the screen's diagonal.
        for (scale, orbit, circling, turning, ..) in LAYERS {
            assert!(turning.abs() * 3.0 >= 0.3, "{scale}");
            if orbit > 0.0 {
                let travel = orbit * 0.5 * 2.0 * (circling.abs() * 3.0 * 0.5).sin();
                assert!(travel >= 0.04, "{scale}: {travel}");
            }
        }
    }

    #[test]
    fn a_new_cover_fades_in_over_the_previous_one_and_time_stops_on_pause() {
        let ctx = egui::Context::default();
        let mut backdrop = Backdrop::default();
        let image = Arc::new(ColorImage::new(
            [16, 16],
            vec![Color32::from_rgb(200, 30, 30); 256],
        ));
        let run = |backdrop: &mut Backdrop, time: f64, url: &str, playing: bool| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    let rect = ui.max_rect();
                    backdrop.paint(ui, rect, Some(url), Some(image.clone()), playing);
                },
            );
            output.textures_delta.clear();
            output
        };
        run(&mut backdrop, 0.0, "first", true);
        // The blur arrives from its worker.
        for step in 1..200 {
            if backdrop.current.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            run(&mut backdrop, f64::from(step) * 0.01, "first", true);
        }
        assert_eq!(backdrop.current.as_ref().unwrap().url, "first");
        let clock = backdrop.clock;
        run(&mut backdrop, 3.0, "first", false);
        run(&mut backdrop, 4.0, "first", false);
        assert_eq!(
            backdrop.clock, clock,
            "paused music keeps the backdrop still"
        );
        run(&mut backdrop, 5.0, "second", true);
        for step in 1..200 {
            if backdrop
                .current
                .as_ref()
                .is_some_and(|layer| layer.url == "second")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            run(&mut backdrop, 5.0 + f64::from(step) * 0.01, "second", true);
        }
        assert_eq!(
            backdrop.previous.as_ref().unwrap().url,
            "first",
            "fading out"
        );
        run(&mut backdrop, 20.0, "second", true);
        assert!(backdrop.previous.is_none(), "dropped once faded");
    }
}
