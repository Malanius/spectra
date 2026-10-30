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

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::config::Config;
use crate::theme::ThemeId;

/// Summary features computed once per frame from the mono mix, so styles
/// that only need coarse levels don't each run their own analyzer.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioFeatures {
    /// Band-group averages from a fast-attack 24-band analyzer, 0..1.
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub level: f32,
    /// RMS of the current window in dBFS.
    pub rms_db: f32,
    /// Absolute sample peak of the current window, linear.
    pub peak: f32,
}

/// One analysis window of audio, shared by all visualizer styles.
pub struct AudioFrame<'a> {
    pub mono: &'a [f32],
    pub left: &'a [f32],
    pub right: &'a [f32],
    pub rate: u32,
    /// True when no fresh samples are arriving (paused / suspended stream).
    pub silent: bool,
    pub features: AudioFeatures,
}

/// A visualization style. To add a new one: implement this trait and add
/// one entry to [`STYLES`] — cycling, the style menu, config persistence,
/// and CLI validation all derive from that table.
pub trait Visualizer {
    /// Advance state by `dt` seconds. `max_bars` is how many bars currently
    /// fit on screen; styles that don't draw bars may ignore it.
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, max_bars: usize, dt: f32);
    /// Draw into `area`. Must not assume any particular minimum size.
    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config);
}

/// Registry entry for one style: the config/CLI id, the display name, and
/// the constructor.
pub struct StyleInfo {
    pub id: &'static str,
    pub name: &'static str,
    factory: fn(&Config) -> Box<dyn Visualizer>,
}

/// All styles, in cycle order. The single source of truth.
pub const STYLES: [StyleInfo; 13] = [
    StyleInfo { id: "bars", name: "Bars", factory: |c| Box::new(bars::Bars::new(c)) },
    StyleInfo { id: "mirror", name: "Mirrored stereo", factory: |c| Box::new(mirror::Mirror::new(c)) },
    StyleInfo { id: "ripples", name: "Ripples", factory: |c| Box::new(ripples::Ripples::new(c)) },
    StyleInfo { id: "fractal", name: "Fractal", factory: |c| Box::new(fractal::Fractal::new(c)) },
    StyleInfo { id: "spectrogram", name: "Spectrogram", factory: |c| Box::new(spectrogram::Spectrogram::new(c)) },
    StyleInfo { id: "scope", name: "Oscilloscope", factory: |c| Box::new(scope::Scope::new(c)) },
    StyleInfo { id: "vector", name: "Vectorscope", factory: |c| Box::new(vector::Vector::new(c)) },
    StyleInfo { id: "vu", name: "VU meters", factory: |c| Box::new(vu::Vu::new(c)) },
    StyleInfo { id: "fire", name: "Fire", factory: |c| Box::new(fire::Fire::new(c)) },
    StyleInfo { id: "particles", name: "Particles", factory: |c| Box::new(particles::Particles::new(c)) },
    StyleInfo { id: "matrix", name: "Matrix rain", factory: |c| Box::new(matrix::Matrix::new(c)) },
    StyleInfo { id: "radial", name: "Radial", factory: |c| Box::new(radial::Radial::new(c)) },
    StyleInfo { id: "skyline", name: "Skyline", factory: |c| Box::new(skyline::Skyline::new(c)) },
];

/// Instantiate every visualizer, in [`STYLES`] order.
pub fn create_all(cfg: &Config) -> Vec<Box<dyn Visualizer>> {
    STYLES.iter().map(|s| (s.factory)(cfg)).collect()
}

/// Style ids, for CLI value validation.
pub fn style_ids() -> impl Iterator<Item = &'static str> {
    STYLES.iter().map(|s| s.id)
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

/// Paint two stacked pixels into one cell with half blocks. `None` leaves
/// that half unpainted.
pub(crate) fn put_half_block(cell: &mut Cell, upper: Option<Color>, lower: Option<Color>) {
    match (upper, lower) {
        (Some(u), Some(l)) => {
            cell.set_symbol("▀").set_fg(u).set_bg(l);
        }
        (Some(u), None) => {
            cell.set_symbol("▀").set_fg(u);
        }
        (None, Some(l)) => {
            cell.set_symbol("▄").set_fg(l);
        }
        (None, None) => {}
    }
}

