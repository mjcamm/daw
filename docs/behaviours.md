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
2. Add a test to `crates/daw-ui/src/ui_tests/<page>.rs` (one file per page; register new files in `ui_tests/mod.rs`). `Harness::new("<name>")` opens the starter song (a Synth node, one clip over bars 1–4) headless; `h.bounds(..)` / `h.arrange_lane(..)` find things, `h.click(at, clicks)` presses, `h.snapshot()` reads the engine, `h.view.read_with(h.cx, ..)` reads view state (selection, open panels). Add harness helpers (drags, keys via `h.cx.simulate_keystrokes("ctrl-z")`) as tests need them. `h.redraw()` (and helpers that call it) rebuilds every pane, which hides bugs in cached panes; for hover looks and cursors, move with `h.hover(at)`, which draws only what the app asks to redraw, as a real move does. Bottom panels slide up in real time when they open: set the editor's `opened` back a second before pointing at it.
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
| A parent's combined (inherited) block moves its children's clips; Delete leaves it, and the parent | `arrange::delete_leaves_an_inherited_block_and_its_parent` |
| Delete (and every shortcut) shows at once, with no mouse move after the key | `arrange::delete_shows_at_once` |
| On Arrange, clicking a clip selects its track; a parent's combined block selects that parent | `arrange::clicking_a_clip_selects_its_track` |
| A click on empty space on Arrange (a lane where no clip is, below the lanes, below the nodes) clears the selection (node, clips, block) and closes the bottom panel | `arrange::clicking_empty_space_clears_the_selection_and_closes_the_panel` |
| With the bottom panel open, a single click on a clip shows it there (piano roll or audio panel) and selects its node; switching between the two swaps in place, at the same height, without sliding up again | `arrange::with_the_panel_open_a_click_on_a_clip_shows_it`, `arrange::the_piano_roll_takes_the_audio_panels_place_without_sliding` |
| The drawn loop cursor sits at the pointer, on top of everything, through a whole loop drag (Parameters and Signals draw theirs the same way) | `arrange::the_loop_cursor_follows_the_pointer_through_a_loop_drag` |
| Drag clips to move (snapped, between rows); Ctrl copies; Alt flips snap; Escape cancels | – |
| A clip drag is heard as it moves; Escape puts it back | `live_drags::a_clip_drag_is_heard_as_it_moves_and_escape_puts_it_back` |
| Drag a clip's edge to resize (trim and reveal for audio); a press on its outline or up to 4 px past it takes the end, so narrow clips resize too | `arrange::dragging_from_just_past_a_clips_end_resizes_it` |
| The cursor on the lanes follows the clips as they are: right after a drag lands or an undo (pointer still), moving away and back after a drag, and a resize cursor holds through the whole drag, off the row too | `arrange::the_resize_cursor_follows_the_clip_after_a_drag`, `arrange::after_a_drag_moving_away_and_back_shows_the_right_cursor` |
| Drag the top of a clip's right end (or its loop icon, shown on hover or selection) to loop it: whole repeats, linked; repeats softer with a line at each cycle end (`clip-loops.md`) | `arrange::dragging_the_top_of_a_clips_end_loops_it` |
| A looped clip's right end only drags repeats (top to bottom, on its outline and just past it), never resizes; its start still trims | `arrange::a_looped_clips_end_drags_repeats_never_resizes` |
| While dragging, the clip shows as it will land: a loop's repeats softer with their cycle lines | `arrange::dragging_out_a_loop_shows_its_repeats` |
| Dropping clips below the last row makes a new node of their kind | – |
| A dropped or resized clip cuts what it lands on (clips never overlap on a node) | – |
| Rows that can't take the clip refuse: red ghost, "Can't go here" | – |
| Drag a combined block: moves every clip in it; Delete removes them | – |
| Scissors tool (key 3; 1 for pointer): click cuts a clip, or every selected clip there | – |
| Delete removes the selected clips, else the selected node | – |
| Toolbar: + (MIDI or Audio node), group into a new node, duplicate, delete | – |
| Drag a node row onto a row or a map bar to move or nest it | – |
| Chevrons fold a parent (shared with the mixer); pulling the map scrolls it | – |
| Signal button: Publish MIDI Signal on a MIDI node (its notes), Publish Audio Signal on an audio node (its sound before its effects); lists the node's signals with go-to | `arrange::the_signal_button_publishes_by_the_nodes_kind`, `audio_signals::an_audio_nodes_signal_draws_its_loudness_once_worked_out` |
| Click the ruler (below its loop strip) to locate | – |
| Click a node to select it (enables Parameters, points the editor at it) | – |
| Track → Import Audio…: each file on a new audio node at the playhead | – |
| Drop files: into an audio node, or new nodes in the edge zones; a MIDI node refuses | – |

## Piano roll (`piano-roll.md`, `note-flow.md`)

