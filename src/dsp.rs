use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::config::{Config, ConfigKey};

/// (name, attack, decay) — lerp factors per frame at ~60 fps.
pub const SMOOTHING_PRESETS: [(&str, f32, f32); 3] = [
    ("fast", 0.95, 0.45),
    ("medium", 0.72, 0.22),
    ("smooth", 0.45, 0.10),
];

/// How the FFT bins inside one display band are folded into a single value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BandAgg {
    /// Total band energy (default) — flat response for pink-ish material.
    #[default]
    Energy,
    /// Loudest bin — punchy, but narrow spikes dominate.
    Max,
    /// Mean bin energy — smooth, recessed treble.
    Avg,
    /// Peak-weighted average — between Max and Avg.
    Hybrid,
}

impl ConfigKey for BandAgg {
    fn from_key(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "energy" | "sum" => Some(Self::Energy),
            "max" | "peak" => Some(Self::Max),
            "avg" | "mean" => Some(Self::Avg),
            "hybrid" | "peak-avg" => Some(Self::Hybrid),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Energy => "energy",
            Self::Max => "max",
            Self::Avg => "avg",
            Self::Hybrid => "hybrid",
        }
    }
}

/// Displayed dynamic range in dB (bar bottom to bar top).
const RANGE_DB: f32 = 64.0;
/// Seconds a peak cap holds before it starts falling.
const PEAK_HOLD_S: f32 = 0.7;
/// Peak cap fall acceleration, in bar-heights per second squared.
const PEAK_GRAVITY: f32 = 3.5;
/// Anything below this raw band level is treated as silence by the AGC.
const SILENCE_DB: f32 = -75.0;

/// Auto-gain ceiling: follows the recent loudest level, rising fast and
/// releasing slowly, so any source fills the display. Shared by `Analyzer`
/// and the level-based styles.
pub struct Agc {
    ceil_db: f32,
    /// Levels at or below this are ignored, so pauses don't pump the gain.
    silence_db: f32,
}

impl Agc {
    pub fn new(silence_db: f32) -> Self {
        Self {
            ceil_db: -20.0,
            silence_db,
        }
    }

    /// Adapt the ceiling toward `db`. Does nothing with auto gain off.
    pub fn follow(&mut self, db: f32, auto: bool, dt: f32) {
        if !auto || db <= self.silence_db {
            return;
        }
        let k = if db > self.ceil_db {
            (dt * 8.0).min(1.0) // rise fast
        } else {
            (dt * 0.15).min(1.0) // release slowly
        };
        self.ceil_db += (db - self.ceil_db) * k;
        self.ceil_db = self.ceil_db.clamp(-50.0, 0.0);
    }

    /// Current adaptive ceiling, for render probes.
    #[cfg(feature = "render")]
    pub fn ceil_db(&self) -> f32 {
        self.ceil_db
    }

    /// Map `db` to 0..1 over the `range_db` below the ceiling. Without auto
    /// gain the ceiling is a fixed 0 dBFS; manual gain shifts it either way.
    pub fn norm(&self, db: f32, auto: bool, gain_db: f32, range_db: f32) -> f32 {
        let ceil = if auto { self.ceil_db + 2.0 } else { 0.0 } - gain_db;
        ((db - (ceil - range_db)) / range_db).clamp(0.0, 1.0)
    }
}

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    fft_size: usize,
    window: Vec<f32>,
    /// Scale factor turning windowed FFT power into ~dBFS.
    power_scale: f32,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    /// Fractional FFT-bin range per bar, log-spaced in frequency.
    bands: Vec<(f32, f32)>,
    raw_db: Vec<f32>,
    bars: Vec<f32>,
    peaks: Vec<f32>,
    peak_vel: Vec<f32>,
    peak_hold: Vec<f32>,
    rate: f32,
    n_bars: usize,
    freq_min: f32,
    freq_max: f32,
    gain_db: f32,
    auto_gain: bool,
    smoothing: usize,
    /// Smoothing set by `with_fixed_smoothing`; `sync` leaves it alone.
    fixed_smoothing: bool,
    pub agg: BandAgg,
    agc: Agc,
}

