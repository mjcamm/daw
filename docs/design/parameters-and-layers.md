# Parameters and Layers (Phase 2 design)

*Status: implemented in Phase 2. Decisions here resolve the matching open questions in `daw-foundation.md`.*

## Parameters

- Every automatable value is a **parameter** on a **device**: the node's instrument, an effect in its chain, or the channel itself (volume, pan). Built-in devices and, later, plugins describe their parameters the same way: name, unit, range, scale and default.
- Each parameter has a **normalized value** in `0..=1`. The parameter's **scale** maps it to real units: linear, logarithmic (frequencies, times), decibels (volume, with 0 as silence) or a fixed set of choices.
- Each parameter has a **default state**: the normalized value it rests at when nothing acts on it.

### Decisions

- **Layers work in normalized space.** A wobble drawn on a cutoff moves evenly in octaves, not in hertz, and the same shape means the same thing on any parameter. Plugins (VST3, CLAP) also expose normalized values, so third-party parameters behave identically. Real units are only for display.
- **Out-of-range values are clamped once, after the whole stack.** Intermediate results may leave `0..=1`, so a later layer can pull back what an earlier one pushed past the edge.
- **Moving a knob edits the default state.** It never records. Recording movement will be an explicit, armed mode added later.

## Layers

- A parameter has a **stack of layers**, evaluated bottom to top, starting from the default state.
- Each layer has a **blend mode** and can be switched off.
- Each layer has a **mix** (`0..=1`, default full), like a layer's opacity: it scales the weight of every block on the layer, so it sets how much of the layer reaches the result. Added in Phase 3 at the user's request (`Edit::SetLayerMix`).
- A layer holds **blocks** on the timeline. Blocks in one layer never overlap. Where a layer has no block (a **gap**), it has no effect.

### Blend modes

The block produces a shape value `x` in `0..=1` and a weight `w` (its depth, scaled by its edge fades). `v` is the value from the layers below.

| Mode | Result | Typical use |
|---|---|---|
| Replace | `v + (x - v)·w` | Take over: a drawn curve, a hard setting |
| Add | `v + x·w` | Push up: a riser |
| Subtract | `v - x·w` | Pull down: a dip |
| Multiply | `v · (1 + (x - 1)·w)` | Scale towards zero: ducking |
| Modulate | `v + (2x - 1)·w` | Move around the current value: a wobble |

## Blocks

- A block has a start, a length and a **shape**, drawn locally. Phase 3 adds the other kind of block: one linked to a signal.
- The shape is a list of points across one **cycle**. A block's cycle length can be shorter than the block, in which case the shape repeats; that is how a one-beat wobble fills four bars. By default the cycle is the whole block.
- Each segment between points is linear, held (a step), or curved.
- Per-block controls:
  - **Depth:** 0 to 1.
  - **Invert:** use `1 - x`.
  - **Nudge:** shifts where the shape is read, without moving the block.
  - **Fade in / fade out:** ramp the weight from 0 at the block edges, so blocks never click in or out.

## Evaluation on the audio thread

- Layer evaluation is a set of pure functions (`daw-automation`), tested without the engine.
- The audio side evaluates every parameter at a fixed **control rate**: every 32 frames, counted from the processor's start rather than per host block, so output stays independent of block size. Gains ramp between control points; other parameters step at the control rate.
- Automation applies while stopped too, at the stopped position.
