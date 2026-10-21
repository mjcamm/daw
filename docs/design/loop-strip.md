# Loop Strip

*Status: agreed with the user 2026-10-21 (modelled on Studio One, with one difference: dragging the middle of a loop draws a new one instead of moving it). **Built** 2026-10-21: rules in `crates/daw-ui/src/loop_strip.rs` (pure, unit tested), drawing and presses in `loop_view.rs`, engine on/off in `daw-engine` (file version 15). UI tests in `ui_tests/loop_strip.rs`. Not yet tried by the user.*

## Where

Every ruler has it: Arrange, Parameters and Signals (one shared ruler), the piano roll and the audio panel. There is one loop for the song, shown on all of them.

```text
 ┌──────────────────────────────────────────────────────────┐
 │      ▌████████████ 6.1 – 8.1 ███████████▐                │  loop strip, top 14 px
 │ 1        2        3        4        5        6           │  bar numbers: click locates
 └──────────────────────────────────────────────────────────┘
```

- Rulers are 28 px: the loop strip on the top 14 px, bar numbers below (the user's call). The piano roll's (18 px) and the audio panel's (22 px) grew to 28 to fit it.
- No tint across the lanes: the loop shows in the ruler only (the user's call).

## Gestures

| Gesture | Result |
|---|---|
| Drag on the strip, anywhere but a handle (including inside the loop) | Draws a fresh loop from the press to the pointer, either way round. On release it is the loop and looping switches on |
| Drag the start or end handle | Moves that end; looping stays on or off as it was |
| Click the strip without dragging | Nothing |
| Click below the strip (the bar numbers) | Locates, as before |
| Escape while dragging | Cancels |

- **Handles** are 6 px tabs inside each end (half the loop each when it is short); a press up to 3 px outside an end still takes its handle. They show a sideways cursor.
- **Snapping:** the page's grid. Arrange, Parameters, Signals and the audio panel use the header's snap; the piano roll its own grid. Slide places ends where the pointer is. Alt flips it while dragging. A loop is at least one grid step long (sliding: the engine's shortest, a 64th note), and ends never cross.
- While dragging, the loop shows where it runs ("6.1 – 8.1": the end is where the next bar or beat starts).

## On and off

- The transport's loop button switches looping on and off and keeps the region (one way to switch it, the user's rule). With looping off the region is drawn grey and can still be dragged. With no region the button is dim and its tooltip says to drag on a ruler's strip.
- Loop changes are not undo steps, like locating.

## Engine

- `Snapshot.loop_range` is the region, kept while looping is off; `Snapshot.loop_enabled` says whether it plays.
- `Command::SetLoop(Some(range))` sets the region and switches looping on; `SetLoop(None)` clears both. `Command::SetLoopEnabled(bool)` switches looping and keeps the region (it stays off without one).
- File version 15 saves `loop_enabled`; older files loop over any region they have.
