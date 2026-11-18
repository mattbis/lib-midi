#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod quantizer;
pub mod tracker;

use dsp::{AnalysisWindow, DetectionMode};
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use parking_lot::Mutex;
use quantizer::{RootKey, ScaleType};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use tracker::{
    EnvelopeCcTarget, MidiEventBuffer, MidiOutEvent, StereoRealtimeMidiEngine, StereoRoutingMode,
    TrackerConfig,
};

pub const HISTORY_FRAMES: usize = 96;

#[derive(Clone, Copy)]
pub struct NoteHistoryCol {
    pub primary_note_a: u8, // 255 = none
    pub primary_note_b: u8, // 255 = none
    pub rms_norm: f32,
}

impl Default for NoteHistoryCol {
    fn default() -> Self {
        Self {
            primary_note_a: 255,
            primary_note_b: 255,
            rms_norm: 0.0,
        }
    }
}

#[derive(Clone)]
pub struct KeyboardSnapshot {
    pub active_a: [u8; 128],
    pub active_b: [u8; 128],
    pub history: [NoteHistoryCol; HISTORY_FRAMES],
    pub history_head: usize,
}

impl Default for KeyboardSnapshot {
    fn default() -> Self {
        Self {
            active_a: [0; 128],
            active_b: [0; 128],
            history: [NoteHistoryCol::default(); HISTORY_FRAMES],
            history_head: 0,
        }
    }
}

pub struct SharedUiTelemetry {
    pub peak_l_bits: AtomicU32,
    pub peak_r_bits: AtomicU32,
    pub rms_db_a_bits: AtomicU32,
    pub rms_db_b_bits: AtomicU32,
    pub flux_a_bits: AtomicU32,
    pub centroid_a_bits: AtomicU32,
    pub primary_note_a: AtomicU8,
    pub primary_freq_a_bits: AtomicU32,
    pub primary_cents_a_bits: AtomicU32,
    pub primary_conf_a_bits: AtomicU32,
    pub sample_rate_bits: AtomicU32,
    pub hop_size: AtomicUsize,
    pub active_voices: AtomicUsize,
    pub total_notes_emitted: AtomicUsize,
    pub panic_requested: AtomicBool,
    pub keyboard: Mutex<KeyboardSnapshot>,
}

impl Default for SharedUiTelemetry {
    fn default() -> Self {
        Self {
            peak_l_bits: AtomicU32::new(0.0_f32.to_bits()),
            peak_r_bits: AtomicU32::new(0.0_f32.to_bits()),
            rms_db_a_bits: AtomicU32::new((-100.0_f32).to_bits()),
            rms_db_b_bits: AtomicU32::new((-100.0_f32).to_bits()),
            flux_a_bits: AtomicU32::new(0.0_f32.to_bits()),
            centroid_a_bits: AtomicU32::new(0.0_f32.to_bits()),
            primary_note_a: AtomicU8::new(255),
            primary_freq_a_bits: AtomicU32::new(0.0_f32.to_bits()),
            primary_cents_a_bits: AtomicU32::new(0.0_f32.to_bits()),
            primary_conf_a_bits: AtomicU32::new(0.0_f32.to_bits()),
            sample_rate_bits: AtomicU32::new(48000.0_f32.to_bits()),
            hop_size: AtomicUsize::new(256),
            active_voices: AtomicUsize::new(0),
            total_notes_emitted: AtomicUsize::new(0),
            panic_requested: AtomicBool::new(false),
            keyboard: Mutex::new(KeyboardSnapshot::default()),
        }
    }
}

#[derive(Params)]
pub struct MtzRealtimeMidiParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    // --- Detection & Routing ---
    #[id = "det_mode"]
    pub detection_mode: EnumParam<DetectionMode>,

    #[id = "win_size"]
    pub window_size: EnumParam<AnalysisWindow>,

    #[id = "st_route"]
    pub stereo_routing: EnumParam<StereoRoutingMode>,

    #[id = "input_gain"]
    pub input_gain_db: FloatParam,

    #[id = "audio_thru"]
    pub audio_thru: BoolParam,

    // --- Gate & Tracking ---
    #[id = "onset_db"]
    pub onset_threshold_db: FloatParam,

    #[id = "hyst_db"]
    pub release_hysteresis_db: FloatParam,

    #[id = "min_conf"]
    pub min_confidence: FloatParam,

    #[id = "min_hold"]
    pub min_hold_ms: FloatParam,

    #[id = "stab_hops"]
    pub stability_hops: IntParam,

    #[id = "retrig_flux"]
    pub retrigger_flux: FloatParam,

    #[id = "max_poly"]
    pub max_polyphony: IntParam,

    // --- Pitch, Scale & Range ---
    #[id = "root_key"]
    pub root_key: EnumParam<RootKey>,

    #[id = "scale_type"]
    pub scale_type: EnumParam<ScaleType>,

    #[id = "transpose"]
    pub transpose_semitones: IntParam,

    #[id = "min_note"]
    pub min_note: IntParam,

    #[id = "max_note"]
    pub max_note: IntParam,

    #[id = "a4_tuning"]
    pub a4_tuning_hz: FloatParam,

    // --- MIDI Output & Expression ---
    #[id = "midi_ch"]
    pub midi_channel: IntParam,

    #[id = "vel_sens"]
    pub velocity_sensitivity: FloatParam,

    #[id = "min_vel"]
    pub min_velocity: IntParam,

    #[id = "pb_enable"]
    pub pitch_bend_enabled: BoolParam,

    #[id = "pb_range"]
    pub pitch_bend_range: FloatParam,

    #[id = "env_cc"]
    pub env_cc_target: EnumParam<EnvelopeCcTarget>,

    #[id = "bright_cc74"]
    pub brightness_cc74: BoolParam,
}

