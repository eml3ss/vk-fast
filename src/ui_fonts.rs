//! Find script fallbacks once, off the UI thread, then share them across
//! windows. Font files are mapped, not read: only the glyphs drawn are ever
//! paged in, and the pages are shared with other programs using the fonts.
use eframe::egui::{self, FontData, FontDefinitions};
use skrifa::MetadataProvider;
use std::{
    collections::HashSet,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    thread::JoinHandle,
};

const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_READ_BYTES: usize = 128 * 1024 * 1024;
const MAX_FONTS: usize = 40;
const SCRIPTS: &[(&str, &str)] = &[
    ("Japanese", "日本語あいうえおアイウエオ"),
    ("Chinese", "汉语简体漢語繁體"),
    ("Korean", "한국어한글"),
    ("Arabic", "العربية"),
    ("Hebrew", "עברית"),
    ("Devanagari", "हिन्दी"),
    ("Thai", "ภาษาไทย"),
    ("Bengali", "বাংলা"),
    ("Tamil", "தமிழ்"),
    ("Telugu", "తెలుగు"),
    ("Gujarati", "ગુજરાતી"),
    ("Kannada", "ಕನ್ನಡ"),
    ("Malayalam", "മലയാളം"),
    ("Sinhala", "සිංහල"),
    ("Khmer", "ខ្មែរ"),
    ("Lao", "ລາວ"),
    ("Myanmar", "မြန်မာ"),
    ("Georgian", "ქართული"),
    ("Armenian", "Հայերեն"),
    ("Ethiopic", "አማርኛ"),
    ("Tibetan", "བོད་ཡིག"),
    ("Mongolian", "ᠮᠣᠩᠭᠣᠯ"),
    ("Odia", "ଓଡ଼ିଆ"),
    ("Gurmukhi", "ਪੰਜਾਬੀ"),
    ("Javanese", "꧁꧂ꦗꦮ"),
    // Decorative letters and signs common in VK titles and names.
    ("Math letters", "𝐀𝐚𝐴𝑎𝑨𝒂𝒜𝓐𝓪𝔄𝔞𝔸𝕒𝕬𝖆𝖠𝗮𝘈𝘢𝘼𝙖𝙰𝚊𝟎𝟘𝟙"),
    ("Small capitals", "ᴀʙᴄᴅᴇꜰɢʜɪᴊᴋʟᴍɴᴏᴘʀꜱᴛᴜᴠᴡʏᴢᵃᵇᶜᵈᵉⁿʰⁱ₊˚ʚɞ"),
    ("Enclosed", "ⒶⓐⓏⓩ①⑳ＡａＺｚ🅰🄰🅐"),
    ("Symbols", "★☆♡♥✦✧⋆✿❀☾☽⚡♪♫☯✞⛧ꕤ⸝⊹⋆｡°✩₊⁺⌁⚘☁︎"),
];

struct Candidate {
    path: PathBuf,
    index: u32,
}
struct Fallback {
    name: String,
    data: Arc<FontData>,
}
static FALLBACKS: OnceLock<Vec<Fallback>> = OnceLock::new();
static LOADER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
/// The window to repaint once the fallbacks are ready.
static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);
/// Set on a context whose fonts include the fallbacks.
const COMPLETE: &str = "vk-fast-script-fallbacks";

/// Inter, the primary font every other one lines up with.
pub const INTER: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");

/// A font's ascent and line height (ascent, descent and gap), in em.
fn em_metrics(face: &skrifa::FontRef) -> Option<(f32, f32)> {
    let metrics = face.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    let em = f32::from(metrics.units_per_em);
    (em > 0.0).then(|| {
        (
            metrics.ascent / em,
            (metrics.ascent - metrics.descent + metrics.leading) / em,
        )
    })
}

