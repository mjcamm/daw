# DAW Foundation: Guiding Principles & Top-Level Design

*Status: draft for review. Working name: TBD.*

This document captures the high-level vision, principles and application design agreed so far. It is the foundation for the detailed design spec. Implementation detail is deliberately left out.

---

## 1. Vision

A digital audio workstation for electronic and dance music where **movement is the core of the music, and automation is the core of the app**.

Existing DAWs treat automation as an afterthought: thin lanes hidden under tracks, drawn point by point, hard to reuse. This DAW is designed the other way round. The pitch:

> **Draw a movement once, use it everywhere, and build your track around movement.**

It is an original design, not a clone or clean-room copy of any existing DAW. It will be open source and given away freely.

---

## 2. Guiding Principles

1. **Automation is a first-class citizen.** Design decisions always consider movement and automations easier to create, see, reuse and change.
2. **Few concepts that combine well.** The whole app is explained by three ideas: **Nodes, Layers and Signals**. New features should fit these rather than add new concepts.
3. **One source of truth.** All project state lives in the engine. The UI displays snapshots and sends commands; it never owns state.
4. **Performance is non-negotiable.** Real-time audio never glitches. The UI feels immediate.
5. **Always in sync.** Signals run in song time; nothing restarts or drifts unless the user asks it to.
6. **Visible cause and effect.** It must always be possible to answer "why is this moving / automating?" in one click.
7. **Simple foundations, emergent complexity.** Each building block is simple on its own. Depth comes from combining them, not from hidden settings: a song starts with a few nodes, layers and signals, and grows richer as the user stacks, nests and links them.
8. **Focus effort on what's unique.** Reuse proven open-source building blocks for generic plumbing; spend effort on the node, layer and signal model and its UI.
9. **Built to survive change.** Engine and UI are separated so either side can be replaced without rebuilding the other.
10. **Test driven development** Ensure the project is structured in a clean modular maintainable fashion
---

## 3. Core Concepts

### 3.1 Nodes: the project is a tree

- The project is a **hierarchy of nodes**, like folders, rather than a flat list of tracks.
- **Every node is a full channel**: a plugin chain, volume, pan and its own automation page.
- There is **only one node type**. A node can have its own clips (audio, or MIDI played through its instrument), child nodes nested inside it, or both.
- **Audio flows up the tree.** A node receives the summed audio of its children, adds its own clips' audio, then passes the mix through its plugin chain, volume and pan before sending it up to its parent.
- Folders collapse and expand; a collapsed node shows a summary of its contents.


### 3.2 Parameters: layering parameters to create movement

