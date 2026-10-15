# Status and Next Steps

*Last updated: 2026-10-15. Read this first when picking up the project.*

## Where we are

Phases 0–2 of `build-plan.md` are done (foundations, headless engine, parameters and layers in the engine). **Phase 3 (core-loop UI) is in progress.** The UI was moved ahead of signals; signals are Phase 4.

Working in the app (`cargo run -p daw-ui`):

- **Launch page:** new project (asks for a name, creates `Documents/DAW Projects/<name>/project.json`), Open… (folder picker), recent projects list.
- **New songs** start with one Synth node plus the demo's bass and arpeggio clips at 124 BPM, looping 4 bars, no layers, empty undo history.
- **Header:** song name (click for menu: Save, Rename, Back to projects; double-click to rename), "Edited/Saved" status, tabs (Arrange · Parameters · Signals · Mixer), snap control (magnet chip: Snap/Slide and grid 1 bar … 1/64), transport (to start, play/stop, loop, position).
- **Arrange page:** node row (colour, name with double-click rename, stereo meter, M/S), timeline with clips, notes, loop band, playhead; click ruler to locate. Clicking the node row selects it; clicking empty space below deselects.
- **Parameters page** (tab greyed out until a node is selected): sections Global (Channel), Instrument (Synth), Effects (listed only, no adding here). "+ Add Parameter" picks a parameter to create a layer set; each set shows its live value and result curve; layers list top-of-stack first (Photoshop order), with grip to drag-reorder, name (double-click rename), blend mode menu, eye toggle, × delete. "+ Add Layer" adds at the bottom of the stack.
- **Block editing** on layer lanes (built, **not yet tried by the user**; see `docs/design/block-editing.md`): double-click for a new block, drag to move, drag edges to widen/narrow the window (contents stay put in song time), corner handle to repeat, points on the selected block (drag, double-click to add, right-click to remove), settings bar (depth, fades, repeat, nudge, invert), Delete to remove. Each drag is one undo step.
- **Shortcuts:** space play/stop, Ctrl/Cmd+S save, Ctrl/Cmd+Z undo, Ctrl/Cmd+Shift+Z redo, Delete/Backspace deletes the selected block, Alt while dragging flips snap/slide. Autosave backup every 30 s (`project.autosave.json`), removed on save. No crash-recovery prompt yet.

## Waiting on the user

1. **Blend modes:** the user does not want **Replace**. Asked (unanswered): remove it only from the menu, or from the engine too (then convert the demo's filter sweep to a low resting value plus an Add layer)? Also whether to keep **Modulate** (moves around the current value, for wobbles) and under what name. New layers currently default to Replace; switch the default to Add once decided.
2. **Height fix** on the Parameters page (flex growth instead of `min_h_full`) is pushed but not yet confirmed by the user on screen.
3. **Try block editing.** Open points: the selected block's lane grows to 96 px (layout jumps on selection; alternative is a fixed taller lane); points slide freely (no snap); snap setting is not saved with the project.
4. **Milestone gate placement:** `build-plan.md` puts "does shaping movement feel great?" after Phase 4; this file had it at the end of Phase 3. Ask which, then make the two agree.

## Next up

1. The user tries block editing; iterate on feel.
2. Then the remaining Phase 3 items: the milestone gate ("does shaping movement feel great?") with the user, then Phase 4 signals.

## How the user likes to work

- **Ask about layout before building each UI element** (they answer in plain numbered text; the multiple-choice question tool was rejected once). Mockups in `docs/ui-ideas/` are look-and-feel only.
- **Not "bootstrappy":** calm, dense, native pro-app look per `docs/design/look-and-feel.md`; quiet controls, hairlines, colour only for content and the one accent.
- Icons from Lucide (pinned `lucide-static@0.460.0`), not hand-drawn.
- No GitHub Actions on push (manual cross-platform workflow only on request). Run `./scripts/check.sh` before every commit; commit and push when work is done. **Keep this file and `docs/design/` updated as work progresses** (the user asked for this explicitly).
- Call out when the user's ears or eyes are needed ("Listen now" / "Try it").
- The user often has the app open: on Windows the build then fails with "failed to remove file ... daw.exe". The code still compiled; ask them to close it.

## Known gaps and notes

- One node only; the node tree is Phase 5. Mute/solo/meter are per node already.
- No way to add effects in the UI yet (planned: from the track header, Logic-style). The engine supports a Filter effect.
- Parameters update every 32 samples (gains ramp, others step); revisit if anything sounds zippery.
- Live playback runs the audio thread at real-time priority (cpal `realtime` feature); `daw-player play` prints callback timing stats.
- Reference WAVs in `crates/daw-engine/tests/references/` total about 4 MB; keep new ones short.
- `docs/gpui-notes.md` holds hard-won GPUI knowledge; add to it.
