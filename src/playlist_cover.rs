//! Explicit native file selection and bounded local playlist-cover preparation.

use std::{
    fs::{self, File},
    io::{Cursor, Read},
};

use eframe::egui;
use image::{ImageFormat, ImageReader, Limits, imageops::FilterType};

use crate::i18n::Language;

const MAX_INPUT: u64 = 20 * 1024 * 1024;
const MAX_DECODE: u64 = 64 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const EDGE: u32 = 512;
/// What shows through transparent parts of an uploaded cover: the colour of
/// the original dark cards.
const UPLOAD_BACKDROP: egui::Color32 = egui::Color32::from_rgb(23, 27, 33);

pub struct ChosenCover {
    pub jpeg: Vec<u8>,
    pub image: egui::ColorImage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverError {
    Read,
    TooLarge,
    Format,
}

impl CoverError {
    pub fn localize(self, language: Language) -> &'static str {
        match self {
            Self::Read => language.text("Не удалось прочитать изображение.", "Could not read the image."),
            Self::TooLarge => language.text("Изображение слишком большое. Уменьшите разрешение или выберите файл размером до 20 МиБ.", "The image is too large. Reduce its resolution or choose a file up to 20 MiB."),
            Self::Format => language.text("Не удалось открыть изображение. Выберите PNG, JPEG или WebP.", "Could not open the image. Choose PNG, JPEG or WebP."),
        }
    }
}

/// This blocking picker is called only by an explicit Change cover action, on
/// the application's worker. Choosing a file performs no upload or network I/O.
pub fn choose(language: Language) -> Result<Option<ChosenCover>, CoverError> {
    let Some(path) = rfd::FileDialog::new()
        .set_title(language.text("Выбрать обложку плейлиста", "Choose playlist cover"))
        .add_filter(
            language.text("Изображения", "Images"),
            &["png", "jpg", "jpeg", "webp"],
        )
        .pick_file()
    else {
        return Ok(None);
    };
    let metadata = fs::metadata(&path).map_err(|_| CoverError::Read)?;
    if !metadata.is_file() {
        return Err(CoverError::Read);
    }
    if metadata.len() > MAX_INPUT {
        return Err(CoverError::TooLarge);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| CoverError::Read)?
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CoverError::Read)?;
    from_bytes(&bytes).map(Some)
}

fn from_bytes(bytes: &[u8]) -> Result<ChosenCover, CoverError> {
    if bytes.len() as u64 > MAX_INPUT {
        return Err(CoverError::TooLarge);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| CoverError::Format)?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP)
    ) {
        return Err(CoverError::Format);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => CoverError::TooLarge,
        _ => CoverError::Format,
    })?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err(CoverError::Format);
    }
    // Crop a borrowed square view first: resize-to-fill can allocate an enormous
    // intermediate image for a valid but extremely wide or tall source.
    let edge = width.min(height);
    let square = image::imageops::crop_imm(
        &decoded,
        (width - edge) / 2,
        (height - edge) / 2,
        edge,
        edge,
    );
    let resized = image::imageops::resize(&*square, EDGE, EDGE, FilterType::Lanczos3);
    // A fixed backdrop, whatever the theme: the cover is uploaded to VK.
    let background = UPLOAD_BACKDROP.to_array();
    let mut rgb = Vec::with_capacity((EDGE * EDGE * 3) as usize);
    for pixel in resized.pixels() {
        let alpha = u32::from(pixel[3]);
        for channel in 0..3 {
            rgb.push(
                ((u32::from(pixel[channel]) * alpha
                    + u32::from(background[channel]) * (255 - alpha)
                    + 127)
                    / 255) as u8,
            );
        }
    }
    let image = egui::ColorImage::from_rgb([EDGE as usize, EDGE as usize], &rgb);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode(&rgb, EDGE, EDGE, image::ExtendedColorType::Rgb8)
        .map_err(|_| CoverError::Format)?;
    Ok(ChosenCover { jpeg, image })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(image: image::RgbaImage) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn selected_cover_is_center_cropped_to_an_opaque_512_pixel_jpeg_and_preview() {
        let mut source = image::RgbaImage::from_pixel(320, 160, image::Rgba([0, 0, 255, 255]));
        for x in 80..240 {
            for y in 0..160 {
                source.put_pixel(x, y, image::Rgba([240, 20, 30, 255]));
            }
        }
        let cover = from_bytes(&png(source)).unwrap();
        assert_eq!(cover.image.size, [512, 512]);
        assert!(cover.image.pixels.iter().all(|pixel| pixel.a() == 255));
        let reader = ImageReader::new(Cursor::new(&cover.jpeg))
            .with_guessed_format()
            .unwrap();
        assert_eq!(reader.format(), Some(ImageFormat::Jpeg));
        let decoded = reader.decode().unwrap().into_rgb8();
        assert_eq!(decoded.dimensions(), (512, 512));
        for x in [4, 256, 507] {
            let pixel = decoded.get_pixel(x, 256);
            assert!(pixel[0] > 225 && pixel[1] < 35 && pixel[2] < 45);
        }
        assert!(cover.jpeg.len() < 1024 * 1024);
    }

    #[test]
    fn alpha_is_blended_onto_the_app_surface_and_thin_sources_stay_bounded() {
        let transparent = from_bytes(&png(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([255, 0, 0, 0]),
        )))
        .unwrap();
        assert!(
            transparent
                .image
                .pixels
                .iter()
                .all(|pixel| *pixel == UPLOAD_BACKDROP)
        );
        let thin = from_bytes(&png(image::RgbaImage::from_pixel(
            8192,
            1,
            image::Rgba([20, 40, 60, 255]),
        )))
        .unwrap();
        assert_eq!(thin.image.size, [512, 512]);
        assert_eq!(thin.image.pixels[0], egui::Color32::from_rgb(20, 40, 60));
    }

    #[test]
    fn oversized_input_and_decoded_images_are_rejected_without_a_picker() {
        assert!(matches!(
            from_bytes(&vec![0; MAX_INPUT as usize + 1]),
            Err(CoverError::TooLarge)
        ));
        let huge_dimension = png(image::RgbaImage::from_pixel(
            8193,
            1,
            image::Rgba([0, 0, 0, 255]),
        ));
        assert!(matches!(
            from_bytes(&huge_dimension),
            Err(CoverError::TooLarge)
        ));
        // The source is valid and compresses well, but its RGBA decode exceeds
        // the independent allocation cap while both dimensions are legal.
        let huge_decode = png(image::RgbaImage::from_pixel(
            4097,
            4097,
            image::Rgba([0, 0, 0, 255]),
        ));
        assert!(matches!(
            from_bytes(&huge_decode),
            Err(CoverError::TooLarge)
        ));
    }

    #[test]
    fn malformed_and_unapproved_formats_are_rejected() {
        for input in [
            b"".as_slice(),
            b"<html>not an image</html>",
            b"\x89PNG\r\n\x1a\ntruncated",
            b"GIF89a",
        ] {
            assert!(matches!(from_bytes(input), Err(CoverError::Format)));
        }
        for language in [Language::Russian, Language::English] {
            for error in [CoverError::Read, CoverError::TooLarge, CoverError::Format] {
                assert!(!error.localize(language).is_empty());
            }
        }
    }
}
