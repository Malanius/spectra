use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{put_half_block, AudioFrame, Rng, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::{dim_color, ThemeId};

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
    /// Fractional rows of rise carried to the next frame.
    rise_acc: f32,
    /// Level smoothed over ~1 s, for cooling. Cooling on the instant level
    /// jumped up after every kick and killed the kick's flames mid-rise.
    level_slow: f32,
    /// Per-column cooling multiplier, a slowly drifting noise. Columns that
    /// cool less burn taller, which makes flame tongues; uniform cooling
    /// left a flat-topped slab.
    tongues: Vec<f32>,
}

const W: usize = 140;
const H: usize = 90;
const BANDS: usize = 32;
/// Rise speed in grid rows per second. At one row per frame a flame took
/// longer to reach its height than the gap between kicks, so beats blurred
/// into a steady slab.
const RISE: f32 = 150.0;

impl Fire {
    pub fn new(cfg: &Config) -> Self {
        Self {
            analyzer: Analyzer::new(cfg, BANDS),
            heat: vec![0.0; W * H],
            rng: Rng(0xfeedface1234),
            level: 0.0,
            rise_acc: 0.0,
            level_slow: 0.0,
            tongues: vec![1.0; W],
        }
    }

    /// Restoke the bed and move the fire up one row.
    fn step(&mut self, bars: &[f32], cool_base: f32) {
        // Stoke the bed: interpolate the band values across the bottom row,
        // with flicker so the fire never looks synthetic.
        for x in 0..W {
            let pos = x as f32 * (BANDS - 1) as f32 / (W - 1) as f32;
            let (i, f) = (pos as usize, pos.fract());
            let band = bars[i] * (1.0 - f) + bars[(i + 1).min(BANDS - 1)] * f;
            let flicker = 0.75 + 0.5 * self.rng.f32();
            // Squared, so kicks stand out: bands sit at 0.5-0.75 in a dense
            // mix and a kick only adds ~0.2.
            self.heat[x] = (band * band * 1.6 * flicker).clamp(0.0, 1.3);
        }

        // Random walk with a pull back to 1, so it can't drift to a bound.
        for t in &mut self.tongues {
            *t += (self.rng.f32() - 0.5) * 0.12 + (1.0 - *t) * 0.02;
        }
        for x in 0..W {
            let l = self.tongues[x.saturating_sub(1)];
            let r = self.tongues[(x + 1).min(W - 1)];
            let t = &mut self.tongues[x];
            *t = (*t * 0.6 + (l + r) * 0.2).clamp(0.4, 1.6);
        }

        // Each pixel takes the heat of a pixel below, picked with sideways
        // jitter, and cools by a random amount. Copying rather than
        // averaging keeps the hot and cold streaks apart, which is what
        // makes flame tongues; averaging smoothed them into a flat slab.
        for y in (1..H).rev() {
            for x in 0..W {
                let jitter = (self.rng.f32() * 3.0) as usize; // 0, 1 or 2
                let src = (x + jitter).saturating_sub(1).min(W - 1);
                let below = &self.heat[(y - 1) * W..y * W];
                let h = 0.7 * below[src] + 0.3 * below[x];
                let cool = cool_base * self.tongues[x] * (0.4 + 1.2 * self.rng.f32());
                self.heat[y * W + x] = (h - cool).max(0.0);
            }
        }
    }
}

impl Visualizer for Fire {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);
        let bars = self.analyzer.bars().to_vec();
        let bars = &bars[..];
        self.level = bars.iter().sum::<f32>() / bars.len() as f32;

        // Cooling per row decides flame height: heat ~1 travels ~1/cool
        // rows. Linear in level so flame height keeps tracking dynamics
        // instead of saturating at full screen on ordinary program material.
        // cfg.fire.cooling scales it: >1 = shorter flames, <1 = taller.
        self.level_slow += (self.level - self.level_slow) * dt.min(1.0);
        let cool_base = (0.010 + 0.040 * (1.0 - self.level_slow)) * cfg.fire.cooling;
        self.rise_acc += dt * RISE;
        while self.rise_acc >= 1.0 {
            self.rise_acc -= 1.0;
            self.step(bars, cool_base);
        }
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        // Flame height per third of the bed: the highest lit row, 0..1.
        let height = |x0: usize, x1: usize| {
            (0..H)
                .rev()
                .find(|&y| self.heat[y * W + x0..y * W + x1].iter().any(|&h| h >= 0.035))
                .map_or(0.0, |y| (y + 1) as f32 / H as f32)
        };
        vec![
            ("level", self.level),
            ("h_bass", height(0, W / 3)),
            ("h_mid", height(W / 3, 2 * W / 3)),
            ("h_treble", height(2 * W / 3, W)),
        ]
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let color = |h: f32| {
            if h >= 0.97 {
                theme.peak_color() // white-hot core
            } else {
                // Cool edges fade into the dark instead of ending in a hard
                // line of the gradient's (often bright) low color. ANSI
                // colors pass through dim_color unchanged.
                dim_color(theme.bar_color(h), (h * 2.5).min(1.0))
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