| Behaviour | Test |
|---|---|
| Select tool (1): click, Shift+click, box select; drag to move, edges to resize, Ctrl copies | – |
| The resize cursor follows the notes as they are: after a resize, moving away and back (a resize lands even if notes were heard during it) | `piano_roll::after_a_note_resize_moving_away_and_back_shows_the_right_cursor` |
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
| Bend tool (4): drag a note's end dot up or down to bend it (a straight glide, snapped to semitones); heard as it moves, one undo step | `bends::dragging_the_end_dot_bends_the_note_and_undoes_in_one_step` |
| Drag a note's front dot down and it slides up into the note; it stays the same note | `bends::dragging_the_front_dot_down_slides_up_into_the_note` |
| An end dot dragged sideways changes the note's length; its bend stretches with it | `bends::an_end_dot_dragged_sideways_resizes_the_note` |
| Double-click a note's line to add a bend point; double-click a point to remove it (never the ends) | `bends::double_clicks_add_and_remove_bend_points` |
| With the Bend tool, dragging a note anywhere off its dots (a click on its line too) moves it whole, bend and all | `bends::dragging_a_bent_note_off_its_dots_moves_it_whole` |
| Corner, Smooth or Broken for the selected bend point; a curved point's handles drag to shape the bend | `bends::a_selected_bend_point_turns_smooth_and_its_handle_curves_the_bend` |
| Bent notes draw along their bends in every tool, and the other tools (and box selections) find them there | `bends::the_select_tool_finds_a_bent_note_along_its_line` |
| Each note's name shows at its start (11 px, semibold) where it fits comfortably: rows 17 px or taller and the note wide enough for the whole name | – |

## Audio panel (`audio-clips.md`)

| Behaviour | Test |
|---|---|
| Click a clip selects it; settings column shows it | – |
| BPM typed, Enter applies, the clip keeps showing the same audio | – |
| Follow tempo switch; Gain and Fade sliders (double-click for default) | – |

## Parameters (`core-loop-ui.md`, `signals.md`)

