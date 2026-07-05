use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::audio::{enumerate_devices, AudioDevice, Capture, CaptureTarget};
use crate::config::Config;
use crate::dsp::{Analyzer, SMOOTHING_PRESETS};
use crate::theme::ThemeId;
use crate::ui;

const FRAME: Duration = Duration::from_millis(16);
/// No fresh samples for this long → treat the stream as silent.
const STALL: Duration = Duration::from_millis(250);
const STATUS_TTL: Duration = Duration::from_secs(3);

pub enum Popup {
    None,
    Help,
    Devices {
        devices: Vec<AudioDevice>,
        selected: usize,
    },
}

pub struct App {
    pub cfg: Config,
    pub theme: ThemeId,
    pub analyzer: Analyzer,
    pub capture: Option<Capture>,
    pub device_label: String,
    pub popup: Popup,
    pub should_quit: bool,
    /// How many bars fit at the current terminal width (updated every frame).
    pub max_bars: usize,
    status: Option<(String, Instant)>,
    frame_buf: Vec<f32>,
    last_pushed: u64,
    last_data_at: Instant,
}

impl App {
    pub fn new(cfg: Config, initial: CaptureTarget, device_label: String) -> Result<Self> {
        let theme = ThemeId::from_key(&cfg.theme).unwrap_or(ThemeId::Terminal);
        let analyzer = Analyzer::new(
            cfg.fft_size,
            cfg.bars,
            cfg.freq_min,
            cfg.freq_max,
            cfg.gain_db,
            cfg.auto_gain,
            cfg.smoothing,
        );
        let mut app = Self {
            cfg,
            theme,
            analyzer,
            capture: None,
            device_label,
            popup: Popup::None,
            should_quit: false,
            max_bars: 96,
            status: None,
            frame_buf: Vec::new(),
            last_pushed: 0,
            last_data_at: Instant::now(),
        };
        app.frame_buf = vec![0.0; app.cfg.fft_size];

        match Capture::start(initial.clone()) {
            Ok(c) => app.capture = Some(c),
            Err(e) if initial.node_name.is_some() => {
                // Configured device unavailable — fall back to the default output.
                app.set_status(format!("device unavailable ({e}), using default output"));
                app.capture = Some(Capture::start(CaptureTarget::default())?);
                app.device_label = "Default output".into();
                app.cfg.device = None;
                app.cfg.device_is_sink = true;
            }
            Err(e) => return Err(e),
        }
        Ok(app)
    }

    pub fn status(&self) -> Option<&str> {
        match &self.status {
            Some((msg, at)) if at.elapsed() < STATUS_TTL => Some(msg),
            _ => None,
        }
    }

    fn set_status(&mut self, msg: String) {
        self.status = Some((msg, Instant::now()));
    }

    /// Pull the latest window of samples and advance the analyzer.
    fn tick(&mut self, dt: f32) {
        let fft_size = self.analyzer.fft_size();
        let (rate, silent) = match &self.capture {
            Some(cap) => {
                let pushed = cap.pushed.load(Ordering::Relaxed);
                if pushed != self.last_pushed {
                    self.last_pushed = pushed;
                    self.last_data_at = Instant::now();
                }
                let queue = cap.samples.lock().unwrap();
                let n = queue.len().min(fft_size);
                let skip = queue.len() - n;
                let pad = fft_size - n;
                self.frame_buf[..pad].fill(0.0);
                for (dst, &s) in self.frame_buf[pad..].iter_mut().zip(queue.iter().skip(skip)) {
                    *dst = s;
                }
                drop(queue);
                (
                    cap.sample_rate.load(Ordering::Relaxed),
                    self.last_data_at.elapsed() > STALL,
                )
            }
            None => (0, true),
        };
        self.analyzer.update(&self.frame_buf, rate, dt, silent);
    }

