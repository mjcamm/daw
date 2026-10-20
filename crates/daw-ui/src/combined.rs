//! Combined blocks on Arrange (`docs/design/note-flow.md`): a parent's lane
//! shows, wherever any node below it has clips, one block per run of
//! overlapping or touching clips, with their notes in a softer colour. The
//! parent's own clips draw solid on top as usual. Blocks are never stored.
//! Dragging a block moves every clip in it (cutting what they land on, as
//! clip drags do); Delete removes them; each one undo step.

use crate::clip_edit::{self, AnyClip, Landing};
use crate::project::ProjectView;
use crate::theme::Theme;
use crate::tooltip;
use crate::tree_editor::tree_nodes;
use daw_engine::{ClipId, Command, Edit, MidiClip, NodeId, Snapshot, Ticks};
use gpui::{
    AnyElement, Context, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, div, prelude::*, px,
};

const INSET: f32 = 4.;
const LABEL_HEIGHT: f32 = 16.;
/// Pixels the pointer moves before a press on a block becomes a drag.
const DRAG_THRESHOLD: f32 = 3.;

/// A run of clips below a node, as one block.
#[derive(Debug, Clone, PartialEq)]
pub struct CombinedBlock {
    pub start: Ticks,
    pub end: Ticks,
    /// The clips in it and the nodes they are on, in time order.
    pub clips: Vec<(NodeId, ClipId)>,
}

/// A MIDI clip and the node it is on.
pub type PlacedClip = (NodeId, ClipId, MidiClip);

/// Every MIDI clip in the project, where the snapshot has it.
pub fn all_clips(snapshot: &Snapshot) -> Vec<PlacedClip> {
    snapshot
        .nodes
        .iter()
        .flat_map(|n| n.clips.iter().map(|(id, c)| (n.id, *id, c.clone())))
        .collect()
}

/// The blocks for a node's lane: the clips of every node below it, merged
/// where they overlap or touch.
pub fn combined_blocks(snapshot: &Snapshot, node: NodeId) -> Vec<CombinedBlock> {
    combined_blocks_of(snapshot, node, &all_clips(snapshot))
}

/// The blocks for a node's lane from `clips` (the snapshot's, or where a
/// drag would put them, so blocks follow a drag as it happens).
pub fn combined_blocks_of(
    snapshot: &Snapshot,
    node: NodeId,
    clips: &[PlacedClip],
) -> Vec<CombinedBlock> {
    let below: Vec<NodeId> = tree_nodes(snapshot, node)
        .into_iter()
        .skip(1)
        .map(|(n, _)| n.id)
        .collect();
    let mut clips: Vec<(NodeId, ClipId, Ticks, Ticks)> = clips
        .iter()
        .filter(|(n, _, _)| below.contains(n))
        .map(|(n, id, c)| (*n, *id, c.start, c.start + c.length))
        .collect();
    clips.sort_by_key(|(_, id, start, _)| (*start, *id));
    let mut blocks: Vec<CombinedBlock> = Vec::new();
    for (owner, id, start, end) in clips {
        match blocks.last_mut() {
            Some(block) if start <= block.end => {
                block.end = block.end.max(end);
                block.clips.push((owner, id));
            }
            _ => blocks.push(CombinedBlock {
                start,
                end,
                clips: vec![(owner, id)],
            }),
        }
    }
    blocks
}

/// A note as it sounds: (start, end, pitch) in song time.
pub type HeardNote = (Ticks, Ticks, u8);

/// The notes a clip plays, in song time: only the
/// part of each note inside the clip, as the engine plays them. A clip
/// trimmed from either side keeps its outside notes to reveal again; they
/// do not sound, so blocks do not show them.
pub fn heard_notes(clip: &MidiClip) -> Vec<HeardNote> {
    clip.notes
        .iter()
        .filter_map(|n| {
            let start = n.start.max(Ticks::ZERO);
            let end = (n.start + n.length).min(clip.length);
            (start < end).then(|| (clip.start + start, clip.start + end, n.pitch))
        })
        .collect()
}

/// A combined block being dragged, previewed until release.
#[derive(Debug, Clone)]
pub(crate) struct CombinedDrag {
    node: NodeId,
    start: Ticks,
    clips: Vec<(NodeId, ClipId, MidiClip)>,
    origin: Point<Pixels>,
    pub shift: Ticks,
    started: bool,
}

