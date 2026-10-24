//! Clips on Arrange in hand: selecting, moving, copying and resizing them.
//!
//! A press on a clip selects it (Shift adds or removes) and starts a drag
//! of the selection; a press near either end resizes instead. While the
//! drag runs the originals stay faintly in place, ghosts show where the
//! clips would land with the bar and beat, the receiving rows light up, and
//! rows that cannot take clips turn the ghosts red. Ctrl copies, Alt flips
//! snapping, Escape cancels, and the timeline scrolls near its edges. The
//! drop is one undo step (`clip_edit::land`). The rules live in
//! `clip_edit.rs`; this file is the hand on the mouse.

use crate::clip_edit::{self, Grab, Held, Landing, Plan};
use crate::project::ProjectView;
use crate::scroll::{self, Extent};
use crate::slider::Axis;
use crate::theme::Theme;
use crate::tree::{self, display_order};
use crate::zoom::Zoom;
use daw_engine::{ClipId, Command, Edit, MidiClip, Node, NodeId, NodeKind, Ticks};
use gpui::{AnyElement, FontWeight, Modifiers, MouseDownEvent, Pixels, Point, div, prelude::*, px};

/// Pixels the pointer moves before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 3.;
/// Width of the resize zone at each end of a clip.
use crate::clip_paint::{CLIP_INSET, CLIP_LABEL_HEIGHT};

/// A drag of the selected clips, previewed until release.
#[derive(Debug, Clone)]
pub struct ClipDrag {
    grab: Grab,
    held: Vec<Held>,
    grabbed: usize,
    origin: Point<Pixels>,
    pointer: Point<Pixels>,
    /// The timeline's scroll when the drag began, so scrolling during the
    /// drag counts as movement.
    scroll_start: Point<Pixels>,
    started: bool,
    copy: bool,
    flip_snap: bool,
    pixels_per_beat: f32,
    row_height: f32,
    /// Each row's node and the kind of clips it takes, in display order.
    rows: Vec<(NodeId, Option<NodeKind>)>,
}

impl ClipDrag {
    /// Which part of the clips the drag holds.
    pub fn grab(&self) -> Grab {
        self.grab
    }

    /// Where the pointer is.
    pub fn pointer(&self) -> Point<Pixels> {
        self.pointer
    }

    /// Whether the drag has moved far enough to count.
    pub fn started(&self) -> bool {
        self.started
    }

    /// The clips being moved (left faint in place), unless copying.
    /// The clips held, in the order the plan places them.
    pub fn held(&self) -> &[Held] {
        &self.held
    }

    pub fn moving(&self) -> Vec<ClipId> {
        if self.copy && self.grab == Grab::Body {
            Vec::new()
        } else {
            self.held.iter().map(|h| h.id).collect()
        }
    }
}

/// Rows in display order: each node and the kind of clips it takes (the
/// Song takes none).
fn row_kinds(rows: &[&daw_engine::NodeSnapshot]) -> Vec<(NodeId, Option<NodeKind>)> {
    rows.iter()
        .map(|n| (n.id, n.parent.is_some().then_some(n.kind)))
        .collect()
}

impl ProjectView {
    fn timeline_scroll(&self) -> Point<Pixels> {
        gpui::point(
            self.arrange_view.x.offset().x,
            self.arrange_view.y.offset().y,
        )
    }

    /// Starts a press on a clip: selects it and, unless Shift took it out
    /// of the selection, begins a drag of the selection.
    pub(crate) fn press_clip(&mut self, id: ClipId, grab: Grab, event: &MouseDownEvent) {
        self.selected_combined = None;
        self.clear_note_selection();
        if event.modifiers.shift {
            if !self.selected_clips.remove(&id) {
                self.selected_clips.insert(id);
            }
        } else if !self.selected_clips.contains(&id) {
            self.selected_clips.clear();
            self.selected_clips.insert(id);
        }
        if !self.selected_clips.contains(&id) {
            return;
        }
        let snapshot = self.session.snapshot();
        let shown = display_order(snapshot, &self.folded);
        let rows = row_kinds(&shown);
        let held: Vec<Held> = shown
            .iter()
            .enumerate()
            .flat_map(|(row, node)| {
                clip_edit::node_clips(node)
                    .filter(|(clip, _)| self.selected_clips.contains(clip))
                    .map(move |(clip, c)| Held {
                        id: clip,
                        node: node.id,
                        row,
                        reach: clip_edit::reach(snapshot, &c),
                        clip: c,
                    })
            })
            .collect();
        let Some(grabbed) = held.iter().position(|h| h.id == id) else {
            return;
        };
        self.clip_drag = Some(ClipDrag {
            grab,
            held,
            grabbed,
            origin: event.position,
            pointer: event.position,
            scroll_start: self.timeline_scroll(),
            started: false,
            copy: false,
            flip_snap: false,
            pixels_per_beat: self.arrange_view.zoom.pixels_per_beat,
            row_height: self.arrange_view.zoom.row_height,
            rows,
        });
    }