impl Default for MtzRealtimeMidiParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),

            detection_mode: EnumParam::new("Detection Mode", DetectionMode::PolyphonicBus),
            window_size: EnumParam::new("Window Size", AnalysisWindow::Samples2048),
            stereo_routing: EnumParam::new("FX Bus Stereo Routing", StereoRoutingMode::StereoSum),
            input_gain_db: FloatParam::new(
                "Input Trim",
                0.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 24.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.1),
            audio_thru: BoolParam::new("Audio Pass-Thru", true),

            onset_threshold_db: FloatParam::new(
                "Onset Gate",
                -38.0,
                FloatRange::Linear {
                    min: -72.0,
                    max: -6.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.5),
            release_hysteresis_db: FloatParam::new(
                "Gate Hysteresis",
                6.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 18.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.5),
            min_confidence: FloatParam::new(
                "Pitch Clarity",
                0.50,
                FloatRange::Linear {
                    min: 0.15,
                    max: 0.92,
                },
            )
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage())
            .with_step_size(0.01),
            min_hold_ms: FloatParam::new(
                "Min Note Hold",
                45.0,
                FloatRange::Linear {
                    min: 10.0,
                    max: 250.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(1.0),
            stability_hops: IntParam::new(
                "Stability Frames",
                2,
                IntRange::Linear { min: 1, max: 5 },
            ),
            retrigger_flux: FloatParam::new(
                "Transient Re-Trigger",
                0.55,
                FloatRange::Linear {
                    min: 0.15,
                    max: 1.0,
                },
            )
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage())
            .with_step_size(0.01),
            max_polyphony: IntParam::new(
                "Max Polyphony",
                4,
                IntRange::Linear { min: 1, max: 8 },
            ),

            root_key: EnumParam::new("Scale Root", RootKey::C),
            scale_type: EnumParam::new("Scale Quantize", ScaleType::Chromatic),
            transpose_semitones: IntParam::new(
                "Transpose",
                0,
                IntRange::Linear { min: -24, max: 24 },
            )
            .with_unit(" st"),
            min_note: IntParam::new("Min Note", 24, IntRange::Linear { min: 21, max: 96 }),
            max_note: IntParam::new("Max Note", 108, IntRange::Linear { min: 36, max: 108 }),
            a4_tuning_hz: FloatParam::new(
                "A4 Reference",
                440.0,
                FloatRange::Linear {
                    min: 415.0,
                    max: 466.0,
                },
            )
            .with_unit(" Hz")
            .with_step_size(0.1),

            midi_channel: IntParam::new("MIDI Channel", 1, IntRange::Linear { min: 1, max: 16 }),
            velocity_sensitivity: FloatParam::new(
                "Velocity Sens",
                1.0,
                FloatRange::Linear {
                    min: 0.25,
                    max: 2.25,
                },
            )
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage())
            .with_step_size(0.01),
            min_velocity: IntParam::new("Min Velocity", 20, IntRange::Linear { min: 1, max: 120 }),
            pitch_bend_enabled: BoolParam::new("Pitch Bend Tracking", false),
            pitch_bend_range: FloatParam::new(
                "Pitch Bend Range",
                2.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 24.0,
                },
            )
            .with_unit(" st")
            .with_step_size(1.0),
            env_cc_target: EnumParam::new("Envelope -> CC", EnvelopeCcTarget::Off),
            brightness_cc74: BoolParam::new("Brightness -> CC 74", false),
        }
    }
}

pub struct MtzRealtimeMidiPlugin {
    params: Arc<MtzRealtimeMidiParams>,
    engine: StereoRealtimeMidiEngine,
    event_buf: MidiEventBuffer,
    telemetry: Arc<SharedUiTelemetry>,
    ui_frame_divider: usize,
}