impl CombinedDrag {
    /// Whether this drag holds `clip`.
    fn holds(&self, clip: ClipId) -> bool {
        self.started && self.clips.iter().any(|(_, id, _)| *id == clip)
    }

    /// Where a clip shows while dragged with its block.
    pub fn shifted(&self, clip: ClipId, at: &MidiClip) -> Option<MidiClip> {
        (self.started && self.clips.iter().any(|(_, id, _)| *id == clip)).then(|| MidiClip {
            start: at.start + self.shift,
            ..at.clone()
        })
    }
}

impl ProjectView {
    fn block_clips(&self, block: &CombinedBlock) -> Vec<(NodeId, ClipId, MidiClip)> {
        let snapshot = self.session.snapshot();
        block
            .clips
            .iter()
            .filter_map(|(node, id)| {
                let clip = snapshot.node(*node)?.clips.iter().find(|(c, _)| c == id)?;
                Some((*node, *id, clip.1.clone()))
            })
            .collect()
    }

    /// Every MIDI clip where it shows now: moved by a clip or block drag
    /// under way (copies get ids no clip has), so every parent's blocks
    /// follow the drag before it lands.
    pub(crate) fn shown_clips(&self) -> Vec<PlacedClip> {
        let snapshot = self.session.snapshot();
        let mut clips = all_clips(snapshot);
        if let Some(drag) = self.combined_drag.as_ref().filter(|d| d.started) {
            for (_, id, clip) in &mut clips {
                if drag.holds(*id) {
                    clip.start = clip.start + drag.shift;
                }
            }
        }
        if let Some((drag, plan)) = self.clip_plan() {
            let moving = drag.moving();
            clips.retain(|(_, id, _)| !moving.contains(id));
            for (index, (held, (node, clip))) in drag.held().iter().zip(&plan.places).enumerate() {
                if let (Some(node), AnyClip::Midi(clip)) = (node, clip) {
                    let id = if moving.contains(&held.id) {
                        held.id
                    } else {
                        ClipId(u64::MAX - index as u64)
                    };
                    clips.push((*node, id, clip.clone()));
                }
            }
        }
        clips
    }

    fn press_combined(&mut self, node: NodeId, block: &CombinedBlock, event: &MouseDownEvent) {
        self.selected_clips.clear();
        self.clear_note_selection();
        self.selected_combined = Some((node, block.start));
        self.combined_drag = Some(CombinedDrag {
            node,
            start: block.start,
            clips: self.block_clips(block),
            origin: event.position,
            shift: Ticks::ZERO,
            started: false,
        });
    }

    /// Follows a block drag; false if none is running.
    pub(crate) fn drag_combined(&mut self, position: Point<Pixels>, modifiers: Modifiers) -> bool {
        let snap = if modifiers.alt {
            self.snap.inverted()
        } else {
            self.snap
        };
        let ppb = self.arrange_view.zoom.pixels_per_beat;
        let Some(drag) = &mut self.combined_drag else {
            return false;
        };
        let dx = (position.x - drag.origin.x) / px(1.);
        if !drag.started && dx.abs() < DRAG_THRESHOLD {
            return true;
        }
        drag.started = true;
        let moved = Ticks::from_beats_f64(f64::from(dx / ppb));
        let start = snap.round(drag.start + moved).max(Ticks::ZERO);
        drag.shift = start - drag.start;
        true
    }

    /// Ends a block drag: every clip in it moves, cutting what it lands on.
    pub(crate) fn finish_combined_drag(&mut self) {
        let Some(drag) = self.combined_drag.take() else {
            return;
        };
        if !drag.started || drag.shift == Ticks::ZERO {
            return;
        }
        let landings: Vec<Landing> = drag
            .clips
            .iter()
            .map(|(node, id, clip)| Landing {
                id: Some(*id),
                node: *node,
                clip: clip_edit::AnyClip::Midi(MidiClip {
                    start: clip.start + drag.shift,
                    ..clip.clone()
                }),
            })
            .collect();
        let moving: Vec<ClipId> = drag.clips.iter().map(|(_, id, _)| *id).collect();
        let (edit, _) = clip_edit::land(self.session.snapshot(), &landings, &moving, Vec::new());
        self.session.apply(Command::Edit(edit));
        self.selected_combined = Some((drag.node, drag.start + drag.shift));
    }

