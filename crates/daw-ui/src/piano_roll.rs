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
use crate::live::{Live, PaneKind};
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
/// As tall as the other rulers: the loop strip over the bar numbers.
const RULER_HEIGHT: f32 = 28.;
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

/// An open editor: which node (it shows all of that node's notes, in song
/// time), the tool, the selection and the view.
pub struct NoteEditor {
    pub node: NodeId,
    /// The node whose notes are being edited (solid), and where new notes
    /// go when a row is played by several nodes below `node`: the pen in
    /// the tree. The rest of the branch shows softer; pressing one of those
    /// notes makes its node the target.
    pub target: NodeId,
    pub tool: Tool,
    pub selection: Selection,
    pub height: f32,
    /// Where the grid's content was drawn: its top-left is the clip start
    /// at pitch 127, and it moves as the grid scrolls.
    pub(crate) grid: Rc<Cell<Bounds<Pixels>>>,
    velocity: Rc<Cell<Bounds<Pixels>>>,
    fitted: bool,
    drag: Option<NoteDrag>,
    /// Notes as the drag would leave them.
    preview: Option<(Vec<MidiNote>, Selection)>,
    /// Opened at, for the slide-up.
    pub(crate) opened: Instant,
    /// Where the grid's content was when the drag began. Drags measure in
    /// that frame, so scrolling while dragging (auto-scroll near the edges)
    /// moves the notes with the view.
    drag_grid: Point<Pixels>,
    /// The pointer and keys during a drag, to follow while auto-scrolling.
    pointer: Option<(Point<Pixels>, gpui::Modifiers)>,
    /// After an edit, the selected notes by value, until the snapshot
    /// with them arrives (notes may change order between clips), and the
    /// revision the edit was made at.
    pending: Option<(Vec<MidiNote>, u64)>,
    /// Where to scroll to on the next draw, in beats.
    scroll_to: Option<f64>,
    /// A tree row to flash, and since when.
    flash: Option<(NodeId, Instant)>,
    /// Counts target changes, so "Drawing to" replays its flash each time.
    flashes: usize,
    /// The edited node's notes the selection belongs to: a change to them
    /// the editor did not make (an undo, an edit on Arrange) clears the
    /// selection, whose indices would point at other notes. (The project
    /// revision is no use here: every command, even a preview sound,
    /// advances it.)
    seen: Option<(NodeId, Vec<MidiNote>)>,
    /// The rows as last drawn.
    pub rows: Rows,
    /// Folded to the notes that play; `None` folds when the note map is
    /// limited (a drum kit) and not for full-range instruments.
    pub folded: Option<bool>,
    /// Where the pointer is over the grid, for the pen's preview note.
    hover: Option<Point<Pixels>>,
    /// What the grid drew for the pointer at `hover`: the pen's cell
    /// (snapped start and pitch) and whether it is on a note's end. A move
    /// that finds another redraws (compared with what was drawn, so it is
    /// right after a drag or an edit moves notes under a still pointer).
    pub(crate) hover_key: Cell<Option<(Ticks, u8, bool)>>,
}

impl NoteEditor {
    pub fn new(node: NodeId, height: f32) -> Self {
        Self {
            node,
            target: node,
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
            pending: None,
            scroll_to: None,
            flash: None,
            flashes: 0,
            seen: None,
            rows: Rows::all(),
            folded: None,
            hover: None,
            hover_key: Cell::new(None),
        }
    }

    /// Makes `node` the one being edited and drawn to, with a flash.
    pub fn retarget(&mut self, node: NodeId) {
        if self.target != node {
            self.target = node;
            self.selection.clear();
            self.pending = None;
        }
        self.flash = Some((node, Instant::now()));
        self.flashes += 1;
    }

    pub fn flash(&self) -> Option<(NodeId, Instant)> {
        self.flash
    }

    pub fn flashes(&self) -> usize {
        self.flashes
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
        let snapshot = self.session.snapshot();
        let node = snapshot.node(editor.target)?;
        // The edited node's whole timeline: song time from the start, a little
        // past the song's last clip.
        Some((
            editor,
            MidiClip {
                start: Ticks::ZERO,
                length: song_length(snapshot),
                notes: crate::timeline::notes_of(&node.clips),
                looping: None,
            },
        ))
    }

    /// Where the editor's time starts, in beats: the song's start.
    pub(crate) fn note_editor_start(&self) -> Option<f32> {
        self.note_editor.as_ref().map(|_| 0.)
    }

