use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{AudioFrame, Visualizer};
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
/// Seconds between stored slices (two slices per terminal row).
const SLICE: f32 = 0.033;
const MAX_SLICES: usize = 500;

impl Spectrogram {
    pub fn new(cfg: &Config) -> Self {
        Self {
            analyzer: Analyzer::new(
                cfg.fft_size,
                BANDS,
                cfg.freq_min,
                cfg.freq_max,
                cfg.gain_db,
                cfg.auto_gain,
                0, // fastest smoothing: the waterfall itself provides history
            ),
            history: VecDeque::new(),
            acc: 0.0,
        }
    }
}

impl Visualizer for Spectrogram {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.gain_db = cfg.gain_db;
        self.analyzer.auto_gain = cfg.auto_gain;
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);

        self.acc += dt;
        if self.acc >= SLICE {
            self.acc %= SLICE;
            self.history.push_back(self.analyzer.bars().to_vec());
            if self.history.len() > MAX_SLICES {
                self.history.pop_front();
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 || self.history.is_empty() {
            return;
        }
        let total_px = area.height as usize * 2; // two slices per row
        let len = self.history.len();
        let color = |v: f32| -> Option<Color> {
            if v < 0.03 {
                None
            } else {
                Some(theme.bar_color(v))
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
                let upper = slice_at(row * 2).and_then(|s| color(s[band]));
                let lower = slice_at(row * 2 + 1).and_then(|s| color(s[band]));
                let cell = &mut buf[(area.x + col as u16, area.y + row as u16)];
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
        }
    }
}