| Behaviour | Test |
|---|---|
| + Add Parameter; + Add Layer menu: Manual drawn or Listen to signal › (never an audio signal from a node the parameter's node is inside) | – (the rule: `signals::tests::a_node_never_listens_to_an_audio_signal_from_a_node_it_is_inside`) |
| + Add Parameter on a plugin lists its parameters in its groups; search and Enter pick one, then its first layer's type (one undo) | `plugin_params::a_plugin_parameter_is_found_by_search_and_gets_a_layer_in_one_undo` |
| A plugin's parameter list: arrows move; Escape or a click outside closes; parameters with layers aren't offered | `plugin_params::the_plugin_parameter_list_works_from_the_keys_and_closes` |
| Learn beside a plugin's + Add Parameter opens its window and adds the next knob turned there, with a drawn layer; clicking again stops | `plugin_params::learn_opens_the_plugin_and_waits_for_a_knob_until_stopped` |
| Double-click empty layer lane: a one-bar flat block | – |
| Drag a block to move; edges resize as a window; top of the right edge loops | – |
| A click on empty lane on Parameters or Signals clears the selected block; the node stays (the page shows it) | `arrange::clicking_an_empty_layer_lane_clears_the_selected_block` |
| Block edges grab from their outline and up to 4 px past them (Signals lanes too) | `parameters::dragging_from_just_past_a_blocks_end_resizes_it` |
| A looped block's right end only drags repeats (never shorter than one cycle); its first cycle's end still sets the cycle; windows on listening layers still resize | `parameters::a_looped_blocks_end_drags_repeats_never_resizes` |
| The cursor on Parameters and Signals lanes follows the blocks as they are: after a drag, moving away and back; a resize cursor holds through an edge drag, off the lane too | `parameters::after_a_drag_moving_away_and_back_shows_the_right_cursor` |
| Drag a point; double-click adds a point; right-click removes one | – |
| Selecting a block opens the inspector with its settings: depth (0% to 200%, 100% in the middle) and fades on sliders (double-click resets), repeat and nudge dragged sideways, invert a switch; heard as they move, one undo step each (Parameters and Signals) | `parameters::a_selected_blocks_settings_are_in_the_inspector` |
| A window on a listening layer has no repeat setting | `parameters::a_windows_settings_have_no_repeat` |
| Delete removes the selected block | – |
| Layer rows: drag to reorder, rename, blend mode (Add, Subtract, Multiply), mix, show/hide, delete | `parameters::a_layers_blend_mode_is_picked_from_its_menu` (blend mode) |
| Resting-value slider sets the default state | – |
| A listening layer's chip go-to opens the signal | – |

## Signals (`signals.md`, `inspector.md`)

| Behaviour | Test |
|---|---|
| + Add Signal makes a drawn signal; draw on its lane like a layer | – |
| Colour bar picks a colour; double-click renames; × deletes | – |
| Uses chip lists listening layers with go-to | – |
| Click a row selects it; a notes or audio signal's settings show in the inspector | – |
| An audio signal's lane (and every layer listening to it) shows its line, worked out in the background once edits settle; kept beside the song, so a reopened song shows it at once | `audio_signals::an_audio_nodes_signal_draws_its_loudness_once_worked_out` |

## Shape panel (`curve-editor.md`)

| Behaviour | Test |
|---|---|
| Click a notes signal opens its shape in the bottom panel; × closes it; a drawn signal (or nothing) selected closes it | `shape::clicking_a_notes_signal_opens_its_shape_and_a_drawn_one_closes_it` |
| Double-click empty space adds a point (snapped to the grid and eighths, Alt flips); double-click a point removes it; never the two ends | `shape::double_clicks_add_and_remove_points_on_the_grid` |
| Drag a point to move it (between its neighbours; the ends only up and down); heard as it moves, one undo step | `shape::dragging_a_point_moves_it_heard_as_it_moves_and_undoes_in_one_step` |
| Corner, Smooth or Broken for the selected point; a selected curved point's handles drag to bend (Smooth keeps them in line) | `shape::a_selected_point_turns_smooth_or_broken_and_its_handles_bend_the_curve` |
| Trigger or Loop; the window (1/8 to 4 bars); the grid on its own snap chip | `shape::the_toolbar_sets_the_mode_and_the_window` |
| A stamp card fills the whole window with its shape (one copy, whatever the grid); Save as stamp keeps the curve for every project; a saved stamp's × deletes it, double-click its name renames it; cards that don't fit go under › | `shape::stamps_fill_the_window_and_saved_ones_can_be_deleted` |

## Inspector (`inspector.md`)

| Behaviour | Test |
|---|---|
| Tray button opens and closes it; lights when the selection has settings | – |
| Drag the left edge resizes it | – |
| Notes signal: pitch range and velocity clamp switches (its shape is in the Signals page's bottom panel) | – |
| Audio signal: Attack, Release, Floor and Ceiling on sliders (double-click resets; the floor stays a decibel under the ceiling), the line redrawn as they move, one undo step each; a Level bar shows the signal at the song position while playing | `audio_signals::an_audio_signals_follower_is_set_in_the_inspector` |
| A selected node on Arrange shows its mixer strip | – |

## Mixer (`mixer.md`)

| Behaviour | Test |
|---|---|
| Fader and pan follow the pointer and are heard while dragged; each drag is one undo step | `live_drags::a_fader_follows_the_pointer_and_is_heard_while_dragged`, `live_drags::a_pan_follows_the_pointer_and_is_heard_while_dragged` |
| Fader and pan double-click resets; mute and solo | – |
| Each strip's fader has dB marks beside it (12 to -60, 0 dB brighter, thinned on short faders); fader, meters and marks share one scale, -60 to +12 dB, meters red from 0 dB (as Studio One, Live, Logic) | `live_drags::strips_show_db_marks_beside_the_fader` |
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
| Shift+Space plays from the loop's start (kept while looping is off; the song's start without a loop), from stopped or while playing | `transport::shift_space_plays_from_the_loop_start` |
| Ctrl+wheel zooms rows and Ctrl+Shift+wheel zooms time on every timeline (Arrange, Parameters, Signals, piano roll, audio panel), about the pointer; a plain wheel still scrolls | `zoom::ctrl_wheel_zooms_rows_and_ctrl_shift_wheel_zooms_time_everywhere` |
| The metronome button beside the transport switches the click on and off (lit when on); kept for every song (`metronome.md`) | `metronome::the_metronome_button_switches_the_click`, `metronome::the_transport_switch_is_kept_in_the_preferences` |
| Hovering the volume button opens a vertical slider for the click's level; it stays while over the button or the slider, or while held; the click follows the drag | `metronome::hovering_the_volume_button_opens_its_slider` |
| Preferences → Metronome: sound (picking one plays it), accent on beat 1, pitch and volume; kept in the preferences and heard at once | `metronome::the_metronome_page_sets_the_click_and_keeps_it` |
| Every drag that changes the song (sliders, blocks, notes and velocities, clips, notes-signal and audio-clip settings) is heard as it moves and lands as one undo step; every slider's thumb stays under the pointer (Shift: five times finer) | `live_drags::*` |
| Loop strip on every ruler (`loop-strip.md`): drag draws a fresh loop, snapped to the page's grid (Slide: unsnapped), and switches looping on | `loop_strip::dragging_on_the_loop_strip_draws_a_loop_that_plays`, `loop_strip::sliding_leaves_the_loop_off_the_grid` |
| Any drag on the strip but a handle draws a fresh loop, inside the loop too | `loop_strip::a_drag_inside_the_loop_draws_a_fresh_one` |
| Drag a loop's start or end handle to move that end | `loop_strip::dragging_a_handle_moves_only_that_end` |
| The piano roll's strip snaps to the roll's own grid | `loop_strip::the_piano_roll_strip_uses_the_rolls_grid` |
| Click a ruler below its loop strip to place the playhead, to the nearest quarter beat (Arrange, Parameters, Signals, piano roll, audio panel: one shared handler, `locate_on_ruler`) | `rulers::clicking_arranges_ruler_places_the_playhead`, `rulers::clicking_the_piano_rolls_ruler_places_the_playhead`, `rulers::clicking_the_audio_panels_ruler_places_the_playhead` |
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
