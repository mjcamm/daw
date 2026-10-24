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

1. **Measure** (done): speed tests with budgets, frame log, and with `DAW_FRAME_LOG` set a "UI n ms" readout beside the CPU meter (the place the user agreed to, 2026-10-20).
2. **Menus open on press** (done), like native menus.
3. **Panes that redraw only when they change** (done 2026-10-20; steady frames on the full song: Arrange 220 → 25 ms, with the piano roll 240 → 22, Parameters 60 → 3, mixer 12 → 2.5). Arrange, the bottom panel, Parameters, Signals, Mixer and the inspector each become a cached GPUI view (`AnyView::cached`) that renders through `ProjectView` (all state stays there). A pane re-renders when the song changes, on input (only the pane under the pointer for a plain move; all during a drag), or while its own animation runs. **A live layer** above them is the only thing drawn every frame: playheads, meters, fader automation markers, the position readout and CPU meter, at places the panes record when they render.
4. **Dense content painted in one pass** (done 2026-10-20 for Arrange, Parameters and the piano roll; `paint.rs`): notes in clips and combined blocks, grid lines, piano roll notes and velocity bars are one canvas each; bars of one colour go into one path; only clips, blocks, rows, notes and curve stretches near the screen get elements or are painted. The piano roll's resize cursor comes from `note_edit::hit` on hover instead of two hidden elements per note. Then (2026-10-20, second pass): **clips on Arrange are painted** (`clip_paint.rs`: frame, label, notes or waveform; the lane hit-tests presses and the edge cursor); **blocks on layer and signal lanes are painted** (`paint_block`; the lane's existing hit test now also gives the resize cursor); **waveforms are one path** each; the **mixer** builds only strips near the screen (the first always, for fader length) and **Parameters** only rows near the screen.
5. **No frames when idle** (done with step 3): frames run only while playing, dragging, animating or while a meter is above −60 dB; otherwise a 50 ms timer services the engine and redraws when something changes.

## Results (2026-10-20, full-size song)

Headless, release, median ms per frame (`ui_tests/perf.rs`):

| Scene | Before | Steady now | Full redraw now |
|---|---|---|---|
| Arrange | 220 | 2–4 | 9–11 |
| Arrange + piano roll | 240 | 2–4 | 14 |
| Parameters | 60 | 2 | 9–15 |
| Mixer | 12 | 2–3 | 9–12 |

The running app, release, frames forced on (`DAW_FRAME_LOG=always`), Arrange playing: CPU work 2–4 ms a frame; **gap 6.9–7.0 ms on the NVIDIA GPU (on time at 144 Hz), 11–15 ms with 30–40 ms spikes on the built-in Intel GPU**. So:

- **The faster GPU is requested** on computers with two (`gpu.rs`, `build.rs`: the `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` exports). GPUI takes the first adapter Windows lists, which on a laptop is the built-in one. The user chose speed over battery (2026-10-20). One-GPU computers are unaffected.
- **Paths beat quads** for many small bars: GPUI files every shape into an index each frame (~2 µs each, cached or not); a path is one shape however many triangles. On the Intel GPU quads were slower too (20–28 ms against 11–16).
- **Measuring tips:** a busy second copy of the app (the old build redraws everything every frame) skews numbers; quit it first. Phase marks and the frame log both print UTC times. The speed tests print the **best** frame too: compare changes by it, since other programs only ever make frames slower.

### Third pass (2026-10-20)

- **Arrange's node list is its own pane** (`PaneKind::ArrangeList`, nested in the page): lane drags and hover changes redraw the lanes without it. Panes can nest; the pointer belongs to the smallest pane under it. A cached pane also redraws when it moves (GPUI keys its cache on its bounds), so scrolling never leaves it stale.
- **Combined blocks** gather the song's clips once per redraw (not once per parent) and prepare only notes near the screen; the pitch span still covers the whole block so rows stay put while scrolling.
- **The ruler** labels only bars near the screen; **lane rows** far off screen are empty boxes.
- **Where the time goes now:** building the Arrange pane's elements (all our code) takes ~0.4 ms; GPUI's layout and painting of them ~5–6 ms. What is left is GPUI's per-element and per-shape cost; every glyph of every label is a shape, and Arrange shows a lot of text.
- **Measuring:** this machine is noisy (another copy of the app was busy); running the speed tests at high priority (`Start-Process` the test binary, `PriorityClass = High`) gives repeatable numbers.

