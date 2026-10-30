use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{AudioFrame, BrailleCanvas, PeakScaler, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Oscilloscope: the raw waveform as a braille trace. Splits into two
/// stereo traces when the area is tall enough. A rising-zero-crossing
/// trigger keeps periodic signals steady.
pub struct Scope {
    left: Vec<f32>,
    right: Vec<f32>,
    rate: u32,
    /// Slow auto-scale so quiet material still shows a visible trace.
    scaler: PeakScaler,
}

impl Scope {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
            rate: 0,
            scaler: PeakScaler::new(0.9),
        }
    }
}

impl Visualizer for Scope {
    fn update(&mut self, frame: &AudioFrame, _cfg: &Config, _max_bars: usize, dt: f32) {
        self.rate = frame.rate;
        if frame.silent {
            self.left.clear();
            self.right.clear();
            return;
        }
        self.left.clear();
        self.left.extend_from_slice(frame.left);
        self.right.clear();
        self.right.extend_from_slice(frame.right);
        self.scaler.follow(frame.features.peak, dt);
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config) {
        if area.width < 4 || area.height < 3 {
            return;
        }
        let stereo = area.height >= 14;
        let scale = self.scaler.scale(cfg.gain_db);
        let window_s = cfg.scope.window_ms / 1000.0;

        if stereo {
            let half = Rect {
                height: area.height / 2,
                ..area
            };
            let lower = Rect {
                y: area.y + area.height / 2,
                height: area.height - area.height / 2,
                ..area
            };
            draw_trace(&self.left, self.rate, scale, window_s, half, buf, theme, "L");
            draw_trace(&self.right, self.rate, scale, window_s, lower, buf, theme, "R");
        } else {
            // Mono: average the channels on the fly.
            let mono: Vec<f32> = self
                .left
                .iter()
                .zip(&self.right)
                .map(|(l, r)| (l + r) * 0.5)
                .collect();
            draw_trace(&mono, self.rate, scale, window_s, area, buf, theme, "");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_trace(
    samples: &[f32],
    rate: u32,
    scale: f32,
    window_s: f32,
    area: Rect,
    buf: &mut Buffer,
    theme: ThemeId,
    tag: &str,
) {
    let mut canvas = BrailleCanvas::new(area.width, area.height);
    let (wd, hd) = (canvas.width_dots(), canvas.height_dots());
    let mid = hd / 2;

    // Center line.
    for x in (0..wd).step_by(3) {
        canvas.dot(x, mid, 0.25);
    }

    if rate > 0 && !samples.is_empty() {
        let window = ((rate as f32 * window_s) as usize).clamp(64, samples.len());
        let start = trigger_offset(samples, window);
        let view = &samples[start..(start + window).min(samples.len())];

        // Min/max per dot column, like a real scope at slow timebase.
        let n = view.len();
        let mut prev_top = mid;
        let mut prev_bot = mid;
        for x in 0..wd {
            let a = (x as usize) * n / wd as usize;
            let b = (((x + 1) as usize) * n / wd as usize).max(a + 1).min(n);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &s in &view[a..b] {
                lo = lo.min(s);
                hi = hi.max(s);
            }
            let y_of = |v: f32| {
                mid - ((v * scale).clamp(-1.0, 1.0) * (hd / 2 - 1) as f32).round() as i32
            };
            let (top, bot) = (y_of(hi), y_of(lo));
            // Connect to the previous column so steep edges stay solid.
            canvas.line(x, top.min(prev_bot), x, bot.max(prev_top), 1.0);
            (prev_top, prev_bot) = (top, bot);
        }
    }

    canvas.blit(buf, area, |i| {
        if i > 0.5 {
            theme.accent()
        } else {
            theme.dim()
        }
    });

    if !tag.is_empty() && area.width > 2 {
        buf[(area.x + 1, area.y)]
            .set_symbol(tag)
            .set_style(Style::default().fg(theme.dim()));
    }
}

/// First rising zero-crossing in the older half of the buffer, so the shown
/// window is always fully populated.
fn trigger_offset(samples: &[f32], window: usize) -> usize {
    let latest_start = samples.len().saturating_sub(window);
    let search_from = latest_start / 2;
    let mut prev = samples.get(search_from).copied().unwrap_or(0.0);
    for (i, &s) in samples[search_from..latest_start].iter().enumerate().skip(1) {
        if prev < 0.0 && s >= 0.0 {
            return search_from + i;
        }
        prev = s;
    }
    latest_start
}
