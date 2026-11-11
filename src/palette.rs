//! Appearance themes: the colours every page, panel and widget is drawn in.
//!
//! The active theme is one global choice, read by `theme`'s colour
//! accessors on every frame, so a switch in Settings repaints everything at
//! once without a restart. The dark theme is the original look of VK Fast:
//! its values are exactly the colours the app always had.

use std::{
    cell::Cell,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
};

use eframe::egui::Color32;

use crate::i18n::Language;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    /// The original dark blue-grey look.
    #[default]
    Dark,
    /// Pure black page and panels: an OLED screen switches those pixels off.
    Oled,
    /// Softer, lighter neutral greys without the blue cast.
    Graphite,
    /// A deeper navy blue.
    Midnight,
    /// White cards and panels on a light grey page, dark text.
    Light,
}

impl Theme {
    pub const ALL: [Self; 5] = [
        Self::Dark,
        Self::Oled,
        Self::Graphite,
        Self::Midnight,
        Self::Light,
    ];

    /// The value saved in `preferences.json` and written to the logs.
    pub fn code(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Oled => "oled",
            Self::Graphite => "graphite",
            Self::Midnight => "midnight",
            Self::Light => "light",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.code() == code)
    }

    pub fn title(self, language: Language) -> &'static str {
        match self {
            Self::Dark => language.text("Тёмная", "Dark"),
            Self::Oled => language.text("Чёрная (OLED)", "Black (OLED)"),
            Self::Graphite => language.text("Графит", "Graphite"),
            Self::Midnight => language.text("Полночь", "Midnight"),
            Self::Light => language.text("Светлая", "Light"),
        }
    }

    pub fn palette(self) -> &'static Palette {
        match self {
            Self::Dark => &DARK,
            Self::Oled => &OLED,
            Self::Graphite => &GRAPHITE,
            Self::Midnight => &MIDNIGHT,
            Self::Light => &LIGHT,
        }
    }

    fn index(self) -> u8 {
        Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0) as u8
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

/// How `accent` is saved: a ready accent by name, any other colour as
/// `#rrggbb`.
pub fn accent_code(accent: Color32) -> String {
    Accent::of(accent).map_or_else(
        || format!("#{:02x}{:02x}{:02x}", accent.r(), accent.g(), accent.b()),
        |accent| accent.code().to_owned(),
    )
}

/// The named colours of a theme. Greys drawn by individual widgets that
/// have no name here follow the theme through [`Palette::tone`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Links, the playing row, the passed part of sliders.
    pub accent: Color32,
    /// Secondary text and icons.
    pub muted: Color32,
    /// Main text.
    pub text: Color32,
    /// The brightest text and icons: white in dark themes.
    pub strong: Color32,
    /// Error and warning text.
    pub error: Color32,
    pub warning: Color32,
    /// Light themes draw dark text on light surfaces.
    pub light: bool,
    /// The page behind everything.
    pub background: Color32,
    /// The sidebar, player bar and the lyrics/queue panel.
    pub panel: Color32,
    /// The line between panels and the page; none where their fills differ.
    pub panel_line: Option<Color32>,
    /// Cards, fields, menus and dialogs.
    pub surface: Color32,
    /// Filled buttons: Play, switches that are on.
    pub primary: Color32,
    /// The selected sidebar item, pressed controls, hovered caption buttons.
    pub selected: Color32,
    pub scroll_handle: Color32,
    pub scroll_hovered: Color32,
    pub scroll_active: Color32,
    /// Text fields.
    pub field: Color32,
    pub link: Color32,
    /// Buttons and other controls at rest, their outline, and when hovered.
    pub control: Color32,
    pub control_stroke: Color32,
    pub control_hovered: Color32,
    pub control_hovered_stroke: Color32,
    /// How much of the original greys' blue cast is kept (1 = all of it).
    chroma: f32,
    /// A cast added to greys, strongest in the darkest ones.
    tint: [f32; 3],
    /// The accent the blue colours above were turned to.
    accent_color: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The original accent.
const BLUE: Color32 = rgb(54, 112, 196);