/// egui puts a fallback glyph's baseline at that font's own ascent plus half
/// the difference of the line heights. That meets Inter's baseline only when
/// both fonts balance ascent and descent alike; Japanese, Chinese and Korean
/// fonts, among others, do not, and their glyphs sat well below Latin ones
/// on the same line. This is the drawing offset (egui's `y_offset_factor`,
/// which leaves layout alone) that puts the baseline on Inter's.
fn baseline_offset(face: &skrifa::FontRef, scale: f32) -> f32 {
    static PRIMARY: OnceLock<Option<(f32, f32)>> = OnceLock::new();
    let primary = *PRIMARY.get_or_init(|| {
        skrifa::FontRef::new(INTER)
            .ok()
            .and_then(|face| em_metrics(&face))
    });
    let (Some((ascent, height)), Some((own_ascent, own_height))) = (primary, em_metrics(face))
    else {
        return 0.0;
    };
    if scale <= 0.0 {
        return 0.0;
    }
    // In em of the requested size; the font's own metrics follow its scale.
    let shift = ascent - scale * own_ascent - (height - scale * own_height) / 2.0;
    (shift / scale).clamp(-0.5, 0.5)
}

/// Line `data` up with Inter on the baseline, keeping its other tweaks.
pub fn align_baseline(data: &mut FontData) {
    if let Ok(face) = skrifa::FontRef::from_index(&data.font, data.index) {
        data.tweak.y_offset_factor = baseline_offset(&face, data.tweak.scale);
    }
}

/// Starts finding the fallbacks; the window does not wait for them.
pub fn preload() {
    let Ok(mut loader) = LOADER.lock() else {
        return;
    };
    if loader.is_some() || FALLBACKS.get().is_some() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("vk-fonts".into())
        .spawn(|| {
            let samples = SCRIPTS.iter().flat_map(|(_, text)| text.chars()).collect();
            let _ = FALLBACKS.set(load_candidates(candidates(), samples));
            if let Ok(wake) = WAKE.lock()
                && let Some(ctx) = wake.as_ref()
            {
                ctx.request_repaint();
            }
        });
    match spawned {
        Ok(worker) => *loader = Some(worker),
        Err(_) => {
            let _ = FALLBACKS.set(Vec::new());
        }
    }
}

/// Waits for the fallbacks, for checks that need them at once.
pub fn wait() {
    preload();
    let worker = LOADER.lock().ok().and_then(|mut loader| loader.take());
    if let Some(worker) = worker {
        let _ = worker.join();
    }
}

/// Marks whether `ctx` got the fallbacks with its fonts.
pub fn mark(ctx: &egui::Context) {
    let complete = FALLBACKS.get().is_some();
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(COMPLETE), complete));
}

/// Called every tick: installs the fallbacks once they are ready on a window
/// that started without them. Only an atomic check until then.
pub fn refresh(ctx: &egui::Context) {
    let complete = ctx.data(|data| data.get_temp::<bool>(egui::Id::new(COMPLETE)));
    if complete != Some(false) {
        return;
    }
    if FALLBACKS.get().is_none() {
        if let Ok(mut wake) = WAKE.lock() {
            *wake = Some(ctx.clone());
        }
        return;
    }
    if let Ok(mut wake) = WAKE.lock() {
        *wake = None;
    }
    crate::theme::install_fonts(ctx);
}

/// This path only clones cached Arcs: no file or process access during UI frames.
pub fn append(definitions: &mut FontDefinitions) {
    if let Some(fonts) = FALLBACKS.get() {
        append_set(definitions, fonts);
    }
}
fn append_set(definitions: &mut FontDefinitions, fonts: &[Fallback]) {
    for font in fonts {
        definitions
            .font_data
            .insert(font.name.clone(), font.data.clone());
        for family in definitions.families.values_mut() {
            if !family.contains(&font.name) {
                family.push(font.name.clone());
            }
        }
    }
}

