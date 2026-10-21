# Status and Next Steps

*Last updated: 2026-10-21, as a handover between sessions (everything committed, `3f9b1a7`). Read this first when picking up the project; then "In progress" below.*

## Where we are

- **Phases 0–2 done:** foundations, headless engine, parameters and layers.
- **Phase 3 (core-loop UI): built and tried by the user**, who is happy with it. Decisions: `docs/design/core-loop-ui.md`.
- **Phase 4 (motion signals): built** (2026-10-15, `docs/design/motion-signals.md`): layers (not blocks) listen to signals, signals are centred, notes are ADSR hits, removing a signal sends its listeners back to drawing. **Signals page** (drawn signals, colour, rename, delete, uses list with go-to) and **listening layers on Parameters** (type picked when adding, then fixed; chip with hover explanation and go-to; windows show the signal). Driven and checked by Claude via `scripts/ui-drive.ps1`. The "done when" walkthrough (a kick ducking several parameters, all in the UI) is still to run.
- **Publish as signal and the inspector** (2026-10-16): the node row on Arrange publishes its notes as a signal; the **inspector** (right panel, folding into a tray, `docs/design/inspector.md`) configures a selected notes signal (envelope with a live graph, optional pitch range and velocity clamp, each with a switch) and shows a selected node's mixer strip on Arrange. Tried by the user ("feels great").
- **Phase 5 (node tree): built except the richer demo song** (`docs/design/node-tree.md`): nodes form a tree under the Song (the master); sound and notes flow up; mute and solo work across the tree; per-node meters; project-wide ids. Every node is MIDI or Audio and a branch never mixes kinds. **Arrange:** parent first (the Song at the top, each parent above its children), the nesting map with folding, a toolbar (add a MIDI or Audio node, group, duplicate, delete), drag to move or nest, animated rows. Parent lanes show **combined blocks** of the clips below (`docs/design/note-flow.md`). **Clips** move, copy, resize, loop and cut what they land on, between nodes too, with a live preview. **Mixer** (`docs/design/mixer.md`): aligned strips, parent strips left of their children, layered instruments, effect slots, pan, fader, meters, mute and solo, the nesting map along the bottom.
- **Piano roll and the tree editor** (`docs/design/piano-roll.md`, `note-flow.md`): the bottom panel follows the selected node; select, pen and scissors tools, velocity lane, quantize, note preview; a parent's view edits its whole branch in place. Tried and tuned by the user (2026-10-18: "all looks good").
- **Samples and the SingleSampler** (2026-10-18, `docs/design/samples.md`): one WAV sample, one-shot, played by its trigger note (auto-assigned from C1 among siblings, changeable in the sampler's panel on the strip).
- **Audio settings and engine safety** (2026-10-18, `docs/design/audio-settings.md`): driver, device, rate, buffer and threads applied live; test tone; CPU meter; denormals flushed, output guarded, nodes rendered in parallel with bit-identical results. ASIO waits on the user's go-ahead (it needs the ASIO SDK and LLVM installed).
- **Audio clips** (2026-10-19, `docs/design/audio-clips.md`): stretched live at playback by the user's own stretcher (`crates/daw-stretch`, `docs/design/stretcher.md`). Track → Import Audio…, dropping files on Arrange, waveforms, move/copy/trim/reveal, the scissors tool on Arrange (keys 1 and 3), and the audio panel (double-click an audio clip: BPM, Follow tempo, Gain, Fades).
- **UI speed** (2026-10-20, `docs/design/ui-speed.md`): cached panes that redraw only on change, a live layer for playheads and meters, no frames when idle, dense content painted and culled, the faster GPU requested on dual-GPU laptops. Playback frames 1.3–2.6 ms; full redraws 5.5–8.8 ms (release, full song). **Next, if wanted:** the last few ms are GPUI's per-element and per-glyph cost; less text on Arrange or fewer interactive elements per row and strip.
- **Looping clips** (2026-10-20, `docs/design/clip-loops.md`): MIDI and audio clips loop like blocks, with linked repeats; expanded at compile time, so the audio thread is unchanged.
- **Tempo** (2026-10-21): typed in a box beside the transport.
- **Loop strip** (2026-10-21, `docs/design/loop-strip.md`): the top half of every ruler (Arrange, Parameters, Signals, piano roll, audio panel). Any drag draws a fresh loop, snapped to the page's grid; handles move either end; the loop button switches looping and keeps the region (file version 15 saves both).

The project file is at **version 17** (`crates/daw-engine/src/file.rs`); `upgrade.rs` converts older files.

## In progress: Phase 8, VST3 hosting

Decisions: `docs/design/vst3-hosting.md`. VST3 API facts and gotchas: `docs/vst3-notes.md` (read both before plugin work). Agreed with the user: plugins in their own processes from the start ("build it right first"), default grouping Together (as Bitwig), plugin state inside the project file, our own Rust test plugin, unlinked floating editor windows (a cross-process owner window was measured to freeze the DAW when the plugin hangs), parameters picked from a list or learned by wiggling a knob, the `vst3` crate for bindings.

**Built (all committed, tested):**

- **Step 1, plugins in a process** (`crates/daw-plugins`): `block.rs` (shared-memory audio, deadlines on a high-resolution timer, late/busy/dead), `control.rs` (JSON over the host's stdin/stdout), `client.rs` (`HostProcess`: start, calls, death seen as EOF, job object), `host.rs` (the host process: one audio thread per instance). Engine: `EffectKind::Plugin` + `Effect.plugin: Option<PluginSpec>` (file version 16), `daw-engine/src/plugins.rs` keeps instances in step with the song, live deadline 70% of a block, offline (`AudioProcessor::set_offline`) none, `PluginState` in snapshots, `Command::ReloadPlugins`. The app and player run themselves as hosts (`--plugin-host`, `daw_host::plugins`).
- **Step 2, scanning** (`scan.rs`, `vst3.rs`): bundles found recursively, `moduleinfo.json` or a scanner process (`--plugin-scan <bundle>`, 4 at once, 30 s timeout), cached in `%APPDATA%\daw\plugins.json`. User's machine: 148 plugins, none failed, 21.7 s first time. The app scans in the background from startup; **Preferences → Plugins** (`daw-ui/src/plugin_settings.rs`): folders, Add Folder, Rescan, progress, Couldn't load with Retry.
- **Step 3, first part, VST3 effects run** (`vst3_host.rs`, `PluginSpec::Vst3 { bundle, cid, name, vendor }`): Steinberg's call order, host interfaces, buses, process from shared memory without allocating. `daw-test-plugin` is a real VST3 ("Test Gain": processor + controller, parameter 0, state). Seven of the user's commercial effects pass `tests/installed_plugins.rs` (opt-in).
- **Plugin state in the song** (file version 17): each plugin effect's `state` (`PluginData`: component and controller bytes, base64) sits beside its effect in the file. Control protocol 2: `Create` carries state, `GetState` answers `Reply::State`. The host reads state through a `Vst3State` handle on its main thread while the instance processes. The engine (`plugins.rs`, `saved`) keeps each effect's last known state: from the file, asked of running plugins by `Engine::to_file` (now `&mut self`; hung ones keep theirs), and taken when an effect goes so undo brings it back as it was; plugins that can't run keep theirs in the file. Loading a song detaches the old song's plugins (`AudioMsg::DetachPlugin`) and starts the new ones from their state.

**Next, in order:**

1. **The modal browser** (layout agreed 2026-10-21, sketch in the conversation): "+ Instrument" and the effect slots' "+" open one centred modal (about 560 × 520, dimmed backdrop, click outside or Esc closes) titled Add Instrument / Add Effect; a segmented **Built-in | Plugins** split top left with a search field beside it (search covers the shown side; an empty result says if the other side matches); plugins grouped under vendor headings, each row its name with its type on the right; click (or arrows + Enter) adds and closes; it opens on the side used last; "Scanning n of N…" while a scan runs. Replaces today's 260 px instrument popover and the effect "+" menu (both listed in `docs/behaviours.md`: their lines change with this, as agreed). Needs the catalog handed from `AppView` to `ProjectView`. Instruments from plugins can be listed but only added once step 4 lands (or list effects only until then: ask).
2. **Parameters from a plugin** ("+ Add Parameter" lists a plugin's automatable parameters; the engine's `ParamId` currently indexes built-in device params; plugin params need their VST3 id).
3. **Step 4, instruments** (note events into `IEventList`; the shared block has no event area yet).
4. **Step 5, editor windows** (floating, owned by the host process, kept above the DAW without linking windows; knob moves as resting values; learn), **6, delay compensation**, **7, restart flags, bypass, presets.**

**Gotchas from this session:**

- Never set `RUSTFLAGS` and never add target folders: each is a separate full GPUI build (the scripts no longer set it). Build and test in the normal `target/`; use `target/claude` only to launch Claude's own copy of the app while the user's is open.
- `daw-plugins`' tests build `daw-test-plugin` with a nested cargo into `target/test-plugin` (`tests/common`). Linking `daw_testkit` installs the allocation guard: a test that uses it must not declare its own `#[global_allocator]`.
- Editing files: `cargo fmt` rewraps lines, so exact-text edits must match the formatted text. Windows PowerShell 5.1's `Out-File -Encoding utf8` writes a byte-order mark, which serde_json rejects (it silently resets the user's preferences); write JSON without one.

## In the app (`cargo run -p daw-ui`)

- **Menu bar:** File → Preferences… (Ctrl+,: Audio, Appearance and Plugins) and Quit (Ctrl+Q, saves first); Track → Import Audio….
- **Launch page:** new project (`Documents/DAW Projects/<name>/project.json`), Open…, recent projects. New songs start with one Synth node and one clip of demo notes.
- **Header:** song name menu (Save, Rename, Back to projects) with "4/4 · Saved" under it, page tabs (Arrange, Parameters, Signals, Mixer), the pointer / scissors tool and snap control, transport (back, play/stop, loop, position) and **tempo** (click to type, Enter applies, Escape cancels; 20 to 999 BPM, two decimals), CPU meter.
- **Arrange:** the node list (colour, name, meter, signal button, M/S) with the nesting map and toolbar, and clips on the timeline. Click the ruler to locate; click a node to select it (this enables Parameters and points the bottom panel at it). Double-click a clip, or an empty lane, to open the bottom panel: the piano roll for MIDI, the audio panel for audio.
- **Parameters:** per device, "+ Add Parameter" makes a result row with its layers. Layers: drag to reorder, rename, blend mode (Add or Multiply, both centred), mix, show/hide, delete. Block editing on the lanes, settings bar, resting-value slider.
- **Signals:** "+ Add Signal" makes a drawn signal; draw on its lane like a layer. Colour bar picks a colour, double-click renames, × deletes, the uses chip lists listening layers with go-to icons. Click a row to select it; a notes signal's settings are in the inspector.
- **Mixer:** strips with instruments (+ Instrument opens the browser), effect slots, pan, fader, meters, M/S; the nesting map along the bottom.
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

- **Preferences → Plugins** (2026-10-21): the folder list, Rescan, and that 148 plugins show. `cargo run -p daw-player -- plugins` lists them by vendor.
- **Tempo** (2026-10-21): the field beside the transport, and changing it while audio clips play.
- **Loop strip** (2026-10-21): drawing loops on each ruler, the handles, snapping, the loop button. The bottom panels' rulers grew from 18/22 px to 28 px.
- **UI speed:** does it feel instant?
- **Looping clips:** looping MIDI and audio clips, and hearing the seams (looped audio not yet seen running by Claude either).
- **Audio clips:** importing, dropping files from Explorer (Claude could not test that), clip BPM and stretching by ear. The song tempo is now typed beside the transport (2026-10-21), so stretching can be heard by changing either the song's or a clip's BPM.
- **Signals page and listening layers:** driven by Claude, not yet tried by the user.

## Next up

1. **Phase 8, VST3 hosting:** see "In progress" above.
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

## Known gaps

- **TODO (user, 2026-10-19): audio file types beyond WAV** (MP3, FLAC, AIFF; `symphonia` is the likely crate) for samples and audio clips. Reading files in the background, too.
- No node colour picker yet (the engine has `SetNodeColor`).
- Plugin hosting: starting the host process and creating instances happens on the control (UI) thread, so the first plugin in a song pauses the UI briefly; Windows only (macOS and Linux report "unsupported"); no delay compensation yet; plugins always run in realtime mode (offline exports should set up `kOffline`).
- One built-in effect (Filter) and two instruments (Synth, SingleSampler). Effects are added on the mixer; their settings show only on Parameters for now.
- Recording, MIDI keyboard input, MIDI file import and more instruments are Phase 9.
- The inspector shows nothing on Parameters, for drawn signals, or for a selected effect or instrument.
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Reference WAVs (`crates/*/tests/references/`) total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
