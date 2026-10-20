# Node Tree (Phase 5 design)

*Status: agreed with the user 2026-10-16. **Built**: the engine (tree, edits, audio, meters, file version 7), Arrange (`crates/daw-ui/src/tree.rs`, `arrange.rs`), clips between nodes, the mixer (`mixer.md`) and the inspector's channel strip. Later changes: node kinds and parent-first order (below), notes flowing up and combined blocks on parent lanes (`note-flow.md`). Still to do: a richer demo song. Sketches: https://claude.ai/artifact/4xzV4YFut9r4ovpByU7qFT*

## Model

- The project is a **tree of nodes** under one root, the **Song** (the master output, `NodeId::ROOT`). There is one node type: every node has an effect chain, volume and pan, its own clips and its own children, in any combination.
- **Every node is MIDI or Audio** (`NodeKind`, the user's call). A branch never mixes kinds: a node goes only under a parent of its own kind; the Song takes either and holds no clips. MIDI clips go on MIDI nodes, audio clips on audio nodes (the engine refuses others: `NotMidi`, `NotAudio`).
- **Instruments are optional** (the user's call): only MIDI nodes have them, as a list (`Node.instruments`, layered); a parent often has none, so the Parameters page shows only its effects and channel. The Song has none.
- **Audio flows up:** a node's output is its children's summed outputs plus its instruments' sound (and its audio clips), through its effects, then its volume and pan, into its parent. The Song's output is the master.
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

`Snapshot.nodes` lists every node in tree order (parent before children), each with its id, parent, depth, name, colour, mute and solo, clips and devices. The UI shows the same order: each parent before its children, the Song first. Meters are per node (`Engine::take_meters`).

## Audio

- The control side compiles the tree into a flat list in **render order** (children before their parent), each entry knowing its parent's slot. The audio side keeps one player per node (instrument voices, effect memory, gain smoothing), matched by node id across state swaps, so editing never resets sound. Players come from a pool made up front (`MAX_NODES`), so adding a node never allocates on the audio thread.
- One transport for all nodes. Each chunk is split at control-rate boundaries and every node's note events; in each span every node renders in render order, adding its output into its parent's bus. Live and offline still share `AudioProcessor::process`; output stays independent of block size.

## File

Version 7 stores the tree: `root` holds the Song, each node lists its `children`. Older files load with their single node as the Song's first child; the Song is a plain bus at 0 dB, centre, so old songs sound bit-for-bit the same.

## Arrange as built

- **Rows** in display order (parent first, the user's call reversing the first build): the Song at the top ("Master"; no signal button, no rename, no drag), then each parent above its children. Subtitles read "MIDI · Synth" (its instruments), "MIDI", "Audio" or "Master".
- **Toolbar** in the NODES heading, acting on the selected node: **+** (menu: "MIDI node" or "Audio node"; goes just after the selected node under its parent if that parent is the same kind, else at the end of the Song, and is selected), **group into a new node** (one undo step via `Edit::Batch`; the new parent, named "Group", takes the node's kind and colour), **duplicate** (the whole subtree with fresh ids, clips and layers included, named "X 2", placed just after), **delete** (also the Delete key). Greyed when nothing (or the Song) is selected. New nodes take the first palette colour no node uses.
- **Nesting map** left of the rows, 18 px per column, top-level parents in column 0. The Song has no bar (user, 2026-10-18: it covered every row and said nothing); with no parents there is no map. A bar starts at the top of its parent's row. Up to four columns show in full; past that the map shows four and a half, so the half column says there is more (user, 2026-10-18). While a node is dragged the map widens to show every column plus one, so every drop slot can be reached (deep slots were hidden past the half column); each slot sits where the new parent's bar will appear. Press anywhere on the map but a chevron and pull sideways to scroll it (the mixer's map pulls up and down); chevrons only fold (user, 2026-10-18). The pull is `FrameDrag::Pan` in `frame.rs`. A plain wheel over the map scrolls it sideways while it has more columns than fit (otherwise the wheel scrolls the rows as before; user, 2026-10-18). A parent's bar runs from its own row down beside its visible children, with a chevron at the top (fold, shared with the mixer). A folded or empty parent shows a short pill on its own row. GPUI cannot rotate text, so bars carry no names; hovering a bar names it.
- **Moving:** drag a row onto another row to put it just after that node in its parent (onto the Song: last in the Song); drag onto a bar, or the drop slot each row offers in the map while dragging, to put it last in that parent. A node only goes under a parent of its own kind. Pure logic and tests in `tree.rs` (`move_edit`).
- **Motion** (`crates/daw-ui/src/motion.rs`, asked for by the user): every change to the rows animates over 200 ms, ease-out. New rows grow open; removed or folded-away rows close up as ghosts, their content anchored at the top edge, so folded children slide back up under their parent and unfolded ones slide out from under it; rows whose order changes glide. The map's bars follow the drawn rows, shrinking into a parent's pill as it folds. Opening the page does not animate.
- **Clips** (`clip_edit.rs` rules, `clip_drag.rs` interaction; agreed with the user as standard DAW behaviour):
  - Click selects (accent outline), Shift+click adds or removes, a press on empty lane clears; Delete removes the selected clips (else the selected node).
  - Dragging the body moves the selection in time and between rows, snapped on the grabbed clip (Alt flips), others keep their offsets; nothing goes before the song start. The edges (6 px) resize every selected clip alike; notes keep their song positions, and a clip is never shorter than a sixteenth.
  - Preview: originals stay faint, solid-backed ghosts show where clips land, receiving rows tint, a pill shows the bar and beat ("Copy · …", "New node · …"). Rows of the other kind (and the Song) refuse: red ghost, "Can't go here", and the drop does nothing (the engine also refuses: `NotMidi`, `NotAudio`).
  - Ctrl (Cmd on macOS) copies; Escape cancels; the timeline scrolls near its edges.
  - A dropped or resized clip wins: what it covers on its node is trimmed, split or removed. Dropping clips from one row below the last row makes a new node of their kind for them. Each gesture is one undo step (`Edit::Batch` of `SetClip`, `AddClip`, `RemoveClip`), and the placed clips become the selection.
- **Parent lanes show combined blocks** of the clips below them (`note-flow.md`). A merged audio waveform for parents may come with Phase 6's audio analysis.

## UI steps

1. Arrange rows and the nesting map (built; first with parents below, then reversed to parent first).
2. Adding, grouping, duplicating, moving and nesting nodes; dragging clips between nodes (built).
3. Parameters shows the selected node (built).
4. Mixer (`mixer.md`) and the inspector's channel strip (built).
5. **To do:** a richer demo song: drums (with a separate kick) and music parents.
