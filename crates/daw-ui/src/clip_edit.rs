//! Clips on Arrange: moving, copying and resizing them, and what a drop does
//! to the clips it lands on. Pure functions over clips and the snapshot, so
//! the rules are tested without a window.
//!
//! - Positions are song time in ticks. Moving a clip moves its notes with
//!   it; resizing an edge keeps every note where it is in the song.
//! - A clip dropped (or resized) over others wins: the parts of other clips
//!   it covers are cut away, trimming, splitting or removing them.
//! - Everything one gesture does is one [`Edit::Batch`], so one undo.

use daw_engine::{ClipId, Edit, MidiClip, MidiNote, NodeId, Snapshot, Ticks};

/// A clip's end in song time.
pub fn end(clip: &MidiClip) -> Ticks {
    clip.start + clip.length
}

/// The clip moved by `delta`, notes and all.
pub fn moved(clip: &MidiClip, delta: Ticks) -> MidiClip {
    MidiClip {
        start: clip.start + delta,
        ..clip.clone()
    }
}

/// The part of `clip` inside `from..to` (song time), as a clip of its own,
/// or `None` if nothing of it is there. Notes keep their song positions;
/// notes wholly outside are dropped.
pub fn trimmed(clip: &MidiClip, from: Ticks, to: Ticks) -> Option<MidiClip> {
    let start = from.max(clip.start);
    let stop = to.min(end(clip));
    if start >= stop {
        return None;
    }
    let shift = start - clip.start;
    let length = stop - start;
    let notes: Vec<MidiNote> = clip
        .notes
        .iter()
        .map(|n| MidiNote {
            start: n.start - shift,
            ..*n
        })
        .filter(|n| n.start < length && n.start + n.length > Ticks::ZERO)
        .collect();
    Some(MidiClip {
        start,
        length,
        notes,
    })
}

