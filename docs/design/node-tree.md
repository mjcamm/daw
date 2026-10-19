# Node Tree (Phase 5 design)

*Status: agreed with the user 2026-10-16. **Engine built** (everything above "UI"): tree, edits, audio, meters, file version 7; **Arrange tree built** (UI steps 1 and 2, `crates/daw-ui/src/tree.rs`, `arrange.rs`). Still to do: dragging clips between nodes, the mixer, the inspector's channel strip, a richer demo song. UI layout for the mixer: `mixer.md`. Sketches: https://claude.ai/artifact/4xzV4YFut9r4ovpByU7qFT*

## Model

- The project is a **tree of nodes** under one root, the **Song** (the master output, `NodeId::ROOT`). There is one node type: every node has an effect chain, volume and pan, its own clips and its own children, in any combination.
- **Instruments are optional** (the user's call): a node that plays notes has one (the Synth); a parent has none, so the Parameters page shows only its effects and channel. The Song starts without one.
- **Audio flows up:** a node's output is its children's summed outputs plus its instrument's sound, through its effects, then its volume and pan, into its parent. The Song's output is the master.
- **Pan law:** a node with an instrument (a mono source) pans with equal power, as before; a node without one (a stereo bus) uses balance, so a centred parent passes its input unchanged. Nesting never costs level.
- **Mute** silences a node's output, so muting a parent silences everything under it. **Solo:** while any node is soloed, an instrument sounds only if its node or an ancestor is soloed; everything still passes through its parents' chains to the Song.
- **Ids are project-wide:** nodes, clips, effects, layers and blocks each have one id space across the whole tree, so edits name things by id alone. A parameter is named by node, device and index (`ParamId.node`).
- **Notes signals name their source node** (`NotesSource.node`). Removing that node leaves the signal silent ("source removed"); undo restores both.

## Edits

| Edit | Inverse |
|---|---|
| `AddNode { id, parent, index, node }` (a whole subtree, ids checked unused) | `RemoveNode` |
| `RemoveNode { id }` (its subtree goes too; never the Song) | `AddNode` with the subtree |
| `MoveNode { id, parent, index }` (not into its own subtree) | `MoveNode` back |
| `RenameNode`, `SetMute`, `SetSolo`, `SetNodeColor` (each with `node`) | themselves with the old value |
| `AddClip { node, id, clip }`, `AddEffect { node, ... }` | `RemoveClip`, `RemoveEffect` |
| Layer, block and signal edits | unchanged (found by id anywhere in the tree) |

## Snapshot

`Snapshot.nodes` lists every node in tree order (parent before children), each with its id, parent, depth, name, colour, mute and solo, clips and devices. The UI decides display order (`mixer.md`: parents after their children). Meters are per node (`Engine::take_meters`).

## Audio

- The control side compiles the tree into a flat list in **render order** (children before their parent), each entry knowing its parent's slot. The audio side keeps one player per node (instrument voices, effect memory, gain smoothing), matched by node id across state swaps, so editing never resets sound. Players come from a pool made up front (`MAX_NODES`), so adding a node never allocates on the audio thread.
- One transport for all nodes. Each chunk is split at control-rate boundaries and every node's note events; in each span every node renders in render order, adding its output into its parent's bus. Live and offline still share `AudioProcessor::process`; output stays independent of block size.

## File

Version 7 stores the tree: `root` holds the Song, each node lists its `children`. Older files load with their single node as the Song's first child; the Song is a plain bus at 0 dB, centre, so old songs sound bit-for-bit the same.

## Arrange as built

- **Rows** in display order: each parent after its children, the Song last ("Master"; no signal button, no rename, no drag). Subtitles read "MIDI · Synth" (its instruments), "MIDI", "Audio" or "Master".
- **Toolbar** in the NODES heading, acting on the selected node: **+** (menu: "MIDI node" or "Audio node"; goes just after the selected node under its parent if that parent is the same kind, else at the end of the Song, and is selected), **group into a new node** (one undo step via `Edit::Batch`; the new parent, named "Group", takes the node's kind and colour), **duplicate** (the whole subtree with fresh ids, clips and layers included, named "X 2", placed just after), **delete** (also the Delete key). Greyed when nothing (or the Song) is selected. New nodes take the first palette colour no node uses.
- **Nesting map** left of the rows, 18 px per column, top-level parents in column 0. The Song has no bar (user, 2026-10-18: it covered every row and said nothing); with no parents there is no map. A bar starts at the top of its parent's row. Up to four columns show in full; past that the map shows four and a half, so the half column says there is more (user, 2026-10-18). While a node is dragged the map widens to show every column plus one, so every drop slot can be reached (deep slots were hidden past the half column); each slot sits where the new parent's bar will appear. Press anywhere on the map but a chevron and pull sideways to scroll it (the mixer's map pulls up and down); chevrons only fold (user, 2026-10-18). The pull is `FrameDrag::Pan` in `frame.rs`. A plain wheel over the map scrolls it sideways while it has more columns than fit (otherwise the wheel scrolls the rows as before; user, 2026-10-18). A parent's bar runs beside its visible children with a chevron at the top (fold, UI state shared later with the mixer), and an arrow (Lucide `corner-down-right`) points into the parent's row. A folded or empty parent shows a short pill on its own row. GPUI cannot rotate text, so bars carry no names; hovering a bar names it. Up to 4 columns show, then the map scrolls sideways (Shift+wheel).
- **Moving:** drag a row onto another row to put it just after that node in its parent (onto the Song: last in the Song); drag onto a bar to put it last in that parent. Pure logic and tests in `tree.rs` (`move_edit`).
- **Motion** (`crates/daw-ui/src/motion.rs`, asked for by the user): every change to the rows animates over 200 ms, ease-out. New rows grow open; removed or folded-away rows close up as ghosts anchored at their bottom edge, so folded children slide down into their parent and unfolded ones rise out of it; rows whose order changes glide. The map's bars follow the drawn rows, shrinking into a parent's pill as it folds. Opening the page does not animate.
- **Clips** (`clip_edit.rs` rules, `clip_drag.rs` interaction; agreed with the user as standard DAW behaviour):
  - Click selects (accent outline), Shift+click adds or removes, a press on empty lane clears; Delete removes the selected clips (else the selected node).
  - Dragging the body moves the selection in time and between rows, snapped on the grabbed clip (Alt flips), others keep their offsets; nothing goes before the song start. The edges (6 px) resize every selected clip alike; notes keep their song positions, and a clip is never shorter than a sixteenth.
  - Preview: originals stay faint, solid-backed ghosts show where clips land, receiving rows tint, a pill shows the bar and beat ("Copy · …", "New node · …"). Rows without an instrument refuse: red ghost, "Can't go here", and the drop does nothing (the engine also refuses: `NoInstrument`).
  - Ctrl (Cmd on macOS) copies; Escape cancels; the timeline scrolls near its edges.
  - A dropped or resized clip wins: what it covers on its node is trimmed, split or removed. Dropping clips from one row below the last row makes a new instrument node for them. Each gesture is one undo step (`Edit::Batch` of `SetClip`, `AddClip`, `RemoveClip`), and the placed clips become the selection.
- **Parent lanes stay blank** (the user's call): the final design is the parent's merged audio waveform with its children's MIDI overlaid, which needs Phase 6's audio analysis. No placeholder until then.

## UI (steps after the engine)

1. **Arrange:** flat, aligned node rows; parents **below** their children. The **nesting map** runs down the left edge, one column per level (innermost nearest the rows, the Song at the far left), each bar beside a parent's children with an arrow down into the parent's row. Its chevron folds the parent (shared with the mixer). Up to **four** columns show, then the map scrolls sideways; its width comes from the timeline, not the names.
2. **Adding and moving:** "+ Add node", "Group into a new node", "Duplicate node" (copies clips), dragging a row onto a parent's bar to nest it, dragging clips between nodes.
3. **Parameters** shows the selected node.
4. **Mixer** (`mixer.md`) and the inspector's channel strip.
5. A richer demo song: drums (with a separate kick) and music parents.
