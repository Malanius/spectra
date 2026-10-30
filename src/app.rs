use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::audio::{
    enumerate_devices, AudioDevice, Capture, CaptureTarget, LinkedSource, DEFAULT_DEVICE_LABEL,
};
use crate::config::{Config, MAX_BARS, MIN_BARS};
use crate::dsp::{Analyzer, SMOOTHING_PRESETS};
use crate::nowplaying::{self, NowPlaying};
use crate::ui;
use crate::viz::{self, AudioFeatures, AudioFrame, Visualizer};

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
    Styles {
        selected: usize,
    },
}

pub struct App {
    pub cfg: Config,
    pub capture: Option<Capture>,
    /// What the header shows: the node actually being captured.
    pub device_label: String,
    /// Label of the device that was asked for. Differs from `device_label`
    /// while WirePlumber has rerouted the stream, e.g. after an unplug.
    requested_label: String,
    /// Last linked source reported by the capture thread.
    linked: Option<LinkedSource>,
    pub popup: Popup,
    pub should_quit: bool,
    /// How many bars fit at the current terminal width (updated every frame).
    pub max_bars: usize,
    visualizers: Vec<Box<dyn Visualizer>>,
    viz_idx: usize,
    now_playing: nowplaying::Shared,
    status: Option<(String, Instant)>,
    mono_buf: Vec<f32>,
    left_buf: Vec<f32>,
    right_buf: Vec<f32>,
    /// Sliding sample history, drained from the capture ring each frame and
    /// trimmed to the FFT window length.
    history_left: VecDeque<f32>,
    history_right: VecDeque<f32>,
    /// Shared per-frame feature extraction (see `AudioFeatures`).
    feature_analyzer: Analyzer,
    last_pushed: u64,
    last_data_at: Instant,
}

/// Bands in the shared feature analyzer.
const FEATURE_BANDS: usize = 24;

impl App {
    pub fn new(cfg: Config, initial: CaptureTarget, device_label: String) -> Result<Self> {
        let visualizers = viz::create_all(&cfg);
        let viz_idx = viz::STYLES
            .iter()
            .position(|s| s.id == cfg.visualizer)
            .unwrap_or(0);
        let fft_size = cfg.fft_size;
        // Fast attack: features feed beat detection.
        let feature_analyzer = Analyzer::new(&cfg, FEATURE_BANDS).with_fixed_smoothing(0);
        let mut app = Self {
            cfg,
            capture: None,
            requested_label: device_label.clone(),
            device_label,
            linked: None,
            popup: Popup::None,
            should_quit: false,
            max_bars: MAX_BARS,
            visualizers,
            viz_idx,
            now_playing: nowplaying::spawn_poller(),
            status: None,
            mono_buf: vec![0.0; fft_size],
            left_buf: vec![0.0; fft_size],
            right_buf: vec![0.0; fft_size],
            history_left: VecDeque::with_capacity(fft_size + 1),
            history_right: VecDeque::with_capacity(fft_size + 1),
            feature_analyzer,
            last_pushed: 0,
            last_data_at: Instant::now(),
        };

        match Capture::start(initial.clone()) {
            Ok(c) => app.capture = Some(c),
            Err(e) if initial.node_name.is_some() => {
                // Configured device unavailable — fall back to the default output.
                app.set_status(format!("device unavailable ({e}), using default output"));
                app.use_default_device()?;
            }
            Err(e) => return Err(e),
        }
        Ok(app)
    }

    pub fn viz(&self) -> &dyn Visualizer {
        self.visualizers[self.viz_idx].as_ref()
    }

    pub fn viz_index(&self) -> usize {
        self.viz_idx
    }

