//! Offline frame renderer, a dev tool for tuning styles (`--features render`).
//!
//! `spectra render clip.wav --style ripples --out dir/` steps a WAV file
//! through the same feature extraction and visualizer code the app runs, at
//! a fixed 60 fps, and writes per style:
//!
//! - `sheet_NNN.png`: contact sheets of rendered frames, each tile labeled
//!   with its time and levels, outlined when an onset fired since the
//!   previous tile
//! - `timeline.png`: input features and the style's probes over time
//! - `metrics.csv`: the same values, one row per frame

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Parser;
use fontdue::{Font, FontSettings};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::config::{Config, ConfigKey};
use crate::theme::ThemeId;
use crate::viz::{self, AudioFeatures, AudioFrame, FeatureExtractor};

const FPS: f32 = 60.0;
const WARMUP: f32 = 5.0;

#[derive(Parser)]
#[command(name = "spectra render", about = "Render visualizer frames from a WAV file to PNG")]
struct Args {
    /// Input WAV file.
    input: PathBuf,
    /// Style id, or "all".
    #[arg(short, long, default_value = "all")]
    style: String,
    /// Output directory (one subdirectory per style).
    #[arg(short, long, default_value = "render-out")]
    out: PathBuf,
    /// Config file to use. Default: built-in defaults, not the user's config.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Color theme.
    #[arg(short, long)]
    theme: Option<String>,
    /// Visualizer area in cells.
    #[arg(long, default_value_t = 80)]
    cols: u16,
    #[arg(long, default_value_t = 24)]
    rows: u16,
    /// Start offset in seconds.
    #[arg(long, default_value_t = 0.0)]
    start: f32,
    /// Seconds to render. Default: to the end of the file.
    #[arg(long)]
    duration: Option<f32>,
    /// Frames between contact-sheet tiles (6 = every 100 ms).
    #[arg(long, default_value_t = 6)]
    every: usize,
    /// Tiles per sheet and tiles per sheet row.
    #[arg(long, default_value_t = 12)]
    tiles: usize,
    #[arg(long, default_value_t = 3)]
    sheet_cols: usize,
    /// Font size in pixels.
    #[arg(long, default_value_t = 12.0)]
    font_size: f32,
}

pub fn main(args: impl Iterator<Item = String>) -> Result<()> {
    let args = Args::parse_from(args);
    let mut cfg = match &args.config {
        Some(p) => toml::from_str(&fs::read_to_string(p)?)
            .with_context(|| format!("parsing {}", p.display()))?,
        None => Config::default(),
    };
    cfg.sanitize();
    if let Some(t) = &args.theme {
        cfg.theme = ThemeId::from_key(t).with_context(|| format!("unknown theme {t}"))?;
    }
    let styles: Vec<&viz::StyleInfo> = if args.style == "all" {
        viz::STYLES.iter().collect()
    } else {
        args.style
            .split(',')
            .map(|id| {
                viz::STYLES
                    .iter()
                    .find(|s| s.id == id)
                    .with_context(|| format!("unknown style {id}"))
            })
            .collect::<Result<_>>()?
    };

    let audio = Audio::load(&args.input)?;
    let fonts = Fonts::load(args.font_size)?;
    let total = audio.left.len() as f32 / audio.rate as f32;
    let span = args.duration.unwrap_or(total - args.start).min(total - args.start);
    if span <= 0.0 {
        bail!("nothing to render: file is {total:.1}s long");
    }
    let frames = (span * FPS) as usize;

    for style in styles {
        let dir = args.out.join(style.id);
        fs::create_dir_all(&dir)?;
        let n = render_style(style, &cfg, &audio, &fonts, &args, frames, &dir)?;
        println!("{}: {frames} frames, {n} sheets -> {}", style.id, dir.display());
    }
    Ok(())
}

