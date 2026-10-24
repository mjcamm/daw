//! An open project: header and the current page.

use crate::block_edit::Snap;
use crate::browser::{BrowserEvent, Choice, DeviceBrowser, PluginShelf, Side, Target};
use crate::export_dialog::{ExportDialog, ExportEvent, SongFacts};
use crate::frame::{FrameDrag, TimelineView};
use crate::layout::PIXELS_PER_BEAT;
use crate::live::{LiveLayer, PaneKind};
use crate::menu::OpenMenu;
use crate::pane::Pane;
use crate::param_picker::{ParamPicker, PickerEvent};
use crate::parameters::{BaseDrag, BlockDrag, DEFAULT_ZOOM, MixDrag};
use crate::session::Session;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;
use crate::zoom::Zoom;
use daw_engine::{
    BlockId, Command, Edit, LayerId, MidiClip, NodeId, NodeSnapshot, SignalId, Snapshot,
};
use gpui::{
    AnyElement, AnyView, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, ModifiersChangedEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, ScrollWheelEvent, StyleRefinement, Subscription, Window, actions, canvas, div,
    prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;

actions!(
    daw,
    [
        TogglePlay,
        Save,
        Undo,
        Redo,
        DeleteSelection,
        CancelDrag,
        ToolSelect,
        ToolPen,
        ToolScissors,
        NudgeUp,
        NudgeDown,
        OctaveUp,
        OctaveDown,
        NudgeLeft,
        NudgeRight,
        SelectAll,
        CopyNotes,
        PasteNotes,
        Quantize,
        ExportSong
    ]
);

/// The key context of an open project, for its shortcuts.
pub const KEY_CONTEXT: &str = "Workspace";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Arrange,
    Parameters,
    Signals,
    Mixer,
}

impl Page {
    pub const ALL: [Self; 4] = [Self::Arrange, Self::Parameters, Self::Signals, Self::Mixer];

