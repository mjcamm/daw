# UI Speed

*Started 2026-10-20. The user: "I want the UI to feel absolutely blazing fast so ensuring its foundations are solid is key"; "blazing speed efficient ui and cpu handling, and everything super responsive, meters ui feel etc". Work in progress; steps below.*

## Targets

- A frame during playback (steady) under **1 ms** of UI work; any frame after input (full) under **5 ms**, on a full-size song. The user's display is 144 Hz: 6.9 ms per frame in all.
- Idle (stopped, nothing moving): no frames drawn, almost no CPU.
- Input shows on the next frame. Menus open on press.

## Measuring

- **Speed tests** (`crates/daw-ui/src/ui_tests/perf.rs`, part of `./scripts/full-test.sh`, release, one at a time): a full-size song (4 groups × 6 synths, a 4-bar clip every 4 bars across 64 bars, about 12,000 notes; 4 parameters × 3 layers of blocks), timed headless per scene. **Steady**: the view asks for the next frame (what playback does). **Full**: everything rebuilt. Budgets fail the run; they tighten as each step lands. Headless has no fonts or GPU, so real frames cost a little more.
- **The running app:** `DAW_FRAME_LOG=1` prints frame gaps every 2 s (`frame_log.rs`). Drive it with `scripts/ui-drive.ps1`.

## What we found (2026-10-20)

| | Before |
|---|---|
| `cargo run` (debug), starter song, Arrange | ~50 ms a frame (20 fps): GPUI itself unoptimized |
| Same, libraries optimized in debug | ~9 ms |
| Release, starter song | ~7 ms (on time at 144 Hz); 11–20 ms with the piano roll open |
| Release, full-size song (headless), Arrange / + piano roll / Parameters / Mixer | 220 / 240 / 60 / 12 ms |

- **Libraries are optimized in debug builds** (`[profile.dev.package."*"]`, our crates stay debug so the realtime guard runs). One slow first build (~25 min), then as before.
- **The cost is GPUI laying out and painting elements**: about 4–13 µs each, worse per element as counts grow (headless baseline). A frame can afford roughly 500–1,000. Arrange on a full song makes tens of thousands: every note of every clip is an element, drawn again in each parent's combined block and in the Song row, across the whole song, not only what is on screen. Building the elements is a small part (Arrange: 52 of 378 ms).
- **Everything rebuilds every frame**: `ProjectView` is one view and asks for a new frame every frame (for the playhead and meters), so every page and panel is rebuilt ~144 times a second even when nothing changed.

## The plan

1. **Measure** (done): speed tests with budgets, frame log. An on-screen frame-time readout waits on the user's choice of place.
2. **Menus open on press** (done), like native menus.
3. **Panes that redraw only when they change.** Arrange, the bottom panel, Parameters, Signals, Mixer and the inspector each become a cached GPUI view (`AnyView::cached`) that renders through `ProjectView` (all state stays there). A pane re-renders when the song changes, on input (only the pane under the pointer for a plain move; all during a drag), or while its own animation runs. **A live layer** above them is the only thing drawn every frame: playheads, meters, fader automation markers, the position readout and CPU meter, at places the panes record when they render.
4. **Dense content painted in one pass**: notes, grid lines, waveforms and clip contents become one `canvas` per lane or clip, and only what is on screen is drawn.
5. **No frames when idle**: ask for frames only while playing, animating, dragging or while meters fall; keep servicing the engine on a timer.

## GPUI facts this rests on

- `window.request_animation_frame()` notifies only the current view; `window.refresh()` re-renders everything and ignores caches.
- Notifying a view marks its ancestors dirty too, never its siblings, so anything that moves every frame must sit outside cached panes (hence the live layer).
- A cached view reuses last frame's layout, prepaint and paint, including its listeners; prepaint closures inside it do not run again.
