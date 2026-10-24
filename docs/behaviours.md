# Behaviours

Every user-facing behaviour of the app, one line each, with the headless UI test that guards it (`crates/daw-ui/src/ui_tests/`). The full test run (`./scripts/full-test.sh`) runs them all. The details of each behaviour live in the design doc named in its section.

**Rules** (also in `CLAUDE.md`):

- A new behaviour gets a line here and, where the harness can reach it, a UI test, in the same change.
- **Removing or changing a listed behaviour needs the user's OK**, even as a side effect of other work. If a change makes a line untrue, stop and ask.
- Lines marked *untested* are backfilled over time, most-used first.

Test names are in `ui_tests/<file>.rs`. A dash means untested.

## Building out the suite

The dashes below are the backlog. To cover one:

1. Tag what the test presses with `.debug_selector(|| format!("<page>-<thing>-{}", id.0))` in the view (it does nothing in normal builds).
2. Add a test to `crates/daw-ui/src/ui_tests/<page>.rs` (one file per page; register new files in `ui_tests/mod.rs`). `Harness::new("<name>")` opens the starter song (a Synth node, one clip over bars 1–4) headless; `h.bounds(..)` / `h.arrange_lane(..)` find things, `h.click(at, clicks)` presses, `h.snapshot()` reads the engine, `h.view.read_with(h.cx, ..)` reads view state (selection, open panels). Add harness helpers (drags, keys via `h.cx.simulate_keystrokes("ctrl-z")`) as tests need them.
3. Assert the result in the engine (clips, notes, nodes) and in view state, not pixels.
4. Check it catches the bug: break the behaviour briefly and see the test fail.
5. Put the test's name on the behaviour's line here.

Run just these with `cargo test -p daw-ui --features ui-tests ui_tests` (add a name to filter); `./scripts/full-test.sh` runs them with everything else. Only `check.sh` runs on every commit, so these never slow the everyday loop. Things the harness can't reach (system file drops, audio you need to hear) stay as manual checks with `scripts/ui-drive.ps1` or the user.

## Arrange (`node-tree.md`, `note-flow.md`, `audio-clips.md`)

| Behaviour | Test |
|---|---|
| Double-click an empty MIDI lane: a one-bar clip on the grid line under the pointer (shorter before the next clip), selected, editor open on it | `arrange::double_clicking_an_empty_midi_lane_adds_a_one_bar_clip_and_opens_the_editor` |
| Double-click a MIDI clip: the editor opens on its node at the clip | – |
| Double-click an audio clip or empty audio lane: the audio panel opens | – |
| Double-click a combined block on a parent: the editor opens on the parent there | – |
| Click a clip selects it; Shift+click adds or removes; a press on empty lane clears | – |
| Drag clips to move (snapped, between rows); Ctrl copies; Alt flips snap; Escape cancels | – |
| A clip drag is heard as it moves; Escape puts it back | `live_drags::a_clip_drag_is_heard_as_it_moves_and_escape_puts_it_back` |
| Drag a clip's edge to resize (trim and reveal for audio); a press on its outline or up to 4 px past it takes the end, so narrow clips resize too | `arrange::dragging_from_just_past_a_clips_end_resizes_it` |
| Drag the top of a clip's right end (or its loop icon, shown on hover or selection) to loop it: whole repeats, linked; repeats softer with a line at each cycle end (`clip-loops.md`) | `arrange::dragging_the_top_of_a_clips_end_loops_it` |
| Dropping clips below the last row makes a new node of their kind | – |
| A dropped or resized clip cuts what it lands on (clips never overlap on a node) | – |
| Rows that can't take the clip refuse: red ghost, "Can't go here" | – |
| Drag a combined block: moves every clip in it; Delete removes them | – |
| Scissors tool (key 3; 1 for pointer): click cuts a clip, or every selected clip there | – |
| Delete removes the selected clips, else the selected node | – |
| Toolbar: + (MIDI or Audio node), group into a new node, duplicate, delete | – |
| Drag a node row onto a row or a map bar to move or nest it | – |
| Chevrons fold a parent (shared with the mixer); pulling the map scrolls it | – |
| Signal button: publish notes as a signal; lists the node's signals with go-to | – |
| Click the ruler (below its loop strip) to locate | – |
| Click a node to select it (enables Parameters, points the editor at it) | – |
| Track → Import Audio…: each file on a new audio node at the playhead | – |
| Drop files: into an audio node, or new nodes in the edge zones; a MIDI node refuses | – |

