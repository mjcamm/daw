//! Compiled motion signals: shapes over song time that linked blocks read.
//!
//! A signal gives a value and a presence at every song position. Presence
//! is 0 in a gap, where linked blocks have no effect, just like a gap in a
//! layer.

use crate::shape::shape_value;
use crate::stack::CompiledBlock;
use daw_protocol::{NotesSource, ShapePoint, Signal, SignalId, SignalSource};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum CompiledSignal {
    /// Says nothing anywhere: what a link to a missing signal reads.
    Silent,
    /// Drawn blocks, sorted by start, non-overlapping.
    Drawn(Vec<CompiledBlock>),
    /// An envelope restarted at each trigger.
    Notes {
        /// Note starts in beats, sorted.
        triggers: Vec<f64>,
        /// Envelope length in beats.
        length: f64,
        points: Vec<ShapePoint>,
    },
}

impl CompiledSignal {
    /// Compiles `signal`. `notes` are the (start beat, pitch) of every note
    /// on the source node, in any order; only notes signals use them.
    pub fn compile(signal: &Signal, notes: impl IntoIterator<Item = (f64, u8)>) -> Self {
        match &signal.source {
            SignalSource::Drawn(blocks) => {
                let mut blocks: Vec<CompiledBlock> = blocks
                    .iter()
                    .map(|(_, block)| CompiledBlock::compile(block))
                    .collect();
                blocks.sort_by(|a, b| a.start.total_cmp(&b.start));
                Self::Drawn(blocks)
            }
            SignalSource::Notes(source) => Self::from_notes(source, notes),
        }
    }

    fn from_notes(source: &NotesSource, notes: impl IntoIterator<Item = (f64, u8)>) -> Self {
        let mut triggers: Vec<f64> = notes
            .into_iter()
            .filter(|(_, pitch)| (source.lowest..=source.highest).contains(pitch))
            .map(|(start, _)| start)
            .collect();
        triggers.sort_by(f64::total_cmp);
        triggers.dedup();
        Self::Notes {
            triggers,
            length: source.length.as_beats(),
            points: source.shape.points.clone(),
        }
    }

    /// Value and presence at `beat`. Allocation-free.
    pub fn sample(&self, beat: f64) -> (f64, f64) {
        const SILENT: (f64, f64) = (0.0, 0.0);
        match self {
            Self::Silent => SILENT,
            Self::Drawn(blocks) => {
                let after = blocks.partition_point(|b| b.start <= beat);
                after
                    .checked_sub(1)
                    .map(|i| &blocks[i])
                    .filter(|b| b.contains(beat))
                    .map_or(SILENT, |b| b.sample(beat))
            }
            Self::Notes {
                triggers,
                length,
                points,
            } => {
                let after = triggers.partition_point(|t| *t <= beat);
                let Some(i) = after.checked_sub(1) else {
                    return SILENT;
                };
                let since = beat - triggers[i];
                if since < *length {
                    (shape_value(points, since / length), 1.0)
                } else {
                    SILENT
                }
            }
        }
    }
}

