//! Compiled motion signals: centred shapes over song time that listening
//! layers read.
//!
//! A signal gives a value and a presence at every song position. Values use
//! the same space as block shapes: `0.5` is the centre line (no effect), `1`
//! the top (+1) and `0` the bottom (-1). Presence is 0 where a drawn signal
//! has a gap, so a listening layer has no effect there, just like a gap in a
//! layer.

use crate::stack::CompiledBlock;
use daw_protocol::{NotesSource, Signal, SignalId, SignalSource};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum CompiledSignal {
    /// Says nothing anywhere: what a layer listening to a missing signal
    /// reads.
    Silent,
    /// Drawn blocks, sorted by start, non-overlapping.
    Drawn(Vec<CompiledBlock>),
    /// ADSR hits from notes. Always present; rests on the centre line.
    Notes(NotesEnvelope),
}

/// A note as a notes signal sees it: start and end in beats, pitch and
/// velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignalNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub velocity: u8,
}

/// Every hit of a notes signal and the envelope that shapes them, with times
/// in beats.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesEnvelope {
    /// Sorted by start, one per start.
    hits: Vec<Hit>,
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Hit {
    start: f64,
    end: f64,
    /// Highest level of the hit, `0..=1`.
    peak: f64,
    /// Level the hit starts from: where the previous hit had got to.
    from: f64,
}

impl NotesEnvelope {
    fn compile(source: &NotesSource, notes: impl IntoIterator<Item = SignalNote>) -> Self {
        let mut envelope = Self {
            hits: Vec::new(),
            attack: source.attack.as_beats(),
            decay: source.decay.as_beats(),
            sustain: source.sustain,
            release: source.release.as_beats(),
        };
        let mut notes: Vec<SignalNote> = notes
            .into_iter()
            .filter(|n| !source.pitch_range || (source.lowest..=source.highest).contains(&n.pitch))
            .collect();
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        for note in notes {
            let velocity = f64::from(note.velocity) / 127.0;
            let peak = if source.velocity_clamp {
                velocity.clamp(source.velocity_min, source.velocity_max)
            } else {
                velocity
            };
            let hit = Hit {
                start: note.start,
                end: note.end,
                peak,
                from: 0.0,
            };
            match envelope.hits.last_mut() {
                // Notes starting together are one hit: the strongest, held
                // for the longest.
                Some(last) if last.start == note.start => {
                    last.peak = last.peak.max(hit.peak);
                    last.end = last.end.max(hit.end);
                }
                _ => envelope.hits.push(hit),
            }
        }
        for i in 1..envelope.hits.len() {
            let (previous, start) = (envelope.hits[i - 1], envelope.hits[i].start);
            envelope.hits[i].from = envelope.level(&previous, start);
        }
        envelope
    }

    /// The level (`0..=1`) of `hit` at `beat`, which is at or after its start.
    fn level(&self, hit: &Hit, beat: f64) -> f64 {
        let held = |t: f64| {
            let sustain = self.sustain * hit.peak;
            if t < self.attack {
                hit.from + (hit.peak - hit.from) * t / self.attack
            } else if t < self.attack + self.decay {
                hit.peak + (sustain - hit.peak) * (t - self.attack) / self.decay
            } else {
                sustain
            }
        };
        let (since, length) = (beat - hit.start, hit.end - hit.start);
        if since < length {
            return held(since);
        }
        let released = since - length;
        if released < self.release {
            held(length) * (1.0 - released / self.release)
        } else {
            0.0
        }
    }

    /// The level (`0..=1`) at `beat`. Allocation-free.
    fn sample(&self, beat: f64) -> f64 {
        let after = self.hits.partition_point(|h| h.start <= beat);
        after
            .checked_sub(1)
            .map_or(0.0, |i| self.level(&self.hits[i], beat))
    }
}

impl CompiledSignal {
    /// Compiles `signal`. `notes` are every note on the source node, in any
    /// order; only notes signals use them.
    pub fn compile(signal: &Signal, notes: impl IntoIterator<Item = SignalNote>) -> Self {
        match &signal.source {
            SignalSource::Drawn(blocks) => {
                let mut blocks: Vec<CompiledBlock> = blocks
                    .iter()
                    .map(|(_, block)| CompiledBlock::compile(block))
                    .collect();
                blocks.sort_by(|a, b| a.start.total_cmp(&b.start));
                Self::Drawn(blocks)
            }
            SignalSource::Notes(source) => Self::Notes(NotesEnvelope::compile(source, notes)),
        }
    }

