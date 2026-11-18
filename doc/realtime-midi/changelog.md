# Changelog — `mtz-realtime-midi` (`src.rs/realtime-midi`)

All notable changes to the **MTZ Realtime MIDI** (`mtz-realtime-midi`) 2-channel FX bus Audio-to-MIDI VST3/CLAP plugin are documented in this file.

## [0.1.0] - 2026-11-18

### Added
- **License**: Added GNU General Public License v3.0 (`GPL-3.0-or-later`) in root `LICENSE`.
- **Core Plugin Infrastructure (`src.rs/realtime-midi`)**:
  - Initial VST3 (`.vst3`), CLAP (`.clap`), and raw dynamic library (`.dll`) plugin target under vendor **`mtz`** (`com.mtz.realtime-midi`).
  - 2-channel Stereo FX Bus Audio Input -> 2-channel Stereo Audio Output (with switchable unity **Audio Pass-Thru** or muted audio output) and sample-accurate **MIDI Event Output** (`NoteOn`, `NoteOff`, `MidiPitchBend`, `MidiCC`).
  - 100% real-time safe audio thread execution (`assert_process_allocs` enabled) with pre-allocated ring buffers, FFT scratch arrays, and fixed-capacity MIDI event queues.
- **Three Selectable Real-Time DSP Detection Modes (`dsp.rs`)**:
  - **Polyphonic FX Bus (Harmonic Sieve)**: Windowed Radix-2 Cooley-Tukey FFT (`1024` or `2048` samples) with parabolic log-magnitude peak interpolation, spectral flatness noise rejection, and iterative harmonic series subtraction ($f_0 \dots 8f_0$) supporting up to 8 simultaneous voices.
  - **Monophonic Lead / Bass (Fast YIN)**: Time-domain Cumulative Mean Normalized Difference Function (CMNDF) pitch estimator with sub-cent parabolic minimum interpolation and clarity confidence metric.
  - **Percussive 3-Band Trigger (Kick / Snare / Hat)**: Fast/slow dual-envelope transient detector across Low (`35–160 Hz` -> `C1` / Note 36), Mid (`160–2200 Hz` -> `D1` / Note 38), and High (`2200–12000 Hz` -> `F#1` / Note 42) bands.
- **2-Channel FX Bus Stereo Routing Modes (`tracker.rs`)**:
  - **Stereo Sum (`L+R -> Base Ch`)**: Sums Left and Right channels into a unified detector on `Base Channel`.
  - **Dual Split (`L -> Ch N, R -> Ch N+1`)**: Runs two independent real-time pitch/note trackers in parallel so Left FX bus audio emits MIDI on `Base Channel` and Right FX bus audio emits MIDI on `Base Channel + 1`.
  - **Mid/Side Split (`Mid -> Ch N, Side -> Ch N+1`)**: Extracts Center/Mid `(L+R)*0.5` to `Base Channel` and Stereo Side `(L-R)*0.5` to `Base Channel + 1`.
- **Musical Note State Machine, Scale Quantizer & Expression (`quantizer.rs`, `tracker.rs`)**:
  - Dual-threshold gate hysteresis (`Onset Gate dB` and `Gate Hysteresis dB`), configurable `Stability Frames`, `Min Note Hold (ms)`, and spectral-flux transient re-triggering.
  - Scale quantization across 9 scales (`Chromatic`, `Major`, `Natural Minor`, `Harmonic Minor`, `Pentatonic Major`, `Pentatonic Minor`, `Dorian`, `Mixolydian`, `Blues`) with `Root Key` (`C`..`B`), `Transpose` (`-24..+24` semitones), `Min/Max Note` bounds, and `A4 Reference` tuning (`415..466 Hz`).
  - Continuous Monophonic Pitch Bend tracking (`±1..24` semitones), RMS Envelope -> MIDI CC (`Off`, `CC 1 Mod Wheel`, `CC 2 Breath`, `CC 11 Expression`), and Spectral Centroid Brightness -> `CC 74`.
- **Interactive Real-Time `egui` Visualizer (`editor.rs`)**:
  - Live FX bus RMS level meter with visual Onset and Release hysteresis threshold markers, Transient Flux bar, and Spectral Brightness meter.
  - Live detected primary note, frequency (Hz), cents deviation, and confidence readout.
  - Rolling 96-frame pitch history timeline and 88-key active MIDI piano roll color-coded by stream (`Stream A` green, `Stream B` orange).