/// Per-frame values logged to the CSV and the timeline.
struct Row {
    t: f32,
    features: AudioFeatures,
    /// Fraction of cells with something drawn in them.
    coverage: f32,
    probes: Vec<(&'static str, f32)>,
}

fn render_style(
    style: &viz::StyleInfo,
    cfg: &Config,
    audio: &Audio,
    fonts: &Fonts,
    args: &Args,
    frames: usize,
    dir: &Path,
) -> Result<usize> {
    let mut cfg = cfg.clone();
    cfg.visualizer = style.id.to_string();
    let mut vis = style.create(&cfg);
    let mut extractor = FeatureExtractor::new(&cfg);
    let area = Rect::new(0, 0, args.cols, args.rows);
    let max_bars = crate::ui::max_bars(&cfg, args.cols);
    let fft = cfg.fft_size;
    let (mut mono, mut left, mut right) = (vec![0.0; fft], vec![0.0; fft], vec![0.0; fft]);
    let dt = 1.0 / FPS;

    let mut rows = Vec::with_capacity(frames);
    let mut tiles = Vec::new();
    let mut sheets = 0;
    // Run up to WARMUP seconds before --start without output, so styles
    // with history (trails, ripples, particles) look as they would live.
    let warmup = (args.start.min(WARMUP) * FPS) as usize;
    for i in 0..warmup + frames {
        let t = args.start + (i as f32 - warmup as f32) * dt;
        audio.window(t, &mut left, &mut right, &mut mono);
        let features = extractor.update(&cfg, &mono, audio.rate, dt, false);
        let frame = AudioFrame {
            mono: &mono,
            left: &left,
            right: &right,
            rate: audio.rate,
            silent: false,
            features,
        };
        vis.update(&frame, &cfg, max_bars, dt);
        let Some(i) = i.checked_sub(warmup) else { continue };
        let mut buf = Buffer::empty(area);
        vis.render(area, &mut buf, cfg.theme, &cfg);
        let lit = buf.content.iter().filter(|c| !is_blank(c)).count();
        rows.push(Row {
            t,
            features,
            coverage: lit as f32 / buf.content.len().max(1) as f32,
            probes: vis.probe(),
        });

        if i % args.every == args.every - 1 {
            let since = rows.len().saturating_sub(args.every);
            let onset = rows[since..]
                .iter()
                .flat_map(|r| &r.probes)
                .filter(|(k, _)| k.ends_with("onset"))
                .fold(0.0f32, |a, &(_, v)| a.max(v));
            tiles.push(tile(&buf, fonts, rows.last().unwrap(), onset));
            if tiles.len() == args.tiles {
                write_sheet(&tiles, args.sheet_cols, &dir.join(format!("sheet_{sheets:03}.png")))?;
                sheets += 1;
                tiles.clear();
            }
        }
    }
    if !tiles.is_empty() {
        write_sheet(&tiles, args.sheet_cols, &dir.join(format!("sheet_{sheets:03}.png")))?;
        sheets += 1;
    }
    write_csv(&rows, &dir.join("metrics.csv"))?;
    write_timeline(&rows, fonts, &dir.join("timeline.png"))?;
    Ok(sheets)
}

/// Decoded WAV as stereo f32.
struct Audio {
    left: Vec<f32>,
    right: Vec<f32>,
    rate: u32,
}

impl Audio {
    fn load(path: &Path) -> Result<Self> {
        let mut r = hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
        let spec = r.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                r.samples::<i32>()
                    .map(|s| s.map(|v| v as f32 * scale))
                    .collect::<Result<_, _>>()?
            }
        };
        let ch = spec.channels.max(1) as usize;
        let left = samples.iter().step_by(ch).copied().collect();
        let right = samples.iter().skip((ch > 1) as usize).step_by(ch).copied().collect();
        Ok(Self { left, right, rate: spec.sample_rate })
    }

    /// Fill the analysis window that ends at time `t`, zero-padded before
    /// the start of the file, like the app's history buffer at startup.
    fn window(&self, t: f32, left: &mut [f32], right: &mut [f32], mono: &mut [f32]) {
        let n = left.len();
        let end = ((t * self.rate as f32) as usize).min(self.left.len());
        for i in 0..n {
            let (l, r) = match (end + i).checked_sub(n) {
                Some(j) => (self.left[j], self.right[j]),
                None => (0.0, 0.0),
            };
            left[i] = l;
            right[i] = r;
            mono[i] = (l + r) * 0.5;
        }
    }
}

