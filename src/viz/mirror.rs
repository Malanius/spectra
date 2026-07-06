use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{bar_layout, put_hspan, AudioFrame, Visualizer, EIGHTHS};
use crate::config::Config;
use crate::dsp::{Analyzer, BandAgg};
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
        let mk = || {
            Analyzer::new(
                cfg.fft_size,
                cfg.bars,
                cfg.freq_min,
                cfg.freq_max,
                cfg.gain_db,
                cfg.auto_gain,
                cfg.smoothing,
            )
        };
        Self {
            left: mk(),
            right: mk(),
            side_buf: Vec::new(),
        }
    }
}

impl Visualizer for Mirror {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, max_bars: usize, dt: f32) {
        let n = cfg.bars.clamp(4, max_bars);
        let ms = cfg.mirror.mode.eq_ignore_ascii_case("ms");
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
        let agg = BandAgg::from_key(&cfg.band_agg).unwrap_or(BandAgg::Energy);
        for (analyzer, samples) in [(&mut self.left, top), (&mut self.right, bottom)] {
            analyzer.gain_db = cfg.gain_db;
            analyzer.auto_gain = cfg.auto_gain;
            analyzer.smoothing = cfg.smoothing;
            analyzer.agg = agg;
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
            draw_half(
                buf, area, theme, cfg, x, center_y, up_rows, Direction::Up,
                self.left.bars()[i], self.left.peaks()[i],
            );
            draw_half(
                buf, area, theme, cfg, x, center_y, down_rows, Direction::Down,
                self.right.bars()[i], self.right.peaks()[i],
            );
        }

        // Channel tags in the top/bottom-left corners when there is room.
        if area.width >= 4 && area.height >= 7 {
            let (top_tag, bottom_tag) = if cfg.mirror.mode.eq_ignore_ascii_case("ms") {
                ('M', 'S')
            } else {
                ('L', 'R')
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

#[derive(Clone, Copy)]
enum Direction {
    Up,
    Down,
}

#[allow(clippy::too_many_arguments)]
fn draw_half(
    buf: &mut Buffer,
    area: Rect,
    theme: ThemeId,
    cfg: &Config,
    x: u16,
    center_y: u16,
    rows: usize,
    dir: Direction,
    value: f32,
    peak: f32,
) {
    if rows == 0 {
        return;
    }
    let bw = cfg.bar_width;
    // Terminal y for the cell `row` steps away from the center line.
    let y_at = |row: usize| match dir {
        Direction::Up => center_y - 1 - row as u16,
        Direction::Down => center_y + 1 + row as u16,
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
        let (sym, style) = match dir {
            Direction::Up => (EIGHTHS[rem - 1], style),
            // No upper-eighth glyphs exist: draw the complementary lower
            // block with fg/bg swapped so the colored part hangs from the top.
            Direction::Down => (EIGHTHS[8 - rem - 1], style.add_modifier(Modifier::REVERSED)),
        };
        put_hspan(buf, area, x, bw, y_at(full_cells), sym, style);
    }

    if cfg.peaks {
        let pe = (peak * (rows * 8) as f32).round() as usize;
        if pe >= 2 {
            let pcell = (pe.div_ceil(8) - 1).min(rows - 1);
            let top_fill_cell = if fill == 0 { None } else { Some((fill - 1) / 8) };
            if top_fill_cell.is_none_or(|t| pcell > t) {
                // Cap sits on the outer edge of its cell: top edge for the
                // upward half, bottom edge for the downward half.
                let sym = match dir {
                    Direction::Up => "▔",
                    Direction::Down => "▁",
                };
                let style = Style::default().fg(theme.peak_color());
                put_hspan(buf, area, x, bw, y_at(pcell), sym, style);
            }
        }
    }
}
