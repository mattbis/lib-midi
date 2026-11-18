use crate::quantizer::freq_to_midi_float;
use nih_plug::prelude::Enum;
use std::f32::consts::PI;

pub const MAX_WINDOW_SIZE: usize = 2048;
pub const MAX_FFT_BINS: usize = MAX_WINDOW_SIZE / 2 + 1;
pub const MAX_CANDIDATES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum DetectionMode {
    #[name = "Polyphonic FX Bus (Harmonic Sieve)"]
    PolyphonicBus,
    #[name = "Monophonic Lead / Bass (Fast YIN)"]
    MonophonicYin,
    #[name = "Percussive 3-Band Trigger (Kick/Snare/Hat)"]
    Percussive3Band,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum AnalysisWindow {
    #[name = "1024 (~21 ms @ 48k - Fast)"]
    Samples1024,
    #[name = "2048 (~42 ms @ 48k - Deep Bass/Chords)"]
    Samples2048,
}

impl AnalysisWindow {
    #[inline]
    pub const fn size(self) -> usize {
        match self {
            AnalysisWindow::Samples1024 => 1024,
            AnalysisWindow::Samples2048 => 2048,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PitchCandidate {
    pub freq_hz: f32,
    pub midi_float: f32,
    pub amplitude: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameAnalysis {
    pub rms: f32,
    pub rms_db: f32,
    pub spectral_flux: f32,
    pub spectral_centroid_norm: f32,
    pub candidates: [PitchCandidate; MAX_CANDIDATES],
    pub num_candidates: usize,
}

impl Default for FrameAnalysis {
    fn default() -> Self {
        Self {
            rms: 0.0,
            rms_db: -100.0,
            spectral_flux: 0.0,
            spectral_centroid_norm: 0.0,
            candidates: [PitchCandidate::default(); MAX_CANDIDATES],
            num_candidates: 0,
        }
    }
}

/// Real-time, zero-allocation single-stream audio analyzer.
/// Pre-allocates ring buffer and FFT/YIN scratch arrays so `push_sample` and `analyze_frame`
/// never allocate on the audio thread.
pub struct StreamAnalyzer {
    sample_rate: f32,
    ring_buf: [f32; MAX_WINDOW_SIZE],
    write_pos: usize,
    samples_since_hop: usize,
    hop_size: usize,

    // Precomputed Hann windows
    hann_1024: [f32; 1024],
    hann_2048: [f32; 2048],

    // Scratch buffers for FFT & YIN (no heap allocation on audio thread)
    time_frame: [f32; MAX_WINDOW_SIZE],
    fft_re: [f32; MAX_WINDOW_SIZE],
    fft_im: [f32; MAX_WINDOW_SIZE],
    mag_spectrum: [f32; MAX_FFT_BINS],
    prev_mag_spectrum: [f32; MAX_FFT_BINS],
    residual_spectrum: [f32; MAX_FFT_BINS],
    yin_diff: [f32; MAX_WINDOW_SIZE / 2],

    // Percussive 3-band envelope state
    band_fast_env: [f32; 3],
    band_slow_env: [f32; 3],
    flux_ema: f32,
}

impl StreamAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
        let mut hann_1024 = [0.0_f32; 1024];
        for (i, w) in hann_1024.iter_mut().enumerate() {
            *w = 0.5 * (1.0 - (2.0 * PI * i as f32 / 1023.0).cos());
        }
        let mut hann_2048 = [0.0_f32; 2048];
        for (i, w) in hann_2048.iter_mut().enumerate() {
            *w = 0.5 * (1.0 - (2.0 * PI * i as f32 / 2047.0).cos());
        }

        Self {
            sample_rate: sample_rate.max(8000.0),
            ring_buf: [0.0; MAX_WINDOW_SIZE],
            write_pos: 0,
            samples_since_hop: 0,
            hop_size: 256,
            hann_1024,
            hann_2048,
            time_frame: [0.0; MAX_WINDOW_SIZE],
            fft_re: [0.0; MAX_WINDOW_SIZE],
            fft_im: [0.0; MAX_WINDOW_SIZE],
            mag_spectrum: [0.0; MAX_FFT_BINS],
            prev_mag_spectrum: [0.0; MAX_FFT_BINS],
            residual_spectrum: [0.0; MAX_FFT_BINS],
            yin_diff: [0.0; MAX_WINDOW_SIZE / 2],
            band_fast_env: [0.0; 3],
            band_slow_env: [0.0001; 3],
            flux_ema: 0.001,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(8000.0);
        // Aim for ~5 ms hop size (128 @ <= 32kHz, 256 @ 44.1/48kHz, 512 @ >= 88.2kHz)
        self.hop_size = if self.sample_rate >= 88200.0 {
            512
        } else if self.sample_rate <= 32000.0 {
            128
        } else {
            256
        };
        self.reset();
    }

    pub fn hop_size(&self) -> usize {
        self.hop_size
    }

    pub fn reset(&mut self) {
        self.ring_buf.fill(0.0);
        self.write_pos = 0;
        self.samples_since_hop = 0;
        self.prev_mag_spectrum.fill(0.0);
        self.band_fast_env = [0.0; 3];
        self.band_slow_env = [0.0001; 3];
        self.flux_ema = 0.001;
    }

    /// Pushes one audio sample into the ring buffer. Returns `true` when a hop boundary is reached
    /// and `analyze` should be called.
    #[inline]
    pub fn push_sample(&mut self, sample: f32) -> bool {
        let clean = if sample.is_finite() { sample } else { 0.0 };
        self.ring_buf[self.write_pos] = clean;
        self.write_pos = (self.write_pos + 1) & (MAX_WINDOW_SIZE - 1);
        self.samples_since_hop += 1;
        if self.samples_since_hop >= self.hop_size {
            self.samples_since_hop = 0;
            true
        } else {
            false
        }
    }

    /// Performs zero-allocation frame analysis on the most recent `window_size` samples.
    pub fn analyze(
        &mut self,
        mode: DetectionMode,
        window: AnalysisWindow,
        max_polyphony: usize,
        min_confidence: f32,
        a4_hz: f32,
    ) -> FrameAnalysis {
        let win_size = window.size();
        let num_bins = win_size / 2 + 1;

        // 1. Unwrap the most recent `win_size` samples from `ring_buf` into `time_frame`
        //    and compute time-domain RMS.
        let start_pos = (self.write_pos + MAX_WINDOW_SIZE - win_size) & (MAX_WINDOW_SIZE - 1);
        let mut sum_sq = 0.0_f32;
        for i in 0..win_size {
            let s = self.ring_buf[(start_pos + i) & (MAX_WINDOW_SIZE - 1)];
            self.time_frame[i] = s;
            sum_sq += s * s;
        }
        // Use the most recent hop region for faster attack RMS responsiveness
        let hop_start = win_size.saturating_sub(self.hop_size.max(256));
        let mut hop_sum_sq = 0.0_f32;
        for i in hop_start..win_size {
            let s = self.time_frame[i];
            hop_sum_sq += s * s;
        }
        let full_rms = (sum_sq / win_size as f32).sqrt();
        let hop_rms = (hop_sum_sq / (win_size - hop_start).max(1) as f32).sqrt();
        let rms = full_rms.max(hop_rms * 0.9);
        let rms_db = if rms > 1e-7 {
            20.0 * rms.log10()
        } else {
            -100.0
        };

        let mut out = FrameAnalysis {
            rms,
            rms_db,
            ..Default::default()
        };

        // Early exit if completely silent
        if rms < 1e-5 {
            for b in 0..num_bins {
                self.prev_mag_spectrum[b] *= 0.5;
            }
            return out;
        }

        // 2. Compute windowed FFT for spectral flux, centroid, and polyphonic/percussive modes
        for i in 0..win_size {
            let w = match window {
                AnalysisWindow::Samples1024 => self.hann_1024[i],
                AnalysisWindow::Samples2048 => self.hann_2048[i],
            };
            self.fft_re[i] = self.time_frame[i] * w;
            self.fft_im[i] = 0.0;
        }
        fft_in_place(&mut self.fft_re[..win_size], &mut self.fft_im[..win_size]);

        let norm = 2.0 / win_size as f32;
        let bin_hz = self.sample_rate / win_size as f32;
        let mut raw_flux = 0.0_f32;
        let mut weighted_freq_sum = 0.0_f32;
        let mut mag_sum = 0.0_f32;

        for b in 0..num_bins {
            let re = self.fft_re[b];
            let im = self.fft_im[b];
            let mag = (re * re + im * im).sqrt() * norm;
            self.mag_spectrum[b] = mag;

            let diff = mag - self.prev_mag_spectrum[b];
            if diff > 0.0 {
                raw_flux += diff;
            }
            self.prev_mag_spectrum[b] = mag;

            let f = b as f32 * bin_hz;
            if (60.0..=10000.0).contains(&f) {
                weighted_freq_sum += f * mag;
                mag_sum += mag;
            }
        }

        // Normalize spectral flux relative to running average so onsets stand out clearly
        self.flux_ema = 0.92 * self.flux_ema + 0.08 * raw_flux.max(1e-4);
        out.spectral_flux = ((raw_flux / (self.flux_ema * 2.5 + 1e-4)) - 0.35).clamp(0.0, 1.0);

        // Compute normalized spectral centroid (100 Hz .. 8000 Hz -> 0.0 .. 1.0)
        if mag_sum > 1e-5 {
            let centroid_hz = (weighted_freq_sum / mag_sum).clamp(100.0, 8000.0);
            out.spectral_centroid_norm =
                ((centroid_hz / 100.0).ln() / (8000.0_f32 / 100.0).ln()).clamp(0.0, 1.0);
        }

        // 3. Dispatch pitch / trigger extraction by mode
        match mode {
            DetectionMode::MonophonicYin => {
                if let Some(cand) = self.detect_yin(win_size, min_confidence, a4_hz, rms) {
                    out.candidates[0] = cand;
                    out.num_candidates = 1;
                }
            }
            DetectionMode::PolyphonicBus => {
                let target_poly = max_polyphony.clamp(1, MAX_CANDIDATES);
                out.num_candidates = self.detect_polyphonic_harmonic_sieve(
                    win_size,
                    num_bins,
                    bin_hz,
                    target_poly,
                    min_confidence,
                    a4_hz,
                    rms,
                    &mut out.candidates,
                );
                // If user set polyphony == 1 and harmonic sieve had borderline confidence on low bass,
                // cross-check with YIN for rock-solid low-end tracking
                if out.num_candidates == 0 {
                    if let Some(cand) = self.detect_yin(win_size, min_confidence, a4_hz, rms) {
                        out.candidates[0] = cand;
                        out.num_candidates = 1;
                    }
                }
            }
            DetectionMode::Percussive3Band => {
                out.num_candidates = self.detect_percussive_bands(
                    num_bins,
                    bin_hz,
                    min_confidence,
                    a4_hz,
                    &mut out.candidates,
                );
            }
        }

        out
    }

    /// YIN time-domain pitch estimator (de Cheveigné & Kawahara) with cumulative mean
    /// normalized difference function (CMNDF) and parabolic interpolation.
    fn detect_yin(
        &mut self,
        win_size: usize,
        min_confidence: f32,
        a4_hz: f32,
        rms: f32,
    ) -> Option<PitchCandidate> {
        let half_w = win_size / 2;
        // Search range: 45 Hz up to 2200 Hz
        let tau_min = ((self.sample_rate / 2200.0) as usize).max(2);
        let tau_max = ((self.sample_rate / 45.0) as usize).min(half_w - 2);
        if tau_min >= tau_max {
            return None;
        }

        self.yin_diff[0] = 1.0;
        let mut running_sum = 0.0_f32;

        // Compute difference function d(tau) on the most recent half-window
        let offset = win_size - half_w * 2;
        let frame = &self.time_frame[offset..];

        for tau in 1..=tau_max + 1 {
            let mut sum = 0.0_f32;
            for j in 0..half_w {
                let delta = frame[j] - frame[j + tau];
                sum += delta * delta;
            }
            running_sum += sum;
            if running_sum > 1e-9 {
                self.yin_diff[tau] = sum * (tau as f32) / running_sum;
            } else {
                self.yin_diff[tau] = 1.0;
            }
        }

        // Aperiodicity threshold derived from `min_confidence`
        let aperiodicity_thresh = (1.0 - min_confidence).clamp(0.08, 0.55);

        let mut best_tau = 0usize;
        let mut tau = tau_min;
        while tau <= tau_max {
            if self.yin_diff[tau] < aperiodicity_thresh {
                while tau + 1 <= tau_max && self.yin_diff[tau + 1] < self.yin_diff[tau] {
                    tau += 1;
                }
                best_tau = tau;
                break;
            }
            tau += 1;
        }

        // Fallback to global minimum if it meets a reasonable clarity threshold
        if best_tau == 0 {
            let mut min_val = f32::MAX;
            for (t, &val) in self.yin_diff.iter().enumerate().take(tau_max + 1).skip(tau_min) {
                if val < min_val {
                    min_val = val;
                    best_tau = t;
                }
            }
            if min_val > aperiodicity_thresh * 1.25 {
                return None;
            }
        }

        let confidence = (1.0 - self.yin_diff[best_tau]).clamp(0.0, 1.0);
        if confidence < min_confidence {
            return None;
        }

        // Parabolic interpolation around minimum `best_tau`
        let y0 = self.yin_diff[best_tau - 1];
        let y1 = self.yin_diff[best_tau];
        let y2 = self.yin_diff[best_tau + 1];
        let denom = 2.0 * (y0 + y2 - 2.0 * y1);
        let frac = if denom.abs() > 1e-7 {
            ((y0 - y2) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };

        let refined_tau = best_tau as f32 + frac;
        let freq_hz = self.sample_rate / refined_tau;
        let midi_float = freq_to_midi_float(freq_hz, a4_hz);
        if !(21.0..=108.0).contains(&midi_float) {
            return None;
        }

        Some(PitchCandidate {
            freq_hz,
            midi_float,
            amplitude: (rms * 2.2).clamp(0.0, 1.0),
            confidence,
        })
    }

    /// Polyphonic Iterative Harmonic Sieve:
    /// Extracts up to `max_polyphony` simultaneous notes from the magnitude spectrum by:
    /// 1. Copying the spectrum to `residual_spectrum` and finding peak energy.
    /// 2. Iteratively scoring candidate fundamental peaks via harmonic summation ($f_0 .. 5f_0$)
    ///    while penalizing sub-harmonic positions ($f_0/2$) to prevent octave-up errors.
    /// 3. Subtracting the detected fundamental and its harmonics ($1..=8$) from `residual_spectrum`
    ///    so overtones of lower chord notes are not misidentified as separate higher notes.
    #[allow(clippy::too_many_arguments)]
    fn detect_polyphonic_harmonic_sieve(
        &mut self,
        _win_size: usize,
        num_bins: usize,
        bin_hz: f32,
        max_polyphony: usize,
        min_confidence: f32,
        a4_hz: f32,
        rms: f32,
        out_candidates: &mut [PitchCandidate; MAX_CANDIDATES],
    ) -> usize {
        let min_bin = ((50.0 / bin_hz).ceil() as usize).max(2);
        let max_bin = ((2600.0 / bin_hz).floor() as usize).min(num_bins.saturating_sub(3));
        if min_bin >= max_bin {
            return 0;
        }

        let mut max_mag = 0.0_f32;
        let mut mag_sum = 0.0_f32;
        for b in 0..num_bins {
            let m = self.mag_spectrum[b];
            self.residual_spectrum[b] = m;
            if b >= min_bin && b <= max_bin && m > max_mag {
                max_mag = m;
            }
            mag_sum += m;
        }

        if max_mag < 1e-4 {
            return 0;
        }

        let mean_mag = mag_sum / (num_bins as f32);
        // Spectral flatness check (reject broadband white noise)
        let peak_to_mean = max_mag / (mean_mag + 1e-7);
        if peak_to_mean < 3.5 {
            return 0;
        }

        // Relative floor for secondary chord voices relative to the loudest spectral peak
        let voice_rel_floor = (0.14 + (1.0 - min_confidence) * 0.12).clamp(0.10, 0.45);
        let mut count = 0usize;

        for _voice_idx in 0..max_polyphony {
            let mut best_bin = 0usize;
            let mut best_salience = 0.0_f32;
            let mut best_fund_mag = 0.0_f32;

            for b in min_bin..=max_bin {
                let m0 = self.residual_spectrum[b];
                // Must be a local peak in residual spectrum and above relative floor
                if m0 < max_mag * voice_rel_floor
                    || m0 < self.residual_spectrum[b - 1]
                    || m0 < self.residual_spectrum[b + 1]
                {
                    continue;
                }

                // Parabolic bin refinement around maximum `y1`
                let y0 = self.residual_spectrum[b - 1];
                let y1 = m0;
                let y2 = self.residual_spectrum[b + 1];
                let denom = 2.0 * (2.0 * y1 - y0 - y2);
                let frac = if denom.abs() > 1e-7 {
                    ((y2 - y0) / denom).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                let exact_bin = b as f32 + frac;

                // Sum weighted harmonic partials (h = 1..=5)
                let mut salience = m0;
                for h in 2..=5 {
                    let h_bin = (exact_bin * h as f32).round() as usize;
                    if h_bin + 1 < num_bins {
                        let hm = self.residual_spectrum[h_bin]
                            .max(self.residual_spectrum[h_bin - 1])
                            .max(self.residual_spectrum[h_bin + 1]);
                        salience += hm * (0.65 / (h as f32).sqrt());
                    }
                }

                // Penalize if this peak is likely the 2nd harmonic of an existing strong sub-octave
                let half_bin = (exact_bin * 0.5).round() as usize;
                if half_bin >= min_bin {
                    let sub_m = self.residual_spectrum[half_bin]
                        .max(self.residual_spectrum[half_bin.saturating_sub(1)])
                        .max(self.residual_spectrum[(half_bin + 1).min(num_bins - 1)]);
                    if sub_m > m0 * 0.35 {
                        salience *= 0.45;
                    }
                }

                if salience > best_salience {
                    best_salience = salience;
                    best_bin = b;
                    best_fund_mag = m0;
                }
            }

            if best_bin == 0 {
                break;
            }

            // Refine frequency of winning peak using original magnitude spectrum for accuracy
            let y0 = self.mag_spectrum[best_bin - 1].max(1e-7).ln();
            let y1 = self.mag_spectrum[best_bin].max(1e-7).ln();
            let y2 = self.mag_spectrum[best_bin + 1].max(1e-7).ln();
            let denom = 2.0 * (2.0 * y1 - y0 - y2);
            let frac = if denom.abs() > 1e-7 {
                ((y2 - y0) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let exact_bin = best_bin as f32 + frac;
            let freq_hz = exact_bin * bin_hz;
            let midi_float = freq_to_midi_float(freq_hz, a4_hz);

            // Compute confidence from salience & spectral SNR
            let norm_salience = (best_salience / (max_mag * 1.15 + 1e-6)).clamp(0.0, 1.0);
            let snr_conf = ((peak_to_mean - 3.5) / 14.0).clamp(0.0, 1.0);
            let confidence = (norm_salience * 0.7 + snr_conf * 0.3).clamp(0.0, 1.0);

            if confidence < min_confidence * 0.85 || !(21.0..=108.0).contains(&midi_float) {
                break;
            }

            // Avoid duplicate candidates within 0.85 semitones of an already selected candidate
            let is_duplicate = out_candidates[..count]
                .iter()
                .any(|c| (c.midi_float - midi_float).abs() < 0.85);

            if !is_duplicate {
                let rel_amp = (best_fund_mag / max_mag).clamp(0.25, 1.0);
                out_candidates[count] = PitchCandidate {
                    freq_hz,
                    midi_float,
                    amplitude: (rms * 2.4 * rel_amp).clamp(0.0, 1.0),
                    confidence,
                };
                count += 1;
            }

            // Subtract fundamental and harmonics (h = 1..=8) from `residual_spectrum`
            for h in 1..=8 {
                let center = (exact_bin * h as f32).round() as isize;
                // Wider notch on higher harmonics due to inharmonicity / window main-lobe width
                let radius = if h == 1 { 2 } else { 3 };
                for idx in (center - radius)..=(center + radius) {
                    if idx >= 0 && (idx as usize) < num_bins {
                        self.residual_spectrum[idx as usize] *= 0.05;
                    }
                }
            }
        }

        count
    }

    /// 3-Band Percussive Transient Detector:
    /// Maps Low (Kick -> C1 / 36), Mid (Snare -> D1 / 38), High (Hi-Hat -> F#1 / 42)
    /// when fast/slow envelope ratios spike above threshold.
    fn detect_percussive_bands(
        &mut self,
        num_bins: usize,
        bin_hz: f32,
        min_confidence: f32,
        a4_hz: f32,
        out_candidates: &mut [PitchCandidate; MAX_CANDIDATES],
    ) -> usize {
        // Band frequency ranges (Hz) and default General MIDI drum note numbers:
        // Band 0: 35..160 Hz -> Note 36 (C1 Kick)
        // Band 1: 160..2200 Hz -> Note 38 (D1 Snare)
        // Band 2: 2200..12000 Hz -> Note 42 (F#1 Closed Hat)
        const BANDS: [(f32, f32, u8); 3] = [
            (35.0, 160.0, 36),
            (160.0, 2200.0, 38),
            (2200.0, 12000.0, 42),
        ];

        let mut count = 0usize;
        for (band_idx, &(f_lo, f_hi, gm_note)) in BANDS.iter().enumerate() {
            let b_start = ((f_lo / bin_hz).floor() as usize).clamp(1, num_bins.saturating_sub(1));
            let b_end = ((f_hi / bin_hz).ceil() as usize).clamp(b_start, num_bins.saturating_sub(1));

            let mut energy = 0.0_f32;
            for b in b_start..=b_end {
                energy += self.mag_spectrum[b];
            }
            energy /= (b_end - b_start + 1).max(1) as f32;

            self.band_fast_env[band_idx] = 0.35 * self.band_fast_env[band_idx] + 0.65 * energy;
            self.band_slow_env[band_idx] =
                (0.92 * self.band_slow_env[band_idx] + 0.08 * energy).max(1e-5);

            let ratio = self.band_fast_env[band_idx] / self.band_slow_env[band_idx];
            let trigger_ratio = 1.45 + min_confidence * 1.1;

            if ratio > trigger_ratio && self.band_fast_env[band_idx] > 0.0015 {
                let conf = ((ratio - 1.2) / 2.5).clamp(0.0, 1.0);
                let freq_hz = crate::quantizer::midi_to_freq_hz(gm_note, a4_hz);
                out_candidates[count] = PitchCandidate {
                    freq_hz,
                    midi_float: gm_note as f32,
                    amplitude: (self.band_fast_env[band_idx] * 18.0).clamp(0.15, 1.0),
                    confidence: conf,
                };
                count += 1;
            }
        }

        count
    }
}

/// In-place Radix-2 Cooley-Tukey complex FFT (`re.len()` must be a power of two).
pub fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && n == im.len());

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Butterfly stages
    let mut len = 2usize;
    while len <= n {
        let half = len >> 1;
        let angle = -2.0 * PI / (len as f32);
        let wlen_re = angle.cos();
        let wlen_im = angle.sin();

        let mut i = 0usize;
        while i < n {
            let mut w_re = 1.0_f32;
            let mut w_im = 0.0_f32;
            for k in 0..half {
                let u_re = re[i + k];
                let u_im = im[i + k];
                let v_re = re[i + k + half] * w_re - im[i + k + half] * w_im;
                let v_im = re[i + k + half] * w_im + im[i + k + half] * w_re;

                re[i + k] = u_re + v_re;
                im[i + k] = u_im + v_im;
                re[i + k + half] = u_re - v_re;
                im[i + k + half] = u_im - v_im;

                let next_w_re = w_re * wlen_re - w_im * wlen_im;
                let next_w_im = w_re * wlen_im + w_im * wlen_re;
                w_re = next_w_re;
                w_im = next_w_im;
            }
            i += len;
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monophonic_yin_sine_440hz() {
        let sr = 48000.0;
        let mut analyzer = StreamAnalyzer::new(sr);
        let mut last_frame = FrameAnalysis::default();

        for n in 0..2048 {
            let t = n as f32 / sr;
            let s = 0.5 * (2.0 * PI * 440.0 * t).sin();
            if analyzer.push_sample(s) {
                last_frame = analyzer.analyze(
                    DetectionMode::MonophonicYin,
                    AnalysisWindow::Samples2048,
                    1,
                    0.5,
                    440.0,
                );
            }
        }

        assert_eq!(last_frame.num_candidates, 1);
        let c = last_frame.candidates[0];
        assert!(
            (c.freq_hz - 440.0).abs() < 1.5,
            "Expected ~440 Hz, got {}",
            c.freq_hz
        );
        assert!((c.midi_float - 69.0).abs() < 0.1);
    }

    #[test]
    fn test_polyphonic_dyad_c4_e4_g4() {
        let sr = 48000.0;
        let mut analyzer = StreamAnalyzer::new(sr);
        let mut last_frame = FrameAnalysis::default();

        // Synthesize C4 (261.63 Hz), E4 (329.63 Hz), G4 (392.00 Hz) with 2nd harmonic
        let freqs = [261.63_f32, 329.63, 392.00];
        for n in 0..2048 {
            let t = n as f32 / sr;
            let mut s = 0.0_f32;
            for &f in &freqs {
                s += 0.25 * (2.0 * PI * f * t).sin() + 0.08 * (2.0 * PI * (2.0 * f) * t).sin();
            }
            if analyzer.push_sample(s) {
                last_frame = analyzer.analyze(
                    DetectionMode::PolyphonicBus,
                    AnalysisWindow::Samples2048,
                    4,
                    0.4,
                    440.0,
                );
            }
        }

        assert!(
            last_frame.num_candidates >= 3,
            "Expected at least 3 chord notes, got {}",
            last_frame.num_candidates
        );
        let detected_notes: Vec<u8> = last_frame.candidates[..last_frame.num_candidates]
            .iter()
            .map(|c| c.midi_float.round() as u8)
            .collect();
        assert!(detected_notes.contains(&60), "Missing C4 in {:?}", detected_notes);
        assert!(detected_notes.contains(&64), "Missing E4 in {:?}", detected_notes);
        assert!(detected_notes.contains(&67), "Missing G4 in {:?}", detected_notes);
    }
}
