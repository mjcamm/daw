# Status and Next Steps

*Last updated: 2026-10-16. Read this first when picking up the project.*

## Where we are

- **Phases 0–2 done:** foundations, headless engine, parameters and layers.
- **Phase 3 (core-loop UI): built and tried by the user**, who is happy with it. Decisions: `docs/design/core-loop-ui.md`.
- **Phase 4 (motion signals): engine done to the revised design** (2026-10-15, `docs/design/motion-signals.md`): layers (not blocks) listen to signals, signals are centred, notes are ADSR hits, removing a signal sends its listeners back to drawing. File version 4. **Signals page built** (drawn signals, colour, rename, delete, uses list with go-to) and **listening layers on Parameters** (type picked when adding, then fixed; chip with hover explanation and go-to; windows show the signal). Driven and checked by Claude via `scripts/ui-drive.ps1`; not yet tried by the user. Layer rows are two lines (name; type chip and blend mode) and rows on every timeline page are at least 44 px; not yet seen running.
- **Phase 5 (node tree): engine done** (2026-10-16, `docs/design/node-tree.md`): nodes form a tree under the Song (the master), folders have no instrument, sound sums up through folders (a centred folder passes sound unchanged; existing songs render bit-for-bit the same), mute and solo work across the tree, per-node meters, project-wide ids, notes signals name their source node, file version 7 (older files nest their node under the Song). **Arrange tree built** (2026-10-16): flat rows with folders after their children and the Song last, the vertical nesting map with folding, a toolbar (add instrument or folder, group, duplicate, delete; Delete key too), and drag to move or nest. Rows animate (grow, close, glide; folding slides children into their folder). Folder lanes stay blank until the merged waveform (Phase 6). **Clips** move, copy (Ctrl), resize and cut what they land on, between nodes too, with a live preview; dropping below the last row makes a new node. Driven and checked by Claude; not yet tried by the user. **Mixer built** (`docs/design/mixer.md`): aligned strips with effect slots (add, on/off, remove), pan, fader with automation marker, stereo meters with peak hold, mute and solo; the nesting map along the bottom with folding shared with Arrange.
- **Publish as signal and the inspector** (2026-10-16): the node row on Arrange publishes its notes as a signal; the **inspector** (right panel, folding into a tray, `docs/design/inspector.md`) configures a selected notes signal: an optional pitch range, ADSR envelope with a live graph, and an optional velocity clamp (Min/Max). The two optional parts have on/off switches, off by default (file version 6). Driven and checked by Claude; tried by the user ("feels great").

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

- **Piano roll** (2026-10-16, `docs/design/piano-roll.md`): double-click a clip on Arrange for a bottom panel with select, pen and scissors tools, a keyboard, a velocity lane, note preview, and the usual keys. Driven and checked by Claude; not yet tried by the user.
- **Layered instruments** (file version 8): a MIDI node holds a list of instruments (`Node.instruments`, `DeviceRef::Instrument(DeviceId)`), each playing every note, summed; `AddInstrument`, `RemoveInstrument`, and `SetDeviceEnabled` for instruments and effects alike. Older files' single instrument becomes a list of one. Added on the mixer through the instrument browser.
- **Node kinds** (2026-10-16, the user's call): every node is MIDI or Audio (`NodeKind`). MIDI clips go on any MIDI node, with or without an instrument; audio nodes take audio clips once they exist; the Song holds no clips. "Folder" is not a type: any node with children is one. Only MIDI nodes have instruments.
- **Parent first** (the user's call, reversing the earlier "folders below"): on Arrange the Song is at the top and each parent sits above its children; on the mixer each parent strip sits left of its children. Opening slides children out from under the parent; folding slides them back. Map bars start at the parent's row (or strip) with the chevron; while a node is dragged, every row offers a drop slot to nest into it. Map sizes ease as levels come and go. The Song has no map bar (2026-10-18); Arrange's bars start at the top of their folder's row, and past four columns the map shows four and a half.

- **Layers are centred** (file version 3): the middle of a lane is no effect, top +1, bottom −1. Only Add and Multiply remain; `crates/daw-engine/src/upgrade.rs` converts older files exactly (Replace only when nothing below it acts).
- **Layer mix** scales every block's weight on a layer.
- **Signals** (file version 4): drawn, or ADSR hits from MIDI notes. A layer listens with `Layer.signal`; its blocks are windows onto the signal. No range: block depth and invert do it.
- **Velocity clamp** (file version 5): a hit's peak is its velocity held between Min and Max (was a "velocity amount"). Version 6 added on/off switches for the clamp and the pitch range.

## Waiting on the user

Nothing open. (2026-10-16: the user tried publish + inspector, "feels great"; dark mode "looks great".)

## Next up

1. **Phase 5, rest** (`docs/design/node-tree.md`): a richer demo song (drums with a separate kick, a music folder). An effect's or instrument's settings in the inspector are on hold (the user's call). The node's strip shows in the panel on Arrange (built 2026-10-16). (`docs/design/mixer.md`), the inspector's channel strip, a richer demo song (drums with a separate kick, music folder).
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

- New clips can't be made from nothing yet (only moved, copied, resized, and their notes edited in the piano roll). Effects are added on the mixer; their settings show only on Parameters for now. No node colour picker yet.
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Reference WAVs in `crates/daw-engine/tests/references/` total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
- Inspector shows nothing on Arrange and Parameters yet, and nothing for drawn signals.
