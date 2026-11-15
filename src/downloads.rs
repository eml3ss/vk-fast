//! Saving tracks to the user's folder: the system's Downloads folder unless
//! the user picks another, a file name from the track, and our own ID3 tag
//! with the title, artists, album and cover in place of whatever tag VK's
//! file came with (often only its encoder's name).
use std::{
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// The system's Downloads folder: the XDG user directory on Linux (which
/// may be localized, e.g. "Загрузки"), the shell folder on Windows; the
/// `Downloads` folder in the home directory where neither says.
pub fn default_dir() -> PathBuf {
    #[cfg(windows)]
    {
        windows_downloads()
            .or_else(|| {
                std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Downloads"))
            })
            .unwrap_or_else(|| PathBuf::from("Downloads"))
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        config
            .and_then(|config| std::fs::read_to_string(config.join("user-dirs.dirs")).ok())
            .zip(home.clone())
            .and_then(|(text, home)| xdg_download_dir(&text, &home))
            .or_else(|| home.map(|home| home.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("Downloads"))
    }
}

/// `XDG_DOWNLOAD_DIR` from a `user-dirs.dirs` file: `"$HOME/…"` or an
/// absolute path; a bare `$HOME` means no folder of its own.
#[cfg(any(not(windows), test))]
fn xdg_download_dir(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("XDG_DOWNLOAD_DIR="))?;
    let value = value.trim().trim_matches('"');
    let path = if let Some(rest) = value.strip_prefix("$HOME") {
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            return None;
        }
        home.join(rest)
    } else {
        PathBuf::from(value)
    };
    path.is_absolute().then_some(path)
}

#[cfg(windows)]
fn windows_downloads() -> Option<PathBuf> {
    // The Downloads known folder as Explorer keeps it, %USERPROFILE% and all.
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .ok()?;
    let value: String = key
        .get_value("{374DE290-123F-4565-9164-39C4925E467B}")
        .ok()?;
    let expanded = match std::env::var("USERPROFILE") {
        Ok(home) => value.replace("%USERPROFILE%", &home),
        Err(_) => value,
    };
    let path = PathBuf::from(expanded);
    (path.is_absolute() && !path.to_string_lossy().contains('%')).then_some(path)
}

/// A file name from a track: "Artist — Title", with characters systems
/// refuse replaced, never empty, at most 150 characters before `extension`.
pub fn file_name(artist: &str, title: &str, extension: &str) -> String {
    let stem = match (artist.trim(), title.trim()) {
        ("", title) => title.to_owned(),
        (artist, "") => artist.to_owned(),
        (artist, title) => format!("{artist} — {title}"),
    };
    let stem: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || r#"/\:*?"<>|"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(150)
        .collect::<String>()
        .trim_matches(['.', ' '])
        .to_owned();
    let stem = if stem.is_empty() {
        "track".into()
    } else {
        stem
    };
    format!("{stem}.{extension}")
}

/// Moves the finished `file` into `dir` as `name`, as "name (2)" and so on
/// when that is taken: an existing file is never replaced.
pub fn place(file: tempfile::NamedTempFile, dir: &Path, name: &str) -> io::Result<PathBuf> {
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    let mut file = file;
    for attempt in 1..=999 {
        let candidate = if attempt == 1 {
            name.to_owned()
        } else if extension.is_empty() {
            format!("{stem} ({attempt})")
        } else {
            format!("{stem} ({attempt}).{extension}")
        };
        let path = dir.join(candidate);
        match file.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                file = error.file;
            }
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "No free file name",
    ))
}

/// What an MP3's ID3 tag says.
pub struct Tags<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    /// A JPEG or PNG picture: the front cover.
    pub cover: Option<&'a [u8]>,
}

/// An ID3v2.3 tag: text in UTF-16 (which every player reads), the cover as
/// an attached picture.
pub fn id3(tags: &Tags<'_>) -> Vec<u8> {
    fn frame(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(body);
    }
    fn text(out: &mut Vec<u8>, id: &[u8; 4], value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let mut body = vec![1, 0xff, 0xfe];
        for unit in value.encode_utf16() {
            body.extend_from_slice(&unit.to_le_bytes());
        }
        frame(out, id, &body);
    }
    let mut frames = Vec::new();
    text(&mut frames, b"TIT2", tags.title);
    text(&mut frames, b"TPE1", tags.artist);
    if let Some(album) = tags.album {
        text(&mut frames, b"TALB", album);
    }
    if let Some(cover) = tags.cover {
        let mime: &[u8] = if cover.starts_with(b"\x89PNG") {
            b"image/png"
        } else {
            b"image/jpeg"
        };
        let mut body = vec![0];
        body.extend_from_slice(mime);
        // A zero after the type, picture type 3 (front cover), no description.
        body.extend_from_slice(&[0, 3, 0]);
        body.extend_from_slice(cover);
        frame(&mut frames, b"APIC", &body);
    }
    let size = frames.len() as u32;
    let mut tag = b"ID3\x03\x00\x00".to_vec();
    // The tag's size, seven bits a byte.
    tag.extend_from_slice(&[
        ((size >> 21) & 0x7f) as u8,
        ((size >> 14) & 0x7f) as u8,
        ((size >> 7) & 0x7f) as u8,
        (size & 0x7f) as u8,
    ]);
    tag.extend_from_slice(&frames);
    tag
}

