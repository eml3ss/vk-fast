//! Read the theme catalog once, before opening the first window. Keep the
//! shipped JSON as a fallback for read-only installs and invalid local files.
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
    sync::{LazyLock, OnceLock},
};

use serde::Deserialize;

use crate::{i18n::Language, palette::Palette};

const MAX_BYTES: u64 = 32 * 1024;
const TEMPLATES: [(&str, &str); 9] = [
    ("dark.json", include_str!("../assets/themes/dark.json")),
    ("oled.json", include_str!("../assets/themes/oled.json")),
    (
        "graphite.json",
        include_str!("../assets/themes/graphite.json"),
    ),
    (
        "midnight.json",
        include_str!("../assets/themes/midnight.json"),
    ),
    ("light.json", include_str!("../assets/themes/light.json")),
    (
        "kanagawa.json",
        include_str!("../assets/themes/kanagawa.json"),
    ),
    (
        "catppuccin.json",
        include_str!("../assets/themes/catppuccin.json"),
    ),
    (
        "tokyo-night.json",
        include_str!("../assets/themes/tokyo-night.json"),
    ),
    (
        "rose-pine.json",
        include_str!("../assets/themes/rose-pine.json"),
    ),
];

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub id: String,
    name: Names,
    pub palette: Palette,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Names {
    ru: String,
    en: String,
}

impl Definition {
    pub fn title(&self, language: Language) -> &str {
        // These names are user data, not UI strings to register in diagnostics.
        match language {
            Language::Russian => self.name.ru.trim(),
            Language::English => self.name.en.trim(),
        }
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let theme: Self = serde_json::from_slice(bytes).ok()?;
        (valid_id(&theme.id)
            && [&theme.name.ru, &theme.name.en].into_iter().all(|name| {
                !name.trim().is_empty()
                    && name.chars().count() <= 80
                    && !name.chars().any(char::is_control)
            })
            && theme.palette.valid_tones())
        .then_some(theme)
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

static BUNDLED: LazyLock<[Definition; TEMPLATES.len()]> = LazyLock::new(|| {
    TEMPLATES.map(|(_, json)| Definition::parse(json.as_bytes()).expect("Valid bundled theme"))
});
static CATALOG: OnceLock<Vec<Definition>> = OnceLock::new();

pub fn available() -> &'static [Definition] {
    CATALOG.get().map_or(&BUNDLED[..], Vec::as_slice)
}

pub fn original_dark() -> &'static Palette {
    &BUNDLED[0].palette
}

pub fn builtin_id(id: &str) -> bool {
    BUNDLED.iter().any(|theme| theme.id == id)
}

pub fn initialize() {
    let _ = CATALOG.get_or_init(|| {
        crate::app_paths::config_dir()
            .and_then(|config| load_at(&config))
            .unwrap_or_else(|_| {
                crate::diagnostics::event(
                    "appearance",
                    "theme catalog unavailable; using defaults",
                );
                BUNDLED.to_vec()
            })
    });
}

fn load_at(config: &Path) -> io::Result<Vec<Definition>> {
    let directory = config.join("themes");
    fs::create_dir_all(&directory)?;
    if !fs::symlink_metadata(&directory)?.is_dir() {
        return Err(io::Error::other("Invalid themes directory"));
    }
    let mut paths: Vec<_> = fs::read_dir(&directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect();
    paths.sort_unstable();

    let mut catalog = BUNDLED.to_vec();
    let mut seen = HashSet::new();
    let mut skipped = 0;
    for path in paths {
        let Some(theme) = read_theme(&path).filter(|theme| seen.insert(theme.id.clone())) else {
            skipped += 1;
            continue;
        };
        if let Some(index) = BUNDLED.iter().position(|builtin| builtin.id == theme.id) {
            catalog[index] = theme;
        } else {
            catalog.push(theme);
        }
    }
    // Keep the built-ins in their original order, independent of their names.
    catalog[BUNDLED.len()..].sort_unstable_by(|left, right| left.id.cmp(&right.id));

    let mut install_failed = 0;
    for ((filename, json), theme) in TEMPLATES.iter().zip(BUNDLED.iter()) {
        if !seen.contains(&theme.id) && install_template(&directory.join(filename), json).is_err() {
            install_failed += 1;
        }
    }
    if skipped != 0 || install_failed != 0 {
        crate::diagnostics::event(
            "appearance",
            &format!("themes skipped={skipped} defaults_not_written={install_failed}"),
        );
    }
    Ok(catalog)
}

fn read_theme(path: &Path) -> Option<Definition> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    Definition::parse(&bytes)
}

