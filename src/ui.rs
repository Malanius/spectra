use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Popup};
use crate::config::Config;
use crate::dsp::SMOOTHING_PRESETS;

/// How many bars can fit on a terminal of the given width (hard cap 96).
pub fn max_bars(cfg: &Config, width: u16) -> usize {
    let per = (cfg.bar_width + cfg.gap) as usize;
    let usable = width.saturating_sub(2) as usize + cfg.gap as usize;
    (usable / per.max(1)).clamp(4, 96)
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
        .render(viz_area, frame.buffer_mut(), app.theme, &app.cfg);

    // With the HUD hidden the status line has no home — float it over the
    // visualization so feedback stays visible.
    if !app.cfg.hud {
        if let Some(msg) = app.status() {
            let area = frame.area();
            let style = Style::default()
                .fg(app.theme.peak_color())
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
        "{} · {} · {} · gain {:+.0} dB{} ",
        app.viz_name(),
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
    let theme = app.theme;
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
    let theme = app.theme;
    let key = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    let txt = Style::default().fg(theme.dim());
    let line = Line::from(vec![
        Span::styled(" v/V", key),
        Span::styled(" style  ", txt),
        Span::styled("m", key),
        Span::styled(" menu  ", txt),
        Span::styled("↑↓", key),
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
        Span::styled("f", key),
        Span::styled(" focus  ", txt),
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
    Rect::new(a.x + (a.width - w) / 2, a.y + (a.height - h) / 2, w, h)
}

fn draw_help(frame: &mut Frame, app: &App) {
    let theme = app.theme;
    let key = Style::default()
        .fg(theme.accent())
        .add_modifier(Modifier::BOLD);
    let rows: [(&str, &str); 12] = [
        ("v / V", "next / previous visualization style"),
        ("m", "visualization style menu"),
        ("f", "hide/show header & footer"),
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

fn draw_styles(frame: &mut Frame, app: &App, selected: usize) {
    let theme = app.theme;
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
