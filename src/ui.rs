use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, Popup};
use crate::config::Config;
use crate::dsp::SMOOTHING_PRESETS;

const EIGHTHS: [&str; 7] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇"];

/// How many bars can fit on a terminal of the given width (hard cap 96).
pub fn max_bars(cfg: &Config, width: u16) -> usize {
    let per = (cfg.bar_width + cfg.gap) as usize;
    let usable = width.saturating_sub(2) as usize + cfg.gap as usize;
    (usable / per.max(1)).clamp(4, 96)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let show_labels = frame.area().height >= 12;
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(if show_labels { 1 } else { 0 }),
        Constraint::Length(1),
    ])
    .split(frame.area());

    draw_header(frame, rows[0], app);
    let geometry = draw_bars(frame, rows[1], app);
    if show_labels {
        draw_freq_labels(frame, rows[2], app, geometry);
    }
    draw_footer(frame, rows[3], app);

    match &app.popup {
        Popup::Help => draw_help(frame, app),
        Popup::Devices { devices, selected } => draw_devices(frame, app, devices, *selected),
        Popup::None => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.theme;
    let mut spans = vec![
        Span::styled(
            " ▎SPECTRA ",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("● {} ", app.device_label),
            Style::default().fg(theme.dim()),
        ),
    ];
    if let Some(msg) = app.status() {
        spans.push(Span::styled(
            format!(" {msg} "),
            Style::default()
                .fg(theme.peak_color())
                .add_modifier(Modifier::BOLD),
        ));
    }
    let right = format!(
        "{} · {} · gain {:+.0} dB{} ",
        theme.display_name(),
        SMOOTHING_PRESETS[app.cfg.smoothing].0,
        app.cfg.gain_db,
        if app.cfg.auto_gain { " · AGC" } else { "" },
    );
    let left_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = (area.width as usize).saturating_sub(left_w + right.chars().count());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, Style::default().fg(theme.dim())));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Geometry of the rendered bars: (x of first bar, cell stride per bar).
type BarGeometry = (u16, u16);

fn draw_bars(frame: &mut Frame, area: Rect, app: &App) -> BarGeometry {
    let bars = app.analyzer.bars();
    let peaks = app.analyzer.peaks();
    let theme = app.theme;
    let n = bars.len();
    if n == 0 || area.height == 0 || area.width == 0 {
        return (area.x, 1);
    }

    let bw = app.cfg.bar_width;
    let stride = bw + app.cfg.gap;
    let total = (n as u16) * stride - app.cfg.gap;
    let x0 = area.x + area.width.saturating_sub(total) / 2;
    let rows = area.height as usize;
    let buf = frame.buffer_mut();

    for (i, &v) in bars.iter().enumerate() {
        let x = x0 + i as u16 * stride;
        let fill = (v * (rows * 8) as f32).round() as usize;
        let full_cells = fill / 8;
        let rem = fill % 8;

        for row in 0..rows.min(full_cells) {
            let color = theme.bar_color((row as f32 + 0.5) / rows as f32);
            put_row(buf, area, x, bw, row, "█", color);
        }
        if rem > 0 && full_cells < rows {
            let color = theme.bar_color((full_cells as f32 + 0.5) / rows as f32);
            put_row(buf, area, x, bw, full_cells, EIGHTHS[rem - 1], color);
        }

        if app.cfg.peaks {
            let pe = (peaks[i] * (rows * 8) as f32).round() as usize;
            if pe >= 2 {
                // Cell whose top edge is nearest the peak height.
                let pcell = (pe.div_ceil(8) - 1).min(rows - 1);
                let top_fill_cell = if fill == 0 { None } else { Some((fill - 1) / 8) };
                if top_fill_cell.is_none_or(|t| pcell > t) {
                    put_row(buf, area, x, bw, pcell, "▔", theme.peak_color());
                }
            }
        }
    }
    (x0, stride)
}

fn put_row(buf: &mut Buffer, area: Rect, x: u16, width: u16, row: usize, sym: &str, fg: Color) {
    let y = area.y + area.height - 1 - row as u16;
    for dx in 0..width {
        let px = x + dx;
        if px < area.x + area.width {
            buf[(px, y)].set_symbol(sym).set_fg(fg);
        }
    }
}