    /// Follows the pointer; false if no clip drag is running.
    pub(crate) fn drag_clips(&mut self, position: Point<Pixels>, modifiers: Modifiers) -> bool {
        let Some(drag) = &mut self.clip_drag else {
            return false;
        };
        drag.pointer = position;
        drag.copy = modifiers.secondary();
        drag.flip_snap = modifiers.alt;
        let moved = (position.x - drag.origin.x) / px(1.);
        let moved_y = (position.y - drag.origin.y) / px(1.);
        if moved.hypot(moved_y) >= DRAG_THRESHOLD {
            drag.started = true;
        }
        true
    }

    /// Where the clips would land now.
    pub(crate) fn clip_plan(&self) -> Option<(&ClipDrag, Plan)> {
        let drag = self.clip_drag.as_ref().filter(|d| d.started)?;
        let scrolled = drag.scroll_start - self.timeline_scroll();
        let dx = (drag.pointer.x - drag.origin.x + scrolled.x) / px(1.);
        let dy = (drag.pointer.y - drag.origin.y + scrolled.y) / px(1.);
        let delta = Ticks::from_beats_f64(f64::from(dx / drag.pixels_per_beat));
        let row_shift = if drag.grab == Grab::Body {
            (dy / drag.row_height).round() as i64
        } else {
            0
        };
        let snap = if drag.flip_snap {
            self.snap.inverted()
        } else {
            self.snap
        };
        let plan = clip_edit::plan(
            &drag.held,
            drag.grabbed,
            drag.grab,
            delta,
            row_shift,
            &drag.rows,
            |t| snap.round(t),
            self.session.snapshot().tempo_bpm,
        );
        Some((drag, plan))
    }

    /// The cursor for the whole window while a clip's end or a block's edge
    /// is held, so it stays a resize cursor when the pointer strays off the
    /// row.
    pub(crate) fn drag_cursor(&self) -> Option<gpui::CursorStyle> {
        let clip = self.clip_drag.as_ref().and_then(|drag| match drag.grab() {
            Grab::Start | Grab::End => Some(gpui::CursorStyle::ResizeLeftRight),
            Grab::Body | Grab::Loop => None,
        });
        clip.or_else(|| self.block_drag_cursor())
    }

    /// Ends a clip drag: lands the clips as one edit if they can go there.
    /// A press that never moved is a click: with the bottom panel open, it
    /// shows the clicked clip, and its node is the one selected (the panel
    /// follows the selected node).
    pub(crate) fn finish_clip_drag(&mut self) {
        let landing = self.clip_drag_landing();
        let clicked = self
            .clip_drag
            .as_ref()
            .filter(|drag| !drag.started)
            .map(|drag| {
                let held = &drag.held[drag.grabbed];
                (held.id, held.node, held.clip.kind())
            });
        self.clip_drag = None;
        if let Some((id, node, kind)) = clicked
            && (self.note_editor.is_some() || self.audio_editor.is_some())
        {
            self.selected_node = Some(node);
            match kind {
                NodeKind::Midi => self.open_note_editor(id),
                NodeKind::Audio => self.open_audio_clip(id),
            }
        }
        if let Some((edit, landed)) = landing {
            self.session.apply(Command::Edit(edit));
            // The clips just placed (copies included) are the selection.
            self.selected_clips = landed.into_iter().collect();
        }
    }

