use std::collections::VecDeque;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{AudioFrame, OnsetDetector, Rng, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Starfield: beats fire particle bursts from the center; treble drizzles
/// slow drifters from random spots. Particles fade through the theme
/// gradient as they age.
pub struct Particles {
    particles: VecDeque<Particle>,
    onset: OnsetDetector,
    rng: Rng,
    drizzle: f32,
    cap: usize,
}

struct Particle {
    /// Position and velocity in normalized [0,1]² space.
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
    /// Gradient position of this particle's color.
    hue: f32,
    strength: f32,
}

const BASE_MAX_PARTICLES: f32 = 320.0;

impl Particles {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            particles: VecDeque::new(),
            // More sensitive than the shared feature onset: bursts should
            // fire on softer transients than other styles' beat effects.
            onset: OnsetDetector::new(1.12, 0.12, 0.13),
            rng: Rng(0xa5a5_1234_dead_beef),
            drizzle: 0.0,
            cap: BASE_MAX_PARTICLES as usize,
        }
    }

    fn spawn(&mut self, p: Particle) {
        if self.particles.len() >= self.cap {
            self.particles.pop_front(); // drop the oldest
        }
        self.particles.push_back(p);
    }
}

impl Visualizer for Particles {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        let density = cfg.particles.density;
        self.cap = ((BASE_MAX_PARTICLES * density) as usize).clamp(64, 1024);
        let (bass, treble) = (frame.features.bass, frame.features.treble);

        // Beat → radial burst from the center, bass hues, speed by energy.
        if let Some(strength) = self.onset.feed(bass, frame.silent, dt) {
            let count = ((16.0 + strength * 22.0) * density) as usize;
            for _ in 0..count {
                let ang = self.rng.f32() * std::f32::consts::TAU;
                let speed = (0.16 + 0.5 * strength) * (0.5 + self.rng.f32());
                let hue = 0.55 + 0.45 * self.rng.f32();
                let life = 1.0 + self.rng.f32() * 1.0;
                let p = Particle {
                    x: 0.5,
                    y: 0.5,
                    // Compensate cell aspect so bursts expand as circles.
                    vx: ang.cos() * speed,
                    vy: ang.sin() * speed * 0.55,
                    age: 0.0,
                    life,
                    hue,
                    strength: (0.85 + strength).min(1.6),
                };
                self.spawn(p);
            }
        }

        // Drizzle: activity anywhere in the spectrum keeps stars drifting.
        if !frame.silent {
            self.drizzle += dt * (16.0 * treble + 6.0 * bass) * density;
        }
        while self.drizzle >= 1.0 {
            self.drizzle -= 1.0;
            let (x, y) = (self.rng.f32(), self.rng.f32());
            let ang = self.rng.f32() * std::f32::consts::TAU;
            let life = 0.8 + self.rng.f32() * 0.9;
            let hue = 0.15 + 0.35 * self.rng.f32();
            let p = Particle {
                x,
                y,
                vx: ang.cos() * 0.04,
                vy: ang.sin() * 0.025 - 0.02,
                age: 0.0,
                life,
                hue,
                strength: 0.55 + 0.5 * treble,
            };
            self.spawn(p);
        }

        for p in &mut self.particles {
            p.age += dt;
            let drag = (-dt * 0.9).exp();
            p.vx *= drag;
            p.vy *= drag;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
        }
        self.particles
            .retain(|p| p.age < p.life && (-0.05..1.05).contains(&p.x) && (-0.05..1.05).contains(&p.y));
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        const RAMP: [&str; 5] = ["·", "∙", "•", "✦", "✹"];
        for p in &self.particles {
            let px = (p.x * area.width as f32) as i32;
            let py = (p.y * area.height as f32) as i32;
            if px < 0 || py < 0 || px >= area.width as i32 || py >= area.height as i32 {
                continue;
            }
            let brightness = ((1.0 - p.age / p.life) * p.strength).clamp(0.0, 1.0);
            if brightness < 0.04 {
                continue;
            }
            let idx = ((brightness * RAMP.len() as f32) as usize).min(RAMP.len() - 1);
            let style = Style::default().fg(theme.bar_color(p.hue * brightness.max(0.4)));
            buf[(area.x + px as u16, area.y + py as u16)]
                .set_symbol(RAMP[idx])
                .set_style(style);
        }
    }
}