/// What is left of `clip` after cutting out `from..to`: nothing, one piece
/// (trimmed at one end) or two (split around the cut).
pub fn cut(clip: &MidiClip, from: Ticks, to: Ticks) -> Vec<MidiClip> {
    if to <= clip.start || from >= end(clip) {
        return vec![clip.clone()];
    }
    [
        trimmed(clip, clip.start, from),
        trimmed(clip, to, end(clip)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The clip with its start edge moved to `start`, keeping its end and every
/// note's song position.
pub fn with_start(clip: &MidiClip, start: Ticks) -> MidiClip {
    let shift = start - clip.start;
    MidiClip {
        start,
        length: clip.length - shift,
        notes: clip
            .notes
            .iter()
            .map(|n| MidiNote {
                start: n.start - shift,
                ..*n
            })
            .collect(),
    }
}

/// The clip with its end edge moved to `stop`.
pub fn with_end(clip: &MidiClip, stop: Ticks) -> MidiClip {
    MidiClip {
        length: stop - clip.start,
        ..clip.clone()
    }
}

/// A clip landing somewhere: an existing clip moved or resized (`Some` id)
/// or a new one (a copy, or a clip for a new node).
#[derive(Debug, Clone, PartialEq)]
pub struct Landing {
    pub id: Option<ClipId>,
    pub node: NodeId,
    pub clip: MidiClip,
}

/// An id no clip uses yet.
pub fn next_clip_id(snapshot: &Snapshot) -> ClipId {
    ClipId(
        snapshot
            .nodes
            .iter()
            .flat_map(|n| n.clips.iter().map(|(id, _)| id.0))
            .max()
            .unwrap_or(0)
            + 1,
    )
}

/// The edits that land `landings` and cut what they cover, as one step,
/// and the ids the landed clips end up with. Clips in `moving` are being
/// moved, so they are never cut. `extra` edits (such as adding a node for
/// the clips) go first.
pub fn land(
    snapshot: &Snapshot,
    landings: &[Landing],
    moving: &[ClipId],
    extra: Vec<Edit>,
) -> (Edit, Vec<ClipId>) {
    let mut next = next_clip_id(snapshot).0;
    let mut fresh = || {
        let id = ClipId(next);
        next += 1;
        id
    };
    let mut edits = extra;
    for node in &snapshot.nodes {
        let covering: Vec<(Ticks, Ticks)> = landings
            .iter()
            .filter(|l| l.node == node.id)
            .map(|l| (l.clip.start, end(&l.clip)))
            .collect();
        if covering.is_empty() {
            continue;
        }
        for (id, clip) in node.clips.iter().filter(|(id, _)| !moving.contains(id)) {
            let mut pieces = vec![clip.clone()];
            for &(from, to) in &covering {
                pieces = pieces.iter().flat_map(|p| cut(p, from, to)).collect();
            }
            if pieces.len() == 1 && pieces[0] == *clip {
                continue;
            }
            let mut pieces = pieces.into_iter();
            match pieces.next() {
                Some(first) => edits.push(Edit::SetClip {
                    id: *id,
                    node: node.id,
                    clip: first,
                }),
                None => edits.push(Edit::RemoveClip { id: *id }),
            }
            for piece in pieces {
                edits.push(Edit::AddClip {
                    node: node.id,
                    id: fresh(),
                    clip: piece,
                });
            }
        }
    }
    let mut landed = Vec::with_capacity(landings.len());
    for landing in landings {
        let id = landing.id.unwrap_or_else(&mut fresh);
        landed.push(id);
        edits.push(match landing.id {
            Some(_) => Edit::SetClip {
                id,
                node: landing.node,
                clip: landing.clip.clone(),
            },
            None => Edit::AddClip {
                node: landing.node,
                id,
                clip: landing.clip.clone(),
            },
        });
    }
    (Edit::Batch(edits), landed)
}

/// Which part of a clip a drag holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// The body: moves the clips in time and between rows.
    Body,
    /// The start edge: resizes.
    Start,
    /// The end edge: resizes.
    End,
}

/// A selected clip as a drag found it: where it was and on which row.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub id: ClipId,
    pub node: NodeId,
    pub row: usize,
    pub clip: MidiClip,
}

/// Where a drag would put the clips, and whether it can.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Each held clip's place, in the order held. Node is `None` for the
    /// new node a drop below the last row makes.
    pub places: Vec<(Option<NodeId>, MidiClip)>,
    /// False if any clip would land where clips cannot go.
    pub allowed: bool,
    /// The rows that would receive clips (the new-node row is `rows.len()`).
    pub rows: Vec<usize>,
}

/// Shortest a resize leaves a clip: a sixteenth note.
pub const MIN_LENGTH: Ticks = Ticks(daw_engine::TICKS_PER_BEAT / 4);

/// Plans a drag of `held` (the selected clips; `grabbed` indexes the one
/// under the pointer). `delta` is the pointer's sideways move in song time,
/// `row_shift` how many rows it moved down. `rows` says, for each row, its
/// node and whether it plays clips. `snap` places the grabbed clip's moving
/// edge; the others keep their offsets from it.
pub fn plan(
    held: &[Held],
    grabbed: usize,
    grab: Grab,
    delta: Ticks,
    row_shift: i64,
    rows: &[(NodeId, bool)],
    snap: impl Fn(Ticks) -> Ticks,
) -> Plan {
    let anchor = &held[grabbed].clip;
    match grab {
        Grab::Body => {
            let mut shift = snap(anchor.start + delta) - anchor.start;
            // Nothing moves before the song start.
            let earliest = held
                .iter()
                .map(|h| h.clip.start)
                .min()
                .unwrap_or(Ticks::ZERO);
            if earliest + shift < Ticks::ZERO {
                shift = Ticks::ZERO - earliest;
            }
            let same_row = held.iter().all(|h| h.row == held[0].row);
            let mut allowed = true;
            let mut target_rows = Vec::new();
            let places = held
                .iter()
                .map(|h| {
                    let row = h.row as i64 + row_shift;
                    let node = if row >= rows.len() as i64 && same_row {
                        target_rows.push(rows.len());
                        None
                    } else {
                        let found = usize::try_from(row)
                            .ok()
                            .and_then(|r| rows.get(r).map(|n| (r, *n)));
                        match found {
                            Some((r, (node, plays))) => {
                                allowed &= plays;
                                target_rows.push(r);
                                Some(node)
                            }
                            None => {
                                allowed = false;
                                Some(h.node)
                            }
                        }
                    };
                    (node, moved(&h.clip, shift))
                })
                .collect();
            target_rows.sort_unstable();
            target_rows.dedup();
            Plan {
                places,
                allowed,
                rows: target_rows,
            }
        }
        Grab::Start | Grab::End => {
            let edge = if grab == Grab::Start {
                anchor.start
            } else {
                end(anchor)
            };
            let shift = snap(edge + delta) - edge;
            let mut rows_hit: Vec<usize> = held.iter().map(|h| h.row).collect();
            rows_hit.sort_unstable();
            rows_hit.dedup();
            let places = held
                .iter()
                .map(|h| {
                    let clip = if grab == Grab::Start {
                        let start = (h.clip.start + shift)
                            .max(Ticks::ZERO)
                            .min(end(&h.clip) - MIN_LENGTH);
                        with_start(&h.clip, start)
                    } else {
                        let stop = (end(&h.clip) + shift).max(h.clip.start + MIN_LENGTH);
                        with_end(&h.clip, stop)
                    };
                    (Some(h.node), clip)
                })
                .collect();
            Plan {
                places,
                allowed: true,
                rows: rows_hit,
            }
        }
    }
}

