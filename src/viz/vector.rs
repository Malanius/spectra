use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{AudioFrame, BrailleCanvas, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Goniometer / vectorscope: plots left vs. right sample pairs rotated 45°,
/// with phosphor-style persistence. Mono material collapses to a vertical
/// line, wide stereo blooms into a cloud, out-of-phase content leans
/// horizontal.
pub struct Vector {
    /// Persistence field in a fixed square virtual resolution.
    field: Vec<f32>,
    gain: f32,
}

/// Virtual pixel grid edge (square). Braille dots are ~square on screen, so
/// this maps cleanly onto any terminal size.
const N: usize = 160;
/// Phosphor decay rate per second.
const DECAY: f32 = 7.0;

impl Vector {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            field: vec![0.0; N * N],
            gain: 1.0,
        }
    }
}

impl Visualizer for Vector {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        let fade = (-DECAY * dt).exp();
        for v in &mut self.field {
            *v *= fade;
            if *v < 0.01 {
                *v = 0.0;
            }
        }
        if frame.silent {
            return;
        }

        // Auto-scale on the recent peak; manual gain stacks on top.
        let peak = frame
            .mono
            .iter()
            .fold(0.0f32, |a, &s| a.max(s.abs()))
            .max(1e-4);
        let target = (0.85 / peak).clamp(0.5, 30.0);
        let k = if target < self.gain {
            (dt * 8.0).min(1.0)
        } else {
            (dt * 0.7).min(1.0)
        };
        self.gain += (target - self.gain) * k;
        let scale = self.gain * 10f32.powf(cfg.gain_db / 20.0);

        // Plot the freshest slice of samples (~one frame's worth).
        let want = ((frame.rate as f32 * dt) as usize + 64).min(frame.left.len());
        let start = frame.left.len() - want;
        let half = (N / 2) as f32;
        for (&l, &r) in frame.left[start..].iter().zip(&frame.right[start..]) {
            // Rotate 45°: x = side (L-R), y = mid (L+R).
            let x = (l - r) * std::f32::consts::FRAC_1_SQRT_2 * scale;
            let y = (l + r) * std::f32::consts::FRAC_1_SQRT_2 * scale;
            let px = (half + x * half).clamp(0.0, (N - 1) as f32) as usize;
            let py = (half - y * half).clamp(0.0, (N - 1) as f32) as usize;
            let cell = &mut self.field[py * N + px];
            *cell = (*cell + 0.45).min(1.0);
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width < 4 || area.height < 3 {
            return;
        }
        let mut canvas = BrailleCanvas::new(area.width, area.height);
        let (wd, hd) = (canvas.width_dots(), canvas.height_dots());
        let edge = wd.min(hd);
        let (ox, oy) = ((wd - edge) / 2, (hd - edge) / 2);

        // Dim crosshair: vertical = mono axis, horizontal = side axis.
        for i in (0..edge).step_by(3) {
            canvas.dot(ox + edge / 2, oy + i, 0.12);
            canvas.dot(ox + i, oy + edge / 2, 0.12);
        }

        for py in 0..N {
            for px in 0..N {
                let v = self.field[py * N + px];
                if v < 0.05 {
                    continue;
                }
                let dx = ox + (px * edge as usize / N) as i32;
                let dy = oy + (py * edge as usize / N) as i32;
                canvas.dot(dx, dy, v);
            }
        }

        canvas.blit(buf, area, |i| {
            if i <= 0.13 {
                theme.dim()
            } else {
                theme.bar_color(i)
            }
        });

        if area.width >= 12 {
            let style = Style::default().fg(theme.dim());
            buf[(area.x + area.width / 2, area.y)]
                .set_symbol("M")
                .set_style(style);
            buf[(area.x + 1, area.y + area.height / 2)]
                .set_symbol("S")
                .set_style(style);
        }
    }
}
