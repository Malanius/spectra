use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme key, see `ThemeId::key()`.
    pub theme: String,
    /// Desired number of frequency bars (clamped to what fits on screen).
    pub bars: usize,
    /// Width of each bar in terminal cells.
    pub bar_width: u16,
    /// Gap between bars in cells.
    pub gap: u16,
    /// Manual input gain in dB.
    pub gain_db: f32,
    /// Automatic level normalization.
    pub auto_gain: bool,
    /// Smoothing preset index: 0 = fast, 1 = medium, 2 = smooth.
    pub smoothing: usize,
    /// Show falling peak-hold caps.
    pub peaks: bool,
    /// PipeWire node.name to capture from; None = default output monitor.
    pub device: Option<String>,
    /// Whether the chosen device is a sink (capture its monitor) or a source.
    pub device_is_sink: bool,
    /// FFT window size (power of two).
    pub fft_size: usize,
    /// Analyzed frequency range in Hz.
    pub freq_min: f32,
    pub freq_max: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "terminal".into(),
            bars: 32,
            bar_width: 2,
            gap: 1,
            gain_db: 0.0,
            auto_gain: true,
            smoothing: 1,
            peaks: true,
            device: None,
            device_is_sink: true,
            fft_size: 2048,
            freq_min: 30.0,
            freq_max: 16000.0,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("spectra").join("config.toml"))
    }

    /// Load the config, falling back to defaults if missing or invalid.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path().context("could not determine config directory")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Clamp values to sane ranges (protects against hand-edited configs).
    pub fn sanitize(&mut self) {
        self.bars = self.bars.clamp(8, 96);
        self.bar_width = self.bar_width.clamp(1, 8);
        self.gap = self.gap.min(4);
        self.gain_db = self.gain_db.clamp(-30.0, 30.0);
        self.smoothing = self.smoothing.min(2);
        if !self.fft_size.is_power_of_two() || !(256..=16384).contains(&self.fft_size) {
            self.fft_size = 2048;
        }
        self.freq_min = self.freq_min.clamp(10.0, 500.0);
        self.freq_max = self.freq_max.clamp(self.freq_min * 4.0, 22000.0);
    }
}
