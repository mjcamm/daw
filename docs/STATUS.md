# Status and Next Steps

*Last updated: 2026-10-16. Read this first when picking up the project.*

## Where we are

- **Phases 0–2 done:** foundations, headless engine, parameters and layers.
- **Phase 3 (core-loop UI): built and tried by the user**, who is happy with it. Decisions: `docs/design/core-loop-ui.md`.
- **Phase 4 (motion signals): engine done to the revised design** (2026-10-15, `docs/design/motion-signals.md`): layers (not blocks) listen to signals, signals are centred, notes are ADSR hits, removing a signal sends its listeners back to drawing. File version 4. **Signals page built** (drawn signals, colour, rename, delete, uses list with go-to) and **listening layers on Parameters** (type picked when adding, then fixed; chip with hover explanation and go-to; windows show the signal). Driven and checked by Claude via `scripts/ui-drive.ps1`; not yet tried by the user. Layer rows are two lines (name; type chip and blend mode) and rows on every timeline page are at least 44 px; not yet seen running.
- **Phase 5 (node tree): engine done** (2026-10-16, `docs/design/node-tree.md`): nodes form a tree under the Song (the master), parents have no instrument, sound sums up through parents (a centred parent passes sound unchanged; existing songs render bit-for-bit the same), mute and solo work across the tree, per-node meters, project-wide ids, notes signals name their source node, file version 7 (older files nest their node under the Song). **Arrange tree built** (2026-10-16): flat rows with parents after their children and the Song last, the vertical nesting map with folding, a toolbar (add a MIDI or Audio node, group into a new node, duplicate, delete; Delete key too), and drag to move or nest. Rows animate (grow, close, glide; folding slides children into their parent). Parent lanes stay blank until the merged waveform (Phase 6). **Clips** move, copy (Ctrl), resize and cut what they land on, between nodes too, with a live preview; dropping below the last row makes a new node. Driven and checked by Claude; not yet tried by the user. **Mixer built** (`docs/design/mixer.md`): aligned strips with effect slots (add, on/off, remove), pan, fader with automation marker, stereo meters with peak hold, mute and solo; the nesting map along the bottom with folding shared with Arrange.
- **Publish as signal and the inspector** (2026-10-16): the node row on Arrange publishes its notes as a signal; the **inspector** (right panel, folding into a tray, `docs/design/inspector.md`) configures a selected notes signal: an optional pitch range, ADSR envelope with a live graph, and an optional velocity clamp (Min/Max). The two optional parts have on/off switches, off by default (file version 6). Driven and checked by Claude; tried by the user ("feels great").

- **Audio clips** (2026-10-19, `docs/design/audio-clips.md`): agreed with the user. Stretching is live, at playback, with the user's own stretcher brought over as `crates/daw-stretch` (`docs/design/stretcher.md`). **Engine built:** `AudioClip` (a window onto a sample: offset, own BPM, follow tempo, gain, fades), `AddAudioClip` / `SetAudioClip` / `RemoveClip`, file version 12, conversion to the engine's rate, waveform overviews in `SampleSnapshot.peaks`, playback through one stretcher per audio node (any block size, any thread count). **UI so far:** Track → Import Audio… (each file on a new audio node, named after it, at the playhead), dropping files on Arrange (into an audio node, or new nodes between rows; the edge zones scale with zoom), clips with waveforms, move/copy/trim/reveal. the scissors tool on Arrange (pointer / scissors left of snap, keys 1 and 3). the audio panel (double-click an audio clip: its waveform per side, and BPM, Follow tempo, Gain and Fades on the right). **Try it (user):** importing, dropping files from Explorer (Claude could not test that), BPM and stretching by ear.

## In the app (`cargo run -p daw-ui`)

