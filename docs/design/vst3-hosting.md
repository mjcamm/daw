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
*Picking and automation built (2026-10-21): file version 18, control protocol 3; tests in `daw-engine` (`project.rs`, `audio.rs`), `daw-plugins/tests/engine.rs` and `vst3_effect.rs`, `ui_tests/plugin_params.rs`.*

- A plugin parameter is named by node, device and the plugin's **parameter id** (not its index): `ParamId { device: Effect(id), index: <VST3 id> }`, so a plugin update that adds or reorders parameters keeps every layer on the right one. Only the parameters given layers join the song: `Effect::plugin_params` (`PluginParam`: id, name, resting value, layers), added by `Edit::AddPluginParam` (undone by `RemovePluginParam`, refused while it has layers), at most `MAX_PLUGIN_PARAMS` (128) per plugin. The name is kept so it still shows if the plugin goes missing. Still to come: marking a parameter the plugin no longer has as missing.
- **The list:** the host answers `GetParams` from its main thread (`Vst3State::params`): `kCanAutomate`, not hidden or read-only, each with its unit (`IUnitInfo`) path as its group ("Osc A / Unison"), its unit label and the value the plugin holds. The engine asks once an instance starts and puts it in the snapshot (`PluginSnapshot::params`). On the user's machine: OTT 20 parameters, Serum 2 FX 541 in 31 groups, smartEQ3 164 in 27.
- **Picking** (agreed with the user): "+ Add Parameter" on a plugin opens a searchable dropdown (`daw-ui/src/param_picker.rs`): ungrouped parameters first, then each group under a heading in the plugin's order; arrows, Enter, Escape, a click outside closes; parameters with layers aren't offered. Choosing one goes on to its first layer's type as for built-in devices; the parameter joins the song with that layer in one `Edit::Batch` (one undo), resting at the plugin's current value. Values show as percentages until the plugin's own value text comes with editor windows.
- **Learn (wiggle the knob):** while armed, the first parameter the user touches in the plugin's window (`beginEdit` / `performEdit` from the plugin, forwarded over the control channel) is added. Mouse-wheel moves may come without `beginEdit`.
- **Knob moves edit the resting value** (as for built-in devices): a `beginEdit` … `endEdit` gesture becomes one undoable edit of the default state. Not recording; that stays a later, armed mode.
- **Automation delivery** (built): layers are evaluated on the 32-frame control grid; each chunk sends every song parameter's first value, then a point wherever it changes (`audio.rs`, `plugin_changes`), in the shared parameter area (512 changes a block; more are dropped), rebuilt into `IParamValueQueue`s (sample accurate to the grid). A parameter at rest costs one change per chunk, so the plugin always follows the song. The plugin's window follows along (Editor windows, below).
- **`restartComponent`:** titles or flags changed: rescan parameter info; values changed: re-read values; latency changed: re-plan delay compensation; I/O changed: redo the bus setup (outside the audio callback); reload: deactivate, reload state, reactivate.
- **Bypass:** a plugin's own bypass parameter (`kIsBypass`) is used when it has one, so its latency stays constant; otherwise the host stops calling it and passes sound through, delayed by its latency.

## State

*Built (2026-10-21): file version 17, control protocol 2; tests in `daw-plugins/tests/vst3_effect.rs`.*

