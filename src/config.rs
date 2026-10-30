use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::dsp::BandAgg;
use crate::theme::ThemeId;
use crate::viz;

/// A config value stored as a string key. Parsing is lenient: keys are
/// case-insensitive, aliases are accepted, and an unknown key falls back to
/// the default instead of failing the whole file.
pub trait ConfigKey: Copy + Default {
    fn from_key(s: &str) -> Option<Self>;
    /// Canonical key written back to the config file.
    fn key(self) -> &'static str;
}

/// `#[serde(with = "key_serde")]` for [`ConfigKey`] fields.
mod key_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::ConfigKey;

    pub fn serialize<S: Serializer, T: ConfigKey>(v: &T, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v.key())
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: ConfigKey>(d: D) -> Result<T, D::Error> {
        let s = String::deserialize(d)?;
        Ok(T::from_key(&s).unwrap_or_default())
    }
}

/// Allowed range for the configured bar count.
pub const MIN_BARS: usize = 8;
pub const MAX_BARS: usize = 96;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(with = "key_serde")]
    pub theme: ThemeId,
    /// Visualization style id, see `viz::STYLES`.
    pub visualizer: String,
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
    /// Show the header and footer bars (toggled with 'f').
    pub hud: bool,
    /// PipeWire node.name to capture from; None = default output monitor.
    pub device: Option<String>,
    /// Whether the chosen device is a sink (capture its monitor) or a source.
    pub device_is_sink: bool,
    /// FFT window size (power of two).
    pub fft_size: usize,
    /// Analyzed frequency range in Hz.
    pub freq_min: f32,
    pub freq_max: f32,
    /// Band aggregation for bars/mirror: "energy", "max", "avg", "hybrid".
    #[serde(with = "key_serde")]
    pub band_agg: BandAgg,
    pub mirror: MirrorCfg,
    pub scope: ScopeCfg,
    pub fire: FireCfg,
    pub particles: ParticlesCfg,
    pub radial: RadialCfg,
    pub spectrogram: SpectrogramCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct MirrorCfg {
    #[serde(with = "key_serde")]
    pub mode: MirrorMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MirrorMode {
    /// Left channel up, right channel down.
    #[default]
    Lr,
    /// Mid (L+R) up, side (L−R) down: a stereo width view.
    Ms,
}

impl ConfigKey for MirrorMode {
    fn from_key(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "lr" => Some(Self::Lr),
            "ms" => Some(Self::Ms),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Lr => "lr",
            Self::Ms => "ms",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScopeCfg {
    /// Time window shown on screen, in milliseconds.
    pub window_ms: f32,
}

impl Default for ScopeCfg {
    fn default() -> Self {
        Self { window_ms: 35.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FireCfg {
    /// Cooling multiplier: >1 = shorter flames, <1 = taller.
    pub cooling: f32,
}

impl Default for FireCfg {
    fn default() -> Self {
        Self { cooling: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticlesCfg {
    /// Spawn-rate multiplier for bursts and drizzle.
    pub density: f32,
}

impl Default for ParticlesCfg {
    fn default() -> Self {
        Self { density: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RadialCfg {
    /// Spokes around the full circle (rounded down to even).
    pub spokes: usize,
}

impl Default for RadialCfg {
    fn default() -> Self {
        Self { spokes: 96 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrogramCfg {
    /// Milliseconds per stored slice (two slices per terminal row) — the
    /// scroll speed.
    pub slice_ms: f32,
    /// Display threshold: values below this are not drawn.
    pub floor: f32,
    /// 0..0.9: how much older rows fade toward black (RGB themes only).
    pub fade: f32,
}

impl Default for SpectrogramCfg {
    fn default() -> Self {
        Self {
            slice_ms: 33.0,
            floor: 0.03,
            fade: 0.0,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeId::default(),
            visualizer: "bars".into(),
            bars: 32,
            bar_width: 2,
            gap: 1,
            gain_db: 0.0,
            auto_gain: true,
            smoothing: 1,
            peaks: true,
            hud: true,
            device: None,
            device_is_sink: true,
            fft_size: 2048,
            freq_min: 30.0,
            freq_max: 16000.0,
            band_agg: BandAgg::default(),
            mirror: MirrorCfg::default(),
            scope: ScopeCfg::default(),
            fire: FireCfg::default(),
            particles: ParticlesCfg::default(),
            radial: RadialCfg::default(),
            spectrogram: SpectrogramCfg::default(),
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
        if !viz::STYLES.iter().any(|s| s.id == self.visualizer) {
            self.visualizer = "bars".into();
        }
        self.bars = self.bars.clamp(MIN_BARS, MAX_BARS);
        self.bar_width = self.bar_width.clamp(1, 8);
        self.gap = self.gap.min(4);
        self.gain_db = self.gain_db.clamp(-30.0, 30.0);
        self.smoothing = self.smoothing.min(2);
        if !self.fft_size.is_power_of_two() || !(256..=16384).contains(&self.fft_size) {
            self.fft_size = 2048;
        }
        self.freq_min = self.freq_min.clamp(10.0, 500.0);
        self.freq_max = self.freq_max.clamp(self.freq_min * 4.0, 22000.0);
        self.scope.window_ms = self.scope.window_ms.clamp(5.0, 200.0);
        self.fire.cooling = self.fire.cooling.clamp(0.25, 4.0);
        self.particles.density = self.particles.density.clamp(0.1, 4.0);
        self.radial.spokes = self.radial.spokes.clamp(24, 256) & !1;
        self.spectrogram.slice_ms = self.spectrogram.slice_ms.clamp(10.0, 200.0);
        self.spectrogram.floor = self.spectrogram.floor.clamp(0.005, 0.3);
        self.spectrogram.fade = self.spectrogram.fade.clamp(0.0, 0.9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_leniently() {
        let cfg: Config = toml::from_str(
            "theme = \"Mocha\"\nband_agg = \"PEAK\"\n[mirror]\nmode = \"MS\"\n",
        )
        .unwrap();
        assert_eq!(cfg.theme, ThemeId::CatppuccinMocha);
        assert_eq!(cfg.band_agg, BandAgg::Max);
        assert_eq!(cfg.mirror.mode, MirrorMode::Ms);
    }

    #[test]
    fn unknown_key_resets_only_that_field() {
        let cfg: Config = toml::from_str("theme = \"nope\"\nbars = 40\n").unwrap();
        assert_eq!(cfg.theme, ThemeId::Terminal);
        assert_eq!(cfg.bars, 40);
    }

    #[test]
    fn keys_round_trip() {
        let cfg = Config {
            theme: ThemeId::TokyoNight,
            band_agg: BandAgg::Hybrid,
            mirror: MirrorCfg {
                mode: MirrorMode::Ms,
            },
            ..Config::default()
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("theme = \"tokyo-night\""));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.theme, cfg.theme);
        assert_eq!(back.band_agg, cfg.band_agg);
        assert_eq!(back.mirror.mode, cfg.mirror.mode);
    }

    #[test]
    fn sanitize_resets_unknown_style() {
        let mut cfg = Config {
            visualizer: "nope".into(),
            ..Config::default()
        };
        cfg.sanitize();
        assert_eq!(cfg.visualizer, "bars");
    }
}
