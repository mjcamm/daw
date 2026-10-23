# Export

*Built 2026-10-23 (Phase 9's mixdown; stems later). Agreed with the user: File → Export… (Ctrl+E) opens a small modal; WAV or MP3; the app stays usable while it renders; LAME for MP3 behind a cargo feature; the song ends at its last clip.*

## What the user sees

- **File → Export…** or **Ctrl+E** opens the dialog (`daw-ui/src/export_dialog.rs`), on any page. Rows of segmented choices:
  - **Format:** WAV | MP3 (MP3 is greyed out in builds without the `mp3` feature).
  - **Quality:** WAV 16-bit (dithered) | 24-bit | 32-bit float, default 24; MP3 192 | 256 | 320 kbps, default 320. Each format keeps its own choice while switching.
  - **Range:** Whole song (start to the end of the last clip, with its length) | Loop (the loop region, on or off; greyed out without one).
  - **Tail:** Until silent (releases and effects ring out, up to 10 s, trailing silence cut) | Off.
  - A note: stereo at the engine's rate (an MP3 above 48 kHz is saved at 48 kHz, MP3's highest), or why Export… is off (no clips, empty loop).
- **Export…** opens the system save dialog in `<project>/Exports/` (made if missing) with the song's name; the format's extension is added if the name lacks it.
- **While it renders** the dialog shows a progress bar and Cancel, and drops its backdrop: the app around it works (the song plays and edits on; the export renders a copy taken when it started). Cancel leaves no file, and an existing file is only replaced by a finished export.
- **Done:** the file's name, its length and peak (orange "clipped" at full scale), Show in Folder and Done. **Failed:** why, Back and Close.
- The choices are remembered while the project is open.

## How it works

- `daw-host/src/export.rs` (shared by the app and tests): `export(ExportJob, &Progress)` loads the job's `ProjectFile` into an engine of its own (`Engine::new_offline`: stereo, the session's rate and threads, render workers at normal priority so the live song always comes first, its own plugin host process), turns looping off, locates to the range's start, plays, and renders offline (`set_offline(true)`: plugins take as long as they need) in blocks of 1024 through `AudioProcessor::process`, the live path. So a file is sample-for-sample what playing the song gives (`tests/export.rs` checks it against `daw_testkit::render`).
- **Latency:** the first `processor.latency()` frames are skipped, so the file lines up with the song (`daw-plugins/tests/engine.rs`, `an_export_starts_the_songs_latency_in`). The song part ends on its exact frame, then the transport stops and the tail renders into memory until 0.5 s below −96 dBFS or 10 s.
- **Writing:** a `.partial` file beside the target, renamed over it when complete; removed on cancel or failure. WAV through `hound` (16-bit: triangular dither of ±1 step from a fixed seed, so exports repeat exactly; 24-bit rounded; float as is). MP3 through LAME (`mp3lame-encoder`): CBR, best quality, joint stereo, resampled by LAME above 48 kHz, no VBR tag.
- **The app:** `Session::export_job` takes the song with `Engine::to_file` (running plugins are asked for their state, as on save). The dialog runs `export` on a thread named "export" and polls its `Progress` and `JoinHandle` every 50 ms; it is a cached view (`perf_export`), redrawn only by that poll.

## Licence (MP3)

LAME is LGPL-2.0 and the `mp3lame-encoder` bindings are LGPL-3.0; both are compiled from C source into the binary (needs a C compiler; MSVC on Windows). The project is MIT OR Apache-2.0, so MP3 sits behind `daw-host`'s `mp3` feature, on by default. A release must either build with `--no-default-features` (WAV only), or meet the LGPL for the linked LAME (offer the source and a way to relink, e.g. by linking LAME dynamically). `THIRD-PARTY-LICENSES.md` (and `licenses/`) gives the notices, sources and how to rebuild or leave MP3 out; ship it with releases. Whether to link LAME as a DLL instead is to settle before Phase 11.

## Later

- Stems (one file per node, or per chosen nodes).
- Normalize, sample-rate conversion and a choice of rate for WAV.
- Plugins in VST3 offline mode (`kOffline`) during exports (they run in realtime mode with no deadline today).
- The export's plugin host runs its audio threads at audio priority, as the live one does; a heavy plugin song could compete with live play while it exports.
- Cancelling a running export when the app quits (the `.partial` file is left today).
