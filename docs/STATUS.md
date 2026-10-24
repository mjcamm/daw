# Status and Next Steps

*Last updated: 2026-10-21, as a handover between sessions (everything committed, `3f9b1a7`). Read this first when picking up the project; then "In progress" below.*

## Where is done

- **foundations:** foundations, headless engine, parameters and layers.
- **core-loop UI:**, who is happy with it. Decisions: `docs/design/core-loop-ui.md`.
- **signals: built** (2026-10-15, `docs/design/motion-signals.md`):
- **Publish as signal and the contextual panel** (2026-10-16): the node row on Arrange publishes its notes as a signal; the **contextual panel** (right panel, folding into a tray, `docs/design/contextual-panel.md`) configures a selected notes signal (envelope with a live graph, optional pitch range and velocity clamp, each with a switch) and shows a selected node's mixer strip on Arrange. Tried by the user ("feels great").
- **node tree:** (`docs/design/node-tree.md`): nodes form a tree under the Song (the master); sound and notes flow up; mute and solo work across the tree; per-node meters; project-wide ids. Every node is MIDI or Audio and a branch never mixes kinds. **Arrange:** parent first (the Song at the top, each parent above its children), the nesting map with folding, a toolbar (add a MIDI or Audio node, group, duplicate, delete), drag to move or nest, animated rows. Parent lanes show **combined blocks** of the clips below (`docs/design/note-flow.md`). **Clips** move, copy, resize, loop and cut what they land on, between nodes too, with a live preview. **Mixer** (`docs/design/mixer.md`): aligned strips, parent strips left of their children, layered instruments, effect slots, pan, fader, meters, mute and solo, the nesting map along the bottom.
- **Piano roll and the tree editor** (`docs/design/piano-roll.md`, `note-flow.md`): the bottom panel follows the selected node; select, pen and scissors tools, velocity lane, quantize, note preview; a parent's view edits its whole branch in place. Tried and tuned by the user (2026-10-18: "all looks good").
- **Samples and the SingleSampler** (2026-10-18, `docs/design/samples.md`): one WAV sample, one-shot, played by its trigger note (auto-assigned from C1 among siblings, changeable in the sampler's panel on the strip).
- **Audio settings and engine safety** (2026-10-18, `docs/design/audio-settings.md`): driver, device, rate, buffer and threads applied live; test tone; CPU meter; denormals flushed, output guarded, nodes rendered in parallel with bit-identical results. ASIO waits on the user's go-ahead (it needs the ASIO SDK and LLVM installed).
- **Audio clips** (2026-10-19, `docs/design/audio-clips.md`): stretched live at playback by the user's own stretcher (`crates/daw-stretch`, `docs/design/stretcher.md`). Track → Import Audio…, dropping files on Arrange, waveforms, move/copy/trim/reveal, the scissors tool on Arrange (keys 1 and 3), and the audio panel (double-click an audio clip: BPM, Follow tempo, Gain, Fades).
- **UI speed** (2026-10-20, `docs/design/ui-speed.md`): cached panes that redraw only on change, a live layer for playheads and meters, no frames when idle, dense content painted and culled, the faster GPU requested on dual-GPU laptops. Playback frames 1.3–2.6 ms; full redraws 5.5–8.8 ms (release, full song). **Next, if wanted:** the last few ms are GPUI's per-element and per-glyph cost; less text on Arrange or fewer interactive elements per row and strip.
- **Looping clips** (2026-10-20, `docs/design/clip-loops.md`): MIDI and audio clips loop like blocks, with linked repeats; expanded at compile time, so the audio thread is unchanged.
- **Tempo** (2026-10-21): typed in a box beside the transport.
- **Export** (2026-10-23, `docs/design/export.md`): File → Export… (Ctrl+E): WAV (16 dithered / 24 / 32 float) or MP3 (192 / 256 / 320 kbps, LAME behind `daw-host`'s `mp3` feature, LGPL: settle before release), whole song (to the last clip's end) or the loop, tail until silent or off. Renders a copy of the song offline on its own thread and plugin host (`daw_host::export`), sample-for-sample the live sound, latency trimmed; the app stays usable; cancel leaves no file.
- **Metronome** (2026-10-24, `docs/design/metronome.md`): a click on every beat while playing, accented on beat 1, after the Song's channel and never in exports. Transport: metronome button and a volume button whose vertical slider opens on hover. Preferences → Metronome: sound (Beep, Wood, Tick, Cowbell), accent, pitch, volume; kept in the preferences for every song.
- **Drags are heard as they move** (2026-10-24, the user's request; `audio-engine.md`, previews): faders, pans, every slider, blocks and points on Parameters and Signals, notes and velocities, clip moves, notes-signal and audio-clip settings play as they're dragged (`Command::Preview`), still one undo step each. Sliders' thumbs follow the pointer (a drag the slider's length covers the range; Shift five times finer). Loop drags still apply on release.
- **Loop strip** (2026-10-21, `docs/design/loop-strip.md`): the top half of every ruler (Arrange, Parameters, Signals, piano roll, audio panel). Any drag draws a fresh loop, snapped to the page's grid; handles move either end; the loop button switches looping and keeps the region (file version 15 saves both).

The project file is at **version 19** (`crates/daw-engine/src/file.rs`); `upgrade.rs` converts older files.

## Phase 8, VST3 hosting: built (2026-10-23)

Decisions: `docs/design/vst3-hosting.md`. VST3 API facts and gotchas: `docs/vst3-notes.md` (read both before plugin work). Agreed with the user: plugins in their own processes from the start ("build it right first"), default grouping Together (as Bitwig), plugin state inside the project file, our own Rust test plugin, unlinked floating editor windows (a cross-process owner window was measured to freeze the DAW when the plugin hangs), parameters picked from a list or learned by wiggling a knob, the `vst3` crate for bindings.

**Built (all committed, tested):**

- **Step 1, plugins in a process** (`crates/daw-plugins`): `block.rs` (shared-memory audio, deadlines on a high-resolution timer, late/busy/dead), `control.rs` (JSON over the host's stdin/stdout), `client.rs` (`HostProcess`: start, calls, death seen as EOF, job object), `host.rs` (the host process: one audio thread per instance). Engine: `EffectKind::Plugin` + `Effect.plugin: Option<PluginSpec>` (file version 16), `daw-engine/src/plugins.rs` keeps instances in step with the song, live deadline 70% of a block, offline (`AudioProcessor::set_offline`) none, `PluginState` in snapshots, `Command::ReloadPlugins`. The app and player run themselves as hosts (`--plugin-host`, `daw_host::plugins`).
- **Step 2, scanning** (`scan.rs`, `vst3.rs`): bundles found recursively, `moduleinfo.json` or a scanner process (`--plugin-scan <bundle>`, 4 at once, 30 s timeout), cached in `%APPDATA%\daw\plugins.json`. User's machine: 148 plugins, none failed, 21.7 s first time. The app scans in the background from startup; **Preferences → Plugins** (`daw-ui/src/plugin_settings.rs`): folders, Add Folder, Rescan, progress, Couldn't load with Retry.
- **Step 3, first part, VST3 effects run** (`vst3_host.rs`, `PluginSpec::Vst3 { bundle, cid, name, vendor }`): Steinberg's call order, host interfaces, buses, process from shared memory without allocating. `daw-test-plugin` is a real VST3 ("Test Gain": processor + controller, parameter 0, state). Seven of the user's commercial effects pass `tests/installed_plugins.rs` (opt-in).
- **The device browser** (`daw-ui/src/browser.rs`, `mixer.md`): "+ Instrument" and "+ Effect" open one modal (Built-in | Plugins, search, vendor headings, arrows + Enter, Escape or click outside closes, opens on the side used last, scan progress). Plugin effects and instruments add as `PluginSpec::Vst3`. UI tests `ui_tests/browser.rs`, speed scene `perf_browser`. Test sessions run no plugin host.
- **Plugin state in the song** (file version 17): each plugin effect's `state` (`PluginData`: component and controller bytes, base64) sits beside its effect in the file. Control protocol 2: `Create` carries state, `GetState` answers `Reply::State`. The host reads state through a `Vst3State` handle on its main thread while the instance processes. The engine (`plugins.rs`, `saved`) keeps each effect's last known state: from the file, asked of running plugins by `Engine::to_file` (now `&mut self`; hung ones keep theirs), and taken when an effect goes so undo brings it back as it was; plugins that can't run keep theirs in the file. Loading a song detaches the old song's plugins (`AudioMsg::DetachPlugin`) and starts the new ones from their state.
- **Plugin parameters** (file version 18, control protocol 3; `vst3-hosting.md`, Parameters): a plugin parameter is `ParamId { device: Effect(id), index: <VST3 id> }`; those with layers live in `Effect::plugin_params` (`Edit::AddPluginParam` / `RemovePluginParam`, at most 128). The host lists automatable parameters with their unit groups (`GetParams`, `Vst3State::params`); the engine asks once a plugin starts and puts the list in `PluginSnapshot::params`. Each chunk sends a parameter's first value, then its changes on the 32-frame grid (`audio.rs`, `plugin_changes`). "+ Add Parameter" on a plugin opens a searchable, grouped dropdown (`daw-ui/src/param_picker.rs`; layout agreed with the user); the parameter joins the song with its first layer in one undo, resting at the plugin's value. Values show as percentages for now. UI tests `ui_tests/plugin_params.rs` (the engine's `show_plugin_params` stands in for a host there), speed scene `perf_param_picker` (0.35 / 1 ms). The test plugin gained "Tilt" (group "Tone") and a read-only "Meter".
- **Restarts and bypass** (step 7, 2026-10-23; control protocol 7): a plugin's `restartComponent` reactivates it for a new latency or buses (the song re-lined) and re-reads its parameter list for new titles or values. Switched off, a plugin keeps its latency: run with its own bypass parameter if it has one, else stood in for by a delay. Compensation now counts plugins switched on or off. Delay lines are sized per line (`Delays::rooms`). The test plugin has a bypass parameter and a window key that announces a latency of 64 and a renamed parameter.
- **Delay compensation** (step 6, 2026-10-23; `docs/design/delay-compensation.md`, control protocol 6): plugins report latency after activation; `compensation.rs` works out each node's delays (its built-in sources and plugin instruments line up at its sum point with its children; switched-on plugin effects add theirs); delay lines per node come from the control side (`AudioMsg::SetDelays`, freed through the `Leftover` queue); each node's chunk now runs sources, then children, then effects, channel and output delay (`finish_chunk`). Automation is read at the time of the sound passing it; the playhead lags by the song's latency while playing; `daw-player render` trims it from the start. Test device `TestPlugin::Delay`.
- **Plugin instruments** (step 4, 2026-10-23; file version 19; `vst3-hosting.md`, Instruments): "+ Instrument" → Plugins adds one (layered like built-in instruments); notes reach it on their exact frames through a note-event area in the shared block and a host `IEventList`; parameters, Learn, windows, state and window places work as for effects. "Test Synth" in the test plugin is a real instrument (a cosine per note, sample-exact). The user's Serum 2 plays (opt-in test).
- **Plugin windows** (step 5, done before step 4 at the user's call, 2026-10-23; control protocol 5; `vst3-hosting.md`, Editor windows): double-click a plugin's slot on a strip; any number open; title "OTT — Bass" (follows renames); resizing either way with the plugin's limits and screen scale; lifted above the DAW's main window when it comes forward, without linking windows or going topmost, so the DAW's dialogs stay above (a foreground hook in the host); where each window was is saved with its effect in the song (`window`, no file version change: older apps ignore it). The host's main thread now runs a message loop (`daw-plugins/src/editor.rs`, `host.rs`). Knob turns move a song parameter's resting value (one undo per turn); the window follows the song's values. **Learn** beside a plugin's "+ Add Parameter" opens the window and adds the next knob turned, with a drawn layer. Opening never waits (Serum takes 1.5 s). The test plugin has a window tests drive with posted clicks. All seven of the user's tried effects open (`installed_plugins.rs`).

**Next, in order:**

Nothing: Phase 8's "done when" is met. **Presets are skipped for now** (the user's call, 2026-10-23: each plugin's own preset browser works and is saved with the song; a DAW preset list was proposed as a "Preset" button beside Learn). Multiple plugin outputs and multitimbral plugins later (the user: playing several instruments on one node is enough). Not yet heard by the user in the app: see "Waiting on the user".

**Gotchas from this session:**

- Never set `RUSTFLAGS` and never add target folders: each is a separate full GPUI build (the scripts no longer set it). Build and test in the normal `target/`; use `target/claude` only to launch Claude's own copy of the app while the user's is open.
- `daw-plugins`' tests build `daw-test-plugin` with a nested cargo into `target/test-plugin` (`tests/common`). Linking `daw_testkit` installs the allocation guard: a test that uses it must not declare its own `#[global_allocator]`.
- Editing files: `cargo fmt` rewraps lines, so exact-text edits must match the formatted text. Windows PowerShell 5.1's `Out-File -Encoding utf8` writes a byte-order mark, which serde_json rejects (it silently resets the user's preferences); write JSON without one.

## In the app (`cargo run -p daw-ui`)

- **Menu bar:** File → Preferences… (Ctrl+,: Audio, Appearance and Plugins) and Quit (Ctrl+Q, saves first); Track → Import Audio….
- **Launch page:** new project (`Documents/DAW Projects/<name>/project.json`), Open…, recent projects. New songs start with one Synth node and one clip of demo notes.
- **Menu bar:** File → Export… (Ctrl+E) as well.
- **Header:** song name menu (Save, Rename, Back to projects) with "4/4 · Saved" under it, page tabs (Arrange, Parameters, Signals, Mixer), the pointer / scissors tool and snap control, transport (back, play/stop, loop, position) and **tempo** (click to type, Enter applies, Escape cancels; 20 to 999 BPM, two decimals), CPU meter.
- **Arrange:** the node list (colour, name, meter, signal button, M/S) with the nesting map and toolbar, and clips on the timeline. Click the ruler to locate; click a node to select it (this enables Parameters and points the bottom panel at it). Double-click a clip, or an empty lane, to open the bottom panel: the piano roll for MIDI, the audio panel for audio.
- **Parameters:** per device, "+ Add Parameter" makes a result row with its layers. Layers: drag to reorder, rename, blend mode (Add or Multiply, both centred), mix, show/hide, delete. Block editing on the lanes, settings bar, resting-value slider.
- **Signals:** "+ Add Signal" makes a drawn signal; draw on its lane like a layer. Colour bar picks a colour, double-click renames, × deletes, the uses chip lists listening layers with go-to icons. Click a row to select it; a notes signal's settings are in the inspector.
- **Mixer:** strips with instruments and effect slots ("+ Instrument" and "+ Effect" open the device browser), pan, fader, meters, M/S; the nesting map along the bottom.
- **Inspector:** the tray icon at the far right lights up when the selection has settings; click to open or close, drag the panel's left edge to resize.
- **Rulers:** the top half is the loop strip (drag to draw a loop, handles move its ends); click below it to locate.
- **Timeline pages:** scrollbars with zoom sliders at their ends; wheel scrolls vertically, Shift+wheel sideways.
- **Shortcuts:** Space play/stop, Ctrl+S save, Ctrl+Z / Ctrl+Shift+Z undo/redo, Delete removes what is selected (notes, then clips, then the node; a block on Parameters and Signals), 1 / 3 pointer / scissors (1 / 2 / 3 select, pen, scissors over the piano roll), Q quantize, Alt flips snap while dragging. Autosave every 30 s (`project.autosave.json`, removed on save).

## Model notes worth knowing

- **Layers are centred** (file version 3): the middle of a lane is no effect, top +1, bottom −1. Only Add and Multiply remain; older files convert exactly (Replace only when nothing below it acts). **Layer mix** scales every block's weight on a layer.
- **Signals** (version 4): drawn, or ADSR hits from MIDI notes. A layer listens with `Layer.signal`; its blocks are windows onto the signal. No range: block depth and invert do it. **Velocity clamp** (version 5) and on/off switches for it and the pitch range (version 6).
- **Loop on/off** (version 15): the region is kept while looping is off (`Snapshot.loop_enabled`, `Command::SetLoopEnabled`).
- **Tree** (version 7): older files nest their node under the Song, bit-for-bit the same sound. **Layered instruments** (version 8): `Node.instruments`, each playing the notes it takes. **Samples** (version 9).
- **Node kinds** (the user's call): every node is MIDI or Audio (`NodeKind`). There is no folder type: a parent is any node with children. **A branch never mixes kinds** (version 11): a node goes only under a parent of its own kind; the Song takes either and holds no clips. Only MIDI nodes have instruments.
- **Notes flow up** (`note-flow.md`): instruments play only the notes they take (a SingleSampler its trigger note); a node's notes pass up to its parents' instruments, never down.
- **Clips on a node never overlap** (version 13 merges old stacked MIDI clips). **Audio clips** (version 12) and **looping clips** (version 14).
- **Parent first** (the user's call, reversing the earlier "parents below"): on Arrange the Song is at the top and each parent sits above its children; on the mixer each parent strip sits left of its children. Map bars start at the parent's row (or strip) with the chevron; the Song has no bar.

## Waiting on the user (try it)

- **Metronome** (2026-10-24): switch it on beside the transport and play; hover the volume button and drag its slider; Preferences → Metronome: pick each sound (it plays), switch the accent, move the pitch. Listen that the click sits on the beat with plugins that add latency, and loops. Claude checked timing and levels in tests, not by ear.
- **Looped clips and signals** (2026-10-24): loop a clip that a notes signal reads: the Signals page lane and listening layers should show a hit at every repeat (the sound already followed the repeats; only the drawing missed them). Hover a looped clip's loop zone on Arrange: the loop cursor shows above the clip.
- **Live drags** (2026-10-24): during playback, drag a fader and a pan (the thumb should stay under the pointer), a layer's mix, a block's points, a notes signal's envelope, notes and velocities in the piano roll, and clips on Arrange: each should be heard as it moves. Ctrl+Z after each should undo the whole drag. Claude checked faders, pans and clips in tests, not by ear.

- **Export** (2026-10-23): Ctrl+E, export a song with plugins as WAV and as MP3 and listen to both; try Loop, Tail Off, Cancel mid-way, and playing or editing while it renders. Claude checked the files' contents in tests, not by ear.
- **Device browser and plugin effects** (2026-10-21): "+ Effect" on a strip, Plugins, search, add one of your effects (OTT, ValhallaRoom…), play, save, reopen the song: the plugin should come back with its settings (no editor windows yet; the state round trip is tested with the test plugin).
- **Plugin instruments** (2026-10-23): "+ Instrument" on a strip → Plugins → Serum 2; play a clip; double-click its slot for its window; Learn a knob. Claude heard nothing (no ears): Serum's chord was checked for sound and timing in a test.
- **Plugin windows and Learn** (2026-10-23): double-click OTT's slot on its strip; resize Serum 2 FX's window; open two at once; click another app and back (it covers them; clicking the DAW brings them back above it); move a window, save, restart, reopen it (same place); rename the node (title follows); Track → Import Audio… with a plugin window open (the file picker stays above it); turn a knob of a parameter on Parameters and undo; Learn, then turn a knob. Claude saw each of the seven effects' windows open and close (briefly, from a test), not in the app.
- **Plugin parameters** (2026-10-21): on Parameters, select the node, "+ Add Parameter" under the plugin, search (e.g. OTT's "Depth"), Enter, Manual drawn, draw a block, play: the plugin should follow it. Not yet seen running in the app by Claude with a real plugin (tested with the test plugin in a song, and the real plugins' lists).

- **Preferences → Plugins** (2026-10-21): the folder list, Rescan, and that 148 plugins show. `cargo run -p daw-player -- plugins` lists them by vendor.
- **Tempo** (2026-10-21): the field beside the transport, and changing it while audio clips play.
- **Loop strip** (2026-10-21): drawing loops on each ruler, the handles, snapping, the loop button. The bottom panels' rulers grew from 18/22 px to 28 px.
- **UI speed:** does it feel instant?
- **Looping clips:** looping MIDI and audio clips, and hearing the seams (looped audio not yet seen running by Claude either).
- **Audio clips:** importing, dropping files from Explorer (Claude could not test that), clip BPM and stretching by ear. The song tempo is now typed beside the transport (2026-10-21), so stretching can be heard by changing either the song's or a clip's BPM.
- **Signals page and listening layers:** driven by Claude, not yet tried by the user.

## Next up

1. **Next phase, to choose with the user:** Phase 6 (audio signals: sends, sidechains, following an audio signal's loudness; next in the plan), Phase 9's stem export (mixdown export is built), or Phase 7 (built-in devices).
2. **Phase 5, rest:** a richer demo song (drums with a separate kick, a music parent). An effect's or instrument's settings in the inspector are on hold (the user's call).
3. **Backfill UI tests** (`docs/behaviours.md`, "Building out the suite"): three so far (Arrange double-click, looping a clip, menus). Arrange and the piano roll first, as the most used; a few at a time alongside other work.
4. Phase 4's "done when" walkthrough (kick ducking several parameters in the UI). Showing a node's pitches on notes lanes is deferred; the range picker is enough for now.

## Design rules the user has set

- **One way to do anything.** No duplicate routes to the same result.
- **Visible, not hidden.** Clear icons and buttons (e.g. go-to icons), not secret clicks or right-click-only menus.

## How the user likes to work

- **Ask about layout before building each UI element.** They answer in plain numbered text (the multiple-choice tool was rejected once). Offer a recommendation.
- Calm, dense, native pro-app look (`docs/design/look-and-feel.md`), not "bootstrappy". Lucide icons (pinned `lucide-static@0.460.0`).
- Run `./scripts/check.sh`, then commit and push when work is done. Run `./scripts/full-test.sh` (adds the headless UI tests) when a feature is finished or before a milestone, not every commit.
- **Don't drop behaviours** (2026-10-20, after the Arrange double-click was lost in a rewrite): `docs/behaviours.md` lists them with their UI tests; ask before removing or changing one. No GitHub Actions on push.
- **Keep this file and `docs/design/` current** as work lands: decisions and pointers, not narration.
- Say when their eyes or ears are needed ("Try it", "Listen now").
- On Windows, a running app blocks rebuilding `daw.exe` ("failed to remove file"). `check.sh` still works; ask them to close the app to run it.

## Before a release (checklist)

- **Licence notices** (`THIRD-PARTY-LICENSES.md`): run `./scripts/licenses.sh` so `licenses/NOTICES.md` matches the shipped dependencies, then ship `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD-PARTY-LICENSES.md` and the `licenses/` folder with the app (in the installer or beside the executable).
- **MP3 / LAME (LGPL):** decide whether to keep LAME compiled in, link it as a DLL, or build WAV-only (`docs/design/export.md`, Licence).

## Known gaps

- **TODO (user, 2026-10-19): audio file types beyond WAV** (MP3, FLAC, AIFF; `symphonia` is the likely crate) for samples and audio clips. Reading files in the background, too.
- No node colour picker yet (the engine has `SetNodeColor`).
- Plugin parameters show as percentages (the plugin's own value text comes with editor windows); one the plugin no longer has isn't marked missing yet; duplicating a node with a plugin starts the copy without the plugin's state.
- Plugin hosting: starting the host process and creating instances happens on the control (UI) thread, so the first plugin in a song pauses the UI briefly; Windows only (macOS and Linux report "unsupported"); no delay compensation yet; plugins always run in realtime mode (offline exports should set up `kOffline`).
- One built-in effect (Filter) and two instruments (Synth, SingleSampler). Effects are added on the mixer; their settings show only on Parameters for now.
- Recording, MIDI keyboard input, MIDI file import and more instruments are Phase 9.
- The inspector shows nothing on Parameters, for drawn signals, or for a selected effect or instrument.
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Reference WAVs (`crates/*/tests/references/`) total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
