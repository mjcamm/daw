//! The desktop app.

mod app;
mod arrange;
mod block_edit;
mod clip_drag;
mod clip_edit;
mod frame;
mod header;
mod icons;
mod inspector;
mod launch;
mod layout;
mod menu;
mod mixer;
mod motion;
mod note_edit;
mod parameters;
mod piano_roll;
mod project;
mod scroll;
mod session;
mod signals;
mod slider;
mod text_input;
mod theme;
mod tooltip;
mod tree;
mod zoom;

use app::{AppView, OpenPreferences, Quit};
use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, prelude::*,
    px, size,
};
use project::{
    CancelDrag, CopyNotes, DeleteSelection, NudgeDown, NudgeLeft, NudgeRight, NudgeUp, OctaveDown,
    OctaveUp, PasteNotes, Quantize, Redo, Save, SelectAll, TogglePlay, ToolPen, ToolScissors,
    ToolSelect, Undo,
};
use theme::Theme;

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

/// App shortcuts. `secondary` is Cmd on macOS and Ctrl elsewhere. Shortcuts
/// that would steal typing are off while a text field has focus.
fn key_bindings() -> Vec<KeyBinding> {
    let project = Some(project::KEY_CONTEXT);
    let not_typing = Some("Workspace && !TextInput");
    vec![
        KeyBinding::new("space", TogglePlay, not_typing),
        KeyBinding::new("secondary-s", Save, project),
        KeyBinding::new("secondary-z", Undo, not_typing),
        KeyBinding::new("secondary-shift-z", Redo, not_typing),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", OpenPreferences, None),
        KeyBinding::new("delete", DeleteSelection, not_typing),
        KeyBinding::new("escape", CancelDrag, not_typing),
        KeyBinding::new("backspace", DeleteSelection, not_typing),
        // The piano roll, while it is open.
        KeyBinding::new("1", ToolSelect, not_typing),
        KeyBinding::new("2", ToolPen, not_typing),
        KeyBinding::new("3", ToolScissors, not_typing),
        KeyBinding::new("q", Quantize, not_typing),
        KeyBinding::new("up", NudgeUp, not_typing),
        KeyBinding::new("down", NudgeDown, not_typing),
        KeyBinding::new("shift-up", OctaveUp, not_typing),
        KeyBinding::new("shift-down", OctaveDown, not_typing),
        KeyBinding::new("left", NudgeLeft, not_typing),
        KeyBinding::new("right", NudgeRight, not_typing),
        KeyBinding::new("secondary-a", SelectAll, not_typing),
        KeyBinding::new("secondary-c", CopyNotes, not_typing),
        KeyBinding::new("secondary-v", PasteNotes, not_typing),
    ]
}

fn main() {
    Application::new()
        .with_assets(icons::Assets)
        .run(|cx: &mut App| {
            cx.bind_keys(text_input::key_bindings());
            cx.bind_keys(key_bindings());
            app::register_global_actions(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("DAW".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                cx.set_global(Theme::for_appearance(window.appearance()));
                cx.new(|cx| AppView::new(window, cx))
            });
            if let Err(e) = opened {
                eprintln!("could not open the window: {e}");
                cx.quit();
            }
            cx.on_window_closed(|cx| cx.quit()).detach();
            cx.activate(true);
        });
}
