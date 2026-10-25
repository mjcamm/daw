# Bent Notes

*Status: agreed with the user 2026-10-25. Step 1 (model and file) built; the rest to come.*

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
- **Painted:** ribbons, dots and handles go in the notes canvas (`ui-speed.md`); hit-testing walks only visible notes.
- **No cutting in Bend mode** (the user's call). Scissors on a bent note in its own tool: later.
- **Later:** bent notes in Arrange's clip drawings (agreed, a later step).

## Playing bends

Bends must work on most instruments, built in, VST3 and later CLAP. The engine speaks one thing, a **per-note tuning** in semitones, and each kind of instrument hears it its own way.

- **Compile:** each scheduled note carries its bend; the curves sit in the immutable `RenderState`, so the audio thread reads and never allocates.
- **Audio thread:** at each control point (`CONTROL_FRAMES`, the song-aligned 32-frame grid) every sounding bent note's tuning is read from song time, so it is the same for any block size, live or offline. A note starts with its start tuning in its note-on (no swoop from the home pitch).
- **Built-in Synth:** each voice ramps its frequency to the new tuning across the 32 frames, so the bend is smooth and still block-size independent.
- **VST3**, best first, per plugin:
  1. **Note expression tuning** (`kTuningTypeID`) when the plugin lists it: per note, by note id, no range to match.
  2. **MPE**: each note on its own channel (2–16) with channel pitch bend through the plugin's MIDI mapping, ±48 semitones (the MPE default).
  3. **Channel pitch bend** on one channel: one bend at a time (the latest note's), with a bend range to set on the instrument to match the plugin.
- **CLAP** (later): `CLAP_NOTE_EXPRESSION_TUNING` takes semitones per note id, the same event one for one.
- **Samplers:** the SingleSampler ignores pitch today, so it ignores bends.

## Steps

1. The model and file (bend on `MidiNote`, version 24, upgrade), tested.
2. The engine and the Synth: tuning events on the control grid, voices following them; tested for block-size independence and against a reference render.
3. The piano roll: ribbons drawn everywhere, the Bend tool, the kind toggle, labels; UI tests and a speed scene.
4. VST3: note expression, then MPE, then channel bend; tried on the user's instruments.
5. Later: Arrange's clip drawings, CLAP.
