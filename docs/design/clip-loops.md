# Looping Clips

*Status: agreed with the user 2026-10-20 ("yes to all"): the same gesture as blocks, linked repeats, audio loops its window, cuts keep looping. **Built** 2026-10-20: model and playback (engine tests: a looped clip renders bit-for-bit like its repeats as plain clips), Arrange (gesture, drawing, cursors; a headless UI test drags a loop out), piano roll, audio panel. Tried by Claude in the app on MIDI; looped audio not yet seen running. Not yet tried by the user.*

## Model

A clip (MIDI or audio) can loop: `looping: Option<ClipLoop>` (`daw-protocol/src/clip_loop.rs`, file version 14; older files load unchanged).

- **`cycle`**: the length of one run of the content. The clip plays it again and again across its length; the last repeat may be partial.
- **`phase`**: where in the cycle the clip's start falls. A clip trimmed from the left, or the right half of a cut, keeps playing the loop exactly where it was in song time.
- **Linked repeats**: there is one cycle of content. A looped MIDI clip's notes are relative to the cycle's start and play at every repeat, each cut where the cycle ends (`MidiClip::played_notes`). A looped audio clip's `offset` is where the cycle's start plays in the file; every repeat plays the same window.
- The engine refuses a cycle of zero or a phase outside `0..cycle` (`InvalidClip::Loop`).

## Playing

Loops are expanded when the project is compiled, so the audio thread is unchanged: a looped MIDI clip's notes are its played notes; a looped audio clip is its plain windows back to back (`AudioClip::pieces`), each seeking where the loop says. Seams inside a looped audio clip fade over `SEAM_FADE` (8 ticks, ~4 ms at 120 BPM) each side so they never click; the clip's own fades stay at its ends. A looped clip sounds exactly like its repeats laid out as plain clips (`daw-engine/tests/clip_loops.rs`).

## Arrange

- **The same gesture as blocks** (`core-loop-ui.md`): the top quarter of a clip's right edge, and a loop icon there (shown on hover or when selected), drag out repeats; the rest of the edge still trims a plain clip. **On a looped clip the whole right end only drags repeats** (the user's call, 2026-10-24: top to bottom, on the outline and just past it; its start still trims), so a loop always ends on a whole repeat. While dragging, the ghost shows the repeats and cycle lines as they will land. Dragging adds or removes whole repeats; dragging back stops at the clip itself, and with every repeat gone (and the loop starting at its beginning) the clip is plain again.
- **Repeats draw softer**, with a line where each cycle ends: clip, clip, clip.
- **Edges are windows**, as before: trimming the start moves the phase so the content stays put; trimming the end changes only the length. Cutting with the scissors leaves two looped clips, the right one's phase carrying on. Moving and copying are unchanged. Clips never overlap.
- An audio clip's BPM change scales its cycle and phase with its length.

## Piano roll

- Each note of a looped clip shows once, at its first place in the clip, and is edited there; the repeats show faded behind it.
- A note placed anywhere inside a looped clip goes into the loop at that point of the cycle, so it plays at every repeat (and shows at its first place).