    /// What the pointer shows at `at` in the grid (`NoteEditor::hover_key`).
    fn roll_hover_key(&self, at: Point<Pixels>) -> Option<(Ticks, u8, bool)> {
        let (editor, clip) = self.editor_clip()?;
        let zoom = self.notes_view.zoom;
        let (beat, pitch) = editor.at(at, zoom);
        let cell = self.note_grid.floor(Ticks::from_beats_f64(beat.max(0.)));
        let edge = matches!(
            note_edit::hit(&clip.notes, beat, pitch, f64::from(zoom.pixels_per_beat)),
            Some((_, note_edit::Part::Start | note_edit::Part::End))
        );
        Some((cell, pitch, edge))
    }

    /// Readies the bottom panel under Arrange for this frame: the panel
    /// follows the selection and closes when its node is gone. Returns its
    /// height as it slides up, and whether it is still sliding.
    pub(crate) fn prepare_bottom_panel(&mut self) -> Option<(f32, bool)> {
        self.switch_bottom_panel();
        if let Some(editor) = &self.audio_editor {
            let alive = self
                .session
                .snapshot()
                .node(editor.node)
                .is_some_and(|n| n.kind == daw_engine::NodeKind::Audio);
            if alive {
                let slide = slide(editor.opened);
                return Some((editor.height * slide, slide < 1.));
            }
            self.close_audio_editor();
        }
        if self.note_editor.is_some() && self.editor_clip().is_none() {
            self.note_editor = None;
        }
        self.follow_selection();
        let editor = self.note_editor.as_ref()?;
        let slide = slide(editor.opened);
        Some((editor.height * slide, slide < 1.))
    }

    /// Opens the editor on a node at `beat` (song time), or points the
    /// open editor there without sliding it up again. In place of an open
    /// audio panel it takes the panel's place and height, already up.
    pub(crate) fn open_node_editor(&mut self, node: NodeId, beat: f64) {
        let mut editor = NoteEditor::new(node, DEFAULT_HEIGHT);
        if let Some(open) = &self.note_editor {
            editor.height = open.height;
            editor.tool = open.tool;
            editor.opened = open.opened;
            editor.folded = open.folded;
        } else if let Some(audio) = &self.audio_editor {
            editor.height = audio.height;
            editor.opened = audio.opened;
        }
        self.close_audio_editor();
        editor.target = self.default_target(node);
        editor.scroll_to = Some(beat);
        self.note_editor = Some(editor);
        self.selected_node = Some(node);
    }

    /// Opens the editor on a clip's node, at the clip.
    pub(crate) fn open_note_editor(&mut self, clip: ClipId) {
        let snapshot = self.session.snapshot();
        let found = snapshot.nodes.iter().find_map(|n| {
            n.clips
                .iter()
                .find(|(id, _)| *id == clip)
                .map(|(_, c)| (n.id, c.start.as_beats()))
        });
        if let Some((node, beat)) = found {
            self.open_node_editor(node, beat);
        }
    }

    /// The pen's first place for a node: itself if it plays notes, else
    /// the first node below it that does.
    fn default_target(&self, node: NodeId) -> NodeId {
        crate::tree_editor::tree_nodes(self.session.snapshot(), node)
            .into_iter()
            .find(|(n, _)| !crate::tree_editor::own_notes(n).is_empty())
            .map_or(node, |(n, _)| n.id)
    }