    pub fn label(self) -> &'static str {
        match self {
            Self::Arrange => "Arrange",
            Self::Parameters => "Parameters",
            Self::Signals => "Signals",
            Self::Mixer => "Mixer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameTarget {
    Song,
    Node(NodeId),
    Layer(LayerId),
    Signal(SignalId),
}

/// A name being edited in place.
pub(crate) struct Rename {
    pub target: RenameTarget,
    pub input: Entity<TextInput>,
    _subscription: Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectEvent {
    /// Go back to the launch page.
    Close,
    /// The metronome changed from the transport: the app keeps it.
    Metronome(daw_engine::Metronome),
}

pub struct ProjectView {
    pub(crate) session: Session,
    pub(crate) page: Page,
    /// A loop being drawn or an end of it moved on a ruler's loop strip.
    pub(crate) loop_drag: Option<crate::loop_strip::LoopDrag>,
    pub(crate) loop_strip_bounds: crate::loop_view::StripBounds,
    /// Where the timeline ruler was last drawn, for turning clicks into
    /// song positions.
    pub(crate) open_menu: Option<OpenMenu>,
    /// The menu the current press closed by landing outside it, so that
    /// pressing the menu's own trigger closes it rather than reopening it.
    /// Cleared at the start of every press.
    dismissed_menu: Option<OpenMenu>,
    /// The node selected on Arrange; the Parameters page shows it.
    pub(crate) selected_node: Option<NodeId>,
    /// Folders folded in the nesting map. UI state, not saved.
    pub(crate) folded: std::collections::HashSet<NodeId>,
    /// Why the last sample import failed, shown in the sampler's panel.
    pub(crate) sample_error: Option<String>,
    /// Why the last audio import left files out, if it did.
    pub(crate) import_error: Option<String>,
    /// Where Arrange's lanes were last drawn, for files dragged over them.
    pub(crate) arrange_lanes_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Files being dragged over Arrange from outside: the pointer and how
    /// many files. Cleared once no drag is running.
    pub(crate) file_hover: Option<(gpui::Point<Pixels>, usize)>,
    /// Arrange's tool: the pointer, or the scissors.
    pub(crate) arrange_tool: crate::arrange::ArrangeTool,
    /// The clip and part of it under the pointer on Arrange (node, clip,
    /// part) as the lanes last drew it, worked out from `arrange_pointer`
    /// against the clips as drawn (its cursor and loop icon). A move that
    /// finds another part redraws them.
    pub(crate) arrange_hover: Cell<Option<(NodeId, daw_engine::ClipId, crate::clip_edit::Grab)>>,
    /// The pointer while it is over Arrange's lanes: what the lanes' cursor
    /// and loop icon follow, so they are right after a drag or an edit
    /// moves a clip under a still pointer.
    pub(crate) arrange_pointer: Option<gpui::Point<Pixels>>,
    /// The pointer over the metronome's volume button or its popover.
    pub(crate) volume_hover: crate::metronome_ui::VolumeHover,
    pub(crate) volume_drag: Option<crate::metronome_ui::VolumeDrag>,
    /// Metronome settings changed and not yet handed to the app to keep.
    pub(crate) metronome_to_keep: Option<daw_engine::Metronome>,
    /// A hover effect changed (a cursor, a preview): the pane under the
    /// pointer redraws. Plain moves redraw nothing otherwise.
    pub(crate) hover_changed: bool,
    /// What the pointer shows over a layer or signal lane.
    pub(crate) lane_hover: Cell<Option<crate::parameters::LaneHover>>,
    /// The pointer over the lanes while the scissors are out.
    pub(crate) scissors_at: Option<gpui::Point<Pixels>>,
    /// Whether the pointer is over the piano roll panel: tool keys go to it.
    pub(crate) roll_hovered: bool,
    /// The piano roll in the bottom panel, if a clip is open.
    pub(crate) note_editor: Option<crate::piano_roll::NoteEditor>,
    /// The bottom panel for an audio node, in the piano roll's place.
    pub(crate) audio_editor: Option<crate::audio_panel::AudioEditor>,
    /// Typing a clip's BPM in the audio panel.
    pub(crate) bpm_input: Option<crate::audio_panel::BpmInput>,
    /// Typing the song tempo beside the transport.
    pub(crate) tempo_input: Option<crate::header::TempoInput>,
    /// A note being heard from the editor, and since when.
    pub(crate) note_preview: Option<(NodeId, u8, std::time::Instant)>,
    /// Notes copied in the editor.
    pub(crate) note_clipboard: Vec<daw_engine::MidiNote>,
    /// The combined block selected on Arrange: its node and start.
    pub(crate) selected_combined: Option<(NodeId, daw_engine::Ticks)>,
    /// A combined block being dragged.
    pub(crate) combined_drag: Option<crate::combined::CombinedDrag>,
    /// Clips selected on Arrange.
    pub(crate) selected_clips: std::collections::BTreeSet<daw_engine::ClipId>,
    /// Clips being dragged on Arrange; one edit on release.
    pub(crate) clip_drag: Option<crate::clip_drag::ClipDrag>,
    /// The mixer's strip width, scrolling and meters.
    pub(crate) mixer_view: crate::mixer::MixerView,
    /// A pan, scrollbar or strip-width drag on the mixer.
    pub(crate) mixer_drag: Option<crate::mixer::MixerDrag>,
    /// The mixer's strips as they animate in, out and around.
    pub(crate) strip_motion: crate::motion::RowMotion,
    /// The effect selected in a mixer strip.
    pub(crate) selected_effect: Option<daw_engine::DeviceId>,
    /// The width of Arrange's nesting map, easing as levels come and go.
    pub(crate) map_width: crate::motion::Eased,
    /// The sideways scroll of Arrange's nesting map.
    pub(crate) node_map_x: gpui::ScrollHandle,
    /// Arrange's rows as they animate in, out and around.
    pub(crate) row_motion: crate::motion::RowMotion,
    /// Where the Parameters page lanes were last drawn: the top-left is
    /// beat 0 of the first row.
    pub(crate) lanes_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Snap or slide for block edits. A UI preference, not project state.
    pub(crate) snap: Snap,
    /// The piano roll's own snap or slide and grid (finer than Arrange's).
    pub(crate) note_grid: Snap,
    pub(crate) selected_block: Option<BlockId>,
    /// A block being dragged, shown as a preview until the mouse is
    /// released; then it becomes one edit.
    pub(crate) block_drag: Option<BlockDrag>,
    /// A parameter's resting value being dragged with its slider; sent as
    /// one edit on release.
    pub(crate) base_drag: Option<BaseDrag>,
    /// A layer's mix being dragged with its slider; one edit on release.
    pub(crate) mix_drag: Option<MixDrag>,
    /// Each timeline page's zoom and scroll position.
    pub(crate) arrange_view: TimelineView,
    pub(crate) parameters_view: TimelineView,
    pub(crate) signals_view: TimelineView,
    /// The piano roll's zoom and scroll.
    pub(crate) notes_view: TimelineView,
    /// A layer just jumped to from elsewhere, highlighted briefly so it is
    /// easy to spot.
    pub(crate) highlight: Option<(LayerId, std::time::Instant)>,
    /// The signal selected on the Signals page; the inspector shows it.
    pub(crate) selected_signal: Option<SignalId>,
    /// Whether the inspector is open, and how wide. UI state, not saved.
    pub(crate) inspector_open: bool,
    pub(crate) inspector_width: f32,
    /// The inspector's edge being dragged to resize it.
    pub(crate) inspector_resize: Option<crate::inspector::ResizeDrag>,
    /// A notes signal setting being dragged in the inspector; one edit on
    /// release.
    pub(crate) notes_drag: Option<crate::inspector::NotesDrag>,
    /// A signal just jumped to from elsewhere, highlighted briefly.
    pub(crate) signal_highlight: Option<(SignalId, std::time::Instant)>,
    /// Every signal compiled, for drawing what listening layers do; kept
    /// until the engine's state changes.
    pub(crate) signal_cache: std::cell::RefCell<Option<(u64, Rc<daw_automation::Signals>)>>,
    /// Which step the add-layer menu shows.
    pub(crate) layer_menu: crate::parameters::LayerMenu,
    /// The parameter chosen in an add-parameter menu, now picking its first
    /// layer's type.
    pub(crate) pending_param: Option<daw_engine::ParamId>,
    /// A scrollbar or zoom slider being dragged.
    pub(crate) frame_drag: Option<FrameDrag>,
    pub(crate) rename: Option<Rename>,
    /// The device browser, while open.
    pub(crate) browser: Option<(Entity<DeviceBrowser>, Subscription)>,
    /// A plugin's parameter list, while its add-parameter menu is open.
    pub(crate) param_picker: Option<(Entity<ParamPicker>, Subscription)>,
    /// File → Export…, while open.
    pub(crate) export: Option<(Entity<ExportDialog>, Subscription)>,
    /// The export settings chosen last; the dialog opens with them.
    export_settings: daw_host::export::ExportSettings,
    /// The browser's side the user chose last; it opens there.
    browser_side: Side,
    /// The plugins the browser offers (handed over by the app).
    plugins: PluginShelf,
    pub(crate) save_error: Option<String>,
    window_title: String,
    pub(crate) focus: FocusHandle,
    /// What moves every frame, drawn above the panes.
    pub(crate) live: Rc<crate::live::LiveLayer>,
    /// Set while a pane renders something mid-animation, so it redraws
    /// next frame.
    pub(crate) animating: Cell<bool>,
    /// The cached parts of the window; made on the first render.
    panes: Option<Panes>,
    /// The song revision the panes last drew.
    drawn_revision: u64,
    /// Where each pane is on screen, for sending input to the right one.
    pane_bounds: PaneBounds,
    /// The pane the pointer was last over.
    hovered_pane: Option<PaneKind>,
    /// The pane the last press landed in: while a drag runs, only it
    /// redraws every frame.
    pressed_pane: Option<PaneKind>,
    /// When the last frame was drawn; the engine is serviced on a timer
    /// while no frames run.
    last_frame: std::time::Instant,
}

/// Where each pane was laid out, written as the shell is drawn.
type PaneBounds = Rc<std::cell::RefCell<Vec<(PaneKind, Bounds<Pixels>)>>>;

/// The cached parts of the window (`pane.rs`).
struct Panes {
    main: Entity<Pane>,
    bottom: Entity<Pane>,
    inspector: Entity<Pane>,
    arrange_list: Entity<Pane>,
}

impl Panes {
    fn get(&self, kind: PaneKind) -> &Entity<Pane> {
        match kind {
            PaneKind::Main => &self.main,
            PaneKind::Bottom => &self.bottom,
            PaneKind::Inspector => &self.inspector,
            PaneKind::ArrangeList => &self.arrange_list,
        }
    }
}

const PANES: [PaneKind; 4] = [
    PaneKind::Main,
    PaneKind::Bottom,
    PaneKind::Inspector,
    PaneKind::ArrangeList,
];

impl EventEmitter<ProjectEvent> for ProjectView {}

/// Every clip in the song, on every node.
pub(crate) fn song_clips(snapshot: &Snapshot) -> impl Iterator<Item = &MidiClip> {
    snapshot
        .nodes
        .iter()
        .flat_map(|n| n.clips.iter().map(|(_, c)| c))
}

impl ProjectView {
    /// The selected node, if it still exists.
    pub(crate) fn shown_node(&self) -> Option<&NodeSnapshot> {
        self.session.snapshot().node(self.selected_node?)
    }

