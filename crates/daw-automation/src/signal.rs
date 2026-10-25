//! Compiled signals: centred shapes over song time that listening
//! layers read.
//!
//! A signal gives a value and a presence at every song position. Values use
//! the same space as block shapes: `0.5` is the centre line (no effect), `1`
//! the top (+1) and `0` the bottom (-1). Presence is 0 where a drawn signal
//! has a gap, so a listening layer has no effect there, just like a gap in a
//! layer.

use crate::curve::curve_value;
use crate::stack::CompiledBlock;
use daw_protocol::{CurvePoint, HitMode, NotesSource, Signal, SignalId, SignalSource};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum CompiledSignal {
    /// Says nothing anywhere: what a layer listening to a missing signal
    /// reads.
    Silent,
    /// Drawn blocks, sorted by start, non-overlapping.
    Drawn(Vec<CompiledBlock>),
    /// A shape played on every note. Always present; rests on the centre
    /// line.
    Notes(NotesEnvelope),
    /// An audio node's loudness, followed as the song plays: read from
    /// [`Live`] by its `slot`. Always present; silence is the centre line.
    Audio { slot: usize },
    /// An audio signal as a render of the song found it, for drawing:
    /// `values[i]` (`0.5` is the centre line) holds from `i * step_beats`
    /// until the next. Always present; the centre line outside it.
    Recorded {
        step_beats: f64,
        values: std::sync::Arc<[f32]>,
    },
}

/// Where signals followed as the song plays are read: the audio side's
/// values at the moment being evaluated.
pub trait Live {
    /// The value (`0.5` is the centre line) of the audio signal in `slot`.
    fn audio(&self, slot: usize) -> f64;
}

/// Nothing followed: every audio signal rests on the centre line.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLive;

impl Live for NoLive {
    fn audio(&self, _slot: usize) -> f64 {
        0.5
    }
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

/// How long the line glides from where it was whenever a hit starts or ends,
/// so a restart or a stop never jumps: 15 ticks (1/64 beat).
pub const GLIDE_BEATS: f64 = 15.0 / 960.0;

/// Every hit of a notes signal and the shape each plays, with times in
/// beats. Values use the shape space: `0.5` is the centre line.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesEnvelope {
    /// Sorted by start, one per start.
    hits: Vec<Hit>,
    shape: Vec<CurvePoint>,
    mode: HitMode,
    /// One play of the shape, in beats.
    window: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Hit {
    start: f64,
    /// When the shape stops: the window's end in Trigger mode, the note's
    /// in Loop mode. A later hit cuts it short.
    end: f64,
    /// How far the shape reaches from the centre line, `0..=1`.
    size: f64,
    /// Where the line was when the hit started, which it glides from.
    from: f64,
}

impl NotesEnvelope {
    fn compile(source: &NotesSource, notes: impl IntoIterator<Item = SignalNote>) -> Self {
        let window = source.window.as_beats();
        let mut envelope = Self {
            hits: Vec::new(),
            shape: source.shape.points.clone(),
            mode: source.mode,
            window,
        };
        let mut notes: Vec<SignalNote> = notes
            .into_iter()
            .filter(|n| !source.pitch_range || (source.lowest..=source.highest).contains(&n.pitch))
            .collect();
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        for note in notes {
            let velocity = f64::from(note.velocity) / 127.0;
            let size = if source.velocity_clamp {
                velocity.clamp(source.velocity_min, source.velocity_max)
            } else {
                velocity
            };
            let end = match source.mode {
                HitMode::Trigger => note.start + window,
                HitMode::Loop => note.end,
            };
            match envelope.hits.last_mut() {
                // Notes starting together are one hit: the strongest, held
                // for the longest.
                Some(last) if last.start == note.start => {
                    last.size = last.size.max(size);
                    last.end = last.end.max(end);
                }
                _ => envelope.hits.push(Hit {
                    start: note.start,
                    end,
                    size,
                    from: 0.5,
                }),
            }
        }
        for i in 1..envelope.hits.len() {
            let (previous, start) = (envelope.hits[i - 1], envelope.hits[i].start);
            envelope.hits[i].from = envelope.level(&previous, start);
        }
        envelope
    }