Release, high priority, best frame: Arrange full 5.7–6.4 ms, Arrange pane alone 5.5–6.4, with the piano roll 7.4–8.8, mixer 6.9–8.7, Parameters 7.2–7.4, Signals 2.6–3.4. Steady (playback) frames 1.3–2.6 ms everywhere.

## How panes work (as built)

- `pane.rs`: `Pane` views (Main = the page, Bottom = piano roll or audio panel, Inspector), each shown through `AnyView::cached` and rendering through `ProjectView::render_pane`. `ProjectView::render` is the shell: header, layout, input tracking, the live layer.
- **When a pane redraws:** the song revision changed (all panes); a press, release, key or modifier change (all); a wheel (the pane under the pointer); the pointer moving into another pane (both); a hover effect reporting a change (`hover_changed`, below); every frame while dragging, **only the pane the press began in** (drags that preview elsewhere, like a notes signal's envelope, and file drags redraw all); while its own animation runs (`ProjectView::animate()` during render).
- **Hover effects report their changes.** A plain mouse move redraws nothing by itself; GPUI redraws a pane on its own when an element's `.hover()` style flips. Our own pointer-driven looks call `ProjectView::hover_changed` only when what they show changes: the clip-end cursor on Arrange (`arrange_edge`), the scissors line, the piano roll's pen cell and note-end cursor (`hover_key`), and the Parameters and Signals lanes' cursors (which read the pointer while drawing, so they report every move). Measured: 6 s of moving over a full song's lanes drew 2 frames.
- **Settle frame:** a pane that redraws draws once more next frame, because some parts size themselves from what the last layout measured (fader length, scrollbar thumbs).
- **Culling:** `paint::visible_x` / `visible_y` give what is near the screen (a quarter screen either side); scrolling redraws the pane the same frame. Painters also skip what is outside the content mask.
- **Live layer** (`live.rs`): panes place `live_slot` markers (playheads, meters, fader markers, parameter values); the markers record their box when laid out; the shell paints them all every frame in one canvas. Slots of panes not shown are dropped each frame.
- **Rule for new UI:** nothing inside a pane may read the song position, meters or the clock without going through the live layer or `animate()`; anything that changes outside input or the song must redraw panes itself (`invalidate_all`); anything that looks different under the pointer (beyond a GPUI `.hover()` style) calls `hover_changed` when that look changes; a new drag that previews in another pane joins the "redraw all" list in `ProjectView::render`.
- **Drags are heard as they move** (`audio-engine.md`, previews): the engine plays a drag's edit each frame without a new revision, so hearing a drag redraws nothing more; its cost is the song compiled once a frame (`perf_drag_preview`, 1 ms budget). Note that a drag frame on the mixer is about 8 ms on the full-size song (the whole mixer pane redraws each frame of a drag, as before).

## GPUI facts this rests on

- `window.request_animation_frame()` notifies only the current view; `window.refresh()` re-renders everything and ignores caches.
- Notifying a view marks its ancestors dirty too, never its siblings, so anything that moves every frame must sit outside cached panes (hence the live layer).
- A cached view reuses last frame's layout, prepaint and paint, including its listeners; prepaint closures inside it do not run again.
- Notifying a view that was not on screen last frame marks nothing dirty, but GPUI also drops the cache of anything not drawn, so a pane that comes back always redraws.
- Reusing a cached pane still copies its drawn shapes every frame, so a pane with tens of thousands of shapes costs milliseconds even when cached (step 4).
