//! An open project: header and the current page.

use crate::block_edit::Snap;
use crate::frame::{FrameDrag, TimelineView};
use crate::layout::PIXELS_PER_BEAT;
use crate::menu::OpenMenu;
use crate::parameters::{BaseDrag, BlockDrag, DEFAULT_ZOOM, MixDrag};
use crate::session::Session;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;
use crate::zoom::Zoom;
use daw_engine::{
    BlockId, Command, Edit, LayerId, LoopRange, MidiClip, NodeId, NodeSnapshot, SignalId, Snapshot,
};
use gpui::{
    Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable, MouseMoveEvent,
    MouseUpEvent, Pixels, Subscription, Window, actions, canvas, div, prelude::*, px,
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
        Quantize
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
}

pub struct ProjectView {
    pub(crate) session: Session,
    pub(crate) page: Page,
    /// The loop region to restore when looping is switched back on.
    pub(crate) saved_loop: Option<LoopRange>,
    /// Where the timeline ruler was last drawn, for turning clicks into
    /// song positions.
    pub(crate) ruler_bounds: Rc<Cell<Bounds<Pixels>>>,
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
    /// The piano roll in the bottom panel, if a clip is open.
    pub(crate) note_editor: Option<crate::piano_roll::NoteEditor>,
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
    pub(crate) save_error: Option<String>,
    window_title: String,
    focus: FocusHandle,
}

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
        let saved_loop = session.snapshot().loop_range;
        Self {
            session,
            page: Page::Arrange,
            saved_loop,
            ruler_bounds: Rc::default(),
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
            note_editor: None,
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
            save_error: None,
            window_title: String::new(),
            focus,
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

    pub(crate) fn toggle_loop(&mut self) {
        let current = self.session.snapshot().loop_range;
        if current.is_some() {
            self.saved_loop = current;
            self.session.apply(Command::SetLoop(None));
        } else if let Some(range) = self.saved_loop {
            self.session.apply(Command::SetLoop(Some(range)));
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
        // Redraw every frame: the playhead and meters move continuously.
        window.request_animation_frame();
        self.session.tick();
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
        // The piano roll slides up under Arrange.
        let editor = if self.page == Page::Arrange {
            self.render_note_editor(&theme, cx)
        } else {
            None
        };
        let page = match self.page {
            Page::Arrange => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.render_arrange(&theme, cx))
                .children(editor)
                .into_any_element(),
            Page::Parameters if self.shown_node().is_some() => self
                .render_parameters(&theme, window, cx)
                .into_any_element(),
            Page::Signals => self.render_signals(&theme, window, cx).into_any_element(),
            Page::Mixer => self.render_mixer(&theme, cx),
            other => placeholder(&theme, other).into_any_element(),
        };
        let banner = |text: String| {
            div()
                .px(px(18.))
                .py(px(6.))
                .bg(theme.warning)
                .text_color(theme.on_warning)
                .child(text)
        };

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(self.drag_tracker(cx))
            // Runs before any menu's press-outside handler.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.dismissed_menu = None))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _, _| this.toggle_play()))
            .on_action(cx.listener(|this, _: &Save, _, _| this.save()))
            .on_action(cx.listener(|this, _: &Undo, _, _| this.session.apply(Command::Undo)))
            .on_action(cx.listener(|this, _: &Redo, _, _| this.session.apply(Command::Redo)))
            .on_action(cx.listener(|this, _: &DeleteSelection, _, _| this.delete_selection()))
            .on_action(cx.listener(|this, _: &CancelDrag, _, _| {
                this.cancel_clip_drag();
            }))
            .on_action(cx.listener(|this, _: &ToolSelect, _, _| {
                this.set_note_tool(crate::piano_roll::Tool::Select)
            }))
            .on_action(cx.listener(|this, _: &ToolPen, _, _| {
                this.set_note_tool(crate::piano_roll::Tool::Pen)
            }))
            .on_action(cx.listener(|this, _: &ToolScissors, _, _| {
                this.set_note_tool(crate::piano_roll::Tool::Scissors)
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
            .child(self.render_header(&theme, cx))
            .children(
                self.session
                    .audio_error
                    .clone()
                    .map(|e| banner(format!("No audio output: {e}"))),
            )
            .children(self.save_error.clone().map(banner))
            .children(self.import_error.clone().map(banner))
            .child(self.with_inspector(page, &theme, cx))
    }
}

impl ProjectView {
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
            || self.inspector_resize.is_some()
            || self.notes_drag.is_some()
            || self.clip_drag.is_some()
            || self.mixer_drag.is_some()
            || self.combined_drag.is_some()
            || self.note_editor.as_ref().is_some_and(|e| e.dragging())
    }

    /// While something is being dragged, follows the mouse anywhere in the
    /// window until it is released. Element mouse handlers only fire while
    /// hovered, so these are window-level listeners, registered each frame.
    fn drag_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = self.dragging();
        let view = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                if !dragging {
                    return;
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
