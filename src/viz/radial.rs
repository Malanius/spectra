use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{AudioFrame, BrailleCanvas, OnsetDetector, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Radial spectrum: the bands wrapped around a circle like a pulsing iris.
/// Bass sits at the top and sweeps down both sides symmetrically to treble
/// at the bottom; the inner ring punches open on each kick.
pub struct Radial {
    analyzer: Analyzer,
    onset: OnsetDetector,
    /// Kick punch, 1 right after an onset, decaying to 0.
    pulse: f32,
}

const BANDS: usize = 48;
/// Kick punch decay time, in seconds.
const PULSE_S: f32 = 0.25;

impl Radial {
    pub fn new(cfg: &Config) -> Self {
        Self {
            analyzer: Analyzer::new(cfg, BANDS),
            onset: OnsetDetector::new(2.0, 0.08, 0.15),
            pulse: 0.0,
        }
    }
}

impl Visualizer for Radial {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);

        // Following the smoothed bass moved the iris by ~2 dots in a dense
        // mix, where bass never drops far; a kick punch reads clearly.
        self.pulse = (self.pulse - dt / PULSE_S).max(0.0);
        if let Some(strength) = self.onset.feed(frame.features.bass, frame.silent, dt) {
            self.pulse = self.pulse.max((0.6 + strength).min(1.0));
        }
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        let bars = self.analyzer.bars();
        let n = bars.len() / 3;
        let avg = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
        vec![
            ("pulse", self.pulse),
            ("len_lo", avg(&bars[..n])),
            ("len_hi", avg(&bars[2 * n..])),
        ]
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
        let r_inner = r_max * (0.18 + 0.14 * self.pulse);

        for spoke in 0..spokes {
            // Mirror: 0 at the top, sweeping down both sides to the bottom.
            let half = spokes / 2;
            let band = if spoke < half {
                spoke * BANDS / half
            } else {
                (spokes - 1 - spoke) * BANDS / half
            }
            .min(BANDS - 1);
            // Squared: in a dense mix every band sits at 0.6-0.9, which drew
            // a uniform disc. Squaring pulls the steady bed in and lets
            // peaks reach out.
            let v = bars[band] * bars[band];
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
            // It flares on a kick.
            let ring = 0.15 + 0.85 * self.pulse;
            canvas.dot((cx + cos * r_inner) as i32, (cy + sin * r_inner) as i32, ring);
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
