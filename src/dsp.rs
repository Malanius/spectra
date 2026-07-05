use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// (name, attack, decay) — lerp factors per frame at ~60 fps.
pub const SMOOTHING_PRESETS: [(&str, f32, f32); 3] = [
    ("fast", 0.95, 0.45),
    ("medium", 0.72, 0.22),
    ("smooth", 0.45, 0.10),
];

/// Displayed dynamic range in dB (bar bottom to bar top).
const RANGE_DB: f32 = 64.0;
/// Seconds a peak cap holds before it starts falling.
const PEAK_HOLD_S: f32 = 0.7;
/// Peak cap fall acceleration, in bar-heights per second squared.
const PEAK_GRAVITY: f32 = 3.5;
/// Anything below this raw level is treated as silence by the AGC.
const SILENCE_DB: f32 = -75.0;

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    fft_size: usize,
    window: Vec<f32>,
    /// Scale factor turning windowed FFT magnitude into ~dBFS.
    mag_scale: f32,
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
    pub gain_db: f32,
    pub auto_gain: bool,
    pub smoothing: usize,
    agc_ceil_db: f32,
}

impl Analyzer {
    pub fn new(
        fft_size: usize,
        n_bars: usize,
        freq_min: f32,
        freq_max: f32,
        gain_db: f32,
        auto_gain: bool,
        smoothing: usize,
    ) -> Self {
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
            mag_scale: 2.0 / window_sum,
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
            freq_min,
            freq_max,
            gain_db,
            auto_gain,
            smoothing: smoothing.min(SMOOTHING_PRESETS.len() - 1),
            agc_ceil_db: -20.0,
        };
        a.set_bars(n_bars);
        a
    }

    pub fn fft_size(&self) -> usize {
        self.fft_size
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

        // Ceiling: with AGC on, follow the recent loudest band so any source
        // fills the display; otherwise a fixed 0 dBFS ceiling. Manual gain
        // shifts the ceiling in both modes.
        if self.auto_gain {
            let frame_max = self
                .raw_db
                .iter()
                .fold(f32::NEG_INFINITY, |a, &b| a.max(b));
            if frame_max > SILENCE_DB {
                let k = if frame_max > self.agc_ceil_db {
                    (dt * 8.0).min(1.0) // rise fast
                } else {
                    (dt * 0.15).min(1.0) // release slowly
                };
                self.agc_ceil_db += (frame_max - self.agc_ceil_db) * k;
                self.agc_ceil_db = self.agc_ceil_db.clamp(-50.0, 0.0);
            }
        }
        let ceil = if self.auto_gain {
            self.agc_ceil_db + 2.0
        } else {
            0.0
        } - self.gain_db;
        let floor = ceil - RANGE_DB;

        let (_, attack, decay) = SMOOTHING_PRESETS[self.smoothing];
        // Presets are tuned for 60 fps; scale lerp factors to the actual dt.
        let scale = (dt * 60.0).clamp(0.25, 4.0);
        let attack = 1.0 - (1.0 - attack).powf(scale);
        let decay = 1.0 - (1.0 - decay).powf(scale);

        for i in 0..self.n_bars {
            let target = ((self.raw_db[i] - floor) / RANGE_DB).clamp(0.0, 1.0);
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
            let mag = if lo_i <= hi_i && hi_i > 0 {
                (lo_i..=hi_i)
                    .map(|b| self.spectrum[b].norm())
                    .fold(0.0f32, f32::max)
            } else {
                // Band narrower than one bin: interpolate at its center.
                let c = ((lo + hi) * 0.5).clamp(0.0, max_bin as f32);
                let b0 = c.floor() as usize;
                let b1 = (b0 + 1).min(max_bin);
                let frac = c - b0 as f32;
                self.spectrum[b0].norm() * (1.0 - frac) + self.spectrum[b1].norm() * frac
            };
            self.raw_db[i] = 20.0 * (mag * self.mag_scale + 1e-10).log10();
        }
    }
}
