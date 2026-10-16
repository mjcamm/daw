//! Theme tokens from `docs/design/look-and-feel.md`. Views never hard-code a
//! colour; they read one of these.

use daw_host::preferences::Appearance;
use gpui::{App, Global, Hsla, Rgba, WindowAppearance, rgb, rgba};

/// One content colour (for nodes, clips, signals) as it appears in a theme.
#[derive(Debug, Clone, Copy)]
pub struct ContentColor {
    /// Soft background of a clip or block.
    pub fill: Hsla,
    /// Thin outline around it.
    pub outline: Hsla,
    /// Notes, curves, labels and swatches drawn on or for it.
    pub detail: Hsla,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code, reason = "the full token set; later pages use the rest")]
pub struct Theme {
    pub dark: bool,
    pub window: Hsla,
    pub sidebar: Hsla,
    pub group: Hsla,
    pub hover: Hsla,
    pub control: Hsla,
    pub control_selected: Hsla,
    pub hairline: Hsla,
    pub grid: Hsla,
    pub edge: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub record: Hsla,
    pub active: Hsla,
    pub warning: Hsla,
    pub on_warning: Hsla,
    /// Dims the page behind a modal.
    pub scrim: Hsla,
    pub meter_low: Hsla,
    pub meter_high: Hsla,
    /// The piano roll's keyboard: white and black keys, as on a piano in
    /// either theme (slightly softened in dark).
    pub key_white: Hsla,
    pub key_black: Hsla,
    pub palette: [ContentColor; 9],
    pub muted_content: ContentColor,
}

impl Global for Theme {}

/// The light theme pairs a pale fill with a deep detail colour.
const LIGHT_PALETTE: [(u32, u32); 9] = [
    (0xFBDCD6, 0xC2402B),
    (0xFCE6CC, 0xB4600F),
    (0xF6EDC2, 0x86700A),
    (0xD9F0D2, 0x2E7D32),
    (0xCFEDEA, 0x127567),
    (0xD3E5FA, 0x1E62B0),
    (0xE2DDFA, 0x5946C9),
    (0xF7D9EF, 0xA23488),
    (0xE8E8EA, 0x5E5E63),
];

/// The dark theme uses one bright colour, tinted for fills and outlines.
const DARK_PALETTE: [u32; 9] = [
    0xFF8A75, 0xFFB061, 0xE8CF5A, 0x7DD87A, 0x5FD3C2, 0x6AAFFF, 0xA99BFF, 0xF28AD8, 0xA1A1A6,
];

fn hsla(color: Rgba) -> Hsla {
    color.into()
}

fn with_alpha(color: u32, alpha: f32) -> Hsla {
    let mut c = hsla(rgb(color));
    c.a = alpha;
    c
}

fn bright(color: u32) -> ContentColor {
    ContentColor {
        fill: with_alpha(color, 0.18),
        outline: with_alpha(color, 0.35),
        detail: hsla(rgb(color)),
    }
}

impl Theme {
    /// The theme for a preference: light, dark, or following the window's
    /// (the system's) appearance.
    pub fn for_preference(preference: Appearance, window: WindowAppearance) -> Self {
        match preference {
            Appearance::System => Self::for_appearance(window),
            Appearance::Light => Self::light(),
            Appearance::Dark => Self::dark(),
        }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            window: hsla(rgb(0x1E1E1E)),
            sidebar: hsla(rgb(0x262628)),
            group: hsla(rgb(0x2C2C2E)),
            hover: hsla(rgba(0xFFFF_FF14)),
            control: hsla(rgba(0xFFFF_FF1A)),
            control_selected: hsla(rgba(0xFFFF_FF24)),
            hairline: hsla(rgba(0xFFFF_FF17)),
            grid: hsla(rgba(0xFFFF_FF0A)),
            edge: hsla(rgba(0x0000_0099)),
            text: hsla(rgba(0xFFFF_FFDB)),
            text_secondary: hsla(rgba(0xFFFF_FF80)),
            text_tertiary: hsla(rgba(0xFFFF_FF4D)),
            accent: hsla(rgb(0x0A84FF)),
            on_accent: hsla(rgb(0xFFFFFF)),
            record: hsla(rgb(0xFF453A)),
            active: hsla(rgb(0x30D158)),
            warning: hsla(rgb(0xFF9F0A)),
            on_warning: hsla(rgb(0x000000)),
            scrim: hsla(rgba(0x0000_0080)),
            meter_low: hsla(rgb(0x30D158)),
            meter_high: hsla(rgb(0xFF453A)),
            key_white: hsla(rgb(0xC8C8CC)),
            key_black: hsla(rgb(0x1A1A1C)),
            palette: DARK_PALETTE.map(bright),
            muted_content: ContentColor {
                fill: with_alpha(0x6E6E73, 0.1),
                outline: with_alpha(0x6E6E73, 0.35),
                detail: hsla(rgb(0x6E6E73)),
            },
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            window: hsla(rgb(0xFFFFFF)),
            sidebar: hsla(rgb(0xF5F5F7)),
            group: hsla(rgb(0xFFFFFF)),
            hover: hsla(rgba(0x0000_000D)),
            control: hsla(rgba(0x0000_000F)),
            control_selected: hsla(rgb(0xFFFFFF)),
            hairline: hsla(rgba(0x0000_001A)),
            grid: hsla(rgba(0x0000_000A)),
            edge: hsla(rgba(0x0000_002E)),
            text: hsla(rgba(0x0000_00D9)),
            text_secondary: hsla(rgba(0x0000_0080)),
            text_tertiary: hsla(rgba(0x0000_0040)),
            accent: hsla(rgb(0x007AFF)),
            on_accent: hsla(rgb(0xFFFFFF)),
            record: hsla(rgb(0xFF3B30)),
            active: hsla(rgb(0x34C759)),
            warning: hsla(rgb(0xFF9500)),
            on_warning: hsla(rgb(0xFFFFFF)),
            scrim: hsla(rgba(0x0000_0040)),
            meter_low: hsla(rgb(0x34C759)),
            meter_high: hsla(rgb(0xFF3B30)),
            key_white: hsla(rgb(0xFFFFFF)),
            key_black: hsla(rgb(0x2C2C2E)),
            palette: LIGHT_PALETTE.map(|(fill, deep)| ContentColor {
                fill: hsla(rgb(fill)),
                outline: with_alpha(deep, 0.3),
                detail: hsla(rgb(deep)),
            }),
            muted_content: ContentColor {
                fill: hsla(rgb(0xF0F0F2)),
                outline: with_alpha(0x5E5E63, 0.2),
                detail: with_alpha(0x5E5E63, 0.6),
            },
        }
    }

    /// The content colour at a palette index (wrapping).
    pub fn content(&self, index: u8) -> ContentColor {
        self.palette[usize::from(index) % self.palette.len()]
    }

    pub fn get(cx: &App) -> Self {
        *cx.global::<Self>()
    }
}

/// The platform's UI font.
pub const UI_FONT: &str = if cfg!(target_os = "windows") {
    "Segoe UI"
} else if cfg!(target_os = "macos") {
    ".SystemUIFont"
} else {
    "Inter"
};

/// Monospaced font with tabular figures, for time, dB and Hz.
pub const MONO_FONT: &str = if cfg!(target_os = "windows") {
    "Cascadia Mono"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};