    /// Value (`0.5` is the centre line) and presence at `beat`.
    /// Allocation-free.
    pub fn sample(&self, beat: f64) -> (f64, f64) {
        const SILENT: (f64, f64) = (0.5, 0.0);
        match self {
            Self::Silent => SILENT,
            Self::Drawn(blocks) => {
                let after = blocks.partition_point(|b| b.start <= beat);
                after
                    .checked_sub(1)
                    .map(|i| &blocks[i])
                    .filter(|b| b.contains(beat))
                    .map_or(SILENT, |b| b.sample(beat, None))
            }
            // A hit goes from the centre line (0) up to the top (+1).
            Self::Notes(envelope) => (0.5 + 0.5 * envelope.sample(beat), 1.0),
        }
    }
}

/// Compiled signals by id, for compiling the layers that listen to them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Signals(BTreeMap<SignalId, CompiledSignal>);

impl Signals {
    pub fn insert(&mut self, id: SignalId, signal: CompiledSignal) {
        self.0.insert(id, signal);
    }

    pub fn get(&self, id: SignalId) -> Option<&CompiledSignal> {
        self.0.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Automation;
    use daw_protocol::{BlendMode, Block, BlockId, Layer, LayerId, Shape, Ticks};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn beats(n: f64) -> Ticks {
        Ticks::from_beats_f64(n)
    }

    const KICK: u8 = 36;

    /// A note of `length` beats at full velocity.
    fn note(start: f64, length: f64, pitch: u8) -> SignalNote {
        SignalNote {
            start,
            end: start + length,
            pitch,
            velocity: 127,
        }
    }

    fn kick(start: f64) -> SignalNote {
        note(start, 0.1, KICK)
    }

    /// Kicks only, with the given attack, decay, sustain and release (beats).
    fn envelope(attack: f64, decay: f64, sustain: f64, release: f64) -> NotesSource {
        NotesSource {
            pitch_range: true,
            velocity_clamp: true,
            lowest: KICK,
            highest: KICK,
            attack: beats(attack),
            decay: beats(decay),
            sustain,
            release: beats(release),
            velocity_min: 1.0,
            velocity_max: 1.0,
        }
    }

    fn notes_signal(source: NotesSource) -> Signal {
        Signal {
            name: "Kick".to_owned(),
            color: 0,
            source: SignalSource::Notes(source),
        }
    }

    /// The hit level (`0..=1`) and presence of a notes signal at `beat`.
    fn level(source: NotesSource, notes: &[SignalNote], beat: f64) -> (f64, f64) {
        let (x, w) =
            CompiledSignal::compile(&notes_signal(source), notes.iter().copied()).sample(beat);
        (2.0 * x - 1.0, w)
    }

    fn drawn(blocks: Vec<Block>) -> Signal {
        Signal {
            name: "Drawn".to_owned(),
            color: 0,
            source: SignalSource::Drawn(
                blocks
                    .into_iter()
                    .enumerate()
                    .map(|(i, b)| (BlockId(i as u64), b))
                    .collect(),
            ),
        }
    }

    #[test]
    fn notes_signals_rest_on_the_centre_line_and_are_always_present() {
        let pluck = envelope(0.0, 0.5, 0.0, 0.0);
        assert_eq!(level(pluck.clone(), &[kick(1.0)], 0.5), (0.0, 1.0));
        assert_eq!(level(pluck, &[kick(1.0)], 9.0), (0.0, 1.0));
    }

    #[test]
    fn each_hit_rises_by_attack_and_falls_by_decay() {
        let source = envelope(0.5, 1.0, 0.0, 0.0);
        let hits = [note(0.0, 4.0, KICK)];
        assert!(close(level(source.clone(), &hits, 0.25).0, 0.5), "attack");
        assert!(close(level(source.clone(), &hits, 0.5).0, 1.0), "peak");
        assert!(close(level(source.clone(), &hits, 1.0).0, 0.5), "decay");
        assert!(close(level(source, &hits, 2.0).0, 0.0), "sustain 0");
    }

    #[test]
    fn zero_attack_jumps_straight_to_the_peak() {
        let source = envelope(0.0, 1.0, 0.0, 0.0);
        assert!(close(level(source, &[kick(2.0)], 2.0).0, 1.0));
    }

    #[test]
    fn sustain_holds_while_the_note_is_held_then_release_falls_to_zero() {
        let source = envelope(0.0, 0.5, 0.5, 1.0);
        let held = [note(0.0, 2.0, KICK)];
        assert!(close(level(source.clone(), &held, 1.9).0, 0.5), "held");
        assert!(
            close(level(source.clone(), &held, 2.5).0, 0.25),
            "releasing"
        );
        assert!(close(level(source, &held, 3.5).0, 0.0), "released");
    }

    #[test]
    fn a_short_note_releases_from_wherever_it_got_to() {
        // A one-beat attack cut off by the note ending a quarter of the way.
        let source = envelope(1.0, 0.0, 1.0, 1.0);
        let short = [note(0.0, 0.25, KICK)];
        assert!(close(level(source.clone(), &short, 0.25).0, 0.25));
        assert!(close(level(source, &short, 0.75).0, 0.125));
    }

    #[test]
    fn a_new_note_starts_from_wherever_the_line_is() {
        let source = envelope(0.5, 0.0, 1.0, 1.0);
        let notes = [note(0.0, 0.25, KICK), note(0.5, 1.0, KICK)];
        // The first hit reached 0.5, then released for a quarter of a beat.
        let at_second = 0.5 * (1.0 - 0.25);
        assert!(
            close(level(source.clone(), &notes, 0.5).0, at_second),
            "no jump"
        );
        let halfway = at_second + (1.0 - at_second) * 0.5;
        assert!(close(level(source, &notes, 0.75).0, halfway));
    }

    #[test]
    fn velocity_sets_the_peak_within_the_clamp() {
        let at = |velocity: u8| SignalNote {
            velocity,
            ..kick(0.0)
        };
        let clamped = |min: f64, max: f64, velocity: u8| {
            let source = NotesSource {
                velocity_min: min,
                velocity_max: max,
                ..envelope(0.0, 1.0, 0.0, 0.0)
            };
            level(source, &[at(velocity)], 0.0).0
        };
        assert!(
            close(clamped(0.0, 1.0, 64), 64.0 / 127.0),
            "follows velocity"
        );
        assert!(close(clamped(0.0, 1.0, 127), 1.0));
        assert!(close(clamped(0.6, 1.0, 64), 0.6), "raised to the minimum");
        assert!(close(clamped(0.0, 0.3, 127), 0.3), "held to the maximum");
        assert!(close(clamped(1.0, 1.0, 1), 1.0), "every hit full");
    }

    #[test]
    fn without_the_clamp_hits_follow_velocity() {
        let soft = SignalNote {
            velocity: 64,
            ..kick(0.0)
        };
        let source = NotesSource {
            velocity_clamp: false,
            velocity_min: 1.0,
            velocity_max: 1.0,
            ..envelope(0.0, 1.0, 0.0, 0.0)
        };
        assert!(close(level(source, &[soft], 0.0).0, 64.0 / 127.0));
    }

    #[test]
    fn without_the_pitch_range_every_note_hits() {
        let source = NotesSource {
            pitch_range: false,
            ..envelope(0.0, 1.0, 0.0, 0.0)
        };
        let snare = [note(0.0, 1.0, 60)];
        assert!(close(level(source, &snare, 0.0).0, 1.0));
    }

    #[test]
    fn notes_outside_the_pitch_range_are_ignored() {
        let source = envelope(0.0, 1.0, 0.0, 0.0);
        let notes = [note(0.0, 1.0, KICK), note(0.25, 0.1, 60)];
        assert!(
            close(level(source, &notes, 0.5).0, 0.5),
            "the snare did not hit"
        );
    }

    #[test]
    fn note_order_does_not_matter() {
        let signal = notes_signal(envelope(0.1, 0.2, 0.3, 0.4));
        let a = CompiledSignal::compile(&signal, [kick(2.0), kick(0.0)]);
        let b = CompiledSignal::compile(&signal, [kick(0.0), kick(2.0)]);
        assert_eq!(a, b);
    }

    #[test]
    fn notes_starting_together_are_one_hit() {
        let source = NotesSource {
            lowest: 0,
            highest: 127,
            ..envelope(0.0, 0.0, 1.0, 0.0)
        };
        let chord = [note(0.0, 1.0, 60), note(0.0, 2.0, 64)];
        assert!(
            close(level(source, &chord, 1.5).0, 1.0),
            "held for the longest"
        );
    }

    #[test]
    fn drawn_signals_say_nothing_in_their_gaps() {
        let block = Block::new(beats(1.0), beats(1.0), Shape::constant(0.7));
        let s = CompiledSignal::compile(&drawn(vec![block]), []);
        assert_eq!(s.sample(0.5).1, 0.0);
        assert_eq!(s.sample(1.5), (0.7, 1.0));
        assert_eq!(s.sample(2.0).1, 0.0);
    }

    fn signals_with(id: SignalId, signal: &Signal, notes: &[SignalNote]) -> Signals {
        let mut signals = Signals::default();
        signals.insert(id, CompiledSignal::compile(signal, notes.iter().copied()));
        signals
    }

    /// A layer listening to `signal` through `blocks`.
    fn listening(mode: BlendMode, signal: SignalId, blocks: Vec<Block>) -> (LayerId, Layer) {
        let mut layer = Layer::new(mode).listening(signal);
        layer.blocks = blocks
            .into_iter()
            .enumerate()
            .map(|(i, b)| (BlockId(i as u64), b))
            .collect();
        (LayerId(1), layer)
    }

    #[test]
    fn listening_layers_read_the_signal_in_song_time_through_their_blocks() {
        // A ramp from the centre line to the top across beats 0..4, heard
        // through two windows: each hears the signal at the song position,
        // whatever its own shape.
        let id = SignalId(1);
        let ramp = Block::new(beats(0.0), beats(4.0), Shape::ramp(0.5, 1.0));
        let signals = signals_with(id, &drawn(vec![ramp]), &[]);
        let windows = vec![
            Block::new(beats(0.0), beats(1.0), Shape::constant(0.0)),
            Block::new(beats(2.0), beats(2.0), Shape::constant(0.0)),
        ];
        let layer = listening(BlendMode::Add, id, windows);
        let a = Automation::compile_layers_with(0.2, &[layer], &signals);
        assert!(close(a.value_at(0.5), 0.2 + 0.125));
        assert_eq!(a.value_at(1.5), 0.2, "no window here");
        assert!(close(a.value_at(3.0), 0.2 + 0.75));
    }

    #[test]
    fn a_drawn_layer_ignores_signals() {
        let id = SignalId(1);
        let flat = Block::new(beats(0.0), beats(4.0), Shape::constant(1.0));
        let signals = signals_with(id, &drawn(vec![flat]), &[]);
        let mut layer = listening(
            BlendMode::Add,
            id,
            vec![Block::new(beats(0.0), beats(4.0), Shape::constant(0.75))],
        );
        layer.1.signal = None;
        let a = Automation::compile_layers_with(0.2, &[layer], &signals);
        assert!(close(a.value_at(1.0), 0.7), "its own shape");
    }

    #[test]
    fn a_signal_gap_lets_nothing_through() {
        let id = SignalId(1);
        let short = Block::new(beats(0.0), beats(1.0), Shape::constant(1.0));
        let signals = signals_with(id, &drawn(vec![short]), &[]);
        let window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0));
        let a = Automation::compile_layers_with(
            0.2,
            &[listening(BlendMode::Add, id, vec![window])],
            &signals,
        );
        assert!(close(a.value_at(0.5), 1.0));
        assert_eq!(a.value_at(2.0), 0.2);
    }

    #[test]
    fn block_controls_still_apply_to_the_signal() {
        let id = SignalId(1);
        let high = Block::new(beats(0.0), beats(4.0), Shape::constant(1.0));
        let signals = signals_with(id, &drawn(vec![high]), &[]);
        let mut window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0));
        window.depth = 0.5;
        window.invert = true;
        window.fade_in = beats(2.0);
        let a = Automation::compile_layers_with(
            0.8,
            &[listening(BlendMode::Add, id, vec![window])],
            &signals,
        );
        // Inverted (-1), half depth, half way through the fade: -0.25.
        assert!(close(a.value_at(1.0), 0.55));
    }

    #[test]
    fn nudge_shifts_where_the_signal_is_read() {
        let id = SignalId(1);
        let ramp = Block::new(beats(0.0), beats(4.0), Shape::ramp(0.0, 1.0));
        let signals = signals_with(id, &drawn(vec![ramp]), &[]);
        let mut window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0));
        window.nudge = beats(1.0);
        let a = Automation::compile_layers_with(
            0.5,
            &[listening(BlendMode::Add, id, vec![window])],
            &signals,
        );
        assert!(close(a.value_at(1.0), 0.5), "reads beat 2: the centre");
    }

    #[test]
    fn a_layer_listening_to_a_missing_signal_has_no_effect() {
        let window = Block::new(beats(0.0), beats(4.0), Shape::constant(1.0));
        let layer = listening(BlendMode::Add, SignalId(9), vec![window]);
        let a = Automation::compile_layers(0.3, &[layer]);
        assert_eq!(a.value_at(1.0), 0.3);
    }

    #[test]
    fn a_kick_signal_ducks_every_listening_parameter_in_sync() {
        // Hits straight up, back down over half a beat; Multiply layers
        // with the window inverted pull down at each kick.
        let id = SignalId(1);
        let kicks = [0.0, 1.0, 2.0, 3.0].map(|start| note(start, 0.9, KICK));
        let signals = signals_with(id, &notes_signal(envelope(0.0, 0.5, 0.0, 0.0)), &kicks);
        let duck = |base: f64| {
            let mut window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.5));
            window.invert = true;
            let layer = listening(BlendMode::Multiply, id, vec![window]);
            Automation::compile_layers_with(base, &[layer], &signals)
        };
        let (volume, cutoff) = (duck(0.8), duck(0.6));
        for kick in [0.0, 1.0, 2.0, 3.0] {
            assert_eq!(volume.value_at(kick), 0.0, "fully ducked at {kick}");
            assert_eq!(cutoff.value_at(kick), 0.0);
            assert!(close(volume.value_at(kick + 0.25), 0.4));
            assert!(close(cutoff.value_at(kick + 0.25), 0.3));
            assert!(close(volume.value_at(kick + 0.75), 0.8), "recovered");
        }
    }
}
