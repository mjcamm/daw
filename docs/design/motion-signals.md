# Motion Signals (Phase 4 design)

*Status: revised 2026-10-15 with the user (layer-level listening, centred signals, ADSR note hits, Signals page layout). Engine built and tested to this design (`crates/daw-automation/src/signal.rs`, `crates/daw-engine/tests/signals.rs`). UI not started.*

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
- **The type is chosen when the layer is added and then fixed** (revised by the user; the engine still has `SetLayerSignal`, which the UI does not use). Removing a signal is the one thing that turns its listeners into drawn layers.
- **Blocks inside a signal cannot listen to signals.** No chains, no cycles.
- **Removing a signal that layers listen to is allowed:** those layers go back to Manual drawn, keeping their blocks. Undo restores everything.

## Signals page layout

Built in `crates/daw-ui/src/signals.rs`. Same timeline frame as Arrange and Parameters (`core-loop-ui.md`); its own zoom and scroll.

- **Left column, one row per signal** (as tall as its lane): colour bar, name (double-click to rename) with "Drawn" under it, the uses chip, and an × that deletes at once (undo restores it and its listeners). A published signal shows muted "from Synth" with a go-to icon to that node on Arrange.
- **Colour:** click the colour bar for the nine palette swatches. A new signal takes the first colour no other signal uses.
- **Uses chip:** "Unused" (muted, inert), "1 use", "N uses". Clicking it lists Device › Parameter › Layer, each with a go-to icon: it opens the Parameters page scrolled to that layer, highlighted for a moment.
- **"+ Add Signal"** below the list makes "Signal N", drawn, with an empty lane. Below it, hint text: *"Nodes can publish their notes as signals from the Arrange page."*
- **Drawn signals** are edited exactly like layer lanes: the lane code is shared (`ProjectView::lane_cell`, `Lane::Layer` / `Lane::Signal`), including the bottom settings bar. Block ids come from one pool across layers and signals.
- **Notes signals** draw the line their hits make (read-only). Still to design: faint note ticks, and the bottom bar (pitch filter, attack, decay, sustain, release, velocity amount).
- **"SIGNALS"** heads the column beside the ruler.

## Elsewhere in the UI

- **Arrange, node row:** a signal icon button. It opens a menu: "Publish notes as motion signal" (when the node has notes), "Publish sound as motion signal" and "Publish sound as audio signal" (greyed, "coming later" until Phase 6), then the node's existing signals, each with a go-to icon. A node can publish several signals (kick and hat from one drum node). A new notes signal starts with all notes and a short, snappy envelope.
- **Parameters, adding a layer** (built): "+ Add Layer", and the first layer from "+ Add Parameter", open a menu: **Manual drawn** or **Listen to signal ›**, which swaps the menu to the signals (colour dot and name, "‹ Back"; with none, a go-to row to the Signals page). The menu says "A layer keeps its type once added": **the type is fixed** (the user's call), and a listening layer stays on its signal. New layers are "Layer N".
- **Parameters, layer row** (built): a chip shows the type. Drawn: pencil and "Drawn". Listening: an inbound arrow and the signal's name in its colour, then a **go-to icon** to the signal on the Signals page (highlighted for a moment). Hovering the chip explains it.
- **Windows** (built): blocks on a listening layer draw in the signal's colour and show the signal through them; no points, no loop handle, no Repeat in the settings bar. Result rows include listening layers.

## Edits

| Edit | Inverse |
|---|---|
| `AddSignal { id, signal, listeners }`: a front end sends no listeners | `RemoveSignal` |
| `RemoveSignal { id }`: listening layers go back to Manual drawn | `AddSignal` with those layers as `listeners` |
| `RenameSignal { id, name }` | itself with the old name |
| `SetSignalColor { id, color }` | itself with the old colour |
| `SetNotesSource { id, source }`: notes signals only | itself with the old source |
| `AddSignalBlock { signal, id, block }`: drawn signals only | `RemoveBlock` |
| `SetLayerSignal { id, signal }`: `None` is Manual drawn | itself with the old signal |

- `AddSignal` rejects a listener that already listens to another signal (`AlreadyListening`), so undo never loses a link.
- A layer can only listen to a signal that exists (`UnknownSignal`).
- Signal blocks share the project-wide block id space, so `SetBlock` and `RemoveBlock` work on them unchanged. Ids are chosen by the sender.

## Snapshot: "Used by"

`Snapshot.signals` lists every signal in id order with its name, colour, source and **used by**: every listening layer as (parameter, layer), in device order.

## Evaluation

- Internally a signal's value uses the same space as block shapes: `0.5` is the centre line, `1` the top, `0` the bottom. A notes hit at level `h` is `0.5 + 0.5·h`.
- `daw-automation` has `CompiledSignal` with `sample(beat) -> (value, presence)`. Notes signals pre-compute each hit's starting level at compile time (where the previous hit had got to), so sampling stays a binary search plus arithmetic.
- Notes starting together are one hit: the strongest velocity, held for the longest.
- The control side compiles signals first; each listening layer carries its own copy of its compiled signal (`CompiledLayer.signal`). The audio side stays pure functions of song time, allocation-free, so live and offline match and block size does not matter.

## File

Version 4. Older files convert on load (`crates/daw-engine/src/upgrade.rs`). The first-draft signals (version 2 and 3) never reached the UI, so they convert as closely as the new model allows: a layer whose blocks all link to one signal listens to it (link ranges dropped); other links are dropped; an old notes envelope becomes a hit decaying over its length.

## Done when (from `build-plan.md`)

A MIDI kick pattern publishes a ducking signal that is listened to by several parameters and stays in sync everywhere, all done in the UI.
