//! A four-bar demo pattern: offbeat bass and a sixteenth-note arpeggio over
//! Am - F - C - G, looping, shaped by layers:
//!
//! - a filter effect sweeps open across the four bars;
//! - the volume pumps on every beat, like sidechain ducking;
//! - the synth cutoff wobbles through bars 3 and 4.

use daw_engine::devices::{channel, filter, instrument};
use daw_engine::{
    BlendMode, Block, BlockId, ClipId, Command, CommandError, DeviceId, DeviceRef, Edit, Effect,
    EffectKind, Engine, Layer, LayerId, LoopRange, MidiClip, MidiNote, ParamId, Segment, Shape,
    ShapePoint, Ticks,
};

/// What to listen for, printed by the player.
pub const DESCRIPTION: &str = "Listen for: the filter opening over each four bars, the volume \
                                   pumping on every beat, and the arpeggio wobbling in bars 3 and 4.";

const FILTER: DeviceId = DeviceId(1);

pub const TEMPO_BPM: f64 = 124.0;
pub const BARS: i64 = 4;

/// Root note and arpeggio chord tones for each bar.
const PROGRESSION: [(u8, [u8; 4]); BARS as usize] = [
    (45, [57, 60, 64, 69]), // Am
    (41, [53, 57, 60, 65]), // F
    (48, [55, 60, 64, 67]), // C
    (43, [55, 59, 62, 67]), // G
];

/// Order the chord tones are played in across a beat pair: up then down.
const ARP_ORDER: [usize; 8] = [0, 1, 2, 3, 2, 1, 0, 1];

/// The full demo: the starter notes, shaped by layers.
pub fn load(engine: &mut Engine) -> Result<(), CommandError> {
    engine.apply(Command::Edit(Edit::RenameSong {
        name: "Demo".to_owned(),
    }))?;
    add_starter_notes(engine)?;
    add_layers(engine)
}

/// What every new song starts with: the bass and arpeggio clips at the
/// demo tempo, looping, with no effects or layers yet.
pub fn add_starter_notes(engine: &mut Engine) -> Result<(), CommandError> {
    engine.apply(Command::Edit(Edit::SetTempo { bpm: TEMPO_BPM }))?;
    add(engine, 1, bass())?;
    add(engine, 2, arpeggio())?;
    engine.apply(Command::SetLoop(Some(LoopRange {
        start: Ticks::ZERO,
        end: Ticks::from_bars(BARS),
    })))
}

fn add(engine: &mut Engine, id: u64, notes: Vec<MidiNote>) -> Result<(), CommandError> {
    engine.apply(Command::Edit(Edit::AddClip {
        id: ClipId(id),
        clip: MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_bars(BARS),
            notes,
        },
    }))
}

fn sixteenths(n: i64) -> Ticks {
    Ticks(n * Ticks::from_beats(1).0 / 4)
}

fn bass() -> Vec<MidiNote> {
    let mut notes = Vec::new();
    for (bar, (root, _)) in PROGRESSION.iter().enumerate() {
        for beat in 0..4 {
            notes.push(MidiNote {
                start: Ticks::from_bars(bar as i64) + Ticks::from_beats(beat) + sixteenths(2),
                length: sixteenths(1),
                pitch: *root,
                velocity: 110,
            });
        }
    }
    notes
}

fn arpeggio() -> Vec<MidiNote> {
    let mut notes = Vec::new();
    for (bar, (_, chord)) in PROGRESSION.iter().enumerate() {
        for step in 0..16 {
            notes.push(MidiNote {
                start: Ticks::from_bars(bar as i64) + sixteenths(step),
                length: Ticks(sixteenths(1).0 * 3 / 4),
                pitch: chord[ARP_ORDER[step as usize % ARP_ORDER.len()]],
                velocity: if step % 2 == 0 { 96 } else { 64 },
            });
        }
    }
    notes
}

fn add_layers(engine: &mut Engine) -> Result<(), CommandError> {
    engine.apply(Command::Edit(Edit::AddEffect {
        id: FILTER,
        index: 0,
        effect: Effect::new(EffectKind::Filter),
    }))?;

    // The filter rests low and opens slowly at first, then quickly, over the
    // whole loop: from the centre line (no change) up by 0.6.
    let filter_cutoff = ParamId {
        device: DeviceRef::Effect(FILTER),
        index: filter::CUTOFF,
    };
    engine.apply(Command::Edit(Edit::SetParam {
        param: filter_cutoff,
        value: 0.35,
    }))?;
    let sweep = Block::new(
        Ticks::ZERO,
        Ticks::from_bars(BARS),
        Shape {
            points: vec![
                ShapePoint::new(0.0, 0.5).with_segment(Segment::Curve(0.6)),
                ShapePoint::new(1.0, 0.8),
            ],
        },
    );
    add_layer(engine, filter_cutoff, 1, BlendMode::Add, sweep)?;

    // Each beat starts quiet and swells back to the centre line (no
    // change): ducking, drawn once.
    let mut pump = Block::new(
        Ticks::ZERO,
        Ticks::from_bars(BARS),
        Shape {
            points: vec![
                ShapePoint::new(0.0, 0.075).with_segment(Segment::Curve(-0.5)),
                ShapePoint::new(0.7, 0.5),
            ],
        },
    );
    pump.cycle = Some(Ticks::from_beats(1));
    let volume = ParamId {
        device: DeviceRef::Channel,
        index: channel::VOLUME,
    };
    add_layer(engine, volume, 2, BlendMode::Multiply, pump)?;

    // A triangle wobble every eighth note, fading in over a beat.
    let mut wobble = Block::new(
        Ticks::from_bars(2),
        Ticks::from_bars(2),
        Shape {
            points: vec![
                ShapePoint::new(0.0, 0.0),
                ShapePoint::new(0.5, 1.0),
                ShapePoint::new(1.0, 0.0),
            ],
        },
    );
    wobble.cycle = Some(sixteenths(2));
    wobble.depth = 0.2;
    wobble.fade_in = Ticks::from_beats(1);
    let synth_cutoff = ParamId {
        device: DeviceRef::Instrument,
        index: instrument::CUTOFF,
    };
    add_layer(engine, synth_cutoff, 3, BlendMode::Add, wobble)
}

/// Adds a single-block layer, using one id for both the layer and the block.
fn add_layer(
    engine: &mut Engine,
    param: ParamId,
    id: u64,
    mode: BlendMode,
    block: Block,
) -> Result<(), CommandError> {
    let mut layer = Layer::new(mode);
    layer.blocks.push((BlockId(id), block));
    engine.apply(Command::Edit(Edit::AddLayer {
        param,
        id: LayerId(id),
        index: 0,
        layer,
    }))
}
