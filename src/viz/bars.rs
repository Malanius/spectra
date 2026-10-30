use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{bar_layout, draw_bar, AudioFrame, BarColumn, Grow, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Classic hi-fi look: vertical bars with falling peak-hold caps and a
/// frequency axis along the bottom.
pub struct Bars {
    analyzer: Analyzer,
}

impl Bars {
    pub fn new(cfg: &Config) -> Self {
        Self {
            analyzer: Analyzer::new(cfg, cfg.bars),
        }
    }
}

impl Visualizer for Bars {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.agg = cfg.band_agg;
        self.analyzer.set_bars(cfg.bars.clamp(4, max_bars));
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config) {
        let show_labels = area.height >= 11;
        let bars_area = if show_labels {
            Rect { height: area.height - 1, ..area }
        } else {
            area
        };
        let (x0, stride) = draw_bars(&self.analyzer, bars_area, buf, theme, cfg);
        if show_labels {
            let label_row = Rect {
                y: area.y + area.height - 1,
                height: 1,
                ..area
            };
            draw_freq_labels(&self.analyzer, label_row, buf, theme, cfg, x0, stride);
        }
    }
}

/// Draw the bar field; returns (x of first bar, stride) for the axis labels.
fn draw_bars(
    analyzer: &Analyzer,
    area: Rect,
    buf: &mut Buffer,
    theme: ThemeId,
    cfg: &Config,
) -> (u16, u16) {
    let bars = analyzer.bars();
    let peaks = analyzer.peaks();
    let n = bars.len();
    if n == 0 || area.height == 0 || area.width == 0 {
        return (area.x, 1);
    }

    let (x0, stride) = bar_layout(area, n, cfg);
    for (i, (&v, &p)) in bars.iter().zip(peaks).enumerate() {
        let col = BarColumn {
            x: x0 + i as u16 * stride,
            base_y: area.y + area.height - 1,
            rows: area.height as usize,
            grow: Grow::Up,
        };
        draw_bar(buf, area, theme, cfg, col, v, p);
    }
    (x0, stride)
}

fn draw_freq_labels(
    analyzer: &Analyzer,
    area: Rect,
    buf: &mut Buffer,
    theme: ThemeId,
    cfg: &Config,
    x0: u16,
    stride: u16,
) {
    let n = analyzer.bars().len();
    if n == 0 {
        return;
    }
    let style = Style::default().fg(theme.dim());
    let targets = [
        50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0f32,
    ];
    let mut last_end: i32 = -2;
    for &t in &targets {
        // Bar whose center frequency is nearest this target.
        let (best, freq) = (0..n)
            .map(|i| (i, analyzer.band_freq(i)))
            .min_by(|a, b| {
                (a.1.ln() - t.ln())
                    .abs()
                    .total_cmp(&(b.1.ln() - t.ln()).abs())
            })
            .unwrap();
        if (freq.ln() - t.ln()).abs() > 0.35 {
            continue; // no bar reasonably close (out of range)
        }
        let label = if t >= 1000.0 {
            format!("{}k", t / 1000.0)
        } else {
            format!("{t}")
        };
        let cx = x0 as i32 + best as i32 * stride as i32 + cfg.bar_width as i32 / 2;
        let start = cx - label.len() as i32 / 2;
        if start <= last_end + 1 {
            continue;
        }
        for (j, ch) in label.chars().enumerate() {
            let px = start + j as i32;
            if px >= area.x as i32 && px < (area.x + area.width) as i32 {
                buf[(px as u16, area.y)].set_char(ch).set_style(style);
            }
        }
        last_end = start + label.len() as i32;
    }
}
