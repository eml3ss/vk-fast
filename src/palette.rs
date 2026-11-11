//! Appearance themes: the colours every page, panel and widget is drawn in.
//!
//! The active theme is one global choice, read by `theme`'s colour
//! accessors on every frame, so a switch in Settings repaints everything at
//! once without a restart. The dark theme is the original look of VK Fast:
//! its values are exactly the colours the app always had.

use std::{
    cell::Cell,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

use eframe::egui::Color32;
use serde::{Deserialize, Deserializer};

use crate::{i18n::Language, themes};
use themes::original_dark;

/// A position in the startup catalog. Preferences store the stable JSON id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Theme(usize);

impl Theme {
    pub fn all() -> impl ExactSizeIterator<Item = Self> {
        (0..themes::available().len()).map(Self)
    }

    pub fn from_code(code: &str) -> Option<Self> {
        themes::available()
            .iter()
            .position(|theme| theme.id == code)
            .map(Self)
    }

    fn definition(self) -> &'static themes::Definition {
        &themes::available()[self.0]
    }

    pub fn code(self) -> &'static str {
        &self.definition().id
    }

    pub fn diagnostic_code(self) -> &'static str {
        let code = self.code();
        if themes::builtin_id(code) {
            code
        } else {
            "custom"
        }
    }

    pub fn title(self, language: Language) -> &'static str {
        self.definition().title(language)
    }

    pub fn palette(self) -> &'static Palette {
        &self.definition().palette
    }
}

/// Ready accents shown as swatches; any other colour can be picked too. The
/// accent colours links, the playing row, sliders and filled buttons. Blue
/// is the original; another colour takes its place everywhere it is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Blue,
    Violet,
    Pink,
    Red,
    Orange,
    Green,
    Teal,
}

impl Accent {
    pub const ALL: [Self; 7] = [
        Self::Blue,
        Self::Violet,
        Self::Pink,
        Self::Red,
        Self::Orange,
        Self::Green,
        Self::Teal,
    ];

    /// The name saved in `preferences.json` and written to the logs.
    pub fn code(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Violet => "violet",
            Self::Pink => "pink",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Green => "green",
            Self::Teal => "teal",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|accent| accent.code() == code)
    }

    pub fn title(self, language: Language) -> &'static str {
        match self {
            Self::Blue => language.text("Синий", "Blue"),
            Self::Violet => language.text("Фиолетовый", "Violet"),
            Self::Pink => language.text("Розовый", "Pink"),
            Self::Red => language.text("Красный", "Red"),
            Self::Orange => language.text("Оранжевый", "Orange"),
            Self::Green => language.text("Зелёный", "Green"),
            Self::Teal => language.text("Бирюзовый", "Teal"),
        }
    }

    /// The accent itself, as bright as the original blue.
    pub fn color(self) -> Color32 {
        match self {
            Self::Blue => BLUE,
            Self::Violet => rgb(128, 92, 214),
            Self::Pink => rgb(204, 74, 146),
            Self::Red => rgb(210, 70, 74),
            Self::Orange => rgb(214, 118, 42),
            Self::Green => rgb(44, 158, 96),
            Self::Teal => rgb(30, 150, 162),
        }
    }

    /// The ready accent of exactly `color`, if it is one.
    pub fn of(color: Color32) -> Option<Self> {
        Self::ALL.into_iter().find(|accent| accent.color() == color)
    }
}

/// An accent colour from `preferences.json`: a ready accent's name or any
/// `#rrggbb` colour.
pub fn parse_accent(code: &str) -> Option<Color32> {
    if let Some(accent) = Accent::from_code(code) {
        return Some(accent.color());
    }
    let hex = code.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some(Color32::from_rgb(channel(0)?, channel(2)?, channel(4)?))
}

/// How `accent` is saved: the theme's own as nothing, a ready accent by
/// name, any other colour as `#rrggbb`.
pub fn accent_code(accent: Color32) -> String {
    if accent == THEME_ACCENT {
        return String::new();
    }
    Accent::of(accent).map_or_else(
        || format!("#{:02x}{:02x}{:02x}", accent.r(), accent.g(), accent.b()),
        |accent| accent.code().to_owned(),
    )
}

