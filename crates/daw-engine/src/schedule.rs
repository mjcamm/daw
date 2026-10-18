//! Compiling the project into the flat, time-sorted form the audio side plays.

use crate::project::Project;
use crate::samples::{SampleData, SamplePool};
use daw_automation::{Automation, CompiledSignal, SignalNote, Signals};
use daw_protocol::{DeviceId, EffectKind, InstrumentKind, Node, NodeId, ParamState, SignalSource};
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
}

/// One node, ready to play.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRender {
    pub id: NodeId,
    /// Position of the parent in [`RenderState::nodes`]; `None` for the Song.
    pub parent: Option<usize>,
    /// False when the node is muted. Muting a folder silences what is under
    /// it, since its output carries their sound.
    pub output_on: bool,
    /// False while another node is soloed and neither this node nor any
    /// node above it is.
    pub instrument_on: bool,
    /// Every note on the node's timeline, sorted by start.
    pub notes: Vec<ScheduledNote>,
    /// The instruments, layered: each plays every note.
    pub instruments: Vec<InstrumentState>,
    /// The effect chain in signal order.
    pub effects: Vec<EffectState>,
    /// One automation per channel parameter, in index order.
    pub channel: Vec<Automation>,
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

/// Every signal compiled, each notes signal reading its source node's notes.
/// A signal whose node no longer exists reads none.
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
            SignalSource::Drawn(_) => &[],
        };
        signals.insert(*id, CompiledSignal::compile(signal, notes.iter().copied()));
    }
    signals
}

/// Every note on a node's timeline, sorted by start.
pub fn schedule_notes(node: &Node) -> Vec<ScheduledNote> {
    let mut notes: Vec<ScheduledNote> = node
        .clips
        .values()
        .flat_map(|clip| {
            clip.notes.iter().filter_map(move |note| {
                // Only the part of a note inside the clip plays.
                let start = note.start.max(daw_protocol::Ticks::ZERO);
                let end = (note.start + note.length).min(clip.length);
                (start < end).then(|| ScheduledNote {
                    start_beats: (clip.start + start).as_beats(),
                    end_beats: (clip.start + end).as_beats(),
                    pitch: note.pitch,
                    velocity: note.velocity,
                })
            })
        })
        .collect();
    // Stable sort keeps clip order for simultaneous notes: deterministic.
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
        output_on: !node.muted,
        instrument_on: !any_soloed || soloed,
        notes: schedule_notes(node),
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
            })
            .collect(),
        channel: compile_params(&node.channel, signals),
    });
    for child in children {
        out[child].parent = Some(index);
    }
    index
}

impl RenderState {
    /// The project with no sample audio (samplers are silent).
    pub fn compile(project: &Project) -> Self {
        Self::compile_with(project, &SamplePool::default())
    }

    /// The project, with samples' audio from `samples`.
    pub fn compile_with(project: &Project, samples: &SamplePool) -> Self {
        let signals = compile_signals(project);
        let cx = Context {
            any_soloed: project.nodes().iter().any(|e| e.node.soloed),
            signals: &signals,
            project,
            samples,
        };
        let mut nodes = Vec::new();
        compile_node(NodeId::ROOT, &project.root, false, &cx, &mut nodes);
        Self {
            tempo_bpm: project.tempo_bpm,
            nodes,
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
            },
            MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_beats(4),
                notes: vec![note(2880, 10, 65), note(0, 10, 60)],
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
