# CLAUDE.md

A DAW where movement is the core of the music and automation is the core of the app.
Read `docs/STATUS.md` first (where things stand, what is next, open questions, how the user likes to work), then `daw-foundation.md` (vision, principles, concepts) and `build-plan.md` (phases).

## Commands

```sh
./scripts/check.sh                           # everything below, plus release builds; must pass before committing
./scripts/full-test.sh                       # check.sh plus the headless UI tests; run occasionally (below)
cargo test --workspace                       # all tests (debug: realtime guard active)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
DAW_BLESS=1 cargo test -p <crate> --test <t> # write/update reference audio
cargo run -p daw-player -- play --compare    # hear the demo live; verifies live == offline
cargo run -p daw-player -- render out.wav    # render the demo to a file
cargo run -p daw-player -- plugins           # scan the VST3 folders and list what is found
cargo run -p daw-ui                          # the app
./scripts/licenses.sh                        # regenerate licenses/NOTICES.md after dependency changes
```

Checks run locally. Run `./scripts/check.sh` before every commit.

**The full test run** (`./scripts/full-test.sh`) adds the headless UI tests (`crates/daw-ui/src/ui_tests/`, behind the `ui-tests` feature): the real app view in a test window, driven by simulated clicks and keys. Run it when a feature is finished, after changing shared input or editing code, before a milestone, and when the user asks; not on every commit.

A manual GitHub workflow (`.github/workflows/cross-platform.yml`) runs the same checks on Linux, Windows and macOS. Only run it when the user asks for a cross-platform check; it never runs on push.

## Crates

- `daw-protocol`: engine/UI boundary. Commands in, snapshots out. Depends on nothing; a front end depends only on this.
- `daw-engine`: authoritative state. `Engine` (control thread: commands, undo, snapshots) and `AudioProcessor` (audio thread), connected only by lock-free `rtrb` queues. The control side compiles the project into an immutable `RenderState` and swaps it in whole. Nodes render in parallel on a worker pool (`parallel.rs`); samples are decoded into a pool (`samples.rs`). **Read `docs/design/audio-engine.md` before engine work.**
- `daw-automation`: pure evaluation of parameter layer stacks (shapes, blend modes, blocks). No engine knowledge; heavily unit tested.
- `daw-engine/src/devices.rs`: every built-in device's parameters (name, unit, scale, default) and how values map onto DSP settings. Layers and devices meet here.
- `daw-dsp`: standalone, portable DSP modules (synth, oscillator, sampler, filter, filter effect, envelope, gain, smoothing, pan). No engine knowledge.
- `daw-stretch`: the real-time time-stretcher (`StemStream`) that plays audio clips; brought over from the user's `audioperformerv2`. Read `docs/design/stretcher.md`.
- `daw-plugins`: plugins in their own processes: the shared-memory audio channel with deadlines (`block`), control messages, the DAW side (`client`) and the host side (`host`). Windows only for now. **Read `docs/design/vst3-hosting.md` and `docs/vst3-notes.md` before plugin work.**
- `daw-test-plugin`: a tiny VST3 plugin (cdylib) for tests: "Test Gain" is a real effect (processor and controller, one parameter, state). Built from source by `daw-plugins`' tests into `target/test-plugin` (`tests/common`). `DAW_TEST_PLUGIN_SCAN=crash|hang` makes loading it misbehave. `cargo test -p daw-plugins --test installed_plugins -- --ignored --nocapture` tries installed commercial effects.
- `daw-device`: audio output devices with no engine knowledge: drivers, device choices, `AudioSettings`, `OutputDevice` (starts any render callback), callback timing and load. Shared with the performance app (`../daw-performance-app`).
- `daw-host`: runs the engine on a real audio device (re-exports `daw-device`; `engine_config` for a device), project folders (sample import), preferences (including audio settings), and the demo song. Shared by the app and the player.
- `daw-player`: command-line host. Live playback, `--compare` (live == offline), offline render to WAV.
- `daw-ui`: the desktop app (binary `daw`), built with GPUI. Views read snapshots and send commands through `Session`; they never touch engine state. **Read `docs/gpui-notes.md` and `docs/design/ui-speed.md` before UI work** and add to them when you learn something. Theme tokens, widgets (slider, switch, text input, tooltip, scrollbars), painting helpers, icons and the frame log live in `daw-widgets`. Ask the user about layout before building each new UI element.
- `daw-widgets`: the look and feel as a library: `theme` (tokens), `slider`, `switch`, `text_input`, `tooltip`, `scroll`, `paint`, `icons` (Lucide SVGs in `crates/daw-widgets/assets/icons/`), `frame_log`. Shared with the performance app, so changes here affect both.
- `daw-rt`: realtime safety guard (`GuardedAllocator`, `realtime`, `count_violations`) and `without_denormals`.
- `daw-testkit`: headless offline render (`render`), reference audio comparison (`assert_reference!`). Linking it installs the guard.