pub const DARK: Palette = Palette {
    accent: rgb(54, 112, 196),
    muted: rgb(156, 166, 181),
    text: rgb(231, 235, 242),
    strong: Color32::WHITE,
    error: rgb(255, 158, 153),
    warning: rgb(255, 178, 120),
    light: false,
    background: rgb(15, 17, 21),
    panel: rgb(23, 27, 33),
    panel_line: None,
    surface: rgb(23, 27, 33),
    primary: rgb(34, 77, 139),
    selected: rgb(27, 44, 67),
    scroll_handle: rgb(57, 65, 77),
    scroll_hovered: rgb(75, 85, 99),
    scroll_active: rgb(91, 104, 122),
    field: rgb(18, 21, 27),
    link: rgb(98, 148, 219),
    control: rgb(32, 38, 47),
    control_stroke: rgb(43, 50, 61),
    control_hovered: rgb(42, 50, 63),
    control_hovered_stroke: rgb(69, 87, 112),
    chroma: 1.0,
    tint: [0.0; 3],
    accent_color: BLUE,
};

const OLED: Palette = Palette {
    accent: rgb(54, 112, 196),
    muted: rgb(150, 156, 166),
    text: rgb(231, 235, 242),
    strong: Color32::WHITE,
    error: rgb(255, 158, 153),
    warning: rgb(255, 178, 120),
    light: false,
    background: rgb(0, 0, 0),
    panel: rgb(0, 0, 0),
    panel_line: Some(rgb(34, 35, 39)),
    surface: rgb(17, 18, 20),
    primary: rgb(34, 77, 139),
    selected: rgb(22, 38, 61),
    scroll_handle: rgb(50, 52, 57),
    scroll_hovered: rgb(70, 73, 80),
    scroll_active: rgb(88, 92, 100),
    field: rgb(8, 8, 9),
    link: rgb(98, 148, 219),
    control: rgb(29, 30, 34),
    control_stroke: rgb(42, 43, 48),
    control_hovered: rgb(40, 42, 47),
    control_hovered_stroke: rgb(66, 80, 102),
    chroma: 0.3,
    tint: [0.0; 3],
    accent_color: BLUE,
};

const GRAPHITE: Palette = Palette {
    accent: rgb(66, 124, 208),
    muted: rgb(166, 169, 176),
    text: rgb(234, 235, 238),
    strong: Color32::WHITE,
    error: rgb(255, 158, 153),
    warning: rgb(255, 178, 120),
    light: false,
    background: rgb(32, 33, 36),
    panel: rgb(41, 42, 46),
    panel_line: None,
    surface: rgb(41, 42, 46),
    primary: rgb(40, 86, 150),
    selected: rgb(45, 60, 82),
    scroll_handle: rgb(76, 78, 84),
    scroll_hovered: rgb(96, 99, 106),
    scroll_active: rgb(114, 118, 126),
    field: rgb(36, 37, 40),
    link: rgb(108, 156, 224),
    control: rgb(52, 54, 59),
    control_stroke: rgb(64, 66, 72),
    control_hovered: rgb(62, 64, 70),
    control_hovered_stroke: rgb(92, 104, 124),
    chroma: 0.25,
    tint: [0.0; 3],
    accent_color: BLUE,
};

const MIDNIGHT: Palette = Palette {
    accent: rgb(58, 118, 210),
    muted: rgb(150, 164, 190),
    text: rgb(228, 234, 246),
    strong: Color32::WHITE,
    error: rgb(255, 158, 153),
    warning: rgb(255, 178, 120),
    light: false,
    background: rgb(9, 13, 24),
    panel: rgb(15, 21, 37),
    panel_line: None,
    surface: rgb(15, 21, 37),
    primary: rgb(34, 77, 150),
    selected: rgb(24, 44, 80),
    scroll_handle: rgb(46, 58, 82),
    scroll_hovered: rgb(62, 77, 106),
    scroll_active: rgb(78, 95, 128),
    field: rgb(12, 17, 30),
    link: rgb(104, 152, 228),
    control: rgb(24, 33, 52),
    control_stroke: rgb(36, 47, 70),
    control_hovered: rgb(34, 45, 68),
    control_hovered_stroke: rgb(64, 86, 124),
    chroma: 1.5,
    tint: [-3.0, -1.0, 6.0],
    accent_color: BLUE,
};