    pub fn new(session: Session, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            session,
            page: Page::Arrange,
            loop_drag: None,
            loop_strip_bounds: Default::default(),
            open_menu: None,
            dismissed_menu: None,
            selected_node: None,
            folded: std::collections::HashSet::new(),
            row_motion: crate::motion::RowMotion::default(),
            map_width: crate::motion::Eased::default(),
            node_map_x: gpui::ScrollHandle::new(),
            mixer_view: crate::mixer::MixerView::default(),
            mixer_drag: None,
            strip_motion: crate::motion::RowMotion::default(),
            selected_effect: None,
            selected_clips: std::collections::BTreeSet::new(),
            selected_combined: None,
            combined_drag: None,

            sample_error: None,
            import_error: None,
            arrange_lanes_bounds: Rc::default(),
            file_hover: None,
            arrange_tool: crate::arrange::ArrangeTool::Pointer,
            scissors_at: None,
            arrange_hover: Cell::new(None),
            arrange_pointer: None,
            volume_hover: Default::default(),
            volume_drag: None,
            metronome_to_keep: None,
            hover_changed: false,
            lane_hover: Cell::new(None),
            roll_hovered: false,
            note_editor: None,
            audio_editor: None,
            bpm_input: None,
            tempo_input: None,
            note_preview: None,
            note_clipboard: Vec::new(),
            clip_drag: None,
            lanes_bounds: Rc::default(),
            snap: Snap::default(),
            note_grid: Snap {
                enabled: true,
                grid: crate::block_edit::Grid::Sixteenth,
            },
            selected_block: None,
            block_drag: None,
            base_drag: None,
            mix_drag: None,
            arrange_view: TimelineView::new(Zoom::new(PIXELS_PER_BEAT, 56.)),
            parameters_view: TimelineView::new(DEFAULT_ZOOM),
            signals_view: TimelineView::new(crate::signals::DEFAULT_ZOOM),
            notes_view: TimelineView::shared(crate::piano_roll::DEFAULT_ZOOM),
            highlight: None,
            signal_highlight: None,
            selected_signal: None,
            inspector_open: false,
            inspector_width: crate::inspector::DEFAULT_WIDTH,
            inspector_resize: None,
            notes_drag: None,
            signal_cache: std::cell::RefCell::default(),
            layer_menu: crate::parameters::LayerMenu::Kind,
            pending_param: None,
            frame_drag: None,
            rename: None,
            browser: None,
            param_picker: None,
            export: None,
            export_settings: crate::export_dialog::default_settings(),
            browser_side: Side::default(),
            plugins: PluginShelf::default(),
            save_error: None,
            window_title: String::new(),
            focus,
            live: Rc::default(),
            animating: Cell::new(false),
            panes: None,
            drawn_revision: u64::MAX,
            pane_bounds: Rc::default(),
            hovered_pane: None,
            pressed_pane: None,
            last_frame: std::time::Instant::now(),
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub(crate) fn toggle_play(&mut self) {
        let command = if self.session.snapshot().playing {
            Command::Stop
        } else {
            Command::Play
        };
        self.session.apply(command);
    }

    /// The transport's loop button: looping on or off, keeping the region.
    pub(crate) fn toggle_loop(&mut self) {
        let snapshot = self.session.snapshot();
        if snapshot.loop_range.is_some() {
            let on = !snapshot.loop_enabled;
            self.session.apply(Command::SetLoopEnabled(on));
        }
    }

    pub(crate) fn save(&mut self) {
        self.open_menu = None;
        self.save_error = self
            .session
            .save()
            .err()
            .map(|e| format!("Could not save: {e}"));
    }

    /// Saves if there are unsaved changes. False if saving failed (the
    /// reason shows in the project's banner).
    pub(crate) fn save_if_edited(&mut self) -> bool {
        if self.session.is_edited() {
            self.save();
        }
        self.save_error.is_none()
    }

    /// Saves any changes, then asks to go back to the launch page.
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        if self.session.is_edited() {
            self.save();
            if self.save_error.is_some() {
                return;
            }
        }
        cx.emit(ProjectEvent::Close);
    }