/// The named colours of a theme. Greys drawn by individual widgets that
/// have no name here follow the theme through [`Palette::tone`].
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    /// Links, the playing row, the passed part of sliders.
    #[serde(deserialize_with = "deserialize_color")]
    pub accent: Color32,
    /// Secondary text and icons.
    #[serde(deserialize_with = "deserialize_color")]
    pub muted: Color32,
    /// Main text.
    #[serde(deserialize_with = "deserialize_color")]
    pub text: Color32,
    /// The brightest text and icons: white in dark themes.
    #[serde(deserialize_with = "deserialize_color")]
    pub strong: Color32,
    /// Error and warning text.
    #[serde(deserialize_with = "deserialize_color")]
    pub error: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub warning: Color32,
    /// Light themes draw dark text on light surfaces.
    pub light: bool,
    /// The page behind everything.
    #[serde(deserialize_with = "deserialize_color")]
    pub background: Color32,
    /// The sidebar, player bar and the lyrics/queue panel.
    #[serde(deserialize_with = "deserialize_color")]
    pub panel: Color32,
    /// The line between panels and the page; none where their fills differ.
    #[serde(default, deserialize_with = "deserialize_optional_color")]
    pub panel_line: Option<Color32>,
    /// Cards, fields, menus and dialogs.
    #[serde(deserialize_with = "deserialize_color")]
    pub surface: Color32,
    /// Filled buttons: Play, switches that are on.
    #[serde(deserialize_with = "deserialize_color")]
    pub primary: Color32,
    /// The selected sidebar item, pressed controls, hovered caption buttons.
    #[serde(deserialize_with = "deserialize_color")]
    pub selected: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub scroll_handle: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub scroll_hovered: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub scroll_active: Color32,
    /// Text fields.
    #[serde(deserialize_with = "deserialize_color")]
    pub field: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub link: Color32,
    /// Buttons and other controls at rest, their outline, and when hovered.
    #[serde(deserialize_with = "deserialize_color")]
    pub control: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub control_stroke: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub control_hovered: Color32,
    #[serde(deserialize_with = "deserialize_color")]
    pub control_hovered_stroke: Color32,
    /// How much of the original greys' blue cast is kept (1 = all of it).
    chroma: f32,
    /// A cast added to greys, strongest in the darkest ones.
    tint: [f32; 3],
    /// The accent the blue colours above were turned to.
    #[serde(skip, default = "original_accent")]
    accent_color: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The original accent.
const BLUE: Color32 = rgb(54, 112, 196);

fn original_accent() -> Color32 {
    BLUE
}

fn decode_theme_color<E: serde::de::Error>(code: &str) -> Result<Color32, E> {
    code.starts_with('#')
        .then(|| parse_accent(code))
        .flatten()
        .ok_or_else(|| E::custom("Expected a #rrggbb theme colour"))
}

fn deserialize_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Color32, D::Error> {
    decode_theme_color(&String::deserialize(deserializer)?)
}

fn deserialize_optional_color<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Color32>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|code| decode_theme_color(&code))
        .transpose()
}

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// The accent as `0xaarrggbb`; [`THEME_ACCENT`] (all zero) by default.
static ACCENT: AtomicU32 = AtomicU32::new(0);

/// No accent of its own picked: every theme keeps its own accent colour
/// (blue for the built-in ones). Saved as an empty `accent`.
pub const THEME_ACCENT: Color32 = Color32::TRANSPARENT;

thread_local! {
    /// Scenes over covers can use the original dark palette regardless of the
    /// current theme or changes to its on-disk definition.
    static SCENE: Cell<Option<&'static Palette>> = const { Cell::new(None) };
}

