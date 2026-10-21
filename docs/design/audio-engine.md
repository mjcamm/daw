# Audio Engine

How the engine fits together, for picking up work on it. Feature details live in their own docs: `samples.md` (SingleSampler, sample pool), `audio-clips.md` (audio clips and stretching), `audio-settings.md` (devices, safety, multicore), `motion-signals.md` and `parameters-and-layers.md` (automation), `node-tree.md` (the tree).

## Two sides

| Control side (`Engine`, any thread the front end owns) | Audio side (`AudioProcessor`, the device's thread) |
|---|---|
| Applies `Command`s, keeps undo, builds `Snapshot`s | Renders sound; never allocates, frees, locks or blocks |
| Compiles the project into an immutable `RenderState` after every edit | Swaps a new `RenderState` in whole at the start of a block |
| Decodes sample files into the pool; frees retired states | Hands each replaced state back through the garbage queue |

They meet only through two lock-free `rtrb` queues (`AudioMsg` in, retired states out) and `SharedStatus` atomics (position, meters). `Engine::maintain` delivers messages and frees garbage; the UI calls it every frame. `Engine::reconnect` builds a new processor for a new device setup and keeps everything else.

## A block's life (`audio.rs`)

1. `process` wraps the block in `daw_rt::realtime` (debug builds abort on any allocation) and `without_denormals`.
2. `receive` applies queued messages: play/stop/locate/loop, previews, the test tone, state swaps (`assign_players` then `plan_levels`).
3. The block is cut into chunks of at most `CHUNK_FRAMES` (1024). For each chunk, `render` runs the tree **level by level** (leaves first) on the worker pool. Each node's `NodePlayer::render_chunk` walks the chunk with **its own copy of the transport**: parameters update every `CONTROL_FRAMES` (32) counted from the processor's start, notes start and end on their exact frames, loop jumps release and re-seek. A parent's bus starts as its children's buses summed in render order.
4. The Song's bus goes out through `safe` (no NaN, clip at ±1) to the device.

## Invariants (each guarded by tests)

- **Same output for any block size and any thread count**, so live play equals offline render (`tests/playback.rs`, `tests/parallel.rs`). Anything that depends on where a block or span starts breaks this: keep per-frame state in the voice or player, and update parameters only on the control grid.
- **Song time is exact**: the transport stores an anchor beat plus whole frames since it (`transport.rs`); anchors move only on tempo change, locate and loop jump.
- **Nothing is freed on the audio thread**: players, instruments and effects live in pools sized up front (`MAX_NODES`, `MAX_INSTRUMENTS`, `MAX_EFFECTS`); sample audio is shared by `Arc` owned by render states, which are dropped only on the control side.
- **Reference audio** (`<crate>/tests/references/*.wav`) only changes when a change was heard or can be explained; bless with `DAW_BLESS=1`.

## Where things are

| Path | What |
|---|---|
| `daw-protocol/src/lib.rs`, `automation.rs` | Commands, edits, snapshots, ids, instrument and effect kinds |
| `daw-protocol/src/audio_clip.rs`, `clip_loop.rs`, `peaks.rs`, `time.rs` | Audio clips, clip loops (expanded into plain pieces at compile time), waveform overviews, ticks |
| `daw-engine/src/lib.rs` | `Engine`: commands, snapshots, `reconnect`, the sample folder |
| `daw-engine/src/project.rs` | The project model; `Project::apply` validates every edit and returns its inverse |
| `daw-engine/src/schedule.rs` | Compiling the project into a `RenderState` |
| `daw-engine/src/audio.rs` | `AudioProcessor`, `NodePlayer`, instrument and effect instances, the output guard, the test tone |
| `daw-engine/src/parallel.rs` | The worker pool and `Shards` (the only `unsafe` in the engine) |
| `daw-engine/src/devices.rs` | Every built-in device's parameters and how values reach the DSP |
| `daw-engine/src/samples.rs` | WAV decoding, the sample pool, rate conversion for clips, waveform overviews |
| `daw-engine/src/clips.rs` | Audio clips on the audio thread: `ClipPlayer` (one stretcher per audio node) |
| `daw-engine/src/plugins.rs` | Plugin effects on the control side: instances in a host process, channels to the audio side (`vst3-hosting.md`) |
| `daw-stretch/src/` | The time-stretcher, `StemStream` (`stretcher.md`) |
| `daw-engine/src/file.rs`, `upgrade.rs` | The project file (version 16, `file::VERSION`) and converting older versions |
| `daw-dsp/src/` | Standalone DSP: synth, sampler, filter, envelopes, pan, smoothing |
| `daw-rt/src/` | The allocation guard and `without_denormals` |
| `daw-host/src/lib.rs` | Devices through `cpal`: drivers, device choices, opening a stream, `CallbackStats` load |
| `daw-host/src/preferences.rs` | App preferences, including `AudioSettings` |
| `daw-host/src/projects.rs`, `demo.rs` | Project folders, sample import, recent projects; the demo and starter song |
| `daw-host/examples/devices.rs` | Prints what each audio device reports |
| `daw-testkit/src/` | Offline `render`, `VARIED_BLOCKS`, `assert_reference!` |

## Adding a built-in instrument or effect

1. The DSP as a standalone type in `daw-dsp` (no heap memory; `new`, `note_on`/`note_off`/`release_all` or `process`, a render that overwrites its output).
2. A kind in `InstrumentKind` / `EffectKind` (`daw-protocol/src/automation.rs`); it is saved by name, so adding one needs no file upgrade.
3. Its parameters and their mapping in `devices.rs` (`instrument_params` / `effect_params`, name, description, `INSTRUMENTS` for the browser).
4. A variant in `InstrumentInstance` / `EffectInstance` (`audio.rs`); the compiler points at every `match` to extend.
5. Tests: the DSP alone, then through the engine (exact timing, any block size).
