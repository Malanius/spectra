pub mod bars;
pub mod fire;
pub mod fractal;
pub mod matrix;
pub mod mirror;
pub mod particles;
pub mod radial;
pub mod ripples;
pub mod scope;
pub mod skyline;
pub mod spectrogram;
pub mod vector;
pub mod vu;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::config::Config;
use crate::theme::ThemeId;

/// One analysis window of audio, shared by all visualizer styles.
pub struct AudioFrame<'a> {
    pub mono: &'a [f32],
    pub left: &'a [f32],
    pub right: &'a [f32],
    pub rate: u32,
    /// True when no fresh samples are arriving (paused / suspended stream).
    pub silent: bool,
}

/// A visualization style. To add a new one: implement this trait, register
/// it in [`create_all`], and add its id to [`IDS`].
pub trait Visualizer {
    /// Stable identifier used in the config file and --style flag.
    fn id(&self) -> &'static str;
    /// Human-readable name shown in the header and status line.
    fn name(&self) -> &'static str;
    /// Advance state by `dt` seconds. `max_bars` is how many bars currently
    /// fit on screen; styles that don't draw bars may ignore it.
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, max_bars: usize, dt: f32);
    /// Draw into `area`. Must not assume any particular minimum size.
    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config);
}

/// Ids of all styles, in cycle order (single source for CLI validation).
pub const IDS: [&str; 13] = [
    "bars",
    "mirror",
    "ripples",
    "fractal",
    "spectrogram",
    "scope",
    "vector",
    "vu",
    "fire",
    "particles",
    "matrix",
    "radial",
    "skyline",
];

/// Instantiate every visualizer, in the same order as [`IDS`].
pub fn create_all(cfg: &Config) -> Vec<Box<dyn Visualizer>> {
    vec![
        Box::new(bars::Bars::new(cfg)),
        Box::new(mirror::Mirror::new(cfg)),
        Box::new(ripples::Ripples::new(cfg)),
        Box::new(fractal::Fractal::new(cfg)),
        Box::new(spectrogram::Spectrogram::new(cfg)),
        Box::new(scope::Scope::new(cfg)),
        Box::new(vector::Vector::new(cfg)),
        Box::new(vu::Vu::new(cfg)),
        Box::new(fire::Fire::new(cfg)),
        Box::new(particles::Particles::new(cfg)),
        Box::new(matrix::Matrix::new(cfg)),
        Box::new(radial::Radial::new(cfg)),
        Box::new(skyline::Skyline::new(cfg)),
    ]
}

/// Lower-eighth block characters, index i covers (i+1)/8 of the cell.
pub(crate) const EIGHTHS: [&str; 7] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇"];

/// Draw `sym` across a horizontal span of `width` cells, clipped to `area`.
pub(crate) fn put_hspan(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    width: u16,
    y: u16,
    sym: &str,
    style: Style,
) {
    if y < area.y || y >= area.y + area.height {
        return;
    }
    for dx in 0..width {
        let px = x + dx;
        if px >= area.x && px < area.x + area.width {
            buf[(px, y)].set_symbol(sym).set_style(style);
        }
    }
}

/// Horizontal placement shared by the bar-based styles: returns
/// (x of first bar, cell stride between bars).
pub(crate) fn bar_layout(area: Rect, n: usize, cfg: &Config) -> (u16, u16) {
    let stride = cfg.bar_width + cfg.gap;
    let total = (n as u16) * stride - cfg.gap.min(stride);
    (area.x + area.width.saturating_sub(total) / 2, stride)
}

/// xorshift64* — deterministic, dependency-free randomness.
pub(crate) struct Rng(pub u64);

impl Rng {
    pub fn f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0.wrapping_mul(0x2545f4914f6cdd1d) >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// Detects sudden jumps of a level above its recent average — used to turn
/// bass energy into discrete "beat" events. `ratio` is how far above the
/// running average a level must jump; `floor` is the minimum absolute level.
pub(crate) struct OnsetDetector {
    ema: f32,
    cooldown: f32,
    ratio: f32,
    floor: f32,
    /// Minimum seconds between onsets (refractory period).
    refractory: f32,
}

impl OnsetDetector {
    pub fn new(ratio: f32, floor: f32, refractory: f32) -> Self {
        Self {
            ema: 0.0,
            cooldown: 0.0,
            ratio,
            floor,
            refractory,
        }
    }