    pub(crate) fn start_rename(
        &mut self,
        target: RenameTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu = None;
        let snapshot = self.session.snapshot();
        let current = match target {
            RenameTarget::Song => snapshot.song_name.clone(),
            RenameTarget::Node(id) => snapshot
                .node(id)
                .map(|n| n.name.clone())
                .unwrap_or_default(),
            RenameTarget::Layer(id) => layer_name(snapshot, id),
            RenameTarget::Signal(id) => snapshot
                .signals
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_default(),
        };
        let input = cx.new(|cx| TextInput::new(&current, "Name", cx));
        window.focus(&input.read(cx).focus_handle(cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &TextInputEvent, window, cx| {
                let name = match event {
                    TextInputEvent::Confirm(name) => Some(name.clone()),
                    TextInputEvent::Cancel => None,
                    TextInputEvent::Changed(_) => return,
                };
                this.finish_rename(name, window, cx);
            },
        );
        self.rename = Some(Rename {
            target,
            input,
            _subscription: subscription,
        });
    }

    /// Ends renaming, applying `name` if given. Empty names are refused by
    /// the engine, leaving the old name.
    pub(crate) fn finish_rename(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let (Some(rename), Some(name)) = (self.rename.take(), name) {
            let edit = match rename.target {
                RenameTarget::Song => Edit::RenameSong { name },
                RenameTarget::Node(node) => Edit::RenameNode { node, name },
                RenameTarget::Layer(id) => Edit::RenameLayer { id, name },
                RenameTarget::Signal(id) => Edit::RenameSignal { id, name },
            };
            self.session.apply(Command::Edit(edit));
        }
        self.rename = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// The text of the field being edited for `target`, if any.
    pub(crate) fn rename_input(&self, target: RenameTarget) -> Option<Entity<TextInput>> {
        self.rename
            .as_ref()
            .filter(|r| r.target == target)
            .map(|r| r.input.clone())
    }
}

impl Render for ProjectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::frame_log::frame();
        self.last_frame = std::time::Instant::now();
        self.session.tick();
        self.ensure_panes(cx);
        // The song changed (an edit, undo, load): every pane redraws.
        let revision = self.session.snapshot().revision;
        if revision != self.drawn_revision {
            self.drawn_revision = revision;
            self.invalidate_all(cx);
        }
        if !cx.has_active_drag() {
            self.file_hover = None;
        }
        let title = format!("{} — DAW", self.session.snapshot().song_name);
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let theme = Theme::get(cx);
        if matches!(self.page, Page::Arrange | Page::Mixer) {
            let snapshot = self.session.snapshot();
            let previous = self.session.previous_snapshot();
            let rows = crate::tree::display_order(snapshot, &self.folded);
            let last = |id| {
                snapshot
                    .node(id)
                    .or_else(|| previous.and_then(|p| p.node(id)))
                    .cloned()
            };
            let motion = if self.page == Page::Arrange {
                &mut self.row_motion
            } else {
                &mut self.strip_motion
            };
            motion.update(&rows, last, std::time::Instant::now());
            self.autoscroll_clip_drag();
            self.autoscroll_note_drag();
        }
        self.end_note_preview();
        // A drag sounds as it would land, once a frame; a drag dropped
        // with Escape stops sounding.
        self.preview_drag();
        // A metronome volume drag that landed: the app keeps it.
        self.keep_metronome(cx);

        // While something follows the pointer, the pane it started in
        // redraws each frame (others whose size changes redraw on their
        // own; the release redraws them all). Drags from outside the
        // window (files), and drags that preview in other panes, redraw
        // every pane. A new drag that previews elsewhere belongs here.
        let busy = self.dragging() || cx.has_active_drag();
        if busy {
            // A notes signal's settings in the inspector preview on the
            // page's lanes too.
            let everywhere = self.notes_drag.is_some() || self.block_setting_drag();
            match self.pressed_pane.filter(|_| self.dragging() && !everywhere) {
                Some(kind) => self.invalidate(kind, cx),
                None => self.invalidate_all(cx),
            }
        }
        // A hover effect changed: redraw where the pointer is (and was).
        if std::mem::take(&mut self.hover_changed) {
            match self.hovered_pane {
                Some(kind) => self.invalidate(kind, cx),
                None => self.invalidate_all(cx),
            }
        }
        let now = crate::live::Now {
            beat: self.session.position_beats(),
            meters: self.session.meters().clone(),
        };
        // Frames run only while something moves; otherwise the engine is
        // serviced on a timer (`ensure_panes`).
        if busy
            || self.session.snapshot().playing
            || self.live.meters_moving(&now)
            || crate::frame_log::always()
        {
            window.request_animation_frame();
        }

        let page = self.page_area(window, cx);
        let banner = |text: String| {
            div()
                .px(px(18.))
                .py(px(6.))
                .bg(theme.warning)
                .text_color(theme.on_warning)
                .child(text)
        };
        let live = self.live.clone();
        let live_theme = theme;
        let live_layer = canvas(
            |_, _, _| {},
            move |_, (), window, cx| {
                live.paint(&now, &live_theme, window, cx);
                crate::frame_log::painted();
            },
        )
        .absolute()
        .size_full();

        let header = self.render_header(&theme, cx).into_any_element();
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(self.drag_tracker(cx))
            .child(self.input_tracker(cx))
            // Runs before any menu's press-outside handler.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.dismissed_menu = None))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _, _| this.toggle_play()))
            .on_action(cx.listener(|this, _: &Save, _, _| this.save()))
            .on_action(cx.listener(|this, _: &ExportSong, window, cx| {
                this.open_export(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Undo, _, _| this.session.apply(Command::Undo)))
            .on_action(cx.listener(|this, _: &Redo, _, _| this.session.apply(Command::Redo)))
            .on_action(cx.listener(|this, _: &DeleteSelection, _, _| this.delete_selection()))
            .on_action(cx.listener(|this, _: &CancelDrag, _, _| {
                this.cancel_clip_drag();
                this.loop_drag = None;
            }))
            .on_action(cx.listener(|this, _: &ToolSelect, _, _| {
                this.choose_tool(
                    crate::piano_roll::Tool::Select,
                    Some(crate::arrange::ArrangeTool::Pointer),
                )
            }))
            .on_action(cx.listener(|this, _: &ToolPen, _, _| {
                this.choose_tool(crate::piano_roll::Tool::Pen, None)
            }))
            .on_action(cx.listener(|this, _: &ToolScissors, _, _| {
                this.choose_tool(
                    crate::piano_roll::Tool::Scissors,
                    Some(crate::arrange::ArrangeTool::Scissors),
                )
            }))
            .on_action(cx.listener(|this, _: &NudgeUp, _, _| this.nudge_notes(0, 1)))
            .on_action(cx.listener(|this, _: &NudgeDown, _, _| this.nudge_notes(0, -1)))
            .on_action(cx.listener(|this, _: &OctaveUp, _, _| this.nudge_notes(0, 12)))
            .on_action(cx.listener(|this, _: &OctaveDown, _, _| this.nudge_notes(0, -12)))
            .on_action(cx.listener(|this, _: &NudgeLeft, _, _| this.nudge_notes(-1, 0)))
            .on_action(cx.listener(|this, _: &NudgeRight, _, _| this.nudge_notes(1, 0)))
            .on_action(cx.listener(|this, _: &SelectAll, _, _| this.select_all_notes()))
            .on_action(cx.listener(|this, _: &CopyNotes, _, _| this.copy_notes()))
            .on_action(cx.listener(|this, _: &PasteNotes, _, _| this.paste_notes()))
            .on_action(cx.listener(|this, _: &Quantize, _, _| this.quantize_notes()))
            .child(header)
            .children(
                self.session
                    .audio_error
                    .clone()
                    .map(|e| banner(format!("No audio output: {e}"))),
            )
            .children(self.save_error.clone().map(banner))
            .children(self.import_error.clone().map(banner))
            .child(page)
            .child(live_layer)
            .when_some(self.browser.as_ref(), |d, (browser, _)| {
                // Its own cached view: playback frames reuse it.
                d.child(div().absolute().size_full().child(
                    AnyView::from(browser.clone()).cached(StyleRefinement::default().size_full()),
                ))
            })
            .when_some(self.export.as_ref(), |d, (export, _)| {
                // Its own cached view: playback frames reuse it.
                d.child(div().absolute().size_full().child(
                    AnyView::from(export.clone()).cached(StyleRefinement::default().size_full()),
                ))
            })
    }
}

impl ProjectView {
    /// Makes the panes on the first render, and starts servicing the engine
    /// while no frames run.
    fn ensure_panes(&mut self, cx: &mut Context<Self>) {
        if self.panes.is_some() {
            return;
        }
        let project = cx.weak_entity();
        let mut pane = |kind| {
            let project = project.clone();
            cx.new(|_| Pane::new(project, kind))
        };
        self.panes = Some(Panes {
            main: pane(PaneKind::Main),
            bottom: pane(PaneKind::Bottom),
            inspector: pane(PaneKind::Inspector),
            arrange_list: pane(PaneKind::ArrangeList),
        });
        // Idle: no frames, but the engine still needs servicing (garbage,
        // meters, autosave) and a new state must show.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let alive = this.update(cx, |view, cx| {
                    if view.last_frame.elapsed() < std::time::Duration::from_millis(40) {
                        return;
                    }
                    let revision = view.session.snapshot().revision;
                    view.session.tick();
                    let snapshot = view.session.snapshot();
                    // Quieter than a meter shows (-60 dB) is silence.
                    let sounding = view
                        .session
                        .meters()
                        .values()
                        .any(|m| crate::layout::meter_height(m.left.max(m.right)) > 0.);
                    if sounding || snapshot.playing || snapshot.revision != revision {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// A pane's view, for tests that redraw one pane.
    #[cfg(all(test, feature = "ui-tests"))]
    pub(crate) fn pane(&self, kind: PaneKind) -> Entity<Pane> {
        self.panes.as_ref().expect("panes made").get(kind).clone()
    }

    /// Redraws every pane next frame.
    pub(crate) fn invalidate_all(&self, cx: &mut Context<Self>) {
        for kind in PANES {
            self.invalidate(kind, cx);
        }
    }

    fn invalidate(&self, kind: PaneKind, cx: &mut Context<Self>) {
        if let Some(panes) = &self.panes {
            panes.get(kind).update(cx, |_, cx| cx.notify());
        }
    }

    /// Input happened: presses, releases, keys and drags redraw every pane;
    /// a wheel redraws the pane under the pointer.
    fn input(&mut self, at: Option<gpui::Point<Pixels>>, cx: &mut Context<Self>) {
        cx.notify();
        let Some(at) = at else {
            self.invalidate_all(cx);
            return;
        };
        let under = self.pane_at(at);
        match under {
            Some(kind) => self.invalidate(kind, cx),
            None => self.invalidate_all(cx),
        }
        self.hovered_pane = under;
    }

    /// The pointer moved with no button held. Nothing redraws unless a
    /// hover effect says it changed (`hover_changed`), or the pointer moved
    /// to another pane (both redraw, so hover effects there settle).
    fn pointer_moved(&mut self, at: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let under = self.pane_at(at);
        if under != self.hovered_pane {
            for kind in [under, self.hovered_pane].into_iter().flatten() {
                self.invalidate(kind, cx);
            }
            self.hovered_pane = under;
            cx.notify();
        }
    }

    /// Marks that a hover effect changed, so the pane under the pointer
    /// redraws.
    pub(crate) fn hover_changed(&mut self, cx: &mut Context<Self>) {
        self.hover_changed = true;
        cx.notify();
    }

    /// The pane at a point in the window, if any.
    fn pane_at(&self, at: gpui::Point<Pixels>) -> Option<PaneKind> {
        // Panes nest (the node list sits inside Arrange): the smallest wins.
        self.pane_bounds
            .borrow()
            .iter()
            .filter(|(_, b)| b.contains(&at))
            .min_by(|(_, a), (_, b)| {
                let area = |b: &Bounds<Pixels>| (b.size.width / px(1.)) * (b.size.height / px(1.));
                area(a).total_cmp(&area(b))
            })
            .map(|(k, _)| *k)
    }

    /// Window-level listeners that send input to `input`. They run before
    /// the elements' own handlers; the panes redraw after.
    fn input_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let v = view.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| {
                            if e.pressed_button.is_none() {
                                this.pointer_moved(e.position, cx);
                            } else {
                                this.input(None, cx);
                            }
                        });
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| this.input(Some(e.position), cx));
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| {
                            this.pressed_pane = this.pane_at(e.position);
                            this.input(None, cx);
                        });
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| this.input(None, cx));
                    }
                });
                let v = view.clone();
                window.on_key_event(move |_: &KeyDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| this.input(None, cx));
                    }
                });
                let v = view.clone();
                window.on_key_event(move |_: &ModifiersChangedEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| this.input(None, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// A pane in the layout: `outer` sizes it; GPUI reuses its last frame
    /// until it is told to redraw.
    pub(crate) fn pane_element(&self, kind: PaneKind, outer: gpui::Div) -> gpui::Div {
        let Some(panes) = &self.panes else {
            return outer;
        };
        let view =
            AnyView::from(panes.get(kind).clone()).cached(StyleRefinement::default().size_full());
        let bounds = self.pane_bounds.clone();
        outer
            .relative()
            .child(
                canvas(
                    move |b, _, _| {
                        let mut all = bounds.borrow_mut();
                        all.retain(|(k, _)| *k != kind);
                        all.push((kind, b));
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(view)
    }

    /// The page with the bottom panel under Arrange and the inspector at
    /// the right.
    fn page_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let main = self.pane_element(
            PaneKind::Main,
            div().flex_1().min_h_0().min_w_0().flex().flex_col(),
        );
        let column = if self.page == Page::Arrange {
            let bottom = self.prepare_bottom_panel().map(|(height, sliding)| {
                if sliding {
                    window.request_animation_frame();
                }
                // Tool keys go to the panel while the pointer is on it.
                div()
                    .id("note-panel")
                    .flex_none()
                    .h(px(height))
                    .flex()
                    .flex_col()
                    .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                        this.roll_hovered = *hovered;
                    }))
                    .child(self.pane_element(PaneKind::Bottom, div().size_full()))
            });
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(main)
                .children(bottom)
                .into_any_element()
        } else {
            main.into_any_element()
        };
        // Slots of panes not drawn this frame would leave their playheads
        // and meters behind.
        let mut shown = vec![PaneKind::Main];
        if self.page == Page::Arrange {
            shown.push(PaneKind::ArrangeList);
        }
        if self.page == Page::Arrange && (self.note_editor.is_some() || self.audio_editor.is_some())
        {
            shown.push(PaneKind::Bottom);
        }
        if self.inspector_open {
            shown.push(PaneKind::Inspector);
        }
        self.live.keep_only(&shown);
        // Nor may the pointer be found in a pane that is not there.
        self.pane_bounds
            .borrow_mut()
            .retain(|(kind, _)| shown.contains(kind));
        let inspector = self.inspector_open.then(|| {
            self.pane_element(
                PaneKind::Inspector,
                div().flex_none().w(px(self.inspector_width)).h_full(),
            )
            .into_any_element()
        });
        let theme = Theme::get(cx);
        self.with_inspector(column, inspector, &theme, cx)
            .into_any_element()
    }

    /// What a pane shows (`pane.rs`).
    pub(crate) fn render_pane(
        &mut self,
        kind: PaneKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::get(cx);
        match kind {
            PaneKind::Main => match self.page {
                Page::Arrange => self.render_arrange(&theme, cx).into_any_element(),
                Page::Parameters if self.shown_node().is_some() => self
                    .render_parameters(&theme, window, cx)
                    .into_any_element(),
                Page::Signals => self.render_signals(&theme, window, cx).into_any_element(),
                Page::Mixer => self.render_mixer(&theme, cx),
                other => placeholder(&theme, other).into_any_element(),
            },
            PaneKind::Bottom => {
                let panel = if self.audio_editor.is_some() {
                    self.render_audio_editor(&theme, cx)
                } else {
                    self.render_note_editor(&theme, cx)
                };
                panel.unwrap_or_else(|| div().into_any_element())
            }
            PaneKind::Inspector => self.inspector_panel(&theme, cx),
            PaneKind::ArrangeList => self.render_arrange_list(&theme, cx),
        }
    }

    /// Marks that something on screen is mid-animation, so its pane redraws
    /// next frame.
    pub(crate) fn animate(&self) {
        self.animating.set(true);
    }

    /// A marker for the live layer (`live.rs`) in the pane rendering now.
    pub(crate) fn live_slot(&self, pane: PaneKind, live: crate::live::Live) -> gpui::Canvas<()> {
        LiveLayer::slot(&self.live, pane, live)
    }
}

