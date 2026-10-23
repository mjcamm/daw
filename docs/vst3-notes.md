# VST3 Notes

Working knowledge of the VST3 API for hosting, kept because the official docs are spread out and some pages are missing. Decisions for this app are in `docs/design/vst3-hosting.md`. Add to this when something takes digging. *Unverified* marks what came from memory or secondary sources; check it against the SDK headers (the `vst3` crate's generated bindings) before relying on it.

**References:** Steinberg's developer portal (`https://steinbergmedia.github.io/vst3_dev_portal/pages/`, Technical Documentation → Workflow Diagrams is the most useful part; the IPlugView call sequence page is a 404). The SDK is MIT since 3.8 (2025-10): its host examples (audio host, editor host, validator) may be adapted, keeping Steinberg's notice. JUCE is AGPL and Ardour/Carla GPL: read, never copy.

## Bundles and locations

- A plugin is a folder: `Name.vst3/Contents/x86_64-win/Name.vst3` (the DLL; same name as the folder), `Contents/MacOS/Name`, `Contents/x86_64-linux/Name.so`. `Contents/Resources/moduleinfo.json` (3.7.5+) lists the classes without loading code. Single-file `.vst3` DLLs are deprecated since 3.6.10 but still around.
- Windows folders: `%LOCALAPPDATA%\Programs\Common\VST3` (user), `C:\Program Files\Common Files\VST3` (global), `$APPFOLDER\VST3`. macOS: `~/Library/Audio/Plug-ins/VST3`, `/Library/Audio/Plug-ins/VST3`. Linux: `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3` (and `lib64`). Scan recursively; for the same class id, the first (higher priority) location wins.

## Loading

- Windows: `InitDll` after `LoadLibrary` and `ExitDll` before `FreeLibrary`, both **optional**. macOS `bundleEntry` / `bundleExit` and Linux `ModuleEntry` / `ModuleExit` are **required**; refuse plugins without them.
- Then `GetPluginFactory`; the returned factory carries a reference for the caller (`ComPtr::from_raw`, no extra `addRef`). Audio plugins are category `"Audio Module Class"`; `IPluginFactory2/3` add vendor, version and `|`-separated subcategories (`Fx|Delay`, `Instrument|Synth`, `Instrument|Drum`; seen on real plugins). Linux hosts must call `IPluginFactory3::setHostContext` (the run loop below).
- Load with `LoadLibraryExW(.., LOAD_WITH_ALTERED_SEARCH_PATH)` so a plugin's own DLLs beside it are found, and `SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX)` first so a failed load never shows a dialog no one will click.
- **Class ids as text** (`moduleinfo.json`, `INLINE_UID`) are four 32-bit numbers in hex. On Windows the `TUID` bytes are in COM order (first number little-endian, the second's halves swapped), elsewhere big-endian; `vst3::uid(a, b, c, d)` builds the bytes and `vst3.rs::cid_string` is its inverse. Verified: Serum 2 loaded gives the same id its `moduleinfo.json` lists.
- `moduleinfo.json` belongs in `Contents/Resources/`; some older bundles have it elsewhere (Serum 1: `Contents/`) or not at all: those are loaded. It may be JSON5 (comments): if it does not parse as JSON, load instead.
- Measured 2026-10-21 on the user's machine: 143 bundles, 148 plugins, none failed, 21.7 s with 4 scanner processes at once (most without `moduleinfo.json`); an unchanged rescan takes no time.

## Call sequence (UI thread unless noted)

1. Component: `createInstance`, `initialize(hostContext)`, `getControllerClassID`.
2. Controller: create and `initialize` it if separate, else query it from the component. `setComponentHandler`. Connect component and controller with `IConnectionPoint::connect` (separate case only), then `setComponentState` with the component's state.
3. Buses: `setBusArrangements`; if it returns false, read `getBusArrangement` per bus and use that. `activateBus`.
4. `IProcessContextRequirements` once, before activation: which `ProcessContext` fields the plugin needs. A plugin without it may get nothing in `process`.
5. `setupProcessing`, `setActive(true)`, then `setProcessing(true)` and `process` (both **audio thread**, must not allocate or lock).
6. Stop: `setProcessing(false)`, `setActive(false)`, disconnect, `terminate` both, release.

- Any change of rate, block size or buses: `setProcessing(false)` → `setActive(false)` → change → `setActive(true)` → `setProcessing(true)`. Don't call `setActive` on a plain reset: some plugins allocate heavily there.
- **All edit-controller methods are UI thread.** Whether `getState`/`setState` may run while `process` runs is not stated (offline rendering may call state on the processing thread, per a portal FAQ quoted on the JUCE forum).

## Parameters

- Values are normalised `f64` in `0..=1`. Ids `0..=0x7FFF_FFFF` belong to the plugin, the rest to the host; `kNoParamId = 0xFFFF_FFFF`. Ids are stable across versions (only additions and removals).
- Flags: `kCanAutomate`, `kIsReadOnly` (not automatable), `kIsHidden` (read-only, not automatable), `kIsBypass` (at most one; instruments shouldn't), `kIsList`, `kIsWrapAround`, `kIsProgramChange` (don't automate by default).
- `stepCount`: 0 continuous, 1 on/off, n steps. Discrete: `normalized = value / stepCount`; `value = min(stepCount, floor(normalized * (stepCount + 1)))`.
- In `process`: each parameter id appears **once** in `IParameterChanges`; points in its `IParamValueQueue` are sorted by sample offset. Feed automation only through `process`, and mirror it to the GUI with `setParamNormalized` on the controller (UI thread). A plugin reports audio-thread changes through the output parameter changes, not `performEdit` (some plugins get this wrong).
- Edits from the plugin's GUI: `beginEdit` (mouse down) → `performEdit` (drag) → `endEdit` (mouse up), on the UI thread. **A `performEdit` may come without `beginEdit`** (mouse wheel). Buttons and menus call all three at once.
- `restartComponent` flags (*values unverified*): `kReloadComponent` 1, `kIoChanged` 2, `kParamValuesChanged` 4, `kLatencyChanged` 8, `kParamTitlesChanged` 16, `kMidiCCAssignmentChanged` 32, `kNoteExpressionChanged` 64, `kIoTitlesChanged` 128, `kPrefetchableSupportChanged` 256, `kRoutingInfoChanged` 512, `kKeyswitchChanged` 1024, `kParamIDMappingChanged` 2048. Plugins should call it on the UI thread. Under ~10 changed values a plugin should use `performEdit` instead of `kParamValuesChanged`.
- Bypass: with a `kIsBypass` parameter, use it (latency stays). Without one, the host stops calling `process`.

## State and presets

- Save: component `getState`, controller `getState` (two `IBStream`s). Restore: component `setState`, then controller `setComponentState` (the component's stream) and `setState` (its own). Rescan parameter values after.
- `.vstpreset`: `"VST3"`, int32 version, 32-byte ASCII class id, int64 offset of the chunk list; the list is `"List"`, int32 count, then per chunk a 4-byte id, int64 offset, int64 size. `Vst::PresetFile` in the SDK reads and writes it.

## Editor

- `createView("editor")`, `isPlatformTypeSupported("HWND" | "NSView" | "X11EmbedWindowID")`, `canResize`, `setFrame(plugFrame)`, `setContentScaleFactor`, `attached(parent, type)`, then size the window from `getSize`; to close, `removed()`, `setFrame(null)`, release, then destroy the window. Works with all seven of the user's tried effects (`editor.rs`).
- Some editors take a while to open (Serum 2 FX and Cassette 1.5 s): never wait for one on the DAW's UI thread.
- The controller never talks to the processor itself: `performEdit` from the window must reach the processor as an input parameter change, or the sound won't follow the knob. Plugins may call `performEdit` without `beginEdit` / `endEdit` (mouse wheel).
- Plugin resizes: `IPlugFrame::resizeView(size)` → host resizes its window → `onSize`. User resizes (only if `canResize`): `checkSizeConstraint` → resize → `onSize`.
- `IPlugViewContentScaleSupport::setContentScaleFactor`: call just before or after `attached`, and again when the window moves to a screen with another scale.
- Linux: the host provides `Linux::IRunLoop` through the `IPlugFrame` and the host context (plugins use it for timers and file descriptors).

## As built here

- `vst3_host.rs` implements what plugins call back into: `IHostApplication` (its `createInstance` makes `IMessage` and `IAttributeList` objects, which SDK-based plugins ask for when connecting processor and controller), `IComponentHandler`, a memory `IBStream`, and preallocated `IParameterChanges` / `IParamValueQueue` (128 parameters × 64 points a block).
- Buses: the main input and output are asked for stereo; other buses keep the plugin's arrangement and get silent scratch buffers (every bus gets buffers, active or not). A mono main output is copied to both sides.
- State: `Vst3State` (component and controller pointers, and the module) stays with the host's main thread for `getState` while the instance processes on its audio thread; `setState` happens at `Create`, before activation.
- Parameters: `Vst3State::params` reads `getParameterInfo` on the main thread. Offer only `kCanAutomate` without `kIsHidden` or `kIsReadOnly` (bypass and program-change parameters stay in). Groups come from `IUnitInfo` (queried on the controller; many plugins have none, so everything is ungrouped): each unit's `parentUnitId` chain up to `kRootUnitId` (0) names it. `stepCount` may be negative in bad plugins; clamp it.
- Threading: create, connect, buses, `setupProcessing`, `setActive` on the host process's main thread (with COM initialised); the instance then moves to its audio thread (`setProcessing` and `process`) and comes back to be deactivated.
- Measured 2026-10-21 (`tests/installed_plugins.rs`, opt-in): OTT, ValhallaRoom, ValhallaDelay, Serum 2 FX, Filtron, smartEQ3, Cassette all load (45–709 ms) and run 2 s of noise with no late block; slowest block 0.8–11 ms (smartEQ3's first).

## Out of process (lessons from yabridge and others)

- Audio in shared memory sized at activation; control messages separately. yabridge pairs a shared audio buffer with a per-processor socket that both carries per-block metadata and synchronises.
- **Blocking one-call-at-a-time channels deadlock on re-entrant calls** (editor open, resize, `restartComponent` during a call). Use request ids and handle callbacks while waiting.
- Some products share state between instances in one process (analysers across EQ instances): such instances must share a host process.
- Editor and resize calls only on the plugin process's UI thread; pump its messages.
- **Never link windows across processes** (owner via `GWLP_HWNDPARENT`, or parent/child): Windows joins the two processes' input queues, so a hung plugin freezes the DAW's UI. Measured 2026-10-21 (Windows 11): a test window owned by a window in another process, hanging for 8 s inside a click; a click on the owner then took ~7.5 s to arrive, against 15–53 ms with no link (three runs each).
- Keeping plugin windows above the DAW without a link (built): the host process watches foreground changes itself (`SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)`, delivered through its own message loop) and, when the DAW's process brings an unowned window forward, lifts its windows to `HWND_TOP` with `SWP_NOACTIVATE`. Topmost windows were tried first: they also covered the DAW's own dialogs (file pickers are owned windows of the same process).
- `WM_EXITSIZEMOVE` marks the end of a user's move or resize; `SetWindowPos` from code never sends it.
- MMCSS: `AvSetMmThreadCharacteristicsW("Pro Audio")` for the host's audio thread; `AvRevertMmThreadCharacteristics` when done. `audio_thread_priority` (already used by the engine) does this.
- **Wait timeouts round to the system tick** (up to ~15.6 ms): `WaitForSingleObject(event, 5)` may wait 15 ms. Audio deadlines wait on the event *and* a high-resolution waitable timer (`CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`) with `WaitForMultipleObjects`: measured ~5 ms for a 5 ms deadline.
- A host process's death shows at once as EOF on its stdout; no polling needed.