## Piano roll (`piano-roll.md`, `note-flow.md`)

| Behaviour | Test |
|---|---|
| Select tool (1): click, Shift+click, box select; drag to move, edges to resize, Ctrl copies | – |
| Double-click empty grid adds a note one grid step long; double-click a note deletes it | – |
| Pen tool (2): click to add and drag its length; click a note to delete | – |
| Scissors tool (3): click a note to split it | – |
| Drawing where the node has no clip makes a one-bar clip | – |
| A looped clip's notes show once (editable); its repeats show faded; a note placed in a loop goes into the cycle | – |
| Pressing a softer (inherited) note switches the edited node and carries on into the move | – |
| Keys: Delete, up/down (Shift: octave), left/right a grid step, Ctrl+A, Ctrl+C, Ctrl+V at the playhead | – |
| Quantize (button or Q) | – |
| Velocity lane: drag a bar sets the selection's velocities | – |
| Click a key previews the note; adding or moving notes previews them | – |
| Tree panel: click a name selects the node; pen picks the drawing target | – |
| Fold shows only played pitches | – |
| × closes the panel; its top edge resizes it | – |

## Audio panel (`audio-clips.md`)

| Behaviour | Test |
|---|---|
| Click a clip selects it; settings column shows it | – |
| BPM typed, Enter applies, the clip keeps showing the same audio | – |
| Follow tempo switch; Gain and Fade sliders (double-click for default) | – |

## Parameters (`core-loop-ui.md`, `motion-signals.md`)

| Behaviour | Test |
|---|---|
| + Add Parameter; + Add Layer menu: Manual drawn or Listen to signal › | – |
| + Add Parameter on a plugin lists its parameters in its groups; search and Enter pick one, then its first layer's type (one undo) | `plugin_params::a_plugin_parameter_is_found_by_search_and_gets_a_layer_in_one_undo` |
| A plugin's parameter list: arrows move; Escape or a click outside closes; parameters with layers aren't offered | `plugin_params::the_plugin_parameter_list_works_from_the_keys_and_closes` |
| Learn beside a plugin's + Add Parameter opens its window and adds the next knob turned there, with a drawn layer; clicking again stops | `plugin_params::learn_opens_the_plugin_and_waits_for_a_knob_until_stopped` |
| Double-click empty layer lane: a one-bar flat block | – |
| Drag a block to move; edges resize as a window; top of the right edge loops | – |
| Drag a point; double-click adds a point; right-click removes one | – |
| Delete removes the selected block | – |
| Layer rows: drag to reorder, rename, blend mode, mix, show/hide, delete | – |
| Resting-value slider sets the default state | – |
| A listening layer's chip go-to opens the signal | – |

## Signals (`motion-signals.md`, `inspector.md`)

| Behaviour | Test |
|---|---|
| + Add Signal makes a drawn signal; draw on its lane like a layer | – |
| Colour bar picks a colour; double-click renames; × deletes | – |
| Uses chip lists listening layers with go-to | – |
| Click a row selects it; a notes signal's settings show in the inspector | – |

## Inspector (`inspector.md`)

| Behaviour | Test |
|---|---|
| Tray button opens and closes it; lights when the selection has settings | – |
| Drag the left edge resizes it | – |
| Notes signal: envelope sliders; pitch range and velocity clamp switches | – |
| A selected node on Arrange shows its mixer strip | – |

## Mixer (`mixer.md`)

