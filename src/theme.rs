use ratatui::style::Color;

use crate::config::ConfigKey;

/// Built-in color themes. `Terminal` maps the classic green/yellow/red layout
/// onto ANSI palette colors so the terminal's own theme decides the exact look.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeId {
    #[default]
    Terminal,
    Classic,
    CatppuccinMocha,
    Gruvbox,
    Nord,
    TokyoNight,
}

type Rgb = (u8, u8, u8);

/// Gradient stops as (position 0..1, color). Positions must be ascending.
enum Palette {
    /// True-color gradient, linearly interpolated between stops.
    Gradient(&'static [(f32, Rgb)]),
    /// Discrete ANSI steps: the last stop at or below `t` wins.
    Steps(&'static [(f32, Color)]),
}

const TERMINAL: Palette = Palette::Steps(&[
    (0.00, Color::Green),
    (0.60, Color::Yellow),
    (0.85, Color::Red),
]);

const CLASSIC: Palette = Palette::Gradient(&[
    (0.00, (0x00, 0xb4, 0x28)),
    (0.55, (0x18, 0xd8, 0x30)),
    (0.75, (0xe8, 0xe0, 0x00)),
    (0.90, (0xff, 0x90, 0x00)),
    (1.00, (0xff, 0x30, 0x30)),
]);

const CATPPUCCIN: Palette = Palette::Gradient(&[
    (0.00, (0x89, 0xb4, 0xfa)), // blue
    (0.25, (0x94, 0xe2, 0xd5)), // teal
    (0.50, (0xa6, 0xe3, 0xa1)), // green
    (0.70, (0xf9, 0xe2, 0xaf)), // yellow
    (0.85, (0xfa, 0xb3, 0x87)), // peach
    (1.00, (0xf3, 0x8b, 0xa8)), // red
]);

const GRUVBOX: Palette = Palette::Gradient(&[
    (0.00, (0x98, 0x97, 0x1a)),
    (0.45, (0xb8, 0xbb, 0x26)),
    (0.70, (0xfa, 0xbd, 0x2f)),
    (0.88, (0xfe, 0x80, 0x19)),
    (1.00, (0xfb, 0x49, 0x34)),
]);

const NORD: Palette = Palette::Gradient(&[
    (0.00, (0x5e, 0x81, 0xac)),
    (0.35, (0x88, 0xc0, 0xd0)),
    (0.60, (0xa3, 0xbe, 0x8c)),
    (0.80, (0xeb, 0xcb, 0x8b)),
    (0.92, (0xd0, 0x87, 0x70)),
    (1.00, (0xbf, 0x61, 0x6a)),
]);

const TOKYO_NIGHT: Palette = Palette::Gradient(&[
    (0.00, (0x73, 0xda, 0xca)),
    (0.35, (0x7a, 0xa2, 0xf7)),
    (0.65, (0xbb, 0x9a, 0xf7)),
    (0.85, (0xff, 0x9e, 0x64)),
    (1.00, (0xf7, 0x76, 0x8e)),
]);

impl ThemeId {
    pub const ALL: [ThemeId; 6] = [
        ThemeId::Terminal,
        ThemeId::Classic,
        ThemeId::CatppuccinMocha,
        ThemeId::Gruvbox,
        ThemeId::Nord,
        ThemeId::TokyoNight,
    ];

    pub fn next(self) -> ThemeId {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ThemeId::Terminal => "Terminal",
            ThemeId::Classic => "Classic hi-fi",
            ThemeId::CatppuccinMocha => "Catppuccin Mocha",
            ThemeId::Gruvbox => "Gruvbox",
            ThemeId::Nord => "Nord",
            ThemeId::TokyoNight => "Tokyo Night",
        }
    }

    fn palette(self) -> &'static Palette {
        match self {
            ThemeId::Terminal => &TERMINAL,
            ThemeId::Classic => &CLASSIC,
            ThemeId::CatppuccinMocha => &CATPPUCCIN,
            ThemeId::Gruvbox => &GRUVBOX,
            ThemeId::Nord => &NORD,
            ThemeId::TokyoNight => &TOKYO_NIGHT,
        }
    }