    /// The edit a clip drag makes if let go now, and the clips it places;
    /// none where the clips can't go.
    pub(crate) fn clip_drag_landing(&self) -> Option<(Edit, Vec<ClipId>)> {
        self.clip_plan()
            .filter(|(_, plan)| plan.allowed)
            .map(|(drag, plan)| {
                let snapshot = self.session.snapshot();
                let copy = drag.copy && drag.grab == Grab::Body;
                let new_node = tree::next_node_id(snapshot);
                let mut extra = Vec::new();
                if plan.places.iter().any(|(node, _)| node.is_none()) {
                    let source = snapshot.node(drag.held[0].node);
                    let name = source.map_or("Synth", |n| n.name.as_str());
                    let node =
                        Node::new(&tree::next_name(snapshot, name), tree::next_color(snapshot));
                    // A new node of the clips' kind: MIDI with a synth to
                    // play them, or audio.
                    let node = match drag.held[0].clip.kind() {
                        NodeKind::Midi => node.with_instrument(tree::next_device_id(snapshot)),
                        NodeKind::Audio => node.with_kind(NodeKind::Audio),
                    };
                    extra.push(Edit::AddNode {
                        id: new_node,
                        parent: NodeId::ROOT,
                        index: snapshot.root().children.len(),
                        node,
                    });
                }
                let landings: Vec<Landing> = drag
                    .held
                    .iter()
                    .zip(&plan.places)
                    .map(|(held, (node, clip))| Landing {
                        id: (!copy).then_some(held.id),
                        node: node.unwrap_or(new_node),
                        clip: clip.clone(),
                    })
                    .collect();
                let moving = drag.moving();
                clip_edit::land(snapshot, &landings, &moving, extra)
            })
    }

    /// Escape: drops a drag without changing anything.
    pub(crate) fn cancel_clip_drag(&mut self) -> bool {
        self.clip_drag.take().is_some()
    }

    /// Deletes the selected clips as one step; false if none are selected.
    pub(crate) fn delete_clips(&mut self) -> bool {
        if self.selected_clips.is_empty() {
            return false;
        }
        let edits = self
            .selected_clips
            .iter()
            .map(|id| Edit::RemoveClip { id: *id })
            .collect();
        self.selected_clips.clear();
        self.session.apply(Command::Edit(Edit::Batch(edits)));
        true
    }

    /// Opens the editor on a node at the time where its lane was
    /// double-clicked (drawing there makes the clips).
    pub(crate) fn open_editor_at(&mut self, node: NodeId, position: Point<Pixels>) {
        let view = &self.arrange_view;
        let bounds = view.x.bounds();
        let x = (position.x - bounds.left() - view.x.offset().x) / px(1.);
        let beat = view.zoom.x_to_beats(x).max(0.);
        // Open a little before the spot, on the bar.
        let start = (beat / 4.).floor() * 4.;
        self.open_node_editor(node, start);
    }

    /// Adds an empty clip where an empty lane of a MIDI node was
    /// double-clicked: a bar long from the grid line at or before the
    /// pointer (shorter if the next clip comes sooner), selected, with the
    /// editor opened on it.
    pub(crate) fn add_clip_at(&mut self, node: NodeId, position: Point<Pixels>) {
        let view = &self.arrange_view;
        let bounds = view.x.bounds();
        let x = (position.x - bounds.left() - view.x.offset().x) / px(1.);
        let at = Ticks::from_beats_f64(view.zoom.x_to_beats(x).max(0.));
        let start = self.snap.floor(at);
        let snapshot = self.session.snapshot();
        let Some(lane) = snapshot.node(node).filter(|n| n.takes_midi()) else {
            return;
        };
        let clips: Vec<MidiClip> = lane.clips.iter().map(|(_, c)| c.clone()).collect();
        let Some(length) = clip_edit::room_for_clip(&clips, start, Ticks::from_beats(4)) else {
            // No room here: just open the editor at the spot.
            self.open_editor_at(node, position);
            return;
        };
        let id = clip_edit::next_clip_id(snapshot);
        let clip = MidiClip {
            start,
            length,
            notes: Vec::new(),
            looping: None,
        };
        self.session
            .apply(Command::Edit(Edit::AddClip { node, id, clip }));
        self.selected_clips = [id].into();
        // The snapshot may not hold the new clip yet, so open by its node.
        self.open_node_editor(node, start.as_beats());
    }