/// Compiled signals by id, for compiling the blocks that link to them.
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
    use daw_protocol::{BlendMode, Block, BlockId, Layer, LayerId, Shape, SignalLink, Ticks};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn beats(n: f64) -> Ticks {
        Ticks::from_beats_f64(n)
    }

    const KICK: u8 = 36;

    /// A ducking envelope: rises from 0 to 1 over half a beat.
    fn ducking() -> Signal {
        Signal {
            name: "Kick duck".to_owned(),
            color: 0,
            source: SignalSource::Notes(NotesSource {
                lowest: KICK,
                highest: KICK,
                length: beats(0.5),
                shape: Shape::ramp(0.0, 1.0),
            }),
        }
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
    fn each_matching_note_restarts_the_envelope() {
        let kicks = [(0.0, KICK), (1.0, KICK), (2.0, KICK)];
        let s = CompiledSignal::compile(&ducking(), kicks);
        for start in [0.0, 1.0, 2.0] {
            assert_eq!(s.sample(start), (0.0, 1.0));
            let (x, w) = s.sample(start + 0.25);
            assert!(close(x, 0.5) && w == 1.0, "{start}");
        }
    }

    #[test]
    fn notes_outside_the_pitch_range_are_ignored() {
        let s = CompiledSignal::compile(&ducking(), [(0.0, KICK), (0.25, 60)]);
        let (x, _) = s.sample(0.375);
        assert!(close(x, 0.75), "the snare at 0.25 did not restart it");
    }

    #[test]
    fn the_signal_is_silent_before_the_first_note_and_after_the_envelope() {
        let s = CompiledSignal::compile(&ducking(), [(1.0, KICK)]);
        assert_eq!(s.sample(0.5), (0.0, 0.0));
        assert_eq!(s.sample(1.5), (0.0, 0.0));
        assert_eq!(s.sample(9.0), (0.0, 0.0));
    }

    #[test]
    fn a_new_note_cuts_the_envelope_short() {
        let s = CompiledSignal::compile(&ducking(), [(0.0, KICK), (0.25, KICK)]);
        assert_eq!(s.sample(0.25), (0.0, 1.0));
    }

    #[test]
    fn note_order_does_not_matter() {
        let a = CompiledSignal::compile(&ducking(), [(2.0, KICK), (0.0, KICK)]);
        let b = CompiledSignal::compile(&ducking(), [(0.0, KICK), (2.0, KICK)]);
        assert_eq!(a, b);
    }

    #[test]
    fn drawn_signals_are_silent_in_their_gaps() {
        let block = Block::new(beats(1.0), beats(1.0), Shape::constant(0.7));
        let s = CompiledSignal::compile(&drawn(vec![block]), []);
        assert_eq!(s.sample(0.5), (0.0, 0.0));
        assert_eq!(s.sample(1.5), (0.7, 1.0));
        assert_eq!(s.sample(2.0), (0.0, 0.0));
    }

    fn signals_with(id: SignalId, signal: &Signal, notes: &[(f64, u8)]) -> Signals {
        let mut signals = Signals::default();
        signals.insert(id, CompiledSignal::compile(signal, notes.iter().copied()));
        signals
    }

    #[test]
    fn linked_blocks_read_the_signal_in_song_time() {
        // A rising ramp across beats 0..4, read by blocks that start at
        // different places: each hears the signal at the song position.
        let id = SignalId(1);
        let ramp = Block::new(beats(0.0), beats(4.0), Shape::ramp(0.0, 1.0));
        let signals = signals_with(id, &drawn(vec![ramp]), &[]);
        let early = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0)).linked(id);
        let late = Block::new(beats(2.0), beats(2.0), Shape::constant(0.0)).linked(id);
        let early = CompiledBlock::compile_with(&early, &signals);
        let late = CompiledBlock::compile_with(&late, &signals);
        assert!(close(early.sample(3.0).0, 0.75));
        assert!(close(late.sample(3.0).0, 0.75));
    }

    #[test]
    fn links_map_the_signal_into_their_range() {
        let id = SignalId(1);
        let flat = Block::new(beats(0.0), beats(4.0), Shape::constant(0.5));
        let signals = signals_with(id, &drawn(vec![flat]), &[]);
        let mut block = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0));
        block.link = Some(SignalLink {
            signal: id,
            low: 0.2,
            high: 0.6,
        });
        let mapped = CompiledBlock::compile_with(&block, &signals);
        assert!(close(mapped.sample(1.0).0, 0.4));
        block.invert = true;
        let inverted = CompiledBlock::compile_with(&block, &signals);
        assert!(close(inverted.sample(1.0).0, 0.6), "invert after mapping");
    }

    #[test]
    fn a_signal_gap_silences_the_linked_block() {
        let id = SignalId(1);
        let signals = signals_with(id, &ducking(), &[(0.0, KICK)]);
        let block = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0)).linked(id);
        let linked = CompiledBlock::compile_with(&block, &signals);
        assert_eq!(linked.sample(0.0).1, 1.0);
        assert_eq!(linked.sample(2.0).1, 0.0, "after the envelope");
    }

    #[test]
    fn nudge_shifts_where_the_signal_is_read() {
        let id = SignalId(1);
        let signals = signals_with(id, &ducking(), &[(1.0, KICK)]);
        let mut block = Block::new(beats(0.0), beats(4.0), Shape::constant(0.0)).linked(id);
        block.nudge = beats(0.25);
        let linked = CompiledBlock::compile_with(&block, &signals);
        assert!(close(linked.sample(1.0).0, 0.5), "reads beat 1.25");
    }

    #[test]
    fn a_link_to_a_missing_signal_has_no_effect() {
        let block = Block::new(beats(0.0), beats(4.0), Shape::constant(1.0)).linked(SignalId(9));
        let compiled = CompiledBlock::compile(&block);
        assert_eq!(compiled.sample(1.0).1, 0.0);
    }

    #[test]
    fn a_ducking_signal_ducks_every_linked_parameter_in_sync() {
        let id = SignalId(1);
        let kicks = [(0.0, KICK), (1.0, KICK), (2.0, KICK), (3.0, KICK)];
        let signals = signals_with(id, &ducking(), &kicks);
        let duck = |base: f64| {
            let mut layer = Layer::new(BlendMode::Multiply);
            // The envelope's 0..1 lands on the bottom half of the lane: from
            // a full duck up to the centre line (no change).
            let mut block = Block::new(beats(0.0), beats(4.0), Shape::constant(1.0));
            block.link = Some(SignalLink {
                signal: id,
                low: 0.0,
                high: 0.5,
            });
            layer.blocks.push((BlockId(1), block));
            Automation::compile_layers_with(base, &[(LayerId(1), layer)], &signals)
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
