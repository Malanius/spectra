use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{put_half_block, AudioFrame, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Scrolling waterfall: frequency left→right, time flowing upward, intensity
/// as color. Rendered with half-blocks so every terminal row holds two time
/// slices.
pub struct Spectrogram {
    analyzer: Analyzer,
    history: VecDeque<Vec<f32>>,
    acc: f32,
}

/// Fixed number of frequency columns stored per slice.
const BANDS: usize = 96;
const MAX_SLICES: usize = 500;
/// Brightness curve exponent over the level above the floor. Bars are
/// AGC-normalized over 64 dB, so in a dense mix most bands sit at 0.4-0.8;
/// a linear ramp leaves the whole screen equally bright.
const GAMMA: f32 = 2.2;
/// ANSI themes can't be dimmed, so brightness picks the palette step
/// instead: blank below the first threshold, the theme's dim color below
/// the second.
const ANSI_BLANK_BELOW: f32 = 0.06;
const ANSI_DIM_BELOW: f32 = 0.25;

impl Spectrogram {
    pub fn new(cfg: &Config) -> Self {
        Self {
            // Fastest smoothing: the waterfall itself provides history.
            analyzer: Analyzer::new(cfg, BANDS).with_fixed_smoothing(0),
            history: VecDeque::new(),
            acc: 0.0,
        }
    }
}

impl Visualizer for Spectrogram {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);

        let slice = cfg.spectrogram.slice_ms / 1000.0;
        self.acc += dt;
        if self.acc >= slice {
            self.acc %= slice;
            self.history.push_back(self.analyzer.bars().to_vec());
            if self.history.len() > MAX_SLICES {
                self.history.pop_front();
            }
        }
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        let mut sorted = self.history.back().cloned().unwrap_or_else(|| vec![0.0]);
        sorted.sort_by(f32::total_cmp);
        let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
        vec![("p10", at(0.1)), ("median", at(0.5)), ("p90", at(0.9))]
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config) {
        if area.width == 0 || area.height == 0 || self.history.is_empty() {
            return;
        }
        let total_px = area.height as usize * 2; // two slices per row
        let len = self.history.len();
        let floor = cfg.spectrogram.floor;
        let fade = cfg.spectrogram.fade;
        // Soft edge above the floor (quiet content dims out instead of
        // popping) plus optional fading of older rows. Both only darken RGB
        // theme colors; ANSI palette colors pass through.
        let color = |v: f32, age: f32| -> Option<Color> {
            if v < floor {
                return None;
            }
            let level = ((v - floor) / (1.0 - floor)).powf(GAMMA);
            let k = level * (1.0 - fade * age);
            match theme.bar_color(v) {
                c @ Color::Rgb(..) => Some(crate::theme::dim_color(c, k.max(0.04))),
                _ if k < ANSI_BLANK_BELOW => None,
                _ if k < ANSI_DIM_BELOW => Some(theme.dim()),
                _ => Some(theme.bar_color(k)),
            }
        };

        for row in 0..area.height as usize {
            for col in 0..area.width as usize {
                let band = col * BANDS / area.width as usize;
                // Pixel row 0 is the top (oldest); the newest slice sits at
                // the bottom edge.
                let slice_at = |px: usize| -> Option<&Vec<f32>> {
                    (len + px).checked_sub(total_px).map(|i| &self.history[i])
                };
                let age_of = |px: usize| (total_px - 1 - px) as f32 / total_px as f32;
                let upper = slice_at(row * 2).and_then(|s| color(s[band], age_of(row * 2)));
                let lower =
                    slice_at(row * 2 + 1).and_then(|s| color(s[band], age_of(row * 2 + 1)));
                let cell = &mut buf[(area.x + col as u16, area.y + row as u16)];
                put_half_block(cell, upper, lower);
            }
        }
    }
}