impl Default for MtzRealtimeMidiPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(MtzRealtimeMidiParams::default()),
            engine: StereoRealtimeMidiEngine::new(48000.0),
            event_buf: MidiEventBuffer::new(),
            telemetry: Arc::new(SharedUiTelemetry::default()),
            ui_frame_divider: 0,
        }
    }
}

impl Plugin for MtzRealtimeMidiPlugin {
    const NAME: &'static str = "mtz-realtime-midi";
    const VENDOR: &'static str = "mtz";
    const URL: &'static str = "https://github.com/mtz/lib-midi";
    const EMAIL: &'static str = "info@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // 2-Channel Stereo FX Bus Input -> 2-Channel Stereo Output (Pass-thru or muted)
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[],
        aux_output_ports: &[],
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.telemetry.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.engine.set_sample_rate(buffer_config.sample_rate);
        self.telemetry
            .sample_rate_bits
            .store(buffer_config.sample_rate.to_bits(), Ordering::Relaxed);
        self.telemetry
            .hop_size
            .store(self.engine.stream_a.hop_size(), Ordering::Relaxed);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.event_buf.clear();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_frames = buffer.samples();
        if num_frames == 0 {
            return ProcessStatus::Normal;
        }

        // Pass through any incoming MIDI events if the user chained MIDI into the FX track
        while let Some(in_event) = context.next_event() {
            context.send_event(in_event);
        }

        self.event_buf.clear();

        // Check if GUI requested MIDI Panic (All Notes Off)
        if self
            .telemetry
            .panic_requested
            .swap(false, Ordering::AcqRel)
        {
            self.engine.panic_all_notes_off(&mut self.event_buf);
        }

        let input_gain_db = self.params.input_gain_db.value();
        let input_gain_lin = util::db_to_gain(input_gain_db);
        let audio_thru = self.params.audio_thru.value();

        let cfg = TrackerConfig {
            mode: self.params.detection_mode.value(),
            window: self.params.window_size.value(),
            routing: self.params.stereo_routing.value(),
            base_channel: (self.params.midi_channel.value().clamp(1, 16) - 1) as u8,
            max_polyphony: self.params.max_polyphony.value().clamp(1, 8) as usize,
            onset_threshold_db: self.params.onset_threshold_db.value(),
            release_hysteresis_db: self.params.release_hysteresis_db.value(),
            min_confidence: self.params.min_confidence.value(),
            min_hold_ms: self.params.min_hold_ms.value(),
            stability_hops: self.params.stability_hops.value().clamp(1, 5) as u8,
            retrigger_flux_thresh: self.params.retrigger_flux.value(),
            velocity_sensitivity: self.params.velocity_sensitivity.value(),
            min_velocity: self.params.min_velocity.value().clamp(1, 126) as u8,
            root_key: self.params.root_key.value(),
            scale_type: self.params.scale_type.value(),
            transpose_semitones: self.params.transpose_semitones.value(),
            min_note: self.params.min_note.value().clamp(0, 127) as u8,
            max_note: self.params.max_note.value().clamp(0, 127) as u8,
            pitch_bend_enabled: self.params.pitch_bend_enabled.value(),
            pitch_bend_range: self.params.pitch_bend_range.value(),
            env_cc_target: self.params.env_cc_target.value(),
            brightness_cc74: self.params.brightness_cc74.value(),
            a4_hz: self.params.a4_tuning_hz.value(),
        };

        let channels = buffer.as_slice();
        if channels.len() >= 2 {
            let (first_ch, rest_ch) = channels.split_at_mut(1);
            let ch_l = &mut first_ch[0][..num_frames];
            let ch_r = &mut rest_ch[0][..num_frames];

            // Apply input trim for analysis if non-unity; keep or mute output according to `audio_thru`
            if (input_gain_lin - 1.0).abs() > 1e-4 {
                for s in ch_l.iter_mut() {
                    *s *= input_gain_lin;
                }
                for s in ch_r.iter_mut() {
                    *s *= input_gain_lin;
                }
            }

            self.engine
                .process_stereo_block(ch_l, ch_r, &cfg, &mut self.event_buf);

            if !audio_thru {
                ch_l.fill(0.0);
                ch_r.fill(0.0);
            } else if (input_gain_lin - 1.0).abs() > 1e-4 && input_gain_lin > 1e-6 {
                // Restore original dry FX bus level on pass-thru so Input Trim only affects detection
                let inv_gain = 1.0 / input_gain_lin;
                for s in ch_l.iter_mut() {
                    *s *= inv_gain;
                }
                for s in ch_r.iter_mut() {
                    *s *= inv_gain;
                }
            }
        }

