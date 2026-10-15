# Block Editing (Phase 3 design)

*Status: built, waiting for the user to try it. The interaction logic is pure and unit tested in `crates/daw-ui/src/block_edit.rs`; the Parameters page (`parameters.rs`) turns mouse positions into calls there.*

## Gestures on a layer lane

| Gesture | Result |
|---|---|
| Double-click empty space | New block: one bar, flat line at 0.5, starting on the snap grid, shortened to fit before the next block |
| Click a block | Selects it: accent outline, points shown, settings bar appears |
| Drag the body | Moves the block, snapped, never overlapping its neighbours |
| Drag an edge | Moves that edge **without moving the contents**: the block is a window onto song time (see below) |
| Drag the corner handle (top right, shown on hover) | Repeats the shape: the cycle is fixed at the current length and the block extends |
| Drag a point (selected block) | Moves it, kept between its neighbours; points do not snap |
| Double-click inside the selected block | Adds a point |
| Right-click a point | Removes it (a shape keeps at least one point) |
| Delete / Backspace | Removes the selected block (Parameters page only) |

## Edges are a window, not a stretch

The user asked for edges to leave the contents fixed and give more room to work, not to stretch the shape like a clip.

- **Single shape** (no repeat, no nudge): the points are redrawn across the new span so each keeps its song position. Widening shows the held first/last value. Narrowing cuts the shape, leaving a point at the edge with the value there. Within one drag this is lossless (the preview is always computed from the block as it was when the drag began); across separate edits, cut points are gone.
- **Repeating shape** (cycle set, or a nudge): the cycle keeps its length and the nudge shifts by the edge's movement, so the repeats stay where they are in song time.
- Fades keep their length in ticks, capped at the block length.

## Snap

- A header control left of the transport (magnet icon plus the current grid) opens a panel with **Snap / Slide** and the grid: 1 bar, 1/2, 1/4, 1/8, 1/16, 1/32, 1/64. Default: Snap at 1/4.
- Holding **Alt** during a drag flips snap and slide for that drag.
- Snap applies to block positions and edges, and sets the step for time settings in the settings bar. When sliding, the shortest block is a 64th of a beat.
- The snap setting is a UI preference held by the app, not saved in the project.

## Settings bar

Shown at the bottom of the page while a block is selected: Depth, Fade In, Fade Out, Repeat (cycle length; "Off" when the shape spans the block), Nudge, and an Invert toggle. Values change by dragging them sideways: depth in 1 % steps, times in grid steps.

## Resting value

Each parameter's result row has a small vertical slider just right of its name. It edits the parameter's default state, the value it rests at with no layers acting (`Edit::SetParam`). Moving it never records movement, in line with "moving a knob edits the default state" in `parameters-and-layers.md`. Drag up to raise; 200 px covers the full range, Shift makes it five times finer. Like block drags, it previews (value readout and result curve) and sends one edit on release.

## Zoom

Every timeline page gets two short slim sliders in its bottom-right corner (`crates/daw-ui/src/zoom.rs`, built to be reused on Arrange and Signals): one standing against the right edge for **vertical zoom** (row height) and one along the bottom for **horizontal zoom** (pixels per beat). Each page keeps its own zoom.

- Wide, log-scale ranges, so it works as a zoom tool: rows 28 to 400 px, timeline 2 to 400 px per beat. Drag right or up to zoom in.
- On the Parameters page, result rows and layer rows are always the same height, set by vertical zoom (default 40 px). Selecting a block no longer grows its lane; zoom in for room to edit points.
- Horizontal zoom keeps the middle of the view on the same beat. Grid lines and ruler labels thin out when zoomed far out.
- Zoom is view state, not saved in the project and not undoable.

## Undo

A drag never touches the engine while it is in progress. The page keeps a preview of the block (shown on its lane, in the result curve and in the settings bar) and sends one `SetBlock` when the mouse is released, so each gesture is one undo step. `History` has no edit merging, so this matters.

## Not done yet

- Curved and held segments (the engine supports them; the UI only makes linear points).
- Point snapping.
- Saving the snap setting.
