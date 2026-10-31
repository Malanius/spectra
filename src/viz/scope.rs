use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{AudioFrame, BrailleCanvas, PeakScaler, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Oscilloscope: the raw waveform as a braille trace. Splits into two
/// stereo traces when the area is tall enough. A rising-zero-crossing
/// trigger on the low-passed mix keeps periodic signals steady.
pub struct Scope {
    /// Own sample history, longer than the analysis buffer: the shown
    /// window plus room to search back for a trigger.
    left: Vec<f32>,
    right: Vec<f32>,
    /// Shown slice of `left`/`right`, shared so both traces line up.
    start: usize,
    window: usize,
    /// Whether `start` came from a trigger rather than the fallback.
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    locked: bool,
    /// Slow auto-scale so quiet material still shows a visible trace.
    scaler: PeakScaler,
}

/// Trigger low-pass cutoff. Hats and noise cross zero many times per
/// cycle of the bass, so triggering on the raw signal lands at a random
/// phase every frame.
const TRIGGER_HZ: f32 = 200.0;
/// How far back from the newest window to search for a trigger. Longer
/// than one cycle of the lowest bass, so a crossing is always in range.
const SEARCH_S: f32 = 0.05;

impl Scope {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
            start: 0,
            window: 0,
            locked: false,
            scaler: PeakScaler::new(0.9),
        }
    }
}

impl Visualizer for Scope {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.window = 0;
        if frame.silent || frame.rate == 0 || frame.left.is_empty() {
            self.left.clear();
            self.right.clear();
            return;
        }
        let rate = frame.rate as f32;
        let window = (rate * cfg.scope.window_ms / 1000.0) as usize;
        let search = (rate * SEARCH_S) as usize;
        let keep = window + search;
        let n = frame.fresh.min(frame.left.len());
        let tail = frame.left.len() - n;
        self.left.extend_from_slice(&frame.left[tail..]);
        self.right.extend_from_slice(&frame.right[tail..]);
        if self.left.len() > keep {
            let drop = self.left.len() - keep;
            self.left.drain(..drop);
            self.right.drain(..drop);
        }

        let len = self.left.len();
        if len < 64 {
            return; // Not enough history yet.
        }
        self.window = window.clamp(64, len);
        let trig = trigger_offset(&self.left, &self.right, self.window, frame.rate);
        self.locked = trig.is_some();
        self.start = trig.unwrap_or(len - self.window);

        // Scale on the louder channel over the shown slice: the mono peak
        // is half a hard-panned channel's, which flattened its tops.
        let range = self.start..self.start + self.window;
        let peak = self.left[range.clone()]
            .iter()
            .chain(&self.right[range])
            .fold(0.0f32, |a, &s| a.max(s.abs()));
        self.scaler.follow(peak, dt);
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        let scale = self.scaler.scale(0.0);
        let range = self.start..self.start + self.window;
        let clipped = if self.window == 0 {
            0
        } else {
            self.left[range.clone()]
                .iter()
                .chain(&self.right[range])
                .filter(|s| (*s * scale).abs() > 1.0)
                .count()
        };
        vec![
            ("scale", scale),
            ("clipped", clipped as f32 / (2 * self.window).max(1) as f32),
            ("locked", self.locked as u8 as f32),
            ("trigger", self.start as f32),
        ]
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config) {
        if area.width < 4 || area.height < 3 {
            return;
        }
        let stereo = area.height >= 14;
        let scale = self.scaler.scale(cfg.gain_db);
        let range = self.start..self.start + self.window;
        let (left, right) = if self.window == 0 {
            (&[][..], &[][..])
        } else {
            (&self.left[range.clone()], &self.right[range])
        };

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
            draw_trace(left, scale, half, buf, theme, "L");
            draw_trace(right, scale, lower, buf, theme, "R");
        } else {
            // Mono: average the channels on the fly.
            let mono: Vec<f32> = left.iter().zip(right).map(|(l, r)| (l + r) * 0.5).collect();
            draw_trace(&mono, scale, area, buf, theme, "");
        }
    }
}

fn draw_trace(view: &[f32], scale: f32, area: Rect, buf: &mut Buffer, theme: ThemeId, tag: &str) {
    let mut canvas = BrailleCanvas::new(area.width, area.height);
    let (wd, hd) = (canvas.width_dots(), canvas.height_dots());
    let mid = hd / 2;

    // Center line.
    for x in (0..wd).step_by(3) {
        canvas.dot(x, mid, 0.25);
    }

    if !view.is_empty() {
        let y_of = |v: f32| mid - ((v * scale).clamp(-1.0, 1.0) * (hd / 2 - 1) as f32).round() as i32;
        // Each dot column covers several samples. Their min/max span is
        // drawn dim, like a real scope at slow timebase, and the column
        // mean bright, so the waveform reads through hat and noise fuzz.
        let n = view.len();
        let mut prev: Option<(i32, i32, i32)> = None;
        let mut means = Vec::with_capacity(wd as usize);
        for x in 0..wd {
            let a = (x as usize) * n / wd as usize;
            let b = (((x + 1) as usize) * n / wd as usize).max(a + 1).min(n);
            let col = &view[a..b];
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &s in col {
                lo = lo.min(s);
                hi = hi.max(s);
            }
            let mean = col.iter().sum::<f32>() / col.len() as f32;
            let (top, bot, m) = (y_of(hi), y_of(lo), y_of(mean));
            // Connect to the previous column so steep edges stay solid.
            let (pt, pb) = prev.map_or((top, bot), |(t, b, _)| (t, b));
            canvas.line(x, top.min(pb), x, bot.max(pt), 0.4);
            means.push(m);
            prev = Some((top, bot, m));
        }
        for (x, w) in means.windows(2).enumerate() {
            canvas.line(x as i32, w[0], x as i32 + 1, w[1], 1.0);
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

/// Latest rising zero-crossing of the low-passed mix that still leaves a
/// full window after it. The signal must first dip below a small negative
/// threshold (hysteresis), so noise around zero can't trigger. `None` when
/// nothing crosses.
fn trigger_offset(left: &[f32], right: &[f32], window: usize, rate: u32) -> Option<usize> {
    let latest_start = left.len().saturating_sub(window);
    let a = 1.0 - (-std::f32::consts::TAU * TRIGGER_HZ / rate as f32).exp();
    let mut lp = 0.0f32;
    let filtered: Vec<f32> = left[..latest_start]
        .iter()
        .zip(right)
        .map(|(l, r)| {
            lp += ((l + r) * 0.5 - lp) * a;
            lp
        })
        .collect();
    // Skip the filter's settling time at the start.
    let settled = &filtered[(rate as usize / 200).min(filtered.len())..];
    let peak = settled.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    if peak < 1e-6 {
        return None;
    }
    let hyst = peak * 0.1;
    let (mut armed, mut found) = (false, None);
    for (i, &v) in settled.iter().enumerate() {
        if v < -hyst {
            armed = true;
        } else if armed && v >= 0.0 {
            armed = false;
            found = Some(filtered.len() - settled.len() + i);
        }
    }
    found
}