impl ProjectView {
    /// A tool key (1, 2, 3): the piano roll's tool while the pointer is on
    /// it (or Arrange has no such tool), else Arrange's.
    fn choose_tool(
        &mut self,
        note: crate::piano_roll::Tool,
        arrange: Option<crate::arrange::ArrangeTool>,
    ) {
        match arrange {
            Some(tool) if self.page == Page::Arrange && !self.roll_hovered => {
                self.arrange_tool = tool;
                self.scissors_at = None;
            }
            _ => self.set_note_tool(note),
        }
    }

    /// The plugins the browser offers, and a scan's progress.
    pub(crate) fn set_plugins(&mut self, shelf: PluginShelf, cx: &mut Context<Self>) {
        self.plugins = shelf.clone();
        if let Some((browser, _)) = &self.browser {
            browser.update(cx, |b, cx| b.set_shelf(shelf, cx));
        }
    }

    /// Opens the device browser for adding to `target`.
    pub(crate) fn open_browser(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu = None;
        let (side, shelf) = (self.browser_side, self.plugins.clone());
        let browser = cx.new(|cx| DeviceBrowser::new(target, side, shelf, window, cx));
        let subscription = cx.subscribe_in(
            &browser,
            window,
            |this, browser, event: &BrowserEvent, window, cx| {
                let target = browser.read(cx).target();
                if let BrowserEvent::Chose(choice) = event {
                    this.add_device(target, choice.clone());
                }
                this.close_browser(window, cx);
            },
        );
        self.browser = Some((browser, subscription));
        self.invalidate_all(cx);
        cx.notify();
    }