    /// While dragging near the timeline's edges, scrolls it. Called every
    /// frame.
    pub(crate) fn autoscroll_clip_drag(&self) {
        let Some(drag) = self.clip_drag.as_ref().filter(|d| d.started) else {
            return;
        };
        for (handle, axis) in [
            (&self.arrange_view.x, Axis::Horizontal),
            (&self.arrange_view.y, Axis::Vertical),
        ] {
            let bounds = handle.bounds();
            let (pointer, low, high) = match axis {
                Axis::Horizontal => (drag.pointer.x, bounds.left(), bounds.right()),
                Axis::Vertical => (drag.pointer.y, bounds.top(), bounds.bottom()),
            };
            let step = scroll::edge_step(pointer / px(1.), low / px(1.), high / px(1.));
            if step != 0. {
                let extent = Extent::of(handle, axis);
                scroll::scroll_to(handle, axis, extent.scrolled + step);
            }
        }
    }

    /// A press on a painted clip (`clip_paint`): a double-click opens its
    /// node in the bottom panel (the piano roll, or the audio panel for an
    /// audio clip); otherwise it selects and picks up the clip, or the end
    /// it landed on to resize.
    pub(crate) fn press_painted_clip(
        &mut self,
        id: ClipId,
        grab: Grab,
        audio: bool,
        event: &MouseDownEvent,
    ) {
        if event.click_count >= 2 {
            self.clip_drag = None;
            if audio {
                self.open_audio_clip(id);
            } else {
                self.open_note_editor(id);
            }
            return;
        }
        self.press_clip(id, grab, event);
    }

    /// Each held clip as it would land, painted as the clip will look (a
    /// loop's repeats and cycle lines included), outlined in the warning
    /// colour where it can't go.
    pub(crate) fn drag_ghosts(
        &self,
        held: &[Held],
        plan: &Plan,
        theme: &Theme,
    ) -> Vec<crate::clip_paint::Painted> {
        let snapshot = self.session.snapshot();
        plan.places
            .iter()
            .zip(held)
            .map(|((_, clip), held)| {
                let source = snapshot.node(held.node);
                let colors = source.map_or(theme.content(0), |n| theme.content(n.color));
                let label = source.map_or("", |n| n.name.as_str());
                let mut ghost = self.clip_look(held.id, clip, label, colors, true, false, theme);
                ghost.loop_icon = false;
                if !plan.allowed {
                    ghost.outline = theme.record;
                }
                ghost
            })
            .collect()
    }

    /// The drag's preview over the lanes: receiving rows lit, ghosts where
    /// the clips would land, the grabbed one labelled with its bar and beat.
    /// `row_tops` gives each row's top; one past the last is the new-node
    /// row.
    pub(crate) fn clip_drag_overlay(&self, row_tops: &[f32], theme: &Theme) -> Vec<AnyElement> {
        let Some((drag, plan)) = self.clip_plan() else {
            return Vec::new();
        };
        let zoom = self.arrange_view.zoom;
        let row_of = |node: Option<NodeId>| match node {
            None => drag.rows.len(),
            Some(id) => drag.rows.iter().position(|(n, _)| *n == id).unwrap_or(0),
        };
        let top_of = |row: usize| {
            row_tops
                .get(row)
                .copied()
                .unwrap_or_else(|| row_tops.last().copied().unwrap_or(0.) + zoom.row_height)
        };
        let warn = theme.record;
        let mut tint = if plan.allowed { theme.accent } else { warn };
        tint.a = 0.07;
        let mut out: Vec<AnyElement> = plan
            .rows
            .iter()
            .map(|&row| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top_of(row)))
                    .h(px(zoom.row_height))
                    .bg(tint)
                    .when(row == drag.rows.len(), |d| {
                        // The new node a drop here makes.
                        d.border_t_1().border_b_1().border_color(theme.accent)
                    })
                    .into_any_element()
            })
            .collect();
        let ghosts = self.drag_ghosts(&drag.held, &plan, theme);
        for (index, (((node, clip), _), ghost)) in
            plan.places.iter().zip(&drag.held).zip(ghosts).enumerate()
        {
            let top = top_of(row_of(*node));
            // A solid backing, so the ghost reads as replacing what it
            // covers rather than mixing with it.
            let backing = div()
                .absolute()
                .left(px(zoom.beat_to_x(clip.start().as_beats())))
                .top(px(CLIP_INSET))
                .w(px(zoom.beat_to_x(clip.length().as_beats())))
                .h(px(zoom.row_height - 2. * CLIP_INSET))
                .rounded(px(5.))
                .bg(theme.window);
            out.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top))
                    .h(px(zoom.row_height))
                    .child(backing)
                    .child(
                        crate::clip_paint::clips_canvas(vec![ghost])
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                    )
                    .into_any_element(),
            );
            if index == drag.grabbed {
                let (at, text) = match drag.grab {
                    Grab::Body | Grab::Start => (clip.start(), clip_edit::bar_beat(clip.start())),
                    Grab::End => (clip.end(), clip_edit::bar_beat(clip.end())),
                    // How many times it plays, and where it ends.
                    Grab::Loop => {
                        let times = clip.looping().map_or(1, |l| l.pieces(clip.length()).len());
                        (
                            clip.end(),
                            format!("×{times} · {}", clip_edit::bar_beat(clip.end())),
                        )
                    }
                };
                let new_node = node.is_none();
                let text = if !plan.allowed {
                    "Can't go here".to_owned()
                } else if new_node {
                    format!("New node · {text}")
                } else if drag.copy && drag.grab == Grab::Body {
                    format!("Copy · {text}")
                } else {
                    text
                };
                out.push(
                    div()
                        .absolute()
                        .left(px(zoom.beat_to_x(at.as_beats())))
                        .top(px((top - 18.).max(0.)))
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(if plan.allowed { theme.accent } else { warn })
                        .text_color(theme.on_accent)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(text)
                        .into_any_element(),
                );
            }
        }
        out
    }
}