    /// The open editor follows the node selected on Arrange, showing the
    /// time Arrange shows.
    fn follow_selection(&mut self) {
        let Some(selected) = self.selected_node else {
            return;
        };
        let Some(editor) = &self.note_editor else {
            return;
        };
        if editor.node == selected || editor.dragging() {
            return;
        }
        let takes = self
            .session
            .snapshot()
            .node(selected)
            .is_some_and(|n| n.kind == daw_engine::NodeKind::Midi);
        if !takes {
            return;
        }
        let zoom = self.arrange_view.zoom;
        let shown = f64::from(-self.arrange_view.x.offset().x / px(1.) / zoom.pixels_per_beat);
        let target = self.default_target(selected);
        if let Some(editor) = &mut self.note_editor {
            editor.node = selected;
            editor.target = target;
            editor.selection.clear();
            editor.scroll_to = Some(shown.max(0.));
            editor.fitted = false;
        }
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

    /// Puts the node's notes back into its clips (new clips where notes
    /// fall outside them) as one edit, and keeps the selection.
    fn commit_notes(&mut self, notes: Vec<MidiNote>, selection: Selection) {
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        if notes == clip.notes {
            return;
        }
        let node = editor.target;
        let chosen: Vec<MidiNote> = selection
            .iter()
            .filter_map(|&i| notes.get(i).cloned())
            .collect();
        self.place_on(node, notes);
        let revision = self.session.snapshot().revision;
        if let Some(editor) = &mut self.note_editor {
            editor.selection = selection;
            editor.pending = Some((chosen, revision));
        }
    }

    /// Makes `node`'s clips hold `notes` (song time), as one edit.
    fn place_on(&mut self, node: NodeId, notes: Vec<MidiNote>) {
        if let Some(edit) = self.notes_edit(node, &notes) {
            self.session.apply(Command::Edit(edit));
        }
    }

    /// The edit that makes `node`'s clips hold `notes`, if any is needed.
    fn notes_edit(&self, node: NodeId, notes: &[MidiNote]) -> Option<Edit> {
        let snapshot = self.session.snapshot();
        let owner = snapshot.node(node)?;
        let bar = Ticks::from_beats(4);
        let next = crate::clip_edit::next_clip_id(snapshot);
        let mut edits = crate::timeline::place_notes(node, &owner.clips, notes, bar, next);
        match edits.len() {
            0 => None,
            1 => Some(edits.remove(0)),
            _ => Some(Edit::Batch(edits)),
        }
    }

    /// The edit a note drag makes if let go now: the notes as previewed.
    pub(crate) fn note_drag_edit(&self) -> Option<Edit> {
        let (editor, clip) = self.editor_clip()?;
        if matches!(editor.drag, None | Some(NoteDrag::Box { .. })) {
            return None;
        }
        let (notes, _) = editor.preview.as_ref()?;
        if *notes == clip.notes {
            return None;
        }
        self.notes_edit(editor.target, notes)
    }

    /// After an edit lands, the selection follows the notes it held.
    fn settle_selection(&mut self) {
        let revision = self.session.snapshot().revision;
        let Some((editor, clip)) = self.editor_clip() else {
            return;
        };
        let Some((chosen, at)) = &editor.pending else {
            let changed = match &editor.seen {
                Some((node, notes)) => *node == editor.target && *notes != clip.notes,
                None => false,
            };
            let fresh = editor
                .seen
                .as_ref()
                .is_none_or(|(node, notes)| *node != editor.target || *notes != clip.notes);
            if fresh {
                let seen = Some((editor.target, clip.notes.clone()));
                if let Some(editor) = &mut self.note_editor {
                    if changed {
                        editor.selection.clear();
                    }
                    editor.seen = seen;
                }
            }
            return;
        };
        if *at == revision {
            return;
        }
        let mut taken = vec![false; clip.notes.len()];
        let selection = chosen
            .iter()
            .filter_map(|want| {
                let i = clip
                    .notes
                    .iter()
                    .enumerate()
                    .position(|(i, n)| !taken[i] && n == want)?;
                taken[i] = true;
                Some(i)
            })
            .collect();
        let seen = Some((editor.target, clip.notes.clone()));
        if let Some(editor) = &mut self.note_editor {
            editor.selection = selection;
            editor.pending = None;
            editor.seen = seen;
        }
    }

    /// Clears the selected notes: a press on Arrange's clips or blocks
    /// calls this, so Delete acts on what was picked last.
    pub(crate) fn clear_note_selection(&mut self) {
        if let Some(editor) = &mut self.note_editor {
            editor.selection.clear();
        }
    }

    /// With the editor on a node that has nodes below it, a new note goes
    /// to the one node that plays its row, or to the pen's node; true if
    /// it went to another node than the editor's.
    fn add_routed_note(&mut self, note: MidiNote) -> bool {
        let Some(editor) = self.note_editor.as_ref() else {
            return false;
        };
        let owner = self.new_note_owner(editor, note.pitch);
        if owner == editor.target {
            return false;
        }
        let Some(target) = self.session.snapshot().node(owner) else {
            return false;
        };
        let mut notes = crate::timeline::notes_of(&target.clips);
        notes.push(note.clone());
        self.place_on(owner, notes);
        self.preview_note(owner, note.pitch, note.velocity);
        if let Some(editor) = &mut self.note_editor {
            editor.flash = Some((owner, Instant::now()));
        }
        true
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
        // Working in the roll lets go of what Arrange had selected, so
        // Delete acts on notes.
        self.selected_clips.clear();
        self.selected_combined = None;
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
        let (node, tool) = (editor.target, editor.tool);
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
                        bend: None,
                    };
                    if self.add_routed_note(note.clone()) {
                        return;
                    }
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
                        bend: None,
                    };
                    if self.add_routed_note(note.clone()) {
                        return;
                    }
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
                let anchor = &original[*grabbed];
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
                let anchor = &original[*grabbed];
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
                let note = &original[*index];
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
        let node = editor.target;
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
        let node = editor.target;
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
        // A node that is gone closes the editor.
        if self.note_editor.is_some() && self.editor_clip().is_none() {
            self.note_editor = None;
        }
        self.follow_selection();
        self.settle_selection();
        self.update_rows();
        self.fit_note_editor();
        let zoom_now = self.notes_view.zoom;
        let (editor, _) = self.editor_clip()?;
        let ghosts = self.inherited_notes(
            editor,
            zoom_now.pixels_per_beat,
            zoom_now.row_height,
            theme,
            cx,
        );
        let drawing_to = self.drawing_to(editor, theme);
        let tree = self.render_tree(editor, theme, cx);
        let (editor, clip) = self.editor_clip()?;
        let snapshot = self.session.snapshot();
        // The header names the node in view; notes and clip spans are the
        // edited node's.
        let viewed = snapshot.node(editor.node)?;
        let node = snapshot.node(editor.target)?;
        let colors = theme.content(node.color);
        let viewed_color = theme.content(viewed.color).detail;
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
        let clip_start = clip.start.as_beats();

