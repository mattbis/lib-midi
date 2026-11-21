# MTZ Realtime MIDI (`mtz-realtime-midi`)

**Vendor:** `mtz`  
**Crate Location:** [`src-rs.dev/realtime-midi`](../../src-rs.dev/realtime-midi)  
**Plugin Formats:** VST3 (`mtz-realtime-midi.vst3`), CLAP (`mtz-realtime-midi.clap`), Shared Library (`mtz-realtime-midi.dll`)

`mtz-realtime-midi` is a low-latency, real-time 2-channel FX bus **Audio-to-MIDI** converter plugin written in Rust using `nih-plug` and `nih_plug_egui`. Drop it onto any stereo FX bus, instrument stem, vocal/guitar track, or drum bus to extract live MIDI notes, velocities, pitch bend, and continuous expression CCs in "realish time" (~10–21 ms analysis window with ~5.3 ms hop updates at 48 kHz).

---

## Key Capabilities

1. **Three Real-Time Detection Modes**:
   - **Polyphonic FX Bus (Harmonic Sieve)**: Designed for 2-channel stereo buses carrying chords, keys, pads, guitars, or layered stems. Uses a windowed FFT with parabolic peak interpolation and an iterative harmonic sieve to extract up to **8 simultaneous notes** while suppressing overtone ghost notes.
   - **Monophonic Lead / Bass (Fast YIN)**: Time-domain YIN pitch detector for vocals, bass, guitar solos, and mono synths with sub-cent pitch accuracy and optional continuous **MIDI Pitch Bend** output.
   - **Percussive 3-Band Trigger**: Splits incoming FX bus audio into Low (`Kick -> C1 / 36`), Mid (`Snare -> D1 / 38`), and High (`Hi-Hat -> F#1 / 42`) frequency bands with dual-envelope transient detection.

2. **Flexible 2-Channel FX Bus Stereo Routing**:
   - **Stereo Sum (`L+R -> Base Ch`)**: Sums Left and Right channels before pitch/transient analysis and outputs on `Base Channel`.
   - **Dual Split (`L -> Ch N, R -> Ch N+1`)**: Runs two independent real-time analyzers in parallel—Left channel converts to MIDI on `Base Channel` (`N`), Right channel converts to MIDI on `Base Channel + 1` (`N+1`).
   - **Mid/Side Split (`Mid -> Ch N, Side -> Ch N+1`)**: Converts the Mid/Center image `(L+R)*0.5` to `Base Channel` (`N`) and the Stereo Side image `(L-R)*0.5` to `Base Channel + 1` (`N+1`).

3. **Zero-Glitch Note Tracking & Expression**:
   - Adjustable **Onset Gate (dB)** and **Gate Hysteresis (dB)** prevent note chattering around threshold boundaries.
   - **Stability Frames** and **Min Note Hold (ms)** eliminate 1-frame consonant or transient pitch glitches.
   - **Spectral Flux Re-Trigger** detects repeated attacks on the same pitch.
   - Built-in **Scale Quantizer** (`Chromatic`, `Major`, `Minor`, `Harmonic Minor`, `Pentatonic`, `Dorian`, `Mixolydian`, `Blues`), **Transpose**, and **Note Range Filter**.
   - **Envelope Follower -> MIDI CC** (`CC 1`, `CC 2`, `CC 11`) and **Spectral Centroid Brightness -> CC 74**.

---

## Building & Packaging

From the `lib-midi` workspace root (`i:\lib\usr\lib-midi`):

```powershell
# Run DSP & tracker unit tests
cargo test --workspace

# Bundle optimized VST3 and CLAP plugins (plus standalone .dll)
cargo xtask bundle mtz-realtime-midi --release
```

### Build Outputs
- **VST3 Bundle:** `target/bundled/mtz-realtime-midi.vst3`
- **CLAP Bundle:** `target/bundled/mtz-realtime-midi.clap`
- **Raw Shared Library (`.dll`):** `target/release/mtz_realtime_midi.dll` (and copied to `dist/realtime-midi/mtz-realtime-midi.dll`)

---

## DAW Routing Quick Guide

1. **Insert `mtz-realtime-midi`** on your 2-channel FX bus or audio track.
   - Keep **Audio Pass-Thru** enabled if you want the FX bus audio to continue sounding, or uncheck **Audio Pass-Thru** if the bus is dedicated purely to MIDI extraction.
2. **Route MIDI Output to a Target Instrument Track**:
   - **Reaper**: Open the track routing dialog on the FX bus track and add a **MIDI Send** (`All -> All` or `Ch 1/2`) to your destination synth track (e.g., `DecentRS Sampler`).
   - **Ableton Live / Bitwig**: On a MIDI track with your synth, set **MIDI From** -> select the FX Bus track -> select `mtz-realtime-midi`.
   - **FL Studio**: Open the plugin wrapper settings (gear icon) -> set a **MIDI Output Port** (e.g., Port `10`), and set your target synth's **MIDI Input Port** to `10`.
3. **Tune the Gate**: Watch the live **Bus Level** meter in the plugin UI and position the red **Onset Gate** marker just above the noise floor of your bus.
