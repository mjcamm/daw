//! Compiling the project into the flat, time-sorted form the audio side plays.

use crate::clips::ClipRender;
use crate::compensation::{Compensation, PluginFacts, compensate};
use crate::project::Project;
use crate::samples::{SampleData, SamplePool};
use daw_automation::{Automation, CompiledSignal, SignalNote, Signals};
use daw_protocol::{
    AudioSource, DeviceId, EffectKind, InstrumentKind, Node, NodeId, NodeKind, ParamState,
    SignalId, SignalSource,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Everything the audio thread needs to render, compiled from the project by
/// the control side. Immutable once sent.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderState {
    pub tempo_bpm: f64,
    /// Every node in render order: children before their parent, so each
    /// node's bus holds all its children's sound when it renders. The Song
    /// is last.
    pub nodes: Vec<NodeRender>,
    /// How late the song comes out (plugins' latency), in frames.
    pub latency: u32,
}

/// One node, ready to play.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRender {
    pub id: NodeId,
    /// Position of the parent in [`RenderState::nodes`]; `None` for the Song.
    pub parent: Option<usize>,
    /// Its delays, and how late the sound reaches each device.
    pub comp: Compensation,
    /// False when the node is muted. Muting a folder silences what is under
    /// it, since its output carries their sound.
    pub output_on: bool,
    /// False while another node is soloed and neither this node nor any
    /// node above it is.
    pub instrument_on: bool,
    /// Every note its instruments may play, sorted by start: its own, and
    /// (when it has instruments) every note from below it, since notes
    /// pass up the tree.
    pub notes: Vec<ScheduledNote>,
    /// The instruments, layered: each plays every note.
    pub instruments: Vec<InstrumentState>,
    /// The effect chain in signal order.
    pub effects: Vec<EffectState>,
    /// One automation per channel parameter, in index order.
    pub channel: Vec<Automation>,
    /// Its audio clips whose audio was read, sorted by start.
    pub clips: Vec<ClipRender>,
    /// The audio signals following its sound before its effects.
    pub follows: Vec<Follow>,
    /// When it renders: after every node it waits for (its children, and
    /// the sources of the audio signals it listens to), so 0 for a node
    /// that waits for none. Nodes of one height render at the same time.
    pub height: usize,
}

/// Most audio signals followed at once; any more are silent.
pub const MAX_AUDIO_SIGNALS: usize = 32;
/// Most audio signals following one node; any more are silent.
pub const MAX_NODE_FOLLOWS: usize = 8;