Design decisions per phase live in `docs/design/`. Read the relevant one before changing that area.

## Rules

- **Test-driven.** Write the failing test first. Prefer pure functions for automation/layer logic.
- **Audio thread never allocates, frees, locks or blocks.** Audio-side code runs inside `daw_rt::realtime`; debug builds abort on any allocation there. Replaced state goes back to the control side through a garbage queue to be freed there. Locks and `thread::sleep` are banned by clippy workspace-wide; control-side code that needs one opts in with `#[allow(clippy::disallowed_types, reason = "...")]`.
- **One source of truth.** State lives in the engine. Front ends send `Command`s and render `Snapshot`s.
- **Every edit is undoable.** `Project::apply` returns the inverse edit; new edits go through `History`.
- **Live and offline render share one code path**: `AudioProcessor::process`. Output must not depend on block size; `output_is_identical_for_any_block_size` guards this.
- **Song time.** Positions are `Ticks` (960 per beat). The audio side keeps an anchor beat plus whole frames since it, so every event's frame is a pure function of song time. Move anchors only on tempo change, locate and loop jump.
- Every binary and test harness must install `daw_rt::GuardedAllocator` as the global allocator.
- **Behaviours are listed and guarded.** `docs/behaviours.md` lists every user-facing behaviour with the UI test that guards it. A new behaviour gets a line and a UI test in the same change. **Removing or changing a listed behaviour needs the user's OK**, even as a side effect of other work. UI tests find elements by `.debug_selector(..)` names, never by pixel positions.
- **The UI must be blazing fast and light on the CPU** (the user's priority; `docs/design/ui-speed.md`). Every frame has 6.9 ms at 144 Hz, and GPUI charges for every element and every painted shape each frame, cached or not. So:
  - **Panes redraw only when they change.** Nothing inside a pane reads the song position, meters or the clock: moving things go through the live layer (`ProjectView::live_slot`), timed visuals call `animate()` while they run, and state changed outside input or an edit calls `invalidate_all`. A look that follows the pointer calls `hover_changed` only when it changes (plain moves redraw nothing); a drag redraws only its own pane unless it is listed as previewing elsewhere.
  - **Dense content is painted, not built from elements:** many notes, lines, bars, clips or blocks go in one canvas (`paint::bars`, `clip_paint`, `paint_block`), never one `div` each; the container hit-tests presses and cursors.
  - **Only what is on screen:** skip clips, rows, notes and curve stretches outside the view (`paint::visible_x` / `visible_y`, the content mask).
  - **No frames when idle:** never call `request_animation_frame` unconditionally.
  - **Measure, don't guess:** a new view or dense content gets a scene in `ui_tests/perf.rs` with a budget; budgets only tighten (loosen one only with the user's OK). Check the running app with `DAW_FRAME_LOG=1` (or `always`).
- Reference audio lives in `<crate>/tests/references/*.wav` (32-bit float). Only bless a change you have listened to or can explain.