    /// Opens a plugin's add-parameter menu: its parameters, searchable,
    /// minus those that have layers. Choosing one goes on to its first
    /// layer's type, as for built-in devices.
    pub(crate) fn open_param_picker(
        &mut self,
        device: &daw_engine::DeviceSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = OpenMenu::AddParam(device.device);
        self.toggle_menu(menu);
        if self.open_menu != Some(menu) {
            self.param_picker = None;
            window.focus(&self.focus);
            return;
        }
        let params = device
            .plugin
            .as_ref()
            .map(|p| p.params.clone())
            .unwrap_or_default();
        let taken = device
            .params
            .iter()
            .filter(|p| !p.layers.is_empty())
            .map(|p| p.id.index)
            .collect();
        let device_ref = device.device;
        // Effect ids are project-wide, so the device names its node.
        let Some(node) = self
            .session
            .snapshot()
            .nodes
            .iter()
            .find(|n| n.devices.iter().any(|d| d.device == device_ref))
            .map(|n| n.id)
        else {
            return;
        };
        let picker = cx.new(|cx| ParamPicker::new(device_ref, params, taken, window, cx));
        let subscription = cx.subscribe_in(
            &picker,
            window,
            move |this, _, event: &PickerEvent, window, cx| {
                match *event {
                    PickerEvent::Chose(id) => {
                        this.pending_param = Some(daw_engine::ParamId {
                            node,
                            device: device_ref,
                            index: id,
                        });
                        this.layer_menu = crate::parameters::LayerMenu::Kind;
                    }
                    PickerEvent::Close => this.open_menu = None,
                }
                this.param_picker = None;
                window.focus(&this.focus);
                this.invalidate_all(cx);
                cx.notify();
            },
        );
        self.param_picker = Some((picker, subscription));
        self.invalidate_all(cx);
        cx.notify();
    }