        // Dispatch generated MIDI events to the host DAW
        let mut emitted_notes = 0usize;
        for &ev in self.event_buf.as_slice() {
            match ev {
                MidiOutEvent::NoteOn {
                    timing,
                    channel,
                    note,
                    velocity,
                } => {
                    emitted_notes += 1;
                    context.send_event(NoteEvent::NoteOn {
                        timing,
                        voice_id: None,
                        channel,
                        note,
                        velocity,
                    });
                }
                MidiOutEvent::NoteOff {
                    timing,
                    channel,
                    note,
                    velocity,
                } => {
                    context.send_event(NoteEvent::NoteOff {
                        timing,
                        voice_id: None,
                        channel,
                        note,
                        velocity,
                    });
                }
                MidiOutEvent::PitchBend {
                    timing,
                    channel,
                    value,
                } => {
                    context.send_event(NoteEvent::MidiPitchBend {
                        timing,
                        channel,
                        value,
                    });
                }
                MidiOutEvent::ControlChange {
                    timing,
                    channel,
                    cc,
                    value,
                } => {
                    context.send_event(NoteEvent::MidiCC {
                        timing,
                        channel,
                        cc,
                        value,
                    });
                }
            }
        }

        if emitted_notes > 0 {
            self.telemetry
                .total_notes_emitted
                .fetch_add(emitted_notes, Ordering::Relaxed);
        }

        // Update lock-free UI telemetry
        self.telemetry
            .peak_l_bits
            .store(self.engine.peak_l.to_bits(), Ordering::Relaxed);
        self.telemetry
            .peak_r_bits
            .store(self.engine.peak_r.to_bits(), Ordering::Relaxed);
        self.telemetry.rms_db_a_bits.store(
            self.engine.stream_a.last_analysis.rms_db.to_bits(),
            Ordering::Relaxed,
        );
        self.telemetry.rms_db_b_bits.store(
            self.engine.stream_b.last_analysis.rms_db.to_bits(),
            Ordering::Relaxed,
        );
        self.telemetry.flux_a_bits.store(
            self.engine.stream_a.last_analysis.spectral_flux.to_bits(),
            Ordering::Relaxed,
        );
        self.telemetry.centroid_a_bits.store(
            self.engine
                .stream_a
                .last_analysis
                .spectral_centroid_norm
                .to_bits(),
            Ordering::Relaxed,
        );
        self.telemetry.primary_note_a.store(
            self.engine.stream_a.primary_detected_note.unwrap_or(255),
            Ordering::Relaxed,
        );
        self.telemetry.primary_freq_a_bits.store(
            self.engine.stream_a.primary_detected_freq.to_bits(),
            Ordering::Relaxed,
        );
        self.telemetry.primary_cents_a_bits.store(
            self.engine.stream_a.primary_detected_cents.to_bits(),
            Ordering::Relaxed,
        );
        self.telemetry.primary_conf_a_bits.store(
            self.engine.stream_a.primary_confidence.to_bits(),
            Ordering::Relaxed,
        );
        let active_total =
            self.engine.stream_a.active_voice_count() + self.engine.stream_b.active_voice_count();
        self.telemetry
            .active_voices
            .store(active_total, Ordering::Relaxed);

        self.ui_frame_divider = self.ui_frame_divider.wrapping_add(num_frames);
        if self.ui_frame_divider >= 512 {
            self.ui_frame_divider = 0;
            if let Some(mut kb) = self.telemetry.keyboard.try_lock() {
                kb.active_a = self.engine.stream_a.active_note_mask();
                kb.active_b = self.engine.stream_b.active_note_mask();
                let head = kb.history_head;
                let rms_norm = ((self.engine.stream_a.last_analysis.rms_db + 72.0) / 72.0)
                    .clamp(0.0, 1.0);
                kb.history[head] = NoteHistoryCol {
                    primary_note_a: self.engine.stream_a.primary_detected_note.unwrap_or(255),
                    primary_note_b: self.engine.stream_b.primary_detected_note.unwrap_or(255),
                    rms_norm,
                };
                kb.history_head = (head + 1) % HISTORY_FRAMES;
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for MtzRealtimeMidiPlugin {
    const CLAP_ID: &'static str = "com.mtz.realtime-midi";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Real-time 2-channel FX bus Audio-to-MIDI converter plugin");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::NoteEffect,
        ClapFeature::Analyzer,
        ClapFeature::Utility,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for MtzRealtimeMidiPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"MtzRealtimeMidi!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Instrument,
        Vst3SubCategory::Analyzer,
        Vst3SubCategory::Stereo,
    ];
}

nih_export_clap!(MtzRealtimeMidiPlugin);
nih_export_vst3!(MtzRealtimeMidiPlugin);
