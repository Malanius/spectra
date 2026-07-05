use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{AudioFrame, Rng, Visualizer};
use crate::config::Config;
use crate::dsp::Analyzer;
use crate::theme::ThemeId;

/// Matrix rain that dances: each screen column is driven by a frequency
/// band — its energy sets the fall speed and glow of the drops, so bass
/// pours on the left while hi-hats flicker on the right.
pub struct Matrix {
    analyzer: Analyzer,
    columns: Vec<Column>,
    rng: Rng,
    time: f32,
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
                seed: (i as u64) * 0x9e37_79b9_7f4a_7c15,
            })
            .collect();
        Self {
            analyzer: Analyzer::new(
                cfg.fft_size,
                BANDS,
                cfg.freq_min,
                cfg.freq_max,
                cfg.gain_db,
                cfg.auto_gain,
                cfg.smoothing,
            ),
            columns,
            rng,
            time: 0.0,
        }
    }
}

impl Visualizer for Matrix {
    fn id(&self) -> &'static str {
        "matrix"
    }

    fn name(&self) -> &'static str {
        "Matrix rain"
    }

    fn update(&mut self, frame: &AudioFrame, cfg: &Config, _max_bars: usize, dt: f32) {
        self.analyzer.gain_db = cfg.gain_db;
        self.analyzer.auto_gain = cfg.auto_gain;
        self.analyzer.smoothing = cfg.smoothing;
        self.analyzer.update(frame.mono, frame.rate, dt, frame.silent);
        self.time += dt;
        let bars = self.analyzer.bars();
        let level = bars.iter().sum::<f32>() / bars.len() as f32;

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
                if col.respawn_in <= 0.0 && !frame.silent {
                    col.active = true;
                    col.head = 0.0;
                    col.tail = 0.15 + self.rng.f32() * 0.3;
                    col.seed = col.seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
                }
            }
        }
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

                let style = if rel < 0.12 {
                    Style::default()
                        .fg(theme.peak_color())
                        .add_modifier(Modifier::BOLD)
                } else if rel > 0.75 {
                    Style::default().fg(theme.dim())
                } else {
                    // Fade down the gradient away from the head; louder
                    // bands glow hotter.
                    let t = ((1.0 - rel) * (0.35 + band)).clamp(0.0, 1.0);
                    Style::default().fg(theme.bar_color(t))
                };
                let mut utf8 = [0u8; 4];
                buf[(area.x + sc as u16, area.y + row as u16)]
                    .set_symbol(glyph.encode_utf8(&mut utf8))
                    .set_style(style);
            }
        }
    }
}
