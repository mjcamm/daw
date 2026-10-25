# DAW Foundation: Guiding Principles & Top-Level Design

*Working name: TBD.*

This document captures the high-level vision, principles and application design agreed so far. It is the foundation for the detailed design spec. Implementation detail is deliberately left out.

---

## 1. Vision

A digital audio workstation for electronic and dance music where **movement and automation management is the core improvement over traditional DAWs**.

Existing DAWs treat automation as an afterthought: thin lanes hidden under tracks, drawn point by point, hard to reuse. 
This DAW is designed the other way round where Automations and parameteter movements are a first class citizen like the arrangement and mixing pages

It is an original design, not a clone or clean-room copy of any existing DAW. It will be open source and given away freely.

---

## 2. Guiding Principles

1. **Automation is a first-class citizen.** Design decisions always consider movement and automations easier to create, see, reuse and change.
2. **Few concepts that combine well.** The whole app is explained by three ideas: **Nodes, Layers and Signals**. New features should fit these rather than add new concepts.
3. **One source of truth.** All project state lives in the engine. The UI displays snapshots and sends commands; it never owns state.
4. **Performance is non-negotiable.** Real-time audio never glitches. The UI feels immediate and responsive.
5. **Always in sync.** Signals run in song time; nothing restarts or drifts unless the user asks it to.
6. **Visible cause and effect in the UI.** It must always be possible to answer "why is this moving / automating?" in one click.
7. **Simple foundations, emergent complexity.** Each building block is simple on its own. Depth comes from combining them, not from hidden settings: a song starts with a few nodes, layers and signals, and grows richer as the user stacks, nests and links them.
8. **Built to survive change.** Engine and UI are separated so either side can be replaced without rebuilding the other.
9. **Test driven development** Ensure the project is structured in a clean modular maintainable fashion. New features and functionality and issues flagged in development should generate quick tests that can be run to ensure future regressions do not occur
---

## 3. Core Concepts

### 3.1 Nodes: the project is a tree

- The project is a **hierarchy of nodes**, like folders, rather than a flat list of tracks.
- **Every node is a full channel**: a plugin chain, volume, pan and its own parameter automation page.
- There is **only one node type**. A node can have its own clips (audio, or MIDI played through its instrument), child nodes nested inside it, or both.
- **Audio or Midi information flows up the tree.** A node receives the summed audio of its children, adds its own clips' audio or midi, for midi passes it through its own instrument, then passes the mix through its plugin chain, volume and pan before sending it up to its parent.
- Nodes can collapse and expand; a collapsed node shows a summary of its contents.


### 3.2 Parameters: layering parameters to create movement

- Every parameter rests at its **default state** (e.g. the plugin's own value). With nothing acting on it, it sits there, so "snap back" is automatic.
- Each parameter has a **stack of layer lanes**. Layers combine using **blend modes** (e.g. add, multiply, replace), like photoshops image-editing layers. A wobble layer plus a rising curve layer gives a rising wobble.
- Layer lanes hold **blocks** that are dragged, stretched, looped and copied like clips.
- A layer can be of the type, either **Drawn** blocks or **Listens to a Signal**. On a Listens to a Signal layer, blocks are windows: the signal plays wherever a block sits (what the signal plays at this point in the song), and gaps let nothing through.
- **Gaps mean no effect**: the layer contributes nothing and the parameter falls through to what's underneath.
- Each block has simple per-use controls: depth, invert, small time nudge, and edge fades to avoid clicks which are shown on the right handle properties panel.

### 3.3 Signals: movement drawn once, used anywhere

- A **Signal** is anything published so it can be pulled in anywhere in the project. There are two kinds:
- **Signals**: a centred shape over time (-1 to +1, like a layer lane), with no units or target of their own. They produce no audio. Each use scales and flips it with its blocks' depth and invert.
- Signals **always run in song time**, like a radio station that is always broadcasting. Anything that links to a signal hears it at the same song position; The engine never drifts: every signal and layer is a pure function of song position. Users can bend out of time on purpose (nudge, acceleration); the engine never does it by accident.
- Every signal shows a **"Used by"** list, and everything linked to a signal shows its name and colour.

- A signal lives on its own signal lane.
- Signals are information only, they dont hold audio.
- Repetition lives on the signal itself (e.g. draw one bar, loop as linked copies).
- Signal **sources**:
  - **Drawn** by hand.
  - **MIDI signal** ("Publish MIDI Signal" on a MIDI node): its note triggers, e.g. a kick pattern producing a ducking shape. Known in advance, perfectly in sync.
  - **Audio signal** ("Publish Audio Signal" on an audio node): its audio turned into data by following its loudness.

  - All audio waveforms converting to a signal must be captured as it plays and saved so the UI can display a waveform, as dynamic effects on the node or children nodes may change the resulting waveform.
---

## 4. Top-Level Application Design

### 4.1 Core screens

**Pages**
1. **Arrangement view**: the node tree alongside the timeline, showing clips and song structure only.
2. **Parameters view**: a conditional page, appears when a node is selected. Shows that node's parameters as rows, each expanding into its layer stack, with the combined result on top.
3. **Signals view**: all signals on a shared timeline. signals are drawn and edited here; Selecting a signal shows its source and everything that uses it and the properties panel
4. **Mixer view**: every node's channel strip, arranged to show the tree. Children are grouped under their parent and collapse with it.

**Contextual panel**
- Appears when when something that has a contextual panel is selected, on any page.


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
- new features added should receive a test of some kind either headless or if more UI based, a manual open and computer use to check it works and no regression is introduced.
---

