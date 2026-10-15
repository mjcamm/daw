# Status and Next Steps

*Last updated: 2026-10-15. Read this first when picking up the project.*

## Where we are

- **Phases 0–2 done:** foundations, headless engine, parameters and layers.
- **Phase 3 (core-loop UI): built and tried by the user**, who is happy with it. Decisions: `docs/design/core-loop-ui.md`.
- **Phase 4 (motion signals): design revised with the user** (2026-10-15): layers (not blocks) listen to signals, signals are centred, notes become ADSR hits, Signals page layout agreed. The first-draft engine (`crates/daw-engine/tests/signals.rs`) is being changed to match; see "Engine changes" in `docs/design/motion-signals.md`. UI not started.

## In the app (`cargo run -p daw-ui`)

- **Menu bar:** File → Preferences… (Ctrl+,: Appearance System/Light/Dark, saved to `preferences.json`) and Quit (Ctrl+Q, saves first).
- **Launch page:** new project (`Documents/DAW Projects/<name>/project.json`), Open…, recent projects. New songs start with one Synth node and the demo notes.
- **Header:** song name menu (Save, Rename, Back to projects), page tabs, snap control, transport.
- **Arrange:** node row (colour, name, meter, M/S) and clips on the timeline; click the ruler to locate; click the node to select it (this enables Parameters).
- **Parameters:** per device, "+ Add Parameter" makes a result row with its layers. Layers: drag to reorder, rename, blend mode (Add or Multiply, both centred), mix, show/hide, delete. Block editing on the lanes, settings bar, resting-value slider.
- **Both timeline pages:** scrollbars with zoom sliders at their ends; wheel scrolls vertically, Shift+wheel sideways.
- **Shortcuts:** Space play/stop, Ctrl+S save, Ctrl+Z / Ctrl+Shift+Z undo/redo, Delete removes the selected block, Alt flips snap while dragging. Autosave every 30 s (`project.autosave.json`, removed on save).

## Model changes worth knowing

- **Layers are centred** (file version 3): the middle of a lane is no effect, top +1, bottom −1. Only Add and Multiply remain; `crates/daw-engine/src/upgrade.rs` converts older files exactly (Replace only when nothing below it acts).
- **Layer mix** scales every block's weight on a layer.
- **Signals** (file version 2, first draft): drawn or from MIDI notes; a block links with `Block.link` and a low..high range. Being replaced by layer-level listening (version 4).

## Waiting on the user

1. **Node signal menu wording** (Arrange): "Publish notes as motion signal", "Publish sound as motion signal" / "Publish sound as audio signal" (greyed until Phase 6).
2. **Dark mode** look check.
3. **Milestone gate placement:** `build-plan.md` has it after Phase 4; earlier notes had it after Phase 3.

## Next up

1. Engine changes listed in `motion-signals.md` (test-first).
2. Signals page, then the layer source pick list on Parameters, then "Publish as signal" on Arrange. Ask about details of each before building.
3. The milestone gate ("does shaping movement feel great?").

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