- **Menu bar:** File → Preferences… (Ctrl+,: Appearance System/Light/Dark, saved to `preferences.json`) and Quit (Ctrl+Q, saves first).
- **Launch page:** new project (`Documents/DAW Projects/<name>/project.json`), Open…, recent projects. New songs start with one Synth node and the demo notes.
- **Signals:** "+ Add Signal" makes a drawn signal; draw on its lane like a layer. Colour bar picks a colour, double-click renames, × deletes, the uses chip lists listening layers with go-to icons. Click a row to select it; a notes signal's settings are in the inspector.
- **Inspector:** the tray icon at the far right lights up when the selection has settings; click to open or close, drag the panel's left edge to resize.
- **Header:** song name menu (Save, Rename, Back to projects), page tabs, snap control, transport.
- **Arrange:** node row (colour, name, meter, signal button, M/S) and clips on the timeline; click the ruler to locate; click the node to select it (this enables Parameters). The signal button publishes the notes as a signal and lists the node's signals with go-to icons.
- **Parameters:** per device, "+ Add Parameter" makes a result row with its layers. Layers: drag to reorder, rename, blend mode (Add or Multiply, both centred), mix, show/hide, delete. Block editing on the lanes, settings bar, resting-value slider.
- **Both timeline pages:** scrollbars with zoom sliders at their ends; wheel scrolls vertically, Shift+wheel sideways.
- **Shortcuts:** Space play/stop, Ctrl+S save, Ctrl+Z / Ctrl+Shift+Z undo/redo, Delete removes the selected block, Alt flips snap while dragging. Autosave every 30 s (`project.autosave.json`, removed on save).

## Model changes worth knowing

