//! A four-bar demo pattern: offbeat bass and a sixteenth-note arpeggio over
//! Am - F - C - G, looping.

use daw_engine::{
    ClipId, Command, CommandError, Edit, Engine, LoopRange, MidiClip, MidiNote, Ticks,
};

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

pub fn load(engine: &mut Engine) -> Result<(), CommandError> {
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
