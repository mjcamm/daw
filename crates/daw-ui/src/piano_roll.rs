//! The bottom editor panel, holding the piano roll: a clip's notes on a
//! grid of pitches over time, edited with the usual DAW tools.
//!
//! Double-click a clip on Arrange to open it. The panel slides up from the
//! bottom and resizes from its top edge; its header holds the tools (select,
//! pen, scissors), the grid and a close button. A keyboard runs down the
//! left (click a key to hear it) and a velocity lane along the bottom, and
//! the same frame as Arrange's (scrollbars with zoom sliders at their ends)
//! scrolls and zooms it: sideways is time, up and down the key height.
//! Every gesture is one undo step (`Edit::SetClip`); the rules live in
//! `note_edit.rs`. The panel is meant to hold an audio editor later too.

use crate::frame::Timeline;
use crate::icons::{Icon, icon};
use crate::inspector::note_name;
use crate::note_edit::{self, Part, Selection};
use crate::parameters::small_button;
use crate::project::ProjectView;
use crate::scroll;
use crate::slider::Axis;
use crate::theme::Theme;
use crate::tooltip;
use crate::zoom::Zoom;
use daw_engine::{ClipId, Command, Edit, MidiClip, MidiNote, NodeId, Ticks};
use gpui::{
    AnyElement, Bounds, Context, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, canvas,
    div, fill, point, prelude::*, px, size,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const DEFAULT_HEIGHT: f32 = 300.;
const MIN_HEIGHT: f32 = 160.;
const HEADER_HEIGHT: f32 = 30.;
const RULER_HEIGHT: f32 = 18.;
const KEYS_WIDTH: f32 = 52.;
const VELOCITY_HEIGHT: f32 = 56.;
/// The piano roll's zoom when first opened (the clip's fit then sets the
/// time zoom): 12 px keys, ranging 6 to 40.
pub const DEFAULT_ZOOM: Zoom = Zoom::new(48., 12.).with_rows(6., 40.);
/// A press on a note moves or resizes it only once the pointer has
/// travelled this far, so a click never nudges it.
const DEAD_ZONE: f32 = 3.;
/// How long a previewed note sounds.
const PREVIEW_TIME: Duration = Duration::from_millis(280);
/// The time zoom a clip is fitted within.
const FIT_ZOOM: (f32, f32) = (8., 400.);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Pen,
    Scissors,
}

/// A drag in the editor, previewed until release.
#[derive(Debug, Clone)]
enum NoteDrag {
    Move {
        origin: Point<Pixels>,
        grabbed: usize,
        original: Vec<MidiNote>,
        selection: Selection,
        copy: bool,
        shift: i32,
    },
    Resize {
        part: Part,
        grabbed: usize,
        origin: Point<Pixels>,
        original: Vec<MidiNote>,
        selection: Selection,
    },
    Box {
        from: Point<Pixels>,
        to: Point<Pixels>,
        original: Selection,
    },
    Pen {
        index: usize,
        origin: Point<Pixels>,
        original: Vec<MidiNote>,
    },
    Velocity {
        origin: Point<Pixels>,
        original: Vec<MidiNote>,
    },
    Height {
        origin: Point<Pixels>,
        original: f32,
    },
}

/// The roll's rows, top to bottom: every pitch, or (folded) only the notes
/// something plays and the pitches that already hold notes.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows(Vec<u8>);

impl Rows {
    pub fn all() -> Self {
        Self((0..=127u8).rev().collect())
    }

    /// `pitches` (any order), highest first.
    pub fn of(mut pitches: Vec<u8>) -> Self {
        pitches.sort_unstable_by(|a, b| b.cmp(a));
        pitches.dedup();
        if pitches.is_empty() {
            return Self::all();
        }
        Self(pitches)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn row_of(&self, pitch: u8) -> Option<usize> {
        self.0.iter().position(|&p| p == pitch)
    }

    /// The pitch on a row; rows past either end are the nearest row's.
    pub fn pitch_at(&self, row: f32) -> u8 {
        let index = (row.max(0.) as usize).min(self.0.len().saturating_sub(1));
        self.0.get(index).copied().unwrap_or(60)
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, u8)> + '_ {
        self.0.iter().copied().enumerate()
    }
}

/// An open editor: which clip, the tool, the selection and the view.
pub struct NoteEditor {
    pub node: NodeId,
    pub clip: ClipId,
    pub tool: Tool,
    pub selection: Selection,
    pub height: f32,
    /// Where the grid's content was drawn: its top-left is the clip start
    /// at pitch 127, and it moves as the grid scrolls.
    grid: Rc<Cell<Bounds<Pixels>>>,
    velocity: Rc<Cell<Bounds<Pixels>>>,
    fitted: bool,
    drag: Option<NoteDrag>,
    /// Notes as the drag would leave them.
    preview: Option<(Vec<MidiNote>, Selection)>,
    /// Opened at, for the slide-up.
    opened: Instant,
    /// Where the grid's content was when the drag began. Drags measure in
    /// that frame, so scrolling while dragging (auto-scroll near the edges)
    /// moves the notes with the view.
    drag_grid: Point<Pixels>,
    /// The pointer and keys during a drag, to follow while auto-scrolling.
    pointer: Option<(Point<Pixels>, gpui::Modifiers)>,
    /// Opened on a clip just added, which the snapshot does not show yet:
    /// until it does, its absence does not close the editor.
    waiting: bool,
    /// The rows as last drawn.
    pub rows: Rows,
    /// Folded to the notes that play; `None` folds when the note map is
    /// limited (a drum kit) and not for full-range instruments.
    pub folded: Option<bool>,
    /// Where the pointer is over the grid, for the pen's preview note.
    hover: Option<Point<Pixels>>,
}

