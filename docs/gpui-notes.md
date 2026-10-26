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
- Continuous redraw: `window.request_animation_frame()` inside `render` asks for another frame. In this app never call it unconditionally: playheads and meters go through the live layer, and animations through `ProjectView::animate()` (`docs/design/ui-speed.md`).
- Window-level drag listeners registered during paint (our `drag_tracker`) only exist from the frame after the press: a fast click can release before them, and the drag stays live until the next press. End any drag on a `MouseMoveEvent` whose `pressed_button` is `None`.
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
- Paths are resolved through the app's `AssetSource` (`load(&self, path) -> gpui::Result<Option<Cow<'static, [u8]>>>` and `list`). This app embeds icons with `include_bytes!` in `crates/daw-widgets/src/icons.rs`.

## Text input

- GPUI has no text field widget. `crates/daw-widgets/src/text_input.rs` is ours, adapted from `examples/input.rs`: cursor, selection, IME, clipboard, Enter/Escape events. Create with `cx.new(|cx| TextInput::new(text, placeholder, cx))`, focus with `window.focus(&input.read(cx).focus_handle(cx))` (needs the `Focusable` trait in scope), and listen with `cx.subscribe_in`.
- `gpui-component` (a widget library) is built on a republished GPUI fork (`gpui-pre`), not crates.io `gpui`, so its types don't mix with ours.

## Menu bar and modals

- This app draws its own menu bar (a strip of `menu::item` dropdowns in `app.rs`) rather than `cx.set_menus`, so it looks the same on every platform.
- Global actions: `cx.on_action(|_: &Quit, cx| ...)` runs when no element on the focus path handles the action. Element `.on_action` handlers stop propagation by default, so the root view's handler wins when it is on the path.
- Modal: an absolutely positioned, `.occlude()`d full-size scrim as the root's last child, with the panel inside it also `.occlude()`d. Clicks on the panel never reach the scrim, so the scrim's `on_click` can close the modal.
- A modal over a project whose root redraws every playback frame: make it its own view and add it as `AnyView::from(entity).cached(..)` inside an absolute full-size div (`browser.rs`), so playback frames reuse it.
- Long lists: `uniform_list(id, count, cx.processor(|this, range, window, cx| rows))` builds only the rows on screen (all rows one height); `.track_scroll(UniformListScrollHandle)` and `scroll_to_item(i, ScrollStrategy::Center)` follow a keyboard highlight.
- Re-theming at runtime: set the `Theme` global, then `window.refresh()` so every view redraws.

## Events between views

- A view that emits: `impl EventEmitter<MyEvent> for MyView {}`, then `cx.emit(MyEvent::X)`.
- Listen with `cx.subscribe_in(&entity, window, |this, emitter, event: &MyEvent, window, cx| ...)` (gives `window`), or `cx.subscribe(...)` without it. Keep the `Subscription`.
- Child views render by putting the `Entity` in the tree: `.child(entity.clone())`.
- Focusing a new text field from a mouse-down handler: call `cx.stop_propagation()` too, or the press bubbles to an ancestor that tracks focus (the project root) and GPUI focuses that instead, so typing never reaches the field (`ProjectView::open_param_picker`). The mixer's add buttons do the same.

## Keys

- `secondary-` in a binding is Cmd on macOS and Ctrl elsewhere: `KeyBinding::new("secondary-s", Save, ctx)`.
- Bindings take a context predicate: set `.key_context("Workspace")` on an element, then bind with `Some("Workspace && !TextInput")` so shortcuts like space don't steal typing in text fields.

## Clicks, menus and dialogs

- Double-click: `ClickEvent::click_count() >= 2` in `on_click`.
- **Menu triggers must toggle:** pressing the trigger of an open menu fires the menu's `on_mouse_down_out` (capture phase, on mouse down) before the trigger's `on_click` (mouse up), so a naive toggle closes then reopens. Record which menu the press dismissed, clear that record in a root `capture_any_mouse_down` (capture listeners run root first), and have the trigger leave a just-dismissed menu closed (`ProjectView::toggle_menu`, `dismiss_menu`).
- Popup menus: `deferred(anchored().offset(..).child(menu))`, but `anchored` is laid out like any absolute element: at the top-left of its parent, covering the trigger. Wrap it in `div().absolute().top(relative(1.)).left_0()` inside a `.relative()` container holding the trigger, so it hangs below (`menu::float` does this). `deferred` paints it above everything; on the menu use `.occlude()` (blocks clicks through) and `.on_mouse_down_out(...)` to close it.
- Native folder picker: `cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some(..) })` returns a receiver; await it in `cx.spawn(async move |this, cx| { ... this.update(cx, |view, cx| ...) })` and `.detach()` the task.
- Window title: `window.set_window_title(..)`. Only call it when the title changes; render runs every frame.

## Trying the app from an agent (Windows)

`scripts/ui-drive.ps1` drives the running `daw.exe` window with plain Win32 calls, so an agent can look at and use the app without computer-use tools. Coordinates are relative to the window's client area (1280x800 by default, DPI-aware).

```powershell
scripts/ui-drive.ps1 shot out.png        # screenshot the window (then Read the PNG)
scripts/ui-drive.ps1 click 365 51        # also: dclick, rclick, move
scripts/ui-drive.ps1 drag "440,148" "540,148"
scripts/ui-drive.ps1 type "Name{ENTER}"  # SendKeys syntax: ^z is Ctrl+Z, ^q quits (saves first)
scripts/ui-drive.ps1 key 46              # a virtual-key code (46 is Delete)
```

- Launch with `Start-Process target\debug\daw.exe`; quit with `type "^q"` so it saves and frees the exe for the next build.
- Test in a throwaway project, not the user's own songs. Remember that every `^z` really undoes.
- To look mid-drag: `down x y`, one or more `move x y`, `shot`, then `up x y`.
- Separate script calls take hundreds of milliseconds each, too slow to catch a 200 ms animation. Copy the script's Win32 preamble into a scratch script that clicks, then takes a burst of shots a few tens of milliseconds apart.

## Headless UI tests

- `gpui = { features = ["test-support"] }` (our `ui-tests` feature) gives a headless platform that builds and runs on Windows: `TestAppContext::single()`, then `add_window_view(|window, cx| View::new(..))` returns the view and a `VisualTestContext`. Text measures as zero width (no fonts), but layout, hit-testing, `canvas` prepaint and listeners all run.
- Find elements with `.debug_selector(|| "name".into())` (a no-op without `test-support`) and `cx.debug_bounds("name")`.
- `simulate_click` only does single clicks; send `MouseDownEvent`/`MouseUpEvent` yourself with `click_count` 1 then 2 for a double-click.
- Windows only draw when refreshed: `cx.update(|window, _| window.refresh())` then `cx.run_until_parked()`. Twice when a frame's render takes in new state the next frame shows.
- Our harness is `crates/daw-ui/src/ui_tests/mod.rs` (`Harness`).

## Caching views (speed)

- `AnyView::from(entity).cached(StyleRefinement::default().size_full())` reuses the view's last layout and paint until the view is notified or `window.refresh()` runs. The style sizes the view's box; its content is laid out as a root inside it, so give the content `size_full`.
- `window.request_animation_frame()` notifies the view being rendered (inside a cached child view: that view), so a view can redraw itself while it animates.
- Notifying a view dirties its ancestors, never its siblings or children: anything moving every frame must sit outside cached views. See `docs/design/ui-speed.md` for how this app's panes and live layer use it.
- Window-level listeners (`window.on_mouse_event`, `window.on_key_event`, registered in a canvas paint) see every input event before elements do; the app uses them to tell panes to redraw.
- `Bounds` has `intersect`; check emptiness with `size.width <= px(0.)`.
- **Shapes cost every frame:** each painted quad or path is filed into the scene's index each frame, even when a cached view is reused (~2 µs each). Many bars of one colour: one `Path` with `push_triangle` (st `(0,1)` on every vertex means filled) is one shape. Reusing a cached view also costs ~1.3–2.5 µs per element per frame.
- **GPU choice on Windows:** GPUI uses the first adapter (`EnumAdapters(0)`), the built-in GPU on most laptops. The `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` exports (`gpu.rs`) move it to the separate GPU.
- Text in a canvas: `window.text_system().shape_line(text, size, &[TextRun { len, font: font(name), color, .. }], None)`, then `line.paint(origin, window.line_height(), window, cx)`; `line.width` for right-aligning.

## Animating rows

- A row animates by setting its height (and `top` offset, `opacity`) from a progress value each frame; the pane must keep redrawing while it runs (`ProjectView::animate()`).
- To make a row slide into or out of the edge below it, give the outer element the animated height with `.overflow_hidden()`, and put the content in an absolutely positioned full-height child anchored to one edge (Arrange anchors at `.top_0()`, so rows slide out from under their parent above).

## Gotchas

- On Windows, building fails with "failed to remove file ... daw.exe" while the app is running. Close it first.

## Scrolling and hover reveal

- Keeping two areas in step horizontally (e.g. a ruler above scrolling lanes): give the scrolling element `.track_scroll(&handle)` with a `ScrollHandle` kept in the view, and draw the other inside an `.overflow_hidden()` box shifted by `.relative().left(handle.offset().x)` (the offset is negative when scrolled). Scrolling redraws the pane the same frame, so they stay together.
- Vertical page scroll around a horizontally scrolling area: an outer `.overflow_y_scroll().flex().flex_col()` holding a row of [fixed left column][`.overflow_x_scroll()` lanes].
- **Filling the height inside a vertical scroll:** percentage heights (`min_h_full`, `h_full`) do not resolve inside `overflow_y_scroll`. Make the scroll element `.flex().flex_col()` and give its child `.flex_grow().flex_shrink_0()`: it fills the page when short and grows past it when long. Inside a horizontal scroll, make the scroll element `.flex()` so its child stretches to full height, with `.flex_shrink_0()` on a fixed-width child.
- **Wheel as sideways scroll** in one spot (Arrange's nesting map): `on_scroll_wheel` on the area, turn `event.delta.pixel_delta(px(20.))`'s y into x, `scroll::scroll_to` the handle, then `cx.stop_propagation()` so the vertical area around it stays put. Return early without stopping when there is nothing to scroll, so the wheel still reaches the outer area.
- **Nested scroll areas scroll diagonally** unless locked: a sideways-only area (`overflow_x_scroll`) turns a plain wheel's vertical delta into sideways scrolling, while the vertical area around it also scrolls. Set `element.style().restrict_scroll_to_axis = Some(true)` (see `scroll::axis_locked`) on both. Windows already sends Shift+wheel as a horizontal delta, so locked areas get "wheel = up/down, Shift+wheel = sideways" for free.
- GPUI draws no scrollbars. Read a scroll area through its `ScrollHandle`: `bounds()` (visible size), `max_offset()` (content minus visible), `offset()` (negative when scrolled); draw a thumb with `relative()` positions and move it with `set_offset`.
- Setting scroll from code: `ScrollHandle::set_offset(point(x, y))` (offsets are negative when scrolled; not clamped until layout). `bounds()` gives the visible viewport. Used to keep the middle of the view still while zooming.
- Show a control only while its row is hovered: name the row `.group("row-1")`, then on the control `.opacity(0.).group_hover("row-1", |s| s.opacity(1.))`.
- Curves: `PathBuilder::stroke(px(1.5))` for lines, `PathBuilder::fill()` for the area under them, painted in a `canvas` sized to the row.

## Drag and drop

- The dragged value is any `Clone + 'static` type. On the handle: `.on_drag(value, |value, grab_offset, window, cx| cx.new(|_| preview))`, where the preview is a view (the value can be its own view by implementing `Render`).
- On targets: `.drag_over::<T>(|style, value, window, cx| style.bg(..))` to highlight while hovering, and `.on_drop(cx.listener(|this, value: &T, window, cx| ...))`. Both are filtered by type, so different drag kinds don't interfere.
- `.cursor_move()` on the handle shows a move cursor.