fn install_template(path: &Path, json: &str) -> io::Result<()> {
    // Publish a complete file and never replace an existing one, even a broken
    // theme or a different theme sharing the template's filename.
    if fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid theme path"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(json.as_bytes())?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    fn edited_dark(id: &str, ru: &str, en: &str) -> serde_json::Value {
        let mut theme: serde_json::Value = serde_json::from_str(TEMPLATES[0].1).unwrap();
        theme["id"] = id.into();
        theme["name"] = serde_json::json!({ "ru": ru, "en": en });
        theme["palette"]["background"] = "#172538".into();
        theme
    }

    #[test]
    fn missing_templates_are_written_but_edits_to_existing_files_are_kept() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("Музыка и настройки").join("config");
        let catalog = load_at(&config).unwrap();
        let directory = config.join("themes");
        assert_eq!(catalog.len(), TEMPLATES.len());
        for ((filename, _), theme) in TEMPLATES.iter().zip(&catalog) {
            let saved = read_theme(&directory.join(filename)).unwrap();
            assert_eq!(saved.id, theme.id);
            assert_eq!(saved.palette, theme.palette);
        }

        let path = directory.join("dark.json");
        let edited = serde_json::to_vec(&edited_dark("dark", "Моя тёмная", "My dark")).unwrap();
        fs::write(&path, &edited).unwrap();
        let catalog = load_at(&config).unwrap();
        assert_eq!(catalog.len(), TEMPLATES.len());
        assert_eq!(catalog[0].title(Language::Russian), "Моя тёмная");
        assert_eq!(catalog[0].palette.background, Color32::from_rgb(23, 37, 56));
        assert_eq!(fs::read(path).unwrap(), edited);
        assert_eq!(original_dark().background, Color32::from_rgb(15, 17, 21));
    }

    #[test]
    fn translated_names_and_identity_are_read_from_json_after_renaming() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("themes");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("На диске совсем другое имя.JSON");
        let edited = edited_dark("night-drive", "Ночная дорога", "Night drive");
        fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
        for pass in 0..2 {
            if pass != 0 {
                fs::rename(&path, directory.join("Renamed theme.json")).unwrap();
            }
            let catalog = load_at(root.path()).unwrap();
            assert_eq!(catalog.len(), TEMPLATES.len() + 1);
            let theme = catalog
                .iter()
                .find(|theme| theme.id == "night-drive")
                .unwrap();
            assert_eq!(theme.title(Language::Russian), "Ночная дорога");
            assert_eq!(theme.title(Language::English), "Night drive");
            assert_eq!(theme.palette.background, Color32::from_rgb(23, 37, 56));
        }
    }

    #[test]
    fn a_theme_can_set_its_navigation_fill_and_text_on_filled_buttons() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("themes");
        fs::create_dir(&directory).unwrap();
        let mut theme = edited_dark("bright", "Яркая", "Bright");
        theme["palette"]["active"] = "#76946a".into();
        theme["palette"]["on_active"] = "#1f1f28".into();
        theme["palette"]["on_primary"] = "#16161d".into();
        fs::write(
            directory.join("bright.json"),
            serde_json::to_vec(&theme).unwrap(),
        )
        .unwrap();
        let mut broken = edited_dark("broken", "Сломанная", "Broken");
        broken["palette"]["on_primary"] = "dark".into();
        fs::write(
            directory.join("broken.json"),
            serde_json::to_vec(&broken).unwrap(),
        )
        .unwrap();
        let catalog = load_at(root.path()).unwrap();
        let bright = catalog.iter().find(|theme| theme.id == "bright").unwrap();
        assert_eq!(
            bright.palette.active,
            Some(Color32::from_rgb(0x76, 0x94, 0x6a))
        );
        assert_eq!(
            bright.palette.on_active,
            Some(Color32::from_rgb(0x1f, 0x1f, 0x28))
        );
        assert_eq!(
            bright.palette.on_primary,
            Some(Color32::from_rgb(0x16, 0x16, 0x1d))
        );
        assert!(catalog.iter().all(|theme| theme.id != "broken"));
        // Themes without them keep the selected fill and their usual text.
        let dark = catalog.iter().find(|theme| theme.id == "dark").unwrap();
        assert_eq!(
            (
                dark.palette.active,
                dark.palette.on_active,
                dark.palette.on_primary
            ),
            (None, None, None)
        );
    }

    #[test]
    fn invalid_files_and_duplicate_ids_do_not_break_the_default_catalog() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("themes");
        fs::create_dir(&directory).unwrap();
        let dark = directory.join("dark.json");
        fs::write(&dark, b"{invalid").unwrap();
        let mut bad = edited_dark("bad", "Ошибка", "Bad");
        bad["palette"]["background"] = "blue".into();
        fs::write(directory.join("bad.json"), bad.to_string()).unwrap();
        let mut duplicate = edited_dark("drive", "Первая", "First");
        fs::write(directory.join("01.json"), duplicate.to_string()).unwrap();
        duplicate["name"]["en"] = "Second".into();
        fs::write(directory.join("02.json"), duplicate.to_string()).unwrap();

        let catalog = load_at(root.path()).unwrap();
        assert_eq!(catalog.len(), TEMPLATES.len() + 1);
        assert_eq!(catalog[0].palette, *original_dark());
        assert_eq!(fs::read(dark).unwrap(), b"{invalid");
        let drive = &catalog[TEMPLATES.len()];
        assert_eq!(drive.id, "drive");
        assert_eq!(drive.title(Language::English), "First");
    }

    #[test]
    fn an_existing_theme_can_use_another_filename_without_seeding_a_duplicate() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("themes");
        fs::create_dir(&directory).unwrap();
        let edited = edited_dark("dark", "Тёмная на диске", "Dark on disk");
        fs::write(directory.join("something else.json"), edited.to_string()).unwrap();
        let catalog = load_at(root.path()).unwrap();
        assert_eq!(catalog.len(), TEMPLATES.len());
        assert_eq!(catalog[0].title(Language::English), "Dark on disk");
        assert!(!directory.join("dark.json").exists());
    }
}
