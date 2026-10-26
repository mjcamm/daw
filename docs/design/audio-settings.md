# Audio Settings and the Audio Engine's Safety

*Status: built 2026-10-18. The user: "We have a file menu for the settings where follow the industry standard to manage audio devices and playback etc", and asked for denormal handling, output safety, the audio settings and multicore rendering.*

## Preferences window

File → Preferences… (Ctrl+,) opens a two-pane window: sections down the left (**Audio**, **Appearance**), the section's settings on the right, Close at the bottom right. Clicking outside closes it.

## Audio section

| Setting | Choices | Notes |
|---|---|---|
| Driver | The system's drivers (WASAPI on Windows), the default marked | Changing it resets device, rate and buffer to that driver's defaults |
| Output device | System default, then every output device | Saved by the system's stable device id |
| Sample rate | Device default, then 44.1 to 192 kHz where the device offers 32-bit float | |
| Buffer size | Device default, then 32 to 2048 samples with latency in ms | When the driver fixes the size (Windows Audio in shared mode: 10 ms), shown read-only with a note that a smaller buffer needs a driver such as ASIO |
| Processing threads | Auto (one per core, one core left free, at most 8), or 1 to the core count | 1 renders everything on the audio thread |

Below them: the running status ("Running: WASAPI · Speakers · 48 kHz · 481 samples · 10.0 ms · 8 threads", the buffer the device actually uses) or why it could not start, and **Test tone** (one second of A4 at −12 dB through the output, over whatever plays; needs an open project).

Every change applies at once: the stream restarts with `Engine::reconnect`, which keeps the song, undo history, position, playing and the loop. A setup that fails falls back to the system default. Settings are saved in the app's preferences (`daw_host::preferences::AudioSettings`) and used for every project. `cargo run -p daw-device --example devices` lists what each device reports.

## CPU meter

In the header, right of the transport: the audio thread's load (processing time over the time the audio lasts), rising at once and falling slowly. Amber from 70%, red for about a second after a dropout (a late callback), with a tooltip pointing to the buffer size.

## Engine safety

- **Denormals:** every block renders with flush-to-zero set (x86-64 MXCSR FTZ+DAZ, ARM64 FPCR FZ; `daw_rt::without_denormals`), restored afterwards.
- **Output guard:** the last step before the device turns any non-finite sample into silence and clips at ±1.0 (`audio::safe`). Live and offline renders both pass through it.

## Multicore rendering

- Each node renders its whole chunk on its own (its notes, parameters and a copy of the transport). Nodes are grouped by height in the tree (leaves first); each group renders in parallel on a pool of worker threads (`parallel.rs`) with the audio thread working too. A parent starts from its children's sum, added in render order, so the sound is bit-identical on any number of threads (`tests/parallel.rs`).
- Workers are made up front with audio priority; a run allocates nothing, takes tasks from one atomic word, wakes workers with `unpark`, and the audio thread spins for the last tasks. Workers spin briefly for more work, then sleep.

## Later

- **ASIO** (smaller buffers on Windows): needs Steinberg's ASIO SDK and LLVM on the build machine; waiting on the user's go-ahead.
- Audio inputs, with recording (Phase 9).