    /// Feed the current level (0..1); returns the level on an onset.
    pub fn feed(&mut self, level: f32, silent: bool, dt: f32) -> Option<f32> {
        let ema = self.ema;
        self.ema += (level - ema) * (dt / 0.6).min(1.0);
        self.cooldown -= dt;
        if !silent && level > self.floor && level > ema * self.ratio + 0.03 && self.cooldown <= 0.0
        {
            self.cooldown = self.refractory;
            Some(level)
        } else {
            None
        }
    }
}

/// Maps a dB level to 0..1 with the same manual-gain / auto-gain semantics
/// as the spectrum analyzer, for styles that work on raw levels.
pub(crate) struct LevelNorm {
    ceil_db: f32,
}

impl LevelNorm {
    pub fn new() -> Self {
        Self { ceil_db: -20.0 }
    }

    pub fn norm(&mut self, db: f32, auto: bool, gain_db: f32, range: f32, dt: f32) -> f32 {
        if auto && db > -70.0 {
            let k = if db > self.ceil_db {
                (dt * 8.0).min(1.0)
            } else {
                (dt * 0.15).min(1.0)
            };
            self.ceil_db += (db - self.ceil_db) * k;
            self.ceil_db = self.ceil_db.clamp(-50.0, 0.0);
        }
        let ceil = if auto { self.ceil_db + 2.0 } else { 0.0 } - gain_db;
        ((db - (ceil - range)) / range).clamp(0.0, 1.0)
    }
}

pub(crate) fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let sq: f32 = samples.iter().map(|s| s * s).sum();
    20.0 * ((sq / samples.len() as f32).sqrt() + 1e-9).log10()
}

/// A 2×4-dots-per-cell braille drawing surface. Braille dots are roughly
/// square on screen, which makes it the right tool for curves and scatter
/// plots. Each cell remembers the strongest intensity drawn into it; `blit`
/// maps that intensity to a color.
pub(crate) struct BrailleCanvas {
    wc: usize,
    hc: usize,
    bits: Vec<u8>,
    intensity: Vec<f32>,
}

const BRAILLE_BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

impl BrailleCanvas {
    pub fn new(cells_w: u16, cells_h: u16) -> Self {
        let (wc, hc) = (cells_w as usize, cells_h as usize);
        Self {
            wc,
            hc,
            bits: vec![0; wc * hc],
            intensity: vec![0.0; wc * hc],
        }
    }

    pub fn width_dots(&self) -> i32 {
        (self.wc * 2) as i32
    }

    pub fn height_dots(&self) -> i32 {
        (self.hc * 4) as i32
    }

    pub fn dot(&mut self, x: i32, y: i32, intensity: f32) {
        if x < 0 || y < 0 || x >= self.width_dots() || y >= self.height_dots() {
            return;
        }
        let idx = (y as usize / 4) * self.wc + x as usize / 2;
        self.bits[idx] |= BRAILLE_BITS[y as usize % 4][x as usize % 2];
        if intensity > self.intensity[idx] {
            self.intensity[idx] = intensity;
        }
    }

    /// Bresenham line in dot coordinates.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, intensity: f32) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.dot(x, y, intensity);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub fn blit(&self, buf: &mut Buffer, area: Rect, mut color: impl FnMut(f32) -> Color) {
        let mut utf8 = [0u8; 4];
        for cy in 0..self.hc.min(area.height as usize) {
            for cx in 0..self.wc.min(area.width as usize) {
                let idx = cy * self.wc + cx;
                if self.bits[idx] == 0 {
                    continue;
                }
                let ch = char::from_u32(0x2800 + self.bits[idx] as u32).unwrap();
                buf[(area.x + cx as u16, area.y + cy as u16)]
                    .set_symbol(ch.encode_utf8(&mut utf8))
                    .set_fg(color(self.intensity[idx]));
            }
        }
    }
}
