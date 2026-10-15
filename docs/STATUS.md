# Status and Next Steps

*Last updated: 2026-10-15. Read this first when picking up the project.*

## Where we are

Phases 0–2 of `build-plan.md` are done (foundations, headless engine, parameters and layers in the engine). **Phase 3 (core-loop UI) is in progress.** The UI was moved ahead of signals; signals are Phase 4.

Working in the app (`cargo run -p daw-ui`):

- **Launch page:** new project (asks for a name, creates `Documents/DAW Projects/<name>/project.json`), Open… (folder picker), recent projects list.
- **New songs** start with one Synth node plus the demo's bass and arpeggio clips at 124 BPM, looping 4 bars, no layers, empty undo history.
- **Header:** song name (click for menu: Save, Rename, Back to projects; double-click to rename), "Edited/Saved" status, tabs (Arrange · Parameters · Signals · Mixer), transport (to start, play/stop, loop, position).
- **Arrange page:** node row (colour, name with double-click rename, stereo meter, M/S), timeline with clips, notes, loop band, playhead; click ruler to locate. Clicking the node row selects it; clicking empty space below deselects.
- **Parameters page** (tab greyed out until a node is selected): sections Global (Channel), Instrument (Synth), Effects (listed only, no adding here). "+ Add Parameter" picks a parameter to create a layer set; each set shows its live value and result curve; layers list top-of-stack first (Photoshop order), with grip to drag-reorder, name (double-click rename), blend mode menu, eye toggle, × delete. "+ Add Layer" adds at the bottom of the stack.
- **Shortcuts:** space play/stop, Ctrl/Cmd+S save, Ctrl/Cmd+Z undo, Ctrl/Cmd+Shift+Z redo. Autosave backup every 30 s (`project.autosave.json`), removed on save. No crash-recovery prompt yet.

## Waiting on the user

1. **Blend modes:** the user does not want **Replace**. Asked (unanswered): remove it only from the menu, or from the engine too (then convert the demo's filter sweep to a low resting value plus an Add layer)? Also whether to keep **Modulate** (moves around the current value, for wobbles) and under what name. New layers currently default to Replace; switch the default to Add once decided.
2. **Height fix** on the Parameters page (flex growth instead of `min_h_full`) is pushed but not yet confirmed by the user on screen.

## Next up: drawing blocks (agreed with the user)

On the Parameters page lanes:

- Drag across an empty part of a layer lane to create a block (starts as a flat line).
- Drag a block to move it; drag its edges to resize.
- Inside a selected block: drag points up/down, double-click to add a point, right-click a point to delete it.
- Selecting a block shows its settings (depth, invert, fades, repeat/cycle length, nudge) in a slim bar at the bottom of the page.
- Block ids: client-chosen, unique project-wide (see `next_layer_id` in `parameters.rs` for the pattern). Engine edits already exist: `AddBlock`, `SetBlock`, `RemoveBlock` (blocks in a layer cannot overlap).

After that, remaining Phase 3: the milestone gate ("does shaping movement feel great?") with the user, then Phase 4 signals.

## How the user likes to work

- **Ask about layout before building each UI element** (they answer in plain numbered text; the multiple-choice question tool was rejected once). Mockups in `docs/ui-ideas/` are look-and-feel only.
- **Not "bootstrappy":** calm, dense, native pro-app look per `docs/design/look-and-feel.md`; quiet controls, hairlines, colour only for content and the one accent.
- Icons from Lucide (pinned `lucide-static@0.460.0`), not hand-drawn.
- No GitHub Actions on push (manual cross-platform workflow only on request). Run `./scripts/check.sh` before every commit; commit and push when work is done.
- Call out when the user's ears or eyes are needed ("Listen now" / "Try it").
- The user often has the app open: on Windows the build then fails with "failed to remove file ... daw.exe". The code still compiled; ask them to close it.

## Known gaps and notes

- One node only; the node tree is Phase 5. Mute/solo/meter are per node already.
- No way to add effects in the UI yet (planned: from the track header, Logic-style). The engine supports a Filter effect.
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Live playback runs the audio thread at real-time priority (cpal `realtime` feature); `daw-player play` prints callback timing stats.
- Reference WAVs in `crates/daw-engine/tests/references/` total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
