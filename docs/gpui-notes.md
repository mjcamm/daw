# GPUI Notes

Working knowledge of GPUI (version 0.2.2 from crates.io), kept because its API docs are thin. Add to this whenever something takes digging to find out.

**Best reference:** the examples in the downloaded crate, `~/.cargo/registry/src/*/gpui-0.2.2/examples/` (`input.rs` for focus, actions and mouse; `painting.rs` for paths; `animation.rs` for SVG assets). Grep `src/` for exact signatures.

## Build

- First build of a crate using GPUI takes about 2 minutes; incremental builds are quick.
- GPUI re-exports `anyhow` and `anyhow::Result` (`gpui::Result`), so no extra dependency is needed for asset sources.

## App and window

```rust
Application::new()
    .with_assets(MyAssets)            // AssetSource, for svg() paths
    .run(|cx: &mut App| {
        cx.bind_keys([KeyBinding::new("space", TogglePlay, None)]);
        cx.open_window(options, |window, cx| {
            cx.set_global(Theme::for_appearance(window.appearance()));
            cx.new(|cx| AppView::new(window, cx))
        })?;
        cx.on_window_closed(|cx| cx.quit()).detach();   // quit with the last window
        cx.activate(true);
    });
```

- Window title: `WindowOptions { titlebar: Some(TitlebarOptions { title: Some(..), .. }), .. }`.
- Light/dark: `window.appearance()` returns `WindowAppearance::{Light, VibrantLight, Dark, VibrantDark}`. Follow changes with `cx.observe_window_appearance(window, |view, window, cx| ...)`; keep the returned `Subscription` in the view or it stops.

## Views and state

- A view implements `Render::render(&mut self, window, cx: &mut Context<Self>) -> impl IntoElement`.
- Event handlers that change the view: `cx.listener(|this, event, window, cx| ...)`. Building them only needs `cx`, so render helpers take `&self` and `&mut Context<Self>`.
- Globals: `impl Global for T {}`, then `cx.set_global(t)` / `cx.global::<T>()`. `Context<V>` derefs to `App`.
- Continuous redraw (playhead, meters): call `window.request_animation_frame()` inside `render`.
- Different branches returning different element types: `.into_any_element()`.

## Layout and styling

- Tailwind-like builder on `div()`: `.flex()`, `.flex_col()`, `.flex_1()`, `.flex_none()`, `.items_center()`, `.justify_between()`, `.gap(px(8.))`, `.px()`, `.py()`, `.w()`, `.h()`, `.size_full()`, `.min_w_0()`, `.min_h_0()`.
- Absolute positioning: parent `.relative()`, child `.absolute().left(px(x)).top(px(y))` (also `.top_0()`, `.bottom_0()`).
- Borders: `.border_1()`, `.border_b_1()`, `.border_r_1()` with `.border_color(c)`. These are 1 px.
- Corners: `.rounded(px(6.))`. Clip children: `.overflow_hidden()`.
- Conditional styling: `.when(cond, |d| d.bg(c))` (from `prelude::*`).
- Hover: `.hover(move |style| style.bg(color))`; copy the colour into a local first because the closure must be `'static`.
- Text: `.text_size(px(12.))`, `.text_color(c)`, `.font_family("Segoe UI")`, `.font_weight(FontWeight::SEMIBOLD)`.
- Colours: `rgb(0xRRGGBB)` and `rgba(0xRRGGBBAA)` give `Rgba`; `.into()` gives `Hsla`, whose `.a` field can be changed for transparency.

## Interaction

- Clicks and scrolling need a stateful element: `div().id("name")` (also `id(("clip", 42u64))`). Then `.on_click(cx.listener(...))`, `.overflow_x_scroll()`.
- Raw mouse: `.on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| ...))`. `e.position` is in window coordinates.
- Turning a click into a local position: put a `canvas(|bounds, _, _| cell.set(bounds), |_, (), _, _| {})` with `.absolute().size_full()` inside the element; its prepaint receives the element's on-screen bounds. Store them in an `Rc<Cell<Bounds<Pixels>>>` held by the view.
- `Pixels` to `f32`: divide by `px(1.)`, e.g. `(e.position.x - origin) / px(1.)`.
- Keyboard: `actions!(namespace, [TogglePlay]);`, bind keys on the app, then on the root element `.track_focus(&focus_handle).on_action(cx.listener(|this, _: &TogglePlay, _, cx| ...))`. Create the handle with `cx.focus_handle()` and call `window.focus(&handle)` so keys work immediately.

## Drawing

- `canvas(prepaint, paint)` gives low-level painting: `window.paint_quad(fill(bounds, color))` and `window.paint_path(path, color)`, with paths from `PathBuilder::fill()` (`move_to`, `line_to`, `close`, `build()`).

## Icons and assets

- `svg().path("icons/play.svg").size(px(14.)).text_color(color)`. The SVG is used as a mask and coloured by `text_color`, so stroke-based icon sets (Lucide) work.
- Paths are resolved through the app's `AssetSource` (`load(&self, path) -> gpui::Result<Option<Cow<'static, [u8]>>>` and `list`). This app embeds icons with `include_bytes!` in `crates/daw-ui/src/icons.rs`.
