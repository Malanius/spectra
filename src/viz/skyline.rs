use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{put_hspan, AudioFrame, Visualizer, EIGHTHS, LEVEL_SILENCE_DB};
use crate::config::Config;
use crate::dsp::Agc;
use crate::theme::ThemeId;

/// Strip-chart of the loudness envelope: time scrolls right-to-left like a
/// chart recorder, drawing the song's dynamics as a mountain silhouette.
/// The RMS body is solid; short peaks leave a bright ridge line above it.
pub struct Skyline {
    history: VecDeque<(f32, f32)>, // (rms 0..1, peak 0..1)
    agc: Agc,
    acc: f32,
    pending_rms: f32,
    pending_peak: f32,
}

/// Seconds per column of history.
const SLICE: f32 = 0.045;
const MAX_SLICES: usize = 600;
/// Vertical range below the ceiling. Wide enough to show a breakdown,
/// narrow enough that the beat moves the body visibly.
const RANGE_DB: f32 = 36.0;

impl Skyline {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            history: VecDeque::new(),
            agc: Agc::new(LEVEL_SILENCE_DB),
            acc: 0.0,
            pending_rms: 0.0,
            pending_peak: 0.0,
        }
    }
}

impl Visualizer for Skyline {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        let (rms, peak) = if frame.silent {
            (0.0, 0.0)
        } else {
            let rms_db = frame.features.rms_db;
            let peak_db = 20.0 * (frame.features.peak + 1e-9).log10();
            // Ceiling on the peaks: on the RMS, peaks 8-12 dB above it in
            // mastered music pinned the ridge to the top row. Now the gap
            // between ridge and body shows the crest factor.
            self.agc.follow(peak_db, cfg.auto_gain, dt);
            let norm = |db| self.agc.norm(db, cfg.auto_gain, cfg.gain_db, RANGE_DB);
            (norm(rms_db), norm(peak_db))
        };
        self.pending_rms = self.pending_rms.max(rms);
        self.pending_peak = self.pending_peak.max(peak);

        self.acc += dt;
        if self.acc >= SLICE {
            self.acc %= SLICE;
            self.history.push_back((self.pending_rms, self.pending_peak));
            self.pending_rms = 0.0;
            self.pending_peak = 0.0;
            if self.history.len() > MAX_SLICES {
                self.history.pop_front();
            }
        }
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        let (rms, peak) = self.history.back().copied().unwrap_or((0.0, 0.0));
        vec![("rms", rms), ("peak", peak), ("ceil", self.agc.ceil_db())]
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 || self.history.is_empty() {
            return;
        }
        let rows = area.height as usize;
        let len = self.history.len();

        for k in 0..(area.width as usize).min(len) {
            let (rms, peak) = self.history[len - 1 - k];
            let x = area.x + area.width - 1 - k as u16;

            let fill = (rms * (rows * 8) as f32).round() as usize;
            let full_cells = fill / 8;
            let rem = fill % 8;
            for row in 0..rows.min(full_cells) {
                let color = theme.bar_color((row as f32 + 0.5) / rows as f32);
                let y = area.y + area.height - 1 - row as u16;
                put_hspan(buf, area, x, 1, y, "█", Style::default().fg(color));
            }
            if rem > 0 && full_cells < rows {
                let color = theme.bar_color((full_cells as f32 + 0.5) / rows as f32);
                let y = area.y + area.height - 1 - full_cells as u16;
                put_hspan(buf, area, x, 1, y, EIGHTHS[rem - 1], Style::default().fg(color));
            }

            // Peak ridge above the body.
            let pe = (peak * (rows * 8) as f32).round() as usize;
            if pe >= 2 {
                let pcell = (pe.div_ceil(8) - 1).min(rows - 1);
                let top_fill = if fill == 0 { None } else { Some((fill - 1) / 8) };
                if top_fill.is_none_or(|t| pcell > t) {
                    let y = area.y + area.height - 1 - pcell as u16;
                    put_hspan(buf, area, x, 1, y, "▔", Style::default().fg(theme.peak_color()));
                }
            }
        }
    }
}
