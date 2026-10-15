//! Standalone DSP modules.
//!
//! Everything here is plain, portable signal processing with no knowledge of
//! the engine, so modules can later be packaged as standalone plugins. Code in
//! this crate runs on the audio thread: it must not allocate, lock or block.

pub mod envelope;
pub mod filter;
pub mod gain;
pub mod oscillator;
pub mod pan;
pub mod smoothing;
pub mod synth;

/// Frequency in Hz of a MIDI note number, with A4 (69) at 440 Hz.
#[inline]
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2.0_f32.powf((note - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_note_frequencies() {
        assert_eq!(midi_to_hz(69.0), 440.0);
        assert!((midi_to_hz(81.0) - 880.0).abs() < 1e-3);
        assert!((midi_to_hz(60.0) - 261.6256).abs() < 1e-3);
    }
}
