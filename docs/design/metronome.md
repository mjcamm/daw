# Metronome

*Agreed with the user 2026-10-24; built the same day.*

## Decisions (the user's)

- **Transport:** a metronome button switches the click on and off (lit when on), and a volume button beside it opens a vertical slider while the pointer is over it (or the slider is held). Lucide's `metronome` icon (from `lucide-static@1.61.0`; 0.460.0 has none) and `volume-2` / `volume-x`.
- **Preferences → Metronome:** Sound (Beep, Wood, Tick, Cowbell; picking one plays it), Accent on beat 1, Pitch (±12 semitones; a click plays when let go), Volume (the transport's slider too). Double-click a slider for its default.
- **Clicks only while playing**, on every beat, following tempo and the loop. 4/4: beat 1 of each bar is the accent.
- **After the Song's channel:** its fader and effects never change the click. **Never in exports** (an export renders a fresh engine from the song file, which holds no click).
- **An app setting**, not part of the song or undo: kept in the preferences (`Preferences::metronome`), the same for every song.

## How it works

- `daw_protocol::Metronome` (on, volume 0–100, sound, accent, pitch); `Command::SetMetronome` and `Command::PreviewClick`; `Snapshot::metronome` shows it.
- The sound is generated: `daw_dsp::click::ClickVoice` (four tones, accent higher and louder, every click identical, rising from silence so nothing pops).
- Audio side (`daw-engine/src/metronome.rs`): before each chunk renders, the beats it crosses are found from song time with `Transport::frames_until` (segment by segment across loop jumps), so a click's frame never depends on block size. Clicks are queued the song's latency later, so they line up with what is heard, then added to the output after the Song's bus.
- UI: the transport's controls are `metronome_ui.rs`; changes reach the engine at once and the app's preferences through `ProjectEvent::Metronome` (a volume drag when it lands). The page is `metronome_settings.rs`. The app sends the saved settings to every project it opens.

## Tests

`daw-dsp` click tests; `daw-engine/tests/metronome.rs` (every beat on its frame, accent, off/stopped/silent, any block size, loops, over the Song's fader, preview, not in the song); UI tests `ui_tests/metronome.rs` (the switch, the volume popover and drag, the page, kept in the preferences).