- **Files dragged in from the system** (Explorer, Finder) arrive as an ordinary drag of `gpui::ExternalPaths`: `.on_drag_move::<ExternalPaths>(cx.listener(|this, e: &DragMoveEvent<ExternalPaths>, _, cx| ...))` follows the pointer (`e.event.position`, `e.drag(cx).paths()`), and `.on_drop(cx.listener(|this, paths: &ExternalPaths, _, _| ...))` receives them. Nothing says when the drag leaves the window, so clear hover state in `render` once `!cx.has_active_drag()` (Arrange's file drop does this).

## Free-form drags (moving, resizing, scrubbing)

- **Nested handlers both fire:** a press on a child (a clip's resize edge) also reaches its parents' `on_mouse_down` (the clip body, the lane). Call `cx.stop_propagation()` in the child's handler to keep it to the child.
- **Overlapping elements both get the press.** Mouse handlers fire on every hovered hitbox under the pointer, not just the top one, unless an element `.occlude()`s what is behind it. A resize edge hanging 2 px over a scrollbar started a scrollbar drag too, which won. Keep handles inside their own element and `.occlude()` them.
- `.on_mouse_move` only fires while the element is hovered, so it loses a drag once the pointer leaves. To follow the mouse anywhere, register window-level listeners while dragging, from a `canvas` paint closure: `window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| ...)` (and `MouseUpEvent` to finish). They last one frame, so register them on every frame while the drag is active (the pane a drag began in redraws every frame while it lasts). They see both phases; act on one (`DispatchPhase::Capture`). Reach the view with a `cx.entity().downgrade()` captured from `render`, then `weak.update(cx, |this, cx| ...)`.
- `MouseDownEvent` has `click_count` (double-click without `on_click`) and `modifiers` (`.alt`, `.shift`, `.platform`).
- Cursors: `.cursor_ew_resize()`, `.cursor_grab()`, `.cursor_crosshair()` and so on, from `gpui-macros` `styles.rs`. An invisible absolutely-positioned child with a cursor style gives a zone its own cursor without blocking the parent's mouse handlers.
- Render helpers that take `cx: &mut Context<Self>` and return `impl IntoElement` capture `cx`'s lifetime (Rust 2024), so two calls in one expression fail to borrow. Return a concrete type (`Stateful<Div>`) instead.
- **A keystroke a shortcut takes never reaches key listeners** (`window.on_key_event`), and an action handler doesn't redraw by itself: a shortcut that changes state shows only on the next input. `cx.observe_keystrokes` runs after every keystroke, its actions done; the project view redraws there (`ProjectView::_keystrokes`).
- **Tests: `debug_bounds` never forgets** (GPUI 0.2.2): the map of tagged elements isn't cleared between frames, so an element drawn once is found ever after. It can show something appearing, never something gone: check the view's state for that.
- **A cursor that holds through a drag:** `window.set_window_cursor_style(style)` during paint (our `drag_tracker` canvas) beats every element's cursor, so it stays when the pointer leaves the element it started on. GPUI picks the cursor anew each frame from what is under the pointer, so a cursor that goes stale is our own state: work out what is under the pointer during render, against what is drawn, not from the last mouse move (Arrange's lanes keep the pointer, `arrange_pointer`, and do this). A move handler that decides whether to redraw must compare with what was drawn (`arrange_hover`, set while drawing), never with its own last result, or a pane drawn since (after a drag) is never told it went stale.
- **Custom cursors** (GPUI 0.2.2 on Windows has no four-way move or loop cursor; `cursor_move()` is a closed hand that falls back to the arrow): hide the system cursor with `.cursor_none(CursorStyle::Arrow)` (the argument is ignored) on the hovered element and draw an icon at `window.mouse_position()`. The deepest hovered element's cursor wins, so leave no cursor-styled child over the zone. Draw the icon with `deferred(anchored().position(pointer - half_icon))`, not as a child of the hovered element: it is then on top of everything (ghosts, other rows, the live layer's playhead), never clipped by its row, and exactly at the pointer while the content scrolls under it. Keep the pointer inside the window: `anchored` keeps it on screen.
- Percentage positions: `.top(relative(0.25))` (from `gpui::relative`).
- Round dots in a canvas: `window.paint_quad(fill(bounds, color).corner_radii(px(r)))`.
