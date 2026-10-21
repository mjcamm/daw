# VST3 Notes

Working knowledge of the VST3 API for hosting, kept because the official docs are spread out and some pages are missing. Decisions for this app are in `docs/design/vst3-hosting.md`. Add to this when something takes digging. *Unverified* marks what came from memory or secondary sources; check it against the SDK headers (the `vst3` crate's generated bindings) before relying on it.

**References:** Steinberg's developer portal (`https://steinbergmedia.github.io/vst3_dev_portal/pages/`, Technical Documentation → Workflow Diagrams is the most useful part; the IPlugView call sequence page is a 404). The SDK is MIT since 3.8 (2025-10): its host examples (audio host, editor host, validator) may be adapted, keeping Steinberg's notice. JUCE is AGPL and Ardour/Carla GPL: read, never copy.

## Bundles and locations

- A plugin is a folder: `Name.vst3/Contents/x86_64-win/Name.vst3` (the DLL; same name as the folder), `Contents/MacOS/Name`, `Contents/x86_64-linux/Name.so`. `Contents/Resources/moduleinfo.json` (3.7.5+) lists the classes without loading code. Single-file `.vst3` DLLs are deprecated since 3.6.10 but still around.
- Windows folders: `%LOCALAPPDATA%\Programs\Common\VST3` (user), `C:\Program Files\Common Files\VST3` (global), `$APPFOLDER\VST3`. macOS: `~/Library/Audio/Plug-ins/VST3`, `/Library/Audio/Plug-ins/VST3`. Linux: `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3` (and `lib64`). Scan recursively; for the same class id, the first (higher priority) location wins.

## Loading

- Windows: `InitDll` after `LoadLibrary` and `ExitDll` before `FreeLibrary`, both **optional**. macOS `bundleEntry` / `bundleExit` and Linux `ModuleEntry` / `ModuleExit` are **required**; refuse plugins without them.
- Then `GetPluginFactory`. Audio plugins are category `"Audio Module Class"`; `IPluginFactory2/3` add vendor, version and subcategories (`Fx`, `Instrument`, … *unverified spellings*). Linux hosts must call `IPluginFactory3::setHostContext` (the run loop below).

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

- `createView("editor")`, `isPlatformTypeSupported("HWND" | "NSView" | "X11EmbedWindowID")`, `setFrame(plugFrame)`, `attached(parent, type)`, size from `getSize`; `removed()` before closing, then release (*sequence unverified: the portal page is missing*).
- Plugin resizes: `IPlugFrame::resizeView(size)` → host resizes its window → `onSize`. User resizes (only if `canResize`): `checkSizeConstraint` → resize → `onSize`.
- `IPlugViewContentScaleSupport::setContentScaleFactor`: call just before or after `attached`, and again when the window moves to a screen with another scale.
- Linux: the host provides `Linux::IRunLoop` through the `IPlugFrame` and the host context (plugins use it for timers and file descriptors).

## Out of process (lessons from yabridge and others)

- Audio in shared memory sized at activation; control messages separately. yabridge pairs a shared audio buffer with a per-processor socket that both carries per-block metadata and synchronises.
- **Blocking one-call-at-a-time channels deadlock on re-entrant calls** (editor open, resize, `restartComponent` during a call). Use request ids and handle callbacks while waiting.
- Some products share state between instances in one process (analysers across EQ instances): such instances must share a host process.
- Editor and resize calls only on the plugin process's UI thread; pump its messages.
- A cross-process **owner or parent window** on Windows joins the two processes' input queues: a hung plugin can freeze the DAW's UI. *Unverified source; prototype before relying on it.*
- MMCSS: `AvSetMmThreadCharacteristicsW("Pro Audio")` for the host's audio thread; `AvRevertMmThreadCharacteristics` when done.
