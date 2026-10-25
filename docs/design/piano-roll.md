# Piano Roll

*Status: built 2026-10-16 (`crates/daw-ui/src/piano_roll.rs` panel, `note_edit.rs` rules), agreed with the user as standard DAW behaviour. The bottom panel now follows the selected node (`note-flow.md`) and hosts the audio panel for audio nodes (`audio-clips.md`).*

## Panel

- **New clips:** double-click an empty spot on a MIDI node's lane on Arrange: an empty clip a bar long starts at the grid line at or before the pointer (Arrange's grid; shorter if the next clip comes sooner), is selected and opens here (user, 2026-10-18). Rule: `clip_edit::room_for_clip`.
- **Opening:** double-click a clip (or an empty MIDI lane) on Arrange. The panel slides up from the bottom (about 0.2 s), resizes from its top edge (at least 160 px), and closes with × in its header. It belongs to Arrange. It shows the selected node, not one clip (`note-flow.md`): selecting another node swaps it in without sliding again.
- **Header:** colour dot and "<node> · notes", the tools (select, pen, scissors), Quantize, the grid picker, ×.
- **Scroll and zoom:** the same frame as Arrange (`frame.rs`, `Timeline::Notes`): a vertical scrollbar with the key-height zoom at its foot, and a horizontal scrollbar with the time zoom at its end (user: "the same zoom tools as the arranger"). Keys range 6 to 40 px (12 to start) and zoom about the middle of the view; time zoom holds the playhead still when it is in view. One scroll area serves both axes (`TimelineView::shared`).
- **Grid:** the editor has its own snap or slide and grid (1 Bar to 1/64, starting at 1/16), set with the same chip and panel as Arrange's (user, 2026-10-16: "a grid 1 bar, 1/4, 1/8, 1/16 etc and slide mode same as arranger has"). The grid draws a line at every step while they are at least 6 px apart, beats stronger, bars strongest. New notes are one grid step long; left and right nudge by a step.
- **Body:** a ruler (song bars), a keyboard down the left (white and black keys from the `key_white` / `key_black` theme tokens; C notes labelled, middle C = C3; click a key to hear it), the grid (black-key rows shaded, octave and beat lines, the area after the clip's end dimmed), and a velocity lane along the bottom. The keyboard, ruler and velocity lane follow the grid's scroll. On opening, the zoom fits the clip and the view centres on its notes.

## Tools

| Tool | Key | Does |
|---|---|---|
| Select | 1 | Click selects, Shift+click adds or removes, drag on empty grid boxes a selection; drag notes to move in time and pitch (snapped, Alt flips), edges (5 px) to resize, Ctrl to copy; double-click empty grid adds a note one grid step long, double-click a note deletes it |
| Pen | 2 | Click empty grid to add a note and drag to set its length; click a note to delete it |
| Scissors | 3 | Click a note to split it at the snapped position (a bent note's halves keep their parts of the bend) |
| Bend | 4 | Bends notes by their dots (`note-bends.md`) |

Also: Delete removes the selected notes (before clips or nodes); up and down move the selection a semitone, Shift for an octave; left and right a grid step; Ctrl+A selects all; Ctrl+C copies, Ctrl+V pastes at the playhead. Dragging a velocity bar sets that note's velocity, and the rest of the selection's alike (each from its own value).

## Feel

- **Quantize** (button, or Q): the selected notes' starts move to the nearest line of the editor's grid (every note when none are selected); lengths stay. It uses the grid even in Slide. Rule: `note_edit::quantized`.
- **Auto-scroll:** dragging notes (moving, resizing, drawing with the pen, box selecting) within 32 px of the grid's edges scrolls it, faster the deeper, and the drag follows; drags measure in the frame they began in, so the notes travel with the view. `scroll::edge_step` is shared with clip drags on Arrange.
- **Moving:** press a note and drag. Nothing moves until the pointer has travelled 3 px, so a click only selects. The grabbed note's start lands on the grid (or slides), and its pitch is the row under the pointer.
- **Resizing:** each end has a resize zone with a sideways cursor (held for the whole drag), so long, zoomed-in and tiny notes all feel reliable (user, 2026-10-16):
  - *Inside* the note it grows with the note's width on screen, 15% of it, between 4 and 10 px, and never more than a quarter of the note, so the middle always moves it.
  - *Outside* the note it hangs up to 5 px into free space on its row, never past half the gap to the next note, so neighbours share a gap fairly. Before the clip start there is nothing to hang into.
  - The rules are `note_edit::edge_inside`, `edge_outside`, `edge_zones` and `hit`; the drawn cursor zones use the same numbers.
- **Hearing notes:** adding a note, moving notes to a new pitch, nudging pitch and clicking a key play the note for about 0.3 s through the node's instruments (`Command::PreviewNote`, not part of the song or undo).
- **Undo:** each gesture is one `Edit::SetClip`.
