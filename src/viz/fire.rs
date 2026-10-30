use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{put_half_block, AudioFrame, Rng, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Demoscene fire fed by the spectrum: each frequency band stokes its slice
/// of the fire bed, so bass makes wide roaring flames on the left and treble
/// sends sparks up on the right. Runs on a fixed virtual grid, rendered with
/// half-blocks (two flame pixels per terminal row).
pub struct Fire {
    analyzer: Analyzer,
    /// Heat field, row-major, row 0 at the bottom (the fire bed).
    heat: Vec<f32>,
    rng: Rng,
    level: f32,
}

const W: usize = 140;
const H: usize = 90;
const BANDS: usize = 32;

impl Fire {
    pub fn new(cfg: &Config) -> Self {
        Self {
            analyzer: Analyzer::new(cfg, BANDS),
            heat: vec![0.0; W * H],
            rng: Rng(0xfeedface1234),
            level: 0.0,
        }
    }
}

impl Visualizer for Fire {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);
        let bars = self.analyzer.bars();
        self.level = bars.iter().sum::<f32>() / bars.len() as f32;

        // Stoke the bed: interpolate the band values across the bottom row,
        // with flicker so the fire never looks synthetic.
        for x in 0..W {
            let pos = x as f32 * (BANDS - 1) as f32 / (W - 1) as f32;
            let (i, f) = (pos as usize, pos.fract());
            let band = bars[i] * (1.0 - f) + bars[(i + 1).min(BANDS - 1)] * f;
            let flicker = 0.75 + 0.5 * self.rng.f32();
            self.heat[x] = (band * 1.25 * flicker).clamp(0.0, 1.15);
        }

        // Propagate upward: each pixel pulls from the three below it and
        // cools; low overall level cools faster so the fire dies down.
        // Cooling per row decides flame height: heat ~1 travels ~1/cool of
        // the H rows, so keep it small.
        // Linear in level so flame height keeps tracking dynamics instead of
        // saturating at full screen on ordinary program material.
        // cfg.fire.cooling scales it: >1 = shorter flames, <1 = taller.
        let cool_base = (0.010 + 0.040 * (1.0 - self.level)) * cfg.fire.cooling;
        for y in (1..H).rev() {
            for x in 0..W {
                let below = &self.heat[(y - 1) * W..y * W];
                let l = below[x.saturating_sub(1)];
                let c = below[x];
                let r = below[(x + 1).min(W - 1)];
                let drift = self.rng.f32();
                // Random lateral bias makes flames lick sideways.
                let mix = if drift < 0.33 {
                    (l * 2.0 + c + r) * 0.25
                } else if drift > 0.66 {
                    (l + c + r * 2.0) * 0.25
                } else {
                    (l + c * 2.0 + r) * 0.25
                };
                let cool = cool_base * (0.6 + 0.8 * self.rng.f32());
                self.heat[y * W + x] = (mix - cool).max(0.0);
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let color = |h: f32| {
            if h >= 0.97 {
                theme.peak_color() // white-hot core
            } else {
                theme.bar_color(h)
            }
        };
        // Two virtual pixels per terminal row via half-blocks. Sample the
        // virtual grid with nearest-neighbor; row 0 of the grid is the
        // bottom of the screen.
        let total_px = area.height as usize * 2;
        for row in 0..area.height as usize {
            for col in 0..area.width as usize {
                let vx = col * W / area.width as usize;
                // Screen pixel p (0 = top) maps to heat row (0 = bottom).
                let sample = |p: usize| -> f32 {
                    let vy = (total_px - 1 - p) * H / total_px;
                    self.heat[vy.min(H - 1) * W + vx]
                };
                let (u, l) = (sample(row * 2), sample(row * 2 + 1));
                let cell = &mut buf[(area.x + col as u16, area.y + row as u16)];
                let lit = |h: f32| (h >= 0.035).then(|| color(h));
                put_half_block(cell, lit(u), lit(l));
            }
        }
    }
}
