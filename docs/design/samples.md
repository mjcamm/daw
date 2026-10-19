# Samples and the SingleSampler

*Status: built (2026-10-18); engine in `daw-dsp/src/sampler.rs`, `daw-engine/src/samples.rs`, import in `daw-host/src/projects.rs`, UI in `daw-ui/src/mixer.rs`. The user: "Lets call it SingleSampler, can we just do the audio engine now then so its just done right … Lets get just basics first. Play audio sample".*

## What it is

**SingleSampler** is a built-in instrument that plays one audio sample. Each note starts the sample from the top and it plays to its end (one-shot), whatever the note's length; velocity sets its level. Several hits overlap. It is the first piece of a drum machine built from nodes (a Drums parent of SingleSampler nodes); the trigger note per drum and auto-assigning the next free note come later.

## Where samples live

- A project is a folder. Imported audio is **copied** into its `audio/` folder (a unique name if the name is taken), so a project carries its own sounds. The project refers to it by a path relative to the project folder, always with `/`.
- The project holds a sample table: `SampleId → SampleRef { name, file }`, saved in the project file (version 9). Adding and removing a sample are undoable edits (`Edit::AddSample`, `Edit::RemoveSample`; a sample in use cannot be removed). An instrument chooses its sample with `Edit::SetInstrumentSample` (undoable).
- The engine decodes samples on the control side into a **sample pool** (`SampleId → Arc<SampleData>`) the first time the project refers to them, reading from the project folder set with `Engine::set_sample_dir`. A file that is missing or cannot be read leaves the instrument silent and shows in the snapshot as missing, with the reason. Decoded audio stays in the pool for the session, so undo and redo never re-read files.
- WAV only for now (16, 24 or 32-bit integer, or 32-bit float; mono or stereo; any sample rate).

## Real-time rules

- The render state carries `Arc<SampleData>` clones; the audio thread only **reads** them. It never clones or drops an `Arc` (replaced states go back to the control side as before), so it never frees sample memory.
- Playback runs at the file's own rate: each voice steps through the sample by `file rate / engine rate` per frame with linear interpolation, so a 44.1 kHz file plays at the right speed and pitch at 48 kHz. A voice's position is plain song-independent state, so output does not depend on block size.
- Instruments render in **stereo** now (the synth writes the same signal to both sides). A mono sample plays in both sides.
- Stopping the transport fades sounding sample voices out over a few milliseconds instead of cutting them, to avoid clicks.

## Parameters

- **Level** (dB, −60 to +6, default 0): the sampler's output level, automatable like any parameter.

## UI (first pass)

- The instrument browser lists SingleSampler under BUILT-IN.
- A MIDI node's subtitle on Arrange names its first instrument and how many more are layered ("MIDI · SingleSampler", "MIDI · Synth +1").
- On a strip, a SingleSampler's chip shows its sample's name. Clicking the chip's name opens a small panel: the sample's name (or "No sample"), and **Choose sample…**, which opens a file picker for WAV files, copies the file into the project and sets it as the sample (one undo step).

- Reading a file happens on the control thread when it is chosen or the project opens, so a very long file pauses the UI briefly. Reading in the background comes with longer audio (audio clips).

## Later

- Trigger note per drum, notes flowing from a parent's clip to its children, auto-assigning the next free note.
- Sample start and end, pitch, reverse, envelopes, choke groups.
- Other file types; audio clips on Audio nodes (the same pool and `audio/` folder).