impl NoteEditor {
    pub fn new(node: NodeId, clip: ClipId, height: f32) -> Self {
        Self {
            node,
            clip,
            tool: Tool::Select,
            selection: Selection::new(),
            height,
            grid: Rc::default(),
            velocity: Rc::default(),
            fitted: false,
            drag: None,
            preview: None,
            opened: Instant::now(),
            drag_grid: Point::default(),
            pointer: None,
            waiting: false,
            rows: Rows::all(),
            folded: None,
            hover: None,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The pointer as (beat from the clip start, pitch).
    fn at(&self, position: Point<Pixels>, zoom: Zoom) -> (f64, u8) {
        self.at_from(self.grid.get().origin, position, zoom)
    }

    /// A point stored during a drag (in the frame it began in) as (beat,
    /// pitch).
    fn at_press(&self, position: Point<Pixels>, zoom: Zoom) -> (f64, u8) {
        self.at_from(self.drag_grid, position, zoom)
    }

    fn at_from(&self, origin: Point<Pixels>, position: Point<Pixels>, zoom: Zoom) -> (f64, u8) {
        let x = (position.x - origin.x) / px(1.);
        let y = (position.y - origin.y) / px(1.);
        let beat = f64::from(x / zoom.pixels_per_beat);
        let pitch = self.rows.pitch_at((y / zoom.row_height).floor());
        (beat, pitch)
    }
}

fn is_black(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

impl ProjectView {
    /// The clip open in the editor, if it still exists.
    fn editor_clip(&self) -> Option<(&NoteEditor, MidiClip)> {
        let editor = self.note_editor.as_ref()?;
        let node = self.session.snapshot().node(editor.node)?;
        let (_, clip) = node.clips.iter().find(|(id, _)| *id == editor.clip)?;
        Some((editor, clip.clone()))
    }

    /// Where the open clip starts, in beats.
    pub(crate) fn note_editor_start(&self) -> Option<f32> {
        self.editor_clip()
            .map(|(_, clip)| clip.start.as_beats() as f32)
    }

    /// Opens a clip just added to `node` (not in the snapshot yet).
    pub(crate) fn open_new_clip(&mut self, node: NodeId, clip: ClipId) {
        let mut editor = NoteEditor::new(node, clip, DEFAULT_HEIGHT);
        if let Some(open) = &self.note_editor {
            editor.height = open.height;
            editor.tool = open.tool;
            editor.opened = open.opened;
        }
        editor.waiting = true;
        self.note_editor = Some(editor);
    }

    /// Opens a clip's notes in the bottom panel.
    pub(crate) fn open_note_editor(&mut self, clip: ClipId) {
        let snapshot = self.session.snapshot();
        let Some(node) = snapshot
            .nodes
            .iter()
            .find(|n| n.clips.iter().any(|(id, _)| *id == clip))
        else {
            return;
        };
        // Already open: nothing to do. Another clip: swap it in without
        // sliding the panel up again.
        let mut editor = NoteEditor::new(node.id, clip, DEFAULT_HEIGHT);
        if let Some(open) = &self.note_editor {
            if open.clip == clip {
                return;
            }
            editor.height = open.height;
            editor.tool = open.tool;
            editor.opened = open.opened;
        }
        self.note_editor = Some(editor);
    }

    pub(crate) fn close_note_editor(&mut self) {
        self.note_editor = None;
    }

    /// Plays a note briefly on the clip's node.
    fn preview_note(&mut self, node: NodeId, pitch: u8, velocity: u8) {
        if let Some((node, pitch, _)) = self.note_preview.take() {
            self.session.apply(Command::PreviewNote {
                node,
                pitch,
                velocity: 0,
                on: false,
            });
        }
        self.session.apply(Command::PreviewNote {
            node,
            pitch,
            velocity,
            on: true,
        });
        self.note_preview = Some((node, pitch, Instant::now()));
    }

    /// Ends a previewed note once it has sounded. Called every frame.
    pub(crate) fn end_note_preview(&mut self) {
        if let Some((node, pitch, at)) = self.note_preview
            && at.elapsed() >= PREVIEW_TIME
        {
            self.note_preview = None;
            self.session.apply(Command::PreviewNote {
                node,
                pitch,
                velocity: 0,
                on: false,
            });
        }
    }

    /// Replaces the open clip's notes as one edit, and the selection.
    fn commit_notes(&mut self, notes: Vec<MidiNote>, selection: Selection) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        if notes == clip.notes {
            return;
        }
        let edit = Edit::SetClip {
            id: editor.clip,
            node: editor.node,
            clip: MidiClip {
                notes,
                ..clip.clone()
            },
        };
        if let Some(editor) = &mut self.note_editor {
            editor.selection = selection;
        }
        self.session.apply(Command::Edit(edit));
    }

    /// The grid step for new notes and snapping.
    fn note_snap(&self, flip: bool) -> crate::block_edit::Snap {
        if flip {
            self.note_grid.inverted()
        } else {
            self.note_grid
        }
    }

    fn grid_length(&self) -> Ticks {
        self.note_grid.grid.ticks().max(note_edit::MIN_LENGTH)
    }

    /// A press on the grid, with the current tool.
    fn press_grid(&mut self, event: &MouseDownEvent) {
        if let Some(editor) = &mut self.note_editor {
            editor.drag_grid = editor.grid.get().origin;
            editor.pointer = None;
        }
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        let zoom = self.notes_view.zoom;
        let (beat, pitch) = editor.at(event.position, zoom);
        let notes = clip.notes.clone();
        let hit = note_edit::hit(&notes, beat, pitch, f64::from(zoom.pixels_per_beat));
        let (node, tool) = (editor.node, editor.tool);
        let selection = editor.selection.clone();
        let snap = self.note_snap(event.modifiers.alt);
        let at = Ticks::from_beats_f64(beat.max(0.));
        let velocity = 100;
        match tool {
            Tool::Select if event.click_count >= 2 => match hit {
                Some((index, _)) => {
                    self.commit_notes(
                        note_edit::deleted(&notes, &[index].into()),
                        Selection::new(),
                    );
                }
                None => {
                    let note = MidiNote {
                        start: snap.floor(at),
                        length: self.grid_length(),
                        pitch,
                        velocity,
                    };
                    let (notes, index) = note_edit::added(&notes, note);
                    self.preview_note(node, pitch, velocity);
                    self.commit_notes(notes, [index].into());
                }
            },
            Tool::Select => {
                let Some(editor) = &mut self.note_editor else {
                    return;
                };
                match hit {
                    Some((index, part)) => {
                        if event.modifiers.shift {
                            if !editor.selection.remove(&index) {
                                editor.selection.insert(index);
                            }
                        } else if !editor.selection.contains(&index) {
                            editor.selection = [index].into();
                        }
                        if !editor.selection.contains(&index) {
                            return;
                        }
                        let selection = editor.selection.clone();
                        editor.drag = Some(match part {
                            Part::Body => NoteDrag::Move {
                                origin: event.position,
                                grabbed: index,
                                original: notes.clone(),
                                selection,
                                copy: event.modifiers.secondary(),
                                shift: 0,
                            },
                            part => NoteDrag::Resize {
                                part,
                                grabbed: index,
                                origin: event.position,
                                original: notes.clone(),
                                selection,
                            },
                        });
                        let pitch = notes[index].pitch;
                        let velocity = notes[index].velocity;
                        self.preview_note(node, pitch, velocity);
                    }
                    None => {
                        let original = if event.modifiers.shift {
                            editor.selection.clone()
                        } else {
                            Selection::new()
                        };
                        editor.selection = original.clone();
                        editor.drag = Some(NoteDrag::Box {
                            from: event.position,
                            to: event.position,
                            original,
                        });
                    }
                }
            }
            Tool::Pen => match hit {
                Some((index, _)) => {
                    let remaining: Selection =
                        selection.iter().filter(|&&i| i != index).copied().collect();
                    let _ = remaining;
                    self.commit_notes(
                        note_edit::deleted(&notes, &[index].into()),
                        Selection::new(),
                    );
                }
                None => {
                    let note = MidiNote {
                        start: snap.floor(at),
                        length: self.grid_length(),
                        pitch,
                        velocity,
                    };
                    let (with, index) = note_edit::added(&notes, note);
                    self.preview_note(node, pitch, velocity);
                    if let Some(editor) = &mut self.note_editor {
                        editor.selection = [index].into();
                        editor.preview = Some((with.clone(), [index].into()));
                        editor.drag = Some(NoteDrag::Pen {
                            index,
                            origin: event.position,
                            original: with,
                        });
                    }
                }
            },
            Tool::Scissors => {
                if let Some((index, _)) = hit
                    && let Some((notes, second)) = note_edit::split(&notes, index, snap.round(at))
                {
                    self.commit_notes(notes, [index, second].into());
                }
            }
        }
    }

    /// A press in the velocity lane: drags the velocity of the note at the
    /// pointer, with the rest of the selection if it is selected.
    fn press_velocity(&mut self, event: &MouseDownEvent) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        let origin = editor.velocity.get().origin;
        let x = (event.position.x - origin.x) / px(1.);
        let ppb = self.notes_view.zoom.pixels_per_beat;
        let tolerance = 6. / ppb;
        let beat = f64::from(x / ppb);
        let found = clip
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| (n.start.as_beats() - beat).abs() <= f64::from(tolerance))
            .min_by(|(_, a), (_, b)| {
                (a.start.as_beats() - beat)
                    .abs()
                    .total_cmp(&(b.start.as_beats() - beat).abs())
            })
            .map(|(i, _)| i);
        let notes = clip.notes.clone();
        let Some(index) = found else {
            return;
        };
        if let Some(editor) = &mut self.note_editor {
            if !editor.selection.contains(&index) {
                editor.selection = [index].into();
            }
            editor.drag = Some(NoteDrag::Velocity {
                origin: event.position,
                original: notes,
            });
        }
    }

