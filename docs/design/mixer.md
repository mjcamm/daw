# Mixer

*Status: layout agreed with the user 2026-10-16; **built** (`crates/daw-ui/src/mixer.rs`). Next: an effect's settings in the inspector when its slot is selected; sends and sidechains in Phase 6 (no placeholder until then). Sketches: https://claude.ai/artifact/4xzV4YFut9r4ovpByU7qFT*

## The idea

A traditional mixer: every node is a strip in one flat row (the tree "un-nested"), and a **nesting map** along the bottom shows how they sum. The strips never show nesting themselves.

```text
[ Kick ][ Hats ][ Drums ][ Bass ][ Synth ][ Music ][ Song ]   strips, all aligned
[ ▾ Drums ──────────→ ][ ▾ Music ─────────────→ ]             map, innermost row first
[ ▾ Song ─────────────────────────────────────────────→ ]     ...the Song at the bottom
```

## Strips

- **All aligned, like a desk:** every strip has the same label height, buttons, slots and fader length. Only the faders change length, and all together.
- **Order:** tree order, with each parent's strip **just after its children**, so sound reads left to right into the strip it feeds. The Song (root, the master output) is last.
- **Contents, top to bottom** (from the mixer mockup in `docs/ui-ideas/`): device chain slots, the audio signals it listens to (sends, sidechains) as chips in the signal's colour, pan, fader with meter, mute and solo, colour bar and name. Same on every strip.

## Nesting map

- **One row per nesting level:** innermost parents nearest the strips, the Song at the bottom.
- **A bar per parent:** runs under the parent's children and ends in an arrow pointing sideways into the parent's own strip. It carries the parent's name and colour.
- **Folding happens here:** each bar's chevron folds the parent, leaving only its own strip. One fold state per parent, shared with Arrange.
- **Grows with depth:** each extra level adds a row and the whole desk gives up that height, so every fader shortens equally. Up to **three rows** show; deeper trees scroll the map instead of shrinking the desk further (the user's call).

## Elsewhere

- **Sends and sidechains** are audio signals (Phase 6): no cables. Hovering a signal chip lights the strip that publishes it.
- **Selecting a strip** selects the node everywhere; the inspector (`inspector.md`) shows its device chain in detail, as in the mockup's right panel.
- **Strip width** zooms with a slider, like the timelines.

## As built

- **Fader:** the volume's resting value (the same drag as Parameters' resting-value slider; one edit on release); double-click for 0 dB; the dB readout below. When layers move the volume, an orange marker on the fader shows where it is at the playhead.
- **Pan:** the app's slider, horizontal, centre-detented (within 2 %), double-click to centre.
- **Meters:** a stereo pair beside the fader; a peak line holds for 1.2 s, then falls; red near clipping.
- **Instruments** (MIDI strips only), above the effects: the node's layered instruments (each plays every note; the user asked for layering), then "+ Instrument", which opens the **instrument browser**: a 260 px popover with BUILT-IN (each instrument with a line on what it is; only the Synth so far) and a greyed PLUGINS note (VST3 and CLAP later). Choosing adds a layer and selects it. Slots switch on and off and remove like effect slots. The area is as tall as the busiest MIDI strip's; other strips leave it blank.
- **Effect slots** below them: one row per effect (on/off dot, name), then "+" (menu: Filter). Clicking a slot selects the node and the effect; the selected slot shows ×. Every strip's slot area is as tall as the busiest strip's, so the strips stay aligned. Effects switch off with `Edit::SetEffectEnabled` (off passes sound untouched; files from before load with it on).
- **Map:** bars in the parent's colour, named on hover, an arrow (Lucide `corner-right-up`) up into the parent's strip, chevrons to fold (shared with Arrange). Strips animate like Arrange's rows (`motion.rs`), closing to the right into their parent.
- **Bars** start at the left edge of their parent's strip, as Arrange's start at the top of the parent's row (user, 2026-10-18).
- **Chevrons:** Arrange's turned a quarter for the sideways layout: open points right, at the children; folded points down (user asked, 2026-10-18).
- **Pulling the map:** press anywhere on the nesting map but a chevron and drag up or down to scroll its rows (as Arrange's map pulls sideways).
- **Footer:** the desk's sideways scrollbar and a strip-width slider (60 to 140 px; the Song is 10 px wider).

## Still to decide when building

- What a folded parent's strip shows of its children (mini meters were suggested).
- How the map behaves when the desk is scrolled sideways (it scrolls with the strips).
