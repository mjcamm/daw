# VST3 Hosting (Phase 8 design)

*Status: agreed with the user, 2026-10-21. Decided with the user: plugins run **in their own processes from the start** ("build it right first"), best practice everywhere, **floating windows** for plugin editors (and later the app's own native plugins), parameters **picked from a list or learned by wiggling a knob**, and the **`vst3` crate** for the bindings. Nothing is built yet.*

## Licences and references

API facts and gotchas: `docs/vst3-notes.md`.


- **Steinberg VST3 SDK:** MIT since VST 3.8 (October 2025). Its host examples (audio host, editor host, validator) are the reference and may be adapted, keeping Steinberg's copyright notice.
- **`vst3` crate** (Coupler, MIT/Apache): Rust bindings generated from the SDK headers, shipped pre-generated (no SDK or libclang at build time), with helpers for calling COM objects and implementing COM interfaces. Unsafe and low-level by design; this project builds the safe host layer on top.
- **JUCE** (AGPLv3 or commercial) and **Ardour / Carla** (GPL): read to understand, never copied (copying would force those licences on this project).
- **yabridge** (bridges Windows plugins to Linux hosts across processes) has the most detailed public write-up of out-of-process plugin hosting; its lessons are used below.

## The shape

```text
 DAW process                                   plugin host process (one per group)
 ┌──────────────────────────────┐              ┌───────────────────────────────────┐
 │ UI ── Engine (control side)  │◄── control ─►│ main thread: loads .vst3, owns    │
 │          │                   │   channel    │ controllers and editor windows    │
 │ audio thread / workers       │              │                                   │
 │   per plugin slot: write in, │◄── shared ──►│ audio thread (MMCSS "Pro Audio"): │
 │   signal, wait (deadline),   │   memory +   │   wait, process(), signal done    │
 │   read out                   │   events     │                                   │
 └──────────────────────────────┘              └───────────────────────────────────┘
        watchdog: notices a host process exit or a missed deadline
```

- **No plugin code ever runs in the DAW process.** A crash or hang takes down only its host process; the DAW keeps playing (that plugin goes silent) and offers Reload, as Bitwig does.
- **The DAW's realtime rules stay absolute.** The DAW audio thread only copies into shared memory, signals and waits with a deadline, so the allocation guard needs no exception. In the host process, plugin calls run inside a counting guard (never aborting), reported as diagnostics ("this plugin allocates on the audio thread").
- **Editors live in the host process**, so a plugin window is a native top-level window owned by the process that runs the plugin. That gives floating windows for free and suits the app's own native plugins later, which can use the same window host.

## Crates

| Crate | What |
|---|---|
| `daw-plugins` (built) | Both sides of the process boundary: `block` (the shared-memory audio channel and hand-off), `control` (messages), `client` (the DAW side: starting a host, calls, noticing it is gone), `host` (the host process side), `sys` (Windows calls). The `daw-plugin-host` binary runs a host for tests |
| `daw-vst3` (to come) | The VST3 layer inside the host process, on the `vst3` crate: module loading, the call sequence, host interfaces (`IHostApplication`, `IComponentHandler`, `IPlugFrame`, `IParameterChanges`, `IEventList`, `IBStream`), state |
| `daw-engine` (built: `plugins.rs`, `audio.rs`) | Plugin effects: `EffectKind::Plugin` with `Effect.plugin` (a `PluginSpec`); the control side keeps an instance per plugin effect and hands its `DawBlock` to the audio side (`AudioMsg::AttachPlugin`); channels come back through the `retired` queue and an instance is ended only once its channel is back. Still to come: delay compensation |

- **The app is its own host:** it starts host processes by running its own executable in plugin-host mode (as Chrome does), so host and app always match and there is no second program to ship or find. The scanner is the same mode with another argument.
- **A job object** ends every host process when the DAW exits, even if it crashes; a host also stops when its stdin closes.

## Audio across processes

- **Shared memory per plugin instance**, made at activation for the largest block: input and output buffers per bus and channel (f32), the block header (sequence number, frame count, process mode), the `ProcessContext` (tempo, time signature, song position in samples and quarter notes, playing and loop flags), a fixed-size parameter-change area (per parameter: up to N points of offset and value) and a fixed-size event area (note on and off with sample offsets). Everything is fixed-size: nothing is allocated per block on either side.
- **Hand-off per block:** the DAW side writes inputs, parameters and events, bumps the sequence number and signals "go" (Windows: an auto-reset event; macOS and Linux: a futex-like wait or semaphore in the shared memory; to verify per platform). The host's audio thread wakes, fills the VST3 structures from shared memory without allocating, calls `process`, and signals "done". The DAW side waits for "done" up to a **deadline** (a share of the block's time). On a miss, that plugin's output is silence for the block and the slot is marked late; a run of misses marks it hung (shown in the UI, Reload offered).
- **Fits the parallel renderer:** plugin nodes are tasks like any other node. A worker that reaches a plugin slot signals it and waits. Several plugins in one tree level run at the same time in their own processes. Later optimisation: signal every plugin slot of a level first, then wait.
- **Thread priority:** the host's audio thread joins MMCSS ("Pro Audio") on Windows and the realtime class on macOS and Linux, as the DAW's audio thread does.
- **Cost:** two cross-process wake-ups per plugin per block (a few microseconds each; to measure). No added latency: the plugin processes the same block within the DAW's callback. A test scene in the speed tests measures many plugin slots.
- **Offline render** drives the same slots with `kOffline` and no deadline, so exports always include every plugin.