    /// Follows an editor drag; false if none is running.
    pub(crate) fn drag_note_editor(
        &mut self,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
    ) -> bool {
        let snap = self.note_snap(modifiers.alt);
        let grid = self.grid_length();
        let Some(editor) = &mut self.note_editor else {
            return false;
        };
        if editor.drag.is_none() {
            return false;
        }
        let zoom = self.notes_view.zoom;
        let ppb = zoom.pixels_per_beat;
        let beats = |dx: Pixels| Ticks::from_beats_f64(f64::from(dx / px(1.) / ppb));
        // The pitch is the row under the pointer now; distances are taken in
        // the frame the drag began in, so scrolling carries the notes along.
        let (_, pointer_pitch) = editor.at(position, zoom);
        editor.pointer = Some((position, modifiers));
        let screen = position;
        let position = position + (editor.drag_grid - editor.grid.get().origin);
        let Some(drag) = &mut editor.drag else {
            return false;
        };
        let still = |origin: Point<Pixels>| {
            let d = position - origin;
            (d.x / px(1.)).abs() < DEAD_ZONE && (d.y / px(1.)).abs() < DEAD_ZONE
        };
        let mut sound = None;
        match drag {
            NoteDrag::Move {
                origin,
                grabbed,
                original,
                selection,
                copy,
                shift,
            } => {
                *copy = modifiers.secondary();
                if editor.preview.is_none() && still(*origin) {
                    return true;
                }
                let anchor = original[*grabbed];
                // The grabbed note's start lands on the grid; its pitch is
                // the row under the pointer.
                let dt = snap.round(anchor.start + beats(position.x - origin.x)) - anchor.start;
                let dpitch = i32::from(pointer_pitch) - i32::from(anchor.pitch);
                let (notes, chosen) = if *copy {
                    note_edit::copied(original, selection, dt, dpitch)
                } else {
                    (
                        note_edit::moved(original, selection, dt, dpitch),
                        selection.clone(),
                    )
                };
                if dpitch != *shift {
                    *shift = dpitch;
                    let moved_to = if *copy {
                        chosen
                            .iter()
                            .nth(selection.iter().position(|i| i == grabbed).unwrap_or(0))
                            .copied()
                    } else {
                        Some(*grabbed)
                    };
                    if let Some(i) = moved_to {
                        sound = Some((notes[i].pitch, notes[i].velocity));
                    }
                }
                editor.preview = Some((notes, chosen));
            }
            NoteDrag::Resize {
                part,
                grabbed,
                origin,
                original,
                selection,
            } => {
                if editor.preview.is_none() && still(*origin) {
                    return true;
                }
                let anchor = original[*grabbed];
                let notes = if *part == Part::End {
                    let edge = anchor.start + anchor.length;
                    let delta = snap.round(edge + beats(position.x - origin.x)) - edge;
                    note_edit::resized_end(original, selection, delta)
                } else {
                    let delta =
                        snap.round(anchor.start + beats(position.x - origin.x)) - anchor.start;
                    note_edit::resized_start(original, selection, delta)
                };
                editor.preview = Some((notes, selection.clone()));
            }
            NoteDrag::Box { to, .. } => {
                *to = position;
            }
            NoteDrag::Pen {
                index,
                origin,
                original,
            } => {
                let note = original[*index];
                let stop = snap.round(note.start + note.length + beats(position.x - origin.x));
                let mut notes = original.clone();
                notes[*index].length = (stop - note.start).max(grid.min(note.length));
                editor.preview = Some((notes, [*index].into()));
            }
            NoteDrag::Velocity { origin, original } => {
                let delta = (-(screen.y - origin.y) / px(1.) / VELOCITY_HEIGHT * 127.).round();
                let notes =
                    note_edit::with_velocity(original, &editor.selection, original, delta as i32);
                editor.preview = Some((notes, editor.selection.clone()));
            }
            NoteDrag::Height { origin, original } => {
                editor.height = (*original - (screen.y - origin.y) / px(1.)).max(MIN_HEIGHT);
            }
        }
        let node = editor.node;
        if let Some((pitch, velocity)) = sound {
            self.preview_note(node, pitch, velocity);
        }
        true
    }

