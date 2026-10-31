use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{rms_db, AudioFrame, BrailleCanvas, Visualizer, LEVEL_SILENCE_DB};
use crate::config::Config;
use crate::dsp::Agc;
use crate::theme::ThemeId;

/// Two analog VU meters with needle physics (a slightly underdamped spring,
/// like the real mechanical movement) and latching peak LEDs.
pub struct Vu {
    channels: [Channel; 2],
    agc: Agc,
}

struct Channel {
    needle: f32,
    velocity: f32,
    target: f32,
    peak_hold: f32,
    /// Mean power over ~300 ms, as dBFS, for the readout. The raw window
    /// RMS swings ~10 dB per kick, too fast to read.
    db_display: f32,
    power: f32,
}

/// Meter range shown on the arc, in dB below the reference ceiling: -20 to
/// +3 VU, as on a real meter.
const RANGE_DB: f32 = 23.0;
/// Scale marks in VU (dB relative to 0 VU, 3 dB under the top), and
/// whether each is labeled.
const MARKS: [(f32, bool); 11] = [
    (-20.0, true), (-10.0, true), (-7.0, false), (-5.0, true), (-3.0, true), (-2.0, false),
    (-1.0, false), (0.0, true), (1.0, false), (2.0, false), (3.0, true),
];
/// Readout averaging time, in seconds.
const READOUT_TAU: f32 = 0.3;
/// How long the peak LED stays lit after a peak pins the scale.
const PEAK_HOLD_S: f32 = 0.4;
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
            agc: Agc::new(LEVEL_SILENCE_DB),
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
            db_display: -90.0,
            power: 0.0,
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
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        // Adapt the shared ceiling once from the loudest channel, then read
        // both channels against it.
        let db_l = if frame.silent { -120.0 } else { rms_db(frame.left) };
        let db_r = if frame.silent { -120.0 } else { rms_db(frame.right) };
        self.agc.follow(db_l.max(db_r), cfg.auto_gain, dt);

        for (ch, db) in self.channels.iter_mut().zip([db_l, db_r]) {
            ch.target = self.agc.norm(db, cfg.auto_gain, cfg.gain_db, RANGE_DB);
            let p = 10f32.powf(db / 10.0);
            ch.power += (p - ch.power) * (dt / READOUT_TAU).min(1.0);
            ch.db_display = (10.0 * (ch.power + 1e-12).log10()).max(-90.0);
            // A peak the slow needle can't show: the fast level pins the
            // top of the scale.
            if ch.target >= 1.0 {
                ch.peak_hold = PEAK_HOLD_S;
            }
            ch.tick(dt);
        }
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        let [l, r] = &self.channels;
        vec![
            ("db_l", l.db_display),
            ("ceil", self.agc.ceil_db()),
            ("target_l", l.target),
            ("needle_l", l.needle),
            ("needle_r", r.needle),
            ("led_l", (l.peak_hold > 0.0) as u8 as f32),
        ]
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width < 20 || area.height < 6 {
            return;
        }
        // Side by side or stacked, whichever fits the bigger meter.
        let vertical = {
            let (w, h) = (area.width, area.height);
            meter_radius(w, h / 2) > meter_radius(w / 2, h)
        };
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

/// Whether a panel has room for scale labels, and the margin in dots kept
/// above the arc and past its ends.
fn label_room(w: u16, h: u16) -> (bool, f32) {
    let labels = w >= 24 && h >= 8;
    (labels, if labels { 6.0 } else { 1.0 })
}

/// Arc radius in dots for a meter panel, fitted on both axes. The pivot may
/// sit below the panel (up to a fifth of the radius), so only the top of
/// the movement shows, like a real meter window.
fn meter_radius(w: u16, h: u16) -> f32 {
    let (labels, top) = label_room(w, h);
    let side = if labels { 8.0 } else { 2.0 };
    // Braille: 2×4 dots per cell; the bottom row is text.
    let (wd, hd) = (w as f32 * 2.0, h.saturating_sub(1) as f32 * 4.0);
    ((wd / 2.0 - side) / SWEEP.sin()).min((hd - top) / 0.8).max(4.0)
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
    let (labels, top) = label_room(panel.width, panel.height);
    let radius = meter_radius(panel.width, panel.height);
    // Center the arc-to-pivot block vertically, pivot no higher than the
    // panel bottom.
    let block = radius + top;
    let cy = (block + ((hd - 1) as f32 - block).max(0.0) / 2.0).round();
    let cx = (wd / 2) as f32;
    let angle = |t: f32| -SWEEP + t.clamp(0.0, 1.0) * 2.0 * SWEEP;
    let at = |t: f32, r: f32| {
        let a = angle(t);
        (cx + a.sin() * r, cy - a.cos() * r)
    };
    let red = 20.0 / RANGE_DB; // 0 VU

    // Scale arc, brighter in the red zone above 0 VU.
    let arc_steps = (radius * 4.0) as usize;
    for s in 0..=arc_steps {
        let t = s as f32 / arc_steps as f32;
        let (x, y) = at(t, radius);
        canvas.dot(x as i32, y as i32, if t >= red { 0.55 } else { 0.3 });
    }
    // Ticks, longer where labeled.
    for &(vu, major) in &MARKS {
        let t = (vu + 20.0) / RANGE_DB;
        let (x0, y0) = at(t, radius * if major { 0.88 } else { 0.93 });
        let (x1, y1) = at(t, radius);
        let i = if t >= red { 0.55 } else { 0.3 };
        canvas.line(x0 as i32, y0 as i32, x1 as i32, y1 as i32, i);
    }
    // Needle, from the pivot.
    let (nx, ny) = at(ch.needle, radius * 0.97);
    canvas.line(cx as i32, cy as i32, nx as i32, ny as i32, 1.0);

    canvas.blit(buf, canvas_area, |i| {
        if i > 0.9 {
            theme.accent()
        } else if i > 0.5 {
            theme.bar_color(0.95) // red zone
        } else {
            theme.dim()
        }
    });

    if labels {
        let mut last_end = 0u16;
        for &(vu, major) in &MARKS {
            if !major {
                continue;
            }
            let t = (vu + 20.0) / RANGE_DB;
            let (x, y) = at(t, radius + 4.0);
            let text = if vu > 0.0 { format!("+{vu}") } else { format!("{vu}") };
            let w = text.len() as u16;
            let col = (x / 2.0) as i32 - w as i32 / 2;
            let row = (y / 4.0) as i32;
            if col < 0 || row < 0 || row >= canvas_area.height as i32 {
                continue;
            }
            let col = canvas_area.x + col as u16;
            // Skip labels that would touch the previous one.
            if col < last_end + 1 || col + w > panel.x + panel.width {
                continue;
            }
            last_end = col + w;
            let fg = if t >= red { theme.bar_color(0.95) } else { theme.dim() };
            let row = canvas_area.y + row as u16;
            for (j, c) in text.chars().enumerate() {
                buf[(col + j as u16, row)].set_char(c).set_style(Style::default().fg(fg));
            }
        }
    }

    // Text row.
    let ty = panel.y + panel.height - 1;
    let text = format!("{tag}  {:>6.1} dBFS", ch.db_display);
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