/// An audio signal following a node's sound, written to `slot` of the
/// audio side's live values for listeners to read.
#[derive(Debug, Clone, PartialEq)]
pub struct Follow {
    pub signal: SignalId,
    pub slot: usize,
    pub source: AudioSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstrumentState {
    pub id: DeviceId,
    pub kind: InstrumentKind,
    /// Off, it is silent.
    pub enabled: bool,
    /// One automation per parameter, in index order.
    pub params: Vec<Automation>,
    /// The audio a SingleSampler plays, if its sample was read. The audio
    /// thread only reads it; render states are freed on the control side.
    pub sample: Option<Arc<SampleData>>,
    /// The notes it plays; others pass it by.
    pub plays: daw_protocol::NoteSet,
    /// A plugin's parameters in the song, by the plugin's ids.
    pub plugin_params: Vec<(u32, Automation)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectState {
    pub id: DeviceId,
    pub kind: EffectKind,
    /// Off, it passes sound through (its memory is kept for when it is
    /// switched back on).
    pub enabled: bool,
    /// One automation per parameter, in index order.
    pub params: Vec<Automation>,
    /// A plugin's parameters in the song, by the plugin's ids.
    pub plugin_params: Vec<(u32, Automation)>,
    /// A plugin's own bypass parameter, used while it is switched off.
    pub bypass_param: Option<u32>,
}

/// A note at an absolute song position, in beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledNote {
    pub start_beats: f64,
    pub end_beats: f64,
    pub pitch: u8,
    pub velocity: u8,
}

fn compile_params(params: &[ParamState], signals: &Signals) -> Vec<Automation> {
    params
        .iter()
        .map(|state| Automation::compile_with(state, signals))
        .collect()
}

/// The audio signals that are followed, in slot order (signal id order):
/// those whose audio node exists, within [`MAX_AUDIO_SIGNALS`] and
/// [`MAX_NODE_FOLLOWS`].
fn audio_follows(project: &Project) -> Vec<(NodeId, Follow)> {
    let mut follows: Vec<(NodeId, Follow)> = Vec::new();
    for (&signal, s) in &project.signals {
        let SignalSource::Audio(source) = &s.source else {
            continue;
        };
        let audio_node = source.node != NodeId::ROOT
            && project
                .node(source.node)
                .is_some_and(|n| n.kind == NodeKind::Audio);
        let on_node = follows.iter().filter(|(n, _)| *n == source.node).count();
        if audio_node && follows.len() < MAX_AUDIO_SIGNALS && on_node < MAX_NODE_FOLLOWS {
            let slot = follows.len();
            follows.push((
                source.node,
                Follow {
                    signal,
                    slot,
                    source: source.clone(),
                },
            ));
        }
    }
    follows
}

/// Every signal compiled, each notes signal reading its source node's notes.
/// A signal whose node no longer exists reads none. An audio signal reads
/// its slot of the live values; one that can't be followed is silent.
pub fn compile_signals(project: &Project) -> Signals {
    let mut notes_of: BTreeMap<NodeId, Vec<SignalNote>> = BTreeMap::new();
    let mut signals = Signals::default();
    for (id, signal) in &project.signals {
        let notes: &[SignalNote] = match &signal.source {
            SignalSource::Notes(source) => notes_of.entry(source.node).or_insert_with(|| {
                project
                    .node(source.node)
                    .map(schedule_notes)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|n| SignalNote {
                        start: n.start_beats,
                        end: n.end_beats,
                        pitch: n.pitch,
                        velocity: n.velocity,
                    })
                    .collect()
            }),
            SignalSource::Drawn(_) | SignalSource::Audio(_) => &[],
        };
        signals.insert(*id, CompiledSignal::compile(signal, notes.iter().copied()));
    }
    for (_, follow) in audio_follows(project) {
        signals.insert(follow.signal, CompiledSignal::Audio { slot: follow.slot });
    }
    signals
}

/// Every automation of a compiled node.
fn automations_mut(node: &mut NodeRender) -> impl Iterator<Item = &mut Automation> {
    let instruments = node.instruments.iter_mut().flat_map(|i| {
        i.params
            .iter_mut()
            .chain(i.plugin_params.iter_mut().map(|(_, a)| a))
    });
    let effects = node.effects.iter_mut().flat_map(|e| {
        e.params
            .iter_mut()
            .chain(e.plugin_params.iter_mut().map(|(_, a)| a))
    });
    instruments.chain(effects).chain(node.channel.iter_mut())
}

