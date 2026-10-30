use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{AudioFrame, OnsetDetector, Rng, Visualizer};
use crate::config::Config;
use crate::dsp::SMOOTHING_PRESETS;
use crate::theme::ThemeId;

/// Ripples on a lake: kicks spawn large, fast ripples near the center, hats
/// and other treble hits drop small, short-lived ones across the surface
/// like rain. Each ripple is
/// an expanding wave packet; overlapping packets interfere, and the summed
/// surface height picks the glyph (intensity) and color (crest vs. trough).
pub struct Ripples {
    ripples: VecDeque<Ripple>,
    rain_acc: f32,
    rng: Rng,
    onset: OnsetDetector,
    /// Treble onsets, one raindrop each.
    hat: OnsetDetector,
    /// Treble smoothed with the user's preset, for the background drizzle.
    treble_s: f32,
}

struct Ripple {
    /// Origin in normalized [0,1]² screen coordinates.
    ox: f32,
    oy: f32,
    age: f32,
    strength: f32,
    /// Spawned by a bass onset rather than rain.
    big: bool,
}

impl Ripple {
    /// Ring expansion speed, in column-width units per second.
    fn speed(&self) -> f32 {
        if self.big { 30.0 } else { 12.0 }
    }

    /// Amplitude decay per second. Kick rings must fade before the next
    /// kick or the lake turns into a standing interference pattern.
    fn decay(&self) -> f32 {
        if self.big { 1.4 } else { 2.2 }
    }

    fn amp(&self) -> f32 {
        self.strength * (-self.decay() * self.age).exp()
    }
}

/// Carrier wavelength in column units (ring-to-ring spacing).
const WAVELEN: f32 = 4.5;
/// Width of the gaussian wave packet, in column units.
const SIGMA: f32 = 1.7;
const MAX_RIPPLES: usize = 28;

impl Ripples {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            ripples: VecDeque::with_capacity(MAX_RIPPLES),
            rain_acc: 0.0,
            rng: Rng(0x9e3779b97f4a7c15),
            onset: OnsetDetector::new(2.0, 0.08, 0.15),
            hat: OnsetDetector::new(1.8, 0.06, 0.08),
            treble_s: 0.0,
        }
    }

    fn spawn(&mut self, ox: f32, oy: f32, strength: f32, big: bool) {
        if self.ripples.len() >= MAX_RIPPLES {
            // Drop the oldest raindrop, so rain never evicts a kick ring.
            let i = self.ripples.iter().position(|r| !r.big).unwrap_or(0);
            self.ripples.remove(i);
        }
        self.ripples.push_back(Ripple {
            ox,
            oy,
            age: 0.0,
            strength,
            big,
        });
    }
}

impl Visualizer for Ripples {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        // Smooth the shared (fast-attack) features with the user's preset —
        // same asymmetric attack/decay math the analyzer applies to bars.
        let (_, attack, decay) = SMOOTHING_PRESETS[cfg.smoothing];
        let scale = (dt * 60.0).clamp(0.25, 4.0);
        let follow = |current: &mut f32, target: f32| {
            let a = if target > *current { attack } else { decay };
            *current += (target - *current) * (1.0 - (1.0 - a).powf(scale));
        };
        follow(&mut self.treble_s, frame.features.treble);
        let treble = self.treble_s;

        // Bass onsets: a sharp rise makes a big ripple somewhere near the
        // middle of the lake. Detected on the raw bass, since smoothing
        // blunts the very rise the detector looks for.
        if let Some(strength) = self.onset.feed(frame.features.bass, frame.silent, dt) {
            let (jx, jy) = (self.rng.f32(), self.rng.f32());
            self.spawn(
                0.5 + (jx - 0.5) * 0.4,
                0.5 + (jy - 0.5) * 0.4,
                0.7 + strength,
                true,
            );
        }

        // Rain: each treble hit drops a ripple at a random spot, and steady
        // treble keeps a light drizzle going between hits.
        let mut drops = 0;
        if self.hat.feed(frame.features.treble, frame.silent, dt).is_some() {
            drops += 1;
        }
        if !frame.silent {
            self.rain_acc += dt * 1.5 * treble;
        }
        while self.rain_acc >= 1.0 {
            self.rain_acc -= 1.0;
            drops += 1;
        }
        for _ in 0..drops {
            let (x, y, s) = (self.rng.f32(), self.rng.f32(), self.rng.f32());
            self.spawn(
                0.06 + x * 0.88,
                0.06 + y * 0.88,
                0.22 + 0.4 * treble + 0.15 * s,
                false,
            );
        }

        for r in &mut self.ripples {
            r.age += dt;
        }
        self.ripples.retain(|r| r.amp() > 0.06 && r.age < 15.0);
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 || self.ripples.is_empty() {
            return;
        }
        // Work in column-width units; a terminal row is ~2 columns tall,
        // so scale y by 2 to keep the rings circular.
        let w = area.width as f32;
        let h2 = area.height as f32 * 2.0;
        let k = std::f32::consts::TAU / WAVELEN;
        let cutoff = 3.0 * SIGMA;

        // Precompute per-ripple values that don't depend on the cell.
        let rings: Vec<(f32, f32, f32, f32)> = self
            .ripples
            .iter()
            .map(|r| {
                let radius = r.age * r.speed();
                let amp = r.amp() / (1.0 + radius * 0.15).sqrt();
                (r.ox * w, r.oy * h2, radius, amp)
            })
            .collect();

        const RAMP: [&str; 6] = ["·", "∘", "░", "▒", "▓", "█"];
        for py in 0..area.height {
            let y = (py as f32 + 0.5) * 2.0;
            for px in 0..area.width {
                let x = px as f32 + 0.5;
                let mut height = 0.0f32;
                for &(ox, oy, radius, amp) in &rings {
                    let dx = x - ox;
                    let dy = y - oy;
                    let reach = radius + cutoff;
                    if dx.abs() > reach || dy.abs() > reach {
                        continue;
                    }
                    let d = (dx * dx + dy * dy).sqrt();
                    let u = d - radius;
                    if u.abs() > cutoff {
                        continue;
                    }
                    let envelope = (-u * u / (2.0 * SIGMA * SIGMA)).exp();
                    height += amp * envelope * (u * k).cos();
                }
                let mag = height.abs() * 1.6;
                if mag < 0.14 {
                    continue;
                }
                let idx = ((mag * RAMP.len() as f32) as usize).min(RAMP.len() - 1);
                // Crests take the hot end of the gradient, troughs the cool end.
                let t = 0.5 + height * 0.8;
                let style = Style::default().fg(theme.bar_color(t));
                buf[(area.x + px, area.y + py)]
                    .set_symbol(RAMP[idx])
                    .set_style(style);
            }
        }
    }
}
