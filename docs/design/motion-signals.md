# Motion Signals (Phase 4 design)

*Status: engine built and tested (`crates/daw-engine/tests/signals.rs`); UI not started. Decisions marked **(confirm)** await the user.*

## What a signal is

A motion signal is a shape over song time, published so blocks anywhere can link to it. It has no units or target of its own.

At every song position a signal gives two numbers, like a block does:

- **value** `x` in `0..=1`: where the shape is.
- **presence** `w` in `0..=1`: whether the signal is saying anything here. In a gap it is 0.

A gap in a signal therefore means the same as a gap in a layer: no effect, the parameter falls through to what is below. **(confirm)**

Signals always run in song time. Anything linked reads the signal at the same song position, so every use stays in sync and nothing ever restarts a signal.

## Kinds of signal

### Drawn

A lane of blocks, exactly like a layer lane: the same `Block` (start, length, shape, cycle, depth, invert, nudge, fades), non-overlapping. "Draw one bar, repeat it" is a block with a one-bar cycle stretched across the song (the corner loop handle in the UI).

Signal blocks share the project-wide block id space, so `SetBlock` and `RemoveBlock` work on them unchanged; a new edit `AddSignalBlock` adds one.

### From MIDI notes

Each matching note restarts an **envelope**: a shape (points over `0..=1`) played across a fixed length from the note's start.

- **Filter:** a pitch range, so one drum note (the kick) can drive the signal.
- **After the envelope ends** (or before the first note), the signal is in a gap. A new note cuts the previous envelope short. **(confirm)**
- **Source node:** the project's only node for now. When the node tree arrives (Phase 5), the source gains a node reference; files without one keep meaning "the first node".
- A ducking signal is an envelope rising from 0 to 1, linked into a **Multiply** layer with the link range 0 to 0.5 (the bottom half of the centred lane): fully ducked at the kick, back to the centre line (no change) over the envelope.

Known in advance from the notes, so it is perfectly in sync and identical live and offline.

## Linking a block to a signal

A block either draws its own shape or links to a signal: `Block.link: Option<SignalLink>`.

- `SignalLink { signal, low, high }`. `low` and `high` (both `0..=1`) set where the bottom and top of the signal land: `x = low + (high - low) · signal_x`. `low > high` flips it.
- A linked block reads the signal at **song** position (plus its nudge), not relative to the block start. Moving the block changes where the signal is let through, never what it plays at a given time.
- The block's own controls still apply: depth, invert (after the range mapping), nudge and fades. Its weight is `depth · fade · signal presence`.
- The block keeps its local shape and cycle while linked, so unlinking brings its drawing back.
- **Blocks inside a signal cannot link to signals.** No chains, no cycles.

## Edits

| Edit | Inverse |
|---|---|
| `AddSignal { id, signal }` | `RemoveSignal` |
| `RemoveSignal { id }`: rejected while any block links to it (`SignalInUse`) **(confirm)** | `AddSignal` |
| `RenameSignal { id, name }` | itself with the old name |
| `SetSignalColor { id, color }` | itself with the old colour |
| `SetNotesSource { id, source }`: notes signals only | itself with the old source |
| `AddSignalBlock { signal, id, block }`: drawn signals only | `RemoveBlock` |

`AddBlock`/`SetBlock` reject a link to a signal that does not exist. Ids (`SignalId`) are chosen by the sender, like every other id.

## Snapshot: "Used by"

`Snapshot.signals` lists every signal in id order with its name, colour, source and **used by**: every linked block as (parameter, layer, block). Everything a front end needs for the "Used by" list and for showing a linked block's signal name and colour.

## Evaluation

- `daw-automation` gains `CompiledSignal` (drawn: compiled blocks; notes: sorted trigger beats, envelope length and points) with `sample(beat) -> (x, w)`, and a lookup of compiled signals by id.
- The control side compiles signals first, then each linked block carries its own copy of its compiled signal. The audio side is unchanged: still pure functions of song time, allocation-free, so live and offline output match and block size does not matter.

## File

Version 2 adds a `signals` list (empty when missing, so version 1 files load unchanged). Loading adds signals before parameters, so links resolve.

## Done when (from `build-plan.md`)

A MIDI kick pattern publishes a ducking signal that is linked into several parameters and stays in sync everywhere, all done in the UI. The engine half is proven by tests: kick notes → notes signal → linked into Volume and Cutoff → both duck at every kick, including across a loop jump. The UI half waits for the user.