/// Where the audio of an MP3 or AAC file lies: after any ID3v2 tags at
/// its start and before an ID3v1 tag at its end. VK's files carry a tag
/// naming only the encoder, which players show as an unknown artist.
pub fn audio_range<R: Read + Seek>(source: &mut R) -> io::Result<(u64, u64)> {
    let length = source.seek(SeekFrom::End(0))?;
    let mut start = 0;
    // Each ID3v2 tag: "ID3", version, flags, then a size in seven-bit bytes.
    loop {
        source.seek(SeekFrom::Start(start))?;
        let mut header = [0; 10];
        if source.read_exact(&mut header).is_err()
            || &header[..3] != b"ID3"
            || header[6..10].iter().any(|byte| byte & 0x80 != 0)
        {
            break;
        }
        let size = header[6..10]
            .iter()
            .fold(0u64, |size, byte| (size << 7) | u64::from(*byte));
        // A footer repeats the header after the frames.
        let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
        let next = start + 10 + size + footer;
        if next >= length {
            break;
        }
        start = next;
    }
    let mut end = length;
    if length >= start + 128 {
        source.seek(SeekFrom::Start(length - 128))?;
        let mut tag = [0; 3];
        if source.read_exact(&mut tag).is_ok() && &tag == b"TAG" {
            end = length - 128;
        }
    }
    source.seek(SeekFrom::Start(start))?;
    Ok((start, end))
}

/// The shape VK's stand-in cover for a track without one has (a big
/// music note, in one of several colours): its 8×8 brightness, centred and
/// scaled to unit spread. Colours differ, the shape does not.
const PLACEHOLDER_SHAPE: [f32; 64] = [
    -0.87, -0.78, -0.34, -0.44, -0.95, -1.36, -1.47, -1.46, -0.45, -0.31, 0.30, 0.22, -0.19, -0.74,
    -1.01, -1.37, 0.01, 0.23, 1.03, 0.31, 0.04, -0.00, -0.23, -1.26, 0.72, 0.98, 1.80, 0.78, 0.48,
    0.48, 0.22, -1.18, 0.64, 1.41, 2.08, 1.24, 1.05, 1.02, 0.46, -1.14, 0.26, 0.96, 1.52, 1.56,
    1.30, 0.55, -0.53, -1.38, -0.53, 0.24, 0.86, 1.66, 0.62, -0.29, -1.19, -1.47, -1.27, -0.77,
    0.38, 1.87, -0.24, -1.16, -1.46, -1.47,
];

/// VK's stand-in cover rather than a real one: its brightness follows the
/// note's shape. VK's six stand-ins match it by 0.75 or more; two hundred
/// real covers by at most about 0.62.
pub fn vk_placeholder(bytes: &[u8]) -> bool {
    let Ok(picture) = image::load_from_memory(bytes) else {
        return false;
    };
    let small = image::imageops::resize(
        &picture.into_luma8(),
        8,
        8,
        image::imageops::FilterType::Triangle,
    );
    let values: Vec<f32> = small.pixels().map(|pixel| f32::from(pixel.0[0])).collect();
    let mean = values.iter().sum::<f32>() / 64.0;
    let spread = (values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f32>()
        / 64.0)
        .sqrt();
    if spread < 1.0 {
        return false;
    }
    let likeness = values
        .iter()
        .zip(PLACEHOLDER_SHAPE)
        .map(|(value, shape)| (value - mean) / spread * shape)
        .sum::<f32>()
        / 64.0;
    likeness >= 0.7
}