    /// Color of a bar cell at relative height `t` (0 = bottom, 1 = top).
    pub fn bar_color(self, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        match self.palette() {
            Palette::Steps(steps) => {
                let mut color = steps[0].1;
                for &(pos, c) in *steps {
                    if t >= pos {
                        color = c;
                    }
                }
                color
            }
            Palette::Gradient(stops) => {
                let mut prev = stops[0];
                for &stop in *stops {
                    if t <= stop.0 {
                        let span = (stop.0 - prev.0).max(1e-6);
                        let k = (t - prev.0) / span;
                        return lerp_rgb(prev.1, stop.1, k);
                    }
                    prev = stop;
                }
                let last = stops[stops.len() - 1].1;
                Color::Rgb(last.0, last.1, last.2)
            }
        }
    }

    /// Color of the falling peak-hold cap.
    pub fn peak_color(self) -> Color {
        match self {
            ThemeId::Terminal => Color::White,
            ThemeId::Classic => Color::Rgb(0xf0, 0xf0, 0xf0),
            ThemeId::CatppuccinMocha => Color::Rgb(0xb4, 0xbe, 0xfe),
            ThemeId::Gruvbox => Color::Rgb(0xeb, 0xdb, 0xb2),
            ThemeId::Nord => Color::Rgb(0xec, 0xef, 0xf4),
            ThemeId::TokyoNight => Color::Rgb(0xc0, 0xca, 0xf5),
        }
    }

    /// Accent color for UI chrome (title, highlights).
    pub fn accent(self) -> Color {
        match self {
            ThemeId::Terminal => Color::Cyan,
            ThemeId::Classic => Color::Rgb(0x18, 0xd8, 0x30),
            ThemeId::CatppuccinMocha => Color::Rgb(0xcb, 0xa6, 0xf7),
            ThemeId::Gruvbox => Color::Rgb(0xfa, 0xbd, 0x2f),
            ThemeId::Nord => Color::Rgb(0x88, 0xc0, 0xd0),
            ThemeId::TokyoNight => Color::Rgb(0x7a, 0xa2, 0xf7),
        }
    }

    /// Muted color for secondary text (footer hints, labels).
    pub fn dim(self) -> Color {
        match self {
            ThemeId::Terminal => Color::DarkGray,
            ThemeId::Classic => Color::Rgb(0x60, 0x70, 0x60),
            ThemeId::CatppuccinMocha => Color::Rgb(0x6c, 0x70, 0x86),
            ThemeId::Gruvbox => Color::Rgb(0x92, 0x83, 0x74),
            ThemeId::Nord => Color::Rgb(0x4c, 0x56, 0x6a),
            ThemeId::TokyoNight => Color::Rgb(0x56, 0x5f, 0x89),
        }
    }
}

impl ConfigKey for ThemeId {
    fn from_key(s: &str) -> Option<ThemeId> {
        match s.to_ascii_lowercase().as_str() {
            "terminal" | "ansi" => Some(ThemeId::Terminal),
            "classic" | "hifi" | "hi-fi" => Some(ThemeId::Classic),
            "catppuccin" | "catppuccin-mocha" | "mocha" => Some(ThemeId::CatppuccinMocha),
            "gruvbox" => Some(ThemeId::Gruvbox),
            "nord" => Some(ThemeId::Nord),
            "tokyo-night" | "tokyonight" | "tokyo" => Some(ThemeId::TokyoNight),
            _ => None,
        }
    }

    /// Stable identifier used in the config file and --theme flag.
    fn key(self) -> &'static str {
        match self {
            ThemeId::Terminal => "terminal",
            ThemeId::Classic => "classic",
            ThemeId::CatppuccinMocha => "catppuccin",
            ThemeId::Gruvbox => "gruvbox",
            ThemeId::Nord => "nord",
            ThemeId::TokyoNight => "tokyo-night",
        }
    }
}

/// Scale an RGB color's brightness by `k` (0..1). ANSI palette colors pass
/// through unchanged — the terminal owns their exact values.
pub fn dim_color(c: Color, k: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let k = k.clamp(0.0, 1.0);
            Color::Rgb(
                (r as f32 * k).round() as u8,
                (g as f32 * k).round() as u8,
                (b as f32 * k).round() as u8,
            )
        }
        other => other,
    }
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Color {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}
