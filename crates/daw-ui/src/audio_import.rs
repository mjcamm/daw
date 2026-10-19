//! Importing audio files: what adding them to the song does, as one edit
//! (`docs/design/audio-clips.md`). Copying the files into the project is the
//! session's job; this decides where they go.

use crate::clip_edit::next_clip_id;
use crate::tree::{next_color, next_name, next_node_id};
use daw_engine::{AudioClip, Edit, Node, NodeId, NodeKind, SampleId, SampleRef, Snapshot, Ticks};

/// A file copied into the project, ready to add.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub sample: SampleRef,
    /// The file's length.
    pub seconds: f64,
}

/// Where files dragged over Arrange would go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileDrop {
    /// Into this audio node (its row index on screen).
    Into { node: NodeId, row: usize },
    /// Onto new audio nodes in `parent` from `index` on, shown as a line at
    /// the top of row `row` (one past the last row: below them all).
    NewNodes {
        parent: NodeId,
        index: usize,
        row: usize,
    },
    /// Nowhere: this row takes no audio.
    Refused { row: usize },
}

/// The edge zone of a row: a quarter of its height, at least 4 px and at
/// most 12 px, so it stays easy to hit at any zoom.
pub fn edge_zone(row_height: f32) -> f32 {
    (row_height / 4.).clamp(4., 12.)
}

/// What a drop at `y` (from the top of the first row) would do. `rows` are
/// the rows on screen in order, with their tops and heights.
pub fn file_drop(snapshot: &Snapshot, rows: &[(NodeId, f32, f32)], y: f32) -> FileDrop {
    // A gap between rows (or after the last): a new node takes the place
    // of the row below it, under the same parent.
    let gap = |row: usize| -> FileDrop {
        let Some(&(id, _, _)) = rows.get(row) else {
            return FileDrop::NewNodes {
                parent: NodeId::ROOT,
                index: snapshot.root().children.len(),
                row,
            };
        };
        let place = snapshot.node(id).and_then(|node| {
            let parent = snapshot.node(node.parent?)?;
            let index = parent.children.iter().position(|c| *c == id)?;
            Some((parent, index))
        });
        match place {
            // A branch is one kind: only the Song or an audio parent.
            Some((parent, index)) if parent.parent.is_none() || parent.kind == NodeKind::Audio => {
                FileDrop::NewNodes {
                    parent: parent.id,
                    index,
                    row,
                }
            }
            _ => FileDrop::Refused {
                row: row.saturating_sub(1),
            },
        }
    };
    let Some(row) = rows.iter().position(|&(_, top, height)| y < top + height) else {
        return gap(rows.len());
    };
    let (id, top, height) = rows[row];
    let zone = edge_zone(height);
    let is_song = row == 0 && snapshot.node(id).is_some_and(|n| n.parent.is_none());
    if y < top + zone && !is_song {
        return gap(row);
    }
    if y >= top + height - zone {
        return gap(row + 1);
    }
    match snapshot.node(id) {
        Some(node) if node.kind == NodeKind::Audio && node.parent.is_some() => {
            FileDrop::Into { node: id, row }
        }
        _ => FileDrop::Refused { row },
    }
}

/// A sample id no sample uses yet.
pub fn next_sample_id(snapshot: &Snapshot) -> SampleId {
    SampleId(
        snapshot
            .samples
            .iter()
            .map(|s| s.id.0)
            .max()
            .map_or(1, |h| h + 1),
    )
}

/// Adds each file on a new audio node of its own, named after the file,
/// placed in `parent` from `index` on, with its clip starting at `at`.
/// One edit, so one undo.
pub fn onto_new_nodes(
    snapshot: &Snapshot,
    files: &[Imported],
    parent: NodeId,
    index: usize,
    at: Ticks,
) -> Edit {
    let mut sample = next_sample_id(snapshot).0;
    let mut node = next_node_id(snapshot).0;
    let mut clip = next_clip_id(snapshot).0;
    // Names and colours count the nodes added so far, as if already there.
    let mut seen = snapshot.clone();
    let mut edits = Vec::new();
    for (i, file) in files.iter().enumerate() {
        let id = SampleId(sample);
        sample += 1;
        edits.push(Edit::AddSample {
            id,
            sample: file.sample.clone(),
        });
        let mut new = Node::new(&next_name(&seen, &file.sample.name), next_color(&seen))
            .with_kind(NodeKind::Audio);
        new.audio_clips.insert(
            daw_engine::ClipId(clip),
            AudioClip::whole_file(id, at, file.seconds, snapshot.tempo_bpm),
        );
        clip += 1;
        seen.nodes.push(placeholder(NodeId(node), &new));
        edits.push(Edit::AddNode {
            id: NodeId(node),
            parent,
            index: index + i,
            node: new,
        });
        node += 1;
    }
    Edit::Batch(edits)
}

/// Adds the files as clips on audio node `node`, one after another from
/// `at`. The new clips win over what they cover (see `clip_edit::land`).
pub fn into_node(snapshot: &Snapshot, files: &[Imported], node: NodeId, at: Ticks) -> Edit {
    let mut sample = next_sample_id(snapshot).0;
    let mut edits = Vec::new();
    let mut landings = Vec::new();
    let mut start = at;
    for file in files {
        let id = SampleId(sample);
        sample += 1;
        edits.push(Edit::AddSample {
            id,
            sample: file.sample.clone(),
        });
        let clip = AudioClip::whole_file(id, start, file.seconds, snapshot.tempo_bpm);
        start = clip.end();
        landings.push(crate::clip_edit::Landing {
            id: None,
            node,
            clip: crate::clip_edit::AnyClip::Audio(clip),
        });
    }
    crate::clip_edit::land(snapshot, &landings, &[], edits).0
}

