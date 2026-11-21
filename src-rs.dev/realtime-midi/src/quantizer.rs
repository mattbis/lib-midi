use nih_plug::prelude::Enum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum RootKey {
    #[name = "C"]
    C = 0,
    #[name = "C# / Db"]
    Cs = 1,
    #[name = "D"]
    D = 2,
    #[name = "D# / Eb"]
    Ds = 3,
    #[name = "E"]
    E = 4,
    #[name = "F"]
    F = 5,
    #[name = "F# / Gb"]
    Fs = 6,
    #[name = "G"]
    G = 7,
    #[name = "G# / Ab"]
    Gs = 8,
    #[name = "A"]
    A = 9,
    #[name = "A# / Bb"]
    As = 10,
    #[name = "B"]
    B = 11,
}

impl RootKey {
    #[inline]
    pub fn semitone_offset(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ScaleType {
    #[name = "Chromatic (12-TET)"]
    Chromatic,
    #[name = "Major (Ionian)"]
    Major,
    #[name = "Minor (Aeolian)"]
    NaturalMinor,
    #[name = "Harmonic Minor"]
    HarmonicMinor,
    #[name = "Pentatonic Major"]
    PentatonicMajor,
    #[name = "Pentatonic Minor"]
    PentatonicMinor,
    #[name = "Dorian"]
    Dorian,
    #[name = "Mixolydian"]
    Mixolydian,
    #[name = "Blues"]
    Blues,
}

impl ScaleType {
    /// Returns a 12-bit mask where bit `i` (0..12) is 1 if interval `i` semitones above root is in the scale.
    #[inline]
    pub const fn interval_mask(self) -> u16 {
        match self {
            ScaleType::Chromatic => 0b1111_1111_1111,
            // 0, 2, 4, 5, 7, 9, 11
            ScaleType::Major => (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 11),
            // 0, 2, 3, 5, 7, 8, 10
            ScaleType::NaturalMinor => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 8) | (1 << 10),
            // 0, 2, 3, 5, 7, 8, 11
            ScaleType::HarmonicMinor => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 8) | (1 << 11),
            // 0, 2, 4, 7, 9
            ScaleType::PentatonicMajor => (1 << 0) | (1 << 2) | (1 << 4) | (1 << 7) | (1 << 9),
            // 0, 3, 5, 7, 10
            ScaleType::PentatonicMinor => (1 << 0) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 10),
            // 0, 2, 3, 5, 7, 9, 10
            ScaleType::Dorian => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 10),
            // 0, 2, 4, 5, 7, 9, 10
            ScaleType::Mixolydian => (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 10),
            // 0, 3, 5, 6, 7, 10
            ScaleType::Blues => (1 << 0) | (1 << 3) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 10),
        }
    }
}

/// Converts a frequency in Hz to fractional MIDI note number (`69.0` = `a4_hz`, typically `440.0` Hz).
#[inline]
pub fn freq_to_midi_float(freq_hz: f32, a4_hz: f32) -> f32 {
    if freq_hz <= 8.0 || a4_hz <= 0.0 {
        return 0.0;
    }
    69.0 + 12.0 * (freq_hz / a4_hz).log2()
}

/// Converts a MIDI note number (`0..=127`) to frequency in Hz.
#[inline]
pub fn midi_to_freq_hz(note: u8, a4_hz: f32) -> f32 {
    a4_hz * 2.0_f32.powf((note as f32 - 69.0) / 12.0)
}

/// Quantizes a fractional MIDI note to the nearest valid note in `(root, scale)`,
/// applies semitone transposition, and returns `(quantized_note, cents_deviation)`
/// if the resulting note falls within `[min_note, max_note]`.
#[inline]
pub fn quantize_pitch(
    raw_midi_float: f32,
    transpose_semitones: i32,
    root: RootKey,
    scale: ScaleType,
    min_note: u8,
    max_note: u8,
) -> Option<(u8, f32)> {
    let shifted = raw_midi_float + transpose_semitones as f32;
    if !(12.0..=120.0).contains(&shifted) {
        return None;
    }

    let mask = scale.interval_mask();
    let root_semi = root.semitone_offset();

    // Search nearby integer MIDI notes around `shifted` to find the closest in-scale note
    let center = shifted.round() as i32;
    let mut best_note = center;
    let mut best_dist = f32::MAX;

    for candidate in (center - 3)..=(center + 3) {
        if !(0..=127).contains(&candidate) {
            continue;
        }
        let rel_pc = (candidate - root_semi).rem_euclid(12);
        if (mask & (1 << rel_pc)) != 0 {
            let dist = (shifted - candidate as f32).abs();
            if dist < best_dist {
                best_dist = dist;
                best_note = candidate;
            }
        }
    }

    let note_u8 = best_note.clamp(0, 127) as u8;
    let lo = min_note.min(max_note);
    let hi = min_note.max(max_note);
    if note_u8 < lo || note_u8 > hi {
        return None;
    }

    let cents_deviation = (shifted - note_u8 as f32) * 100.0;
    Some((note_u8, cents_deviation))
}

/// Formats a MIDI note number (`0..=127`) as standard scientific pitch notation (e.g. `60` -> `"C4"`, `69` -> `"A4"`).
pub fn midi_note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let pc = (note % 12) as usize;
    let octave = (note as i32 / 12) - 1;
    format!("{}{}", NAMES[pc], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freq_midi_roundtrip() {
        let m = freq_to_midi_float(440.0, 440.0);
        assert!((m - 69.0).abs() < 1e-5);
        let f = midi_to_freq_hz(60, 440.0);
        assert!((f - 261.62555).abs() < 1e-3);
    }

    #[test]
    fn test_scale_quantizer() {
        // 61.1 (C#4) in C Major should snap to either C4 (60) or D4 (62)
        // Since 61.1 is closer to 62 (dist 0.9) than 60 (dist 1.1), it snaps to 62 (D4)
        let (note, cents) =
            quantize_pitch(61.1, 0, RootKey::C, ScaleType::Major, 21, 108).unwrap();
        assert_eq!(note, 62);
        assert!((cents - (-90.0)).abs() < 1e-3);

        // In Chromatic, 61.1 snaps to 61 (C#4)
        let (note_chr, cents_chr) =
            quantize_pitch(61.1, 0, RootKey::C, ScaleType::Chromatic, 21, 108).unwrap();
        assert_eq!(note_chr, 61);
        assert!((cents_chr - 10.0).abs() < 1e-3);
    }
}
