# Curves and the Curve Editor

*Status: agreed with the user and built 2026-10-25. First use: the shape a notes signal plays on every note (`motion-signals.md`). Built to be reused: layer blocks and drawn signals are meant to move onto the same curves and editor later (a separate step, with the user's OK, since it changes listed behaviours on those lanes).*

An original design. The user's reference for the feel is a "shaper" plugin's MIDI-triggered volume shape; we work only from what it does as seen in use, never its code, files, preset shapes, artwork or names.

## Curves

A **curve** is one cycle of a shape: time `0..=1` across, value `0..=1` up (`0.5` is the centre line, no effect). It is a list of points sorted by time, each joined to the next by a straight line or a Bézier curve, like a pen path in a vector drawing app.

Each point has a time, a value, a **kind** and two **handles** (an in-handle pointing back towards the previous point and an out-handle pointing on towards the next), kept as offsets from the point.

| Kind | Handles | Looks like |
|---|---|---|
| **Corner** | Not used (kept, so switching back restores them) | Sharp, straight lines both sides |
| **Smooth** | In and out stay in a straight line through the point; dragging one swings the other opposite, keeping its own length | Curves through without a kink |
| **Broken** | Each moves on its own | A sharp tip with curves on both sides |

- **A curve is a value over time,** so it never loops back. When it is read, each handle's time is held inside its own segment (an in-handle no earlier than the previous point, an out-handle no later than the next). With that, each segment's time runs one way only, so a time has one value.
- **A segment** from `a` to `b` is straight when both ends are Corners on that side; otherwise a cubic Bézier through `a`, `a + a.out` (or `a` if a Corner), `b + b.in` (or `b`), `b`. Values outside `0..=1` (a handle pulled past the edge) are clipped.
- **Two points at the same time** make a jump (a square edge), as in shapes today.
- **The ends are pinned:** the first point sits at time 0 and the last at 1; they move only up and down.
- **Reading** is a pure function (`daw_automation::curve_value`): find the segment by binary search, solve the Bézier's time for the position (Newton steps, falling back to halving), return its value. Allocation-free and deterministic, so live and offline match.

## The editor (`daw-ui/src/curve_editor.rs`)

Knows curves, not signals: it shows one curve over a window, edits it, and reports each change. The page around it decides what the curve is for.

- **Gestures:** double-click empty space adds a Corner point; double-click a point removes it (never the two ends); drag a point to move it (between its neighbours, snapped to the grid, Alt flips snap); click a point to select it. A selected Smooth or Broken point shows its handles; drag a handle to bend.
- **Kinds:** a three-way **Corner | Smooth | Broken** toggle for the selected point, dimmed with none selected. Corner → Smooth pulls handles out a third of the way to each neighbour, along the line between the neighbours. Smooth → Broken keeps them; Broken → Smooth swings the in-handle opposite the out-handle.
- **Grid:** vertical lines at the grid division, a centre line, faint quarter lines. Points snap to the grid in time and to eighths in value (Alt flips snapping, as on the lanes).
- **Each drag is one edit,** heard and drawn everywhere as it moves (`Command::Preview`), one undo step.
- **Painted, not built from elements** (`ui-speed.md`): the curve, grid, points and handles are one canvas (every point's outline one path, every inside another); the container hit-tests. Speed scene `perf_signals_with_shape_panel`: a full redraw of the Signals page with a 64-triangle shape open takes about 4 ms, most of it the zigzag line (here and on the signal's lane); a plain shape about 2 ms.
- **Handles stay inside their segments:** a handle dragged past the next point shrinks along its line to reach it, and moving a point shrinks the handles around it, so what is drawn is what plays.

## Stamps

A stamp replaces the whole curve with a shape, **repeated once per grid cell** across the window (a 1-bar window on a 1/4 grid gets four), filling the full height. The result is ordinary points, edited like any other.

- **Built in:** Ramp up, Ramp down, Triangle, Square, Sine (Smooth points), Duck (straight to the bottom, curving back up to the centre).
- **Saved stamps:** **Save as stamp** keeps the current curve as a new stamp, "Stamp N". Saved for the user across every project in `stamps.json` beside the preferences (`%APPDATA%\daw\`, `daw-host`). Songs never depend on it: stamping copies the points.
- **Cards:** each stamp is a small card: a drawing of its shape with its name under it. Double-click a saved stamp's name to rename it; its × deletes it (built-in stamps have neither). Cards sit on their own strip under the toolbar; when they don't fit, a **›** at the end opens the rest in a dropdown.

## First use: a notes signal's shape

See `motion-signals.md` (From notes). The editor sits in a bottom panel on the **Signals page**, like the piano roll on Arrange, opened by clicking a notes signal's row.

```text
[ Trigger | Loop ]  Window [1/8 1/4 1/2 1 2 4] bars  [snap chip]  [+ Save as stamp]   [ Corner | Smooth | Broken ] [×]
[ stamp cards …                                                                                                     › ]
[ the curve over one window                                                                                           ]
```

Pitch range and velocity clamp stay in the inspector. The grid is the app's snap chip (Snap or Slide, then the grid), the panel's own like the piano roll's: editor state, not saved, 1/16 to start. Built in `crates/daw-ui/src/shape_panel.rs` (the panel), `curve_editor.rs` (drawing and hit-testing), `curve_edit.rs` (the edits, pure), `stamps.rs` (built-in stamps) and `daw_host::stamps` (saved ones). UI tests `ui_tests/shape.rs`.