const LIGHT: Palette = Palette {
    accent: rgb(40, 98, 184),
    muted: rgb(96, 104, 118),
    text: rgb(26, 29, 35),
    strong: rgb(12, 14, 18),
    error: rgb(186, 38, 46),
    warning: rgb(164, 88, 8),
    light: true,
    background: rgb(240, 242, 245),
    panel: rgb(255, 255, 255),
    panel_line: Some(rgb(221, 224, 230)),
    surface: rgb(255, 255, 255),
    primary: rgb(46, 102, 186),
    selected: rgb(216, 229, 248),
    scroll_handle: rgb(196, 201, 209),
    scroll_hovered: rgb(172, 178, 188),
    scroll_active: rgb(148, 155, 167),
    field: rgb(246, 247, 249),
    link: rgb(32, 92, 178),
    control: rgb(233, 236, 241),
    control_stroke: rgb(212, 216, 223),
    control_hovered: rgb(222, 226, 233),
    control_hovered_stroke: rgb(140, 163, 198),
    chroma: 0.5,
    tint: [0.0; 3],
    accent_color: BLUE,
};

static ACTIVE: AtomicU8 = AtomicU8::new(0);
/// The accent as `0x00rrggbb`.
static ACCENT: AtomicU32 = AtomicU32::new(0x0036_70c4);

thread_local! {
    /// A theme drawn instead of the active one for a while on this thread:
    /// scenes over covers stay dark in a light theme.
    static SCENE: Cell<Option<Theme>> = const { Cell::new(None) };
}

/// Draw `contents` in `theme`'s colours (with the active accent) on this
/// thread, then return to the active theme.
pub fn scene<R>(theme: Theme, contents: impl FnOnce() -> R) -> R {
    struct Restore(Option<Theme>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCENE.with(|scene| scene.set(self.0));
        }
    }
    let _restore = Restore(SCENE.with(|scene| scene.replace(Some(theme))));
    contents()
}

/// The theme the app is drawn in now.
pub fn active() -> Theme {
    if let Some(theme) = SCENE.with(Cell::get) {
        return theme;
    }
    Theme::ALL
        .get(usize::from(ACTIVE.load(Ordering::Relaxed)))
        .copied()
        .unwrap_or_default()
}

/// Draw everything in `theme` from the next paint on. `theme::apply` also
/// rebuilds egui's own widget colours.
pub fn set_active(theme: Theme) {
    ACTIVE.store(theme.index(), Ordering::Relaxed);
}

/// The accent the app is drawn with now.
pub fn active_accent() -> Color32 {
    let [_, r, g, b] = ACCENT.load(Ordering::Relaxed).to_be_bytes();
    Color32::from_rgb(r, g, b)
}

/// Draw with `accent` instead of the theme's blue from the next paint on.
pub fn set_accent(accent: Color32) {
    ACCENT.store(
        u32::from_be_bytes([0, accent.r(), accent.g(), accent.b()]),
        Ordering::Relaxed,
    );
}

