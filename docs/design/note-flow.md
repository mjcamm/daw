# Notes in the Tree, Group Clips, and the Tree Editor

*Status: agreed with the user 2026-10-19. Built: notes flowing up and trigger notes (engine), group clips (engine, `group_clips.rs` on Arrange: double-click a folder's empty lane to make one), the tree editor (`tree_editor.rs`: tree, focus, pen, routing, "Drawing to", other members' notes). Next: named and folded rows from the note map, and the preview note under the pointer. Builds on `node-tree.md`, `piano-roll.md` and `samples.md`.*

## Notes flow up, like sound

- **Instruments decide which notes exist.** Each instrument plays a set of notes: a Synth all 128, a SingleSampler only its **trigger note**. A node's **note map** is its own instruments' notes plus its children's maps; a node with no instruments (an empty folder) adds nothing of its own. A scale (later) filters a map.
- **A node's notes pass up to its parent.** Each instrument plays its own node's notes and every note from below it, if that note is in the instrument's set. Example: a Synth parent with a Piano child; the Piano's own clip holds a few accents. The Piano plays its accents; the Synth plays its own notes and the Piano's.
- **Notes never travel down.** A note only sounds if an instrument on its node, or above it, plays that pitch; anything else is silent but kept (shown greyed).
- **SingleSampler trigger notes** start at C1 and go up among siblings, so drums in a folder never clash (C1 Kick, C♯1 Snare, D1 Hat…). Each is changeable in the sampler's panel.

## Group clips

- A **group clip** lives on a parent and holds no notes: a window in time with a name. Its **member clips** live on the children (and on the parent itself, for a parent's own instrument), each holding that node's notes, with the same span, linked to the group. Notes stay with the instrument that plays them.
- Moving, copying, resizing or deleting a group does the same to its members, as one undo step. A member can be unlinked into an ordinary clip. A child with no member yet gets one the first time a note is drawn for it in the group.
- On Arrange, the group shows on the parent's lane; members show on the children's lanes, marked as members.

## The tree editor (piano roll on a group)

- A **tree panel** on the right of the piano roll shows the whole group: the parent, its children, deeper levels nested; each row has colour, name and its note count in the group. The same tree shows whichever node the editor was opened from.
- **Focus** (click a name): the roll shows and edits only the focused node's member clip (others hidden), with rows from its note map, named where an instrument names them (a drum's row is its node's name). Focusing the group's own node shows every member's notes in their owners' colours; clicking one focuses its node with that note selected (the parent view does not move other nodes' notes itself). A node with no member yet edits an empty clip spanning the group, which becomes its member with its first note.
- **Drawing target** (click the pen column in the tree): where new notes go. Focusing a child moves the pen to it. With the parent focused, a row with **one owner** draws to that owner whatever the pen says; a row with **several owners** (marked with their count on the keyboard) draws to the pen's node.
- The target is shown three ways in its colour: a **preview note** under the pointer with the owner's name, the lit **pen** in the tree, and a **"Drawing to ● Name"** label in the header (display only). A pen move flashes the new target's row; a drawn note flashes and its owner's count goes up.
