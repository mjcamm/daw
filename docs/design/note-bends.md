# Bent Notes

*Status: agreed with the user 2026-10-25. Steps 1 (model and file), 2 (engine and Synth) and 3 (the piano roll) built; step 4 built for plugins with per-note tuning (Serum 2, the user's first; MPE and channel bend next).*

An original idea: rather than drawing pitch bend on a lane under the notes, the notes themselves bend. A note in the piano roll is a ribbon that follows its pitch over time, shaped with the same pen-style points and handles as the curve editor (`curve-editor.md`).

## The model

- A note keeps its **pitch**: its home, the note it is. The label, quantize, the notes signal's pitch range and an instrument's played range (`plays`) all read it.
- A note may carry a **bend**: a curve of semitones away from its pitch over the note's length. No bend means straight, as today.
- **Both ends move.** Drag a C's front dot down to an A and the note starts on the A and slides up to the C; drag its end dot up and it bends away. Its pitch stays C either way.
- A new bend is a **straight glide** between the two ends (the user's call); handles curve it.
- The bend **stretches with the note** when it is resized (time `0..=1` across the note) and moves with it when it is transposed or moved.
- **Range** ±48 semitones from the note's pitch, held inside the MIDI range 0..=127 when played.
- **Stored as a `Curve`** (`daw_protocol::Curve`), so the evaluation (`daw_automation::curve_value`), the point kinds and the editing rules (`curve_edit.rs`) are the same code: value `0.5` is the note's pitch, each `1/96` a semitone. `MidiNote` gains `bend: Option<Curve>` and so is no longer `Copy`. File version 24; older notes have no bend.

## The piano roll

| Tool | Key | Does |
|---|---|---|
| Select, Pen, Scissors | 1, 2, 3 | As today (`piano-roll.md`), on the ribbon: bent notes are hit along their curve |
| **Bend** | 4 | Shows each note's dots: one at each end and any points between. Drag a dot freely, like a pen point: up and down bends (snapped to semitones), sideways moves it in time (snapped to the grid); an end dot sideways moves that end of the note, so lengths change here too. Alt frees both. Double-click the line to add a point; double-click a point to remove it (never the ends). Click a point to select it; a selected Smooth or Broken point shows its handles. Drag the note anywhere off a dot (even a click on a thin line) to move it whole, bend and all, in time and pitch |

- **Always drawn bent:** outside Bend mode a bent note is still a ribbon; only the dots and handles belong to the tool.
- **Kinds:** while a bend point is selected, the header shows the same **Corner | Smooth | Broken** toggle as the shape panel.
- **Labels:** each note's name at its start once rows are tall enough; while dragging a dot, its pitch beside it ("F3 · +5").
- **Painted:** ribbons, dots and handles go in the notes canvas (`ui-speed.md`); only what is on screen is painted.
- **Built** (`daw-ui/src/bend_edit.rs`, pure and tested: where a note sits, hits, the edits; `piano_roll.rs`: the tool, drags, drawing). A ribbon is the note's line a row thick (measured up and down, so it shows the pitches it covers at each moment), filled and outlined as one path each. The dots are the shape editor's points (`curve_editor::Shapes`, `point_shape`), the kind toggle its `segments`. A bend dragged back flat on the note's pitch is removed (`tidy`). A dot's pitch is heard as it crosses each semitone. Moving notes now moves them by the rows the pointer travels, so a bent note grabbed off its own row doesn't jump. Box selections find bent notes along their lines. A folded roll draws a semitone as one row, so a bend there reads as rows rather than exact pitches. UI tests `ui_tests/bends.rs`; speed scene `perf_piano_roll_bends` (every note of a full-size song's synth bent, dots and labels on: 5.8 ms a full redraw against 5.0 without, budget 6 / 10 ms).
- **No cutting in Bend mode** (the user's call). Scissors on a bent note in its own tool: later.
- **Later:** bent notes in Arrange's clip drawings (agreed, a later step).

## Playing bends

Bends must work on most instruments, built in, VST3 and later CLAP. The engine speaks one thing, a **per-note tuning** in semitones, and each kind of instrument hears it its own way.

- **Compile:** each scheduled note carries its bend; the curves sit in the immutable `RenderState`, so the audio thread reads and never allocates.
- **Audio thread:** at each control point (`CONTROL_FRAMES`, the song-aligned 32-frame grid) every sounding bent note's tuning is read from song time, so it is the same for any block size, live or offline. A note starts with its start tuning in its note-on (no swoop from the home pitch).
- **Built-in Synth:** each voice ramps its frequency to the new tuning across the 32 frames, so the bend is smooth and still block-size independent.
- **Built** (`audio.rs`): a sounding note remembers its place in the node's notes with its start and pitch (`PendingEnd`), so after an edit swaps the notes it finds itself again and plays its new bend at once (a drag is heard as it moves); a note that loses its bend glides home. Each control point tunes a note from its bend there to its bend at the next point (`tune_notes`); a note starting between points glides to the next one. `InstrumentInstance::tune` is the one door: `Synth::glide` today, plugins in step 4. Tests: `daw-engine/tests/bends.rs` (a steady octave bend is bit-identical to the octave, glides rise, any block size, a bend changed mid-note).
- **VST3**, best first, per plugin:
  1. **Note expression tuning** (`kTuningTypeID`) when the plugin lists it: per note, by note id, no range to match.
  2. **MPE**: each note on its own channel (2–16) with channel pitch bend through the plugin's MIDI mapping, ±48 semitones (the MPE default).
  3. **Channel pitch bend** on one channel: one bend at a time (the latest note's), with a bend range to set on the instrument to match the plugin.
- **CLAP** (later): `CLAP_NOTE_EXPRESSION_TUNING` takes semitones per note id, the same event one for one.
- **Built** (step 4): the engine sends a plugin instrument a **tune** note event (semitones) at each control point where a sounding note's tuning changed (`PluginInstrument::tune`); the host process turns it into what the plugin takes (`BendRoute`, decided at creation from the plugin's controller, `vst3_host.rs`): today `kNoteExpressionValueEvent` tuning (`0.5 + semitones / 240`). Serum 2 takes it: its "Tuning" expression, and a C tuned +12 plays exactly twice the cycles (`installed_plugins.rs`, opt-in). Test Synth lists the same expression; tests: `daw-plugins/tests/vst3_instrument.rs` (a tuned note is the octave, a song's bend at any block size). Plugins hear the bend stepped every 32 frames (0.7 ms), not glided between.
- **MPE** (built 2026-10-25): for a plugin whose MIDI mapping gives each channel its own pitch bend parameter (Serum 1), the host gives each note a member channel (2–16, the one free longest; `Mpe` in `vst3_host.rs`), sends its start and end there, resets that channel's bend at its start and turns its tunings into that bend parameter (`BendChanges`, merged into the block's parameter changes). Unit tests in `vst3_host.rs`. **The bend range is the plugin's own setting** and VST3 can't read it: the host assumes MPE's ±48 (`MPE_BEND_RANGE`), but Serum 1's patches default to ±2 (measured: a full bend plays a whole tone, 1.12×; +12 played as half a semitone). Matching it needs a range setting per plugin instrument: to design with the user.
- **Samplers:** the SingleSampler ignores pitch today, so it ignores bends.

## Steps

1. The model and file (bend on `MidiNote`, version 24, upgrade), tested.
2. The engine and the Synth: tuning events on the control grid, voices following them; tested for block-size independence and against a reference render.
3. The piano roll: ribbons drawn everywhere, the Bend tool, the kind toggle, labels; UI tests and a speed scene.
4. VST3: note expression, then MPE, then channel bend; tried on the user's instruments.
5. Later: Arrange's clip drawings, CLAP.
