# `lib-midi`

Multi-tool MIDI DSP, conversion, and plugin workspace by **mtz**.

## Repository Layout

- **[`src.rs/`](src.rs)**: Rust workspace crates and plugins.
  - **[`src.rs/realtime-midi`](src.rs/realtime-midi)**: **`mtz-realtime-midi`** — Real-time 2-channel FX bus Audio-to-MIDI VST3, CLAP, and `.dll` converter plugin.
  - **[`src.rs/xtask`](src.rs/xtask)**: `nih_plug_xtask` bundler utility for building VST3/CLAP plugin packages.
- **[`doc/`](doc)**: Per-tool documentation, architecture notes, and changelogs.
  - **[`doc/realtime-midi/README.md`](doc/realtime-midi/README.md)**: User guide, DAW routing instructions, and build reference for `mtz-realtime-midi`.
  - **[`doc/realtime-midi/architecture.md`](doc/realtime-midi/architecture.md)**: Real-time DSP pipeline, YIN/Harmonic Sieve algorithms, and latency breakdown.
  - **[`doc/realtime-midi/changelog.md`](doc/realtime-midi/changelog.md)**: Version history for `mtz-realtime-midi`.

## Quick Build Commands

```powershell
# Run all unit tests
cargo test --workspace

# Bundle mtz-realtime-midi (.vst3 and .clap)
cargo xtask bundle mtz-realtime-midi --release
```