/// The colours of the active theme with the active accent.
pub fn current() -> Palette {
    active().palette().with_accent(active_accent())
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

impl Palette {
    /// This theme with `accent` in place of its blue. Blue keeps the theme's
    /// own colours exactly.
    pub fn with_accent(&self, accent: Color32) -> Self {
        if accent == BLUE {
            return *self;
        }
        let mut palette = *self;
        palette.accent = palette.hue(self.accent, accent);
        palette.primary = palette.hue(self.primary, accent);
        palette.link = palette.hue(self.link, accent);
        palette.selected = palette.hue(self.selected, accent);
        palette.control_hovered_stroke = palette.hue(self.control_hovered_stroke, accent);
        palette.accent_color = accent;
        palette
    }

    /// `color`, a colour of the blue accent family (a lighter blue, a dark
    /// blue backdrop), in the same place of the active accent's family: its
    /// hue turned by as much as the accent's, its saturation scaled alike.
    pub fn accent_tone(&self, color: Color32) -> Color32 {
        self.hue(color, self.accent_color)
    }

    fn hue(&self, color: Color32, accent: Color32) -> Color32 {
        if accent == BLUE {
            return color;
        }
        let [blue_hue, blue_saturation, blue_lightness] = hsl(BLUE);
        let [hue, saturation, accent_lightness] = hsl(accent);
        // Any colour can be picked: very dark or light ones are brought to a
        // brightness that still reads on the page and under white text.
        let accent_lightness = accent_lightness.clamp(0.3, 0.66);
        let [own_hue, own_saturation, lightness] = hsl(color);
        let saturation =
            (own_saturation * saturation / blue_saturation.max(f32::EPSILON)).clamp(0.0, 1.0);
        // The blue accent's lightness becomes the new accent's; darker and
        // lighter shades keep their place between it and black or white.
        let lightness = if lightness <= blue_lightness {
            lightness * accent_lightness / blue_lightness
        } else {
            accent_lightness
                + (lightness - blue_lightness) * (1.0 - accent_lightness) / (1.0 - blue_lightness)
        };
        from_hsl([own_hue + hue - blue_hue, saturation, lightness], color.a())
    }

    /// The colour that takes the place of `color`, a grey of the dark theme,
    /// in this theme: the same place between this theme's own background,
    /// cards, controls and text as `color` has in the dark theme. Accent and
    /// cover colours are not toned. The dark theme returns `color` itself.
    pub fn tone(&self, color: Color32) -> Color32 {
        if self.chroma == DARK.chroma
            && self.tint == DARK.tint
            && [
                self.background,
                self.surface,
                self.control,
                self.control_stroke,
                self.scroll_active,
                self.muted,
                self.text,
            ] == [
                DARK.background,
                DARK.surface,
                DARK.control,
                DARK.control_stroke,
                DARK.scroll_active,
                DARK.muted,
                DARK.text,
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
            pair(DARK.background, self.background),
            pair(DARK.surface, self.surface),
            pair(DARK.control, self.control),
            pair(DARK.control_stroke, self.control_stroke),
            pair(DARK.scroll_active, self.scroll_active),
            pair(DARK.muted, self.muted),
            pair(DARK.text, self.text),
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
        assert_eq!(Theme::default(), Theme::Dark);
        let dark = Theme::Dark.palette();
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
        let oled = Theme::Oled.palette();
        assert_eq!(oled.background, Color32::BLACK);
        assert_eq!(oled.panel, Color32::BLACK);
        // Everything as dark as the original page or darker is off.
        assert_eq!(oled.tone(DARK.background), Color32::BLACK);
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
    fn every_theme_maps_its_named_greys_to_themselves_and_keeps_order() {
        for theme in Theme::ALL {
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
                (DARK.surface, palette.surface),
                (DARK.control, palette.control),
                (DARK.text, palette.text),
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
        for theme in Theme::ALL {
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
                assert_eq!(colored.tone(DARK.control), palette.tone(DARK.control));
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
            Theme::Dark.palette().with_accent(Accent::default().color()),
            DARK
        );
    }

    #[test]
    fn hsl_round_trips_colours() {
        for color in [
            DARK.accent,
            DARK.selected,
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
        assert_eq!(Accent::Blue.color(), DARK.accent);
        for accent in Accent::ALL {
            assert_eq!(Accent::from_code(accent.code()), Some(accent));
            assert!(!accent.title(Language::Russian).is_empty());
        }
        assert_eq!(Accent::from_code("gold"), None);
    }

    #[test]
    fn light_theme_turns_dark_greys_light_and_light_text_dark() {
        let light = Theme::Light.palette();
        assert!(light.light);
        let level = |color: Color32| light.level_of(color);
        // The original page, cards and controls become light; its white and
        // grey text dark, still apart from each other.
        for grey in [DARK.background, DARK.surface, DARK.control] {
            assert!(level(light.tone(grey)) > 200.0, "{grey:?}");
        }
        assert!(level(light.tone(Color32::WHITE)) < 30.0);
        assert!(level(light.tone(DARK.muted)) < 130.0);
        assert!(level(light.tone(DARK.muted)) > level(light.tone(DARK.text)) + 40.0);
        assert!(level(light.strong) < level(light.text) + 1.0);
        assert!(level(light.error) < 120.0);
    }

    #[test]
    fn a_scene_draws_in_its_theme_and_restores_the_active_one() {
        let before = active();
        let inside = scene(Theme::Oled, || {
            let nested = scene(Theme::Light, || current().light);
            (active(), current().background, nested)
        });
        assert_eq!(inside, (Theme::Oled, Color32::BLACK, true));
        assert_eq!(active(), before);
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
            let accent = DARK.with_accent(picked).accent;
            let lightness = hsl(accent)[2];
            assert!(
                (0.25..=0.7).contains(&lightness),
                "{picked:?} -> {accent:?}"
            );
        }
    }

    #[test]
    fn theme_codes_round_trip_and_unknown_codes_are_rejected() {
        for theme in Theme::ALL {
            assert_eq!(Theme::from_code(theme.code()), Some(theme));
            assert!(!theme.title(Language::Russian).is_empty());
            assert!(!theme.title(Language::English).is_empty());
        }
        assert_eq!(Theme::from_code("white"), None);
        assert_eq!(Theme::from_code(""), None);
    }
}
