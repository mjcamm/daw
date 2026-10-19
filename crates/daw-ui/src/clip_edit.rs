//! Clips on Arrange: moving, copying and resizing them, and what a drop does
//! to the clips it lands on. Pure functions over clips and the snapshot, so
//! the rules are tested without a window.
//!
//! - Positions are song time in ticks. Moving a clip moves its notes with
//!   it; resizing an edge keeps every note where it is in the song.
//! - Clips are MIDI or audio ([`AnyClip`]). An audio clip is a window onto
//!   its file: its edges trim and reveal, never past the file's ends.
//! - A clip dropped (or resized) over others wins: the parts of other clips
//!   it covers are cut away, trimming, splitting or removing them.
//! - Everything one gesture does is one [`Edit::Batch`], so one undo.

use daw_engine::{AudioClip, ClipId, Edit, MidiClip, MidiNote, NodeId, NodeKind, Snapshot, Ticks};

/// A clip of either kind.
#[derive(Debug, Clone, PartialEq)]
pub enum AnyClip {
    Midi(MidiClip),
    Audio(AudioClip),
}

impl AnyClip {
    pub fn start(&self) -> Ticks {
        match self {
            Self::Midi(c) => c.start,
            Self::Audio(c) => c.start,
        }
    }

    pub fn length(&self) -> Ticks {
        match self {
            Self::Midi(c) => c.length,
            Self::Audio(c) => c.length,
        }
    }

    pub fn end(&self) -> Ticks {
        self.start() + self.length()
    }

    /// The kind of node that takes it.
    pub fn kind(&self) -> NodeKind {
        match self {
            Self::Midi(_) => NodeKind::Midi,
            Self::Audio(_) => NodeKind::Audio,
        }
    }

    /// Moved by `delta`, contents and all.
    pub fn moved(&self, delta: Ticks) -> Self {
        match self {
            Self::Midi(c) => Self::Midi(moved(c, delta)),
            Self::Audio(c) => Self::Audio(AudioClip {
                start: c.start + delta,
                ..c.clone()
            }),
        }
    }

    /// The part inside `from..to`, contents kept in song time.
    pub fn trimmed(&self, from: Ticks, to: Ticks, song_bpm: f64) -> Option<Self> {
        match self {
            Self::Midi(c) => trimmed(c, from, to).map(Self::Midi),
            Self::Audio(c) => {
                let (start, stop) = (from.max(c.start), to.min(c.end()));
                (start < stop).then(|| Self::Audio(c.windowed(start, stop, song_bpm)))
            }
        }
    }

    /// What is left after cutting out `from..to`.
    pub fn cut(&self, from: Ticks, to: Ticks, song_bpm: f64) -> Vec<Self> {
        if to <= self.start() || from >= self.end() {
            return vec![self.clone()];
        }
        [
            self.trimmed(self.start(), from, song_bpm),
            self.trimmed(to, self.end(), song_bpm),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// With its start edge at `start`, keeping its end and contents.
    pub fn with_start(&self, start: Ticks, song_bpm: f64) -> Self {
        match self {
            Self::Midi(c) => Self::Midi(with_start(c, start)),
            Self::Audio(c) => Self::Audio(c.windowed(start, c.end(), song_bpm)),
        }
    }

    /// With its end edge at `stop`.
    pub fn with_end(&self, stop: Ticks, song_bpm: f64) -> Self {
        match self {
            Self::Midi(c) => Self::Midi(with_end(c, stop)),
            Self::Audio(c) => Self::Audio(c.windowed(c.start, stop, song_bpm)),
        }
    }

    /// Cut in two at `at` (inside it).
    #[allow(
        dead_code,
        reason = "the scissors tool on Arrange, built next, cuts with it"
    )]
    pub fn split(&self, at: Ticks, song_bpm: f64) -> Option<(Self, Self)> {
        if at <= self.start() || at >= self.end() {
            return None;
        }
        Some(match self {
            Self::Midi(_) => (
                self.trimmed(self.start(), at, song_bpm)?,
                self.trimmed(at, self.end(), song_bpm)?,
            ),
            Self::Audio(c) => {
                let (left, right) = c.split(at, song_bpm);
                (Self::Audio(left), Self::Audio(right))
            }
        })
    }