/// Enough of a node for naming and colouring the next one.
fn placeholder(id: NodeId, node: &Node) -> daw_engine::NodeSnapshot {
    daw_engine::NodeSnapshot {
        id,
        kind: node.kind,
        parent: Some(NodeId::ROOT),
        depth: 1,
        children: Vec::new(),
        name: node.name.clone(),
        color: node.color,
        muted: false,
        soloed: false,
        clips: Vec::new(),
        audio_clips: Vec::new(),
        devices: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Command, Engine, EngineConfig};

    fn file(name: &str, seconds: f64) -> Imported {
        Imported {
            sample: SampleRef {
                name: name.to_owned(),
                file: format!("audio/{name}.wav"),
            },
            seconds,
        }
    }

    fn engine() -> Engine {
        Engine::new(EngineConfig::default()).0
    }

    #[test]
    fn each_file_gets_its_own_audio_node_at_the_playhead() {
        let mut engine = engine();
        let snapshot = engine.snapshot();
        let index = snapshot.root().children.len();
        let edit = onto_new_nodes(
            &snapshot,
            &[file("Kick", 2.0), file("Kick", 1.0)],
            NodeId::ROOT,
            index,
            Ticks::from_beats(8),
        );
        engine.apply(Command::Edit(edit)).unwrap();
        let snapshot = engine.snapshot();
        let added: Vec<_> = snapshot.root().children[index..]
            .iter()
            .map(|id| snapshot.node(*id).unwrap())
            .collect();
        assert_eq!(added.len(), 2);
        assert_eq!(added[0].name, "Kick");
        assert_eq!(added[1].name, "Kick 2", "names stay apart");
        assert_ne!(added[0].color, added[1].color, "and so do colours");
        assert!(added.iter().all(|n| n.kind == NodeKind::Audio));
        let clip = &added[0].audio_clips[0].1;
        // Two seconds at 120 BPM: four beats, from the playhead.
        assert_eq!(
            (clip.start, clip.length),
            (Ticks::from_beats(8), Ticks::from_beats(4))
        );
        assert_eq!(snapshot.samples.len(), 2);
        engine.apply(Command::Undo).unwrap();
        assert_eq!(
            engine.snapshot().samples.len(),
            0,
            "one undo takes it all back"
        );
    }

    #[test]
    fn edge_zones_scale_with_the_row_but_stay_easy_to_hit() {
        assert_eq!(edge_zone(16.), 4.);
        assert_eq!(edge_zone(32.), 8.);
        assert_eq!(edge_zone(200.), 12.);
    }

    /// Song (MIDI node "Synth" under it, then an audio node): rows of 40 px.
    fn song_with_audio() -> (Engine, NodeId) {
        let mut engine = engine();
        let snapshot = engine.snapshot();
        let index = snapshot.root().children.len();
        engine
            .apply(Command::Edit(onto_new_nodes(
                &snapshot,
                &[file("Loop", 1.0)],
                NodeId::ROOT,
                index,
                Ticks::ZERO,
            )))
            .unwrap();
        let audio = engine.snapshot().root().children[index];
        (engine, audio)
    }

    #[test]
    fn a_drop_goes_into_an_audio_row_or_between_rows() {
        let (engine, audio) = song_with_audio();
        let snapshot = engine.snapshot();
        let synth = daw_engine::FIRST_NODE;
        let rows = [
            (NodeId::ROOT, 0., 40.),
            (synth, 40., 40.),
            (audio, 80., 40.),
        ];
        let at = |y| file_drop(&snapshot, &rows, y);
        assert_eq!(
            at(100.),
            FileDrop::Into {
                node: audio,
                row: 2
            }
        );
        assert_eq!(
            at(60.),
            FileDrop::Refused { row: 1 },
            "a MIDI node takes no audio"
        );
        assert_eq!(
            at(20.),
            FileDrop::Refused { row: 0 },
            "nor does the Song itself"
        );
        // The line between the synth and the audio node: in its place.
        let between = FileDrop::NewNodes {
            parent: NodeId::ROOT,
            index: 1,
            row: 2,
        };
        assert_eq!(at(78.), between, "bottom quarter of the row above");
        assert_eq!(at(83.), between, "top quarter of the row below");
        // Below everything: the end of the Song.
        let end = FileDrop::NewNodes {
            parent: NodeId::ROOT,
            index: 2,
            row: 3,
        };
        assert_eq!(at(118.), end);
        assert_eq!(at(300.), end);
    }

    #[test]
    fn files_dropped_on_a_node_play_one_after_another_and_win() {
        let mut engine = engine();
        let snapshot = engine.snapshot();
        let index = snapshot.root().children.len();
        engine
            .apply(Command::Edit(onto_new_nodes(
                &snapshot,
                &[file("Loop", 4.0)],
                NodeId::ROOT,
                index,
                Ticks::ZERO,
            )))
            .unwrap();
        let snapshot = engine.snapshot();
        let node = snapshot.root().children[index];
        // Two one-second files from beat 2 cut into the eight-beat loop.
        let edit = into_node(
            &snapshot,
            &[file("A", 1.0), file("B", 1.0)],
            node,
            Ticks::from_beats(2),
        );
        engine.apply(Command::Edit(edit)).unwrap();
        let snapshot = engine.snapshot();
        let mut spans: Vec<(f64, f64)> = snapshot
            .node(node)
            .unwrap()
            .audio_clips
            .iter()
            .map(|(_, c)| (c.start.as_beats(), c.end().as_beats()))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(spans, [(0.0, 2.0), (2.0, 4.0), (4.0, 6.0), (6.0, 8.0)]);
    }
}
