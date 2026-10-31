use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{AudioFrame, BrailleCanvas, PeakScaler, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Goniometer / vectorscope: plots left vs. right sample pairs rotated 45°,
/// with phosphor-style persistence. Mono material collapses to a vertical
/// line, wide stereo blooms into a cloud, out-of-phase content leans
/// horizontal.
pub struct Vector {
    /// Persistence field in a fixed square virtual resolution.
    field: Vec<f32>,
    scaler: PeakScaler,
    /// Smoothed L/R correlation, -1 (out of phase) .. +1 (mono).
    corr: f32,
    /// Fraction of this frame's samples that fell outside the plot.
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    clipped: f32,
}

/// Virtual pixel grid edge (square). Braille dots are ~square on screen, so
/// this maps cleanly onto any terminal size.
const N: usize = 160;
/// Phosphor decay rate per second.
const DECAY: f32 = 7.0;
/// Phosphor added per sample. The field holds hit density, not brightness:
/// render tone-maps it, so dense regions keep a gradient instead of
/// saturating into a flat blob.
const DEPOSIT: f32 = 0.12;
/// Density cap, so a long-held mono line can't build up forever.
const MAX_DENSITY: f32 = 6.0;

impl Vector {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            field: vec![0.0; N * N],
            scaler: PeakScaler::new(0.85),
            corr: 0.0,
            clipped: 0.0,
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

        // Plot the freshest slice of samples (~one frame's worth), and
        // measure the L/R correlation over the same slice.
        let want = ((frame.rate as f32 * dt) as usize + 64).min(frame.left.len());
        let start = frame.left.len() - want;
        let half = (N / 2) as f32;
        let (mut ll, mut rr, mut lr) = (0.0f32, 0.0f32, 0.0f32);
        // Peak of the plotted coordinates. Mid reaches √2 × the sample peak
        // for centered material, so scaling on the sample peak overshoots.
        let mut extent = 0.0f32;
        for (&l, &r) in frame.left[start..].iter().zip(&frame.right[start..]) {
            ll += l * l;
            rr += r * r;
            lr += l * r;
            extent = extent.max((l + r).abs()).max((l - r).abs());
        }

        // Auto-scale on the recent extent; manual gain stacks on top.
        self.scaler.follow(extent * std::f32::consts::FRAC_1_SQRT_2, dt);
        let scale = self.scaler.scale(cfg.gain_db);
        // Only skip when both channels are silent: one silent channel is
        // a real reading of 0 (hard-panned material), not a missing one.
        if ll + rr > 1e-9 {
            let c = lr / (ll * rr).sqrt().max(1e-9);
            self.corr += (c - self.corr) * (dt * 8.0).min(1.0);
        }
        let mut clipped = 0;
        for (&l, &r) in frame.left[start..].iter().zip(&frame.right[start..]) {
            // Rotate 45°: x = side, y = mid (L+R). Side is R-L so that
            // left-only material leans to the upper left, as on hardware
            // goniometers.
            let x = (r - l) * std::f32::consts::FRAC_1_SQRT_2 * scale;
            let y = (l + r) * std::f32::consts::FRAC_1_SQRT_2 * scale;
            if x.abs() >= 1.0 || y.abs() >= 1.0 {
                // Off the plot (manual gain, or a peak the scaler hasn't
                // caught up with). Clamping would draw a false border.
                clipped += 1;
                continue;
            }
            let px = (half + x * half) as usize;
            let py = (half - y * half) as usize;
            let cell = &mut self.field[py * N + px];
            *cell = (*cell + DEPOSIT).min(MAX_DENSITY);
        }
        self.clipped = clipped as f32 / want.max(1) as f32;
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        vec![
            ("corr", self.corr),
            ("scale", self.scaler.scale(0.0)),
            ("clipped", self.clipped),
        ]
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
                let v = 1.0 - (-self.field[py * N + px]).exp();
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
            // L and R at the top ends of the diagonals, where hard-panned
            // material points.
            let (cx, cy) = (area.x + area.width / 2, area.y + area.height / 2);
            let reach = (edge as u16 / 2 / 2).min(cx - area.x - 1); // dots → cells
            let ry = (edge as u16 / 2 / 4).min(cy - area.y);
            buf[(cx - reach, cy - ry)].set_symbol("L").set_style(style);
            buf[(cx + reach, cy - ry)].set_symbol("R").set_style(style);
        }

        // Correlation meter along the bottom: +1 mono … 0 wide … −1 phase
        // problems.
        if area.width >= 24 && area.height >= 7 {
            let meter_w = (area.width / 3).clamp(11, 41) | 1; // odd width
            let x0 = area.x + (area.width - meter_w) / 2;
            let y = area.y + area.height - 1;
            let dim = Style::default().fg(theme.dim());
            for i in 0..meter_w {
                let sym = if i == meter_w / 2 { "┼" } else { "─" };
                buf[(x0 + i, y)].set_symbol(sym).set_style(dim);
            }
            if x0 >= area.x + 3 {
                for (j, ch) in "-1".chars().enumerate() {
                    buf[(x0 - 3 + j as u16, y)].set_char(ch).set_style(dim);
                }
                for (j, ch) in "+1".chars().enumerate() {
                    buf[(x0 + meter_w + 1 + j as u16, y)].set_char(ch).set_style(dim);
                }
            }
            let pos = ((self.corr + 1.0) / 2.0 * (meter_w - 1) as f32).round() as u16;
            buf[(x0 + pos.min(meter_w - 1), y)]
                .set_symbol("●")
                .set_style(Style::default().fg(theme.accent()));
        }
    }
}
