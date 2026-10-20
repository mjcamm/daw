# Core-Loop UI (Phase 3 design)

Decisions behind the Parameters page and the shared timeline frame. Gesture logic is pure and tested in `crates/daw-ui/src/block_edit.rs`; the page (`parameters.rs`) turns pointer positions into calls there.

## Block gestures (layer lanes)

| Gesture | Result |
|---|---|
| Double-click empty lane | New block: one bar, flat on the centre line, starting on the grid, shortened to fit before the next block |
| Click a block | Select: accent outline, points shown, settings bar at the bottom |
| Drag the body | Move, snapped, never overlapping neighbours |
| Drag the left edge, or the right edge below its top quarter | Resize as a **window**: contents stay put in song time (below) |
| Drag the top quarter of the right edge, or the loop icon beside it | Loop: adds or removes repeats only. Dragging back stops at the block itself; with every repeat gone it is a plain block again |
| Drag a looped block's first cycle end | Resize the block that repeats (as a window); the repeats follow, keeping their count |
| Drag a point | Move it between its neighbours (points do not snap) |
| Double-click in the selected block / right-click a point | Add / remove a point (a shape keeps at least one) |
| Delete or Backspace | Remove the selected block |

- **Edges are a window, not a stretch** (the user's call). On a single shape, points are redrawn across the new span so each keeps its song position: widening continues the edge value and moves an edge point out with the edge; narrowing cuts the shape, leaving a point at the cut. On a repeating shape the cycle keeps its length and the nudge absorbs the edge's movement. Fades keep their length.
- **Loop vs resize are separate zones** on the right edge: a band along the top quarter (at least 6 px, `loop_band`) loops, the rest resizes.
- **A point on an edge is ambiguous**: the first few pixels decide (sideways drags the edge, up or down drags the point).
- **Repeats** of a looped block draw softer than the first cycle, with a line at each cycle's end, so it reads block, block, block.
- **Custom cursors** (loop, four-way move): GPUI has no such cursors on Windows, so each frame the lane hit-tests the pointer with the same `hit_test` clicks use, hides the system cursor and draws the icon at the pointer. The cursor always matches what a press would do.

## Snap

Header control (magnet): Snap or Slide, grid 1 bar to 1/64, default 1/4. Alt during a drag flips it. Snap also sets the step for time values in the settings bar. A UI preference, not saved in the project.

## Lanes and rows

- Every layer lane has a faint **centre line** (no effect). Rows are split by a darker 2 px line across the name column and lanes, so splits and centre lines are easy to tell apart.
- In a block, the bright line is the shape as drawn; a faint second line is what the layer actually does, with depth, fades and mix applied.
- The **result row** shows the final value, clamped once at the end, so it often runs flat at the floor or ceiling and can look unlike any one layer. A faint line marks the resting value: where every layer's centre line lands.

## Layer rows

Two lines (asked for by the user so signal names have room): the name on the first, with show/hide and delete at the right; the type chip (Drawn, or the signal with a go-to icon) and the blend mode on the second. The mix slider stands between them and the buttons, across both lines. The Parameters page opens at 56 px rows.

## Sliders

One design everywhere (`slider.rs`): hairline track, fill, round thumb, relative drags (200 px full range, Shift five times finer).

- **Resting value** (result row, beside the name): the parameter's default state, `SetParam`. Never records.
- **Layer mix** (layer row, with %): how much of the layer reaches the result. Double-click for 100 %.
- **Zoom** (frame, below).

## Timeline frame (Arrange, Parameters, Signals, and the bottom panel)

```text
[ body                                  ][v-scroll]
[                                       ][v-zoom  ]
[ (left column) ][ h-scroll   ][ h-zoom ][corner  ]
```

- `frame.rs` builds it; `scroll.rs` draws the scrollbars (GPUI has none); `zoom.rs` holds zoom maths.
- Zoom is wide and logarithmic: rows 44 to 400 px (always two lines of text), 2 to 400 px per beat. Horizontal zoom holds the playhead still when it is on screen, else the middle of the view. Each page keeps its own zoom and scroll; neither is saved.
- Wheel scrolls up and down only, Shift+wheel sideways: nested scroll areas are axis-locked (see `docs/gpui-notes.md`).

## Undo

Drags never touch the engine while in progress: the page previews the change and sends one edit on release, so each gesture is one undo step (`History` has no merging).

## Not done yet

Curved and held segments (engine supports them), point snapping, saving snap and zoom.
