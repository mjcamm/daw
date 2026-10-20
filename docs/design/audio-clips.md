# Audio Clips

*Status: agreed with the user 2026-10-19. **Engine built** (2026-10-19): the clip model and edits, file version 12 (now 14 with looping), rate conversion, waveform overviews in the snapshot, playback through the stretcher (`daw-engine/src/clips.rs`, tests in `daw-engine/tests/audio_clips.rs`). **UI built** (2026-10-19): Track → Import Audio…, dropping files on Arrange with a live preview, audio clips drawn with waveforms, and moving, copying, trimming and revealing them with the MIDI clip gestures (`clip_edit::AnyClip`); tried by Claude in the app, dropping from Explorer not yet tried. **Scissors and the audio panel built** (2026-10-19, tried by Claude in the app; not yet by the user). Builds on `samples.md` (the sample table, the `audio/` folder, the pool), `node-tree.md` (clip gestures on Arrange) and `piano-roll.md` (the bottom panel). The stretcher came from the user's own `audioperformerv2` project and now lives here as `crates/daw-stretch` (docs in its crate root).*

## Stretching happens live, at playback

Audio is stretched while it plays, not rendered once on import or on a tempo change (the user asked; agreed). Songs will get tempo changes at points through the song, and a pre-rendered copy would have to be rebuilt after every tempo edit and would double the memory. Live costs little: about 8% of one core for 16 stereo stems, and a clip whose tempo matches the song passes straight through (bit-exact, almost free).

- **Ratio** (output length / input length) = clip BPM / song BPM when the clip follows tempo, else 1.
- **Position comes from song time.** At a clip's start, after a locate, a loop jump, or an edit that changes what plays, the stream seeks to the source position the song position calls for (`AudioClip::source_seconds`). A tempo change alone only changes the ratio; the stretcher glides to it within about one window.
- **Later, with a tempo map:** drive the stretcher by position (the source position each frame should read, from the map) rather than by a ratio it integrates, so many tempo changes never add up to drift. With one tempo per song the ratio is constant between edits, so nothing drifts yet.
- **Stretching is one part of audio clips, not the foundation.** The foundation is the clip model below: a clip is a window onto a file.

## The clip model

An audio clip is a window onto a sample (`AudioClip` in `daw-protocol`):

| Field | Meaning |
|---|---|
| `start`, `length` | Song position and length, in ticks |
| `sample` | The `SampleId` it plays (the same sample table as the SingleSampler) |
| `offset` | Seconds into the file where the clip's start plays |
| `bpm` | The file's own tempo, entered in the clip panel. New clips take the song's tempo, so they play unstretched until it is set |
| `follow_tempo` | Stretch to the song tempo (on by default). Off, the audio plays at its own speed from `offset`, and a song tempo change slides it against the grid |
| `gain_db` | Clip gain |
| `fade_in`, `fade_out` | Ticks; the two together fit in the clip |

- **Edges trim and reveal.** Dragging an edge shortens the window, and dragging it back reveals the audio again, up to the file's start and end. Trimming the start moves `offset` by the same stretch of audio.
- **Cutting** (the scissors) makes two clips, each a window onto the whole file, so either can be dragged back out again.
- Audio clips go only on Audio nodes (`NodeKind::Audio`), never in groups (groups are for notes). They share the clip id space with MIDI clips; `RemoveClip` removes either.
- Changing `bpm` in the panel keeps the stretch of audio the clip shows: its length scales with the tempo ratio (a UI rule; the engine stores what it is sent).

## Engine

