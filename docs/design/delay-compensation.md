# Delay Compensation (Phase 8 step 6)

*Status: built 2026-10-23 (`daw-engine/src/compensation.rs`, `audio.rs`). The user asked for it to follow plugin instruments; no UI of its own.*

Many plugins delay their sound (a limiter's lookahead, a linear-phase EQ) and say by how much (`getLatencySamples`). Without compensation a node through such a plugin plays late against the rest of the song. The tree makes it matter at every node: a parent sums its own sounds and its children's.

## The rule

At every node, everything summed arrives lined up with the slowest of it; the song as a whole comes out late by its total latency, and the playhead shows what is heard.

- A node's **sum point** collects its built-in sources (built-in instruments and audio clips: no latency), its plugin instruments (each its own latency) and its children's outputs. Its latency there, `S`, is the largest of those; each is delayed by `S` minus its own.
- Its **output latency** is `S` plus every plugin effect's latency in its chain, **switched on or off**: switching never moves the timing. A switched-off plugin with its own bypass parameter (`kIsBypass`) keeps running with that parameter on; around one without, the sound waits in a delay line as long as the plugin would have taken (`Compensation::bypass`).
- A child's output is delayed (at the child) so it reaches the parent's sum point lined up; the Song's output latency is the song's **total latency** `D`.
- Plugin instruments count switched on or off. Plugins not running count as none.
- A plugin that announces a new latency (`restartComponent`) is activated again and the song re-lined (the engine notices through `Plugins::take_facts_changed`).

## Automation follows the sound

A parameter is read at the song time of the audio passing it: an effect after `L` frames of latency in its node (the sum point's plus the effects before it) reads its layers `L` frames earlier; the volume and pan read at the node's output latency. Instruments read at the song time they play. So a block drawn at beat 4 acts on the sound of beat 4, wherever it is in the chain. The shift is in beats at the chunk's tempo; right after a loop jump, shifted reads come from before the loop's start rather than its end (a few milliseconds, accepted).

## How it runs

- **Latencies:** the host answers `GetLatency` after activation (`Vst3State::latency`, or the test device's); the engine keeps them, with each plugin's own bypass parameter (`Plugins::facts`, a `PluginFacts`), and compiles them into the render state: each node's `Compensation` (sum-point delays for its built-in sources and each plugin instrument, each effect's automation shift, the channel's, and its output delay), and the song's total.
- **Delay lines** live with each node's player in a `Delays` box made on the control side (one capacity for the node's lines, a power of two above the longest delay) and sent when a node needs more (`AudioMsg::SetDelays`); the audio side copies the old lines' history into the new ones and hands the old box back to be freed (`retired_delays`). Allocation-free on the audio thread; a delay longer than its line (only until the bigger box arrives) is clamped.
- **Each chunk** of a node: its sources render into the bus and are delayed, each plugin instrument plays into scratch and is delayed and added, then its children's buses are added, then the effects, volume and pan, then the output delay. Live and offline share this, and the result is the same at any block size.
- **Playhead:** while playing, the shown position is the transport's minus `D`.
- **Files:** a render to a file starts `D` frames in and runs `D` longer, so it lines up with the song (`daw-player render`). Engine tests read the raw output.

## Tests

`compensation.rs` (the rule on trees), `daw-plugins/tests/engine.rs` with the test device `TestPlugin::Delay { frames }` (a host-process plugin that delays by `frames` and reports it): nodes line up, automation follows the sound, any block size gives the same output.
