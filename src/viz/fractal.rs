use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{put_half_block, AudioFrame, OnsetDetector, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Morphing Julia set. The constant `c` travels along the boundary of the
/// Mandelbrot main cardioid — where Julia sets are at their most intricate —
/// pushed along by the bass. Beats shove `c` outward (shapes shatter into
/// dendrites and reform), the overall level breathes the zoom, and treble
/// cycles the palette. Escape times are smooth-colored through the theme
/// gradient; the set interior stays dark.
pub struct Fractal {
    onset: OnsetDetector,
    /// Angle of `c` along the cardioid.
    theta: f32,
    /// Beat impulse: pushes `c` off the cardioid, decays quickly.
    pop: f32,
    /// Beat zoom punch: snaps the view in, then relaxes.
    zoom_pulse: f32,
    /// View rotation, driven by the mids.
    rot: f32,
    color_phase: f32,
    time: f32,
    bass_s: f32,
    bass_fast: f32,
    mid_s: f32,
    level_s: f32,
    treble_s: f32,
}

const MAX_ITER: u32 = 40;

impl Fractal {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            // Strict trigger with a long refractory: the fractal's beat
            // effects are dramatic, so they must fire on real kicks only.
            onset: OnsetDetector::new(2.5, 0.12, 0.28),
            theta: 0.35,
            pop: 0.0,
            zoom_pulse: 0.0,
            rot: 0.0,
            color_phase: 0.0,
            time: 0.0,
            bass_s: 0.0,
            bass_fast: 0.0,
            mid_s: 0.0,
            level_s: 0.0,
            treble_s: 0.0,
        }
    }
}

impl Visualizer for Fractal {
    fn update(&mut self, frame: &AudioFrame, _cfg: &Config, _max_bars: usize, dt: f32) {
        let f = &frame.features;
        let (bass, mid, treble, level) = (f.bass, f.mid, f.treble, f.level);

        // Two bass followers: a slow one for the overall drive and a fast
        // one whose difference from it adds frame-level shimmer.
        let k = (dt * 3.0).min(1.0);
        let kf = (dt * 25.0).min(1.0);
        self.bass_s += (bass - self.bass_s) * k;
        self.bass_fast += (bass - self.bass_fast) * kf;
        self.mid_s += (mid - self.mid_s) * k;
        self.treble_s += (treble - self.treble_s) * k;
        self.level_s += (level - self.level_s) * k;

        if let Some(strength) = self.onset.feed(bass, frame.silent, dt) {
            self.pop = (self.pop + 0.4 + strength * 0.4).min(1.0);
            self.zoom_pulse = 1.0;
            // Palette nudges on the kick, so beats read even in still shapes.
            self.color_phase += 0.08;
        }
        self.pop *= (-dt * 3.0).exp();
        self.zoom_pulse *= (-dt * 4.5).exp();

        self.theta += dt * (0.06 + 0.45 * self.bass_s + 0.8 * self.pop);
        self.rot += dt * (0.015 + 0.20 * self.mid_s + 0.3 * self.pop);
        self.color_phase += dt * (0.04 + 0.4 * self.treble_s);
        self.time += dt;
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        // c on the main cardioid: c(θ) = e^{iθ}/2 − e^{2iθ}/4, pushed
        // outward on beats (the set visibly shatters into dendrites) with a
        // fast-bass shimmer on top.
        let (s1, c1) = self.theta.sin_cos();
        let (s2, c2) = (2.0 * self.theta).sin_cos();
        let shimmer = (self.bass_fast - self.bass_s) * 0.04;
        let out = 1.0 + 0.010 + 0.07 * self.pop + shimmer + 0.008 * (self.time * 0.9).sin();
        let cr = (c1 / 2.0 - c2 / 4.0) * out;
        let ci = (s1 / 2.0 - s2 / 4.0) * out;

        // View window: loudness pulls the camera in, and every kick punches
        // a quick extra zoom that relaxes back.
        let half_w = 1.65
            * (1.0 - 0.16 * self.level_s + 0.03 * (self.time * 0.31).sin())
            * (1.0 - 0.10 * self.zoom_pulse);
        let px_w = area.width as f32;
        let px_h = area.height as f32 * 2.0; // half-block pixels are ~square
        let half_h = half_w * px_h / px_w;
        let (rs, rc) = self.rot.sin_cos();

        let escape = |mut zr: f32, mut zi: f32| -> Option<f32> {
            for i in 0..MAX_ITER {
                let (zr2, zi2) = (zr * zr, zi * zi);
                if zr2 + zi2 > 16.0 {
                    // Smooth (fractional) escape count for banding-free color.
                    let nu = i as f32 + 1.0 - (zr2 + zi2).sqrt().ln().log2().max(0.0);
                    return Some(nu / MAX_ITER as f32);
                }
                let t = zr2 - zi2 + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
            }
            None
        };

        // Triangular wave keeps palette cycling seamless (no wrap seam).
        let tri = |x: f32| 1.0 - (2.0 * x.fract() - 1.0).abs();

        for row in 0..area.height {
            for col in 0..area.width {
                let x = (col as f32 + 0.5) / px_w * 2.0 * half_w - half_w;
                let color_at = |p: f32| {
                    let y = half_h - p / px_h * 2.0 * half_h;
                    // Rotate the view plane (mid-driven spin).
                    let (xr, yr) = (x * rc - y * rs, x * rs + y * rc);
                    // Fast escapes are far from the set: keep them dark so
                    // the fractal glows out of blackness instead of tinting
                    // the whole screen.
                    escape(xr, yr).filter(|&t| t >= 0.06).map(|t| {
                        theme.bar_color(tri(t * 2.4 + self.color_phase).clamp(0.0, 1.0))
                    })
                };
                let upper = color_at(row as f32 * 2.0 + 0.5);
                let lower = color_at(row as f32 * 2.0 + 1.5);
                put_half_block(&mut buf[(area.x + col, area.y + row)], upper, lower);
            }
        }
    }
}