// ---------------------------------------------------------------------------
// Rasterizing terminal cells

struct Fonts {
    faces: Vec<Font>,
    size: f32,
    cell_w: usize,
    cell_h: usize,
    ascent: f32,
}

impl Fonts {
    /// The system monospace font plus symbol fallbacks, found via fontconfig.
    fn load(size: f32) -> Result<Self> {
        let mut faces = Vec::new();
        for pattern in ["monospace", "Noto Sans Symbols2", "Noto Sans Symbols", "Noto Sans Math"] {
            let out = Command::new("fc-match").args(["-f", "%{file}", pattern]).output();
            let Ok(out) = out else { continue };
            let path = String::from_utf8_lossy(&out.stdout).into_owned();
            let Ok(bytes) = fs::read(&path) else { continue };
            if let Ok(f) = Font::from_bytes(bytes, FontSettings::default()) {
                faces.push(f);
            }
        }
        let Some(main) = faces.first() else {
            bail!("no fonts found (needs fc-match)");
        };
        let lm = main.horizontal_line_metrics(size).context("font has no line metrics")?;
        let cell_w = main.metrics('M', size).advance_width.round() as usize;
        let cell_h = (lm.ascent - lm.descent).round() as usize;
        Ok(Self { size, cell_w, cell_h, ascent: lm.ascent, faces })
    }

    /// Draw `ch` at pixel (x, y) (top-left of its cell) with color `fg`.
    fn glyph(&self, img: &mut Image, x: usize, y: usize, ch: char, fg: Rgb) {
        let Some(face) = self.faces.iter().find(|f| f.lookup_glyph_index(ch) != 0) else {
            img.rect(x + 1, y + 1, self.cell_w - 2, self.cell_h - 2, blend(fg, BG, 0.3));
            return;
        };
        let (mut m, mut cov) = face.rasterize(ch, self.size);
        let (mut gx, mut gy) = (
            x as i32 + m.xmin,
            y as i32 + (self.ascent.round() as i32) - m.height as i32 - m.ymin,
        );
        if m.width > self.cell_w {
            // Wide symbol from a fallback font: shrink it into the cell,
            // centered, as terminals do.
            (m, cov) = face.rasterize(ch, self.size * self.cell_w as f32 / m.width as f32);
            gx = x as i32 + (self.cell_w as i32 - m.width as i32) / 2;
            gy = y as i32 + (self.cell_h as i32 - m.height as i32) / 2;
        }
        for row in 0..m.height {
            for col in 0..m.width {
                let a = cov[row * m.width + col] as f32 / 255.0;
                if a > 0.0 {
                    img.blend(gx + col as i32, gy + row as i32, fg, a);
                }
            }
        }
    }

    fn text(&self, img: &mut Image, x: usize, y: usize, s: &str, fg: Rgb) {
        for (i, ch) in s.chars().enumerate() {
            self.glyph(img, x + i * self.cell_w, y, ch, fg);
        }
    }

