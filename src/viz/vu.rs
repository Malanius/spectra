use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{rms_db, AudioFrame, BrailleCanvas, LevelNorm, Visualizer};
use crate::config::Config;
use crate::theme::ThemeId;

/// Two analog VU meters with needle physics (a slightly underdamped spring,
/// like the real mechanical movement) and latching peak LEDs.
pub struct Vu {
    channels: [Channel; 2],
    norm: LevelNorm,
}

struct Channel {
    needle: f32,
    velocity: f32,
    target: f32,
    peak_hold: f32,
    db_display: f32,
}

/// Meter range shown on the arc, in dB below the reference ceiling.
const RANGE_DB: f32 = 23.0;
/// Needle spring constant / damping — mild overshoot like a real VU.
const SPRING_K: f32 = 140.0;
const SPRING_C: f32 = 16.0;
/// Arc sweep half-angle from vertical, in radians.
const SWEEP: f32 = 0.96;

impl Vu {
    pub fn new(_cfg: &Config) -> Self {
        Self {
            channels: [
                Channel::new(),
                Channel::new(),
            ],
            norm: LevelNorm::new(),
        }
    }
}

impl Channel {
    fn new() -> Self {
        Self {
            needle: 0.0,
            velocity: 0.0,
            target: 0.0,
            peak_hold: 0.0,
            db_display: -60.0,
        }
    }

    fn tick(&mut self, dt: f32) {
        // Integrate the spring in small steps for stability at low fps.
        let steps = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let acc = SPRING_K * (self.target - self.needle) - SPRING_C * self.velocity;
            self.velocity += acc * h;
            self.needle += self.velocity * h;
        }
        self.needle = self.needle.clamp(-0.05, 1.05);
        self.peak_hold = (self.peak_hold - dt).max(0.0);
    }
}

impl Visualizer for Vu {
    fn id(&self) -> &'static str {
        "vu"
    }

    fn name(&self) -> &'static str {
        "VU meters"
    }

    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        // Adapt the shared ceiling once from the loudest channel, then read
        // both channels against it (dt = 0 → no further adaptation).
        let db_l = if frame.silent { -120.0 } else { rms_db(frame.left) };
        let db_r = if frame.silent { -120.0 } else { rms_db(frame.right) };
        let loudest = db_l.max(db_r);
        let _ = self
            .norm
            .norm(loudest, cfg.auto_gain, cfg.gain_db, RANGE_DB, dt);

        for (ch, db) in self.channels.iter_mut().zip([db_l, db_r]) {
            ch.target = self.norm.norm(db, cfg.auto_gain, cfg.gain_db, RANGE_DB, 0.0);
            ch.db_display = if db < -90.0 { -90.0 } else { db };
            if ch.target > 0.92 {
                ch.peak_hold = 0.8;
            }
            ch.tick(dt);
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width < 20 || area.height < 6 {
            return;
        }
        // Side-by-side panels; stacked when the terminal is narrow and tall.
        let vertical = area.width < 56 && area.height >= 14;
        let panels: [Rect; 2] = if vertical {
            let h = area.height / 2;
            [
                Rect { height: h, ..area },
                Rect {
                    y: area.y + h,
                    height: area.height - h,
                    ..area
                },
            ]
        } else {
            let w = area.width / 2;
            [
                Rect { width: w, ..area },
                Rect {
                    x: area.x + w,
                    width: area.width - w,
                    ..area
                },
            ]
        };

        for (i, (ch, panel)) in self.channels.iter().zip(panels).enumerate() {
            draw_meter(ch, panel, buf, theme, if i == 0 { "L" } else { "R" });
        }
    }
}

fn draw_meter(ch: &Channel, panel: Rect, buf: &mut Buffer, theme: ThemeId, tag: &str) {
    if panel.height < 4 || panel.width < 12 {
        return;
    }
    // Bottom row is text: channel tag, dB readout, peak LED.
    let canvas_area = Rect {
        height: panel.height - 1,
        ..panel
    };
    let mut canvas = BrailleCanvas::new(canvas_area.width, canvas_area.height);
    let (wd, hd) = (canvas.width_dots(), canvas.height_dots());

    // Needle pivot sits below the visible area so only the top of the
    // movement shows — like a real meter window.
    let cx = wd / 2;
    let cy = hd + hd / 3;
    let radius = (cy - 2) as f32;
    let inner = radius * 0.86;

    // Scale arc with red zone (top ~25% of the sweep).
    let arc_steps = (wd * 2).max(64);
    for s in 0..=arc_steps {
        let t = s as f32 / arc_steps as f32;
        let a = -SWEEP + t * 2.0 * SWEEP;
        let (x, y) = (
            cx as f32 + a.sin() * radius,
            cy as f32 - a.cos() * radius,
        );
        let intensity = if t > 0.75 { 0.55 } else { 0.3 };
        canvas.dot(x as i32, y as i32, intensity);
    }
    // Major ticks.
    for &t in &[0.0f32, 0.25, 0.5, 0.75, 0.9, 1.0] {
        let a = -SWEEP + t * 2.0 * SWEEP;
        let (sa, ca) = (a.sin(), a.cos());
        canvas.line(
            (cx as f32 + sa * inner) as i32,
            (cy as f32 - ca * inner) as i32,
            (cx as f32 + sa * radius) as i32,
            (cy as f32 - ca * radius) as i32,
            if t > 0.75 { 0.55 } else { 0.3 },
        );
    }
    // Needle.
    let a = -SWEEP + ch.needle.clamp(0.0, 1.0) * 2.0 * SWEEP;
    canvas.line(
        cx,
        cy.min(hd + 6),
        (cx as f32 + a.sin() * (radius * 0.97)) as i32,
        (cy as f32 - a.cos() * (radius * 0.97)) as i32,
        1.0,
    );

    canvas.blit(buf, canvas_area, |i| {
        if i > 0.9 {
            theme.accent()
        } else if i > 0.5 {
            theme.bar_color(0.95) // red zone
        } else {
            theme.dim()
        }
    });

    // Text row.
    let ty = panel.y + panel.height - 1;
    let text = format!("{tag}  {:>6.1} dB", ch.db_display);
    let style = Style::default().fg(theme.dim());
    let start = panel.x + (panel.width.saturating_sub(text.len() as u16 + 6)) / 2;
    for (i, c) in text.chars().enumerate() {
        let x = start + i as u16;
        if x < panel.x + panel.width {
            buf[(x, ty)].set_char(c).set_style(style);
        }
    }
    let led_x = start + text.len() as u16 + 3;
    if led_x + 1 < panel.x + panel.width {
        let led_style = if ch.peak_hold > 0.0 {
            Style::default()
                .fg(theme.bar_color(1.0))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim())
        };
        buf[(led_x, ty)].set_symbol("●").set_style(led_style);
    }
}