/// A cover every player shows: JPEG and PNG as they are, anything else
/// (VK also serves WebP) turned into a JPEG. `None` if it is no picture.
pub fn tag_cover(bytes: Vec<u8>) -> Option<Vec<u8>> {
    match image::guess_format(&bytes).ok()? {
        image::ImageFormat::Jpeg | image::ImageFormat::Png => Some(bytes),
        _ => {
            let picture = image::load_from_memory(&bytes).ok()?.into_rgb8();
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92)
                .encode_image(&picture)
                .ok()?;
            Some(jpeg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_downloads_folder_follows_the_users_xdg_setting() {
        let home = Path::new("/home/someone");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Рабочий стол\"\nXDG_DOWNLOAD_DIR=\"$HOME/Загрузки\"\n";
        assert_eq!(xdg_download_dir(text, home), Some(home.join("Загрузки")));
        assert_eq!(
            xdg_download_dir("XDG_DOWNLOAD_DIR=\"/data/dl\"", home),
            Some(PathBuf::from("/data/dl"))
        );
        assert_eq!(xdg_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"", home), None);
        assert_eq!(
            xdg_download_dir("XDG_MUSIC_DIR=\"$HOME/Music\"", home),
            None
        );
    }

    #[test]
    fn a_track_file_is_named_after_it_and_never_replaces_another() {
        assert_eq!(
            file_name("A/B", "Song: Live?", "mp3"),
            "A B — Song Live.mp3"
        );
        assert_eq!(file_name("", "  ", "mp3"), "track.mp3");
        assert_eq!(file_name("Artist", "", "m4a"), "Artist.m4a");
        let dir = tempfile::tempdir().unwrap();
        let first = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        std::fs::write(first.path(), b"one").unwrap();
        let second = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        std::fs::write(second.path(), b"two").unwrap();
        let a = place(first, dir.path(), "x.mp3").unwrap();
        let b = place(second, dir.path(), "x.mp3").unwrap();
        assert_eq!(a.file_name().unwrap(), "x.mp3");
        assert_eq!(b.file_name().unwrap(), "x (2).mp3");
        assert_eq!(std::fs::read(a).unwrap(), b"one");
    }

    #[test]
    fn vks_own_tags_are_left_out_and_only_the_audio_is_kept() {
        // An ID3v2.4 tag naming only the encoder, the audio, an ID3v1 tag.
        let mut file = b"ID3\x04\x00\x00\x00\x00\x00\x0aTSSE..Lavf".to_vec();
        file.extend_from_slice(b"\xff\xfbAUDIO-FRAMES");
        let mut v1 = b"TAG".to_vec();
        v1.resize(128, 0);
        file.extend_from_slice(&v1);
        let (start, end) = audio_range(&mut io::Cursor::new(&file)).unwrap();
        assert_eq!(&file[start as usize..end as usize], b"\xff\xfbAUDIO-FRAMES");
        // Untagged audio stays whole.
        let plain = b"\xff\xfbONLY-AUDIO".to_vec();
        assert_eq!(
            audio_range(&mut io::Cursor::new(&plain)).unwrap(),
            (0, plain.len() as u64)
        );
        // Two tags in a row, the first with a footer.
        let mut two = b"ID3\x04\x00\x10\x00\x00\x00\x02ab3DI\x04\x00\x10\x00\x00\x00\x02".to_vec();
        two.extend_from_slice(b"ID3\x03\x00\x00\x00\x00\x00\x01x");
        two.extend_from_slice(b"\xff\xfbREST");
        let (start, end) = audio_range(&mut io::Cursor::new(&two)).unwrap();
        assert_eq!(&two[start as usize..end as usize], b"\xff\xfbREST");
    }

    #[test]
    fn vks_note_stand_in_is_told_from_a_real_cover_in_any_colour() {
        let encode = |picture: image::RgbImage| {
            let mut png = Vec::new();
            image::DynamicImage::ImageRgb8(picture)
                .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            png
        };
        // The note's shape drawn large in two colours, as VK does.
        for tint in [[230_u8, 130, 90], [60, 110, 240]] {
            let note = image::RgbImage::from_fn(240, 240, |x, y| {
                let shade = PLACEHOLDER_SHAPE[(y / 30 * 8 + x / 30) as usize];
                let light = (0.6 + shade * 0.15).clamp(0.0, 1.0);
                image::Rgb(tint.map(|channel| (f32::from(channel) * light) as u8))
            });
            assert!(vk_placeholder(&encode(note)), "{tint:?}");
        }
        // A real picture: a diagonal split and a flat one; and no picture.
        let split = image::RgbImage::from_fn(240, 240, |x, y| {
            image::Rgb(if x > y { [20, 20, 30] } else { [220, 200, 180] })
        });
        assert!(!vk_placeholder(&encode(split)));
        let flat = image::RgbImage::from_pixel(64, 64, image::Rgb([90, 90, 90]));
        assert!(!vk_placeholder(&encode(flat)));
        assert!(!vk_placeholder(b"not a picture"));
    }

    #[test]
    fn a_webp_cover_becomes_a_jpeg_and_jpeg_and_png_stay_as_they_are() {
        let picture = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 40, 90]));
        let mut webp = Vec::new();
        image::DynamicImage::ImageRgb8(picture)
            .write_to(&mut io::Cursor::new(&mut webp), image::ImageFormat::WebP)
            .unwrap();
        let cover = tag_cover(webp).unwrap();
        assert_eq!(
            image::guess_format(&cover).unwrap(),
            image::ImageFormat::Jpeg
        );
        let jpeg = b"\xff\xd8\xff\xe0 synthetic".to_vec();
        assert_eq!(tag_cover(jpeg.clone()), Some(jpeg));
        assert_eq!(tag_cover(b"not a picture".to_vec()), None);
    }

    #[test]
    fn the_id3_tag_carries_the_names_and_cover_in_its_declared_size() {
        let tag = id3(&Tags {
            title: "Песня",
            artist: "Artist",
            album: Some("Album"),
            cover: Some(b"\xff\xd8\xff synthetic jpeg"),
        });
        assert!(tag.starts_with(b"ID3\x03\x00\x00"));
        let size = tag[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | usize::from(*byte));
        assert_eq!(size, tag.len() - 10);
        for id in [b"TIT2", b"TPE1", b"TALB", b"APIC"] {
            assert!(tag.windows(4).any(|window| window == id), "{id:?}");
        }
        let title: Vec<u8> = "Песня".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(tag.windows(title.len()).any(|window| window == title));
    }
}
