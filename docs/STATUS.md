# Status and Next Steps

*Last updated: 2026-10-16. Read this first when picking up the project.*

## Where we are

- **Phases 0–2 done:** foundations, headless engine, parameters and layers.
- **Phase 3 (core-loop UI): built and tried by the user**, who is happy with it. Decisions: `docs/design/core-loop-ui.md`.
- **Phase 4 (motion signals): engine done to the revised design** (2026-10-15, `docs/design/motion-signals.md`): layers (not blocks) listen to signals, signals are centred, notes are ADSR hits, removing a signal sends its listeners back to drawing. File version 4. **Signals page built** (drawn signals, colour, rename, delete, uses list with go-to) and **listening layers on Parameters** (type picked when adding, then fixed; chip with hover explanation and go-to; windows show the signal). Driven and checked by Claude via `scripts/ui-drive.ps1`; not yet tried by the user. Layer rows are two lines (name; type chip and blend mode) and rows on every timeline page are at least 44 px; not yet seen running.
- **Publish as signal and the inspector** (2026-10-16): the node row on Arrange publishes its notes as a signal; the **inspector** (right panel, folding into a tray, `docs/design/inspector.md`) configures a selected notes signal: an optional pitch range, ADSR envelope with a live graph, and an optional velocity clamp (Min/Max). The two optional parts have on/off switches, off by default (file version 6). Driven and checked by Claude; not yet tried by the user.

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

- **Layers are centred** (file version 3): the middle of a lane is no effect, top +1, bottom −1. Only Add and Multiply remain; `crates/daw-engine/src/upgrade.rs` converts older files exactly (Replace only when nothing below it acts).
- **Layer mix** scales every block's weight on a layer.
- **Signals** (file version 4): drawn, or ADSR hits from MIDI notes. A layer listens with `Layer.signal`; its blocks are windows onto the signal. No range: block depth and invert do it.
- **Velocity clamp** (file version 5): a hit's peak is its velocity held between Min and Max (was a "velocity amount"). Version 6 added on/off switches for the clamp and the pitch range.

## Waiting on the user

1. **Try publish + inspector:** publish from Arrange, follow go-to, tune the envelope and velocity clamp.
2. **Dark mode** look check.
3. **Milestone gate placement:** `build-plan.md` has it after Phase 4; earlier notes had it after Phase 3.

## Next up

1. Phase 4's "done when": build a kick ducking several parameters, all in the UI, and see what is missing. (Showing a node's pitches on notes lanes is deferred by the user; the range picker is enough for now.)
2. The milestone gate ("does shaping movement feel great?").

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

- One node only (node tree is Phase 5). No adding effects in the UI yet (engine has a Filter).
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Reference WAVs in `crates/daw-engine/tests/references/` total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
- Inspector shows nothing on Arrange and Parameters yet, and nothing for drawn signals.
