# DAW Build Plan

*Status: draft. Companion to [daw-foundation.md](daw-foundation.md).*

This plan lays out the build phases only: what each one is for and when it counts as done. Each phase starts by writing its own detailed design, guided by the foundation document. Open questions get resolved in the phase that first needs the answer.

Every phase follows the foundation principles: test-driven, engine-owned state, a real-time-safe audio thread, and an engine that can run headless.

---

## Phase 0: Project foundations

**Goal:** a workspace where correct, real-time-safe, well-tested code is the easy path.

- Rust workspace and crate boundaries (engine, DSP modules, UI, test harness).
- A local check script (formatting, linting, debug and release tests) run before every commit.
- Real-time safety tooling: the audio thread fails loudly in debug builds if it allocates or locks.
- Headless render harness and audio regression tests (render offline, then compare against reference output).
- Skeleton of the engine/UI boundary: commands in, snapshots out, with undo/redo designed in from the start.

**Done when:** an empty engine can be driven by commands, render silence headlessly, and pass the local checks with the safety checks active.

## Phase 1: Headless engine core

**Goal:** a single node that makes sound in song time.

- Transport and song-time clock (tempo, position, play/stop/loop).
- Live audio output through an existing audio I/O crate.
- One node: MIDI clips playing through a simple built-in instrument; volume and pan.
- Offline render to file.

**Done when:** a MIDI clip plays live and renders offline, and both outputs match.

## Phase 2: Parameters and layers

**Goal:** the one rule, working in the engine.

- Shared parameter model with default states.
- Layer stacks, blocks, blend modes, gaps, and per-block controls (depth, invert, nudge, edge fades).
- Layer evaluation written as pure, heavily tested functions.
- One built-in effect (e.g. a filter) whose DSP is a standalone module.
- Resolves: blend modes and value space, out-of-range values, and knob moves (default state vs recording).

**Done when:** layered automation audibly drives the instrument and the effect, as proven by regression tests.

## Phase 3: Core-loop UI

**Goal:** feel the core loop in hand. Brought forward ahead of signals: the feel of shaping movement is the biggest unknown, and signals extend blocks rather than replace them.

- GPUI app shell, theme tokens (dark and light, per `docs/design/look-and-feel.md`), and the snapshot/command wiring.
- Transport controls and the Parameters view: parameters as rows, layer stacks, drawing and editing blocks.
- Project save and load.

**Done when:** a user can build movement with layers in the UI.

## Phase 4: Motion signals

**Goal:** draw a movement once, use it everywhere.

- Motion signals in song time, with repetition via linked copies.
- Sources: drawn, and from MIDI notes.
- Layers listening to signals, their blocks acting as windows (depth and invert per block, no range mapping; revised in `docs/design/motion-signals.md`).
- "Used by" tracking.
- The Signals view, listening layers in the Parameters view, publishing from Arrange, and the inspector for signal settings.

**Done when:** a MIDI kick pattern publishes a ducking signal that several parameters listen to and stays in sync everywhere, all done in the UI.

> **Milestone gate:** does shaping movement feel great? Review and iterate on phases 2 to 4 before building outward.

## Phase 5: Node tree and arrangement

**Goal:** the project as a tree.

- Node hierarchy with audio flowing up the tree.
- Arrangement view: the node tree next to the timeline.
- Mixer view and the contextual channel strip.
- Folder collapse and summaries (resolves how collapsed nodes summarise their contents).

**Done when:** a multi-node song can be arranged, mixed and automated across the tree.

## Phase 6: Audio signals and analysis

**Goal:** sends, sidechains and audio-derived movement.

- Audio signals: sends and sidechain inputs, pulled across branches.
- Motion sources: audio before effects (analysed in advance) and audio after effects (followed live, then captured and saved).
- Following the loudness of an audio signal inside a layer.
- Resolves: how live-followed signals behave before capture, when their input changes, and during offline render.

**Done when:** a kick on one branch ducks a bass on another without routing cables, both live and in exports.

## Phase 7: Built-in devices and visualisers

**Goal:** a useful native toolkit with integrated visuals.

- Core device set (EQ, compressor, etc.), each built as a portable DSP module.
- Analysis off the audio thread; an isolated rendering module for EQ curves, spectrum and spectrogram.

**Done when:** native devices cover basic mixing needs and their visuals stay smooth under load.

## Phase 8: VST3 hosting

**Goal:** use the plugins you already own.

- Plugin scanning, loading and hosting, with plugin editor windows.
- Plugin parameters mapped into the shared parameter model, so layers and signals work the same on plugins as on native devices.
- Resolves: process isolation for crash protection, and robustness when a plugin update changes its parameters.

**Done when:** installed VST3 instruments and effects can be automated with layers and signals like native devices.

## Phase 9: Table-stakes DAW features

**Goal:** enough everyday features to finish a real track.

- Audio clip import and editing (built early, 2026-10-19), and audio/MIDI recording.
- A MIDI editor (the piano roll, built early, 2026-10-16).
- Mixdown export (built 2026-10-23, `docs/design/export.md`) and stem export.
- Any other gaps found by producing real tracks.

**Done when:** a complete dance track can be produced from start to finish in the app.

## Phase 10: CLAP hosting

**Goal:** second plugin format.

- CLAP hosting through the same parameter model and hosting interfaces as VST3.

**Done when:** CLAP plugins behave the same as VST3 plugins.

## Phase 11: Release

**Goal:** give it away.

- Working name, packaging and installers, user docs, and a contributor guide.
- Platform support decisions and testing.

**Done when:** a public release is published.