/// The audio signal slots a node's layers listen to.
fn listened_slots(node: &mut NodeRender) -> Vec<usize> {
    let mut slots: Vec<usize> = automations_mut(node)
        .flat_map(|a| a.layers.iter())
        .filter_map(|l| match l.signal {
            Some(CompiledSignal::Audio { slot }) => Some(slot),
            _ => None,
        })
        .collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// Orders the nodes for the audio side: each renders after its children
/// and after the source of every audio signal it listens to, so the
/// signal's values for a chunk are ready when it reads them. A node may
/// listen to its own audio signal (it is followed before its effects). A
/// listen that would loop back (a node listening to a node above it, which
/// carries its own sound, or two nodes listening to each other) can't be
/// followed: that layer hears nothing.
fn plan_follows(nodes: &mut [NodeRender]) {
    let count = nodes.len();
    let mut source_of = [usize::MAX; MAX_AUDIO_SIGNALS];
    for (index, node) in nodes.iter().enumerate() {
        for follow in &node.follows {
            source_of[follow.slot] = index;
        }
    }
    // Who waits for whom: a parent for its children, then each listen.
    let mut waits: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, node) in nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            waits[parent].push(index);
        }
    }
    // Whether `to` already waits, however indirectly, for `from`.
    let waits_for = |waits: &[Vec<usize>], to: usize, from: usize| {
        let mut seen = vec![false; count];
        let mut stack = vec![to];
        while let Some(at) = stack.pop() {
            if at == from {
                return true;
            }
            if !std::mem::replace(&mut seen[at], true) {
                stack.extend(&waits[at]);
            }
        }
        false
    };
    for listener in 0..count {
        for slot in listened_slots(&mut nodes[listener]) {
            let source = source_of[slot];
            if source == listener {
                continue;
            }
            if waits_for(&waits, source, listener) {
                for automation in automations_mut(&mut nodes[listener]) {
                    for layer in &mut automation.layers {
                        if layer.signal == Some(CompiledSignal::Audio { slot }) {
                            layer.signal = Some(CompiledSignal::Silent);
                        }
                    }
                }
            } else {
                waits[listener].push(source);
            }
        }
    }
    // A node renders one above the tallest node it waits for. Waits can
    // point either way in the list, so settle: a chain is at most `count`
    // long.
    for _ in 0..count {
        let mut changed = false;
        for index in 0..count {
            let height = waits[index]
                .iter()
                .map(|&w| nodes[w].height + 1)
                .max()
                .unwrap_or(0);
            if height != nodes[index].height {
                nodes[index].height = height;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Every note on a node's timeline, sorted by start.
pub fn schedule_notes(node: &Node) -> Vec<ScheduledNote> {
    let mut notes: Vec<ScheduledNote> = node
        .clips
        .values()
        .flat_map(|clip| {
            // Only the parts of notes inside the clip play; a looped clip
            // plays its cycle at every repeat.
            clip.played_notes()
                .into_iter()
                .map(move |note| ScheduledNote {
                    start_beats: (clip.start + note.start).as_beats(),
                    end_beats: (clip.start + note.start + note.length).as_beats(),
                    pitch: note.pitch,
                    velocity: note.velocity,
                })
        })
        .collect();
    // Stable sort keeps clip order for simultaneous notes: deterministic.
    notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
    notes
}

/// Every note on a node's timeline and below it, sorted by start: its own
/// first, then its children's in order, for notes that start together.
pub fn subtree_notes(node: &Node) -> Vec<ScheduledNote> {
    fn gather(node: &Node, out: &mut Vec<ScheduledNote>) {
        out.extend(schedule_notes(node));
        for (_, child) in &node.children {
            gather(child, out);
        }
    }
    let mut notes = Vec::new();
    gather(node, &mut notes);
    // Stable: simultaneous notes keep the order above, deterministically.
    notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
    notes
}

/// Compiles `node` and its subtree into `out`, children first, and returns
/// the node's position.
/// What every node's compilation reads.
struct Context<'a> {
    any_soloed: bool,
    signals: &'a Signals,
    project: &'a Project,
    samples: &'a SamplePool,
    sample_rate: u32,
    comp: &'a BTreeMap<NodeId, Compensation>,
    facts: &'a dyn Fn(DeviceId) -> PluginFacts,
    follows: &'a [(NodeId, Follow)],
}

/// A node's audio clips ready to play, sorted by start. Clips whose audio
/// has not been converted to the engine's rate are left out (silent).
fn compile_clips(node: &Node, cx: &Context<'_>) -> Vec<ClipRender> {
    let song_bpm = cx.project.tempo_bpm;
    let rate = f64::from(cx.sample_rate);
    // A looped clip plays as its plain windows back to back.
    let mut clips: Vec<ClipRender> = node
        .audio_clips
        .iter()
        .flat_map(|(&id, clip)| {
            clip.pieces(song_bpm)
                .into_iter()
                .map(move |piece| (id, piece))
        })
        .filter_map(|(id, clip)| {
            let file = &cx.project.samples.get(&clip.sample)?.file;
            let audio = cx.samples.at_rate(clip.sample, file, cx.sample_rate)?;
            Some(ClipRender {
                id,
                start_beats: clip.start.as_beats(),
                end_beats: clip.end().as_beats(),
                audio: Arc::clone(audio),
                offset_frames: clip.offset * rate,
                frames_per_beat: clip.seconds_per_beat(song_bpm) * rate,
                ratio: clip.ratio(song_bpm),
                gain: 10.0f64.powf(clip.gain_db / 20.0) as f32,
                fade_in_beats: clip.fade_in.as_beats(),
                fade_out_beats: clip.fade_out.as_beats(),
            })
        })
        .collect();
    clips.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
    clips
}

fn compile_node(
    id: NodeId,
    node: &Node,
    soloed_above: bool,
    cx: &Context<'_>,
    out: &mut Vec<NodeRender>,
) -> usize {
    let (any_soloed, signals) = (cx.any_soloed, cx.signals);
    let soloed = soloed_above || node.soloed;
    let children: Vec<usize> = node
        .children
        .iter()
        .map(|(child_id, child)| compile_node(*child_id, child, soloed, cx, out))
        .collect();
    let index = out.len();
    out.push(NodeRender {
        id,
        parent: None,
        comp: cx.comp.get(&id).cloned().unwrap_or_default(),
        output_on: !node.muted,
        instrument_on: !any_soloed || soloed,
        notes: if node.instruments.is_empty() {
            schedule_notes(node)
        } else {
            subtree_notes(node)
        },
        instruments: node
            .instruments
            .iter()
            .map(|(id, instrument)| InstrumentState {
                id: *id,
                kind: instrument.kind,
                enabled: instrument.enabled,
                params: compile_params(&instrument.params, signals),
                sample: instrument.sample.and_then(|sample| {
                    let file = &cx.project.samples.get(&sample)?.file;
                    cx.samples.get(sample, file).cloned()
                }),
                plays: instrument.plays(),
                plugin_params: instrument
                    .plugin_params
                    .iter()
                    .map(|p| (p.id, Automation::compile_with(&p.state, signals)))
                    .collect(),
            })
            .collect(),
        effects: node
            .effects
            .iter()
            .map(|(id, effect)| EffectState {
                id: *id,
                kind: effect.kind,
                enabled: effect.enabled,
                params: compile_params(&effect.params, signals),
                plugin_params: effect
                    .plugin_params
                    .iter()
                    .map(|p| (p.id, Automation::compile_with(&p.state, signals)))
                    .collect(),
                bypass_param: if effect.kind == EffectKind::Plugin {
                    (cx.facts)(*id).bypass
                } else {
                    None
                },
            })
            .collect(),
        channel: compile_params(&node.channel, signals),
        clips: compile_clips(node, cx),
        follows: cx
            .follows
            .iter()
            .filter(|(node, _)| *node == id)
            .map(|(_, follow)| follow.clone())
            .collect(),
        height: 0,
    });
    for child in children {
        out[child].parent = Some(index);
    }
    index
}

impl RenderState {
    /// The project with no sample audio (samplers and clips are silent).
    pub fn compile(project: &Project) -> Self {
        Self::compile_with(project, &SamplePool::default(), 48_000)
    }

    /// The project, with samples' audio from `samples`; audio clips play
    /// their samples as converted to `sample_rate`.
    pub fn compile_with(project: &Project, samples: &SamplePool, sample_rate: u32) -> Self {
        Self::compile_with_plugins(project, samples, sample_rate, &|_| PluginFacts::default())
    }

    /// As [`RenderState::compile_with`], lined up for the running
    /// plugins (`compensation.rs`).
    pub fn compile_with_plugins(
        project: &Project,
        samples: &SamplePool,
        sample_rate: u32,
        facts: &dyn Fn(DeviceId) -> PluginFacts,
    ) -> Self {
        let signals = compile_signals(project);
        let follows = audio_follows(project);
        let (comp, total) = compensate(project, facts);
        let cx = Context {
            any_soloed: project.nodes().iter().any(|e| e.node.soloed),
            signals: &signals,
            project,
            samples,
            sample_rate,
            comp: &comp,
            facts,
            follows: &follows,
        };
        let mut nodes = Vec::new();
        compile_node(NodeId::ROOT, &project.root, false, &cx, &mut nodes);
        plan_follows(&mut nodes);
        Self {
            tempo_bpm: project.tempo_bpm,
            nodes,
            latency: total,
        }
    }

    /// A node's compiled form.
    pub fn node(&self, id: NodeId) -> Option<&NodeRender> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::FIRST_NODE;
    use daw_protocol::{ClipId, MidiClip, MidiNote, Ticks};

    fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: Ticks(start),
            length: Ticks(length),
            pitch,
            velocity: 100,
        }
    }

    fn project_with(clips: Vec<MidiClip>) -> Project {
        let mut project = Project::default();
        let node = project.node_mut(FIRST_NODE).unwrap();
        for (i, clip) in clips.into_iter().enumerate() {
            node.clips.insert(ClipId(i as u64), clip);
        }
        project
    }

    fn notes(state: &RenderState) -> &[ScheduledNote] {
        &state.node(FIRST_NODE).unwrap().notes
    }

    fn starts(state: &RenderState) -> Vec<(f64, u8)> {
        notes(state)
            .iter()
            .map(|n| (n.start_beats, n.pitch))
            .collect()
    }

    #[test]
    fn notes_are_placed_at_clip_start_plus_offset_in_beats() {
        let project = project_with(vec![MidiClip {
            start: Ticks::from_beats(4),
            length: Ticks::from_beats(4),
            notes: vec![note(480, 960, 60)],
            looping: None,
        }]);
        let state = RenderState::compile(&project);
        assert_eq!(
            notes(&state),
            [ScheduledNote {
                start_beats: 4.5,
                end_beats: 5.5,
                pitch: 60,
                velocity: 100
            }]
        );
    }

    #[test]
    fn notes_from_all_clips_are_merged_in_time_order() {
        let project = project_with(vec![
            MidiClip {
                start: Ticks::from_beats(2),
                length: Ticks::from_beats(2),
                notes: vec![note(0, 10, 62), note(960, 10, 64)],
                looping: None,
            },
            MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_beats(4),
                notes: vec![note(2880, 10, 65), note(0, 10, 60)],
                looping: None,
            },
        ]);
        let state = RenderState::compile(&project);
        assert_eq!(starts(&state), [(0.0, 60), (2.0, 62), (3.0, 64), (3.0, 65)]);
    }

    #[test]
    fn notes_are_trimmed_to_the_clip() {
        let project = project_with(vec![MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_beats(1),
            notes: vec![
                note(480, 960, 60),  // overhangs the end: trimmed
                note(960, 100, 62),  // starts at the end: dropped
                note(-480, 960, 64), // starts before the clip: trimmed
            ],
            looping: None,
        }]);
        let state = RenderState::compile(&project);
        let spans: Vec<_> = notes(&state)
            .iter()
            .map(|n| (n.pitch, n.start_beats, n.end_beats))
            .collect();
        assert_eq!(spans, [(64, 0.0, 0.5), (60, 0.5, 1.0)]);
    }

    /// Song › [Drums › [Kick, Hats], Bass]: ids 1 to 4.
    fn tree() -> Project {
        let mut project = Project::default();
        project.root.children.clear();
        let mut drums = daw_protocol::Node::new("Drums", 0);
        drums.children = vec![
            (NodeId(2), crate::project::synth_node()),
            (NodeId(3), crate::project::synth_node()),
        ];
        project.root.children = vec![
            (NodeId(1), drums),
            (NodeId(4), crate::project::synth_node()),
        ];
        project
    }

    fn order(state: &RenderState) -> Vec<(u64, Option<u64>)> {
        state
            .nodes
            .iter()
            .map(|n| (n.id.0, n.parent.map(|p| state.nodes[p].id.0)))
            .collect()
    }

    #[test]
    fn children_render_before_their_parent_and_the_song_last() {
        let state = RenderState::compile(&tree());
        assert_eq!(
            order(&state),
            [
                (2, Some(1)),
                (3, Some(1)),
                (1, Some(0)),
                (4, Some(0)),
                (0, None)
            ]
        );
    }

    #[test]
    fn solo_keeps_instruments_under_a_soloed_node() {
        let mut project = tree();
        project.node_mut(NodeId(1)).unwrap().soloed = true;
        let state = RenderState::compile(&project);
        let on = |id| state.node(NodeId(id)).unwrap().instrument_on;
        assert!(on(2) && on(3), "under the soloed folder");
        assert!(!on(4), "another branch is silenced");
        assert!(state.nodes.iter().all(|n| n.output_on), "nothing is muted");
    }

    #[test]
    fn mute_turns_off_only_the_node_itself() {
        let mut project = tree();
        project.node_mut(NodeId(1)).unwrap().muted = true;
        let state = RenderState::compile(&project);
        assert!(!state.node(NodeId(1)).unwrap().output_on);
        assert!(
            state.node(NodeId(2)).unwrap().output_on,
            "its children are silenced through it"
        );
    }
}
