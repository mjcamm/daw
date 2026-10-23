//! Delay compensation: the delays that line a song up when plugins delay
//! their sound (`docs/design/delay-compensation.md`).
//!
//! At every node, its built-in sources, its plugin instruments and its
//! children's outputs meet at its sum point, each delayed to arrive with
//! the slowest; its plugin effects then add their latency. A plugin keeps
//! its latency when switched off, so switching never moves the timing: one
//! with its own bypass parameter keeps running bypassed, and around one
//! without, the sound waits as long as the plugin would have taken. The
//! Song's output latency is the song's total.

use crate::project::Project;
use daw_protocol::{DeviceId, EffectKind, InstrumentKind, Node, NodeId};
use std::collections::BTreeMap;

/// What compensation needs to know about a running plugin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PluginFacts {
    /// How late its sound comes out, in frames.
    pub latency: u32,
    /// Its own bypass parameter, if it has one.
    pub bypass: Option<u32>,
}

/// One node's delays and how late the sound is at each of its devices, in
/// frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compensation {
    /// Delay for its built-in sources (built-in instruments, audio clips).
    pub sources: u32,
    /// Delay for each instrument, in the node's order; built-in ones play
    /// with `sources`, so theirs is 0.
    pub instruments: Vec<u32>,
    /// How late the sound reaching each effect is (its automation reads
    /// that much earlier).
    pub effects: Vec<u32>,
    /// For each effect, the delay that stands in for it while it is
    /// switched off (a plugin without its own bypass); 0 for the rest.
    pub bypass: Vec<u32>,
    /// How late the sound reaching volume and pan is: the node's output
    /// latency.
    pub channel: u32,
    /// Delay at its output, so it reaches its parent's sum point lined up.
    pub output: u32,
}

impl Compensation {
    /// Whether it delays anything.
    pub fn delays(&self) -> bool {
        self.sources > 0
            || self.output > 0
            || self.instruments.iter().any(|&d| d > 0)
            || self.bypass.iter().any(|&d| d > 0)
    }
}

/// Every node's compensation, and the song's total latency, given each
/// plugin's `latency` in frames.
pub fn compensate(
    project: &Project,
    facts: &dyn Fn(DeviceId) -> PluginFacts,
) -> (BTreeMap<NodeId, Compensation>, u32) {
    let mut out = BTreeMap::new();
    let total = walk(NodeId::ROOT, &project.root, facts, &mut out);
    (out, total)
}