/// Horizontal placement shared by the bar-based styles: returns
/// (x of first bar, cell stride between bars).
pub(crate) fn bar_layout(area: Rect, n: usize, cfg: &Config) -> (u16, u16) {
    let stride = cfg.bar_width + cfg.gap;
    let total = (n as u16) * stride - cfg.gap.min(stride);
    (area.x + area.width.saturating_sub(total) / 2, stride)
}

#[derive(Clone, Copy)]
pub(crate) enum Grow {
    Up,
    Down,
}

/// Placement of one bar: column `x`, first cell row `base_y`, and how many
/// rows it may fill in direction `grow`.
#[derive(Clone, Copy)]
pub(crate) struct BarColumn {
    pub x: u16,
    pub base_y: u16,
    pub rows: usize,
    pub grow: Grow,
}

/// Draw one bar of height `value` (0..1) with its peak-hold cap, using
/// eighth blocks for the partial top cell. Shared by bars and mirror.
pub(crate) fn draw_bar(
    buf: &mut Buffer,
    area: Rect,
    theme: ThemeId,
    cfg: &Config,
    col: BarColumn,
    value: f32,
    peak: f32,
) {
    let BarColumn { x, base_y, rows, grow } = col;
    if rows == 0 {
        return;
    }
    let bw = cfg.bar_width;
    // Terminal y for the cell `row` steps away from the base.
    let y_at = |row: usize| match grow {
        Grow::Up => base_y - row as u16,
        Grow::Down => base_y + row as u16,
    };

    let fill = (value * (rows * 8) as f32).round() as usize;
    let full_cells = fill / 8;
    let rem = fill % 8;

    for row in 0..rows.min(full_cells) {
        let color = theme.bar_color((row as f32 + 0.5) / rows as f32);
        put_hspan(buf, area, x, bw, y_at(row), "█", Style::default().fg(color));
    }
    if rem > 0 && full_cells < rows {
        let color = theme.bar_color((full_cells as f32 + 0.5) / rows as f32);
        let style = Style::default().fg(color);
        let (sym, style) = match grow {
            Grow::Up => (EIGHTHS[rem - 1], style),
            // No upper-eighth glyphs exist: draw the complementary lower
            // block with fg/bg swapped so the colored part hangs from the top.
            Grow::Down => (EIGHTHS[8 - rem - 1], style.add_modifier(Modifier::REVERSED)),
        };
        put_hspan(buf, area, x, bw, y_at(full_cells), sym, style);
    }

    if cfg.peaks {
        let pe = (peak * (rows * 8) as f32).round() as usize;
        if pe >= 2 {
            // Cell whose outer edge is nearest the peak height.
            let pcell = (pe.div_ceil(8) - 1).min(rows - 1);
            let top_fill_cell = if fill == 0 { None } else { Some((fill - 1) / 8) };
            if top_fill_cell.is_none_or(|t| pcell > t) {
                // Cap sits on the outer edge of its cell: top edge for an
                // upward bar, bottom edge for a downward one.
                let sym = match grow {
                    Grow::Up => "▔",
                    Grow::Down => "▁",
                };
                let style = Style::default().fg(theme.peak_color());
                put_hspan(buf, area, x, bw, y_at(pcell), sym, style);
            }
        }
    }
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

/// AGC silence threshold for the broadband RMS / peak levels that the
/// level-based styles feed into [`Agc`](crate::dsp::Agc).
pub(crate) const LEVEL_SILENCE_DB: f32 = -70.0;

/// Linear auto-scale for the waveform styles: steers the recent sample
/// peak toward `headroom` of full scale. Shrinks fast on loud peaks and
/// grows slowly, so quiet material still shows a visible trace.
pub(crate) struct PeakScaler {
    gain: f32,
    headroom: f32,
}

impl PeakScaler {
    pub fn new(headroom: f32) -> Self {
        Self {
            gain: 1.0,
            headroom,
        }
    }

    pub fn follow(&mut self, peak: f32, dt: f32) {
        let target = (self.headroom / peak.max(1e-4)).clamp(0.5, 30.0);
        let k = if target < self.gain {
            (dt * 8.0).min(1.0)
        } else {
            (dt * 0.7).min(1.0)
        };
        self.gain += (target - self.gain) * k;
    }

    /// Auto-scale factor with the manual gain stacked on top.
    pub fn scale(&self, gain_db: f32) -> f32 {
        self.gain * 10f32.powf(gain_db / 20.0)
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
