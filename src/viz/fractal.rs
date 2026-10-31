use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{put_half_block, AudioFrame, OnsetDetector, Visualizer};
use crate::config::Config;
use crate::theme::{dim_color, ThemeId};

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
    /// Strength of the beat fired this frame, 0 if none.
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    last_onset: f32,
}

const MAX_ITER: u32 = 40;
/// Escape time (fraction of MAX_ITER) at which a point reaches full
/// brightness; faster escapes fade toward black.
const GLOW_T: f32 = 0.35;
/// ANSI themes can't fade, so faster escapes than this are left blank.
const ANSI_MIN_T: f32 = 0.14;

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
            last_onset: 0.0,
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

        self.last_onset = 0.0;
        if let Some(strength) = self.onset.feed(bass, frame.silent, dt) {
            self.last_onset = strength;
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

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        vec![
            ("theta", self.theta % std::f32::consts::TAU),
            ("level_s", self.level_s),
            ("pop", self.pop),
            ("zoom_pulse", self.zoom_pulse),
            ("onset", self.last_onset),
        ]
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

        // Color of one sample point, in screen pixel coordinates.
        let sample = |px: f32, py: f32| -> Option<Color> {
            let x = px / px_w * 2.0 * half_w - half_w;
            let y = half_h - py / px_h * 2.0 * half_h;
            // Rotate the view plane (mid-driven spin).
            let (xr, yr) = (x * rc - y * rs, x * rs + y * rc);
            // Fast escapes are far from the set: fade them toward black so
            // the fractal glows out of the dark. Drawing them at full
            // brightness made a flat halo fill the screen, and a hard
            // cutoff carved a black ellipse.
            let t = escape(xr, yr)?;
            match theme.bar_color(tri(t * 2.4 + self.color_phase).clamp(0.0, 1.0)) {
                c @ Color::Rgb(..) => {
                    let k = (t / GLOW_T).min(1.0).powf(1.5);
                    (k > 0.02).then(|| dim_color(c, k))
                }
                // ANSI colors can't dim: draw only where the RGB glow
                // would be past a quarter brightness.
                c => (t >= ANSI_MIN_T).then_some(c),
            }
        };
        // 2×2 supersampling for RGB themes: near the set neighboring pixels
        // escape at very different times, so single samples speckled the
        // boundary with random colors. The set's interior counts as black,
        // which also anti-aliases its edge.
        let rgb = matches!(theme.bar_color(0.5), Color::Rgb(..));
        let pixel = |px: f32, py: f32| -> Option<Color> {
            if !rgb {
                return sample(px + 0.5, py + 0.5); // ANSI: no averaging
            }
            let (mut r, mut g, mut b, mut lit) = (0u32, 0u32, 0u32, 0);
            for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                if let Some(Color::Rgb(cr, cg, cb)) = sample(px + dx, py + dy) {
                    (r, g, b, lit) = (r + cr as u32, g + cg as u32, b + cb as u32, lit + 1);
                }
            }
            (lit > 0).then_some(Color::Rgb((r / 4) as u8, (g / 4) as u8, (b / 4) as u8))
        };

        for row in 0..area.height {
            for col in 0..area.width {
                let (x, y) = (col as f32, row as f32 * 2.0);
                let (upper, lower) = (pixel(x, y), pixel(x, y + 1.0));
                put_half_block(&mut buf[(area.x + col, area.y + row)], upper, lower);
            }
        }
    }
}