impl Analyzer {
    /// Analyzer with `n_bars` bands, taking FFT size, frequency range, gain
    /// and smoothing from `cfg`.
    pub fn new(cfg: &Config, n_bars: usize) -> Self {
        let fft_size = cfg.fft_size;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                let x = i as f32 / (fft_size - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * x).cos()
            })
            .collect();
        let window_sum: f32 = window.iter().sum();
        let input = fft.make_input_vec();
        let spectrum = fft.make_output_vec();
        let mut a = Self {
            fft,
            fft_size,
            window,
            power_scale: (2.0 / window_sum) * (2.0 / window_sum),
            input,
            spectrum,
            bands: Vec::new(),
            raw_db: Vec::new(),
            bars: Vec::new(),
            peaks: Vec::new(),
            peak_vel: Vec::new(),
            peak_hold: Vec::new(),
            rate: 0.0,
            n_bars: 0,
            freq_min: cfg.freq_min,
            freq_max: cfg.freq_max,
            gain_db: cfg.gain_db,
            auto_gain: cfg.auto_gain,
            smoothing: cfg.smoothing.min(SMOOTHING_PRESETS.len() - 1),
            fixed_smoothing: false,
            agg: BandAgg::Energy,
            agc: Agc::new(SILENCE_DB),
        };
        a.set_bars(n_bars);
        a
    }

    /// Pin the smoothing preset so it no longer follows the config.
    pub fn with_fixed_smoothing(mut self, smoothing: usize) -> Self {
        self.smoothing = smoothing.min(SMOOTHING_PRESETS.len() - 1);
        self.fixed_smoothing = true;
        self
    }

    /// Pick up the live gain and smoothing settings (changed by key presses).
    pub fn sync(&mut self, cfg: &Config) {
        self.gain_db = cfg.gain_db;
        self.auto_gain = cfg.auto_gain;
        if !self.fixed_smoothing {
            self.smoothing = cfg.smoothing.min(SMOOTHING_PRESETS.len() - 1);
        }
    }

    pub fn bars(&self) -> &[f32] {
        &self.bars
    }

    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    /// Center frequency of bar `i`, in Hz.
    pub fn band_freq(&self, i: usize) -> f32 {
        let n = self.n_bars.max(1) as f32;
        let fmax = self.effective_fmax();
        self.freq_min * (fmax / self.freq_min).powf((i as f32 + 0.5) / n)
    }

    fn effective_fmax(&self) -> f32 {
        if self.rate > 0.0 {
            self.freq_max.min(self.rate * 0.45)
        } else {
            self.freq_max
        }
    }

    pub fn set_bars(&mut self, n: usize) {
        if n == self.n_bars {
            return;
        }
        self.n_bars = n;
        self.raw_db.resize(n, -120.0);
        self.bars.resize(n, 0.0);
        self.peaks.resize(n, 0.0);
        self.peak_vel.resize(n, 0.0);
        self.peak_hold.resize(n, 0.0);
        self.compute_bands();
    }

    fn compute_bands(&mut self) {
        self.bands.clear();
        if self.rate <= 0.0 || self.n_bars == 0 {
            return;
        }
        let fmax = self.effective_fmax();
        let n = self.n_bars as f32;
        let bin_of = |f: f32| f * self.fft_size as f32 / self.rate;
        for i in 0..self.n_bars {
            let f_lo = self.freq_min * (fmax / self.freq_min).powf(i as f32 / n);
            let f_hi = self.freq_min * (fmax / self.freq_min).powf((i as f32 + 1.0) / n);
            self.bands.push((bin_of(f_lo), bin_of(f_hi)));
        }
    }

    /// Feed one analysis window (`fft_size` mono samples) and advance the
    /// animation by `dt` seconds. Pass `silent = true` to animate toward zero
    /// when no fresh audio is arriving (stream paused / sink suspended).
    pub fn update(&mut self, samples: &[f32], rate: u32, dt: f32, silent: bool) {
        if rate as f32 != self.rate && rate > 0 {
            self.rate = rate as f32;
            self.compute_bands();
        }

        if silent || self.rate <= 0.0 || self.bands.is_empty() {
            for v in self.raw_db.iter_mut() {
                *v = -120.0;
            }
        } else {
            self.analyze(samples);
        }

        // The AGC ceiling follows the recent loudest band.
        let frame_max = self
            .raw_db
            .iter()
            .fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        self.agc.follow(frame_max, self.auto_gain, dt);

        let (_, attack, decay) = SMOOTHING_PRESETS[self.smoothing];
        // Presets are tuned for 60 fps; scale lerp factors to the actual dt.
        let scale = (dt * 60.0).clamp(0.25, 4.0);
        let attack = 1.0 - (1.0 - attack).powf(scale);
        let decay = 1.0 - (1.0 - decay).powf(scale);

        for i in 0..self.n_bars {
            let target = self
                .agc
                .norm(self.raw_db[i], self.auto_gain, self.gain_db, RANGE_DB);
            let a = if target > self.bars[i] { attack } else { decay };
            self.bars[i] += (target - self.bars[i]) * a;

            // Peak caps: jump up with the bar, hold, then fall with gravity.
            if self.bars[i] >= self.peaks[i] {
                self.peaks[i] = self.bars[i];
                self.peak_vel[i] = 0.0;
                self.peak_hold[i] = PEAK_HOLD_S;
            } else if self.peak_hold[i] > 0.0 {
                self.peak_hold[i] -= dt;
            } else {
                self.peak_vel[i] += PEAK_GRAVITY * dt;
                self.peaks[i] = (self.peaks[i] - self.peak_vel[i] * dt).max(self.bars[i]);
            }
        }
    }

    fn analyze(&mut self, samples: &[f32]) {
        let n = samples.len().min(self.fft_size);
        for ((dst, &s), &w) in self.input.iter_mut().zip(samples).zip(&self.window) {
            *dst = s * w;
        }
        self.input[n..].fill(0.0);
        if self
            .fft
            .process(&mut self.input, &mut self.spectrum)
            .is_err()
        {
            return;
        }

        let max_bin = self.spectrum.len() - 1;
        for (i, &(lo, hi)) in self.bands.iter().enumerate() {
            let lo_i = (lo.ceil() as usize).min(max_bin);
            let hi_i = (hi.floor() as usize).min(max_bin);
            // Fold the band's bins per the configured aggregation. Energy
            // (sum) is the default: narrow spikes don't dominate, and since
            // bands are log-spaced (constant fraction of an octave) it is
            // inherently flat for pink-ish program material.
            let power = if lo_i <= hi_i && hi_i > 0 {
                let mut sum = 0.0f32;
                let mut max = 0.0f32;
                for b in lo_i..=hi_i {
                    let p = self.spectrum[b].norm_sqr();
                    sum += p;
                    max = max.max(p);
                }
                let avg = sum / (hi_i - lo_i + 1) as f32;
                match self.agg {
                    BandAgg::Energy => sum,
                    BandAgg::Max => max,
                    BandAgg::Avg => avg,
                    BandAgg::Hybrid => 0.6 * max + 0.4 * avg,
                }
            } else {
                // Band narrower than one bin: interpolate at its center.
                let c = ((lo + hi) * 0.5).clamp(0.0, max_bin as f32);
                let b0 = c.floor() as usize;
                let b1 = (b0 + 1).min(max_bin);
                let frac = c - b0 as f32;
                self.spectrum[b0].norm_sqr() * (1.0 - frac)
                    + self.spectrum[b1].norm_sqr() * frac
            };
            self.raw_db[i] = 10.0 * (power * self.power_scale + 1e-12).log10();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const DT: f32 = 1.0 / 60.0;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn settled(cfg: &Config, samples: &[f32]) -> Analyzer {
        let mut a = Analyzer::new(cfg, 32);
        for _ in 0..120 {
            a.update(samples, RATE, DT, false);
        }
        a
    }

    #[test]
    fn sine_lights_its_own_band() {
        let cfg = Config::default();
        for freq in [100.0, 1000.0, 8000.0] {
            let a = settled(&cfg, &sine(freq, 0.5, cfg.fft_size));
            let (loudest, _) = a
                .bars()
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.total_cmp(y.1))
                .unwrap();
            // Bands are ~0.2 natural-log units wide at 32 bars.
            let off = (a.band_freq(loudest) / freq).ln().abs();
            assert!(off < 0.2, "{freq} Hz landed in band at {} Hz", a.band_freq(loudest));
        }
    }

    #[test]
    fn peaks_never_below_bars() {
        let cfg = Config::default();
        let mut a = settled(&cfg, &sine(1000.0, 0.5, cfg.fft_size));
        for _ in 0..30 {
            a.update(&[], RATE, DT, true);
            assert!(a.bars().iter().zip(a.peaks()).all(|(b, p)| p >= b));
        }
    }

    #[test]
    fn silence_decays_to_zero() {
        let cfg = Config::default();
        let mut a = settled(&cfg, &sine(1000.0, 0.5, cfg.fft_size));
        for _ in 0..300 {
            a.update(&[], RATE, DT, true);
        }
        assert!(a.bars().iter().all(|&b| b < 0.01), "{:?}", a.bars());
    }

    #[test]
    fn manual_gain_maps_range_below_0dbfs() {
        let agc = Agc::new(SILENCE_DB);
        assert_eq!(agc.norm(0.0, false, 0.0, 64.0), 1.0);
        assert_eq!(agc.norm(-64.0, false, 0.0, 64.0), 0.0);
        assert!((agc.norm(-32.0, false, 0.0, 64.0) - 0.5).abs() < 1e-6);
        // +6 dB of manual gain lowers the ceiling by 6 dB.
        assert_eq!(agc.norm(-6.0, false, 6.0, 64.0), 1.0);
    }

    #[test]
    fn agc_rises_fast_and_releases_slowly() {
        let mut agc = Agc::new(SILENCE_DB);
        // One second of loud input pulls the ceiling up almost all the way.
        for _ in 0..60 {
            agc.follow(-5.0, true, DT);
        }
        assert!(agc.ceil_db > -6.0, "rise: {}", agc.ceil_db);
        // One second of quieter input barely moves it back down.
        for _ in 0..60 {
            agc.follow(-40.0, true, DT);
        }
        assert!(agc.ceil_db > -12.0, "release: {}", agc.ceil_db);
    }

    #[test]
    fn agc_ignores_silence_and_manual_mode() {
        let mut agc = Agc::new(SILENCE_DB);
        let start = agc.ceil_db;
        for _ in 0..60 {
            agc.follow(-100.0, true, DT); // below the silence threshold
            agc.follow(0.0, false, DT); // auto gain off
        }
        assert_eq!(agc.ceil_db, start);
    }
}