- **Piano roll** (2026-10-16, `docs/design/piano-roll.md`): double-click a clip on Arrange for a bottom panel with select, pen and scissors tools, a keyboard, a velocity lane, note preview, and the usual keys; its own grid and slide, Arrange's scroll and zoom frame, resize zones that scale with the note. Tried and tuned by the user (2026-10-18: "all looks good").
- **Notes in the tree** (2026-10-19, `docs/design/note-flow.md`): instruments play only their own notes (a SingleSampler its trigger note, auto-assigned from C1 among siblings); a node's notes pass up to its parents' instruments, never down. A parent's lane shows **combined blocks** of the clips below it (inherited notes softer; drag moves them all, Delete removes them). The **editor follows the selected node**, not a clip: song time, all its clips, drawing where it has no clip makes a one-bar clip; the tree beside it shows the whole branch. In a parent's view every note in the branch is **editable in place**: pressing a softer note makes its node the edited one ("Drawing to" flashes) and the press carries on into the move. A branch is all MIDI or all Audio (file version 11). **Clips on a node never overlap** (MIDI or audio; the engine rejects stacking, file version 13 merges old stacked MIDI clips; the starter song is one clip now).
- **Audio settings and engine safety** (2026-10-18, `docs/design/audio-settings.md`): File → Preferences → Audio chooses driver, output device, sample rate, buffer size and processing threads, applied live without losing the song; a test tone; a CPU meter in the header. Denormals are flushed while rendering, the output is guarded (no NaN, never past full scale), and nodes render in parallel across cores with bit-identical results. ASIO waits on the user's go-ahead (it needs the ASIO SDK and LLVM installed).
- **Audio engine: samples and the SingleSampler** (2026-10-18, file version 9, `docs/design/samples.md`): a built-in instrument that plays one WAV sample, one-shot, for every note, at the file's own rate; samples are copied into the project's `audio/` folder and decoded into a pool the audio thread only reads; instruments render in stereo. Add it from the instrument browser; click its slot on the strip to choose a sample. Trigger notes and the drum-machine parent come next.
- **Layered instruments** (file version 8): a MIDI node holds a list of instruments (`Node.instruments`, `DeviceRef::Instrument(DeviceId)`), each playing every note, summed; `AddInstrument`, `RemoveInstrument`, and `SetDeviceEnabled` for instruments and effects alike. Older files' single instrument becomes a list of one. Added on the mixer through the instrument browser.
- **Node kinds** (2026-10-16, the user's call): every node is MIDI or Audio (`NodeKind`). MIDI clips go on any MIDI node, with or without an instrument; audio nodes take audio clips once they exist; the Song holds no clips. There is no folder type: a parent is any node with children. **A branch never mixes kinds** (the user's call, 2026-10-19; file version 11): a node goes only under a parent of its own kind, the Song takes either. Only MIDI nodes have instruments.
- **Parent first** (the user's call, reversing the earlier "parents below"): on Arrange the Song is at the top and each parent sits above its children; on the mixer each parent strip sits left of its children. Opening slides children out from under the parent; folding slides them back. Map bars start at the parent's row (or strip) with the chevron; while a node is dragged, every row offers a drop slot to nest into it. Map sizes ease as levels come and go. The Song has no map bar (2026-10-18); Arrange's bars start at the top of their parent's row, and past four columns the map shows four and a half.

- **Layers are centred** (file version 3): the middle of a lane is no effect, top +1, bottom −1. Only Add and Multiply remain; `crates/daw-engine/src/upgrade.rs` converts older files exactly (Replace only when nothing below it acts).
- **Layer mix** scales every block's weight on a layer.
- **Signals** (file version 4): drawn, or ADSR hits from MIDI notes. A layer listens with `Layer.signal`; its blocks are windows onto the signal. No range: block depth and invert do it.
- **Velocity clamp** (file version 5): a hit's peak is its velocity held between Min and Max (was a "velocity amount"). Version 6 added on/off switches for the clamp and the pitch range.

## Waiting on the user

Nothing open. (2026-10-16: the user tried publish + inspector, "feels great"; dark mode "looks great".)

## Next up

1. **Phase 5, rest** (`docs/design/node-tree.md`): a richer demo song (drums with a separate kick, a music parent). An effect's or instrument's settings in the inspector are on hold (the user's call). The node's strip shows in the panel on Arrange (built 2026-10-16). (`docs/design/mixer.md`), the inspector's channel strip, a richer demo song (drums with a separate kick, music parent).
2. Phase 4's "done when" walkthrough (kick ducking several parameters in the UI) is still to run; the user chose to start Phase 5 first. Showing a node's pitches on notes lanes is deferred; the range picker is enough for now.

## Design rules the user has set

- **One way to do anything.** No duplicate routes to the same result.
- **Visible, not hidden.** Clear icons and buttons (e.g. go-to icons), not secret clicks or right-click-only menus.

## How the user likes to work

- **Ask about layout before building each UI element.** They answer in plain numbered text (the multiple-choice tool was rejected once). Offer a recommendation.
- Calm, dense, native pro-app look (`docs/design/look-and-feel.md`), not "bootstrappy". Lucide icons (pinned `lucide-static@0.460.0`).
- Run `./scripts/check.sh`, then commit and push when work is done. No GitHub Actions on push.
- **Keep this file and `docs/design/` current** as work lands: decisions and pointers, not narration.
- Say when their eyes or ears are needed ("Try it", "Listen now").
- On Windows, a running app blocks rebuilding `daw.exe` ("failed to remove file"). `check.sh` still works; ask them to close the app to run it.

## Known gaps

- **TODO (user, 2026-10-19): audio file types beyond WAV** (MP3, FLAC, AIFF; `symphonia` is the likely crate) for samples and audio clips. Reading files in the background, too.

- MIDI: new clips by double-clicking an empty MIDI lane; quantize and auto-scroll in the piano roll (2026-10-18). Recording, MIDI keyboard input, MIDI file import and more instruments are Phase 9. Effects are added on the mixer; their settings show only on Parameters for now. No node colour picker yet.
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Reference WAVs in `crates/daw-engine/tests/references/` total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
- Inspector shows nothing on Arrange and Parameters yet, and nothing for drawn signals.
