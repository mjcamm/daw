//! The full UI test run: the real app view in a headless window, driven by
//! simulated clicks and keys. One test per user-facing behaviour, listed in
//! `docs/behaviours.md`. Slow to build, so not part of `check.sh`; run with
//! `./scripts/full-test.sh`.

mod arrange;
mod audio_signals;
mod bends;
mod browser;
mod export;
mod live_drags;
mod loop_strip;
mod menus;
mod metronome;
mod parameters;
mod perf;
mod piano_roll;
mod plugin_params;
mod rulers;
mod shape;
mod transport;
mod zoom;

use crate::project::ProjectView;
use crate::session::Session;
use crate::theme::Theme;
use daw_engine::{NodeId, Snapshot};
use gpui::{
    Bounds, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, TestAppContext,
    VisualTestContext, WindowAppearance,
};
use gpui::{Entity, size};
use std::path::PathBuf;

/// A project open in a headless window: the starter song (a Synth node
/// with one clip over bars 1 to 4), no audio device.
pub struct Harness {
    pub view: Entity<ProjectView>,
    pub cx: &'static mut VisualTestContext,
    folder: PathBuf,
    /// Leave the folder in place afterwards (a song made to keep).
    keep: bool,
}

impl Harness {
    pub fn new(name: &str) -> Self {
        let folder =
            std::env::temp_dir().join(format!("daw-ui-test-{name}-{}", std::process::id()));
        Self::in_folder(folder, false)
    }

    /// A project in `folder`, emptied first; kept afterwards if `keep`.
    pub fn in_folder(folder: PathBuf, keep: bool) -> Self {
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("test folder");
        let session = Session::headless(folder.clone()).expect("headless session");
        // Leaked so the window context can live as long as the test.
        let app = Box::leak(Box::new(TestAppContext::single()));
        app.update(|cx| {
            cx.set_global(Theme::for_appearance(WindowAppearance::Dark));
            // The app's own shortcuts, so tests press the keys a user would.
            cx.bind_keys(crate::text_input::key_bindings());
            cx.bind_keys(crate::key_bindings());
        });
        let (view, cx) = app.add_window_view(|window, cx| ProjectView::new(session, window, cx));
        cx.simulate_resize(size(gpui::px(1280.), gpui::px(800.)));
        let mut harness = Self {
            view,
            cx,
            folder,
            keep,
        };
        harness.redraw();
        harness
    }

    /// The project's folder.
    pub fn folder(&self) -> &std::path::Path {
        &self.folder
    }

    /// Draws a frame (the app draws every frame; tests draw on demand).
    pub fn redraw(&mut self) {
        // Twice: rendering takes in the engine's latest snapshot, which the
        // next frame then draws.
        for _ in 0..2 {
            self.cx.update(|window, _| window.refresh());
            self.cx.run_until_parked();
        }
    }

    /// One frame as during playback: the view asked for the next frame, so
    /// only it is notified and cached views may be reused.
    pub fn animation_frame(&mut self) {
        self.view.update(self.cx, |_, cx| cx.notify());
        self.cx.run_until_parked();
    }

    /// Lets rows that slide in or out (new and removed nodes) arrive, then
    /// draws everything where it ends up.
    pub fn settle_rows(&mut self) {
        let start = std::time::Instant::now();
        while start.elapsed() < crate::motion::DURATION {
            std::thread::yield_now();
        }
        self.redraw_all();
        self.redraw_all();
    }

    /// One frame with everything rebuilt, as after an edit.
    pub fn redraw_all(&mut self) {
        self.cx.update(|window, _| window.refresh());
        self.cx.run_until_parked();
    }

    /// The engine's state as the view sees it.
    pub fn snapshot(&mut self) -> Snapshot {
        self.redraw();
        self.view
            .read_with(self.cx, |view, _| view.session().snapshot().clone())
    }

    /// Where an element tagged with `debug_selector` sits on screen.
    pub fn bounds(&mut self, selector: &'static str) -> Bounds<Pixels> {
        self.cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("nothing on screen is tagged {selector:?}"))
    }

    /// The Arrange lane of `node`.
    pub fn arrange_lane(&mut self, node: NodeId) -> Bounds<Pixels> {
        let name: &'static str = Box::leak(format!("arrange-lane-{}", node.0).into_boxed_str());
        self.bounds(name)
    }

    /// A press without a release yet.
    pub fn press(&mut self, at: Point<Pixels>) {
        self.cx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        self.redraw();
    }

    /// Releases the button.
    pub fn release(&mut self, at: Point<Pixels>) {
        self.cx.simulate_event(MouseUpEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
        });
        self.redraw();
    }

    /// Moves the pointer with no button held.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.cx.simulate_event(gpui::MouseMoveEvent {
            position: to,
            modifiers: Modifiers::none(),
            pressed_button: None,
        });
        self.redraw();
    }

    /// Moves the pointer with no button held, drawing only what the app
    /// asks to redraw (as a real move: cached panes stay unless notified).
    pub fn hover(&mut self, to: Point<Pixels>) {
        self.cx.simulate_event(gpui::MouseMoveEvent {
            position: to,
            modifiers: Modifiers::none(),
            pressed_button: None,
        });
        self.cx.run_until_parked();
        // A pane told to redraw while a frame was being drawn draws on the
        // next frame: draw it, reusing every pane not told to.
        self.animation_frame();
    }

    /// Moves the pointer with the left button held (after [`Self::press`]).
    pub fn move_held(&mut self, to: Point<Pixels>) {
        self.cx.simulate_event(gpui::MouseMoveEvent {
            position: to,
            modifiers: Modifiers::none(),
            pressed_button: Some(MouseButton::Left),
        });
        self.redraw();
    }

    /// Turns the wheel `lines` notches (positive: away, up) at `at` with
    /// `modifiers` held.
    pub fn wheel(&mut self, at: Point<Pixels>, lines: f32, modifiers: Modifiers) {
        self.cx.simulate_event(gpui::ScrollWheelEvent {
            position: at,
            delta: gpui::ScrollDelta::Lines(gpui::point(0., lines)),
            modifiers,
            touch_phase: gpui::TouchPhase::Moved,
        });
        self.redraw();
    }

    /// A drag with the left button from `from` to `to`, in `steps` moves.
    pub fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>, steps: usize) {
        self.press(from);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let at = gpui::point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.cx.simulate_event(gpui::MouseMoveEvent {
                position: at,
                modifiers: Modifiers::none(),
                pressed_button: Some(MouseButton::Left),
            });
            self.redraw();
        }
        self.release(to);
    }

    /// A press and release; `clicks` 2 is a double-click.
    pub fn click(&mut self, at: Point<Pixels>, clicks: usize) {
        for count in 1..=clicks {
            self.cx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count: count,
                first_mouse: false,
            });
            self.cx.simulate_event(MouseUpEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count: count,
            });
        }
        self.redraw();
    }

    /// Types `text` into the focused text field.
    pub fn type_text(&mut self, text: &str) {
        self.cx.simulate_input(text);
        self.redraw();
    }

    /// Presses keys, written as in key bindings ("enter", "ctrl-z").
    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.redraw();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}