/// "Bar.beat" for a song position, counted from 1 in 4/4: "3.2".
pub fn bar_beat(t: Ticks) -> String {
    let beats = t.as_beats();
    let bar = (beats / 4.).floor();
    let beat = beats - bar * 4.;
    let whole = beat.fract().abs() < 1e-9;
    if whole {
        format!("{}.{}", bar as i64 + 1, beat as i64 + 1)
    } else {
        format!("{}.{:.2}", bar as i64 + 1, beat + 1.)
    }
}

/// How long a new clip starting at `start` can be, up to `length`: it
/// stops where the next clip on the lane begins. None when `start` is
/// inside a clip or there is no room.
pub fn room_for_clip(clips: &[MidiClip], start: Ticks, length: Ticks) -> Option<Ticks> {
    if clips
        .iter()
        .any(|c| c.start <= start && start < c.start + c.length)
    {
        return None;
    }
    let next = clips.iter().map(|c| c.start).filter(|&s| s > start).min();
    let room = next.map_or(length, |n| (n - start).min(length));
    (room > Ticks::ZERO).then_some(room)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Command, Engine, EngineConfig, FIRST_NODE};

    fn beats(n: i64) -> Ticks {
        Ticks::from_beats(n)
    }

    fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: beats(start),
            length: beats(length),
            pitch,
            velocity: 100,
        }
    }

    /// Beats 4..12 with a note on every second beat.
    fn clip() -> MidiClip {
        MidiClip {
            start: beats(4),
            length: beats(8),
            notes: vec![
                note(0, 1, 60),
                note(2, 1, 62),
                note(4, 1, 64),
                note(6, 1, 65),
            ],
        }
    }

    fn pitches(clip: &MidiClip) -> Vec<u8> {
        clip.notes.iter().map(|n| n.pitch).collect()
    }

    #[test]
    fn cuts_trim_split_or_remove() {
        // Covering the start: trimmed to what is left.
        let left = cut(&clip(), beats(0), beats(6));
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].start, left[0].length), (beats(6), beats(6)));
        assert_eq!(pitches(&left[0]), [62, 64, 65]);
        assert_eq!(left[0].notes[0].start, beats(0), "the note keeps its place");
        // Inside: split in two.
        let split = cut(&clip(), beats(7), beats(9));
        assert_eq!(split.len(), 2);
        assert_eq!((split[0].start, end(&split[0])), (beats(4), beats(7)));
        assert_eq!((split[1].start, end(&split[1])), (beats(9), beats(12)));
        assert_eq!(pitches(&split[0]), [60, 62]);
        assert_eq!(pitches(&split[1]), [65]);
        // Covering it all: gone. Missing it: untouched.
        assert!(cut(&clip(), beats(0), beats(20)).is_empty());
        assert_eq!(cut(&clip(), beats(12), beats(20)), [clip()]);
    }

    #[test]
    fn edges_resize_without_moving_the_notes() {
        let shorter = with_start(&clip(), beats(6));
        assert_eq!((shorter.start, end(&shorter)), (beats(6), beats(12)));
        assert_eq!(
            shorter.notes[1].start,
            beats(0),
            "the second note stays at beat 6"
        );
        let longer = with_start(&clip(), beats(2));
        assert_eq!(
            longer.notes[0].start,
            beats(2),
            "the first note stays at beat 4"
        );
        let end_moved = with_end(&clip(), beats(16));
        assert_eq!(end_moved.length, beats(12));
        assert_eq!(end_moved.notes, clip().notes);
        assert_eq!(moved(&clip(), beats(4)).start, beats(8));
    }

    fn engine_with(clips: &[(u64, MidiClip)]) -> Engine {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        for (id, clip) in clips {
            engine
                .apply(Command::Edit(Edit::AddClip {
                    node: FIRST_NODE,
                    id: ClipId(*id),
                    clip: clip.clone(),
                }))
                .unwrap();
        }
        engine
    }

    fn spans(engine: &Engine) -> Vec<(u64, f64, f64)> {
        let snapshot = engine.snapshot();
        snapshot
            .node(FIRST_NODE)
            .unwrap()
            .clips
            .iter()
            .map(|(id, c)| (id.0, c.start.as_beats(), end(c).as_beats()))
            .collect()
    }

    #[test]
    fn a_dropped_clip_wins_and_is_one_undo() {
        let other = MidiClip {
            start: beats(0),
            length: beats(4),
            notes: vec![note(0, 1, 50)],
        };
        let mut engine = engine_with(&[(1, clip()), (2, other)]);
        // Clip 2 moves into the middle of clip 1, splitting it.
        let snapshot = engine.snapshot();
        let moving = moved(&snapshot.node(FIRST_NODE).unwrap().clips[1].1, beats(7));
        let (edit, landed) = land(
            &snapshot,
            &[Landing {
                id: Some(ClipId(2)),
                node: FIRST_NODE,
                clip: moving,
            }],
            &[ClipId(2)],
            Vec::new(),
        );
        engine.apply(Command::Edit(edit)).unwrap();
        assert_eq!(landed, [ClipId(2)]);
        assert_eq!(
            spans(&engine),
            [(1, 4.0, 7.0), (2, 7.0, 11.0), (3, 11.0, 12.0)]
        );
        engine.apply(Command::Undo).unwrap();
        assert_eq!(spans(&engine), [(1, 4.0, 12.0), (2, 0.0, 4.0)]);
    }

    #[test]
    fn copies_keep_the_original_and_take_new_ids() {
        let mut engine = engine_with(&[(1, clip())]);
        let snapshot = engine.snapshot();
        let (edit, landed) = land(
            &snapshot,
            &[Landing {
                id: None,
                node: FIRST_NODE,
                clip: moved(&clip(), beats(8)),
            }],
            &[],
            Vec::new(),
        );
        engine.apply(Command::Edit(edit)).unwrap();
        assert_eq!(landed, [ClipId(2)], "the copy's new id");
        assert_eq!(spans(&engine), [(1, 4.0, 12.0), (2, 12.0, 20.0)]);
    }

    const DRUMS: NodeId = NodeId(2);
    const BASS: NodeId = NodeId(3);

    /// Synth, Bass (both play), Drums (a folder), Song.
    fn rows() -> Vec<(NodeId, bool)> {
        vec![
            (FIRST_NODE, true),
            (BASS, true),
            (DRUMS, false),
            (NodeId::ROOT, false),
        ]
    }

    fn held(row: usize, node: NodeId, start: i64) -> Held {
        Held {
            id: ClipId(start as u64),
            node,
            row,
            clip: MidiClip {
                start: beats(start),
                ..clip()
            },
        }
    }

    /// Snaps to whole beats.
    fn to_beats(t: Ticks) -> Ticks {
        Ticks::from_beats(t.as_beats().round() as i64)
    }

    #[test]
    fn moving_snaps_the_grabbed_clip_and_keeps_the_others_in_step() {
        let held = [held(0, FIRST_NODE, 4), held(1, BASS, 6)];
        let plan = plan(
            &held,
            1,
            Grab::Body,
            Ticks::from_beats_f64(2.4),
            0,
            &rows(),
            to_beats,
        );
        let starts: Vec<f64> = plan
            .places
            .iter()
            .map(|(_, c)| c.start.as_beats())
            .collect();
        assert_eq!(starts, [6.0, 8.0]);
        assert!(plan.allowed);
        assert_eq!(plan.rows, [0, 1]);
    }

    #[test]
    fn nothing_moves_before_the_song_start() {
        let held = [held(0, FIRST_NODE, 4), held(0, FIRST_NODE, 10)];
        let plan = plan(&held, 1, Grab::Body, beats(-20), 0, &rows(), to_beats);
        assert_eq!(plan.places[0].1.start, Ticks::ZERO);
        assert_eq!(plan.places[1].1.start, beats(6));
    }

    #[test]
    fn rows_that_cannot_play_refuse_the_drop() {
        let one = [held(0, FIRST_NODE, 4)];
        let down = plan(&one, 0, Grab::Body, Ticks::ZERO, 1, &rows(), to_beats);
        assert_eq!(down.places[0].0, Some(BASS));
        assert!(down.allowed);
        let folder = plan(&one, 0, Grab::Body, Ticks::ZERO, 2, &rows(), to_beats);
        assert!(!folder.allowed, "a folder has no instrument");
        let above = plan(&one, 0, Grab::Body, Ticks::ZERO, -1, &rows(), to_beats);
        assert!(!above.allowed);
    }

    #[test]
    fn dropping_below_the_last_row_makes_a_new_node() {
        let one = [held(1, BASS, 4)];
        let below = plan(&one, 0, Grab::Body, Ticks::ZERO, 5, &rows(), to_beats);
        assert_eq!(below.places[0].0, None);
        assert!(below.allowed);
        assert_eq!(below.rows, [4]);
        let two_rows = [held(0, FIRST_NODE, 4), held(1, BASS, 4)];
        let mixed = plan(&two_rows, 0, Grab::Body, Ticks::ZERO, 4, &rows(), to_beats);
        assert!(!mixed.allowed, "only clips from one row make a new node");
    }

    #[test]
    fn edges_resize_every_held_clip_alike_and_never_to_nothing() {
        let held = [held(0, FIRST_NODE, 4), held(1, BASS, 4)];
        let longer = plan(&held, 0, Grab::End, beats(2), 0, &rows(), to_beats);
        assert!(longer.places.iter().all(|(_, c)| end(c) == beats(14)));
        let start = plan(&held, 0, Grab::Start, beats(100), 0, &rows(), to_beats);
        assert!(start.places.iter().all(|(_, c)| c.length == MIN_LENGTH));
        let early = plan(&held, 0, Grab::Start, beats(-100), 0, &rows(), to_beats);
        assert!(early.places.iter().all(|(_, c)| c.start == Ticks::ZERO));
    }

    #[test]
    fn positions_read_as_bar_and_beat() {
        assert_eq!(bar_beat(Ticks::ZERO), "1.1");
        assert_eq!(bar_beat(beats(5)), "2.2");
        assert_eq!(bar_beat(Ticks::from_beats_f64(2.5)), "1.3.50");
    }

    #[test]
    fn a_new_clip_fills_the_room_up_to_the_next_clip() {
        let lane = [
            MidiClip {
                start: beats(4),
                length: beats(4),
                notes: vec![],
            },
            MidiClip {
                start: beats(10),
                length: beats(2),
                notes: vec![],
            },
        ];
        assert_eq!(room_for_clip(&lane, beats(0), beats(4)), Some(beats(4)));
        assert_eq!(
            room_for_clip(&lane, beats(8), beats(4)),
            Some(beats(2)),
            "stops at the next clip"
        );
        assert_eq!(
            room_for_clip(&lane, beats(5), beats(4)),
            None,
            "inside a clip"
        );
        assert_eq!(
            room_for_clip(&lane, beats(12), beats(4)),
            Some(beats(4)),
            "after the last"
        );
    }
}