fn draw_freq_labels(frame: &mut Frame, area: Rect, app: &App, (x0, stride): BarGeometry) {
    let n = app.analyzer.bars().len();
    if n == 0 {
        return;
    }
    let style = Style::default().fg(app.theme.dim());
    let buf = frame.buffer_mut();
    let targets = [
        50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0f32,
    ];
    let mut last_end: i32 = -2;
    for &t in &targets {
        // Bar whose center frequency is nearest this target.
        let (best, freq) = (0..n)
            .map(|i| (i, app.analyzer.band_freq(i)))
            .min_by(|a, b| {
                (a.1.ln() - t.ln())
                    .abs()
                    .total_cmp(&(b.1.ln() - t.ln()).abs())
            })
            .unwrap();
        if (freq.ln() - t.ln()).abs() > 0.35 {
            continue; // no bar reasonably close (out of range)
        }
        let label = if t >= 1000.0 {
            format!("{}k", t / 1000.0)
        } else {
            format!("{t}")
        };
        let cx = x0 as i32 + best as i32 * stride as i32 + app.cfg.bar_width as i32 / 2;
        let start = cx - label.len() as i32 / 2;
        if start <= last_end + 1 {
            continue;
        }
        for (j, ch) in label.chars().enumerate() {
            let px = start + j as i32;
            if px >= area.x as i32 && px < (area.x + area.width) as i32 {
                buf[(px as u16, area.y)]
                    .set_char(ch)
                    .set_style(style);
            }
        }
        last_end = start + label.len() as i32;
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.theme;
    let key = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    let txt = Style::default().fg(theme.dim());
    let line = Line::from(vec![
        Span::styled(" ↑↓", key),
        Span::styled(" gain  ", txt),
        Span::styled("+-", key),
        Span::styled(" bars  ", txt),
        Span::styled("s", key),
        Span::styled(" smooth  ", txt),
        Span::styled("p", key),
        Span::styled(" peaks  ", txt),
        Span::styled("a", key),
        Span::styled(" agc  ", txt),
        Span::styled("t", key),
        Span::styled(" theme  ", txt),
        Span::styled("d", key),
        Span::styled(" device  ", txt),
        Span::styled("?", key),
        Span::styled(" help  ", txt),
        Span::styled("q", key),
        Span::styled(" quit", txt),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn popup_rect(frame: &Frame, width: u16, height: u16) -> Rect {
    let a = frame.area();
    let w = width.min(a.width.saturating_sub(2));
    let h = height.min(a.height.saturating_sub(2));
    Rect::new(
        a.x + (a.width - w) / 2,
        a.y + (a.height - h) / 2,
        w,
        h,
    )
}

fn draw_help(frame: &mut Frame, app: &App) {
    let theme = app.theme;
    let key = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    let rows: [(&str, &str); 9] = [
        ("↑ / ↓", "input gain up / down"),
        ("+ / -", "more / fewer bars"),
        ("s", "cycle smoothing (fast/medium/smooth)"),
        ("p", "toggle peak-hold caps"),
        ("a", "toggle auto gain"),
        ("t", "cycle color theme"),
        ("d", "pick capture device"),
        ("?", "this help"),
        ("q / Esc", "quit (settings are saved)"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("  {k:<8}"), key),
                Span::raw(format!(" {v}")),
            ])
        })
        .collect();
    let area = popup_rect(frame, 52, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent()))
                .title(" Keys "),
        ),
        area,
    );
}

fn draw_devices(
    frame: &mut Frame,
    app: &App,
    devices: &[crate::audio::AudioDevice],
    selected: usize,
) {
    let theme = app.theme;
    let mut items = vec![ListItem::new("  Default output (monitor)")];
    items.extend(devices.iter().map(|d| {
        let tag = if d.is_sink { "monitor" } else { "input" };
        ListItem::new(format!("  {} [{tag}]", d.description))
    }));
    let height = (items.len() as u16 + 2).min(frame.area().height.saturating_sub(2));
    let area = popup_rect(frame, 64, height);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent()))
                    .title(" Capture device (Enter to select, Esc to close) "),
            )
            .highlight_style(
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ),
        area,
        &mut state,
    );
}