- `Edit::AddAudioClip { node, id, clip }`, `Edit::SetAudioClip { id, node, clip }` (move, trim, change settings, between audio nodes), and `RemoveClip`; each returns its inverse. A clip is refused on anything but an audio node, with an unknown sample, a negative offset or fades longer than the clip.
- A sample used by a clip cannot be removed (`SampleInUse`), as for samplers.
- **Sample rate:** a clip plays its sample converted to the engine's rate once (band-limited, `rubato`'s FFT resampler), on the control side when the state is published, and kept in the pool; the stretcher then works at the engine rate. Converting a long file pauses the UI briefly until loading moves to the background.
- **Waveform overviews:** `Peaks` (min and max per 32 frames, then every four times coarser) is made once per sample when it is read and shared in `SampleSnapshot.peaks`.
- **Audio thread:** each audio node with clips plays them through one `StemStream` (its clips never overlap on the node). Clip players are made up front with the processor (`MAX_CLIP_PLAYERS`, 64, about 0.3 MB each) and lent to audio nodes that have clips, so nothing allocates on the audio thread; past 64 such nodes, the rest are silent. The player never holds a clip's audio `Arc` (it could become the last owner and free it on the audio thread); it remembers where it plays and reads the audio from the current render state. Clip starts and ends are events on their exact frames, like notes. Gain and fades apply per frame from song position. Stopping fades out over a few milliseconds.
- **Live equals offline:** frames depend only on the stream's frame grid, which is set from the node id (not from how many streams were made before), so output is the same for any block size and any thread count.

## Importing

- **Track → Import Audio…** (a new top menu): pick one or more WAV files. Each is copied into `audio/` and goes on its own new audio node, after the selected node (or at the end of the Song), starting at the playhead. One undo step.
- **Drag and drop** files from the system onto Arrange:
  - onto an **audio node's** lane: the clips go **in** that node, one after another from the playhead;
  - onto the **new-node zones** (the top and bottom quarter of a row, scaling with zoom, or below the last row): each file on its own new audio node there, at the playhead;
  - a MIDI node refuses ("Can't go here"), as clip drags do. Nesting is done afterwards.
- New clips are the whole file, `bpm` = the song tempo, following tempo.
- WAV only for now (TODO: more formats, see `STATUS.md`).

## The audio panel

Built in `crates/daw-ui/src/audio_panel.rs`; it takes the piano roll's place in the bottom panel.

- **Opening:** double-click an audio clip (its node opens with the clip selected) or an audio node's empty lane. The bottom panel follows the node selected on Arrange: an audio node shows here, a MIDI node in the piano roll, keeping the panel's height.
- **Body:** the node's clips along song time with the same frame as the piano roll (ruler, scrollbars, zoom; the row zoom sets the lane's height). Each clip shows each side of its audio in its own half, and its fades as lines. Click a clip to select it.
- **Settings column on the right** (where the tree editor's tree sits for notes), for the one selected clip of this node: **File** (name, length, rate, stereo or mono), **Tempo** (BPM, typed: click it, Enter applies, and the clip keeps showing the same audio, `AudioClip::with_bpm`; Follow tempo, a switch), **Level** (Gain, −60 to +24 dB) and **Fades** (Fade in, Fade out, on a square-root scale, never overlapping). Sliders preview while dragged and send one edit on release; double-click a slider for its default. The column scrolls when the panel is short.

## Arrange

- Audio clips draw like MIDI clips (soft tinted fill in the node's colour) with the waveform in the bright detail colour. A clip being stretched shows its BPM in its tag.
- Move, copy (Ctrl), trim and cut with the same gestures as MIDI clips; dropping clips below the last row makes a new node of their kind.
- **Scissors tool** on Arrange (the user's layout): a pointer / scissors switch left of the snap control, keys 1 and 3 (they go to the piano roll instead while the pointer is over it). With the scissors out, a layer over the lanes shows a cut line with its bar and beat at the snapped position under the pointer; a click cuts the clip there, or every selected clip there if that clip is selected (both halves stay selected). One undo step (`clip_edit::split_at`).

## Later

- More file types; reading files in the background with progress.
- Tempo detection; warp markers; transient-aware editing.
- Reverse, pitch. (Looping is built: `clip-loops.md`.)
