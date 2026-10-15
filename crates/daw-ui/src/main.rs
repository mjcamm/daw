//! The desktop app.

mod app;
mod arrange;
mod header;
mod icons;
mod layout;
mod session;
mod theme;

use app::{AppView, TogglePlay};
use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, prelude::*,
    px, size,
};
use theme::Theme;

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

fn main() {
    Application::new()
        .with_assets(icons::Assets)
        .run(|cx: &mut App| {
            cx.bind_keys([KeyBinding::new("space", TogglePlay, None)]);
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