- Saved inside the project file (the user's call): the component's and the controller's `getState`, two binary blobs per plugin instance (base64), so a save stays one file write. The plugin's class id and version are saved with them.
- On load: `setState` on the component, `setComponentState` and `setState` on the controller, before activation (the state goes with `Create`).
- Saving asks each running instance (`GetState`); the host process answers from its main thread while the instance keeps processing on its audio thread. A hung or crashed plugin, or one that could not load (missing on this machine), keeps the state it was loaded with, so a save never loses it.
- The engine remembers each plugin effect's last known state, also when the effect is deleted, so undo brings it back as it was.
- `.vstpreset` import and export later (format is documented: header "VST3", class id, chunk list).

## Instruments

*Built (2026-10-23): file version 19, block layout 2; tests `daw-plugins/tests/vst3_instrument.rs` (Test Synth alone and in a song), `ui_tests/browser.rs`; the user's Serum 2 plays a chord with no late blocks (`installed_plugins.rs`, opt-in).*

- An instrument can be a plugin: `InstrumentKind::Plugin` with `Instrument::plugin` (a `PluginSpec`) and `plugin_params`, exactly as for effects: chosen in the browser under "+ Instrument" → Plugins, layered with the node's other instruments, saved with its state and window place, its parameters picked from the list or learned, its window opened by double-clicking its slot. It plays every note (`plays` is all pitches). Device ids are project-wide, so `plugins.rs` keeps instruments and effects in one map; `Edit::AddPluginParam`, `Command::OpenPluginWindow` and `Command::LearnPluginParam` name a `device`.
- **Notes:** the shared block has a fixed note-event area (`NoteEvent`: frame offset, note id, pitch, on or off, velocity `0..=1`; 512 a block). On the audio side a plugin instrument queues each start and end at its frame within the chunk (`ChunkNotes`, 256 per instrument a chunk; previews come at the chunk's start), keeps up to 64 sounding notes so an end or "end everything" (stop, locate, loop jump) can name its pitch, and plays the whole chunk after the built-in instruments (`NodePlayer::play_plugins`), its parameters on the control grid as for effects. The host fills a preallocated `IEventList` (`EventList`) from the block. Output joins the node's bus; it is silent while the plugin has no channel, is late or has crashed.
- Instruments have no audio input; the block still has stereo inputs (unused), so effects and instruments share one layout.

## Editor windows

*Built (2026-10-23; agreed with the user): `daw-plugins/src/editor.rs`, control protocol 5; tests `daw-plugins/tests/vst3_editor.rs`, `vst3_effect.rs` (learn and turns), `ui_tests/plugin_params.rs`. All seven of the user's tried effects open (OTT 310 × 440 in 22 ms … Serum 2 FX 1190 × 740 in 1.5 s), `tests/installed_plugins.rs`.*

- **Opening** (the user's call): double-click a plugin's slot on a mixer strip (`Command::OpenPluginWindow`). Each plugin has its own window and any number can be open. Title "OTT — Bass" (the plugin, then its node). The DAW asks without waiting (`HostProcess::start_open_editor`; Serum takes 1.5 s) and the answer arrives with the host's events; `PluginSnapshot::window` says Open, Closed, or why it failed, and the slot's tooltip shows that ("Double-click to open OTT", "Couldn't open OTT's window: …"). Opening an open window brings it to the front; the DAW lets the host take the foreground (`AllowSetForegroundWindow`) as it asks.
- **The window** is the host process's own top-level window (a tool window: no taskbar button), first placed just below and right of the mouse and kept on that screen. **Where it is goes into the song** (`EffectFile::window`, by effect; reported by the host as `HostEvent::EditorMoved` when placed and whenever the user lets go after moving it), so it reopens there, also after a restart. Its title follows its node's name (`Request::SetEditorTitle` after any edit). Closing it leaves the plugin running; the DAW hears (`HostEvent::EditorClosed`).
- **Resizing:** the plugin asks with `IPlugFrame::resizeView` (the window resizes, then `onSize`). If `canResize`, the window has a sizing border; dragging an edge keeps to `checkSizeConstraint`, and any resize from outside tells the plugin `onSize`. The host process is per-monitor DPI aware: `setContentScaleFactor` before `attached` and on `WM_DPICHANGED` (taking Windows' suggested rectangle).
- **Staying above the DAW, without linking windows:** measured on Windows, a plugin window owned by the DAW's window (across processes) froze the DAW's UI for as long as the plugin hung (`vst3-notes.md`). So windows stay unlinked: the host learns the DAW's process id in `Hello` and listens for foreground changes (`SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`, out of context, skipping its own windows). When one of the DAW's own unowned windows comes forward (the user clicked the DAW), its plugin windows are lifted just above it without taking focus (`editor::stacking`). Nothing is topmost: the DAW's dialogs (file pickers, owned by its window) come up above plugin windows, and other apps cover them as usual.
- **The host's main thread** runs a Windows message loop: messages from the DAW are read on their own thread and wake the loop (`sys::pump_until`), so waiting for the DAW never stops plugin windows. Every 30 ms round it sends what the plugins reported and shows the song's latest values in their windows (`Vst3State::follow`: the audio thread records each value it sends in a lock-free `ParamMirror`; `setParamNormalized` on the controller, skipping knobs the user holds).
- **Knob moves** (`IComponentHandler`): `performEdit` goes to the processor through a lock-free queue (as the block's first points) and to the DAW (`HostEvent::EditBegin / EditValue / EditEnd`). In the engine (`Engine::window_edits`), a turn of a parameter in the song moves its resting value as it goes and is **one undo step** (`History::record`); moves without begin and end (a mouse wheel) count as one turn until they pause for 0.5 s. Moves of other parameters only change the plugin (kept in its state).
- **Learn** (the user's call): "Learn" beside a plugin's "+ Add Parameter" on Parameters opens its window and arms learning (`Command::LearnPluginParam`, `PluginSnapshot::learning`; it reads "Learning: turn a knob in OTT", click again to stop). When the first turned parameter that isn't shown yet finishes its turn, it joins the song resting where it was left, with a drawn "Layer 1" (one undo), and learning stops.
- The same window host later serves the app's own native plugins' editors.

## Scanning

*Built (2026-10-21): `daw-plugins/src/scan.rs` and `vst3.rs`; `daw_host::plugins::update_catalog`; `daw-player plugins` scans and lists. The catalog is `%APPDATA%\daw\plugins.json`. Tested against `daw-test-plugin` (a real VST3 built from this repo) including one that crashes or hangs while loading. The app scans in the background from startup (`plugin_settings.rs`, `AppView::start_scan`); **Preferences → Plugins** (agreed with the user) lists the folders (standard ones read-only; added ones, saved in the preferences, removable), Rescan with the count or "Scanning n of N…", and Couldn't load with the reason in plain words and Retry.*

**The browser** (agreed and built 2026-10-21; `daw-ui/src/browser.rs`, described in `mixer.md`): adding an instrument or an effect opens one modal browser, with Built-in and Plugins as the top-level split; searchable, plugins grouped by vendor; "Scanning n of N…" while a scan runs. Choosing a plugin effect adds `Effect::plugin(PluginSpec::Vst3 { .. })`.

- The scanner is the plugin host binary in scan mode, one process per bundle with a timeout, so a plugin that crashes or hangs while being scanned never affects the DAW. Results (classes, vendor, version, instrument or effect, parameter count) are cached by path, size and modified time; only new or changed bundles are rescanned.
- Folders: the standard ones per platform (Windows: `%LOCALAPPDATA%\Programs\Common\VST3`, `C:\Program Files\Common Files\VST3`), searched recursively; extra folders can be added in Preferences.
- When: in the background at startup, and a Rescan button in Preferences. Plugins that failed to scan are listed with the reason and can be retried.

## Delay compensation

*Built (2026-10-23): `delay-compensation.md` has the rule and how it runs.* Each plugin's latency is asked after activation (`Request::GetLatency`, control protocol 6); nodes line up at every sum, automation follows the sound, the playhead shows what is heard, and renders to a file start the latency in.

## Testing

- **A test plugin built in this repo** (the user's call): a small VST3 written in Rust with the `vst3` crate (gain, a parameter, latency, state, an editor), so tests are self-contained and repeatable. Variants that crash, hang or allocate on purpose test the watchdog, deadlines and diagnostics.
- IPC layer: unit tests for the layout, the hand-off and timeouts. Engine: a plugin slot renders identically live and offline, at any block size, for the test plugin.
- Steinberg's validator on the test plugin; the user's real plugins tried by hand. First: **Serum 2** (instrument, large resizable editor), **OTT** (simple effect), a **Valhalla** reverb (tail). Later, heavier ones as stress tests (Native Instruments, Spectrasonics, iZotope, Arturia).
- Speed tests: many plugin slots playing.

## Build order (each step usable and tested before the next)

1. **IPC and host process with built-in test devices** (no VST3 yet), **built**: the hand-off, deadlines, crash and hang detection (`daw-plugins`, tested with real processes); the engine's plugin effect, live deadlines (70% of each block) and offline waiting, Reload, states in the snapshot (`daw-plugins/tests/engine.rs`); the app and player as their own host (`daw_host::plugins`). Process groups other than Together come with the Preferences page.
2. **Scanner** and Preferences → Plugins (built). The browser modal comes with step 3, so plugins can be added the moment they show.
3. **Effects:** load, buses, process (**built**: `vst3_host.rs`, `PluginSpec::Vst3`; tested with the test plugin alone and in a song, and with seven installed commercial effects), state and the modal browser (**built**), parameters picked from a list and moved by layers (**built**).
4. **Instruments:** note events (**built**, 2026-10-23); multiple outputs later.
5. **Editor windows:** floating, staying above the DAW, resizing, knob moves as resting values, learn (**built**, done before step 4 at the user's call).
6. **Delay compensation** (**built**, `delay-compensation.md`).
7. `restartComponent` handling, bypass, presets, diagnostics.

## Still to decide

- Where plugin windows open from, and the Preferences → Plugins page: laid out with the user when those steps come.

## Sources

- Steinberg VST3 Developer Portal: workflow diagrams (audio processor, edit controller, bus arrangement, resize view), Parameters and Automation, Presets and Program Lists, Locations and Format, Module Architecture, Host Requirements, IProcessContextRequirements, IPlugViewContentScaleSupport (`https://steinbergmedia.github.io/vst3_dev_portal/pages/`)
- Steinberg press release, VST 3.8 under MIT, 2025-10-29
- yabridge architecture (`https://github.com/robbert-vdh/yabridge/blob/master/docs/architecture.md`)
- maolan-engine (`https://github.com/maolan/engine`), Bitwig user guide: plug-in hosting modes
- `vst3` crate (`https://docs.rs/crate/vst3`)
