use crate::dsp::{AnalysisWindow, DetectionMode, FrameAnalysis, StreamAnalyzer, MAX_CANDIDATES};
use crate::quantizer::{quantize_pitch, RootKey, ScaleType};
use nih_plug::prelude::Enum;

pub const MAX_EVENTS_PER_BLOCK: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum StereoRoutingMode {
    #[name = "Stereo Sum (L+R -> Base Ch)"]
    StereoSum,
    #[name = "Dual Split (L -> Ch N, R -> Ch N+1)"]
    DualLeftRight,
    #[name = "Mid/Side Split (Mid -> Ch N, Side -> Ch N+1)"]
    MidSideSplit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum EnvelopeCcTarget {
    #[name = "Off"]
    Off,
    #[name = "CC 1 (Modulation Wheel)"]
    ModWheelCc1,
    #[name = "CC 2 (Breath Controller)"]
    BreathCc2,
    #[name = "CC 11 (Expression)"]
    ExpressionCc11,
}

impl EnvelopeCcTarget {
    #[inline]
    pub const fn cc_number(self) -> Option<u8> {
        match self {
            EnvelopeCcTarget::Off => None,
            EnvelopeCcTarget::ModWheelCc1 => Some(1),
            EnvelopeCcTarget::BreathCc2 => Some(2),
            EnvelopeCcTarget::ExpressionCc11 => Some(11),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiOutEvent {
    NoteOn {
        timing: u32,
        channel: u8,
        note: u8,
        velocity: f32,
    },
    NoteOff {
        timing: u32,
        channel: u8,
        note: u8,
        velocity: f32,
    },
    PitchBend {
        timing: u32,
        channel: u8,
        /// Normalized `0.0..=1.0`, where `0.5` is center (no bend).
        value: f32,
    },
    ControlChange {
        timing: u32,
        channel: u8,
        cc: u8,
        /// Normalized `0.0..=1.0`.
        value: f32,
    },
}

/// Fixed-capacity real-time MIDI event buffer (zero heap allocation).
pub struct MidiEventBuffer {
    events: [MidiOutEvent; MAX_EVENTS_PER_BLOCK],
    len: usize,
}

impl Default for MidiEventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiEventBuffer {
    pub const fn new() -> Self {
        Self {
            events: [MidiOutEvent::NoteOff {
                timing: 0,
                channel: 0,
                note: 0,
                velocity: 0.0,
            }; MAX_EVENTS_PER_BLOCK],
            len: 0,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    pub fn push(&mut self, event: MidiOutEvent) {
        if self.len < MAX_EVENTS_PER_BLOCK {
            self.events[self.len] = event;
            self.len += 1;
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[MidiOutEvent] {
        &self.events[..self.len]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TrackerConfig {
    pub mode: DetectionMode,
    pub window: AnalysisWindow,
    pub routing: StereoRoutingMode,
    pub base_channel: u8,
    pub max_polyphony: usize,
    pub onset_threshold_db: f32,
    pub release_hysteresis_db: f32,
    pub min_confidence: f32,
    pub min_hold_ms: f32,
    pub stability_hops: u8,
    pub retrigger_flux_thresh: f32,
    pub velocity_sensitivity: f32,
    pub min_velocity: u8,
    pub root_key: RootKey,
    pub scale_type: ScaleType,
    pub transpose_semitones: i32,
    pub min_note: u8,
    pub max_note: u8,
    pub pitch_bend_enabled: bool,
    pub pitch_bend_range: f32,
    pub env_cc_target: EnvelopeCcTarget,
    pub brightness_cc74: bool,
    pub a4_hz: f32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            mode: DetectionMode::PolyphonicBus,
            window: AnalysisWindow::Samples2048,
            routing: StereoRoutingMode::StereoSum,
            base_channel: 0,
            max_polyphony: 4,
            onset_threshold_db: -38.0,
            release_hysteresis_db: 6.0,
            min_confidence: 0.50,
            min_hold_ms: 45.0,
            stability_hops: 2,
            retrigger_flux_thresh: 0.55,
            velocity_sensitivity: 1.0,
            min_velocity: 20,
            root_key: RootKey::C,
            scale_type: ScaleType::Chromatic,
            transpose_semitones: 0,
            min_note: 24,
            max_note: 108,
            pitch_bend_enabled: false,
            pitch_bend_range: 2.0,
            env_cc_target: EnvelopeCcTarget::Off,
            brightness_cc74: false,
            a4_hz: 440.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ActiveNoteSlot {
    active: bool,
    velocity: f32,
    age_hops: u16,
    missing_hops: u8,
    last_amplitude: f32,
}

/// Single-channel note state machine paired with a `StreamAnalyzer`.
pub struct ChannelNoteTracker {
    analyzer: StreamAnalyzer,
    notes: [ActiveNoteSlot; 128],
    candidate_streak: [u8; 128],
    last_pitch_bend: f32,
    last_env_cc_val: u8,
    last_cc74_val: u8,
    smoothed_env: f32,
    smoothed_centroid: f32,
    pub last_analysis: FrameAnalysis,
    pub primary_detected_note: Option<u8>,
    pub primary_detected_freq: f32,
    pub primary_detected_cents: f32,
    pub primary_confidence: f32,
}

impl ChannelNoteTracker {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            analyzer: StreamAnalyzer::new(sample_rate),
            notes: [ActiveNoteSlot::default(); 128],
            candidate_streak: [0; 128],
            last_pitch_bend: 0.5,
            last_env_cc_val: 0,
            last_cc74_val: 0,
            smoothed_env: 0.0,
            smoothed_centroid: 0.0,
            last_analysis: FrameAnalysis::default(),
            primary_detected_note: None,
            primary_detected_freq: 0.0,
            primary_detected_cents: 0.0,
            primary_confidence: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.analyzer.set_sample_rate(sample_rate);
    }

    pub fn hop_size(&self) -> usize {
        self.analyzer.hop_size()
    }

    pub fn reset(&mut self) {
        self.analyzer.reset();
        self.notes = [ActiveNoteSlot::default(); 128];
        self.candidate_streak = [0; 128];
        self.last_pitch_bend = 0.5;
        self.last_env_cc_val = 0;
        self.last_cc74_val = 0;
        self.smoothed_env = 0.0;
        self.smoothed_centroid = 0.0;
        self.last_analysis = FrameAnalysis::default();
        self.primary_detected_note = None;
        self.primary_detected_freq = 0.0;
        self.primary_detected_cents = 0.0;
        self.primary_confidence = 0.0;
    }

    /// Emits `NoteOff` for all currently active notes on `channel` and clears state.
    pub fn all_notes_off(&mut self, channel: u8, timing: u32, out_events: &mut MidiEventBuffer) {
        for (note_idx, slot) in self.notes.iter_mut().enumerate() {
            if slot.active {
                slot.active = false;
                slot.age_hops = 0;
                slot.missing_hops = 0;
                out_events.push(MidiOutEvent::NoteOff {
                    timing,
                    channel,
                    note: note_idx as u8,
                    velocity: 0.5,
                });
            }
        }
        self.candidate_streak.fill(0);
        if (self.last_pitch_bend - 0.5).abs() > 1e-3 {
            self.last_pitch_bend = 0.5;
            out_events.push(MidiOutEvent::PitchBend {
                timing,
                channel,
                value: 0.5,
            });
        }
    }

    pub fn active_note_mask(&self) -> [u8; 128] {
        let mut mask = [0u8; 128];
        for (i, slot) in self.notes.iter().enumerate() {
            if slot.active {
                mask[i] = (slot.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
            }
        }
        mask
    }

    pub fn active_voice_count(&self) -> usize {
        self.notes.iter().filter(|n| n.active).count()
    }

    #[inline]
    pub fn process_sample(
        &mut self,
        sample: f32,
        sample_rate: f32,
        timing: u32,
        channel: u8,
        cfg: &TrackerConfig,
        out_events: &mut MidiEventBuffer,
    ) {
        if !self.analyzer.push_sample(sample) {
            return;
        }

        let eff_polyphony = match cfg.mode {
            DetectionMode::MonophonicYin => 1,
            DetectionMode::PolyphonicBus => cfg.max_polyphony.clamp(1, MAX_CANDIDATES),
            DetectionMode::Percussive3Band => 3,
        };

        let analysis = self.analyzer.analyze(
            cfg.mode,
            cfg.window,
            eff_polyphony,
            cfg.min_confidence,
            cfg.a4_hz,
        );
        self.last_analysis = analysis;

        let hop_ms = (self.analyzer.hop_size() as f32 * 1000.0) / sample_rate.max(8000.0);
        let min_hold_hops = ((cfg.min_hold_ms / hop_ms.max(1.0)).round() as u16).max(1);
        let release_thresh_db = cfg.onset_threshold_db - cfg.release_hysteresis_db.abs();

        // 1. Update continuous CC followers (Envelope & Spectral Centroid Brightness)
        let norm_env = ((analysis.rms_db - release_thresh_db) / (0.0 - release_thresh_db).max(6.0))
            .clamp(0.0, 1.0);
        self.smoothed_env = 0.65 * self.smoothed_env + 0.35 * norm_env;

        if let Some(cc_num) = cfg.env_cc_target.cc_number() {
            let cc_u7 = (self.smoothed_env * 127.0).round() as u8;
            if cc_u7 != self.last_env_cc_val {
                self.last_env_cc_val = cc_u7;
                out_events.push(MidiOutEvent::ControlChange {
                    timing,
                    channel,
                    cc: cc_num,
                    value: self.smoothed_env,
                });
            }
        }

        if cfg.brightness_cc74 {
            self.smoothed_centroid =
                0.75 * self.smoothed_centroid + 0.25 * analysis.spectral_centroid_norm;
            let cc74_u7 = (self.smoothed_centroid * 127.0).round() as u8;
            if cc74_u7 != self.last_cc74_val {
                self.last_cc74_val = cc74_u7;
                out_events.push(MidiOutEvent::ControlChange {
                    timing,
                    channel,
                    cc: 74,
                    value: self.smoothed_centroid,
                });
            }
        }

        // 2. Gate check: if below release threshold, age and release active notes
        if analysis.rms_db < release_thresh_db {
            self.primary_detected_note = None;
            self.primary_confidence = 0.0;
            self.candidate_streak.fill(0);
            for (note_idx, slot) in self.notes.iter_mut().enumerate() {
                if slot.active {
                    slot.age_hops = slot.age_hops.saturating_add(1);
                    slot.missing_hops = slot.missing_hops.saturating_add(1);
                    if slot.age_hops >= min_hold_hops && slot.missing_hops >= 2 {
                        slot.active = false;
                        slot.age_hops = 0;
                        slot.missing_hops = 0;
                        out_events.push(MidiOutEvent::NoteOff {
                            timing,
                            channel,
                            note: note_idx as u8,
                            velocity: 0.5,
                        });
                    }
                }
            }
            return;
        }

        // 3. Map detected pitch candidates to quantized MIDI notes
        let mut frame_notes_present = [false; 128];
        let mut frame_note_amp = [0.0_f32; 128];
        let mut primary_cents = 0.0_f32;
        let mut found_primary = false;

        for c in &analysis.candidates[..analysis.num_candidates] {
            let quantized = if cfg.mode == DetectionMode::Percussive3Band {
                let n = (c.midi_float.round() as i32 + cfg.transpose_semitones).clamp(0, 127) as u8;
                Some((n, 0.0))
            } else {
                quantize_pitch(
                    c.midi_float,
                    cfg.transpose_semitones,
                    cfg.root_key,
                    cfg.scale_type,
                    cfg.min_note,
                    cfg.max_note,
                )
            };

            if let Some((note_u8, cents)) = quantized {
                let idx = note_u8 as usize;
                frame_notes_present[idx] = true;
                if c.amplitude > frame_note_amp[idx] {
                    frame_note_amp[idx] = c.amplitude;
                }
                if !found_primary {
                    found_primary = true;
                    self.primary_detected_note = Some(note_u8);
                    self.primary_detected_freq = c.freq_hz;
                    self.primary_detected_cents = cents;
                    self.primary_confidence = c.confidence;
                    primary_cents = cents;
                }
            }
        }

        if !found_primary {
            self.primary_detected_note = None;
            self.primary_confidence = 0.0;
        }

        // 4. Update existing active notes (sustain vs release)
        for (note_idx, slot) in self.notes.iter_mut().enumerate() {
            if !slot.active {
                continue;
            }
            slot.age_hops = slot.age_hops.saturating_add(1);

            if frame_notes_present[note_idx] {
                slot.missing_hops = 0;

                // Check for sharp transient re-trigger on the same note
                let amp = frame_note_amp[note_idx];
                let sharp_attack = analysis.spectral_flux > cfg.retrigger_flux_thresh
                    && amp > slot.last_amplitude * 1.45
                    && analysis.rms_db >= cfg.onset_threshold_db
                    && slot.age_hops >= min_hold_hops;

                if sharp_attack {
                    out_events.push(MidiOutEvent::NoteOff {
                        timing,
                        channel,
                        note: note_idx as u8,
                        velocity: 0.5,
                    });
                    let vel = compute_velocity(
                        amp,
                        analysis.rms_db,
                        cfg.onset_threshold_db,
                        cfg.velocity_sensitivity,
                        cfg.min_velocity,
                    );
                    slot.velocity = vel;
                    slot.age_hops = 0;
                    slot.last_amplitude = amp;
                    out_events.push(MidiOutEvent::NoteOn {
                        timing,
                        channel,
                        note: note_idx as u8,
                        velocity: vel,
                    });
                } else {
                    slot.last_amplitude = 0.85 * slot.last_amplitude + 0.15 * amp;
                }
            } else {
                slot.missing_hops = slot.missing_hops.saturating_add(1);
                // Release note once minimum hold duration has elapsed and pitch is absent for >= 2 hops
                let grace_hops = if cfg.mode == DetectionMode::Percussive3Band {
                    1
                } else {
                    2
                };
                if slot.age_hops >= min_hold_hops && slot.missing_hops >= grace_hops {
                    slot.active = false;
                    slot.age_hops = 0;
                    slot.missing_hops = 0;
                    out_events.push(MidiOutEvent::NoteOff {
                        timing,
                        channel,
                        note: note_idx as u8,
                        velocity: 0.5,
                    });
                }
            }
        }

        // 5. Trigger new notes if signal exceeds `onset_threshold_db`
        let required_streak = if cfg.mode == DetectionMode::Percussive3Band
            || analysis.spectral_flux > cfg.retrigger_flux_thresh
        {
            1
        } else {
            cfg.stability_hops.max(1)
        };

        for note_idx in 0..128 {
            if frame_notes_present[note_idx] {
                self.candidate_streak[note_idx] = self.candidate_streak[note_idx].saturating_add(1);
            } else {
                self.candidate_streak[note_idx] = 0;
            }

            if !self.notes[note_idx].active
                && frame_notes_present[note_idx]
                && analysis.rms_db >= cfg.onset_threshold_db
                && self.candidate_streak[note_idx] >= required_streak
            {
                // Enforce `eff_polyphony` voice limit by stealing the oldest or quietest active note if full
                while self.active_voice_count() >= eff_polyphony {
                    if let Some((steal_idx, _)) = self
                        .notes
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| s.active)
                        .max_by_key(|(_, s)| (s.missing_hops, s.age_hops))
                    {
                        self.notes[steal_idx].active = false;
                        self.notes[steal_idx].age_hops = 0;
                        self.notes[steal_idx].missing_hops = 0;
                        out_events.push(MidiOutEvent::NoteOff {
                            timing,
                            channel,
                            note: steal_idx as u8,
                            velocity: 0.5,
                        });
                    } else {
                        break;
                    }
                }

                let amp = frame_note_amp[note_idx];
                let vel = compute_velocity(
                    amp,
                    analysis.rms_db,
                    cfg.onset_threshold_db,
                    cfg.velocity_sensitivity,
                    cfg.min_velocity,
                );
                self.notes[note_idx] = ActiveNoteSlot {
                    active: true,
                    velocity: vel,
                    age_hops: 0,
                    missing_hops: 0,
                    last_amplitude: amp,
                };
                out_events.push(MidiOutEvent::NoteOn {
                    timing,
                    channel,
                    note: note_idx as u8,
                    velocity: vel,
                });
            }
        }

        // 6. Optional Pitch Bend output (when enabled and 1 note is active)
        if cfg.pitch_bend_enabled && found_primary && self.active_voice_count() == 1 {
            let pb_range_cents = (cfg.pitch_bend_range.max(1.0)) * 100.0;
            let norm_bend = (0.5 + (primary_cents / (2.0 * pb_range_cents))).clamp(0.0, 1.0);
            if (norm_bend - self.last_pitch_bend).abs() > 0.004 {
                self.last_pitch_bend = norm_bend;
                out_events.push(MidiOutEvent::PitchBend {
                    timing,
                    channel,
                    value: norm_bend,
                });
            }
        } else if !cfg.pitch_bend_enabled && (self.last_pitch_bend - 0.5).abs() > 0.004 {
            self.last_pitch_bend = 0.5;
            out_events.push(MidiOutEvent::PitchBend {
                timing,
                channel,
                value: 0.5,
            });
        }
    }
}

#[inline]
fn compute_velocity(
    amp: f32,
    rms_db: f32,
    onset_db: f32,
    sensitivity: f32,
    min_vel_u8: u8,
) -> f32 {
    let headroom = (0.0 - onset_db).max(12.0);
    let db_norm = ((rms_db - onset_db) / headroom).clamp(0.0, 1.0);
    let combined = (db_norm * 0.7 + amp.clamp(0.0, 1.0) * 0.3) * sensitivity.clamp(0.2, 2.5);
    let min_v = (min_vel_u8.clamp(1, 126) as f32) / 127.0;
    (min_v + combined * (1.0 - min_v)).clamp(min_v, 1.0)
}

/// Complete 2-Channel Stereo FX Bus Audio-to-MIDI Engine.
/// Contains two independent `ChannelNoteTracker` instances to support:
/// - Stereo Sum (L+R -> Channel N)
/// - Dual Left/Right Split (L -> Channel N, R -> Channel N+1)
/// - Mid/Side Split (Mid -> Channel N, Side -> Channel N+1)
pub struct StereoRealtimeMidiEngine {
    sample_rate: f32,
    pub stream_a: ChannelNoteTracker,
    pub stream_b: ChannelNoteTracker,
    last_routing: StereoRoutingMode,
    last_base_channel: u8,
    pub peak_l: f32,
    pub peak_r: f32,
}

impl StereoRealtimeMidiEngine {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            stream_a: ChannelNoteTracker::new(sample_rate),
            stream_b: ChannelNoteTracker::new(sample_rate),
            last_routing: StereoRoutingMode::StereoSum,
            last_base_channel: 0,
            peak_l: 0.0,
            peak_r: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(8000.0);
        self.stream_a.set_sample_rate(self.sample_rate);
        self.stream_b.set_sample_rate(self.sample_rate);
    }

    pub fn reset(&mut self) {
        self.stream_a.reset();
        self.stream_b.reset();
        self.peak_l = 0.0;
        self.peak_r = 0.0;
    }

    pub fn panic_all_notes_off(&mut self, out_events: &mut MidiEventBuffer) {
        let ch_a = self.last_base_channel.min(15);
        let ch_b = (self.last_base_channel + 1).min(15);
        self.stream_a.all_notes_off(ch_a, 0, out_events);
        self.stream_b.all_notes_off(ch_b, 0, out_events);
    }

    /// Processes a block of 2-channel stereo audio samples, populating `out_events` with
    /// sample-accurate MIDI messages. Zero heap allocations occur here.
    pub fn process_stereo_block(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        cfg: &TrackerConfig,
        out_events: &mut MidiEventBuffer,
    ) {
        let ch_a = cfg.base_channel.min(15);
        let ch_b = (cfg.base_channel + 1).min(15);

        // If routing mode or MIDI channel changed, cleanly release any hanging notes first
        if cfg.routing != self.last_routing || cfg.base_channel != self.last_base_channel {
            let old_a = self.last_base_channel.min(15);
            let old_b = (self.last_base_channel + 1).min(15);
            self.stream_a.all_notes_off(old_a, 0, out_events);
            self.stream_b.all_notes_off(old_b, 0, out_events);
            self.last_routing = cfg.routing;
            self.last_base_channel = cfg.base_channel;
        }

        let frames = in_l.len().min(in_r.len());
        let mut block_max_l = 0.0_f32;
        let mut block_max_r = 0.0_f32;

        for frame_idx in 0..frames {
            let l = in_l[frame_idx];
            let r = in_r[frame_idx];
            let abs_l = l.abs();
            let abs_r = r.abs();
            if abs_l > block_max_l {
                block_max_l = abs_l;
            }
            if abs_r > block_max_r {
                block_max_r = abs_r;
            }

            let timing = frame_idx as u32;
            match cfg.routing {
                StereoRoutingMode::StereoSum => {
                    let mono = (l + r) * 0.5;
                    self.stream_a.process_sample(
                        mono,
                        self.sample_rate,
                        timing,
                        ch_a,
                        cfg,
                        out_events,
                    );
                }
                StereoRoutingMode::DualLeftRight => {
                    self.stream_a.process_sample(
                        l,
                        self.sample_rate,
                        timing,
                        ch_a,
                        cfg,
                        out_events,
                    );
                    self.stream_b.process_sample(
                        r,
                        self.sample_rate,
                        timing,
                        ch_b,
                        cfg,
                        out_events,
                    );
                }
                StereoRoutingMode::MidSideSplit => {
                    let mid = (l + r) * 0.5;
                    let side = (l - r) * 0.5;
                    self.stream_a.process_sample(
                        mid,
                        self.sample_rate,
                        timing,
                        ch_a,
                        cfg,
                        out_events,
                    );
                    self.stream_b.process_sample(
                        side,
                        self.sample_rate,
                        timing,
                        ch_b,
                        cfg,
                        out_events,
                    );
                }
            }
        }

        self.peak_l = (self.peak_l * 0.82).max(block_max_l.clamp(0.0, 2.0));
        self.peak_r = (self.peak_r * 0.82).max(block_max_r.clamp(0.0, 2.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn test_stereo_engine_emits_note_on_and_note_off() {
        let sr = 48000.0;
        let mut engine = StereoRealtimeMidiEngine::new(sr);
        let cfg = TrackerConfig {
            mode: DetectionMode::MonophonicYin,
            window: AnalysisWindow::Samples1024,
            onset_threshold_db: -35.0,
            stability_hops: 1,
            min_hold_ms: 20.0,
            ..Default::default()
        };

        let mut events = MidiEventBuffer::new();

        // 1. Feed 4096 samples of 440 Hz (A4 = MIDI 69) stereo audio
        let mut buf_l = vec![0.0_f32; 4096];
        let mut buf_r = vec![0.0_f32; 4096];
        for i in 0..4096 {
            let s = 0.4 * (2.0 * PI * 440.0 * (i as f32 / sr)).sin();
            buf_l[i] = s;
            buf_r[i] = s;
        }
        engine.process_stereo_block(&buf_l, &buf_r, &cfg, &mut events);

        let has_a4_on = events.as_slice().iter().any(|e| {
            matches!(
                e,
                MidiOutEvent::NoteOn {
                    channel: 0,
                    note: 69,
                    ..
                }
            )
        });
        assert!(has_a4_on, "Expected NoteOn for MIDI 69 (A4), got {:?}", events.as_slice());

        // 2. Feed silence to trigger NoteOff
        events.clear();
        let silence = vec![0.0_f32; 4096];
        engine.process_stereo_block(&silence, &silence, &cfg, &mut events);

        let has_a4_off = events.as_slice().iter().any(|e| {
            matches!(
                e,
                MidiOutEvent::NoteOff {
                    channel: 0,
                    note: 69,
                    ..
                }
            )
        });
        assert!(has_a4_off, "Expected NoteOff for MIDI 69 (A4), got {:?}", events.as_slice());
    }
}