fn load_candidates(candidates: Vec<Candidate>, mut missing: HashSet<char>) -> Vec<Fallback> {
    let mut fonts = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut read = 0;
    for candidate in candidates.into_iter().take(256) {
        if missing.is_empty() || fonts.len() >= MAX_FONTS || read >= MAX_READ_BYTES {
            break;
        }
        let Ok(path) = candidate.path.canonicalize() else {
            continue;
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FONT_BYTES {
            continue;
        }
        let Ok(bytes) = eframe::MappedFile::map(&file) else {
            continue;
        };
        if bytes.len() as u64 > MAX_FONT_BYTES {
            continue;
        }
        read += bytes.len();
        if total + bytes.len() > MAX_TOTAL_BYTES {
            continue;
        }
        let index = if skrifa::FontRef::from_index(&bytes, candidate.index).is_ok() {
            candidate.index
        } else {
            0
        };
        let Ok(face) = skrifa::FontRef::from_index(&bytes, index) else {
            continue;
        };
        let charmap = face.charmap();
        let covered: Vec<_> = missing
            .iter()
            .copied()
            .filter(|ch| charmap.map(*ch).is_some())
            .collect();
        if covered.is_empty() {
            continue;
        }
        for ch in covered {
            missing.remove(&ch);
        }
        total += bytes.len();
        let mut data = FontData::from_static(bytes.leak());
        data.index = index;
        align_baseline(&mut data);
        fonts.push(Fallback {
            name: format!("VK Fast script fallback {}", fonts.len()),
            data: Arc::new(data),
        });
    }
    fonts
}

#[cfg(not(windows))]
fn candidates() -> Vec<Candidate> {
    // Keep the Japanese face of the shared CJK collection first, as before.
    let mut result = [
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ]
    .into_iter()
    .map(|path| Candidate {
        path: path.into(),
        index: 0,
    })
    .collect::<Vec<_>>();
    // Fonts made for one script come before fontconfig's general list: its
    // first general font (often DejaVu Sans) would otherwise take Arabic
    // and Hebrew with oversized, unjoined letters.
    for root in [
        "/usr/share/fonts/noto",
        "/usr/share/fonts/truetype/noto",
        "/usr/local/share/fonts",
        "/usr/share/fonts/TTF",
        "/usr/share/fonts/truetype/dejavu",
    ] {
        for name in [
            "NotoSansCJKjp-Regular.otf",
            "NotoSansJP-Regular.ttf",
            "NotoSansArabicUI-Regular.ttf",
            "NotoSansArabic-Regular.ttf",
            "NotoSansHebrew-Regular.ttf",
            "NotoSansDevanagari-Regular.ttf",
            "NotoSansThai-Regular.ttf",
            "NotoSansBengali-Regular.ttf",
            "NotoSansTamil-Regular.ttf",
            "NotoSansTelugu-Regular.ttf",
            "NotoSansGeorgian-Regular.ttf",
            "NotoSansArmenian-Regular.ttf",
            "NotoSansJavanese-Regular.ttf",
            "NotoSansMath-Regular.ttf",
            "NotoSansSymbols-Regular.ttf",
            "NotoSansSymbols2-Regular.ttf",
        ] {
            result.push(Candidate {
                path: PathBuf::from(root).join(name),
                index: 0,
            });
        }
    }
    result.extend(fontconfig());
    result
}

#[cfg(not(windows))]
fn fontconfig() -> Vec<Candidate> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let Ok(mut child) = Command::new("fc-match")
        .args([
            "--sort",
            "--format=%{file}\t%{index}\n",
            "sans-serif:lang=ja",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Vec::new();
    };
    let reader = std::thread::Builder::new()
        .name("vk-font-list".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(64 * 1024 + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() <= 64 * 1024).then_some(bytes)
        });
    let Ok(reader) = reader else {
        let _ = child.kill();
        let _ = child.wait();
        return Vec::new();
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let Some(bytes) = reader.join().ok().flatten() else {
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Vec::new();
    };
    text.lines()
        .take(192)
        .filter_map(|line| {
            let (path, index) = line.rsplit_once('\t')?;
            let path = PathBuf::from(path);
            path.is_absolute().then_some(Candidate {
                path,
                index: index.parse().ok()?,
            })
        })
        .collect()
}

#[cfg(windows)]
fn candidates() -> Vec<Candidate> {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .unwrap_or_else(|| "C:\\Windows".into());
    let mut roots = vec![root.join("Fonts")];
    if let Some(local) = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
    {
        roots.push(local.join("Microsoft/Windows/Fonts"));
    }
    roots
        .into_iter()
        .flat_map(|root| {
            [
                "meiryo.ttc",
                "YuGothR.ttc",
                "msgothic.ttc",
                "msyh.ttc",
                "simsun.ttc",
                "malgun.ttf",
                "segoeui.ttf",
                // Decorative letters and signs: math alphabets, enclosed
                // letters, stars, notes; Javanese.
                "seguisym.ttf",
                "cambria.ttc",
                "javatext.ttf",
                "Nirmala.ttf",
                "LeelawUI.ttf",
                "ebrima.ttf",
                "sylfaen.ttf",
                "msjh.ttc",
                "NotoSansCJK-Regular.ttc",
            ]
            .into_iter()
            .map(move |name| Candidate {
                path: root.join(name),
                index: 0,
            })
        })
        .collect()
}

/// Read-only headless check; neither a music session nor the lyrics service is started.
pub fn diagnostic() {
    use eframe::egui::{self, FontFamily, FontId};
    wait();
    println!(
        "Script fallback fonts loaded: {}",
        FALLBACKS.get().map_or(0, Vec::len)
    );
    for phase in ["First frame", "Recreated window"] {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for (language, text) in SCRIPTS {
                let supported = ui.fonts_mut(|fonts| {
                    [
                        FontFamily::Proportional,
                        crate::theme::semibold(),
                        FontFamily::Monospace,
                    ]
                    .into_iter()
                    .all(|family| {
                        let definitions = fonts.definitions();
                        let supported = text.chars().all(|ch| {
                            definitions.families[&family].iter().any(|name| {
                                let data = &definitions.font_data[name];
                                skrifa::FontRef::from_index(&data.font, data.index)
                                    .ok()
                                    .is_some_and(|face| face.charmap().map(ch).is_some())
                            })
                        });
                        supported
                            && !fonts
                                .layout_no_wrap(
                                    text.to_string(),
                                    FontId::new(14.0, family),
                                    egui::Color32::WHITE,
                                )
                                .rows
                                .is_empty()
                    })
                });
                println!("{phase}: {language} = {supported}");
            }
        });
        output.textures_delta.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_fallbacks_append_to_every_family_without_replacing_primary_fonts() {
        let font = Fallback {
            name: "Synthetic fallback".into(),
            data: Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/InterVariable.ttf"
            ))),
        };
        let mut definitions = FontDefinitions::default();
        let original = definitions.families.clone();
        append_set(&mut definitions, std::slice::from_ref(&font));
        append_set(&mut definitions, std::slice::from_ref(&font));
        for (family, baseline) in original {
            let actual = &definitions.families[&family];
            assert_eq!(&actual[..baseline.len()], baseline);
            assert_eq!(&actual[baseline.len()..], std::slice::from_ref(&font.name));
        }
        assert!(Arc::ptr_eq(&definitions.font_data[&font.name], &font.data));
    }
    #[test]
    fn fallback_glyphs_are_drawn_on_inter_s_baseline() {
        let inter = skrifa::FontRef::new(INTER).unwrap();
        assert_eq!(baseline_offset(&inter, 1.0), 0.0);
        let (ascent, height) = em_metrics(&inter).unwrap();
        // egui's own fallbacks, scaled emoji fonts among them, once aligned:
        // where egui puts their baseline, plus the drawing offset, is Inter's.
        let mut definitions = FontDefinitions::default();
        for name in ["Ubuntu-Light", "NotoEmoji-Regular", "emoji-icon-font"] {
            let data = Arc::make_mut(definitions.font_data.get_mut(name).unwrap());
            align_baseline(data);
            let face = skrifa::FontRef::from_index(&data.font, data.index).unwrap();
            let (own_ascent, own_height) = em_metrics(&face).unwrap();
            let scale = data.tweak.scale;
            let baseline = scale * own_ascent
                + (height - scale * own_height) / 2.0
                + scale * data.tweak.y_offset_factor;
            assert!(
                (baseline - ascent).abs() < 1e-4,
                "{name}: {baseline} vs {ascent}"
            );
        }
    }

    #[test]
    fn invalid_fonts_are_skipped_and_repeated_files_are_loaded_once() {
        let mut suffix = [0_u8; 8];
        getrandom::fill(&mut suffix).unwrap();
        let root =
            std::env::temp_dir().join(format!("vk-fast-fonts-{:016x}", u64::from_le_bytes(suffix)));
        std::fs::create_dir(&root).unwrap();
        let invalid = root.join("invalid.ttf");
        std::fs::write(&invalid, b"invalid font").unwrap();
        let valid = root.join("valid.ttf");
        std::fs::write(&valid, include_bytes!("../assets/fonts/InterVariable.ttf")).unwrap();
        let fonts = load_candidates(
            vec![
                Candidate {
                    path: invalid,
                    index: 0,
                },
                Candidate {
                    path: valid.clone(),
                    index: 0,
                },
                Candidate {
                    path: valid,
                    index: 0,
                },
            ],
            ['Я', 'A', '日'].into_iter().collect(),
        );
        assert_eq!(fonts.len(), 1);
        assert_eq!(fonts[0].data.index, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
