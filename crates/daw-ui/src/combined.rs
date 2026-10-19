//! Combined blocks on Arrange (`docs/design/note-flow.md`): a parent's lane
//! shows, wherever any node below it has clips, one block per run of
//! overlapping or touching clips, with their notes in a softer colour. The
//! parent's own clips draw solid on top as usual. Blocks are never stored.
//! Dragging a block moves every clip in it (cutting what they land on, as
//! clip drags do); Delete removes them; each one undo step.

use crate::clip_edit::{self, Landing};
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

/// The blocks for a node's lane: the clips of every node below it, merged
/// where they overlap or touch.
pub fn combined_blocks(snapshot: &Snapshot, node: NodeId) -> Vec<CombinedBlock> {
    let mut clips: Vec<(NodeId, ClipId, Ticks, Ticks)> = tree_nodes(snapshot, node)
        .into_iter()
        .skip(1)
        .flat_map(|(n, _)| {
            n.clips
                .iter()
                .map(|(id, c)| (n.id, *id, c.start, c.start + c.length))
                .collect::<Vec<_>>()
        })
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

    fn press_combined(&mut self, node: NodeId, block: &CombinedBlock, event: &MouseDownEvent) {
        self.selected_clips.clear();
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
    pub(crate) fn combined_elements(
        &self,
        node: NodeId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let drag = self
            .combined_drag
            .as_ref()
            .filter(|d| d.node == node && d.started);
        combined_blocks(snapshot, node)
            .into_iter()
            .enumerate()
            .map(|(index, block)| {
                let dragged = drag.is_some_and(|d| d.start == block.start);
                let shift = if dragged {
                    drag.map_or(Ticks::ZERO, |d| d.shift)
                } else {
                    Ticks::ZERO
                };
                let selected = self.selected_combined == Some((node, block.start));
                let start = block.start + shift;
                let left = zoom.beat_to_x(start.as_beats());
                let width = zoom.beat_to_x((block.end - block.start).as_beats());
                // One row per pitch across the block, lowest at the bottom.
                let clips = self.block_clips(&block);
                let (low, high) = clips
                    .iter()
                    .flat_map(|(_, _, c)| c.notes.iter().map(|n| n.pitch))
                    .fold((u8::MAX, 0u8), |(l, h), p| (l.min(p), h.max(p)));
                let span = f32::from(high.saturating_sub(low)).max(1.);
                let area = (zoom.row_height - 2. * INSET - LABEL_HEIGHT - 6.).max(0.);
                let notes: Vec<AnyElement> = clips
                    .iter()
                    .flat_map(|(owner, _, clip)| {
                        let mut color = snapshot
                            .node(*owner)
                            .map_or(theme.text_tertiary, |n| theme.content(n.color).detail);
                        // Inherited: softer than the parent's own notes.
                        color.a *= 0.45;
                        clip.notes
                            .iter()
                            .filter(|n| n.start < clip.length)
                            .map(|n| {
                                let at = clip.start + n.start - block.start;
                                let end = (n.start + n.length).min(clip.length) - n.start;
                                let row = 1. - f32::from(n.pitch.saturating_sub(low)) / span;
                                div()
                                    .absolute()
                                    .left(px(zoom.beat_to_x(at.as_beats())))
                                    .top(px(LABEL_HEIGHT + row * area))
                                    .w(px((zoom.beat_to_x(end.as_beats()) - 1.).max(2.)))
                                    .h(px(3.))
                                    .rounded(px(1.))
                                    .bg(color)
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let names: Vec<String> = {
                    let mut names: Vec<String> = block
                        .clips
                        .iter()
                        .filter_map(|(n, _)| snapshot.node(*n).map(|n| n.name.clone()))
                        .collect();
                    names.dedup();
                    names
                };
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
                    .children(notes)
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
    }
}
