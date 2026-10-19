# Notes in the Tree, Combined Blocks, and the Tree Editor

*Status: built 2026-10-19 (agreed with the user the same day). Notes flowing up and trigger notes are in the engine; combined blocks are `combined.rs` on Arrange; the node-following editor is `piano_roll.rs` with `timeline.rs` (a node's notes as one timeline, put back into its clips) and `tree_editor.rs` (the branch tree, owners, inherited notes). Not yet: resizing a combined block, scales.*

## Notes flow up, like sound

- **Instruments decide which notes exist.** Each instrument plays a set of notes: a Synth all 128, a SingleSampler only its **trigger note**. A node's **note map** is its own instruments' notes plus its children's maps; a node with no instruments (an empty parent) adds nothing of its own. A scale (later) filters a map.
- **A node's notes pass up to its parent.** Each instrument plays its own node's notes and every note from below it, if that note is in the instrument's set. Example: a Synth parent with a Piano child; the Piano's own clip holds a few accents. The Piano plays its accents; the Synth plays its own notes and the Piano's.
- **Notes never travel down.** A note only sounds if an instrument on its node, or above it, plays that pitch; anything else is silent but kept (shown greyed).
- **SingleSampler trigger notes** start at C1 and go up among siblings, so drums in a parent never clash (C1 Kick, C♯1 Snare, D1 Hat…). Each is changeable in the sampler's panel.

## Combined blocks (replacing group clips, user 2026-10-19)

- **Clips on a node are solid: they never overlap** (user, 2026-10-19: "no stacking in a node ... that's always annoyed me in other DAWs"). The engine rejects any edit that would leave two clips on a node overlapping (`CommandError::StackedClips`, checked after a whole edit, so a batch may overlap halfway); touching is fine. Dropping, pasting or drawing cuts what it lands on instead. Files before version 13 merge stacked MIDI clips into one, each note kept as it was heard. Audio clips follow the same rule (a node plays one at a time).
- Clips live only on the node whose instruments play them. **A parent's lane shows combined blocks**: wherever any node below it has clips, overlapping or touching clips merge into one block. Nothing is stored; blocks always match the clips below. The Song's row shows everything.
- In a combined block, **inherited notes are a softer colour**; a parent's **own** clips draw solid as usual (user: "show ITS OWN notes a solider color, and the inherited notes a softer color").
- Dragging a combined block moves every clip inside it on every node below, one undo step; Delete removes them. No resizing a combined block (which clips would stretch is unclear). Blocks follow any drag as it happens, not on drop (user: "I want all the ghost clips to move in real time"): a clip or block being dragged reshapes every parent's blocks live (`shown_clips`).
- The stored group clips built first (`ClipGroup`, "Pattern n") are removed: one concept. Older files keep every note (members were ordinary clips).

## The editor follows the node, not a clip

- **Selecting a node** (its row or lane) points the editor at that node, at the time Arrange is showing. Double-clicking a clip opens the editor there; double-clicking an empty MIDI lane also adds a one-bar clip there first (`clip_edit::room_for_clip`, as in `piano-roll.md`; restored 2026-10-20 after the node-following editor dropped it by mistake). The editor's time is song time (its ruler counts song bars).
- The roll shows the node's notes from all its clips. **Drawing where the node has no clip makes a one-bar clip** (snapped to the bar) around the note. Each note belongs to the clip its start falls in; an edit that moves a note out of every clip makes it a new one-bar clip. Each gesture is one undo step.

## The tree editor

- A **tree panel** on the right of the piano roll shows the selected node's whole branch (from the top-level node under the Song): parents, children, deeper levels nested; each row has colour, name and its note count. Clicking a name selects that node (the roll follows).
- **Editing in place** (user, 2026-10-19: "stay in that combined view, allowing me to change the note"): a parent's view shows its whole branch's notes. One node is **edited** at a time (the pen in the tree, named by "Drawing to"): its notes are solid and behave as in any roll; the rest are softer in their nodes' colours. Pressing a softer note makes its node the edited one and the same press picks the note up, so it moves or resizes straight away; the view stays on the parent. "Drawing to" flashes in the new node's colour so the switch is noticed. Clicking a name in the tree selects that node (the editor then shows its branch from there).
- **Drawing target**: the edited node, set by the pen column or by pressing one of its notes. With a parent in view, a row with **one owner** draws to that owner whatever the pen says; a row with **several owners** (marked with their count on the keyboard) draws to the pen's node.
- **Rows** follow the focused node's note map. **Fold** (header button) shows only the notes something plays plus any pitch that holds notes (so nothing is hidden); it starts folded when the map is limited (a drum kit) and unfolded for full-range instruments. A named row shows its drum's name; a row several instruments play shows how many; a row nothing plays has a grey label.
- The target is shown three ways in its colour: a **preview note** under the pointer with the owner's name, the lit **pen** in the tree, and a **"Drawing to ● Name"** label in the header (display only). A pen move flashes the new target's row; a drawn note flashes and its owner's count goes up.