- Every parameter rests at its **default state** (e.g. the plugin's own value). With nothing acting on it, it sits there, so "snap back" is automatic.
- Each parameter has a **stack of layer lanes**. Layers combine using **blend modes** (e.g. add, multiply, replace), like image-editing layers. A wobble layer plus a rising curve layer gives a rising wobble.
- Layer lanes hold **blocks** that are dragged, stretched and copied like clips.
- A block is either **drawn locally** or **linked to a Signal** (which plays what the original signal plays at this point in the song), selected from the block itself.
- **Gaps mean no effect**: the layer contributes nothing and the parameter falls through to what's underneath.
- Each block has simple per-use controls: depth, invert, small time nudge, and edge fades to avoid clicks.

### 3.3 Signals: movement drawn once, used anywhere

- A **Signal** is anything published so it can be pulled in anywhere in the project. There are two kinds:
  - **Motion signals**: a shape over time, with no units or target of their own. They produce no audio. Each use sets where the bottom and top of the shape land on its target parameter.
  - **Audio signals**: a node's sound, pulled into sidechain inputs or into other nodes (e.g. sends to a shared reverb).
- Signals **always run in song time**, like a radio station that is always broadcasting. Anything that links to a signal hears it at the same song position; nothing ever restarts a signal.
- **Sidechaining is a Signal.** Cross-branch effects (e.g. kick ducking the bass) are handled by publishing a signal and linking to it, not by drawing routing cables across the tree.
- Every signal shows a **"Used by"** list, and everything linked to a signal shows its name and colour.

#### Motion signals

- A motion signal lives on its own signal lane.
- Repetition lives on the signal itself (e.g. draw one bar, repeat as linked copies).
- Motion signal **sources**:
  - **Drawn** by hand.
  - **From MIDI notes** broadcast by a MIDI node, e.g. a kick pattern producing a ducking shape. Known in advance, perfectly in sync.
  - **From audio before effects**, analysed from the clip. Known in advance.
  - **From audio after effects**, followed live during playback. Effects are dynamic, so this shape cannot be known in advance; it is captured as it plays and saved so the UI can display it.

#### Audio signals

- An audio signal is a node's sound, published so other parts of the tree can pull it in.
- Pulling an audio signal into a layer turns it into motion by following its loudness. This behaves like an "audio after effects" motion source: followed live, captured and saved for display.

### 3.4 The one rule

> **Every parameter has a stack of layers. Each layer holds blocks. Each block is either drawn there or linked to a signal.**

---

## 4. Top-Level Application Design

### 4.1 Core screens

**Pages**
1. **Arrangement view**: the node tree alongside the timeline, showing clips and song structure only.
2. **Layers view**: appears when a node is selected. Shows that node's parameters as rows, each expanding into its layer stack, with the combined result on top.
3. **Signals view**: all signals on a shared timeline. Motion signals are drawn and edited here; audio signals are listed by the node publishing them. Selecting a signal shows its source and everything that uses it.
4. **Mixer view**: every node's channel strip, arranged to show the tree. Children are grouped under their parent and collapse with it, and audio signals (sends and sidechains) show where sound is pulled across branches.

**Contextual panel**
- **Channel strip (right side)**: appears when a node is selected, on any page. A single strip from the mixer: the node's plugin chain, volume and pan, including built-in devices with their visuals.


### 4.2 Built-in devices

- Core devices (EQ, compressor, etc.) are built **natively** into the engine for deep automation integration and integrated visuals.
- Their DSP is written as **standalone, portable modules** so they could later also be packaged as standalone plugins.
- Native devices and third-party plugins share **one parameter model**, so layers and signals work identically on both.

### 4.3 Visualisers

- EQ curves, spectrum analysers and spectrograms are built in.
- Analysis happens in the engine, off the audio thread; drawing happens on the GPU.

### 4.4 Look and feel

- Dark and light themes from day one; no hard-coded colours anywhere.
- Signals are colour-coded throughout the app. And can have their colours picked.
- Nodes can be color coded throughout the app. And can have their colours picked.

---

## 5. Technology Direction

| Area | Decision                                                                                          | Why |
|---|---------------------------------------------------------------------------------------------------|---|
| Audio engine | **Rust**                                                                                          | Real-time safe, top-tier performance, compiler acts as a strong feedback loop for AI-assisted development |
| UI framework | **GPUI**                                                                                          | Native GPU rendering, lowest latency, proven in production by Zed |
| Heavy visuals | **Isolated rendering module**, drawn with GPUI first; a custom GPU renderer only if performance demands it | Keeps the hardest-to-port code behind its own interface, without fighting the UI framework early |
| Plugin hosting | **VST3 first, then CLAP**                                                                | VST3 is essential for user reach, and the plugins available for development and testing are VST3. CLAP follows later: it is simple to host from Rust and automation-friendly |
| Generic plumbing | **Reuse existing open-source crates**                                                             | Don't rebuild what already exists (audio I/O, file decoding, plugin hosting, FFT, etc.) |
| Licence | **Dual-licensed under MIT OR Apache-2.0**                                                         | Given away freely; users may choose either licence |

**Architectural rules:**

- The engine is **UI-agnostic** and can run headless.
- The UI holds no authoritative state; it renders engine snapshots and sends commands.
- The audio thread never allocates memory or locks, enforced by tooling from the start.
- Development is **AI-assisted**, supported by headless rendering, automated audio regression tests and testable pure functions for automation logic.

---

## 6. Development Approach

- **Prove the core loop first**: one instrument, one built-in effect, a layered automation page and a MIDI-driven signal, with no third-party plugins. If shaping movement feels great, build outward.
- Then expand to the node tree, signals across the project, plugin hosting and the remaining table-stakes DAW features.

---

## 7. Non-Goals (for now)

- Not a clone or clean-room reimplementation of any existing DAW.
- Not a general-purpose "do everything" DAW from day one; dance and electronic production is the focus.
- No commercial model planned.

---

## 8. Open Questions

To be resolved in the detailed spec:

- How layers combine in practice: which blend modes, and in what value space (raw control range vs real units).
- What happens when a value exceeds a parameter's range after layering.
- Whether moving a plugin's knob edits its default state or records new movement.
- How collapsed nodes summarise their contents.
- Whether plugins run in separate processes to protect against crashes.
- How the app stays robust when a plugin update changes its parameters.
- How live-followed signals (audio after effects, and audio signals pulled into layers) behave before they have been captured, when the audio feeding them changes, and during offline render/export.
- Working name.
