//! Saving tracks to the user's folder: the system's Downloads folder unless
//! the user picks another, a file name from the track, and an ID3 tag with
//! the title, artists, album and cover for MP3 files that have none.
use std::{
    io,
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