    /// The edit that puts it on `node` as clip `id`, new or replacing.
    fn edit(&self, id: ClipId, node: NodeId, new: bool) -> Edit {
        match (self.clone(), new) {
            (Self::Midi(clip), true) => Edit::AddClip { node, id, clip },
            (Self::Midi(clip), false) => Edit::SetClip { id, node, clip },
            (Self::Audio(clip), true) => Edit::AddAudioClip { node, id, clip },
            (Self::Audio(clip), false) => Edit::SetAudioClip { id, node, clip },
        }
    }
}

/// Every clip on a node, either kind, in id order within each kind.
pub fn node_clips(node: &daw_engine::NodeSnapshot) -> impl Iterator<Item = (ClipId, AnyClip)> + '_ {
    node.clips
        .iter()
        .map(|(id, c)| (*id, AnyClip::Midi(c.clone())))
        .chain(
            node.audio_clips
                .iter()
                .map(|(id, c)| (*id, AnyClip::Audio(c.clone()))),
        )
}

/// How far a clip's edges can go: from the song start (or, for audio, the
/// file's start) to the file's end (`None`: no end).
pub fn reach(snapshot: &Snapshot, clip: &AnyClip) -> (Ticks, Option<Ticks>) {
    let AnyClip::Audio(c) = clip else {
        return (Ticks::ZERO, None);
    };
    let seconds = snapshot
        .samples
        .iter()
        .find(|s| s.id == c.sample)
        .and_then(|s| match s.status {
            daw_engine::SampleStatus::Loaded { seconds, .. } => Some(seconds),
            daw_engine::SampleStatus::Missing { .. } => None,
        });
    let Some(seconds) = seconds else {
        // Unread: the clip as it is is all that is known.
        return (c.start, Some(c.end()));
    };
    let (first, last) = c.file_span(seconds, snapshot.tempo_bpm);
    // Rounded inwards, so an edge never reaches past the audio.
    let ticks = |beats: f64| beats * daw_engine::TICKS_PER_BEAT as f64;
    (
        Ticks(ticks(first).ceil() as i64).max(Ticks::ZERO),
        Some(Ticks(ticks(last).floor() as i64)),
    )
}

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
    pub clip: AnyClip,
}