        let panel_height = editor.height * slide(editor.opened);

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
            .child(div().size(px(8.)).rounded(px(4.)).bg(viewed_color))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} · notes", viewed.name)),
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
            .child(div().text_size(px(12.)).child(self.render_snap(crate::menu::SnapFor::Notes, theme, cx)))
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
        // The node's clips, as spans of beats; time outside them is shaded.
        let spans: Vec<(f32, f32)> = node
            .clips
            .iter()
            .map(|(_, c)| {
                let start = c.start.as_beats() as f32;
                (start, start + c.length.as_beats() as f32)
            })
            .collect();
        let paint_rows = rows.clone();
        let paint = canvas(
            move |bounds, _, _| grid_cell.set(bounds),
            move |bounds, (), window, _| {
                let origin = bounds.origin;
                // Only what is on screen is painted.
                let visible = window.content_mask().bounds.intersect(&bounds);
                let (left, right) = (
                    (visible.left() - origin.x) / px(1.),
                    (visible.right() - origin.x) / px(1.),
                );
                let (top, bottom) = (
                    (visible.top() - origin.y) / px(1.),
                    (visible.bottom() - origin.y) / px(1.),
                );
                let across = |y: f32, h: f32| {
                    Bounds::new(
                        origin + point(px(left), px(y)),
                        size(px((right - left).max(0.)), px(h)),
                    )
                };
                for (row, pitch) in paint_rows.iter() {
                    let y = row as f32 * key;
                    if y + key < top || y > bottom {
                        continue;
                    }
                    if is_black(pitch) {
                        window.paint_quad(fill(across(y, key), black));
                    }
                    // Octave lines; folded, a line under every row.
                    if folded || pitch % 12 == 0 {
                        window.paint_quad(fill(
                            across(y + key - 1., 1.),
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
                let first = ((left / ppb / step).floor() as i32).max(0);
                let last = ((right / ppb / step).ceil() as i32).min(steps);
                for k in first..=last {
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
                let mut at = 0.;
                let mut sorted = spans.clone();
                sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (start, end) in sorted.into_iter().chain([(width / ppb, width / ppb)]) {
                    if start > at {
                        window.paint_quad(fill(
                            Bounds::new(
                                origin + point(px(at * ppb), px(0.)),
                                size(px((start - at) * ppb), px(height)),
                            ),
                            outside,
                        ));
                    }
                    at = f32::max(at, end);
                }
            },
        )
        .absolute()
        .size_full();
        let mut selected_fill = theme.accent;
        selected_fill.a = 0.85;
        let edges = editor.tool == Tool::Select;
        // The notes, in one drawing; only those on screen are painted.
        // A looped clip's repeats, faded behind the notes edited
        // (`docs/design/clip-loops.md`), in one drawing.
        let mut repeat_color = colors.detail;
        repeat_color.a *= 0.22;
        let repeat_bars: Vec<crate::paint::Bar> = crate::timeline::repeats_of(&node.clips)
            .iter()
            .filter_map(|n| {
                let row = rows.row_of(n.pitch)?;
                Some(crate::paint::Bar {
                    x: (n.start - clip.start).as_beats() as f32 * ppb,
                    y: row as f32 * key + 1.,
                    w: (n.length.as_beats() as f32 * ppb - 1.).max(3.),
                    h: key - 2.,
                    color: repeat_color,
                    radius: 2.,
                })
            })
            .collect();
        let repeats = crate::paint::bars(repeat_bars)
            .absolute()
            .top_0()
            .left_0()
            .size_full();
        let note_quads: Vec<(Bounds<Pixels>, gpui::Hsla, gpui::Hsla)> = shown_notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let selected = selection.contains(&i);
                let full = n.length.as_beats() as f32 * ppb;
                let w = (full - 1.).max(3.);
                let shade = 0.45 + 0.55 * f32::from(n.velocity) / 127.;
                let mut body = if selected {
                    selected_fill
                } else {
                    colors.detail
                };
                body.a *= shade;
                let border = if selected {
                    theme.accent
                } else {
                    colors.outline
                };
                let rect = Bounds::new(
                    point(
                        px(n.start.as_beats() as f32 * ppb),
                        px(rows.row_of(n.pitch).unwrap_or(0) as f32 * key + 1.),
                    ),
                    size(px(w), px(key - 2.)),
                );
                (rect, body, border)
            })
            .collect();
        let notes = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let visible = window.content_mask().bounds;
                for (rect, body, border) in &note_quads {
                    let rect = Bounds::new(bounds.origin + rect.origin, rect.size);
                    if rect.intersects(&visible) {
                        window.paint_quad(
                            fill(rect, *body)
                                .corner_radii(px(2.))
                                .border_widths(px(1.))
                                .border_color(*border),
                        );
                    }
                }
            },
        )
        .absolute()
        .size_full();
        // What this drawing shows for the pointer, for the next move to
        // compare with.
        editor
            .hover_key
            .set(editor.hover.and_then(|at| self.roll_hover_key(at)));
        // Over a note's end (as `note_edit::hit` finds it, the same rule
        // presses use) the pointer shows it resizes.
        let over_edge = edges
            && editor.drag.is_none()
            && editor.hover.is_some_and(|at| {
                let (beat, pitch) = editor.at(at, self.notes_view.zoom);
                matches!(
                    note_edit::hit(&shown_notes, beat, pitch, f64::from(ppb)),
                    Some((_, note_edit::Part::Start | note_edit::Part::End))
                )
            });
        // Drawn by the live layer, from the editor's start across the grid.
        let playhead = self
            .live_slot(
                PaneKind::Bottom,
                Live::Playhead {
                    start: clip_start,
                    ppb,
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0();
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
                            editor.hover_key.set(None);
                            this.hover_changed(cx);
                        }
                    }))
                    .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                        // The pen's preview and the resize cursor follow it;
                        // redraw only when the cell or the edge changes.
                        let key = this.roll_hover_key(event.position);
                        if let Some(editor) = &mut this.note_editor {
                            editor.hover = Some(event.position);
                            if editor.hover_key.get() != key {
                                this.hover_changed(cx);
                            }
                        }
                    }))
                    .when(resizing || over_edge, |d| d.cursor_ew_resize())
                    .child(paint)
                    .child(repeats)
                    .children(ghosts)
                    .child(notes)
                    .children(pen_preview)
                    .child(playhead)
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
                .bottom(px(1.))
                .text_size(px(10.))
                .text_color(theme.text_tertiary)
                .child(song_bar.to_string())
        });
        let loop_strip =
            self.render_loop_strip(crate::loop_strip::LoopRuler::Notes, ppb, width, theme, cx);
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
                        .cursor_pointer()
                        .children(labels.collect::<Vec<_>>())
                        .child(loop_strip)
                        // Below the loop strip, a press places the playhead.
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                let ruler = crate::loop_strip::LoopRuler::Notes;
                                this.locate_on_ruler(ruler, event.position, ppb);
                            }),
                        ),
                ),
            );

        // Velocity bars under each note's start, following the scroll.
        let velocity_cell = editor.velocity.clone();
        let bars: Vec<(f32, f32, gpui::Hsla)> = shown_notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let h = f32::from(n.velocity) / 127. * (VELOCITY_HEIGHT - 6.);
                let color = if selection.contains(&i) {
                    theme.accent
                } else {
                    colors.detail
                };
                (n.start.as_beats() as f32 * ppb, h, color)
            })
            .collect();
        // Every bar in one drawing, standing on the lane's floor.
        let bars = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let visible = window.content_mask().bounds;
                for (x, h, color) in &bars {
                    let rect = Bounds::new(
                        point(bounds.left() + px(*x), bounds.bottom() - px(2. + h)),
                        size(px(3.), px(*h)),
                    );
                    if rect.intersects(&visible) {
                        window.paint_quad(fill(rect, *color).corner_radii(px(1.)));
                    }
                }
            },
        )
        .absolute()
        .size_full();
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
                        .child(bars)
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
            pitches.extend(self.inherited_pitches(editor));
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
        let owner = self.new_note_owner(editor, pitch);
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
        if editor.fitted && editor.scroll_to.is_none() {
            return;
        }
        let view = self.notes_view.x.bounds();
        if view.size.width <= px(1.) {
            return;
        }
        let view_width = view.size.width / px(1.);
        let view_height = view.size.height / px(1.);
        let ppb = self.notes_view.zoom.pixels_per_beat;
        let key = self.notes_view.zoom.row_height;
        let from = editor.scroll_to.unwrap_or(0.);
        let to = from + f64::from(view_width / ppb);
        // Centre on the notes in view (or all of them).
        let rows = &editor.rows;
        let row = |pitch: u8| rows.row_of(pitch).unwrap_or(0) as f32;
        let in_view: Vec<&MidiNote> = clip
            .notes
            .iter()
            .filter(|n| (from..to).contains(&n.start.as_beats()))
            .collect();
        let notes: Vec<&MidiNote> = if in_view.is_empty() {
            clip.notes.iter().collect()
        } else {
            in_view
        };
        let middle = if notes.is_empty() {
            row(rows
                .pitch_at(rows.len() as f32 / 2.)
                .max(rows.pitch_at(0.).min(60)))
        } else {
            let (low, high) = notes.iter().fold((f32::MAX, 0f32), |(l, h), n| {
                (l.min(row(n.pitch)), h.max(row(n.pitch)))
            });
            (low + high) / 2.
        };
        let y = ((middle + 0.5) * key - view_height / 2.).max(0.);
        let x = (from as f32 * ppb).max(0.);
        self.notes_view.x.set_offset(point(px(-x), px(-y)));
        if let Some(editor) = &mut self.note_editor {
            editor.fitted = true;
            editor.scroll_to = None;
        }
    }
}

