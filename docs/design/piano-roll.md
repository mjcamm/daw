# Piano Roll

*Status: built 2026-10-16 (`crates/daw-ui/src/piano_roll.rs` panel, `note_edit.rs` rules), agreed with the user as standard DAW behaviour. The bottom panel is meant to host an audio editor later.*

## Panel

- **Opening:** double-click a clip on Arrange. The panel slides up from the bottom (about 0.2 s), resizes from its top edge (at least 160 px), and closes with × in its header. It belongs to Arrange.
- **Header:** colour dot and "<node> · notes", the tools (select, pen, scissors), a zoom slider, ×.
- **Body:** a ruler (song bars), a keyboard down the left (white and black keys from the `key_white` / `key_black` theme tokens; C notes labelled, middle C = C3; click a key to hear it), the grid (black-key rows shaded, octave and beat lines, the area after the clip's end dimmed), and a velocity lane along the bottom. The keyboard, ruler and velocity lane follow the grid's scroll. On opening, the zoom fits the clip and the view centres on its notes.

## Tools

| Tool | Key | Does |
|---|---|---|
| Select | 1 | Click selects, Shift+click adds or removes, drag on empty grid boxes a selection; drag notes to move in time and pitch (snapped, Alt flips), edges (5 px) to resize, Ctrl to copy; double-click empty grid adds a note one grid step long, double-click a note deletes it |
| Pen | 2 | Click empty grid to add a note and drag to set its length; click a note to delete it |
| Scissors | 3 | Click a note to split it at the snapped position |

Also: Delete removes the selected notes (before clips or nodes); up and down move the selection a semitone, Shift for an octave; left and right a grid step; Ctrl+A selects all; Ctrl+C copies, Ctrl+V pastes at the playhead. Dragging a velocity bar sets that note's velocity, and the rest of the selection's alike (each from its own value).

## Feel

- **Hearing notes:** adding a note, moving notes to a new pitch, nudging pitch and clicking a key play the note for about 0.3 s through the node's instruments (`Command::PreviewNote`, not part of the song or undo).
- **Undo:** each gesture is one `Edit::SetClip`.
