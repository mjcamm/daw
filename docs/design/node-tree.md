# Node Tree (Phase 5 design)

*Status: agreed with the user 2026-10-16. **Engine built** (everything above "UI"): tree, edits, audio, meters, file version 7; the UI shows each node as a flat row and the selected node on Parameters. The UI steps are not started. UI layout for the mixer: `mixer.md`. Sketches: https://claude.ai/artifact/4xzV4YFut9r4ovpByU7qFT*

## Model

- The project is a **tree of nodes** under one root, the **Song** (the master output, `NodeId::ROOT`). There is one node type: every node has an effect chain, volume and pan, its own clips and its own children, in any combination.
- **Instruments are optional** (the user's call): a node that plays notes has one (the Synth); a folder has none, so the Parameters page shows only its effects and channel. The Song starts without one.
- **Audio flows up:** a node's output is its children's summed outputs plus its instrument's sound, through its effects, then its volume and pan, into its parent. The Song's output is the master.
- **Pan law:** a node with an instrument (a mono source) pans with equal power, as before; a node without one (a stereo bus) uses balance, so a centred folder passes its input unchanged. Nesting never costs level.
- **Mute** silences a node's output, so muting a folder silences everything under it. **Solo:** while any node is soloed, an instrument sounds only if its node or an ancestor is soloed; everything still passes through its folders' chains to the Song.
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

`Snapshot.nodes` lists every node in tree order (parent before children), each with its id, parent, depth, name, colour, mute and solo, clips and devices. The UI decides display order (`mixer.md`: folders after their children). Meters are per node (`Engine::take_meters`).

## Audio

- The control side compiles the tree into a flat list in **render order** (children before their parent), each entry knowing its parent's slot. The audio side keeps one player per node (instrument voices, effect memory, gain smoothing), matched by node id across state swaps, so editing never resets sound. Players come from a pool made up front (`MAX_NODES`), so adding a node never allocates on the audio thread.
- One transport for all nodes. Each chunk is split at control-rate boundaries and every node's note events; in each span every node renders in render order, adding its output into its parent's bus. Live and offline still share `AudioProcessor::process`; output stays independent of block size.

## File

Version 7 stores the tree: `root` holds the Song, each node lists its `children`. Older files load with their single node as the Song's first child; the Song is a plain bus at 0 dB, centre, so old songs sound bit-for-bit the same.

## UI (steps after the engine)

1. **Arrange:** flat, aligned node rows; folders **below** their children. The **nesting map** runs down the left edge, one column per level (innermost nearest the rows, the Song at the far left), each bar beside a folder's children with an arrow down into the folder's row. Its chevron folds the folder (shared with the mixer). Up to **four** columns show, then the map scrolls sideways; its width comes from the timeline, not the names.
2. **Adding and moving:** "+ Add node", "Group into folder", "Duplicate node" (copies clips), dragging a row onto a folder's bar to nest it, dragging clips between nodes.
3. **Parameters** shows the selected node.
4. **Mixer** (`mixer.md`) and the inspector's channel strip.
5. A richer demo song: drums (with a separate kick) and music folders.
