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
use daw_engine::{BlockId, Command, Edit, LayerId, LoopRange, SignalId};
use gpui::{
    Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable, MouseMoveEvent,
    MouseUpEvent, Pixels, Subscription, Window, actions, canvas, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;

actions!(daw, [TogglePlay, Save, Undo, Redo, DeleteSelection]);

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
    Node,
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
    /// Whether the node is selected; the Parameters page shows the selected
    /// node.
    pub(crate) node_selected: bool,
    /// Where the Parameters page lanes were last drawn: the top-left is
    /// beat 0 of the first row.
    pub(crate) lanes_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Snap or slide for block edits. A UI preference, not project state.
    pub(crate) snap: Snap,
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
    /// A layer just jumped to from elsewhere, highlighted briefly so it is
    /// easy to spot.
    pub(crate) highlight: Option<(LayerId, std::time::Instant)>,
    /// A scrollbar or zoom slider being dragged.
    pub(crate) frame_drag: Option<FrameDrag>,
    pub(crate) rename: Option<Rename>,
    pub(crate) save_error: Option<String>,
    window_title: String,
    focus: FocusHandle,
}

impl EventEmitter<ProjectEvent> for ProjectView {}

impl ProjectView {
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
            node_selected: false,
            lanes_bounds: Rc::default(),
            snap: Snap::default(),
            selected_block: None,
            block_drag: None,
            base_drag: None,
            mix_drag: None,
            arrange_view: TimelineView::new(Zoom::new(PIXELS_PER_BEAT, 56.)),
            parameters_view: TimelineView::new(DEFAULT_ZOOM),
            signals_view: TimelineView::new(crate::signals::DEFAULT_ZOOM),
            highlight: None,
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
            RenameTarget::Node => snapshot.node.name.clone(),
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
                RenameTarget::Node => Edit::RenameNode { name },
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
        let title = format!("{} — DAW", self.session.snapshot().song_name);
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let theme = Theme::get(cx);

        let page = match self.page {
            Page::Arrange => self.render_arrange(&theme, cx).into_any_element(),
            Page::Parameters if self.node_selected => self
                .render_parameters(&theme, window, cx)
                .into_any_element(),
            Page::Signals => self.render_signals(&theme, window, cx).into_any_element(),
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
            .child(self.render_header(&theme, cx))
            .children(
                self.session
                    .audio_error
                    .clone()
                    .map(|e| banner(format!("No audio output: {e}"))),
            )
            .children(self.save_error.clone().map(banner))
            .child(page)
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
                            this.drag_to(event.position, event.modifiers);
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
        .devices
        .iter()
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