fn scene<R>(palette: &'static Palette, contents: impl FnOnce() -> R) -> R {
    struct Restore(Option<&'static Palette>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCENE.with(|scene| scene.set(self.0));
        }
    }
    let _restore = Restore(SCENE.with(|scene| scene.replace(Some(palette))));
    contents()
}

pub fn dark_scene<R>(contents: impl FnOnce() -> R) -> R {
    scene(original_dark(), contents)
}

/// The selected theme, independently of any temporary scene palette.
pub fn active() -> Theme {
    Theme(ACTIVE.load(Ordering::Relaxed))
}

/// Draw everything in `theme` from the next paint on. `theme::apply` also
/// rebuilds egui's own widget colours.
pub fn set_active(theme: Theme) {
    ACTIVE.store(theme.0, Ordering::Relaxed);
}

/// The accent the app is drawn with now.
pub fn active_accent() -> Color32 {
    let [a, r, g, b] = ACCENT.load(Ordering::Relaxed).to_be_bytes();
    if a == 0 {
        THEME_ACCENT
    } else {
        Color32::from_rgb(r, g, b)
    }
}

/// Draw with `accent` instead of the theme's blue from the next paint on.
pub fn set_accent(accent: Color32) {
    let packed = if accent == THEME_ACCENT {
        0
    } else {
        u32::from_be_bytes([255, accent.r(), accent.g(), accent.b()])
    };
    ACCENT.store(packed, Ordering::Relaxed);
}

/// The colours of the active theme with the active accent.
pub fn current() -> Palette {
    SCENE
        .with(Cell::get)
        .unwrap_or_else(|| active().palette())
        .with_accent(active_accent())
}

/// Hue (0..1), saturation and lightness of `color`.
fn hsl(color: Color32) -> [f32; 3] {
    let [r, g, b, _] = color.to_srgba_unmultiplied().map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return [0.0, 0.0, lightness];
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs()).max(f32::EPSILON);
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    [hue, saturation, lightness]
}