    /// File → Export…: opens the export dialog (or brings back the one
    /// open).
    pub(crate) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        if let Some((export, _)) = &self.export {
            if !export.read(cx).is_running() {
                window.focus(&export.focus_handle(cx));
            }
            return;
        }
        let snapshot = self.session.snapshot();
        let seconds = |beats: f64| beats * 60.0 / snapshot.tempo_bpm;
        let song = SongFacts {
            name: snapshot.song_name.clone(),
            folder: self.session.folder().to_owned(),
            song_seconds: seconds(self.session.song_end().as_beats()),
            loop_seconds: snapshot
                .loop_range
                .map(|r| seconds(r.end.as_beats() - r.start.as_beats())),
            sample_rate: self.session.sample_rate(),
        };
        let dialog = cx.new(|cx| ExportDialog::new(self.export_settings, song, window, cx));
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            |this, dialog, event: &ExportEvent, window, cx| {
                this.export_settings = dialog.read(cx).settings();
                match event {
                    ExportEvent::Start(settings, path) => {
                        let job = this.session.export_job(*settings, path.clone());
                        dialog.update(cx, |dialog, cx| dialog.run(job, cx));
                        // The app stays usable while it renders.
                        window.focus(&this.focus);
                    }
                    ExportEvent::Close => {
                        this.export = None;
                        window.focus(&this.focus);
                    }
                }
                this.invalidate_all(cx);
                cx.notify();
            },
        );
        self.export = Some((dialog, subscription));
        self.invalidate_all(cx);
        cx.notify();
    }

    /// The browser's target, while it is open.
    pub(crate) fn browsing(&self, cx: &gpui::App) -> Option<Target> {
        self.browser.as_ref().map(|(b, _)| b.read(cx).target())
    }

    fn close_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((browser, _)) = self.browser.take() {
            self.browser_side = browser.read(cx).side();
        }
        window.focus(&self.focus);
        self.invalidate_all(cx);
        cx.notify();
    }

    /// Adds what the browser chose.
    fn add_device(&mut self, target: Target, choice: Choice) {
        let node = target.node();
        match (target, choice) {
            (Target::Instrument(_), Choice::Instrument(kind)) => self.add_instrument(node, kind),
            (Target::Effect(_), Choice::Effect(kind)) => {
                self.add_effect(node, daw_engine::Effect::new(kind));
            }
            (Target::Effect(_), Choice::Plugin(info)) => {
                let spec = daw_engine::PluginSpec::Vst3 {
                    bundle: info.bundle,
                    cid: info.cid,
                    name: info.name,
                    vendor: info.vendor,
                };
                self.add_effect(node, daw_engine::Effect::plugin(spec));
            }
            (Target::Instrument(_), Choice::Plugin(info)) => {
                let spec = daw_engine::PluginSpec::Vst3 {
                    bundle: info.bundle,
                    cid: info.cid,
                    name: info.name,
                    vendor: info.vendor,
                };
                self.add_plugin_instrument(node, spec);
            }
            _ => {}
        }
    }

    /// Opens `menu`, or closes it if it is open. A press on the trigger of
    /// an open menu first closes it as a press outside the menu; that press
    /// leaves it closed.
    pub(crate) fn toggle_menu(&mut self, menu: OpenMenu) {
        if self.dismissed_menu == Some(menu) {
            return;
        }
        self.open_menu = if self.open_menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        // Menus with steps always open at their first.
        self.layer_menu = crate::parameters::LayerMenu::Kind;
        self.pending_param = None;
    }

    /// Closes the open menu because a press landed outside it.
    pub(crate) fn dismiss_menu(&mut self) {
        self.dismissed_menu = self.open_menu.take();
    }

    /// Whether anything is being dragged (a block, slider, scrollbar...).
    fn dragging(&self) -> bool {
        self.block_drag.is_some()
            || self.base_drag.is_some()
            || self.mix_drag.is_some()
            || self.frame_drag.is_some()
            || self.loop_drag.is_some()
            || self.inspector_resize.is_some()
            || self.notes_drag.is_some()
            || self.clip_drag.is_some()
            || self.mixer_drag.is_some()
            || self.combined_drag.is_some()
            || self.note_editor.as_ref().is_some_and(|e| e.dragging())
            || self.audio_dragging()
            || self.volume_drag.is_some()
    }

    /// While something is being dragged, follows the mouse anywhere in the
    /// window until it is released. Element mouse handlers only fire while
    /// hovered, so these are window-level listeners, registered each frame.
    fn drag_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = self.dragging();
        let cursor = self.drag_cursor();
        let view = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                if !dragging {
                    return;
                }
                // The drag's cursor holds wherever the pointer goes.
                if let Some(style) = cursor {
                    window.set_window_cursor_style(style);
                }
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moved.update(cx, |this, cx| {
                            // A quick click can press and release before
                            // these listeners exist, so the release is never
                            // seen: moving with no button held ends the drag.
                            if event.pressed_button.is_none() {
                                this.finish_drag();
                            } else {
                                this.drag_to(event.position, event.modifiers);
                            }
                            cx.notify();
                        });
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = view.update(cx, |this, cx| {
                            this.finish_drag();
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }
}

/// The current name of a layer, or empty if it no longer exists.
fn layer_name(snapshot: &daw_engine::Snapshot, id: LayerId) -> String {
    snapshot
        .nodes
        .iter()
        .flat_map(|n| &n.devices)
        .flat_map(|d| &d.params)
        .flat_map(|p| &p.layers)
        .find(|(layer_id, _)| *layer_id == id)
        .map(|(_, layer)| layer.name.clone())
        .unwrap_or_default()
}

fn placeholder(theme: &Theme, page: Page) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.text_tertiary)
        .text_size(px(13.))
        .child(format!("{} page: not built yet", page.label()))
}