    /// Ends an editor drag, committing its notes as one edit.
    pub(crate) fn finish_note_drag(&mut self) {
        let Some(editor) = &mut self.note_editor else {
            return;
        };
        let Some(drag) = editor.drag.take() else {
            return;
        };
        let preview = editor.preview.take();
        if let NoteDrag::Box { from, to, original } = drag {
            let zoom = self.notes_view.zoom;
            let (b0, p0) = editor.at_press(from, zoom);
            let (b1, p1) = editor.at_press(to, zoom);
            if let Some((_, clip)) = self.editor_clip() {
                let mut chosen = note_edit::in_box(&clip.notes, b0, b1, p0, p1);
                chosen.extend(original);
                if let Some(editor) = &mut self.note_editor {
                    editor.selection = chosen;
                }
            }
            return;
        }
        if let Some((notes, selection)) = preview {
            self.commit_notes(notes, selection);
        }
    }

    /// The selection as a box drag would leave it, while it runs.
    fn boxed(&self, editor: &NoteEditor, notes: &[MidiNote]) -> Option<Selection> {
        let Some(NoteDrag::Box { from, to, original }) = &editor.drag else {
            return None;
        };
        let zoom = self.notes_view.zoom;
        let (b0, p0) = editor.at_press(*from, zoom);
        let (b1, p1) = editor.at_press(*to, zoom);
        let mut chosen = note_edit::in_box(notes, b0, b1, p0, p1);
        chosen.extend(original.iter().copied());
        Some(chosen)
    }

