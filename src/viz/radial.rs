use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{AudioFrame, BrailleCanvas, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Radial spectrum: the bands wrapped around a circle like a pulsing iris.
/// Bass sits at the top and sweeps down both sides symmetrically to treble
/// at the bottom; the inner ring breathes with the bass.
pub struct Radial {
    analyzer: Analyzer,
    pulse: f32,
}

const BANDS: usize = 48;

impl Radial {
    pub fn new(cfg: &Config) -> Self {
        Self {
            analyzer: Analyzer::new(cfg, BANDS),
            pulse: 0.0,
        }
    }
}

impl Visualizer for Radial {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);

        let bars = self.analyzer.bars();
        let bass = bars[..BANDS / 6].iter().sum::<f32>() / (BANDS / 6) as f32;
        self.pulse += (bass - self.pulse) * (dt * 10.0).min(1.0);
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config) {
        if area.width < 6 || area.height < 4 {
            return;
        }
        // Spokes around the full circle (each band appears once per side).
        let spokes = cfg.radial.spokes.clamp(24, 256) & !1;
        let bars = self.analyzer.bars();
        let mut canvas = BrailleCanvas::new(area.width, area.height);
        let (wd, hd) = (canvas.width_dots(), canvas.height_dots());
        let (cx, cy) = (wd as f32 / 2.0, hd as f32 / 2.0);
        let r_max = (wd.min(hd) as f32) / 2.0 - 1.0;
        let r_inner = r_max * (0.22 + 0.10 * self.pulse);

        for spoke in 0..spokes {
            // Mirror: 0 at the top, sweeping down both sides to the bottom.
            let half = spokes / 2;
            let band = if spoke < half {
                spoke * BANDS / half
            } else {
                (spokes - 1 - spoke) * BANDS / half
            }
            .min(BANDS - 1);
            let v = bars[band];
            let angle = spoke as f32 / spokes as f32 * std::f32::consts::TAU
                - std::f32::consts::FRAC_PI_2;
            let (sin, cos) = angle.sin_cos();
            let len = v * (r_max - r_inner);
            // Draw the spoke as dots so intensity can rise along the radius.
            let steps = (len.ceil() as i32).max(1);
            for s in 0..=steps {
                let r = r_inner + len * s as f32 / steps as f32;
                let t = (r - r_inner) / (r_max - r_inner).max(1.0);
                canvas.dot(
                    (cx + cos * r) as i32,
                    (cy + sin * r) as i32,
                    0.2 + 0.8 * t.max(v * 0.3),
                );
            }
            // Inner ring dot even for silent bands, so the iris stays visible.
            canvas.dot((cx + cos * r_inner) as i32, (cy + sin * r_inner) as i32, 0.15);
        }

        canvas.blit(buf, area, |i| {
            if i <= 0.16 {
                theme.dim()
            } else {
                theme.bar_color(i)
            }
        });
    }
}
