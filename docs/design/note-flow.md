# Notes in the Tree, Combined Blocks, and the Tree Editor

*Status: built 2026-10-19 (agreed with the user the same day). Notes flowing up and trigger notes are in the engine; combined blocks are `combined.rs` on Arrange; the node-following editor is `piano_roll.rs` with `timeline.rs` (a node's notes as one timeline, put back into its clips) and `tree_editor.rs` (the branch tree, owners, inherited notes). Not yet: resizing a combined block, scales.*

## Notes flow up, like sound

- **Instruments decide which notes exist.** Each instrument plays a set of notes: a Synth all 128, a SingleSampler only its **trigger note**. A node's **note map** is its own instruments' notes plus its children's maps; a node with no instruments (an empty parent) adds nothing of its own. A scale (later) filters a map.
- **A node's notes pass up to its parent.** Each instrument plays its own node's notes and every note from below it, if that note is in the instrument's set. Example: a Synth parent with a Piano child; the Piano's own clip holds a few accents. The Piano plays its accents; the Synth plays its own notes and the Piano's.
- **Notes never travel down.** A note only sounds if an instrument on its node, or above it, plays that pitch; anything else is silent but kept (shown greyed).
- **SingleSampler trigger notes** start at C1 and go up among siblings, so drums in a parent never clash (C1 Kick, C♯1 Snare, D1 Hat…). Each is changeable in the sampler's panel.

## Combined blocks (replacing group clips, user 2026-10-19)

- Clips live only on the node whose instruments play them. **A parent's lane shows combined blocks**: wherever any node below it has clips, overlapping or touching clips merge into one block. Nothing is stored; blocks always match the clips below. The Song's row shows everything.
- In a combined block, **inherited notes are a softer colour**; a parent's **own** clips draw solid as usual (user: "show ITS OWN notes a solider color, and the inherited notes a softer color").
- Dragging a combined block moves every clip inside it on every node below, one undo step; Delete removes them. No resizing a combined block (which clips would stretch is unclear).
- The stored group clips built first (`ClipGroup`, "Pattern n") are removed: one concept. Older files keep every note (members were ordinary clips).

## The editor follows the node, not a clip

- **Selecting a node** (its row or lane) points the editor at that node, at the time Arrange is showing. Double-clicking a lane or a clip opens the editor there. The editor's time is song time (its ruler counts song bars).
- The roll shows the node's notes from all its clips. **Drawing where the node has no clip makes a one-bar clip** (snapped to the bar) around the note. Each note belongs to the clip its start falls in; an edit that moves a note out of every clip makes it a new one-bar clip. Each gesture is one undo step.

## The tree editor

- A **tree panel** on the right of the piano roll shows the selected node's whole branch (from the top-level node under the Song): parents, children, deeper levels nested; each row has colour, name and its note count. Clicking a name selects that node (the roll follows).
- **Focus** (click a name): the roll shows and edits only the focused node's member clip (others hidden), with rows from its note map, named where an instrument names them (a drum's row is its node's name). Focusing the group's own node shows every member's notes in their owners' colours; clicking one focuses its node with that note selected (the parent view does not move other nodes' notes itself). A node with no member yet edits an empty clip spanning the group, which becomes its member with its first note.
- **Drawing target** (click the pen column in the tree): where new notes go. Focusing a child moves the pen to it. With the parent focused, a row with **one owner** draws to that owner whatever the pen says; a row with **several owners** (marked with their count on the keyboard) draws to the pen's node.
- **Rows** follow the focused node's note map. **Fold** (header button) shows only the notes something plays plus any pitch that holds notes (so nothing is hidden); it starts folded when the map is limited (a drum kit) and unfolded for full-range instruments. A named row shows its drum's name; a row several instruments play shows how many; a row nothing plays has a grey label.
- The target is shown three ways in its colour: a **preview note** under the pointer with the owner's name, the lit **pen** in the tree, and a **"Drawing to ● Name"** label in the header (display only). A pen move flashes the new target's row; a drawn note flashes and its owner's count goes up.