/// A tempo as typed: whole numbers plain, else to two places.
pub(crate) fn format_bpm(bpm: f64) -> String {
    if bpm.fract().abs() < 1e-9 {
        format!("{bpm:.0}")
    } else {
        format!("{bpm:.2}")
    }
}

/// A MIDI clip's notes as bars, from the clip's top-left: each at its row
/// in the clip's span of pitches, under the label.
pub(crate) fn note_bars(clip: &MidiClip, color: gpui::Hsla, zoom: Zoom) -> Vec<crate::paint::Bar> {
    let note_area = (zoom.row_height - 2. * CLIP_INSET - CLIP_LABEL_HEIGHT - 6.).max(0.);
    // The notes that play: the parts inside the clip, a loop's repeats too.
    let played = clip.played_notes();
    let lowest = played.iter().map(|n| n.pitch).min().unwrap_or(60);
    let highest = played.iter().map(|n| n.pitch).max().unwrap_or(60);
    let span = f32::from(highest - lowest).max(1.0);
    played
        .iter()
        .map(|note| crate::paint::Bar {
            x: zoom.beat_to_x(note.start.as_beats()),
            y: CLIP_LABEL_HEIGHT + (1.0 - f32::from(note.pitch - lowest) / span) * note_area,
            w: (zoom.beat_to_x(note.length.as_beats()) - 1.).max(2.),
            h: 3.,
            color,
            radius: 1.,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{ClipLoop, MidiNote};

    fn clip(pitches: &[u8], looping: Option<ClipLoop>, length: i64) -> MidiClip {
        MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_beats(length),
            notes: pitches
                .iter()
                .map(|&pitch| MidiNote {
                    start: Ticks::ZERO,
                    length: Ticks::from_beats(1),
                    pitch,
                    velocity: 100,
                })
                .collect(),
            looping,
        }
    }

    const ZOOM: Zoom = Zoom::new(24., 56.);

    #[test]
    fn notes_spread_over_the_pitch_range_highest_at_the_top() {
        let bars = note_bars(&clip(&[60, 72, 66], None, 4), gpui::Hsla::default(), ZOOM);
        let ys: Vec<f32> = bars.iter().map(|b| b.y).collect();
        let area = 56. - 2. * CLIP_INSET - CLIP_LABEL_HEIGHT - 6.;
        let (top, bottom) = (CLIP_LABEL_HEIGHT, CLIP_LABEL_HEIGHT + area);
        assert_eq!(ys.len(), 3);
        assert!(ys.contains(&top) && ys.contains(&bottom), "{ys:?}");
    }

    #[test]
    fn a_single_pitch_does_not_divide_by_zero() {
        let bars = note_bars(&clip(&[60, 60], None, 4), gpui::Hsla::default(), ZOOM);
        assert!(bars.iter().all(|b| b.y.is_finite()));
    }

    #[test]
    fn a_looped_clip_shows_its_notes_at_every_repeat() {
        let looping = Some(ClipLoop::new(Ticks::from_beats(2)));
        let bars = note_bars(&clip(&[60], looping, 6), gpui::Hsla::default(), ZOOM);
        let xs: Vec<f32> = bars.iter().map(|b| b.x).collect();
        assert_eq!(xs, vec![0., 48., 96.]);
    }
}
