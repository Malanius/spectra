pub mod bars;
pub mod mirror;
pub mod ripples;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::config::Config;
use crate::theme::ThemeId;

/// One analysis window of audio, shared by all visualizer styles.
pub struct AudioFrame<'a> {
    pub mono: &'a [f32],
    pub left: &'a [f32],
    pub right: &'a [f32],
    pub rate: u32,
    /// True when no fresh samples are arriving (paused / suspended stream).
    pub silent: bool,
}

/// A visualization style. To add a new one: implement this trait, register
/// it in [`create_all`], and add its id to [`IDS`].
pub trait Visualizer {
    /// Stable identifier used in the config file and --style flag.
    fn id(&self) -> &'static str;
    /// Human-readable name shown in the header and status line.
    fn name(&self) -> &'static str;
    /// Advance state by `dt` seconds. `max_bars` is how many bars currently
    /// fit on screen; styles that don't draw bars may ignore it.
    fn update(&mut self, frame: &AudioFrame, cfg: &Config, max_bars: usize, dt: f32);
    /// Draw into `area`. Must not assume any particular minimum size.
    fn render(&self, area: Rect, buf: &mut Buffer, theme: ThemeId, cfg: &Config);
}

/// Ids of all styles, in cycle order (single source for CLI validation).
pub const IDS: [&str; 3] = ["bars", "mirror", "ripples"];

/// Instantiate every visualizer, in the same order as [`IDS`].
pub fn create_all(cfg: &Config) -> Vec<Box<dyn Visualizer>> {
    vec![
        Box::new(bars::Bars::new(cfg)),
        Box::new(mirror::Mirror::new(cfg)),
        Box::new(ripples::Ripples::new(cfg)),
    ]
}

/// Lower-eighth block characters, index i covers (i+1)/8 of the cell.
pub(crate) const EIGHTHS: [&str; 7] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇"];

/// Draw `sym` across a horizontal span of `width` cells, clipped to `area`.
pub(crate) fn put_hspan(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    width: u16,
    y: u16,
    sym: &str,
    style: Style,
) {
    if y < area.y || y >= area.y + area.height {
        return;
    }
    for dx in 0..width {
        let px = x + dx;
        if px >= area.x && px < area.x + area.width {
            buf[(px, y)].set_symbol(sym).set_style(style);
        }
    }
}

/// Horizontal placement shared by the bar-based styles: returns
/// (x of first bar, cell stride between bars).
pub(crate) fn bar_layout(area: Rect, n: usize, cfg: &Config) -> (u16, u16) {
    let stride = cfg.bar_width + cfg.gap;
    let total = (n as u16) * stride - cfg.gap.min(stride);
    (area.x + area.width.saturating_sub(total) / 2, stride)
}