    /// While dragging notes near the grid's edges, scrolls it and follows
    /// the drag. Called every frame.
    pub(crate) fn autoscroll_note_drag(&mut self) {
        let Some(editor) = &self.note_editor else {
            return;
        };
        let scrolls = matches!(
            editor.drag,
            Some(
                NoteDrag::Move { .. }
                    | NoteDrag::Resize { .. }
                    | NoteDrag::Pen { .. }
                    | NoteDrag::Box { .. }
            )
        );
        let Some((pointer, modifiers)) = editor.pointer.filter(|_| scrolls) else {
            return;
        };
        let handle = &self.notes_view.x;
        let bounds = handle.bounds();
        let mut moved = false;
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (at, low, high) = match axis {
                Axis::Horizontal => (pointer.x, bounds.left(), bounds.right()),
                Axis::Vertical => (pointer.y, bounds.top(), bounds.bottom()),
            };
            let step = scroll::edge_step(at / px(1.), low / px(1.), high / px(1.));
            if step != 0. {
                let extent = scroll::Extent::of(handle, axis);
                let max = (extent.content - extent.view).max(0.);
                let scrolled = (extent.scrolled + step).clamp(0., max);
                if scrolled != extent.scrolled {
                    scroll::scroll_to(handle, axis, scrolled);
                    moved = true;
                }
            }
        }
        if moved {
            self.drag_note_editor(pointer, modifiers);
        }
    }

    /// Moves the selected notes' starts (every note's, with none selected)
    /// to the nearest line of the editor's grid.
    pub(crate) fn quantize_notes(&mut self) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        let grid = self.note_grid.grid.ticks();
        let notes = note_edit::quantized(&clip.notes, &editor.selection, grid);
        let selection = editor.selection.clone();
        self.commit_notes(notes, selection);
    }

    // ---- keys

    pub(crate) fn set_note_tool(&mut self, tool: Tool) {
        if let Some(editor) = &mut self.note_editor {
            editor.tool = tool;
        }
    }

    /// Moves the selection a semitone (or octave) or a grid step.
    pub(crate) fn nudge_notes(&mut self, steps: i64, pitch: i32) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        if editor.selection.is_empty() {
            return;
        }
        let dt = Ticks(self.grid_length().0 * steps);
        let notes = note_edit::moved(&clip.notes, &editor.selection, dt, pitch);
        let selection = editor.selection.clone();
        let node = editor.node;
        if pitch != 0
            && let Some(&first) = selection.iter().next()
        {
            self.preview_note(node, notes[first].pitch, notes[first].velocity);
        }
        self.commit_notes(notes, selection);
    }

    pub(crate) fn select_all_notes(&mut self) {
        let Some((_, clip)) = self.editor_clip() else {
            return;
        };
        let all = (0..clip.notes.len()).collect();
        if let Some(editor) = &mut self.note_editor {
            editor.selection = all;
        }
    }

    pub(crate) fn copy_notes(&mut self) {
        if let Some((editor, clip)) = self.editor_clip() {
            self.note_clipboard = note_edit::clipboard(&clip.notes, &editor.selection);
        }
    }

    /// Pastes at the playhead (clamped into the clip).
    pub(crate) fn paste_notes(&mut self) {
        let Some((_, clip)) = self.editor_clip() else {
            return;
        };
        if self.note_clipboard.is_empty() {
            return;
        }
        let playhead = Ticks::from_beats_f64(self.session.position_beats());
        let at = (playhead - clip.start).max(Ticks::ZERO).min(clip.length);
        let (notes, pasted) = note_edit::pasted(&clip.notes, &self.note_clipboard, at);
        self.commit_notes(notes, pasted);
    }

    /// Deletes the selected notes; false if the editor has none selected.
    pub(crate) fn delete_notes(&mut self) -> bool {
        let Some((editor, clip)) = self.editor_clip() else {
            return false;
        };
        if editor.selection.is_empty() {
            return false;
        }
        let notes = note_edit::deleted(&clip.notes, &editor.selection);
        self.commit_notes(notes, Selection::new());
        true
    }

    // ---- drawing

    /// The panel, if a clip is open: header, keyboard and grid, velocity.
    pub(crate) fn render_note_editor(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // A clip that is gone closes the editor (unless it is a new clip
        // still on its way into the snapshot).
        let found = self.editor_clip().is_some();
        if let Some(editor) = &mut self.note_editor {
            if found {
                editor.waiting = false;
            } else if !editor.waiting {
                self.note_editor = None;
            }
        }
        self.update_rows();
        self.fit_note_editor();
        let ghosts: Vec<AnyElement> = Vec::new();
        let drawing_to: Option<AnyElement> = None;
        let tree: Option<AnyElement> = None;
        let (editor, clip) = self.editor_clip()?;
        let snapshot = self.session.snapshot();
        let node = snapshot.node(editor.node)?;
        let colors = theme.content(node.color);
        let zoom = self.notes_view.zoom;
        let ppb = zoom.pixels_per_beat;
        let key = zoom.row_height;
        let shown_notes: Vec<MidiNote> = editor
            .preview
            .as_ref()
            .map_or_else(|| clip.notes.clone(), |(n, _)| n.clone());
        let selection = editor
            .preview
            .as_ref()
            .map(|(_, s)| s.clone())
            .or_else(|| self.boxed(editor, &clip.notes))
            .unwrap_or_else(|| editor.selection.clone());
        let clip_beats = clip.length.as_beats() as f32;
        let width = (clip_beats + 4.) * ppb;
        let rows = editor.rows.clone();
        let fold_on = rows.len() < 128;
        let fold_button = small_button("note-fold", theme)
            .when(fold_on, |d| d.bg(theme.control))
            .child(icon(
                if fold_on { Icon::Unfold } else { Icon::Fold },
                if fold_on {
                    theme.accent
                } else {
                    theme.text_secondary
                },
                14.,
            ))
            .tooltip(tooltip::text(if fold_on {
                "Folded to the notes that play: click for every key"
            } else {
                "Fold to the notes that play"
            }))
            .on_click(cx.listener(move |this, _, _, _| {
                if let Some(editor) = &mut this.note_editor {
                    editor.folded = Some(!fold_on);
                    editor.fitted = false;
                }
            }));
        let height = rows.len() as f32 * key;
        let folded = rows.len() < 128;
        let offset = self.notes_view.x.offset();
        let position = self.session.position_beats() - clip.start.as_beats();

        // Slides up when opened.
        let opened = editor.opened.elapsed().as_secs_f32() / 0.18;
        let slide = 1. - (1. - opened.clamp(0., 1.)).powi(3);
        let panel_height = editor.height * slide;

        let hover = theme.hover;
        let tool_button = |id: &'static str, kind: Icon, tool: Tool, hint: &'static str| {
            let on = editor.tool == tool;
            small_button(id, theme)
                .when(on, |d| d.bg(theme.control))
                .child(icon(
                    kind,
                    if on {
                        theme.accent
                    } else {
                        theme.text_secondary
                    },
                    14.,
                ))
                .tooltip(tooltip::text(hint))
                .on_click(cx.listener(move |this, _, _, _| this.set_note_tool(tool)))
        };
        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .child(div().size(px(8.)).rounded(px(4.)).bg(colors.detail))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} · notes", node.name)),
            )
            .child(div().w(px(16.)))
            .child(tool_button("tool-select", Icon::Pointer, Tool::Select, "Select (1): drag to move, edges to resize, Ctrl to copy, double-click to add or delete"))
            .child(tool_button("tool-pen", Icon::Pencil, Tool::Pen, "Pen (2): click to add a note and drag its length, click a note to delete it"))
            .child(tool_button("tool-scissors", Icon::Scissors, Tool::Scissors, "Scissors (3): click a note to split it"))
            .child(fold_button)
            .child(div().w(px(12.)))
            .children(drawing_to)
            .child(div().flex_1())
            .child(
                div()
                    .id("note-quantize")
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(theme.control)
                    .hover(move |s| s.bg(hover))
                    .text_size(px(12.))
                    .text_color(theme.text_secondary)
                    .child("Quantize")
                    .tooltip(tooltip::text(
                        "Quantize (Q): move the selected notes' starts to the grid (every note when none are selected)",
                    ))
                    .on_click(cx.listener(|this, _, _, _| this.quantize_notes())),
            )
            .child(div().text_size(px(12.)).child(self.render_snap(true, theme, cx)))
            .child(div().w(px(6.)))
            .child(
                small_button("note-editor-close", theme)
                    .child(icon(Icon::Close, theme.text_secondary, 13.))
                    .tooltip(tooltip::text("Close the editor"))
                    .on_click(cx.listener(|this, _, _, _| this.close_note_editor())),
            );

        // The grid: rows (black keys darker), beat lines, the clip's span,
        // notes, the playhead and a box selection.
        let grid_cell = editor.grid.clone();
        let step_beats = self.note_grid.grid.ticks().as_beats() as f32;
        let mut beat_line = theme.grid;
        beat_line.a *= 2.2;
        let (black, line, bar_line, outside) = {
            let mut black = theme.control;
            black.a *= 0.6;
            let mut outside = theme.window;
            outside.a = 0.55;
            (black, theme.grid, theme.hairline, outside)
        };
        let clip_width = clip_beats * ppb;
        let paint_rows = rows.clone();
        let paint = canvas(
            move |bounds, _, _| grid_cell.set(bounds),
            move |bounds, (), window, _| {
                let origin = bounds.origin;
                for (row, pitch) in paint_rows.iter() {
                    let y = row as f32 * key;
                    if is_black(pitch) {
                        window.paint_quad(fill(
                            Bounds::new(origin + point(px(0.), px(y)), size(px(width), px(key))),
                            black,
                        ));
                    }
                    // Octave lines; folded, a line under every row.
                    if folded || pitch % 12 == 0 {
                        window.paint_quad(fill(
                            Bounds::new(
                                origin + point(px(0.), px(y + key - 1.)),
                                size(px(width), px(1.)),
                            ),
                            if folded { line } else { bar_line },
                        ));
                    }
                }
                // Lines at every grid step (when at least 6 px apart), beats
                // stronger, bars strongest.
                let step = if step_beats * ppb >= 6. {
                    step_beats
                } else {
                    1.
                };
                let steps = (width / ppb / step).ceil() as i32;
                for k in 0..=steps {
                    let beat = k as f32 * step;
                    let x = beat * ppb;
                    let color = if beat % 4. == 0. {
                        bar_line
                    } else if beat.fract() == 0. {
                        beat_line
                    } else {
                        line
                    };
                    window.paint_quad(fill(
                        Bounds::new(origin + point(px(x), px(0.)), size(px(1.), px(height))),
                        color,
                    ));
                }
                window.paint_quad(fill(
                    Bounds::new(
                        origin + point(px(clip_width), px(0.)),
                        size(px((width - clip_width).max(0.)), px(height)),
                    ),
                    outside,
                ));
            },
        )
        .absolute()
        .size_full();
        let mut selected_fill = theme.accent;
        selected_fill.a = 0.85;
        let edges = editor.tool == Tool::Select;
        let notes = shown_notes.iter().enumerate().map(|(i, n)| {
            let selected = selection.contains(&i);
            let full = n.length.as_beats() as f32 * ppb;
            let w = (full - 1.).max(3.);
            // The resize zones, as `note_edit::hit` finds them: inside each
            // end, hanging out into free space.
            let (inside, before, after) = note_edit::edge_zones(&shown_notes, i, f64::from(ppb));
            let (inside, before, after) = (inside as f32, before as f32, after as f32);
            let zone = |left: f32, width: f32| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(left))
                    .w(px(width))
                    .cursor_ew_resize()
            };
            let shade = 0.45 + 0.55 * f32::from(n.velocity) / 127.;
            let mut body = if selected {
                selected_fill
            } else {
                colors.detail
            };
            body.a *= shade;
            div()
                .absolute()
                .left(px(n.start.as_beats() as f32 * ppb))
                .top(px(rows.row_of(n.pitch).unwrap_or(0) as f32 * key + 1.))
                .w(px(w))
                .h(px(key - 2.))
                .rounded(px(2.))
                .bg(body)
                .when(edges, |d| {
                    d.child(zone(-before, before + inside))
                        .child(zone(full - inside, inside + after))
                })
                .border_1()
                .border_color(if selected {
                    theme.accent
                } else {
                    colors.outline
                })
        });
        let playhead = (0. ..=f64::from(clip_beats + 4.))
            .contains(&position)
            .then(|| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(position as f32 * ppb))
                    .w(px(1.))
                    .bg(theme.accent)
            });
        let rubber = match &editor.drag {
            Some(NoteDrag::Box { from, to, .. }) => {
                let origin = editor.drag_grid;
                let (x0, x1) = (
                    (from.x.min(to.x) - origin.x) / px(1.),
                    (from.x.max(to.x) - origin.x) / px(1.),
                );
                let (y0, y1) = (
                    (from.y.min(to.y) - origin.y) / px(1.),
                    (from.y.max(to.y) - origin.y) / px(1.),
                );
                let mut tint = theme.accent;
                tint.a = 0.12;
                Some(
                    div()
                        .absolute()
                        .left(px(x0))
                        .top(px(y0))
                        .w(px(x1 - x0))
                        .h(px(y1 - y0))
                        .bg(tint)
                        .border_1()
                        .border_color(theme.accent),
                )
            }
            _ => None,
        };
        let cursor_tool = editor.tool;
        let pen_preview = (cursor_tool == Tool::Pen && editor.drag.is_none())
            .then(|| self.pen_preview(editor, &clip, theme))
            .flatten();
        let resizing = matches!(editor.drag, Some(NoteDrag::Resize { .. }));
        let grid = div()
            .id("note-grid-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.notes_view.x)
            .child(
                div()
                    .id("note-grid")
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .when(cursor_tool == Tool::Pen, |d| d.cursor_crosshair())
                    // The pen's preview goes when the pointer leaves the grid.
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !hovered && let Some(editor) = &mut this.note_editor {
                            editor.hover = None;
                            cx.notify();
                        }
                    }))
                    .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                        if let Some(editor) = &mut this.note_editor
                            && editor.tool == Tool::Pen
                        {
                            editor.hover = Some(event.position);
                            cx.notify();
                        }
                    }))
                    .when(resizing, |d| d.cursor_ew_resize())
                    .child(paint)
                    .children(ghosts)
                    .children(notes.collect::<Vec<_>>())
                    .children(pen_preview)
                    .children(playhead)
                    .children(rubber)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, _| this.press_grid(event)),
                    ),
            );

        // The keyboard follows the grid's vertical scroll.
        let node_id = editor.node;
        let (plays, names, owners) = self.row_names(editor);
        let keys = rows.iter().map(|(_, pitch)| {
            // A drum's row shows its name; a row several instruments play
            // shows how many (where new notes go depends on the pen).
            let name = names
                .iter()
                .filter(|(p, _)| *p == pitch)
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            let label = if !name.is_empty() {
                Some(name)
            } else if folded || pitch % 12 == 0 {
                Some(note_name(pitch))
            } else {
                None
            };
            let shared = owners(pitch);
            let silent = !plays.contains(pitch);
            div()
                .id(("key", usize::from(pitch)))
                .h(px(key))
                .flex()
                .items_center()
                .justify_end()
                .pr(px(4.))
                .border_b_1()
                .border_color(theme.hairline)
                .bg(if is_black(pitch) {
                    theme.key_black
                } else {
                    theme.key_white
                })
                .when(is_black(pitch) && !folded, |d| d.mr(px(18.)))
                .hover(move |s| s.bg(hover))
                .text_size(px(8.))
                .text_color(if silent {
                    theme.text_tertiary
                } else {
                    theme.key_black
                })
                .overflow_hidden()
                .whitespace_nowrap()
                .children(label)
                .when(shared > 1, |d| {
                    d.child(
                        div()
                            .ml(px(3.))
                            .px(px(3.))
                            .rounded(px(3.))
                            .bg(theme.control)
                            .text_color(theme.text_secondary)
                            .child(shared.to_string()),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, _| this.preview_note(node_id, pitch, 100)),
                )
        });
        let keyboard = div()
            .w(px(KEYS_WIDTH))
            .flex_none()
            .overflow_hidden()
            .border_r_1()
            .border_color(theme.edge)
            .bg(theme.sidebar)
            .child(
                div()
                    .relative()
                    .top(offset.y)
                    .flex()
                    .flex_col()
                    .children(keys.collect::<Vec<_>>()),
            );

        // The ruler follows the sideways scroll; bars count in song time.
        let first_bar = clip.start.as_beats() / 4.;
        let labels = (0..=((width / ppb) / 4.).ceil() as i32).map(|bar| {
            let song_bar = (first_bar + f64::from(bar)).floor() as i32 + 1;
            div()
                .absolute()
                .left(px(bar as f32 * 4. * ppb + 3.))
                .bottom(px(2.))
                .text_size(px(10.))
                .text_color(theme.text_tertiary)
                .child(song_bar.to_string())
        });
        let ruler = div()
            .h(px(RULER_HEIGHT))
            .flex_none()
            .flex()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .child(
                div()
                    .w(px(KEYS_WIDTH))
                    .flex_none()
                    .border_r_1()
                    .border_color(theme.edge),
            )
            .child(
                div().flex_1().min_w_0().overflow_hidden().child(
                    div()
                        .relative()
                        .left(offset.x)
                        .w(px(width))
                        .h(px(RULER_HEIGHT))
                        .children(labels.collect::<Vec<_>>()),
                ),
            );

        // Velocity bars under each note's start, following the scroll.
        let velocity_cell = editor.velocity.clone();
        let bars = shown_notes.iter().enumerate().map(|(i, n)| {
            let selected = selection.contains(&i);
            let h = f32::from(n.velocity) / 127. * (VELOCITY_HEIGHT - 6.);
            div()
                .absolute()
                .left(px(n.start.as_beats() as f32 * ppb))
                .bottom(px(2.))
                .w(px(3.))
                .h(px(h))
                .rounded(px(1.))
                .bg(if selected {
                    theme.accent
                } else {
                    colors.detail
                })
        });
        let velocity = div()
            .h(px(VELOCITY_HEIGHT))
            .flex_none()
            .flex()
            .border_t_1()
            .border_color(theme.edge)
            .bg(theme.sidebar)
            .child(
                div()
                    .w(px(KEYS_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_r_1()
                    .border_color(theme.edge)
                    .text_size(px(10.))
                    .text_color(theme.text_tertiary)
                    .child("Velocity"),
            )
            .child(
                div().flex_1().min_w_0().overflow_hidden().child(
                    div()
                        .id("velocity-lane")
                        .relative()
                        .left(offset.x)
                        .w(px(width))
                        .h_full()
                        .child(
                            canvas(
                                move |bounds, _, _| velocity_cell.set(bounds),
                                |_, (), _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .children(bars.collect::<Vec<_>>())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, _| {
                                this.press_velocity(event)
                            }),
                        ),
                ),
            );

        // Arrange's frame: scrollbars with the zoom sliders at their ends.
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(ruler)
            .child(div().flex_1().min_h_0().flex().child(keyboard).child(grid))
            .child(velocity)
            .into_any_element();
        let frame = self.timeline_frame(Timeline::Notes, body, KEYS_WIDTH, theme, cx);

        let mut handle_hover = theme.accent;
        handle_hover.a = 0.35;
        let handle = div()
            .id("note-editor-resize")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(5.))
            .occlude()
            .cursor_ns_resize()
            .hover(move |s| s.bg(handle_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    if let Some(editor) = &mut this.note_editor {
                        editor.drag = Some(NoteDrag::Height {
                            origin: event.position,
                            original: editor.height,
                        });
                    }
                }),
            );

        Some(
            div()
                .h(px(panel_height))
                .flex_none()
                .relative()
                .overflow_hidden()
                .flex()
                .flex_col()
                .bg(theme.window)
                .border_t_1()
                .border_color(theme.edge)
                .child(header)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(div().flex_1().min_w_0().flex().flex_col().child(frame))
                        .children(tree),
                )
                .child(handle)
                .into_any_element(),
        )
    }

    /// The rows for the focused node: folded to its note map and the
    /// pitches holding notes, or every key.
    fn update_rows(&mut self) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        let (plays, _, _) = self.row_names(editor);
        let limited = plays.len() < 128;
        let fold = editor.folded.unwrap_or(limited && !plays.is_empty());
        let rows = if fold {
            let mut pitches: Vec<u8> = plays.iter().collect();
            pitches.extend(clip.notes.iter().map(|n| n.pitch));
            Rows::of(pitches)
        } else {
            Rows::all()
        };
        if let Some(editor) = &mut self.note_editor
            && editor.rows != rows
        {
            editor.rows = rows;
        }
    }

    /// The pen's preview note under the pointer, in the colour of the node
    /// it would go to, with that node's name.
    fn pen_preview(
        &self,
        editor: &NoteEditor,
        clip: &MidiClip,
        theme: &Theme,
    ) -> Option<AnyElement> {
        let hover = editor.hover?;
        let zoom = self.notes_view.zoom;
        let grid = editor.grid.get();
        if !grid.contains(&hover) {
            return None;
        }
        let (beat, pitch) = editor.at(hover, zoom);
        if beat < 0. || beat >= clip.length.as_beats() {
            return None;
        }
        let start = self.note_grid.floor(Ticks::from_beats_f64(beat)).as_beats() as f32;
        let owner = editor.node;
        let node = self.session.snapshot().node(owner)?;
        let colors = theme.content(node.color);
        let mut body = colors.detail;
        body.a = 0.45;
        let key = zoom.row_height;
        let ppb = zoom.pixels_per_beat;
        let row = editor.rows.row_of(pitch)? as f32;
        let width = self.grid_length().as_beats() as f32 * ppb;
        Some(
            div()
                .absolute()
                .left(px(start * ppb))
                .top(px(row * key + 1.))
                .h(px(key - 2.))
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .w(px((width - 1.).max(3.)))
                        .h_full()
                        .rounded(px(2.))
                        .bg(body)
                        .border_1()
                        .border_color(colors.detail),
                )
                .into_any_element(),
        )
    }

    /// On first draw, zooms to fit the clip and scrolls to its notes.
    fn fit_note_editor(&mut self) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        if editor.fitted {
            return;
        }
        let view = self.notes_view.x.bounds();
        if view.size.width <= px(1.) {
            return;
        }
        let view_width = view.size.width / px(1.);
        let view_height = view.size.height / px(1.);
        let ppb = ((view_width - 20.) / (clip.length.as_beats() as f32 + 1.))
            .clamp(FIT_ZOOM.0, FIT_ZOOM.1);
        let key = self.notes_view.zoom.row_height;
        let rows = &editor.rows;
        let row = |pitch: u8| rows.row_of(pitch).unwrap_or(0) as f32;
        let middle = if clip.notes.is_empty() {
            row(rows
                .pitch_at(rows.len() as f32 / 2.)
                .max(rows.pitch_at(0.).min(60)))
        } else {
            let (low, high) = clip.notes.iter().fold((f32::MAX, 0f32), |(l, h), n| {
                (l.min(row(n.pitch)), h.max(row(n.pitch)))
            });
            (low + high) / 2.
        };
        let y = ((middle + 0.5) * key - view_height / 2.).max(0.);
        self.notes_view.zoom.pixels_per_beat = ppb;
        self.notes_view.x.set_offset(point(px(0.), px(-y)));
        if let Some(editor) = &mut self.note_editor {
            editor.fitted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_keys_follow_the_octave() {
        let black: Vec<u8> = (60..72).filter(|&p| is_black(p)).collect();
        assert_eq!(black, [61, 63, 66, 68, 70]);
    }

    #[test]
    fn the_pointer_maps_to_beat_and_pitch() {
        let editor = NoteEditor::new(NodeId(1), ClipId(1), DEFAULT_HEIGHT);
        editor.grid.set(Bounds::new(
            point(px(100.), px(50.)),
            size(px(1000.), px(1536.)),
        ));
        let (beat, pitch) = editor.at(point(px(100. + 96.), px(50. + 5.5 * 12.)), DEFAULT_ZOOM);
        assert!((beat - 2.).abs() < 1e-6, "48 px per beat");
        assert_eq!(pitch, 122);
    }
}