    /// Draw the foreground of one terminal cell, over backgrounds already
    /// painted by the caller so glyphs that overhang their cell survive.
    /// Block elements and braille are drawn as shapes so they tile exactly,
    /// everything else through the font.
    fn cell(&self, img: &mut Image, x: usize, y: usize, cell: &Cell) {
        let (fg, bg) = cell_colors(cell);
        let (w, h) = (self.cell_w, self.cell_h);
        let Some(ch) = cell.symbol().chars().next() else { return };
        let frac = |n: usize, d: usize, of: usize| (of * n + d / 2) / d;
        match ch {
            ' ' => {}
            '▀' => img.rect(x, y, w, h / 2, fg),
            '▄' => img.rect(x, y + h / 2, w, h - h / 2, fg),
            '▌' => img.rect(x, y, w / 2, h, fg),
            '▐' => img.rect(x + w / 2, y, w - w / 2, h, fg),
            '▔' => img.rect(x, y, w, frac(1, 8, h), fg),
            '▕' => img.rect(x + w - frac(1, 8, w), y, frac(1, 8, w), h, fg),
            '▁'..='█' => {
                let n = ch as usize - '▁' as usize + 1;
                let fh = frac(n, 8, h);
                img.rect(x, y + h - fh, w, fh, fg);
            }
            '▉'..='▏' => {
                let n = 8 - (ch as usize - '▉' as usize + 1);
                img.rect(x, y, frac(n, 8, w), h, fg);
            }
            '░' | '▒' | '▓' => {
                let k = [0.25, 0.5, 0.75][ch as usize - '░' as usize];
                img.rect(x, y, w, h, blend(fg, bg, k));
            }
            '▖'..='▟' => {
                // Quadrant bits: upper-left, upper-right, lower-left, lower-right.
                let bits = [0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111];
                let b = bits[ch as usize - '▖' as usize];
                let (hw, hh) = (w / 2, h / 2);
                for (bit, qx, qy, qw, qh) in [
                    (0b1000, x, y, hw, hh),
                    (0b0100, x + hw, y, w - hw, hh),
                    (0b0010, x, y + hh, hw, h - hh),
                    (0b0001, x + hw, y + hh, w - hw, h - hh),
                ] {
                    if b & bit != 0 {
                        img.rect(qx, qy, qw, qh, fg);
                    }
                }
            }
            '\u{2800}'..='\u{28ff}' => {
                let bits = ch as u32 - 0x2800;
                // Dot order: 1-3 down the left, 4-6 down the right, 7, 8.
                const DOTS: [(usize, usize); 8] = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
                let d = (w / 3).max(1);
                for (i, &(dx, dy)) in DOTS.iter().enumerate() {
                    if bits & (1 << i) != 0 {
                        let cx = x + frac(2 * dx + 1, 4, w) - d / 2;
                        let cy = y + frac(2 * dy + 1, 8, h) - d / 2;
                        img.rect(cx, cy, d, d, fg);
                    }
                }
            }
            _ => self.glyph(img, x, y, ch, fg),
        }
    }
}

fn cell_colors(cell: &Cell) -> (Rgb, Rgb) {
    let (mut fg, mut bg) = (rgb(cell.fg, FG), rgb(cell.bg, BG));
    if cell.modifier.contains(Modifier::REVERSED) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.modifier.contains(Modifier::DIM) {
        fg = blend(fg, bg, 0.5);
    }
    (fg, bg)
}

fn is_blank(c: &Cell) -> bool {
    c.symbol() == " " && c.bg == Color::Reset
}

type Rgb = (u8, u8, u8);

const BG: Rgb = (0x14, 0x14, 0x18);
const FG: Rgb = (0xd0, 0xd0, 0xd0);