    /// The shape's value `since` beats into `hit`, scaled by its size, with
    /// the glide in from where the line was. `ending` reads the very end of
    /// a cycle as its last value rather than the next cycle's first.
    fn playing(&self, hit: &Hit, since: f64, ending: bool) -> f64 {
        let cycles = since / self.window;
        let phase = match self.mode {
            HitMode::Trigger => cycles.min(1.0),
            HitMode::Loop if ending && cycles > 0.0 && cycles.fract() == 0.0 => 1.0,
            HitMode::Loop => cycles.fract(),
        };
        let value = 0.5 + (curve_value(&self.shape, phase) - 0.5) * hit.size;
        if since < GLIDE_BEATS {
            hit.from + (value - hit.from) * since / GLIDE_BEATS
        } else {
            value
        }
    }

    /// The value of `hit` at `beat`, which is at or after its start.
    fn level(&self, hit: &Hit, beat: f64) -> f64 {
        if beat < hit.end {
            return self.playing(hit, beat - hit.start, false);
        }
        let stopped = self.playing(hit, hit.end - hit.start, true);
        let since = beat - hit.end;
        if since < GLIDE_BEATS {
            stopped + (0.5 - stopped) * since / GLIDE_BEATS
        } else {
            0.5
        }
    }

    /// The value at `beat`. Allocation-free.
    fn sample(&self, beat: f64) -> f64 {
        let after = self.hits.partition_point(|h| h.start <= beat);
        after
            .checked_sub(1)
            .map_or(0.5, |i| self.level(&self.hits[i], beat))
    }
}

impl CompiledSignal {
    /// Compiles `signal`. `notes` are every note on the source node, in any
    /// order; only notes signals use them. An audio signal compiles silent:
    /// only the engine knows where it is followed ([`Self::Audio`]).
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
            SignalSource::Audio(_) => Self::Silent,
        }
    }

    /// Value (`0.5` is the centre line) and presence at `beat`, with
    /// nothing followed. Allocation-free.
    pub fn sample(&self, beat: f64) -> (f64, f64) {
        self.sample_live(beat, &NoLive)
    }

    /// Value and presence at `beat`, audio signals read from `live`.
    /// Allocation-free.
    pub fn sample_live(&self, beat: f64, live: &dyn Live) -> (f64, f64) {
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
            Self::Notes(envelope) => (envelope.sample(beat), 1.0),
            Self::Audio { slot } => (live.audio(*slot), 1.0),
            Self::Recorded { step_beats, values } => {
                let index = beat / step_beats;
                let value = if index >= 0.0 {
                    values.get(index as usize).map_or(0.5, |v| f64::from(*v))
                } else {
                    0.5
                };
                (value, 1.0)
            }
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
    use daw_protocol::{BlendMode, Block, BlockId, Curve, Layer, LayerId, Shape, Ticks};

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

    /// Kicks only, every hit full size, playing `shape` over `window` beats.
    fn shaped(mode: HitMode, window: f64, shape: Curve) -> NotesSource {
        NotesSource {
            pitch_range: true,
            velocity_clamp: true,
            lowest: KICK,
            highest: KICK,
            mode,
            window: beats(window),
            shape,
            velocity_min: 1.0,
            velocity_max: 1.0,
            ..NotesSource::new(daw_protocol::NodeId(1))
        }
    }

    /// A one-beat Trigger hit from the top straight down to the bottom.
    fn falling() -> NotesSource {
        shaped(HitMode::Trigger, 1.0, Curve::line(1.0, 0.0))
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

    /// Past the glide at the start of a hit at `start`.
    fn settled(start: f64) -> f64 {
        start + GLIDE_BEATS
    }

    #[test]
    fn notes_signals_rest_on_the_centre_line_and_are_always_present() {
        assert_eq!(level(falling(), &[kick(1.0)], 0.5), (0.0, 1.0));
        assert_eq!(level(falling(), &[kick(1.0)], 9.0), (0.0, 1.0));
    }

    #[test]
    fn a_hit_plays_the_shape_over_its_window_using_the_full_range() {
        // From +1 at the start to -1 at the end of the window.
        let hits = [kick(0.0)];
        assert!(close(level(falling(), &hits, 0.25).0, 0.5));
        assert!(close(level(falling(), &hits, 0.5).0, 0.0));
        assert!(close(level(falling(), &hits, 0.75).0, -0.5));
    }

    #[test]
    fn trigger_plays_the_whole_shape_whatever_the_notes_length() {
        // The kick lasts a tenth of a beat; the shape still plays through.
        assert!(close(level(falling(), &[kick(0.0)], 0.9).0, -0.8));
    }

    #[test]
    fn after_its_window_a_trigger_hit_glides_back_to_the_centre() {
        let hits = [kick(0.0)];
        let halfway = level(falling(), &hits, 1.0 + GLIDE_BEATS / 2.0).0;
        assert!(close(halfway, -0.5), "gliding: {halfway}");
        assert_eq!(level(falling(), &hits, settled(1.0)).0, 0.0);
    }

    #[test]
    fn loop_repeats_the_shape_while_the_note_is_held() {
        let source = shaped(HitMode::Loop, 1.0, Curve::line(1.0, 0.0));
        let held = [note(0.0, 2.5, KICK)];
        assert!(close(level(source.clone(), &held, 0.5).0, 0.0));
        assert!(close(level(source.clone(), &held, 1.25).0, 0.5), "again");
        assert!(
            close(level(source.clone(), &held, 2.25).0, 0.5),
            "and again"
        );
        assert_eq!(
            level(source, &held, settled(2.5)).0,
            0.0,
            "stops with the note"
        );
    }

    #[test]
    fn a_loop_ending_on_a_cycle_boundary_stops_from_the_shapes_end() {
        let source = shaped(HitMode::Loop, 1.0, Curve::line(1.0, 0.0));
        let held = [note(0.0, 2.0, KICK)];
        // Gliding to the centre from the bottom (-1), not from the top.
        let halfway = level(source, &held, 2.0 + GLIDE_BEATS / 2.0).0;
        assert!(close(halfway, -0.5), "{halfway}");
    }

    #[test]
    fn a_new_note_restarts_the_shape() {
        let hits = [kick(0.0), kick(0.5)];
        let restarted = level(falling(), &hits, settled(0.5)).0;
        assert!(close(restarted, 1.0 - 2.0 * GLIDE_BEATS), "{restarted}");
        let loops = shaped(HitMode::Loop, 1.0, Curve::line(1.0, 0.0));
        let held = [note(0.0, 4.0, KICK), note(0.5, 4.0, KICK)];
        assert!(close(level(loops, &held, 1.0).0, 0.0), "restarted at 0.5");
    }

    #[test]
    fn a_restart_glides_from_wherever_the_line_was() {
        // At 0.5 the first hit is at the centre (0); the second starts at
        // the top, reached after the glide.
        let hits = [kick(0.0), kick(0.5)];
        assert!(close(level(falling(), &hits, 0.5).0, 0.0), "no jump");
        let halfway = level(falling(), &hits, 0.5 + GLIDE_BEATS / 2.0).0;
        let target = 1.0 - GLIDE_BEATS;
        assert!(close(halfway, target / 2.0), "{halfway}");
    }

    #[test]
    fn velocity_scales_the_shape_towards_the_centre_within_the_clamp() {
        let at = |velocity: u8| SignalNote {
            velocity,
            ..kick(0.0)
        };
        let clamped = |min: f64, max: f64, velocity: u8| {
            let source = NotesSource {
                velocity_min: min,
                velocity_max: max,
                ..shaped(HitMode::Trigger, 1.0, Curve::line(0.0, 0.0))
            };
            -level(source, &[at(velocity)], settled(0.0)).0
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
            ..shaped(HitMode::Trigger, 1.0, Curve::line(0.0, 0.0))
        };
        let size = -level(source, &[soft], settled(0.0)).0;
        assert!(close(size, 64.0 / 127.0), "{size}");
    }

    #[test]
    fn without_the_pitch_range_every_note_hits() {
        let source = NotesSource {
            pitch_range: false,
            ..falling()
        };
        let snare = [note(0.0, 1.0, 60)];
        assert!(close(level(source, &snare, 0.25).0, 0.5));
        assert_eq!(level(falling(), &snare, 0.25).0, 0.0, "range on: no hit");
    }

    #[test]
    fn notes_outside_the_pitch_range_are_ignored() {
        let notes = [kick(0.0), note(0.25, 0.1, 60)];
        assert!(
            close(level(falling(), &notes, 0.5).0, 0.0),
            "the snare did not restart it"
        );
    }

    #[test]
    fn note_order_does_not_matter() {
        let signal = notes_signal(falling());
        let a = CompiledSignal::compile(&signal, [kick(2.0), kick(0.0)]);
        let b = CompiledSignal::compile(&signal, [kick(0.0), kick(2.0)]);
        assert_eq!(a, b);
    }

    #[test]
    fn notes_starting_together_are_one_hit() {
        let source = NotesSource {
            lowest: 0,
            highest: 127,
            ..shaped(HitMode::Loop, 1.0, Curve::line(1.0, 1.0))
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
        // A duck drawn directly: down to the bottom at each kick, back up to
        // the centre over half a beat. Multiply layers pull down with it.
        let id = SignalId(1);
        let kicks = [0.0, 1.0, 2.0, 3.0].map(|start| note(start, 0.9, KICK));
        let duck = shaped(HitMode::Trigger, 0.5, Curve::line(0.0, 0.5));
        let signals = signals_with(id, &notes_signal(duck), &kicks);
        let ducked = |base: f64| {
            let window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.5));
            let layer = listening(BlendMode::Multiply, id, vec![window]);
            Automation::compile_layers_with(base, &[layer], &signals)
        };
        let (volume, cutoff) = (ducked(0.8), ducked(0.6));
        for kick in [0.0, 1.0, 2.0, 3.0] {
            let deepest = kick + GLIDE_BEATS;
            assert!(volume.value_at(deepest) < 0.05, "ducked at {kick}");
            assert!(cutoff.value_at(deepest) < 0.05);
            assert!(close(volume.value_at(kick + 0.25), 0.4));
            assert!(close(cutoff.value_at(kick + 0.25), 0.3));
            assert!(close(volume.value_at(kick + 0.75), 0.8), "recovered");
        }
    }

    /// Followed values by slot, for tests.
    struct Followed(&'static [f64]);

    impl Live for Followed {
        fn audio(&self, slot: usize) -> f64 {
            self.0[slot]
        }
    }

    #[test]
    fn an_audio_signal_compiles_silent_and_is_read_live_through_windows() {
        let source = daw_protocol::AudioSource::new(daw_protocol::NodeId(3));
        let signal = Signal {
            name: "Drums".to_owned(),
            color: 0,
            source: SignalSource::Audio(source),
        };
        assert_eq!(CompiledSignal::compile(&signal, []), CompiledSignal::Silent);

        let id = SignalId(1);
        let mut signals = Signals::default();
        signals.insert(id, CompiledSignal::Audio { slot: 1 });
        // Inverted on Multiply: loud ducks.
        let mut window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0));
        window.invert = true;
        let layer = listening(BlendMode::Multiply, id, vec![window]);
        let a = Automation::compile_layers_with(0.8, &[layer], &signals);
        let loud = Followed(&[0.5, 1.0]);
        let quiet = Followed(&[1.0, 0.5]);
        assert!(close(a.value_at_live(1.0, &loud), 0.0), "full duck");
        assert!(
            close(a.value_at_live(1.0, &quiet), 0.8),
            "silence: no effect"
        );
        assert!(close(a.value_at(1.0), 0.8), "nothing followed: no effect");
        assert!(
            close(a.value_at_live(5.0, &loud), 0.8),
            "outside the window"
        );
    }

    #[test]
    fn a_nudge_does_not_move_an_audio_signal() {
        let id = SignalId(1);
        let mut signals = Signals::default();
        signals.insert(id, CompiledSignal::Audio { slot: 0 });
        let mut window = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0));
        window.nudge = beats(8.0);
        let layer = listening(BlendMode::Add, id, vec![window]);
        let a = Automation::compile_layers_with(0.2, &[layer], &signals);
        assert!(close(a.value_at_live(1.0, &Followed(&[0.75])), 0.2 + 0.5));
    }
}