/// The editor's timeline length: past the song's last clip by eight bars,
/// at least 32 bars.
fn song_length(snapshot: &daw_engine::Snapshot) -> Ticks {
    let end = snapshot
        .nodes
        .iter()
        .flat_map(|n| n.clips.iter().map(|(_, c)| c.start + c.length))
        .max()
        .unwrap_or(Ticks::ZERO);
    (end + Ticks::from_bars(8)).max(Ticks::from_bars(32))
}

/// How far a bottom panel has slid up since it opened, 0 to 1 (about 0.2 s,
/// easing out).
pub(crate) fn slide(opened: Instant) -> f32 {
    let t = (opened.elapsed().as_secs_f32() / 0.18).clamp(0., 1.);
    1. - (1. - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_the_edited_node_clears_the_selection_and_flashes() {
        let mut editor = NoteEditor::new(NodeId(1), DEFAULT_HEIGHT);
        editor.selection.insert(3);
        editor.retarget(NodeId(1));
        assert!(
            editor.selection.contains(&3),
            "the same node keeps its selection"
        );
        editor.retarget(NodeId(2));
        assert_eq!(editor.target, NodeId(2));
        assert!(
            editor.selection.is_empty(),
            "indices belong to the old node's notes"
        );
        assert_eq!(editor.flash().map(|(n, _)| n), Some(NodeId(2)));
        assert_eq!(editor.flashes(), 2, "each switch replays the flash");
    }

    #[test]
    fn black_keys_follow_the_octave() {
        let black: Vec<u8> = (60..72).filter(|&p| is_black(p)).collect();
        assert_eq!(black, [61, 63, 66, 68, 70]);
    }

    #[test]
    fn the_pointer_maps_to_beat_and_pitch() {
        let editor = NoteEditor::new(NodeId(1), DEFAULT_HEIGHT);
        editor.grid.set(Bounds::new(
            point(px(100.), px(50.)),
            size(px(1000.), px(1536.)),
        ));
        let (beat, pitch) = editor.at(point(px(100. + 96.), px(50. + 5.5 * 12.)), DEFAULT_ZOOM);
        assert!((beat - 2.).abs() < 1e-6, "48 px per beat");
        assert_eq!(pitch, 122);
    }
}