/// Map a terminal color to RGB, using xterm's default palette for ANSI.
fn rgb(c: Color, reset: Rgb) -> Rgb {
    const ANSI: [Rgb; 16] = [
        (0x00, 0x00, 0x00), (0xcd, 0x00, 0x00), (0x00, 0xcd, 0x00), (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee), (0xcd, 0x00, 0xcd), (0x00, 0xcd, 0xcd), (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f), (0xff, 0x00, 0x00), (0x00, 0xff, 0x00), (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff), (0xff, 0x00, 0xff), (0x00, 0xff, 0xff), (0xff, 0xff, 0xff),
    ];
    let idx = |i: u8| -> Rgb {
        match i {
            0..=15 => ANSI[i as usize],
            16..=231 => {
                let i = i - 16;
                let lvl = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                (lvl(i / 36), lvl(i / 6 % 6), lvl(i % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                (v, v, v)
            }
        }
    };
    match c {
        Color::Reset => reset,
        Color::Black => ANSI[0],
        Color::Red => ANSI[1],
        Color::Green => ANSI[2],
        Color::Yellow => ANSI[3],
        Color::Blue => ANSI[4],
        Color::Magenta => ANSI[5],
        Color::Cyan => ANSI[6],
        Color::Gray => ANSI[7],
        Color::DarkGray => ANSI[8],
        Color::LightRed => ANSI[9],
        Color::LightGreen => ANSI[10],
        Color::LightYellow => ANSI[11],
        Color::LightBlue => ANSI[12],
        Color::LightMagenta => ANSI[13],
        Color::LightCyan => ANSI[14],
        Color::White => ANSI[15],
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => idx(i),
    }
}

/// Mix `a` over `b` with weight `k` for `a`.
fn blend(a: Rgb, b: Rgb, k: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 * k + y as f32 * (1.0 - k)).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

struct Image {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Image {
    fn new(w: usize, h: usize, fill: Rgb) -> Self {
        let mut px = Vec::with_capacity(w * h * 3);
        for _ in 0..w * h {
            px.extend_from_slice(&[fill.0, fill.1, fill.2]);
        }
        Self { w, h, px }
    }

    fn set(&mut self, x: usize, y: usize, c: Rgb) {
        if x < self.w && y < self.h {
            let i = (y * self.w + x) * 3;
            self.px[i..i + 3].copy_from_slice(&[c.0, c.1, c.2]);
        }
    }

    fn get(&self, x: usize, y: usize) -> Rgb {
        let i = (y * self.w + x) * 3;
        (self.px[i], self.px[i + 1], self.px[i + 2])
    }

    fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let (x, y) = (x as usize, y as usize);
            let under = self.get(x, y);
            self.set(x, y, blend(c, under, a));
        }
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Rgb) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.set(xx, yy, c);
            }
        }
    }

    fn outline(&mut self, x: usize, y: usize, w: usize, h: usize, t: usize, c: Rgb) {
        self.rect(x, y, w, t, c);
        self.rect(x, y + h - t, w, t, c);
        self.rect(x, y, t, h, c);
        self.rect(x + w - t, y, t, h, c);
    }

    /// Copy `src` into this image with its top-left at (x, y).
    fn paste(&mut self, src: &Image, x: usize, y: usize) {
        for sy in 0..src.h {
            for sx in 0..src.w {
                self.set(x + sx, y + sy, src.get(sx, sy));
            }
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&self.px)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Outputs

const LABEL: Rgb = (0x90, 0x90, 0x98);
const ONSET: Rgb = (0xff, 0x8c, 0x1a);
const PAD: usize = 6;

/// One contact-sheet tile: a label line above the rendered frame, outlined
/// in orange when an onset fired since the previous tile.
fn tile(buf: &Buffer, fonts: &Fonts, row: &Row, onset: f32) -> Image {
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let area = buf.area;
    let (w, h) = (area.width as usize * cw, area.height as usize * ch);
    let mut img = Image::new(w + 2 * PAD, h + ch + 2 * PAD, (0x08, 0x08, 0x0a));
    let f = &row.features;
    let mut label = format!(
        "{:6.2}s  bass {:.2} mid {:.2} treb {:.2}",
        row.t, f.bass, f.mid, f.treble
    );
    if onset > 0.0 {
        label += &format!("  ONSET {onset:.2}");
    }
    fonts.text(&mut img, PAD, PAD / 2, &label, if onset > 0.0 { ONSET } else { LABEL });
    let at = |x: u16, y: u16| (PAD + x as usize * cw, PAD + ch + y as usize * ch);
    for y in 0..area.height {
        for x in 0..area.width {
            let (px, py) = at(x, y);
            img.rect(px, py, cw, ch, cell_colors(&buf[(x, y)]).1);
        }
    }
    for y in 0..area.height {
        for x in 0..area.width {
            let (px, py) = at(x, y);
            fonts.cell(&mut img, px, py, &buf[(x, y)]);
        }
    }
    if onset > 0.0 {
        img.outline(PAD - 3, PAD + ch - 3, w + 6, h + 6, 2, ONSET);
    }
    img
}

fn write_sheet(tiles: &[Image], cols: usize, path: &Path) -> Result<()> {
    let (tw, th) = (tiles[0].w, tiles[0].h);
    let rows = tiles.len().div_ceil(cols);
    let mut sheet = Image::new(tw * cols.min(tiles.len()), th * rows, (0x08, 0x08, 0x0a));
    for (i, t) in tiles.iter().enumerate() {
        sheet.paste(t, (i % cols) * tw, (i / cols) * th);
    }
    sheet.save(path)
}

fn write_csv(rows: &[Row], path: &Path) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    write!(w, "t,bass,mid,treble,level,rms_db,peak,coverage")?;
    if let Some(r) = rows.first() {
        for (k, _) in &r.probes {
            write!(w, ",{k}")?;
        }
    }
    writeln!(w)?;
    for r in rows {
        let f = &r.features;
        write!(
            w,
            "{:.4},{:.4},{:.4},{:.4},{:.4},{:.2},{:.4},{:.4}",
            r.t, f.bass, f.mid, f.treble, f.level, f.rms_db, f.peak, r.coverage
        )?;
        for (_, v) in &r.probes {
            write!(w, ",{v:.4}")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

/// Stacked line plots of the features and probes, one lane per series,
/// each scaled to its own 0..max. Onset probes draw as vertical ticks, also
/// repeated faintly across every lane so cause and effect line up.
fn write_timeline(rows: &[Row], fonts: &Fonts, path: &Path) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut series: Vec<(String, Vec<f32>)> = vec![
        ("bass".into(), rows.iter().map(|r| r.features.bass).collect()),
        ("mid".into(), rows.iter().map(|r| r.features.mid).collect()),
        ("treble".into(), rows.iter().map(|r| r.features.treble).collect()),
        ("level".into(), rows.iter().map(|r| r.features.level).collect()),
        ("coverage".into(), rows.iter().map(|r| r.coverage).collect()),
    ];
    for (i, (k, _)) in rows[0].probes.iter().enumerate() {
        series.push((k.to_string(), rows.iter().map(|r| r.probes[i].1).collect()));
    }

    let label_w = fonts.cell_w * 16;
    let plot_w = rows.len().clamp(600, 2400);
    let lane_h = 70;
    let axis_h = fonts.cell_h + 4;
    let mut img = Image::new(label_w + plot_w + PAD, series.len() * lane_h + axis_h, BG);
    let x_of = |i: usize| label_w + i * plot_w / rows.len();

    // Onset ticks from every *onset series, behind all lanes.
    for (name, vals) in &series {
        if !name.ends_with("onset") {
            continue;
        }
        for (i, &v) in vals.iter().enumerate() {
            if v > 0.0 {
                img.rect(x_of(i), 0, 1, series.len() * lane_h, blend(ONSET, BG, 0.25));
            }
        }
    }
    for (lane, (name, vals)) in series.iter().enumerate() {
        let top = lane * lane_h;
        let bottom = top + lane_h - 6;
        img.rect(label_w, bottom, plot_w, 1, (0x30, 0x30, 0x38));
        let max = vals.iter().fold(0.0f32, |a, &v| a.max(v));
        fonts.text(&mut img, PAD, top + 8, name, FG);
        fonts.text(&mut img, PAD, top + 8 + fonts.cell_h, &format!("max {max:.3}"), LABEL);
        if max <= 0.0 {
            continue;
        }
        let y_of = |v: f32| bottom - ((v / max) * (lane_h - 14) as f32) as usize;
        if name.ends_with("onset") {
            for (i, &v) in vals.iter().enumerate() {
                if v > 0.0 {
                    let y = y_of(v);
                    img.rect(x_of(i), y, 2, bottom - y, ONSET);
                }
            }
            continue;
        }
        let color = (0x6c, 0xc4, 0xff);
        let mut prev = y_of(vals[0]);
        for (i, &v) in vals.iter().enumerate() {
            let y = y_of(v);
            let (a, b) = (prev.min(y), prev.max(y));
            img.rect(x_of(i), a, (x_of(i + 1) - x_of(i)).max(1), b - a + 1, color);
            prev = y;
        }
    }
    // Time axis, a tick per second.
    let base = series.len() * lane_h;
    let t0 = rows[0].t;
    let secs = (rows.last().unwrap().t - t0) as usize;
    let step = (secs / 20).max(1);
    for s in (0..=secs).step_by(step) {
        let i = ((s as f32) * FPS) as usize;
        if i >= rows.len() {
            break;
        }
        img.rect(x_of(i), 0, 1, base, (0x26, 0x26, 0x2c));
        fonts.text(&mut img, x_of(i) + 2, base + 2, &format!("{:.0}s", t0 + s as f32), LABEL);
    }
    img.save(path)
}
