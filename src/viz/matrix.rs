use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::{AudioFrame, OnsetDetector, Rng, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::{dim_color, ThemeId};

/// Matrix rain that dances: each screen column is driven by a frequency
/// band — its energy sets the fall speed and glow of the drops, so bass
/// pours on the left while hi-hats flicker on the right. Each kick releases
/// a wave of new drops and a brief flash.
pub struct Matrix {
    analyzer: Analyzer,
    columns: Vec<Column>,
    rng: Rng,
    time: f32,
    onset: OnsetDetector,
    /// Kick flash, 1 right after an onset, decaying to 0.
    flash: f32,
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    last_onset: f32,
}

#[derive(Clone)]
struct Column {
    /// Head position in normalized screen heights; may exceed 1 while the
    /// tail drains off-screen.
    head: f32,
    /// Tail length in normalized heights.
    tail: f32,
    speed: f32,
    active: bool,
    respawn_in: f32,
    seed: u64,
}

/// Fixed virtual column count, mapped onto however many the terminal has.
const COLS: usize = 240;
const BANDS: usize = 32;
/// Share of idle columns a kick restarts at the top.
const KICK_SPAWN: f32 = 0.35;
/// Kick flash decay time, in seconds.
const FLASH_S: f32 = 0.25;
const GLYPHS: &str =
    "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ0123456789*+=<>:・";

impl Matrix {
    pub fn new(cfg: &Config) -> Self {
        let mut rng = Rng(0x1337_c0de_cafe);
        let columns = (0..COLS)
            .map(|i| Column {
                head: rng.f32() * 1.5,
                tail: 0.18 + rng.f32() * 0.22,
                speed: 0.3,
                active: rng.f32() < 0.4,
                respawn_in: rng.f32() * 2.0,
                seed: (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
            })
            .collect();
        Self {
            analyzer: Analyzer::new(cfg, BANDS),
            columns,
            rng,
            time: 0.0,
            onset: OnsetDetector::new(2.0, 0.08, 0.15),
            flash: 0.0,
            last_onset: 0.0,
        }
    }
}

impl Visualizer for Matrix {
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.sync(cfg);
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);
        self.time += dt;
        let bars = self.analyzer.bars();
        let level = bars.iter().sum::<f32>() / bars.len() as f32;

        self.flash = (self.flash - dt / FLASH_S).max(0.0);
        self.last_onset = 0.0;
        let kick = self.onset.feed(frame.features.bass, frame.silent, dt);
        if let Some(strength) = kick {
            self.last_onset = strength;
            self.flash = 1.0;
        }

        for (i, col) in self.columns.iter_mut().enumerate() {
            let band = bars[i * BANDS / COLS];
            if col.active {
                col.speed = 0.25 + band * 1.6;
                col.head += col.speed * dt;
                if col.head - col.tail > 1.05 {
                    col.active = false;
                    col.respawn_in = 0.1 + self.rng.f32() * (2.5 - 2.0 * level.min(1.0));
                }
            } else {
                col.respawn_in -= dt * (0.4 + 2.0 * band + level);
                if kick.is_some() && self.rng.f32() < KICK_SPAWN {
                    col.respawn_in = 0.0;
                }
                if col.respawn_in <= 0.0 && !frame.silent {
                    col.active = true;
                    col.head = 0.0;
                    col.tail = 0.15 + self.rng.f32() * 0.3;
                    col.seed = col.seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
                }
            }
        }
    }

    #[cfg(feature = "render")]
    fn probe(&self) -> Vec<(&'static str, f32)> {
        let active: Vec<&Column> = self.columns.iter().filter(|c| c.active).collect();
        let speed = active.iter().map(|c| c.speed).sum::<f32>() / active.len().max(1) as f32;
        let third = |k: usize| {
            let bars = self.analyzer.bars();
            let n = bars.len() / 3;
            bars[k * n..(k + 1) * n].iter().sum::<f32>() / n as f32
        };
        vec![
            ("active", active.len() as f32 / COLS as f32),
            ("flash", self.flash),
            ("onset", self.last_onset),
            ("speed", speed),
            ("band_lo", third(0)),
            ("band_hi", third(2)),
        ]
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, _cfg: &Config) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let glyphs: Vec<char> = GLYPHS.chars().collect();
        let bars = self.analyzer.bars();
        let h = area.height as f32;

        for sc in 0..area.width as usize {
            let vc = sc * COLS / area.width as usize;
            let col = &self.columns[vc];
            if !col.active {
                continue;
            }
            let band = bars[vc * BANDS / COLS];
            let head_row = col.head * h;
            let tail_rows = (col.tail * h).max(2.0);

            let first = ((head_row - tail_rows).ceil().max(0.0)) as usize;
            let last = (head_row.floor() as usize).min(area.height as usize - 1);
            for row in first..=last.min(area.height as usize - 1) {
                // 0 at the head, 1 at the end of the tail.
                let rel = (head_row - row as f32) / tail_rows;
                if !(0.0..=1.0).contains(&rel) {
                    continue;
                }
                // Glyphs churn faster in hot bands.
                let churn = (self.time * (2.0 + band * 10.0)) as u64;
                let mut hsh = col
                    .seed
                    .wrapping_add((row as u64) << 32)
                    .wrapping_add(churn);
                hsh ^= hsh >> 33;
                hsh = hsh.wrapping_mul(0xff51_afd7_ed55_8ccd);
                hsh ^= hsh >> 33;
                let glyph = glyphs[(hsh % glyphs.len() as u64) as usize];

                // Louder bands take a hotter hue; brightness fades away
                // from the head, and the kick flash lifts the whole screen.
                let hue = theme.bar_color(0.2 + 0.8 * band);
                let style = if rel < 0.08 {
                    Style::default()
                        .fg(theme.peak_color())
                        .add_modifier(Modifier::BOLD)
                } else if let Color::Rgb(..) = hue {
                    let k = (1.0 - rel).powf(1.2) * (0.75 + 0.25 * self.flash);
                    Style::default().fg(dim_color(hue, k.max(0.15)))
                } else if rel > 0.6 && self.flash < 0.5 {
                    // ANSI colors can't dim: the tail end goes to the
                    // theme's dim color instead.
                    Style::default().fg(theme.dim())
                } else {
                    Style::default().fg(hue)
                };
                let mut utf8 = [0u8; 4];
                buf[(area.x + sc as u16, area.y + row as u16)]
                    .set_symbol(glyph.encode_utf8(&mut utf8))
                    .set_style(style);
            }
        }
    }
}
