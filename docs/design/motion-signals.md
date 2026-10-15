# Motion Signals (Phase 4 design)

*Status: revised 2026-10-15 with the user (layer-level listening, centred signals, ADSR note hits, Signals page layout). The engine built earlier (`crates/daw-engine/tests/signals.rs`) implements the first draft and is being changed to match; see "Engine changes" at the end. UI not started.*

## Kinds and sources

A signal is published so layers anywhere can listen to it. There are two kinds, by what a signal carries:

| Kind | Source | When |
|---|---|---|
| **Motion** | Drawn on the Signals page | Phase 4 |
| | A node's **notes**: each note is a hit, shaped by ADSR | Phase 4 |
| | A node's **sound**: follows its loudness (smoothing, gates, attack etc.) | Phase 6 |
| **Audio** | A node's **sound** | Phase 6 |

"MIDI to audio" is not its own case: a MIDI node's sound is its instrument's output, the same as any node's sound.

**One way to do each thing:** drawn signals are made on the Signals page; node signals are made only from the node on the Arrange page ("Publish as signal").

## Motion signals are centred

A motion signal lane looks and works like a layer lane: a centre line, top +1, bottom -1. At every song position a signal gives:

- **value** `s` in `-1..=+1`.
- **presence** `w` in `0..=1`: whether the signal is saying anything here.

Signals always run in song time. Everything listening reads the same song position, so every use stays in sync and nothing ever restarts a signal.

### Drawn

A lane of blocks with the same `Block` as a layer lane and the same gestures (double-click to add, drag, window resize, loop handle, points). A gap means no effect (`w = 0`), like a gap in a layer.

### From notes

Each note matching the pitch filter is a **hit** from 0 to +1, shaped by an envelope:

- **ADSR:** attack up to the peak, decay to the sustain level, held while the note is held, release back to 0 after the note ends. A short note snaps back; a long one holds.
- **Velocity amount** (0 to 100 %): how much velocity scales the peak. At 0 every hit reaches +1.
- **Between hits** the signal rests at 0 (the centre line: no effect). It is always present.
- **A new note restarts from wherever the line is**, so it never jumps.
- **Pitch filter:** a pitch range, so one drum note (the kick) can drive the signal.
- **Source node:** the node that published it. Until the node tree (Phase 5) this is the project's only node.

The line is computed and drawn, not edited by hand; settings live in the signal's bottom bar. Known in advance from the notes, so identical live and offline.

**Ducking:** a notes signal on a Multiply layer with the block inverted: each kick pulls down (Multiply at the bottom is silence), depth sets how hard.

## Layers listen to signals

The choice is made **per layer**, not per block. A layer is either:

- **Manual drawn:** blocks draw their own shapes (as in Phase 3), or
- **Listen to signal:** the layer names one signal.

On a listening layer, **blocks are windows**: the signal plays wherever a block sits, read at song position (plus the block's nudge); gaps let nothing through. Each block draws the signal's shape in the signal's colour. Depth, invert and fades still apply per block, so a duck can fade in over a build. The block's weight is `depth · fade · mix · signal presence`, and its shape value is the signal's value (internally `x = (s + 1) / 2`, inverted if the block is).

- **No range setting.** Depth scales and invert flips; nothing else is needed.
- **Switching** between Manual drawn and Listen to signal (or to another signal) keeps the blocks: drawn blocks become windows and back. Each block keeps its own shape, so switching back brings the drawing back.
- **Blocks inside a signal cannot listen to signals.** No chains, no cycles.
- **Removing a signal that layers listen to is allowed:** those layers go back to Manual drawn, keeping their blocks. Undo restores everything.

## Signals page layout

Same timeline frame as Arrange and Parameters (`core-loop-ui.md`).

- **Left column, one row per signal:** colour swatch, name, "N uses". A published signal also shows muted "from Synth" with a go-to icon to that node on Arrange.
- **"N uses"** opens a list (Parameter › Layer), each entry with a go-to icon to that layer on the Parameters page.
- **"+ Add Signal"** at the bottom of the list makes a drawn motion signal straight away. Below it, hint text: *"Nodes can publish their notes as signals from the Arrange page."*
- **Drawn signals** are edited exactly like layer lanes.
- **Notes signals** draw the node's notes as faint ticks with the resulting line over them; selecting one opens a bottom bar: pitch filter, attack, decay, sustain, release, velocity amount.
- Rows zoom like the other pages.

## Elsewhere in the UI

- **Arrange, node row:** a signal icon button. It opens a menu: "Publish notes as motion signal" (when the node has notes), "Publish sound as motion signal" and "Publish sound as audio signal" (greyed, "coming later" until Phase 6), then the node's existing signals, each with a go-to icon. A node can publish several signals (kick and hat from one drum node). A new notes signal starts with all notes and a short, snappy envelope.
- **Parameters, layer row:** a pick list "Manual drawn" / "Listen to signal". Choosing Listen to signal turns it into a list of signals. A listening layer shows the signal's name and colour and a **go-to icon** to the signal on the Signals page.

## Edits

| Edit | Inverse |
|---|---|
| `AddSignal { id, signal }` | `RemoveSignal` |
| `RemoveSignal { id }`: listening layers switch to Manual drawn | `AddSignal` plus restoring those layers |
| `RenameSignal { id, name }` | itself with the old name |
| `SetSignalColor { id, color }` | itself with the old colour |
| `SetNotesSource { id, source }`: notes signals only | itself with the old source |
| `AddSignalBlock { signal, id, block }`: drawn signals only | `RemoveBlock` |
| `SetLayerSource { param, layer, source }`: Manual drawn or a signal | itself with the old source |

Signal blocks share the project-wide block id space, so `SetBlock` and `RemoveBlock` work on them unchanged. Ids are chosen by the sender.

## Snapshot: "Used by"

`Snapshot.signals` lists every signal in id order with its name, colour, source and **used by**: every listening layer as (parameter, layer).

## Evaluation

- `daw-automation` has `CompiledSignal` with `sample(beat) -> (value, presence)`.
- The control side compiles signals first; each listening layer carries its own copy of its compiled signal. The audio side stays pure functions of song time, allocation-free, so live and offline match and block size does not matter.

## Engine changes from the first draft

1. Signal values centred (`-1..=+1`) instead of `0..=1`.
2. Listening moves from `Block.link` (with `low`/`high`) to the layer (`Layer.source`); range removed.
3. Notes envelope: points over a fixed length become ADSR using note length, velocity amount, rest at 0 and always present, retrigger from the current value.
4. `RemoveSignal` allowed while in use (listening layers fall back to Manual drawn).
5. File version 4; version 2 and 3 files with links convert on load.

## Done when (from `build-plan.md`)

A MIDI kick pattern publishes a ducking signal that is listened to by several parameters and stays in sync everywhere, all done in the UI.