fn from_hsl([hue, saturation, lightness]: [f32; 3], alpha: u8) -> Color32 {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue.rem_euclid(1.0) * 6.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let offset = lightness - chroma / 2.0;
    let channel = |value: f32| ((value + offset) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(channel(r), channel(g), channel(b), alpha)
}

fn luma(color: [f32; 3]) -> f32 {
    color[0] * 0.299 + color[1] * 0.587 + color[2] * 0.114
}

/// `color`, a shade of the `from` accent's family, in the same place of the
/// `to` accent's family: its hue turned by as much as the accents', its
/// saturation scaled alike, and `from`'s lightness becoming `to`'s.
fn shift(color: Color32, from: Color32, to: Color32) -> Color32 {
    if from == to {
        return color;
    }
    let [from_hue, from_saturation, from_lightness] = hsl(from);
    let from_lightness = from_lightness.clamp(0.05, 0.95);
    let [hue, saturation, accent_lightness] = hsl(to);
    // Any colour can be picked: very dark or light ones are brought to a
    // brightness that still reads on the page and under white text.
    let accent_lightness = accent_lightness.clamp(0.3, 0.66);
    let [own_hue, own_saturation, lightness] = hsl(color);
    let saturation =
        (own_saturation * saturation / from_saturation.max(f32::EPSILON)).clamp(0.0, 1.0);
    // The old accent's lightness becomes the new accent's; darker and lighter
    // shades keep their place between it and black or white.
    let lightness = if lightness <= from_lightness {
        lightness * accent_lightness / from_lightness
    } else {
        accent_lightness
            + (lightness - from_lightness) * (1.0 - accent_lightness) / (1.0 - from_lightness)
    };
    from_hsl([own_hue + hue - from_hue, saturation, lightness], color.a())
}

impl Palette {
    pub fn valid_tones(&self) -> bool {
        (0.0..=4.0).contains(&self.chroma)
            && self
                .tint
                .iter()
                .all(|value| (-255.0..=255.0).contains(value))
    }

    /// The theme's own accent when it is not of the original blue family (a
    /// theme file with, say, a green accent); `None` for blue themes.
    pub fn own_accent(&self) -> Option<Color32> {
        let [hue, saturation, _] = hsl(self.accent);
        let [blue, _, _] = hsl(BLUE);
        let turn = (hue - blue).abs();
        let blue_family = saturation < 0.15 || turn.min(1.0 - turn) < 0.04;
        (!blue_family).then_some(self.accent)
    }

    /// This theme with `accent` in place of its own. [`THEME_ACCENT`] keeps
    /// the theme's colours exactly, and so does blue in a blue theme; a
    /// theme with an accent of its own is turned to any picked one, blue too.
    pub fn with_accent(&self, accent: Color32) -> Self {
        let own = self.own_accent();
        let mut palette = *self;
        if accent == THEME_ACCENT || (accent == BLUE && own.is_none()) {
            // Shades of blue drawn by widgets follow the theme's own accent.
            if let Some(own) = own {
                palette.accent_color = own;
            }
            return palette;
        }
        let from = own.unwrap_or(BLUE);
        palette.accent = shift(self.accent, from, accent);
        palette.primary = shift(self.primary, from, accent);
        palette.link = shift(self.link, from, accent);
        palette.selected = shift(self.selected, from, accent);
        palette.control_hovered_stroke = shift(self.control_hovered_stroke, from, accent);
        palette.accent_color = accent;
        palette
    }

    /// `color`, a colour of the blue accent family (a lighter blue, a dark
    /// blue backdrop), in the same place of the active accent's family: its
    /// hue turned by as much as the accent's, its saturation scaled alike.
    pub fn accent_tone(&self, color: Color32) -> Color32 {
        shift(color, BLUE, self.accent_color)
    }

    /// The colour that takes the place of `color`, a grey of the dark theme,
    /// in this theme: the same place between this theme's own background,
    /// cards, controls and text as `color` has in the dark theme. Accent and
    /// cover colours are not toned. The dark theme returns `color` itself.
    pub fn tone(&self, color: Color32) -> Color32 {
        let dark = original_dark();
        if self.light == dark.light
            && self.chroma == dark.chroma
            && self.tint == dark.tint
            && [
                self.background,
                self.surface,
                self.control,
                self.control_stroke,
                self.scroll_active,
                self.muted,
                self.text,
            ] == [
                dark.background,
                dark.surface,
                dark.control,
                dark.control_stroke,
                dark.scroll_active,
                dark.muted,
                dark.text,
            ]
        {
            return color;
        }
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        let channels = [f32::from(r), f32::from(g), f32::from(b)];
        let level = luma(channels);
        let target = self.level(level);
        // Darkest greys carry the full tint; it fades out towards white.
        let tint_weight = (1.0 - target / 160.0).clamp(0.0, 1.0);
        let mapped = std::array::from_fn::<_, 3, _>(|index| {
            let value = if target <= 0.0 {
                0.0
            } else {
                target + (channels[index] - level) * self.chroma + self.tint[index] * tint_weight
            };
            value.round().clamp(0.0, 255.0) as u8
        });
        Color32::from_rgba_unmultiplied(mapped[0], mapped[1], mapped[2], a)
    }

    /// This theme's brightness for a brightness of the dark theme, through
    /// the named colours both themes have.
    fn level(&self, level: f32) -> f32 {
        let dark = original_dark();
        let pair = |dark: Color32, own: Color32| {
            let level = |color: Color32| {
                let [r, g, b, _] = color.to_array();
                luma([f32::from(r), f32::from(g), f32::from(b)])
            };
            (level(dark), level(own))
        };
        // A light theme turns brightness around: the darkest greys become its
        // page, white text its darkest text.
        let (black, white) = if self.light {
            (self.level_of(self.background), 0.0)
        } else {
            (0.0, 255.0)
        };
        let anchors = [
            (0.0, black),
            pair(dark.background, self.background),
            pair(dark.surface, self.surface),
            pair(dark.control, self.control),
            pair(dark.control_stroke, self.control_stroke),
            pair(dark.scroll_active, self.scroll_active),
            pair(dark.muted, self.muted),
            pair(dark.text, self.text),
            (255.0, white),
        ];
        let mut below = anchors[0];
        for anchor in anchors.into_iter().skip(1) {
            if level <= anchor.0 {
                let span = anchor.0 - below.0;
                let fraction = if span > 0.0 {
                    (level - below.0) / span
                } else {
                    1.0
                };
                return below.1 + (anchor.1 - below.1) * fraction;
            }
            below = anchor;
        }
        white
    }

    fn level_of(&self, color: Color32) -> f32 {
        let [r, g, b, _] = color.to_array();
        luma([f32::from(r), f32::from(g), f32::from(b)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_palette_is_the_original_look() {
        assert_eq!(Theme::default().code(), "dark");
        let dark = Theme::default().palette();
        assert_eq!(dark.accent, Color32::from_rgb(54, 112, 196));
        assert_eq!(dark.muted, Color32::from_rgb(156, 166, 181));
        assert_eq!(dark.background, Color32::from_rgb(15, 17, 21));
        assert_eq!(dark.surface, Color32::from_rgb(23, 27, 33));
        assert_eq!(dark.panel, Color32::from_rgb(23, 27, 33));
        assert_eq!(dark.panel_line, None);
        assert_eq!(dark.primary, Color32::from_rgb(34, 77, 139));
        assert_eq!(dark.selected, Color32::from_rgb(27, 44, 67));
        assert_eq!(dark.scroll_handle, Color32::from_rgb(57, 65, 77));
        assert_eq!(dark.scroll_hovered, Color32::from_rgb(75, 85, 99));
        assert_eq!(dark.scroll_active, Color32::from_rgb(91, 104, 122));
        assert_eq!(dark.text, Color32::from_rgb(231, 235, 242));
        assert_eq!(dark.field, Color32::from_rgb(18, 21, 27));
        assert_eq!(dark.link, Color32::from_rgb(98, 148, 219));
        assert_eq!(dark.control, Color32::from_rgb(32, 38, 47));
        assert_eq!(dark.control_stroke, Color32::from_rgb(43, 50, 61));
        assert_eq!(dark.control_hovered, Color32::from_rgb(42, 50, 63));
        assert_eq!(dark.control_hovered_stroke, Color32::from_rgb(69, 87, 112));
        for value in 0..=255u8 {
            for color in [
                Color32::from_gray(value),
                Color32::from_rgb(value, value / 2, 255 - value),
                Color32::from_rgba_unmultiplied(value, 40, 90, value),
            ] {
                assert_eq!(dark.tone(color), color);
            }
        }
    }

    #[test]
    fn oled_page_and_panels_are_pure_black_and_cards_still_stand_out() {
        let oled = Theme::from_code("oled").unwrap().palette();
        assert_eq!(oled.background, Color32::BLACK);
        assert_eq!(oled.panel, Color32::BLACK);
        // Everything as dark as the original page or darker is off.
        assert_eq!(oled.tone(original_dark().background), Color32::BLACK);
        assert_eq!(oled.tone(Color32::from_gray(10)), Color32::BLACK);
        assert!(oled.panel_line.is_some());
        let lift = |color: Color32| {
            color.to_array()[..3]
                .iter()
                .map(|c| u32::from(*c))
                .sum::<u32>()
        };
        assert!(lift(oled.surface) >= 45);
        assert!(lift(oled.control) > lift(oled.surface));
        assert!(lift(oled.selected) > lift(oled.surface));
    }

    #[test]
    fn a_theme_with_an_accent_of_its_own_keeps_it_until_another_is_picked() {
        for theme in Theme::all() {
            let palette = theme.palette();
            assert_eq!(palette.own_accent(), None, "{theme:?}");
            assert_eq!(palette.with_accent(THEME_ACCENT), *palette);
            assert_eq!(palette.with_accent(BLUE), *palette);
        }
        let mut green = *original_dark();
        green.accent = rgb(0x67, 0xe4, 0x95);
        assert_eq!(green.own_accent(), Some(green.accent));
        let kept = green.with_accent(THEME_ACCENT);
        assert_eq!(kept.accent, green.accent);
        // Shades of blue drawn by widgets follow the theme's green.
        let hue = |color| hsl(color)[0];
        assert!((hue(kept.accent_tone(BLUE)) - hue(green.accent)).abs() < 0.01);
        // Picking blue turns the green theme blue.
        assert!((hue(green.with_accent(BLUE).accent) - hue(BLUE)).abs() < 0.01);
        assert!(
            (hue(green.with_accent(Accent::Red.color()).accent) - hue(Accent::Red.color())).abs()
                < 0.01
        );
        assert_eq!(accent_code(THEME_ACCENT), "");
        assert_eq!(accent_code(BLUE), "blue");
        assert_eq!(parse_accent(""), None, "nothing saved: the theme's own");
    }

    #[test]
    fn every_theme_maps_its_named_greys_to_themselves_and_keeps_order() {
        for theme in Theme::all() {
            let palette = theme.palette();
            // Brightness stays in order: a lighter dark-theme grey is never
            // darker in another dark theme, and never lighter in a light
            // one from the cards on.
            let mut previous = None;
            for value in 0..=255u8 {
                let [r, g, b, _] = palette.tone(Color32::from_gray(value)).to_array();
                let level = luma([f32::from(r), f32::from(g), f32::from(b)]);
                if let Some(previous) = previous {
                    if palette.light {
                        if value > 27 {
                            assert!(level <= previous + 1.0, "{theme:?} at {value}");
                        }
                    } else {
                        assert!(level + 1.0 >= previous, "{theme:?} at {value}");
                    }
                }
                previous = Some(level);
            }
            for (dark, own) in [
                (original_dark().surface, palette.surface),
                (original_dark().control, palette.control),
                (original_dark().text, palette.text),
            ] {
                let toned = palette.tone(dark).to_array();
                let own = own.to_array();
                for channel in 0..3 {
                    assert!(
                        toned[channel].abs_diff(own[channel]) <= 12,
                        "{theme:?}: {toned:?} vs {own:?}"
                    );
                }
            }
            assert_eq!(palette.tone(Color32::TRANSPARENT).a(), 0);
        }
    }

    #[test]
    fn blue_accent_keeps_every_theme_exactly_and_others_replace_the_blue() {
        for theme in Theme::all() {
            let palette = theme.palette();
            assert_eq!(palette.with_accent(Accent::Blue.color()), *palette);
            assert_eq!(
                palette.accent_tone(Color32::from_rgb(72, 132, 216)),
                Color32::from_rgb(72, 132, 216)
            );
            for accent in Accent::ALL.into_iter().skip(1) {
                let colored = palette.with_accent(accent.color());
                // Greys stay the theme's own.
                assert_eq!(colored.background, palette.background);
                assert_eq!(colored.surface, palette.surface);
                assert_eq!(
                    colored.tone(original_dark().control),
                    palette.tone(original_dark().control)
                );
                // The accent takes the chosen hue; filled buttons stay darker.
                let [hue, ..] = hsl(colored.accent);
                let [chosen, ..] = hsl(accent.color());
                let turn = (hue - chosen).rem_euclid(1.0);
                assert!(turn.min(1.0 - turn) < 0.03, "{theme:?} {accent:?}");
                let lightness = |color| hsl(color)[2];
                if palette.light {
                    // A light theme's selection is a pale wash of the accent.
                    assert!(lightness(colored.selected) > lightness(colored.primary) + 0.3);
                } else {
                    assert!(lightness(colored.primary) < lightness(colored.accent));
                    assert!(lightness(colored.selected) < lightness(colored.primary));
                }
                assert_ne!(
                    colored.accent_tone(Color32::from_rgb(72, 132, 216)),
                    Color32::from_rgb(72, 132, 216)
                );
            }
        }
        // The original dark theme with the original accent.
        assert_eq!(
            Theme::default()
                .palette()
                .with_accent(Accent::default().color()),
            *original_dark()
        );
    }

    #[test]
    fn hsl_round_trips_colours() {
        for color in [
            original_dark().accent,
            original_dark().selected,
            Color32::from_rgb(214, 118, 42),
            Color32::from_gray(90),
            Color32::WHITE,
            Color32::BLACK,
        ] {
            let back = from_hsl(hsl(color), 255);
            for channel in 0..3 {
                assert!(back.to_array()[channel].abs_diff(color.to_array()[channel]) <= 1);
            }
        }
    }

    #[test]
    fn accent_codes_round_trip_and_unknown_codes_are_rejected() {
        assert_eq!(Accent::default(), Accent::Blue);
        assert_eq!(Accent::Blue.color(), original_dark().accent);
        for accent in Accent::ALL {
            assert_eq!(Accent::from_code(accent.code()), Some(accent));
            assert!(!accent.title(Language::Russian).is_empty());
        }
        assert_eq!(Accent::from_code("gold"), None);
    }

    #[test]
    fn light_theme_turns_dark_greys_light_and_light_text_dark() {
        let light = Theme::from_code("light").unwrap().palette();
        assert!(light.light);
        let level = |color: Color32| light.level_of(color);
        // The original page, cards and controls become light; its white and
        // grey text dark, still apart from each other.
        for grey in [
            original_dark().background,
            original_dark().surface,
            original_dark().control,
        ] {
            assert!(level(light.tone(grey)) > 200.0, "{grey:?}");
        }
        assert!(level(light.tone(Color32::WHITE)) < 30.0);
        assert!(level(light.tone(original_dark().muted)) < 130.0);
        assert!(
            level(light.tone(original_dark().muted))
                > level(light.tone(original_dark().text)) + 40.0
        );
        assert!(level(light.strong) < level(light.text) + 1.0);
        assert!(level(light.error) < 120.0);
    }

    #[test]
    fn a_scene_draws_in_its_theme_and_restores_the_active_one() {
        let before = active();
        let palette_before = current();
        let oled = Theme::from_code("oled").unwrap().palette();
        let light = Theme::from_code("light").unwrap().palette();
        let inside = scene(oled, || {
            let nested = scene(light, || current().light);
            (current().background, nested)
        });
        assert_eq!(inside, (Color32::BLACK, true));
        assert_eq!(active(), before);
        assert_eq!(current(), palette_before);
    }

    #[test]
    fn any_accent_colour_is_saved_by_name_or_hex_and_read_back() {
        for accent in Accent::ALL {
            assert_eq!(accent_code(accent.color()), accent.code());
            assert_eq!(parse_accent(accent.code()), Some(accent.color()));
        }
        let custom = Color32::from_rgb(0xe0, 0x1f, 0x7a);
        assert_eq!(accent_code(custom), "#e01f7a");
        assert_eq!(parse_accent("#e01f7a"), Some(custom));
        assert_eq!(parse_accent("#E01F7A"), Some(custom));
        for invalid in ["", "#", "#12345", "#1234567", "#12345g", "e01f7a", "gold"] {
            assert_eq!(parse_accent(invalid), None, "{invalid}");
        }
        // A picked colour too dark or light to read is brought into range.
        for picked in [
            Color32::BLACK,
            Color32::WHITE,
            Color32::from_rgb(255, 255, 0),
        ] {
            let accent = original_dark().with_accent(picked).accent;
            let lightness = hsl(accent)[2];
            assert!(
                (0.25..=0.7).contains(&lightness),
                "{picked:?} -> {accent:?}"
            );
        }
    }

    #[test]
    fn theme_codes_round_trip_and_unknown_codes_are_rejected() {
        for theme in Theme::all() {
            assert_eq!(Theme::from_code(theme.code()), Some(theme));
            assert!(!theme.title(Language::Russian).is_empty());
            assert!(!theme.title(Language::English).is_empty());
        }
        assert_eq!(Theme::from_code("white"), None);
        assert_eq!(Theme::from_code(""), None);
    }
}