## Control channel

A message channel per host process, over its stdin and stdout (length-prefixed JSON; cross-platform, no names), carrying everything that is not audio: load and unload, bus setup, activate, parameter info, state get and set, editor open and close, and callbacks from the plugin (`beginEdit`, `performEdit`, `endEdit`, `restartComponent`, `resizeView`). Requests carry ids so replies and callbacks interleave safely (yabridge's lesson: blocking one-call-at-a-time sockets deadlock on re-entrant calls such as editor resize). Messages are serialised with a small, versioned format.

## In the engine

- A plugin effect runs over whole chunks (the node's other effects still take parameters on the 32-frame grid), in place on the node's bus.
- **Live** (the processor's default) a plugin block has a deadline of 70% of the device block's time; **offline** (`AudioProcessor::set_offline(true)`, used by every render to a file) plugins take as long as they need. A plugin with no channel yet, or late, hung or crashed, is silent for that chunk; the rest of the song plays on.
- `Snapshot` devices carry `plugin: Option<PluginSnapshot>` (spec and state: Running, Hung after 64 missed blocks in a row, Crashed, Failed with why). `Command::ReloadPlugins` kills the host and starts every plugin again.
- Starting the host and instances happens on the control thread when an edit adds a plugin effect: the first one takes a moment (to move off the UI thread later).

## Process groups

Like Bitwig's hosting modes, a preference decides how instances share processes. The default follows Bitwig's ("Together"), the main DAW that isolates plugins (the user: default to what the big players do; Ableton Live and Logic Pro run most plugins inside the app):

| Mode | Processes | Trade-off |
|---|---|---|
| **Together** (default) | One for all plugins | Least memory; one crash silences all plugins until Reload (never the DAW) |
| By plugin | One per plugin module (all instances of the same `.vst3`) | Plugins whose instances talk to each other (some EQs share analysers across instances) keep working; a crash affects one product |
| Individually | One per instance | Safest, most memory and CPU |

## The VST3 call sequence

The host process follows Steinberg's call order and threading rules exactly: `docs/vst3-notes.md` has them, with the parameter, state and editor details.

## Parameters

- VST3 parameters are normalised `0..=1` doubles with stable 32-bit ids, exactly the space layers already work in (`parameters-and-layers.md`), so layers and signals drive plugins as they drive built-in devices.
- A plugin parameter is named by node, device and the plugin's **parameter id** (not its index), so a plugin update that adds or reorders parameters keeps every layer on the right one. A parameter that disappears keeps its layers, shown as missing.
- **Picking:** "+ Add Parameter" on a plugin lists its automatable parameters (`kCanAutomate`, not hidden or read-only), searchable.
- **Learn (wiggle the knob):** while armed, the first parameter the user touches in the plugin's window (`beginEdit` / `performEdit` from the plugin, forwarded over the control channel) is added. Mouse-wheel moves may come without `beginEdit`.
- **Knob moves edit the resting value** (as for built-in devices): a `beginEdit` … `endEdit` gesture becomes one undoable edit of the default state. Not recording; that stays a later, armed mode.
- **Automation delivery:** layers are evaluated on the 32-frame control grid; each block sends the changed parameters' values at those offsets in the shared parameter area, rebuilt into `IParamValueQueue`s (sample accurate to the grid). The plugin's window follows along through `setParamNormalized` on the controller at UI rate.
- **`restartComponent`:** titles or flags changed: rescan parameter info; values changed: re-read values; latency changed: re-plan delay compensation; I/O changed: redo the bus setup (outside the audio callback); reload: deactivate, reload state, reactivate.
- **Bypass:** a plugin's own bypass parameter (`kIsBypass`) is used when it has one, so its latency stays constant; otherwise the host stops calling it and passes sound through, delayed by its latency.

## State

- Saved inside the project file (the user's call): the component's and the controller's `getState`, two binary blobs per plugin instance (base64), so a save stays one file write. The plugin's class id and version are saved with them.
- On load: `setState` on the component, `setComponentState` and `setState` on the controller.
- `.vstpreset` import and export later (format is documented: header "VST3", class id, chunk list).

## Editor windows

- Opened from the plugin's slot (layout to be agreed). The host process makes a native top-level window and attaches the plugin's view to it; the plugin and the user can resize it (`vst3-notes.md`, Editor). Content scale follows the screen.
- **Staying above the DAW, without linking windows:** measured on Windows, a plugin window owned by the DAW's window (across processes) froze the DAW's UI for as long as the plugin hung (`vst3-notes.md`). So windows stay unlinked: the DAW tells host processes when it becomes or stops being the active app, and they keep their windows above it while it is.
- Window positions are remembered per plugin instance. Closing the window leaves the plugin running.
- The same window host later serves the app's own native plugins' editors.

## Scanning

*Built (2026-10-21): `daw-plugins/src/scan.rs` and `vst3.rs`; `daw_host::plugins::update_catalog`; `daw-player plugins` scans and lists. The catalog is `%APPDATA%\daw\plugins.json`. Tested against `daw-test-plugin` (a real VST3 built from this repo) including one that crashes or hangs while loading. The app scans in the background from startup (`plugin_settings.rs`, `AppView::start_scan`); **Preferences → Plugins** (agreed with the user) lists the folders (standard ones read-only; added ones, saved in the preferences, removable), Rescan with the count or "Scanning n of N…", and Couldn't load with the reason in plain words and Retry.*

**The browser** (agreed 2026-10-21, built with step 3): adding an instrument or an effect opens one modal browser, with Built-in and Plugins as the top-level split; searchable, plugins grouped by vendor; "Scanning n of N…" while a scan runs.

- The scanner is the plugin host binary in scan mode, one process per bundle with a timeout, so a plugin that crashes or hangs while being scanned never affects the DAW. Results (classes, vendor, version, instrument or effect, parameter count) are cached by path, size and modified time; only new or changed bundles are rescanned.
- Folders: the standard ones per platform (Windows: `%LOCALAPPDATA%\Programs\Common\VST3`, `C:\Program Files\Common Files\VST3`), searched recursively; extra folders can be added in Preferences.
- When: in the background at startup, and a Rescan button in Preferences. Plugins that failed to scan are listed with the reason and can be retried.
- The instrument browser and effect menu get a PLUGINS section (already greyed in the instrument browser).

## Delay compensation (needed with plugins)

Many plugins report latency (`getLatencySamples`). The engine has none yet, and the tree makes it matter: a parent sums children whose chains have different latencies. Plan: each node's latency is its chain's sum plus the most-delayed child; shorter children are delayed to match at each sum; the transport runs ahead by the song's total latency so audio still lines up with the playhead, and signals and automation are read at the compensated time. Its own design note when built.

## Testing

- **A test plugin built in this repo** (the user's call): a small VST3 written in Rust with the `vst3` crate (gain, a parameter, latency, state, an editor), so tests are self-contained and repeatable. Variants that crash, hang or allocate on purpose test the watchdog, deadlines and diagnostics.
- IPC layer: unit tests for the layout, the hand-off and timeouts. Engine: a plugin slot renders identically live and offline, at any block size, for the test plugin.
- Steinberg's validator on the test plugin; the user's real plugins tried by hand. First: **Serum 2** (instrument, large resizable editor), **OTT** (simple effect), a **Valhalla** reverb (tail). Later, heavier ones as stress tests (Native Instruments, Spectrasonics, iZotope, Arturia).
- Speed tests: many plugin slots playing.

## Build order (each step usable and tested before the next)

1. **IPC and host process with built-in test devices** (no VST3 yet), **built**: the hand-off, deadlines, crash and hang detection (`daw-plugins`, tested with real processes); the engine's plugin effect, live deadlines (70% of each block) and offline waiting, Reload, states in the snapshot (`daw-plugins/tests/engine.rs`); the app and player as their own host (`daw_host::plugins`). Process groups other than Together come with the Preferences page.
2. **Scanner** and Preferences → Plugins (built). The browser modal comes with step 3, so plugins can be added the moment they show.
3. **Effects:** load, buses, process, parameters (pick from list), state.
4. **Instruments:** note events, multiple outputs later.
5. **Editor windows:** floating, staying above the DAW, resizing, knob moves as resting values, learn.
6. **Delay compensation.**
7. `restartComponent` handling, bypass, presets, diagnostics.

## Still to decide

- Where plugin windows open from, and the Preferences → Plugins page: laid out with the user when those steps come.

## Sources

- Steinberg VST3 Developer Portal: workflow diagrams (audio processor, edit controller, bus arrangement, resize view), Parameters and Automation, Presets and Program Lists, Locations and Format, Module Architecture, Host Requirements, IProcessContextRequirements, IPlugViewContentScaleSupport (`https://steinbergmedia.github.io/vst3_dev_portal/pages/`)
- Steinberg press release, VST 3.8 under MIT, 2025-10-29
- yabridge architecture (`https://github.com/robbert-vdh/yabridge/blob/master/docs/architecture.md`)
- maolan-engine (`https://github.com/maolan/engine`), Bitwig user guide: plug-in hosting modes
- `vst3` crate (`https://docs.rs/crate/vst3`)