    /// Deletes the selected block's clips; false if no block is selected.
    pub(crate) fn delete_combined(&mut self) -> bool {
        let Some((node, start)) = self.selected_combined.take() else {
            return false;
        };
        let Some(block) = combined_blocks(self.session.snapshot(), node)
            .into_iter()
            .find(|b| b.start == start)
        else {
            return false;
        };
        let edits = block
            .clips
            .iter()
            .map(|(_, id)| Edit::RemoveClip { id: *id })
            .collect();
        self.session.apply(Command::Edit(Edit::Batch(edits)));
        true
    }

    /// A parent's combined blocks, with the notes below in their nodes'
    /// colours, softer than the parent's own.
    /// `shown` is every clip as shown this frame (`shown_clips`), made once
    /// for all rows.
    pub(crate) fn combined_elements(
        &self,
        node: NodeId,
        shown: &[PlacedClip],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let drag = self
            .combined_drag
            .as_ref()
            .filter(|d| d.node == node && d.started);
        // Only blocks near the screen get elements, and only notes near the
        // screen are prepared (`paint::visible_x`).
        let near_x = crate::paint::visible_x(&self.arrange_view.x);
        let near = |start: Ticks, end: Ticks| {
            near_x.is_none_or(|(left, right)| {
                zoom.beat_to_x(end.as_beats()) >= left && zoom.beat_to_x(start.as_beats()) <= right
            })
        };
        let by_id: std::collections::HashMap<(NodeId, ClipId), &PlacedClip> =
            shown.iter().map(|c| ((c.0, c.1), c)).collect();
        combined_blocks_of(snapshot, node, shown)
            .into_iter()
            .enumerate()
            .filter(|(_, block)| {
                near_x.is_none_or(|(left, right)| {
                    zoom.beat_to_x(block.end.as_beats()) >= left
                        && zoom.beat_to_x(block.start.as_beats()) <= right
                })
            })
            .map(|(index, block)| {
                // The block being dragged stays marked as it moves.
                let dragged =
                    drag.is_some_and(|d| block.clips.iter().any(|(_, id)| d.holds(*id)));
                let selected = dragged || self.selected_combined == Some((node, block.start));
                let left = zoom.beat_to_x(block.start.as_beats());
                let width = zoom.beat_to_x((block.end - block.start).as_beats());
                // One row per pitch across the block, lowest at the bottom.
                let clips: Vec<&PlacedClip> = block
                    .clips
                    .iter()
                    .filter_map(|key| by_id.get(key).copied())
                    .collect();
                // The pitch span of the whole block, so rows stay put as it
                // scrolls; notes are prepared only for clips near the screen.
                let (low, high) = clips
                    .iter()
                    .flat_map(|(_, _, clip)| {
                        // The notes the clip plays, as `heard_notes` keeps them.
                        clip.notes
                            .iter()
                            .filter(|n| {
                                n.start.max(Ticks::ZERO) < (n.start + n.length).min(clip.length)
                            })
                            .map(|n| n.pitch)
                    })
                    .fold((u8::MAX, 0u8), |(l, h), p| (l.min(p), h.max(p)));
                let heard: Vec<(NodeId, Vec<HeardNote>)> = clips
                    .iter()
                    .filter(|(_, _, clip)| near(clip.start, clip.start + clip.length))
                    .map(|(owner, _, clip)| {
                        let mut notes = heard_notes(clip);
                        notes.retain(|(start, end, _)| near(*start, *end));
                        (*owner, notes)
                    })
                    .collect();
                let span = f32::from(high.saturating_sub(low)).max(1.);
                let area = (zoom.row_height - 2. * INSET - LABEL_HEIGHT - 6.).max(0.);
                let notes: Vec<crate::paint::Bar> = heard
                    .iter()
                    .flat_map(|(owner, notes)| {
                        let mut color = snapshot
                            .node(*owner)
                            .map_or(theme.text_tertiary, |n| theme.content(n.color).detail);
                        // Inherited: softer than the parent's own notes.
                        color.a *= 0.45;
                        notes
                            .iter()
                            .map(|&(start, end, pitch)| {
                                let at = start - block.start;
                                let row = 1. - f32::from(pitch.saturating_sub(low)) / span;
                                crate::paint::Bar {
                                    x: zoom.beat_to_x(at.as_beats()),
                                    y: LABEL_HEIGHT + row * area,
                                    w: (zoom.beat_to_x((end - start).as_beats()) - 1.).max(2.),
                                    h: 3.,
                                    color,
                                    radius: 1.,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                // Each node's name once, in the order its clips come.
                let mut names: Vec<String> = Vec::new();
                for (n, _) in &block.clips {
                    if let Some(n) = snapshot.node(*n)
                        && !names.contains(&n.name)
                    {
                        names.push(n.name.clone());
                    }
                }
                let mut fill = theme.control;
                fill.a *= 0.6;
                let press_block = block.clone();
                div()
                    .id(("combined", (node.0 as usize) << 16 | index))
                    .absolute()
                    .left(px(left))
                    .top(px(INSET))
                    .w(px(width))
                    .h(px(zoom.row_height - 2. * INSET))
                    .rounded(px(5.))
                    .bg(fill)
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.hairline })
                    .when(selected, |d| d.border_2())
                    .overflow_hidden()
                    .child(
                        div()
                            .px(px(6.))
                            .pt(px(2.))
                            .text_size(px(10.))
                            .text_color(theme.text_tertiary)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(names.join(" · ")),
                    )
                    .child(crate::paint::bars(notes).absolute().top_0().left_0().size_full())
                    .tooltip(tooltip::text(
                        "The clips below this node, combined: drag to move them all, double-click to edit",
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count >= 2 {
                                this.combined_drag = None;
                                this.open_node_editor(node, press_block.start.as_beats());
                                return;
                            }
                            this.selected_node = Some(node);
                            this.press_combined(node, &press_block, event);
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Engine, EngineConfig, Node};

    fn clip(start: i64, length: i64) -> MidiClip {
        MidiClip {
            start: Ticks::from_beats(start),
            length: Ticks::from_beats(length),
            notes: Vec::new(),
        }
    }

    #[test]
    fn a_trimmed_clip_shows_only_the_notes_it_plays() {
        let note = |start: i64, length: i64, pitch: u8| daw_engine::MidiNote {
            start: Ticks::from_beats(start),
            length: Ticks::from_beats(length),
            pitch,
            velocity: 100,
        };
        // Trimmed from the left by 4 beats: its first notes now sit before
        // its start, kept to reveal again.
        let clip = MidiClip {
            start: Ticks::from_beats(8),
            length: Ticks::from_beats(4),
            notes: vec![
                note(-4, 1, 60),
                note(-1, 2, 61),
                note(2, 4, 62),
                note(5, 1, 63),
            ],
        };
        let beats = Ticks::from_beats;
        assert_eq!(
            heard_notes(&clip),
            [(beats(8), beats(9), 61), (beats(10), beats(12), 62)],
            "hidden notes go; ones crossing an edge show their inside part"
        );
    }

    #[test]
    fn clips_below_merge_where_they_overlap_or_touch() {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        let mut parent = Node::new("Parent", 0);
        parent.children = vec![
            (NodeId(11), Node::new("A", 1)),
            (NodeId(12), Node::new("B", 2)),
        ];
        let apply = |engine: &mut Engine, edit| engine.apply(Command::Edit(edit)).unwrap();
        apply(
            &mut engine,
            Edit::AddNode {
                id: NodeId(10),
                parent: NodeId::ROOT,
                index: 0,
                node: parent,
            },
        );
        for (node, id, start, length) in
            [(11, 1, 0, 4), (12, 2, 4, 2), (11, 3, 10, 2), (10, 4, 20, 4)]
        {
            apply(
                &mut engine,
                Edit::AddClip {
                    node: NodeId(node),
                    id: ClipId(id),
                    clip: clip(start, length),
                },
            );
        }
        let blocks = combined_blocks(&engine.snapshot(), NodeId(10));
        assert_eq!(
            blocks,
            [
                CombinedBlock {
                    start: Ticks::from_beats(0),
                    end: Ticks::from_beats(6),
                    clips: vec![(NodeId(11), ClipId(1)), (NodeId(12), ClipId(2))],
                },
                CombinedBlock {
                    start: Ticks::from_beats(10),
                    end: Ticks::from_beats(12),
                    clips: vec![(NodeId(11), ClipId(3))],
                },
            ],
            "touching clips merge; the parent's own clip is not in its blocks"
        );
        // Where a drag would put clip 3 (touching the first block), the
        // blocks follow before it lands.
        let mut moved = all_clips(&engine.snapshot());
        for (_, id, clip) in &mut moved {
            if *id == ClipId(3) {
                clip.start = Ticks::from_beats(6);
            }
        }
        let blocks = combined_blocks_of(&engine.snapshot(), NodeId(10), &moved);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].end, Ticks::from_beats(8));
        assert_eq!(blocks[0].clips.len(), 3);
    }
}