/// An id no clip (of either kind) uses yet.
pub fn next_clip_id(snapshot: &Snapshot) -> ClipId {
    ClipId(
        snapshot
            .nodes
            .iter()
            .flat_map(|n| {
                n.clips
                    .iter()
                    .map(|(id, _)| id.0)
                    .chain(n.audio_clips.iter().map(|(id, _)| id.0))
            })
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
    let bpm = snapshot.tempo_bpm;
    let mut edits = extra;
    for node in &snapshot.nodes {
        let covering: Vec<(Ticks, Ticks)> = landings
            .iter()
            .filter(|l| l.node == node.id)
            .map(|l| (l.clip.start(), l.clip.end()))
            .collect();
        if covering.is_empty() {
            continue;
        }
        for (id, clip) in node_clips(node).filter(|(id, _)| !moving.contains(id)) {
            let mut pieces = vec![clip.clone()];
            for &(from, to) in &covering {
                pieces = pieces.iter().flat_map(|p| p.cut(from, to, bpm)).collect();
            }
            if pieces.len() == 1 && pieces[0] == clip {
                continue;
            }
            let mut pieces = pieces.into_iter();
            match pieces.next() {
                Some(first) => edits.push(first.edit(id, node.id, false)),
                None => edits.push(Edit::RemoveClip { id }),
            }
            for piece in pieces {
                edits.push(piece.edit(fresh(), node.id, true));
            }
        }
    }
    let mut landed = Vec::with_capacity(landings.len());
    for landing in landings {
        let id = landing.id.unwrap_or_else(&mut fresh);
        landed.push(id);
        edits.push(landing.clip.edit(id, landing.node, landing.id.is_none()));
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
    pub clip: AnyClip,
    /// How far its edges can go ([`reach`]).
    pub reach: (Ticks, Option<Ticks>),
}

/// Where a drag would put the clips, and whether it can.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Each held clip's place, in the order held. Node is `None` for the
    /// new node a drop below the last row makes.
    pub places: Vec<(Option<NodeId>, AnyClip)>,
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
/// node and the kind of clips it takes, if any. `snap` places the grabbed
/// clip's moving edge; the others keep their offsets from it.
#[allow(clippy::too_many_arguments, reason = "the drag's state, spelled out")]
pub fn plan(
    held: &[Held],
    grabbed: usize,
    grab: Grab,
    delta: Ticks,
    row_shift: i64,
    rows: &[(NodeId, Option<NodeKind>)],
    snap: impl Fn(Ticks) -> Ticks,
    song_bpm: f64,
) -> Plan {
    let anchor = &held[grabbed].clip;
    match grab {
        Grab::Body => {
            let mut shift = snap(anchor.start() + delta) - anchor.start();
            // Nothing moves before the song start.
            let earliest = held
                .iter()
                .map(|h| h.clip.start())
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
                            Some((r, (node, takes))) => {
                                allowed &= takes == Some(h.clip.kind());
                                target_rows.push(r);
                                Some(node)
                            }
                            None => {
                                allowed = false;
                                Some(h.node)
                            }
                        }
                    };
                    (node, h.clip.moved(shift))
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
                anchor.start()
            } else {
                anchor.end()
            };
            let shift = snap(edge + delta) - edge;
            let mut rows_hit: Vec<usize> = held.iter().map(|h| h.row).collect();
            rows_hit.sort_unstable();
            rows_hit.dedup();
            let places = held
                .iter()
                .map(|h| {
                    let (first, last) = h.reach;
                    let clip = if grab == Grab::Start {
                        let start = (h.clip.start() + shift)
                            .max(first)
                            .max(Ticks::ZERO)
                            .min(h.clip.end() - MIN_LENGTH);
                        h.clip.with_start(start, song_bpm)
                    } else {
                        let mut stop = (h.clip.end() + shift).max(h.clip.start() + MIN_LENGTH);
                        if let Some(last) = last {
                            stop = stop.min(last.max(h.clip.start() + MIN_LENGTH));
                        }
                        h.clip.with_end(stop, song_bpm)
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

    fn pitches(clip: &AnyClip) -> Vec<u8> {
        let AnyClip::Midi(clip) = clip else {
            return Vec::new();
        };
        clip.notes.iter().map(|n| n.pitch).collect()
    }

    fn cut(clip: &MidiClip, from: Ticks, to: Ticks) -> Vec<AnyClip> {
        AnyClip::Midi(clip.clone()).cut(from, to, 120.0)
    }

    fn midi(clip: &AnyClip) -> &MidiClip {
        let AnyClip::Midi(clip) = clip else {
            unreachable!("a MIDI clip")
        };
        clip
    }

    #[test]
    fn cuts_trim_split_or_remove() {
        // Covering the start: trimmed to what is left.
        let left = cut(&clip(), beats(0), beats(6));
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].start(), left[0].length()), (beats(6), beats(6)));
        assert_eq!(pitches(&left[0]), [62, 64, 65]);
        assert_eq!(
            midi(&left[0]).notes[0].start,
            beats(0),
            "the note keeps its place"
        );
        // Inside: split in two.
        let split = cut(&clip(), beats(7), beats(9));
        assert_eq!(split.len(), 2);
        assert_eq!((split[0].start(), split[0].end()), (beats(4), beats(7)));
        assert_eq!((split[1].start(), split[1].end()), (beats(9), beats(12)));
        assert_eq!(pitches(&split[0]), [60, 62]);
        assert_eq!(pitches(&split[1]), [65]);
        // Covering it all: gone. Missing it: untouched.
        assert!(cut(&clip(), beats(0), beats(20)).is_empty());
        assert_eq!(cut(&clip(), beats(12), beats(20)), [AnyClip::Midi(clip())]);
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
                clip: AnyClip::Midi(moving),
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
                clip: AnyClip::Midi(moved(&clip(), beats(8))),
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

    const VOCALS: NodeId = NodeId(4);

    /// Synth, Bass (MIDI), Drums (takes nothing here), Vocals (audio), Song.
    fn rows() -> Vec<(NodeId, Option<NodeKind>)> {
        vec![
            (FIRST_NODE, Some(NodeKind::Midi)),
            (BASS, Some(NodeKind::Midi)),
            (DRUMS, None),
            (VOCALS, Some(NodeKind::Audio)),
            (NodeId::ROOT, None),
        ]
    }

    fn held(row: usize, node: NodeId, start: i64) -> Held {
        Held {
            id: ClipId(start as u64),
            node,
            row,
            clip: AnyClip::Midi(MidiClip {
                start: beats(start),
                ..clip()
            }),
            reach: (Ticks::ZERO, None),
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
            120.0,
        );
        let starts: Vec<f64> = plan
            .places
            .iter()
            .map(|(_, c)| c.start().as_beats())
            .collect();
        assert_eq!(starts, [6.0, 8.0]);
        assert!(plan.allowed);
        assert_eq!(plan.rows, [0, 1]);
    }

    #[test]
    fn nothing_moves_before_the_song_start() {
        let held = [held(0, FIRST_NODE, 4), held(0, FIRST_NODE, 10)];
        let plan = plan(
            &held,
            1,
            Grab::Body,
            beats(-20),
            0,
            &rows(),
            to_beats,
            120.0,
        );
        assert_eq!(plan.places[0].1.start(), Ticks::ZERO);
        assert_eq!(plan.places[1].1.start(), beats(6));
    }

    #[test]
    fn rows_that_cannot_play_refuse_the_drop() {
        let one = [held(0, FIRST_NODE, 4)];
        let down = plan(
            &one,
            0,
            Grab::Body,
            Ticks::ZERO,
            1,
            &rows(),
            to_beats,
            120.0,
        );
        assert_eq!(down.places[0].0, Some(BASS));
        assert!(down.allowed);
        let folder = plan(
            &one,
            0,
            Grab::Body,
            Ticks::ZERO,
            2,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(!folder.allowed, "a folder has no instrument");
        let above = plan(
            &one,
            0,
            Grab::Body,
            Ticks::ZERO,
            -1,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(!above.allowed);
        let audio = plan(
            &one,
            0,
            Grab::Body,
            Ticks::ZERO,
            3,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(!audio.allowed, "MIDI clips never land on an audio node");
    }

    #[test]
    fn dropping_below_the_last_row_makes_a_new_node() {
        let one = [held(1, BASS, 4)];
        let below = plan(
            &one,
            0,
            Grab::Body,
            Ticks::ZERO,
            5,
            &rows(),
            to_beats,
            120.0,
        );
        assert_eq!(below.places[0].0, None);
        assert!(below.allowed);
        assert_eq!(below.rows, [5]);
        let two_rows = [held(0, FIRST_NODE, 4), held(1, BASS, 4)];
        let mixed = plan(
            &two_rows,
            0,
            Grab::Body,
            Ticks::ZERO,
            5,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(!mixed.allowed, "only clips from one row make a new node");
    }

    #[test]
    fn edges_resize_every_held_clip_alike_and_never_to_nothing() {
        let held = [held(0, FIRST_NODE, 4), held(1, BASS, 4)];
        let longer = plan(&held, 0, Grab::End, beats(2), 0, &rows(), to_beats, 120.0);
        assert!(longer.places.iter().all(|(_, c)| c.end() == beats(14)));
        let start = plan(
            &held,
            0,
            Grab::Start,
            beats(100),
            0,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(start.places.iter().all(|(_, c)| c.length() == MIN_LENGTH));
        let early = plan(
            &held,
            0,
            Grab::Start,
            beats(-100),
            0,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(early.places.iter().all(|(_, c)| c.start() == Ticks::ZERO));
    }

    #[test]
    fn positions_read_as_bar_and_beat() {
        assert_eq!(bar_beat(Ticks::ZERO), "1.1");
        assert_eq!(bar_beat(beats(5)), "2.2");
        assert_eq!(bar_beat(Ticks::from_beats_f64(2.5)), "1.3.50");
    }

    /// Eight beats of a 120 BPM file from 2 s in, on the audio row.
    fn audio_held() -> Held {
        Held {
            id: ClipId(9),
            node: VOCALS,
            row: 3,
            clip: AnyClip::Audio(AudioClip {
                offset: 2.0,
                ..AudioClip::whole_file(daw_engine::SampleId(1), beats(8), 4.0, 120.0)
            }),
            // The 10 s file: its start would play at beat 4, its end at 24.
            reach: (beats(4), Some(beats(24))),
        }
    }

    #[test]
    fn audio_edges_trim_and_reveal_up_to_the_files_ends() {
        let held = [audio_held()];
        let out = plan(
            &held,
            0,
            Grab::Start,
            beats(-10),
            0,
            &rows(),
            to_beats,
            120.0,
        );
        let AnyClip::Audio(revealed) = &out.places[0].1 else {
            unreachable!()
        };
        assert_eq!(revealed.start, beats(4), "no further than the file's start");
        assert_eq!(revealed.offset, 0.0);
        let out = plan(&held, 0, Grab::End, beats(30), 0, &rows(), to_beats, 120.0);
        assert_eq!(out.places[0].1.end(), beats(24), "no further than its end");
        let out = plan(&held, 0, Grab::Start, beats(2), 0, &rows(), to_beats, 120.0);
        let AnyClip::Audio(trimmed) = &out.places[0].1 else {
            unreachable!()
        };
        assert_eq!(
            (trimmed.start, trimmed.offset),
            (beats(10), 3.0),
            "the audio stays put"
        );
    }

    #[test]
    fn audio_clips_land_only_on_audio_rows() {
        let held = [audio_held()];
        let up = plan(
            &held,
            0,
            Grab::Body,
            Ticks::ZERO,
            -2,
            &rows(),
            to_beats,
            120.0,
        );
        assert!(!up.allowed, "not on a MIDI node");
        let here = plan(&held, 0, Grab::Body, beats(1), 0, &rows(), to_beats, 120.0);
        assert!(here.allowed);
        assert_eq!(here.places[0].1.start(), beats(9));
    }

    #[test]
    fn splitting_an_audio_clip_leaves_two_windows_that_reveal_the_whole_file() {
        let AnyClip::Audio(whole) = audio_held().clip else {
            unreachable!()
        };
        let (left, right) = AnyClip::Audio(whole.clone())
            .split(beats(10), 120.0)
            .unwrap();
        let (AnyClip::Audio(left), AnyClip::Audio(right)) = (left, right) else {
            unreachable!()
        };
        assert_eq!(
            (left.end(), right.start, right.offset),
            (beats(10), beats(10), 3.0)
        );
        assert_eq!(left.file_span(10.0, 120.0), whole.file_span(10.0, 120.0));
        assert_eq!(right.file_span(10.0, 120.0), whole.file_span(10.0, 120.0));
        assert!(
            AnyClip::Audio(whole).split(beats(8), 120.0).is_none(),
            "not at an edge"
        );
    }
}