    fn switch_device(&mut self, target: CaptureTarget, label: String) {
        self.capture = None; // drop stops the old stream first
        match Capture::start(target.clone()) {
            Ok(c) => {
                self.capture = Some(c);
                self.last_pushed = 0;
                self.cfg.device = target.node_name;
                self.cfg.device_is_sink = target.is_sink;
                self.set_status(format!("capturing: {label}"));
                self.device_label = label;
            }
            Err(e) => {
                self.set_status(format!("failed to open device: {e}"));
                if let Ok(c) = Capture::start(CaptureTarget::default()) {
                    self.capture = Some(c);
                    self.device_label = "Default output".into();
                    self.cfg.device = None;
                    self.cfg.device_is_sink = true;
                }
            }
        }
    }

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if let Popup::Devices { devices, selected } = &mut self.popup {
            match code {
                KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('q') => {
                    self.popup = Popup::None
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(devices.len()); // +1 entry for "default"
                }
                KeyCode::Enter => {
                    let (target, label) = if *selected == 0 {
                        (CaptureTarget::default(), "Default output".to_string())
                    } else {
                        let d = &devices[*selected - 1];
                        (
                            CaptureTarget {
                                node_name: Some(d.name.clone()),
                                is_sink: d.is_sink,
                            },
                            d.description.clone(),
                        )
                    };
                    self.popup = Popup::None;
                    self.switch_device(target, label);
                }
                _ => {}
            }
            return;
        }
        if matches!(self.popup, Popup::Help) {
            self.popup = Popup::None;
            return;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Up => {
                self.cfg.gain_db = (self.cfg.gain_db + 2.0).min(30.0);
                self.analyzer.gain_db = self.cfg.gain_db;
                self.set_status(format!("gain {:+.0} dB", self.cfg.gain_db));
            }
            KeyCode::Down => {
                self.cfg.gain_db = (self.cfg.gain_db - 2.0).max(-30.0);
                self.analyzer.gain_db = self.cfg.gain_db;
                self.set_status(format!("gain {:+.0} dB", self.cfg.gain_db));
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                // Work from the displayed count, not a stale config value that
                // may exceed what the terminal fits.
                let shown = self.cfg.bars.min(self.max_bars);
                if shown >= self.max_bars {
                    self.cfg.bars = self.max_bars;
                    self.set_status(format!("{} bars — terminal width limit", self.max_bars));
                } else {
                    self.cfg.bars = (shown + 2).min(self.max_bars);
                    self.set_status(format!("{} bars", self.cfg.bars));
                }
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                let shown = self.cfg.bars.min(self.max_bars);
                self.cfg.bars = shown.saturating_sub(2).max(8);
                self.set_status(format!("{} bars", self.cfg.bars));
            }
            KeyCode::Char('s') => {
                self.cfg.smoothing = (self.cfg.smoothing + 1) % SMOOTHING_PRESETS.len();
                self.analyzer.smoothing = self.cfg.smoothing;
                self.set_status(format!(
                    "smoothing: {}",
                    SMOOTHING_PRESETS[self.cfg.smoothing].0
                ));
            }
            KeyCode::Char('p') => {
                self.cfg.peaks = !self.cfg.peaks;
                self.set_status(format!(
                    "peak hold {}",
                    if self.cfg.peaks { "on" } else { "off" }
                ));
            }
            KeyCode::Char('a') => {
                self.cfg.auto_gain = !self.cfg.auto_gain;
                self.analyzer.auto_gain = self.cfg.auto_gain;
                self.set_status(format!(
                    "auto gain {}",
                    if self.cfg.auto_gain { "on" } else { "off" }
                ));
            }
            KeyCode::Char('t') => {
                self.theme = self.theme.next();
                self.cfg.theme = self.theme.key().to_string();
                self.set_status(format!("theme: {}", self.theme.display_name()));
            }
            KeyCode::Char('d') => match enumerate_devices() {
                Ok(devices) => {
                    self.popup = Popup::Devices {
                        devices,
                        selected: 0,
                    }
                }
                Err(e) => self.set_status(format!("device list failed: {e}")),
            },
            KeyCode::Char('h') | KeyCode::Char('?') => self.popup = Popup::Help,
            _ => {}
        }
    }
}

pub fn run(
    terminal: &mut DefaultTerminal,
    cfg: Config,
    initial: CaptureTarget,
    device_label: String,
) -> Result<Config> {
    let mut app = App::new(cfg, initial, device_label)?;
    let mut last = Instant::now();

    while !app.should_quit {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let width = terminal.size()?.width;
        app.max_bars = ui::max_bars(&app.cfg, width);
        app.analyzer.set_bars(app.cfg.bars.clamp(4, app.max_bars));
        app.tick(dt);
        terminal.draw(|f| ui::draw(f, &app))?;

        let deadline = now + FRAME;
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            if !event::poll(timeout)? {
                break;
            }
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    app.on_key(k.code, k.modifiers)
                }
                _ => {}
            }
        }
    }
    Ok(app.cfg)
}
