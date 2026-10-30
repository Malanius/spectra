use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{bar_layout, draw_bar, AudioFrame, BarColumn, Grow, Visualizer};
use crate::config::{Config, MirrorMode};
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Old-deck stereo view: two spectra around a center line, each with its
/// own peak caps. In "lr" mode the left channel grows up and the right
/// grows down; in "ms" mode it is mid (up) versus side (down), which makes
/// stereo width directly visible.
pub struct Mirror {
    left: Analyzer,
    right: Analyzer,
    /// Scratch for the side (L−R) signal in "ms" mode.
    side_buf: Vec<f32>,
}

impl Mirror {
    pub fn new(cfg: &Config) -> Self {
        Self {
            left: Analyzer::new(cfg, cfg.bars),
            right: Analyzer::new(cfg, cfg.bars),
            side_buf: Vec::new(),
        }
    }
}

impl Visualizer for Mirror {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, max_bars: usize, dt: f32) {
        let n = cfg.bars.clamp(4, max_bars);
        let ms = cfg.mirror.mode == MirrorMode::Ms;
        if ms {
            self.side_buf.clear();
            self.side_buf
                .extend(frame.left.iter().zip(frame.right).map(|(l, r)| (l - r) * 0.5));
        }
        let (top, bottom): (&[f32], &[f32]) = if ms {
            (frame.mono, &self.side_buf)
        } else {
            (frame.left, frame.right)
        };
        for (analyzer, samples) in [(&mut self.left, top), (&mut self.right, bottom)] {
            analyzer.sync(cfg);
            analyzer.agg = cfg.band_agg;
            analyzer.set_bars(n);
            analyzer.update(samples, frame.rate, dt, frame.silent);
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config) {
        let n = self.left.bars().len();
        if n == 0 || area.width == 0 || area.height < 3 {
            return;
        }

        let center_y = area.y + area.height / 2;
        let up_rows = (center_y - area.y) as usize;
        let down_rows = (area.y + area.height - center_y - 1) as usize;
        let (x0, stride) = bar_layout(area, n, cfg);

        // Water line between the channels.
        let line_style = Style::default().fg(theme.dim());
        for x in area.x..area.x + area.width {
            buf[(x, center_y)].set_symbol("─").set_style(line_style);
        }

        for i in 0..n {
            let x = x0 + i as u16 * stride;
            let up = BarColumn {
                x,
                base_y: center_y - 1,
                rows: up_rows,
                grow: Grow::Up,
            };
            let down = BarColumn {
                x,
                base_y: center_y + 1,
                rows: down_rows,
                grow: Grow::Down,
            };
            draw_bar(buf, area, theme, cfg, up, self.left.bars()[i], self.left.peaks()[i]);
            draw_bar(buf, area, theme, cfg, down, self.right.bars()[i], self.right.peaks()[i]);
        }

        // Channel tags in the top/bottom-left corners when there is room.
        if area.width >= 4 && area.height >= 7 {
            let (top_tag, bottom_tag) = match cfg.mirror.mode {
                MirrorMode::Ms => ('M', 'S'),
                MirrorMode::Lr => ('L', 'R'),
            };
            buf[(area.x + 1, area.y)]
                .set_char(top_tag)
                .set_style(line_style);
            buf[(area.x + 1, area.y + area.height - 1)]
                .set_char(bottom_tag)
                .set_style(line_style);
        }
    }
}