    pub fn viz_name(&self) -> &'static str {
        viz::STYLES[self.viz_idx].name
    }

    pub fn now_playing(&self) -> Option<NowPlaying> {
        self.now_playing.lock().unwrap().clone()
    }

    fn select_style(&mut self, idx: usize) {
        self.viz_idx = idx % self.visualizers.len();
        self.cfg.visualizer = viz::STYLES[self.viz_idx].id.to_string();
        self.set_status(format!("style: {}", self.viz_name()));
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

    /// Follow WirePlumber's routing: show the node the stream is really
    /// linked from, and say so when it isn't the requested device.
    fn check_linked_source(&mut self) {
        let Some(cap) = &self.capture else { return };
        let now = cap.source.lock().unwrap().clone();
        if now == self.linked {
            return;
        }
        let was_rerouted = self.is_rerouted();
        self.linked = now;
        // Only a requested device can be rerouted. The default output follows
        // whatever WirePlumber picks and keeps its generic label. While
        // unlinked, keep the label; the visualizer already shows silence.
        let (Some(want), Some(src)) = (&self.cfg.device, &self.linked) else {
            return;
        };
        if src.name == *want {
            self.device_label = self.requested_label.clone();
            if was_rerouted {
                self.set_status(format!("{} is back", self.requested_label));
            }
        } else {
            self.device_label = src.description.clone();
            self.set_status(format!(
                "{} unavailable, capturing {}",
                self.requested_label, src.description
            ));
        }
    }

    fn is_rerouted(&self) -> bool {
        matches!((&self.cfg.device, &self.linked), (Some(want), Some(src)) if src.name != *want)
    }

    /// Pull the latest window of samples and advance the active visualizer.
    fn tick(&mut self, dt: f32) {
        self.check_linked_source();
        let fft_size = self.cfg.fft_size;
        let (rate, silent) = match self.capture.as_mut() {
            Some(cap) => {
                let hl = &mut self.history_left;
                let hr = &mut self.history_right;
                cap.drain(|l, r| {
                    hl.push_back(l);
                    hr.push_back(r);
                });
                while hl.len() > fft_size {
                    hl.pop_front();
                }
                while hr.len() > fft_size {
                    hr.pop_front();
                }
                let pushed = cap.pushed.load(Ordering::Relaxed);
                if pushed != self.last_pushed {
                    self.last_pushed = pushed;
                    self.last_data_at = Instant::now();
                }
                (
                    cap.sample_rate.load(Ordering::Relaxed),
                    self.last_data_at.elapsed() > STALL,
                )
            }
            None => (0, true),
        };

        let n = self.history_left.len().min(fft_size);
        let pad = fft_size - n;
        self.mono_buf[..pad].fill(0.0);
        self.left_buf[..pad].fill(0.0);
        self.right_buf[..pad].fill(0.0);
        for (i, (&l, &r)) in self
            .history_left
            .iter()
            .zip(self.history_right.iter())
            .take(n)
            .enumerate()
        {
            self.left_buf[pad + i] = l;
            self.right_buf[pad + i] = r;
            self.mono_buf[pad + i] = (l + r) * 0.5;
        }
        // Shared per-frame features, computed once for all styles.
        self.feature_analyzer.sync(&self.cfg);
        self.feature_analyzer.update(&self.mono_buf, rate, dt, silent);
        let fb = self.feature_analyzer.bars();
        let bands = fb.len();
        let avg = |s: &[f32]| s.iter().sum::<f32>() / s.len().max(1) as f32;
        let bass = avg(&fb[..bands / 4]);
        let features = AudioFeatures {
            bass,
            mid: avg(&fb[bands / 4..bands / 2]),
            treble: avg(&fb[bands / 2..]),
            level: avg(fb),
            rms_db: if silent {
                -120.0
            } else {
                viz::rms_db(&self.mono_buf)
            },
            peak: self.mono_buf.iter().fold(0.0f32, |a, &s| a.max(s.abs())),
        };

        let frame = AudioFrame {
            mono: &self.mono_buf,
            left: &self.left_buf,
            right: &self.right_buf,
            rate,
            silent,
            features,
        };
        self.visualizers[self.viz_idx].update(&frame, &self.cfg, self.max_bars, dt);
    }

    fn switch_device(&mut self, target: CaptureTarget, label: String) {
        self.capture = None; // drop stops the old stream first
        match Capture::start(target.clone()) {
            Ok(c) => {
                self.capture = Some(c);
                self.last_pushed = 0;
                self.history_left.clear();
                self.history_right.clear();
                self.linked = None;
                self.cfg.device = target.node_name;
                self.cfg.device_is_sink = target.is_sink;
                self.set_status(format!("capturing: {label}"));
                self.requested_label = label.clone();
                self.device_label = label;
            }
            Err(e) => {
                self.set_status(format!("failed to open device: {e}"));
                let _ = self.use_default_device();
            }
        }
    }

    /// Capture the default output's monitor and forget the configured device.
    /// Leaves everything unchanged if even that fails.
    fn use_default_device(&mut self) -> Result<()> {
        self.capture = Some(Capture::start(CaptureTarget::default())?);
        self.linked = None;
        self.device_label = DEFAULT_DEVICE_LABEL.into();
        self.requested_label = DEFAULT_DEVICE_LABEL.into();
        self.cfg.device = None;
        self.cfg.device_is_sink = true;
        Ok(())
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
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let (target, label) = if *selected == 0 {
                        (CaptureTarget::default(), DEFAULT_DEVICE_LABEL.to_string())
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
        if let Popup::Styles { selected } = &mut self.popup {
            let n = self.visualizers.len();
            match code {
                KeyCode::Esc | KeyCode::Char('m') | KeyCode::Char('q') => {
                    self.popup = Popup::None
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *selected = (*selected + n - 1) % n;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1) % n;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let idx = *selected;
                    self.popup = Popup::None;
                    self.select_style(idx);
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
            KeyCode::Char('v') | KeyCode::Char('V') => {
                let n = self.visualizers.len();
                let idx = if code == KeyCode::Char('v') {
                    (self.viz_idx + 1) % n
                } else {
                    (self.viz_idx + n - 1) % n
                };
                self.select_style(idx);
            }
            KeyCode::Char('m') => {
                self.popup = Popup::Styles {
                    selected: self.viz_idx,
                }
            }
            KeyCode::Up => {
                self.cfg.gain_db = (self.cfg.gain_db + 2.0).min(30.0);
                self.set_status(format!("gain {:+.0} dB", self.cfg.gain_db));
            }
            KeyCode::Down => {
                self.cfg.gain_db = (self.cfg.gain_db - 2.0).max(-30.0);
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
                self.cfg.bars = shown.saturating_sub(2).max(MIN_BARS);
                self.set_status(format!("{} bars", self.cfg.bars));
            }
            KeyCode::Char('s') => {
                self.cfg.smoothing = (self.cfg.smoothing + 1) % SMOOTHING_PRESETS.len();
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
                self.set_status(format!(
                    "auto gain {}",
                    if self.cfg.auto_gain { "on" } else { "off" }
                ));
            }
            KeyCode::Char('t') => {
                self.cfg.theme = self.cfg.theme.next();
                self.set_status(format!("theme: {}", self.cfg.theme.display_name()));
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
            KeyCode::Char('f') => {
                self.cfg.hud = !self.cfg.hud;
                self.set_status(if self.cfg.hud {
                    "header & footer shown".into()
                } else {
                    "header & footer hidden — f restores".into()
                });
            }
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
    notice: Option<String>,
) -> Result<Config> {
    let mut app = App::new(cfg, initial, device_label)?;
    if let Some(msg) = notice {
        app.set_status(msg);
    }
    let mut last = Instant::now();

    while !app.should_quit {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        let width = terminal.size()?.width;
        app.max_bars = ui::max_bars(&app.cfg, width);
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
