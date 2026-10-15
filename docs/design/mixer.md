# Mixer

*Status: layout agreed with the user 2026-10-16; not built. Needs the node tree (Phase 5): the engine has one node today. Sketches: https://claude.ai/artifact/4xzV4YFut9r4ovpByU7qFT*

## The idea

A traditional mixer: every node is a strip in one flat row (the tree "un-nested"), and a **nesting map** along the bottom shows how they sum. The strips never show nesting themselves.

```text
[ Kick ][ Hats ][ Drums ][ Bass ][ Synth ][ Music ][ Song ]   strips, all aligned
[ ▾ Drums ──────────→ ][ ▾ Music ─────────────→ ]             map, innermost row first
[ ▾ Song ─────────────────────────────────────────────→ ]     ...the Song at the bottom
```

## Strips

- **All aligned, like a desk:** every strip has the same label height, buttons, slots and fader length. Only the faders change length, and all together.
- **Order:** tree order, with each folder's strip **just after its children**, so sound reads left to right into the strip it feeds. The Song (root, the master output) is last.
- **Contents, top to bottom** (from the mixer mockup in `docs/ui-ideas/`): device chain slots, the audio signals it listens to (sends, sidechains) as chips in the signal's colour, pan, fader with meter, mute and solo, colour bar and name. Same on every strip.

## Nesting map

- **One row per nesting level:** innermost folders nearest the strips, the Song at the bottom.
- **A bar per folder:** runs under the folder's children and ends in an arrow pointing sideways into the folder's own strip. It carries the folder's name and colour.
- **Folding happens here:** each bar's chevron folds the folder, leaving only its own strip. One fold state per folder, shared with Arrange.
- **Grows with depth:** each extra level adds a row and the whole desk gives up that height, so every fader shortens equally. Up to **three rows** show; deeper trees scroll the map instead of shrinking the desk further (the user's call).

## Elsewhere

- **Sends and sidechains** are audio signals (Phase 6): no cables. Hovering a signal chip lights the strip that publishes it.
- **Selecting a strip** selects the node everywhere; the inspector (`inspector.md`) shows its device chain in detail, as in the mockup's right panel.
- **Strip width** zooms with a slider, like the timelines.

## Still to decide when building

- What a folded folder's strip shows of its children (mini meters were suggested).
- How the map behaves when the desk is scrolled sideways (it scrolls with the strips).
