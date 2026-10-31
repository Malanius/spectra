use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Popup};
use crate::audio::DEFAULT_DEVICE_LABEL;
use crate::config::{Config, MAX_BARS};
use crate::dsp::SMOOTHING_PRESETS;

/// How many bars can fit on a terminal of the given width (hard cap `MAX_BARS`).
pub fn max_bars(cfg: &Config, width: u16) -> usize {
    let per = (cfg.bar_width + cfg.gap) as usize;
    let usable = width.saturating_sub(2) as usize + cfg.gap as usize;
    (usable / per.max(1)).clamp(4, MAX_BARS)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let now_playing = app.now_playing();
    let np_row = u16::from(now_playing.is_some());

    let viz_area = if app.cfg.hud {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(np_row),
            Constraint::Length(1),
        ])
        .split(frame.area());
        draw_header(frame, rows[0], app);
        if let Some(np) = &now_playing {
            draw_now_playing(frame, rows[2], app, np);
        }
        draw_footer(frame, rows[3], app);
        rows[1]
    } else {
        // Focus mode: the now-playing line is the only chrome that stays.
        let rows = Layout::vertical([Constraint::Min(4), Constraint::Length(np_row)])
            .split(frame.area());
        if let Some(np) = &now_playing {
            draw_now_playing(frame, rows[1], app, np);
        }
        rows[0]
    };

    app.viz()
        .render(viz_area, frame.buffer_mut(), app.cfg.theme, &app.cfg);

    // With the HUD hidden the status line has no home — float it over the
    // visualization so feedback stays visible.
    if !app.cfg.hud {
        let area = frame.area();
        if let Some(msg) = app.status().filter(|_| area.height > 0) {
            let style = Style::default()
                .fg(app.cfg.theme.peak_color())
                .add_modifier(Modifier::BOLD);
            let buf = frame.buffer_mut();
            for (i, ch) in msg.chars().enumerate() {
                let x = area.x + 1 + i as u16;
                if x >= area.x + area.width {
                    break;
                }
                buf[(x, area.y)].set_char(ch).set_style(style);
            }
        }
    }

    match &app.popup {
        Popup::Help => draw_help(frame, app),
        Popup::Devices { devices, selected } => draw_devices(frame, app, devices, *selected),
        Popup::Styles { selected } => draw_styles(frame, app, *selected),
        Popup::None => {}
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.cfg.theme;
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
        "{}{} · {} · {} · gain {:+.0} dB{} ",
        app.viz_name(),
        if app.cfg.auto_cycle { " ↻" } else { "" },
        theme.display_name(),
        SMOOTHING_PRESETS[app.cfg.smoothing].0,
        app.cfg.gain_db,
        if app.cfg.auto_gain { " · AGC" } else { "" },
    );
    let left_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad = (area.width as usize).saturating_sub(left_w + right.as_str().width());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, Style::default().fg(theme.dim())));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_now_playing(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    np: &crate::nowplaying::NowPlaying,
) {
    let theme = app.cfg.theme;
    let dim = Style::default().fg(theme.dim());
    let mut spans = vec![Span::styled(
        if np.playing { " ▶ " } else { " ⏸ " },
        Style::default().fg(theme.accent()),
    )];
    if !np.artist.is_empty() {
        spans.push(Span::raw(np.artist.clone()));
        spans.push(Span::styled(" — ", dim));
    }
    spans.push(Span::styled(
        np.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    if !np.album.is_empty() {
        spans.push(Span::styled(format!(" · {}", np.album), dim));
    }
    let right = format!("{} ", np.player);
    let left_w: usize = spans.iter().map(|s| s.content.width()).sum();
    let pad = (area.width as usize).saturating_sub(left_w + right.as_str().width());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, dim));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.cfg.theme;
    let key = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    let txt = Style::default().fg(theme.dim());
    let spans: Vec<Span> = KEYS
        .iter()
        .enumerate()
        .flat_map(|(i, k)| {
            let sep = if i == 0 { " " } else { "  " };
            [
                Span::styled(format!("{sep}{}", k.footer_key), key),
                Span::styled(format!(" {}", k.footer_label), txt),
            ]
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// One key binding as shown in the footer and the help popup. Keep in sync
/// with `App::on_key`.
struct KeyHelp {
    footer_key: &'static str,
    footer_label: &'static str,
    help_key: &'static str,
    help_text: &'static str,
}

const fn key_help(
    footer_key: &'static str,
    footer_label: &'static str,
    help_key: &'static str,
    help_text: &'static str,
) -> KeyHelp {
    KeyHelp { footer_key, footer_label, help_key, help_text }
}

const KEYS: [KeyHelp; 13] = [
    key_help("v/V", "style", "v / V", "next / previous visualization style"),
    key_help("m", "menu", "m", "visualization style menu"),
    key_help("c", "cycle", "c", "toggle auto-cycling through styles"),
    key_help("↑↓", "gain", "↑ / ↓", "input gain up / down"),
    key_help("+-", "bars", "+ / -", "more / fewer bars"),
    key_help("s", "smooth", "s", "cycle smoothing (fast/medium/smooth)"),
    key_help("p", "peaks", "p", "toggle peak-hold caps"),
    key_help("a", "agc", "a", "toggle auto gain"),
    key_help("t", "theme", "t", "cycle color theme"),
    key_help("d", "device", "d", "pick capture device"),
    key_help("f", "focus", "f", "hide/show header & footer"),
    key_help("?", "help", "? / h", "this help"),
    key_help("q", "quit", "q / Esc", "quit (settings are saved)"),
];

fn popup_rect(frame: &Frame, width: u16, height: u16) -> Rect {
    let a = frame.area();
    let w = width.min(a.width.saturating_sub(2));
    let h = height.min(a.height.saturating_sub(2));
    Rect::new(a.x + (a.width - w) / 2, a.y + (a.height - h) / 2, w, h)
}

fn draw_help(frame: &mut Frame, app: &App) {
    let theme = app.cfg.theme;
    let key = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|k| {
            Line::from(vec![
                Span::styled(format!("  {:<8}", k.help_key), key),
                Span::raw(format!(" {}", k.help_text)),
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

fn draw_styles(frame: &mut Frame, app: &App, selected: usize) {
    let theme = app.cfg.theme;
    let current = app.viz_index();
    let items: Vec<ListItem> = crate::viz::STYLES
        .iter()
        .enumerate()
        .map(|(i, style)| {
            let marker = if i == current { "● " } else { "  " };
            ListItem::new(format!(" {marker}{}", style.name))
        })
        .collect();
    let height = (items.len() as u16 + 2).min(frame.area().height.saturating_sub(2));
    let area = popup_rect(frame, 44, height);
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent()))
                    .title(" Style (Enter/Space to select, Esc to close) "),
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

fn draw_devices(
    frame: &mut Frame,
    app: &App,
    devices: &[crate::audio::AudioDevice],
    selected: usize,
) {
    let theme = app.cfg.theme;
    let mut items = vec![ListItem::new(format!("  {DEFAULT_DEVICE_LABEL} (monitor)"))];
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
                    .title(" Capture device (Enter/Space to select, Esc to close) "),
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