/// Fills in `id` and everything below it; returns its output latency.
fn walk(
    id: NodeId,
    node: &Node,
    facts: &dyn Fn(DeviceId) -> PluginFacts,
    out: &mut BTreeMap<NodeId, Compensation>,
) -> u32 {
    let children: Vec<(NodeId, u32)> = node
        .children
        .iter()
        .map(|(child, n)| (*child, walk(*child, n, facts, out)))
        .collect();
    let instrument_latency: Vec<u32> = node
        .instruments
        .iter()
        .map(|(device, i)| {
            if i.kind == InstrumentKind::Plugin {
                facts(*device).latency
            } else {
                0
            }
        })
        .collect();
    let sum = children
        .iter()
        .map(|(_, l)| *l)
        .chain(instrument_latency.iter().copied())
        .max()
        .unwrap_or(0);
    for (child, l) in &children {
        if let Some(c) = out.get_mut(child) {
            c.output = sum - l;
        }
    }
    let mut at = sum;
    let mut bypass = Vec::with_capacity(node.effects.len());
    let effects = node
        .effects
        .iter()
        .map(|(device, e)| {
            let here = at;
            let plugin = (e.kind == EffectKind::Plugin).then(|| facts(*device));
            let latency = plugin.map_or(0, |f| f.latency);
            at += latency;
            let stands_in = plugin.is_some_and(|f| f.bypass.is_none()) && !e.enabled;
            bypass.push(if stands_in { latency } else { 0 });
            here
        })
        .collect();
    let instruments = node
        .instruments
        .iter()
        .zip(&instrument_latency)
        .map(|((_, i), l)| {
            if i.kind == InstrumentKind::Plugin {
                sum - l
            } else {
                0
            }
        })
        .collect();
    out.insert(
        id,
        Compensation {
            sources: sum,
            instruments,
            effects,
            bypass,
            channel: at,
            output: 0,
        },
    );
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::FIRST_NODE;
    use daw_protocol::{Edit, Effect, Instrument, PluginSpec, TestPlugin};

    const SLOW: DeviceId = DeviceId(500);
    const SLOWER: DeviceId = DeviceId(501);
    const INSTRUMENT: DeviceId = DeviceId(502);

    /// SLOW has its own bypass parameter (id 3); the others don't.
    fn latency(device: DeviceId) -> PluginFacts {
        let latency = match device {
            SLOW => 100,
            SLOWER => 30,
            INSTRUMENT => 50,
            _ => 0,
        };
        PluginFacts {
            latency,
            bypass: (device == SLOW).then_some(3),
        }
    }

    fn plugin() -> PluginSpec {
        PluginSpec::Test(TestPlugin::Gain)
    }

    fn add_effect(project: &mut Project, node: NodeId, id: DeviceId, index: usize) {
        project
            .apply(Edit::AddEffect {
                node,
                id,
                index,
                effect: Effect::plugin(plugin()),
            })
            .unwrap();
    }

    /// The Song with the first node (a synth) and a second, plain node.
    fn two_nodes() -> Project {
        let mut project = Project::default();
        project
            .apply(Edit::AddNode {
                id: NodeId(2),
                parent: NodeId::ROOT,
                index: 1,
                node: daw_protocol::Node::new("Plain", 0),
            })
            .unwrap();
        project
    }

    #[test]
    fn without_latency_nothing_is_delayed() {
        let (comp, total) = compensate(&two_nodes(), &latency);
        assert_eq!(total, 0);
        assert!(comp.values().all(|c| !c.delays()));
    }

    #[test]
    fn a_slow_effect_delays_its_siblings_and_the_song() {
        let mut project = two_nodes();
        add_effect(&mut project, FIRST_NODE, SLOW, 0);
        add_effect(&mut project, FIRST_NODE, SLOWER, 1);
        let (comp, total) = compensate(&project, &latency);
        assert_eq!(total, 130);
        let first = &comp[&FIRST_NODE];
        assert_eq!(
            first.effects,
            [0, 100],
            "the second effect hears it 100 late"
        );
        assert_eq!((first.channel, first.output), (130, 0));
        assert_eq!(comp[&NodeId(2)].output, 130, "the plain node waits");
        assert_eq!(comp[&NodeId::ROOT].sources, 130);
    }

    #[test]
    fn a_plugin_instrument_lines_up_with_built_in_ones_on_its_node() {
        let mut project = two_nodes();
        project
            .apply(Edit::AddInstrument {
                node: FIRST_NODE,
                id: INSTRUMENT,
                index: 1,
                instrument: Instrument::plugin(plugin()),
            })
            .unwrap();
        add_effect(&mut project, FIRST_NODE, SLOWER, 0);
        let (comp, total) = compensate(&project, &latency);
        let first = &comp[&FIRST_NODE];
        assert_eq!(first.sources, 50, "the built-in synth waits for it");
        assert_eq!(first.instruments, [0, 0]);
        assert_eq!(first.effects, [50]);
        assert_eq!(first.channel, 80);
        assert_eq!(total, 80);

        // Switched off, plugins keep their latency, so the timing stays;
        // around one without its own bypass the sound waits as long.
        project
            .apply(Edit::SetDeviceEnabled {
                id: INSTRUMENT,
                enabled: false,
            })
            .unwrap();
        project
            .apply(Edit::SetDeviceEnabled {
                id: SLOWER,
                enabled: false,
            })
            .unwrap();
        let (comp, total) = compensate(&project, &latency);
        assert_eq!(total, 80);
        assert_eq!(comp[&FIRST_NODE].bypass, [30]);
    }

    #[test]
    fn a_plugin_with_its_own_bypass_needs_no_stand_in_delay() {
        let mut project = two_nodes();
        add_effect(&mut project, FIRST_NODE, SLOW, 0);
        project
            .apply(Edit::SetDeviceEnabled {
                id: SLOW,
                enabled: false,
            })
            .unwrap();
        let (comp, total) = compensate(&project, &latency);
        assert_eq!(total, 100);
        assert_eq!(comp[&FIRST_NODE].bypass, [0], "it runs bypassed");
    }

    #[test]
    fn nested_nodes_line_up_at_every_sum() {
        // Song > Folder (SLOWER) > [Inner (SLOW), Other]; Song > Plain.
        let mut project = two_nodes();
        let mut folder = daw_protocol::Node::new("Folder", 0);
        folder
            .children
            .push((NodeId(11), daw_protocol::Node::new("Inner", 0)));
        folder
            .children
            .push((NodeId(12), daw_protocol::Node::new("Other", 0)));
        project
            .apply(Edit::AddNode {
                id: NodeId(10),
                parent: NodeId::ROOT,
                index: 2,
                node: folder,
            })
            .unwrap();
        add_effect(&mut project, NodeId(11), SLOW, 0);
        add_effect(&mut project, NodeId(10), SLOWER, 0);
        let (comp, total) = compensate(&project, &latency);
        assert_eq!(comp[&NodeId(12)].output, 100, "waits for its sibling");
        assert_eq!(comp[&NodeId(10)].sources, 100);
        assert_eq!(comp[&NodeId(10)].channel, 130);
        assert_eq!(comp[&NodeId(2)].output, 130, "waits for the folder");
        assert_eq!(comp[&FIRST_NODE].output, 130);
        assert_eq!(total, 130);
    }
}