| Behaviour | Test |
|---|---|
| Fader and pan follow the pointer and are heard while dragged; each drag is one undo step | `live_drags::a_fader_follows_the_pointer_and_is_heard_while_dragged`, `live_drags::a_pan_follows_the_pointer_and_is_heard_while_dragged` |
| Fader and pan double-click resets; mute and solo | – |
| + Instrument and + Effect open the device browser (Built-in \| Plugins, plugins under vendor headings); search and Enter add a plugin | `browser::a_plugin_effect_is_found_by_search_and_added_with_enter` |
| The device browser opens on the side used last; arrows and Enter pick; Escape or a click outside closes | `browser::the_browser_remembers_its_side_and_works_from_the_keys` |
| Effect slots switch and remove | – |
| Double-click a plugin's slot on a strip to open its window (its tooltip says why if it can't) | `plugin_params::double_clicking_a_plugin_slot_opens_its_window` |
| SingleSampler chip: Choose sample… | – |
| SingleSampler panel: Plays note (its trigger note, auto-assigned from C1 among siblings) | – |
| Map chevrons fold; strip-width slider | – |

## App (`STATUS.md`, `audio-settings.md`)

| Behaviour | Test |
|---|---|
| Menus open on press (like native menus); pressing an open menu's trigger closes it | `menus::a_menu_opens_on_press_and_its_trigger_closes_it` |
| Space plays and stops | – |
| Every drag that changes the song (sliders, blocks, notes and velocities, clips, notes-signal and audio-clip settings) is heard as it moves and lands as one undo step; every slider's thumb stays under the pointer (Shift: five times finer) | `live_drags::*` |
| Loop strip on every ruler (`loop-strip.md`): drag draws a fresh loop, snapped to the page's grid (Slide: unsnapped), and switches looping on | `loop_strip::dragging_on_the_loop_strip_draws_a_loop_that_plays`, `loop_strip::sliding_leaves_the_loop_off_the_grid` |
| Any drag on the strip but a handle draws a fresh loop, inside the loop too | `loop_strip::a_drag_inside_the_loop_draws_a_fresh_one` |
| Drag a loop's start or end handle to move that end | `loop_strip::dragging_a_handle_moves_only_that_end` |
| The piano roll's strip snaps to the roll's own grid | `loop_strip::the_piano_roll_strip_uses_the_rolls_grid` |
| The transport's loop button switches looping, keeping the region (grey while off) | `loop_strip::the_loop_button_switches_looping_and_keeps_the_region` |
| Tempo beside the transport: click to type, Enter applies (one undo step) | `transport::typing_a_tempo_sets_the_song_tempo` |
| Escape, or a tempo outside 20 to 999, leaves it | `transport::escape_or_an_impossible_tempo_leaves_the_tempo` |
| Ctrl+S saves; Ctrl+Z / Ctrl+Shift+Z undo and redo | – |
| File → Export… (Ctrl+E, `export.md`): format, quality, range and tail, then a save dialog starting in the project's Exports folder; renders with progress, the app usable around it; Done shows length and peak | `export::ctrl_e_exports_the_song_to_the_chosen_file` |
| The export dialog closes with Cancel, Escape or a click outside | `export::the_export_dialog_closes_with_cancel_escape_or_a_click_outside` |
| Snap control: Snap or Slide, grid; Alt flips while dragging | – |
| Page tabs switch pages | – |
| Song name menu: Save, Rename, Back to projects | – |
| File → Preferences (Ctrl+,): Appearance; Audio settings applied live; test tone | – |
| Plugins scan in the background from startup (only new or changed ones) | – (the harness does not reach the app root) |
| Preferences → Plugins: the standard folders, Add Folder and × for added ones (each rescans), Rescan, the count or "Scanning n of N…", Couldn't load with why and Retry | – (as above) |
| Launch page: new project, Open…, recent projects | – |
| Ctrl+Q saves and quits; autosave every 30 s | – |
| With `DAW_FRAME_LOG` set: "UI n ms" beside the CPU meter (median work per frame over 2 s; frames a second in its tooltip) | – |
