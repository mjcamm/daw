//! Authoritative project state and the edits that change it.
//!
//! The project is a tree of nodes under the Song ([`NodeId::ROOT`]). Ids are
//! project-wide: nodes, clips, effects, layers and blocks each have one id
//! space across the whole tree, so edits find things by id alone.

use crate::devices::{
    CHANNEL_PARAMS, INSTRUMENT_PARAMS, ParamInfo, effect_params, fixed_device_params,
    instrument_params,
};
use daw_automation::{validate_block, validate_notes_source};
use daw_protocol::{
    AudioClip, Block, BlockId, ClipId, CommandError, DeviceId, DeviceRef, Edit, Effect, Instrument,
    InstrumentKind, InvalidClip, Layer, LayerId, MAX_CLIP_BPM, MAX_CLIP_GAIN_DB, MIN_CLIP_BPM,
    MIN_CLIP_GAIN_DB, MidiClip, Node, NodeId, NodeKind, ParamId, ParamState, SampleId, SampleRef,
    Signal, SignalId, SignalSource, SignalUse,
};
use std::collections::{BTreeMap, HashSet};

pub const MIN_TEMPO_BPM: f64 = 20.0;
pub const MAX_TEMPO_BPM: f64 = 999.0;
pub const DEFAULT_TEMPO_BPM: f64 = 120.0;
/// Most effects in one node's chain.
pub const MAX_EFFECTS: usize = 16;
/// Most instruments layered on one node.
pub const MAX_INSTRUMENTS: usize = 8;
/// Most nodes in a project, the Song included. The audio side keeps a
/// player for each, made up front.
pub const MAX_NODES: usize = 256;
/// Number of colours in the content palette.
pub const PALETTE_SIZE: u8 = 9;
/// The Song's colour: grey.
pub const SONG_COLOR: u8 = 8;
/// The node a new project starts with, under the Song.
pub const FIRST_NODE: NodeId = NodeId(1);

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub tempo_bpm: f64,
    /// The Song: the root of the node tree and the master output.
    pub root: Node,
    /// Motion signals, by id.
    pub signals: BTreeMap<SignalId, Signal>,
    /// Audio files the project uses, by id.
    pub samples: BTreeMap<SampleId, SampleRef>,
}

/// A node as found by walking the tree.
#[derive(Debug, Clone, Copy)]
pub struct NodeEntry<'a> {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub depth: usize,
    pub node: &'a Node,
}

impl Default for Project {
    /// A Song holding one node that plays notes: [`FIRST_NODE`].
    fn default() -> Self {
        let mut root = song();
        root.children.push((FIRST_NODE, synth_node()));
        Self {
            name: "Untitled".to_owned(),
            tempo_bpm: DEFAULT_TEMPO_BPM,
            root,
            signals: BTreeMap::new(),
            samples: BTreeMap::new(),
        }
    }
}

/// The Song with nothing in it: a plain bus at its defaults.
pub fn song() -> Node {
    let mut node = Node::new("Song", SONG_COLOR).with_kind(NodeKind::Audio);
    node.channel = default_params(&CHANNEL_PARAMS);
    node
}

/// The first node's Synth in a new project.
pub const FIRST_INSTRUMENT: DeviceId = DeviceId(0);

/// A MIDI node named "Synth" with one Synth (`FIRST_INSTRUMENT`), at its
/// defaults.
pub fn synth_node() -> Node {
    let mut node = Node::new("Synth", 6);
    let mut synth = Instrument::new(InstrumentKind::Synth);
    synth.params = default_params(&INSTRUMENT_PARAMS);
    node.instruments.push((FIRST_INSTRUMENT, synth));
    node.channel = default_params(&CHANNEL_PARAMS);
    node
}

pub fn default_params(infos: &[ParamInfo]) -> Vec<ParamState> {
    infos
        .iter()
        .map(|info| ParamState {
            base: info.default_normalized(),
            layers: Vec::new(),
        })
        .collect()
}

/// What the engine needs to know about one node, on its own.
pub trait NodeExt {
    /// Every device on the node, in signal order, with its parameters.
    fn devices(&self) -> impl Iterator<Item = (DeviceRef, &[ParamState])>;
    fn param_infos(&self, device: DeviceRef) -> Option<&'static [ParamInfo]>;
    fn params(&self, device: DeviceRef) -> Option<&Vec<ParamState>>;
    fn params_mut(&mut self, device: DeviceRef) -> Option<&mut Vec<ParamState>>;
    fn effect(&self, id: DeviceId) -> Option<(usize, &Effect)>;
    fn instrument(&self, id: DeviceId) -> Option<(usize, &Instrument)>;
}

impl NodeExt for Node {
    fn devices(&self) -> impl Iterator<Item = (DeviceRef, &[ParamState])> {
        self.instruments
            .iter()
            .map(|(id, i)| (DeviceRef::Instrument(*id), i.params.as_slice()))
            .chain(
                self.effects
                    .iter()
                    .map(|(id, e)| (DeviceRef::Effect(*id), e.params.as_slice())),
            )
            .chain(std::iter::once((
                DeviceRef::Channel,
                self.channel.as_slice(),
            )))
    }

    fn param_infos(&self, device: DeviceRef) -> Option<&'static [ParamInfo]> {
        match device {
            DeviceRef::Effect(id) => self.effect(id).map(|(_, e)| effect_params(e.kind)),
            DeviceRef::Instrument(id) => {
                self.instrument(id).map(|(_, i)| instrument_params(i.kind))
            }
            fixed => fixed_device_params(fixed),
        }
    }

    fn params(&self, device: DeviceRef) -> Option<&Vec<ParamState>> {
        match device {
            DeviceRef::Instrument(id) => self.instrument(id).map(|(_, i)| &i.params),
            DeviceRef::Channel => Some(&self.channel),
            DeviceRef::Effect(id) => self.effect(id).map(|(_, e)| &e.params),
        }
    }

    fn params_mut(&mut self, device: DeviceRef) -> Option<&mut Vec<ParamState>> {
        match device {
            DeviceRef::Instrument(id) => self
                .instruments
                .iter_mut()
                .find(|(i, _)| *i == id)
                .map(|(_, i)| &mut i.params),
            DeviceRef::Channel => Some(&mut self.channel),
            DeviceRef::Effect(id) => self
                .effects
                .iter_mut()
                .find(|(e, _)| *e == id)
                .map(|(_, e)| &mut e.params),
        }
    }

    fn effect(&self, id: DeviceId) -> Option<(usize, &Effect)> {
        self.effects
            .iter()
            .enumerate()
            .find(|(_, (e, _))| *e == id)
            .map(|(i, (_, effect))| (i, effect))
    }

    fn instrument(&self, id: DeviceId) -> Option<(usize, &Instrument)> {
        self.instruments
            .iter()
            .enumerate()
            .find(|(_, (i, _))| *i == id)
            .map(|(index, (_, instrument))| (index, instrument))
    }
}

fn find_node(node: &Node, node_id: NodeId, id: NodeId) -> Option<&Node> {
    if node_id == id {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|(child_id, child)| find_node(child, *child_id, id))
}

fn find_node_mut(node: &mut Node, node_id: NodeId, id: NodeId) -> Option<&mut Node> {
    if node_id == id {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|(child_id, child)| find_node_mut(child, *child_id, id))
}

fn walk_into<'a>(
    id: NodeId,
    parent: Option<NodeId>,
    depth: usize,
    node: &'a Node,
    out: &mut Vec<NodeEntry<'a>>,
) {
    out.push(NodeEntry {
        id,
        parent,
        depth,
        node,
    });
    for (child_id, child) in &node.children {
        walk_into(*child_id, Some(id), depth + 1, child, out);
    }
}

/// How many nodes are in `node`'s subtree, itself included.
/// Checks that `node` and everything under it are one kind, the kind of
/// the parent they go under (any kind under the Song).
fn check_kinds(
    parent: NodeId,
    parent_kind: NodeKind,
    id: NodeId,
    node: &Node,
) -> Result<(), CommandError> {
    if parent != NodeId::ROOT && node.kind != parent_kind {
        return Err(CommandError::MixedKinds { node: id, parent });
    }
    for (child_id, child) in &node.children {
        check_kinds(id, node.kind, *child_id, child)?;
    }
    Ok(())
}

/// Every instrument in `node` and under it.
fn subtree_instruments(node: &Node) -> Vec<(DeviceId, &Instrument)> {
    let mut out: Vec<(DeviceId, &Instrument)> =
        node.instruments.iter().map(|(id, i)| (*id, i)).collect();
    for (_, child) in &node.children {
        out.extend(subtree_instruments(child));
    }
    out
}

/// A sample file must be a relative path inside the project folder, with
/// `/` between parts and no way out of it.
fn check_sample_file(file: &str) -> Result<(), CommandError> {
    let bad = file.is_empty()
        || file.starts_with('/')
        || file.contains('\\')
        || file.contains(':')
        || file
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if bad {
        return Err(CommandError::InvalidSampleFile {
            file: file.to_owned(),
        });
    }
    Ok(())
}

fn count_nodes(node: &Node) -> usize {
    1 + node
        .children
        .iter()
        .map(|(_, c)| count_nodes(c))
        .sum::<usize>()
}

impl Project {
    /// Every node in tree order: each parent before its children, the Song
    /// first.
    pub fn nodes(&self) -> Vec<NodeEntry<'_>> {
        let mut out = Vec::new();
        walk_into(NodeId::ROOT, None, 0, &self.root, &mut out);
        out
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        find_node(&self.root, NodeId::ROOT, id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        find_node_mut(&mut self.root, NodeId::ROOT, id)
    }

    /// A node that exists and takes MIDI clips: a MIDI node, not the Song.
    fn check_plays_clips(&self, node: NodeId) -> Result<(), CommandError> {
        match self.node(node) {
            None => Err(CommandError::UnknownNode { id: node }),
            Some(n) if n.kind != NodeKind::Midi || node == NodeId::ROOT => {
                Err(CommandError::NotMidi { node })
            }
            Some(_) => Ok(()),
        }
    }

    /// A node that exists and takes audio clips: an audio node, not the Song.
    fn check_takes_audio(&self, node: NodeId) -> Result<(), CommandError> {
        match self.node(node) {
            None => Err(CommandError::UnknownNode { id: node }),
            Some(n) if n.kind != NodeKind::Audio || node == NodeId::ROOT => {
                Err(CommandError::NotAudio { node })
            }
            Some(_) => Ok(()),
        }
    }

    /// An audio clip must be valid and name a sample in the table.
    fn check_audio_clip(&self, clip: &AudioClip) -> Result<(), CommandError> {
        validate_audio_clip(clip).map_err(|reason| CommandError::InvalidClip { reason })?;
        if !self.samples.contains_key(&clip.sample) {
            return Err(CommandError::UnknownSample { id: clip.sample });
        }
        Ok(())
    }

    fn node_or_err(&mut self, id: NodeId) -> Result<&mut Node, CommandError> {
        self.node_mut(id).ok_or(CommandError::UnknownNode { id })
    }

    /// A node's parent and its position among the parent's children.
    pub fn parent_of(&self, id: NodeId) -> Option<(NodeId, usize)> {
        self.nodes().into_iter().find_map(|entry| {
            let index = entry.node.children.iter().position(|(c, _)| *c == id)?;
            Some((entry.id, index))
        })
    }

    pub fn param(&self, param: ParamId) -> Option<&ParamState> {
        self.node(param.node)?
            .params(param.device)?
            .get(param.index as usize)
    }

    fn param_mut(&mut self, param: ParamId) -> Result<&mut ParamState, CommandError> {
        self.node_mut(param.node)
            .and_then(|node| node.params_mut(param.device))
            .and_then(|params| params.get_mut(param.index as usize))
            .ok_or(CommandError::UnknownParam { param })
    }

    /// The node holding a clip.
    pub fn clip_node(&self, id: ClipId) -> Option<NodeId> {
        self.nodes()
            .into_iter()
            .find(|e| e.node.clips.contains_key(&id) || e.node.audio_clips.contains_key(&id))
            .map(|e| e.id)
    }

    /// The node holding an effect.
    pub fn effect_node(&self, id: DeviceId) -> Option<NodeId> {
        self.nodes()
            .into_iter()
            .find(|e| e.node.effect(id).is_some())
            .map(|e| e.id)
    }

    /// Whether any instrument plays the sample.
    pub fn sample_in_use(&self, id: SampleId) -> bool {
        subtree_instruments(&self.root)
            .iter()
            .any(|(_, i)| i.sample == Some(id))
            || self
                .nodes()
                .iter()
                .any(|e| e.node.audio_clips.values().any(|c| c.sample == id))
    }

    /// An instrument may only name a sample if it is a SingleSampler and
    /// the sample is in the table.
    fn check_instrument_sample(
        &self,
        id: DeviceId,
        instrument: &Instrument,
    ) -> Result<(), CommandError> {
        let Some(sample) = instrument.sample else {
            return Ok(());
        };
        if instrument.kind != InstrumentKind::SingleSampler {
            return Err(CommandError::NotASampler { id });
        }
        if !self.samples.contains_key(&sample) {
            return Err(CommandError::UnknownSample { id: sample });
        }
        Ok(())
    }

    pub fn instrument_node(&self, id: DeviceId) -> Option<NodeId> {
        self.nodes()
            .into_iter()
            .find(|e| e.node.instrument(id).is_some())
            .map(|e| e.id)
    }

    /// Where a layer lives: its parameter and position in the stack.
    pub fn find_layer(&self, id: LayerId) -> Option<(ParamId, usize)> {
        self.nodes().into_iter().find_map(|entry| {
            entry.node.devices().find_map(|(device, params)| {
                params.iter().enumerate().find_map(|(index, state)| {
                    let position = state.layers.iter().position(|(l, _)| *l == id)?;
                    let param = ParamId {
                        node: entry.id,
                        device,
                        index: index as u32,
                    };
                    Some((param, position))
                })
            })
        })
    }

    /// Where a block lives: its layer's parameter, the layer's position in
    /// the stack, the layer id and the block's position in the layer.
    fn find_block(&self, id: BlockId) -> Option<(ParamId, usize, LayerId, usize)> {
        self.nodes().into_iter().find_map(|entry| {
            entry.node.devices().find_map(|(device, params)| {
                params.iter().enumerate().find_map(|(index, state)| {
                    state
                        .layers
                        .iter()
                        .enumerate()
                        .find_map(|(layer_pos, (layer_id, layer))| {
                            let block_pos = layer.blocks.iter().position(|(b, _)| *b == id)?;
                            let param = ParamId {
                                node: entry.id,
                                device,
                                index: index as u32,
                            };
                            Some((param, layer_pos, *layer_id, block_pos))
                        })
                })
            })
        })
    }

    fn layer_mut(&mut self, id: LayerId) -> Result<&mut Layer, CommandError> {
        let (param, position) = self
            .find_layer(id)
            .ok_or(CommandError::UnknownLayer { id })?;
        Ok(&mut self.param_mut(param)?.layers[position].1)
    }

    /// Every layer that listens to a signal: where it is and which signal,
    /// in tree order, then device order.
    pub fn listeners(&self) -> Vec<(SignalUse, SignalId)> {
        let mut out = Vec::new();
        for entry in self.nodes() {
            for (device, params) in entry.node.devices() {
                for (index, state) in params.iter().enumerate() {
                    let param = ParamId {
                        node: entry.id,
                        device,
                        index: index as u32,
                    };
                    for (layer, l) in &state.layers {
                        if let Some(signal) = l.signal {
                            out.push((
                                SignalUse {
                                    param,
                                    layer: *layer,
                                },
                                signal,
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    /// Every id in use anywhere in the project.
    fn used_ids(&self) -> UsedIds {
        let mut used = UsedIds::default();
        for entry in self.nodes() {
            used.nodes.insert(entry.id);
            used.clips.extend(entry.node.clips.keys().copied());
            used.clips.extend(entry.node.audio_clips.keys().copied());
            used.devices
                .extend(entry.node.effects.iter().map(|(id, _)| *id));
            used.devices
                .extend(entry.node.instruments.iter().map(|(id, _)| *id));
            for (_, params) in entry.node.devices() {
                for state in params {
                    for (layer_id, layer) in &state.layers {
                        used.layers.insert(*layer_id);
                        used.blocks.extend(layer.blocks.iter().map(|(b, _)| *b));
                    }
                }
            }
        }
        for signal in self.signals.values() {
            if let SignalSource::Drawn(blocks) = &signal.source {
                used.blocks.extend(blocks.iter().map(|(b, _)| *b));
            }
        }
        used
    }
}

/// Where a block on a signal's lane is: the signal and its position.
fn find_signal_block(
    signals: &BTreeMap<SignalId, Signal>,
    id: BlockId,
) -> Option<(SignalId, usize)> {
    signals
        .iter()
        .find_map(|(signal_id, signal)| match &signal.source {
            SignalSource::Drawn(blocks) => blocks
                .iter()
                .position(|(b, _)| *b == id)
                .map(|position| (*signal_id, position)),
            SignalSource::Notes(_) => None,
        })
}

/// A drawn signal's blocks, or `WrongSignalKind`.
fn drawn_blocks(
    signals: &mut BTreeMap<SignalId, Signal>,
    id: SignalId,
) -> Result<&mut Vec<(BlockId, Block)>, CommandError> {
    match &mut signals
        .get_mut(&id)
        .ok_or(CommandError::UnknownSignal { id })?
        .source
    {
        SignalSource::Drawn(blocks) => Ok(blocks),
        SignalSource::Notes(_) => Err(CommandError::WrongSignalKind { id }),
    }
}

/// The signal a layer listens to, if any, must exist.
fn check_signal(
    signal: Option<SignalId>,
    signals: &BTreeMap<SignalId, Signal>,
) -> Result<(), CommandError> {
    match signal {
        Some(id) if !signals.contains_key(&id) => Err(CommandError::UnknownSignal { id }),
        _ => Ok(()),
    }
}

fn check_color(color: u8) -> Result<(), CommandError> {
    if color < PALETTE_SIZE {
        Ok(())
    } else {
        Err(CommandError::ColorOutOfRange { color })
    }
}

#[derive(Debug, Default)]
struct UsedIds {
    nodes: HashSet<NodeId>,
    clips: HashSet<ClipId>,
    devices: HashSet<DeviceId>,
    layers: HashSet<LayerId>,
    blocks: HashSet<BlockId>,
}

impl UsedIds {
    fn claim_node(&mut self, id: NodeId) -> Result<(), CommandError> {
        if self.nodes.insert(id) {
            Ok(())
        } else {
            Err(CommandError::DuplicateNode { id })
        }
    }

    fn claim_clip(&mut self, id: ClipId) -> Result<(), CommandError> {
        if self.clips.insert(id) {
            Ok(())
        } else {
            Err(CommandError::DuplicateClip { id })
        }
    }

    fn claim_device(&mut self, id: DeviceId) -> Result<(), CommandError> {
        if self.devices.insert(id) {
            Ok(())
        } else {
            Err(CommandError::DuplicateDevice { id })
        }
    }

    fn claim_layer(&mut self, id: LayerId) -> Result<(), CommandError> {
        if self.layers.insert(id) {
            Ok(())
        } else {
            Err(CommandError::DuplicateLayer { id })
        }
    }

    fn claim_block(&mut self, id: BlockId) -> Result<(), CommandError> {
        if self.blocks.insert(id) {
            Ok(())
        } else {
            Err(CommandError::DuplicateBlock { id })
        }
    }
}

impl Project {
    /// Applies `edit` and returns the edit that reverts it.
    ///
    /// A rejected edit leaves the project unchanged. Each step is checked
    /// before it changes anything; what only a whole edit can break (clips
    /// on a node overlapping, which moving several clips may do
    /// halfway through a batch) is checked after it, reverting it if so.
    pub fn apply(&mut self, edit: Edit) -> Result<Edit, CommandError> {
        let places_clips = places_clips(&edit);
        let inverse = self.apply_step(edit)?;
        if places_clips && let Err(e) = check_unstacked(NodeId::ROOT, &self.root) {
            if let Err(undo) = self.apply_step(inverse) {
                unreachable!("an inverse edit was rejected: {undo}");
            }
            return Err(e);
        }
        Ok(inverse)
    }

    fn apply_step(&mut self, edit: Edit) -> Result<Edit, CommandError> {
        match edit {
            Edit::Batch(edits) => {
                let mut inverses = Vec::with_capacity(edits.len());
                for edit in edits {
                    match self.apply_step(edit) {
                        Ok(inverse) => inverses.push(inverse),
                        Err(e) => {
                            // Undo what was applied, newest first: each
                            // inverse restores exactly what came before.
                            for inverse in inverses.into_iter().rev() {
                                if let Err(undo) = self.apply_step(inverse) {
                                    unreachable!("an inverse edit was rejected: {undo}");
                                }
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Edit::Batch(inverses))
            }
            Edit::RenameSong { name } => {
                let name = clean_name(&name)?;
                let previous = std::mem::replace(&mut self.name, name);
                Ok(Edit::RenameSong { name: previous })
            }
            Edit::RenameNode { node, name } => {
                let name = clean_name(&name)?;
                let target = self.node_or_err(node)?;
                let previous = std::mem::replace(&mut target.name, name);
                Ok(Edit::RenameNode {
                    node,
                    name: previous,
                })
            }
            Edit::SetNodeColor { node, color } => {
                check_color(color)?;
                let target = self.node_or_err(node)?;
                let previous = std::mem::replace(&mut target.color, color);
                Ok(Edit::SetNodeColor {
                    node,
                    color: previous,
                })
            }
            Edit::AddNode {
                id,
                parent,
                index,
                mut node,
            } => {
                let siblings = self
                    .node(parent)
                    .ok_or(CommandError::UnknownNode { id: parent })?
                    .children
                    .len();
                check_index(index, siblings)?;
                if count_nodes(&self.root) + count_nodes(&node) > MAX_NODES {
                    return Err(CommandError::TooManyNodes { max: MAX_NODES });
                }
                for (device, instrument) in subtree_instruments(&node) {
                    self.check_instrument_sample(device, instrument)?;
                }
                let parent_kind = self.node(parent).map_or(NodeKind::Midi, |p| p.kind);
                check_kinds(parent, parent_kind, id, &node)?;
                let mut used = self.used_ids();
                prepare_node(id, &mut node, &mut used, &self.signals, &self.samples)?;
                self.node_or_err(parent)?.children.insert(index, (id, node));
                Ok(Edit::RemoveNode { id })
            }
            Edit::RemoveNode { id } => {
                if id == NodeId::ROOT {
                    return Err(CommandError::RootNode);
                }
                let (parent, index) = self.parent_of(id).ok_or(CommandError::UnknownNode { id })?;
                let (_, node) = self.node_or_err(parent)?.children.remove(index);
                Ok(Edit::AddNode {
                    id,
                    parent,
                    index,
                    node,
                })
            }
            Edit::MoveNode { id, parent, index } => {
                if id == NodeId::ROOT {
                    return Err(CommandError::RootNode);
                }
                let (from_parent, from_index) =
                    self.parent_of(id).ok_or(CommandError::UnknownNode { id })?;
                let moving = self.node(id).ok_or(CommandError::UnknownNode { id })?;
                if find_node(moving, id, parent).is_some() {
                    return Err(CommandError::NodeInsideItself { id });
                }
                let parent_kind = self
                    .node(parent)
                    .ok_or(CommandError::UnknownNode { id: parent })?
                    .kind;
                check_kinds(parent, parent_kind, id, moving)?;
                let siblings = self
                    .node(parent)
                    .ok_or(CommandError::UnknownNode { id: parent })?
                    .children
                    .len();
                // Counted after the node is taken out.
                let siblings = siblings - usize::from(parent == from_parent);
                check_index(index, siblings)?;
                let entry = self.node_or_err(from_parent)?.children.remove(from_index);
                self.node_or_err(parent)?.children.insert(index, entry);
                Ok(Edit::MoveNode {
                    id,
                    parent: from_parent,
                    index: from_index,
                })
            }
            Edit::SetTempo { bpm } => {
                if !(MIN_TEMPO_BPM..=MAX_TEMPO_BPM).contains(&bpm) {
                    return Err(CommandError::TempoOutOfRange {
                        bpm,
                        min: MIN_TEMPO_BPM,
                        max: MAX_TEMPO_BPM,
                    });
                }
                let previous = std::mem::replace(&mut self.tempo_bpm, bpm);
                Ok(Edit::SetTempo { bpm: previous })
            }
            Edit::AddClip { node, id, clip } => {
                self.check_plays_clips(node)?;
                self.used_ids().claim_clip(id)?;
                validate_clip(&clip).map_err(|reason| CommandError::InvalidClip { reason })?;
                self.node_or_err(node)?.clips.insert(id, clip);
                Ok(Edit::RemoveClip { id })
            }
            Edit::RemoveClip { id } => {
                let node = self.clip_node(id).ok_or(CommandError::UnknownClip { id })?;
                let holder = self.node_or_err(node)?;
                if let Some(clip) = holder.audio_clips.remove(&id) {
                    return Ok(Edit::AddAudioClip { node, id, clip });
                }
                let clip = holder
                    .clips
                    .remove(&id)
                    .ok_or(CommandError::UnknownClip { id })?;
                Ok(Edit::AddClip { node, id, clip })
            }
            Edit::AddAudioClip { node, id, clip } => {
                self.check_takes_audio(node)?;
                self.used_ids().claim_clip(id)?;
                self.check_audio_clip(&clip)?;
                self.node_or_err(node)?.audio_clips.insert(id, clip);
                Ok(Edit::RemoveClip { id })
            }
            Edit::SetAudioClip { id, node, clip } => {
                let from = self.clip_node(id).ok_or(CommandError::UnknownClip { id })?;
                self.check_takes_audio(node)?;
                self.check_audio_clip(&clip)?;
                let previous = self
                    .node_or_err(from)?
                    .audio_clips
                    .remove(&id)
                    .ok_or(CommandError::UnknownClip { id })?;
                self.node_or_err(node)?.audio_clips.insert(id, clip);
                Ok(Edit::SetAudioClip {
                    id,
                    node: from,
                    clip: previous,
                })
            }
            Edit::SetClip { id, node, clip } => {
                let from = self.clip_node(id).ok_or(CommandError::UnknownClip { id })?;
                self.check_plays_clips(node)?;
                validate_clip(&clip).map_err(|reason| CommandError::InvalidClip { reason })?;
                let previous = self
                    .node_or_err(from)?
                    .clips
                    .remove(&id)
                    .ok_or(CommandError::UnknownClip { id })?;
                self.node_or_err(node)?.clips.insert(id, clip);
                Ok(Edit::SetClip {
                    id,
                    node: from,
                    clip: previous,
                })
            }
            Edit::SetMute { node, muted } => {
                let target = self.node_or_err(node)?;
                let previous = std::mem::replace(&mut target.muted, muted);
                Ok(Edit::SetMute {
                    node,
                    muted: previous,
                })
            }
            Edit::SetSolo { node, soloed } => {
                let target = self.node_or_err(node)?;
                let previous = std::mem::replace(&mut target.soloed, soloed);
                Ok(Edit::SetSolo {
                    node,
                    soloed: previous,
                })
            }
            Edit::SetParam { param, value } => {
                check_normalized(value)?;
                let state = self.param_mut(param)?;
                let previous = std::mem::replace(&mut state.base, value);
                Ok(Edit::SetParam {
                    param,
                    value: previous,
                })
            }
            Edit::AddEffect {
                node,
                id,
                index,
                mut effect,
            } => {
                let target = self
                    .node(node)
                    .ok_or(CommandError::UnknownNode { id: node })?;
                let len = target.effects.len();
                let mut used = self.used_ids();
                used.claim_device(id)?;
                if len >= MAX_EFFECTS {
                    return Err(CommandError::TooManyEffects { max: MAX_EFFECTS });
                }
                check_index(index, len)?;
                prepare_params(
                    &mut effect.params,
                    effect_params(effect.kind),
                    &mut used,
                    &self.signals,
                )?;
                self.node_or_err(node)?.effects.insert(index, (id, effect));
                Ok(Edit::RemoveEffect { id })
            }
            Edit::RemoveEffect { id } => {
                let node = self
                    .effect_node(id)
                    .ok_or(CommandError::UnknownDevice { id })?;
                let target = self.node_or_err(node)?;
                let index = target
                    .effects
                    .iter()
                    .position(|(e, _)| *e == id)
                    .ok_or(CommandError::UnknownDevice { id })?;
                let (_, effect) = target.effects.remove(index);
                Ok(Edit::AddEffect {
                    node,
                    id,
                    index,
                    effect,
                })
            }
            Edit::AddInstrument {
                node,
                id,
                index,
                mut instrument,
            } => {
                let target = self
                    .node(node)
                    .ok_or(CommandError::UnknownNode { id: node })?;
                if target.kind != NodeKind::Midi {
                    return Err(CommandError::InstrumentOnAudio { node });
                }
                let len = target.instruments.len();
                self.check_instrument_sample(id, &instrument)?;
                let mut used = self.used_ids();
                used.claim_device(id)?;
                if len >= MAX_INSTRUMENTS {
                    return Err(CommandError::TooManyInstruments {
                        max: MAX_INSTRUMENTS,
                    });
                }
                check_index(index, len)?;
                prepare_params(
                    &mut instrument.params,
                    instrument_params(instrument.kind),
                    &mut used,
                    &self.signals,
                )?;
                self.node_or_err(node)?
                    .instruments
                    .insert(index, (id, instrument));
                Ok(Edit::RemoveInstrument { id })
            }
            Edit::RemoveInstrument { id } => {
                let node = self
                    .instrument_node(id)
                    .ok_or(CommandError::UnknownDevice { id })?;
                let target = self.node_or_err(node)?;
                let index = target
                    .instruments
                    .iter()
                    .position(|(i, _)| *i == id)
                    .ok_or(CommandError::UnknownDevice { id })?;
                let (_, instrument) = target.instruments.remove(index);
                Ok(Edit::AddInstrument {
                    node,
                    id,
                    index,
                    instrument,
                })
            }
            Edit::AddSample { id, sample } => {
                if self.samples.contains_key(&id) {
                    return Err(CommandError::DuplicateSample { id });
                }
                check_sample_file(&sample.file)?;
                if sample.name.trim().is_empty() {
                    return Err(CommandError::EmptyName);
                }
                self.samples.insert(id, sample);
                Ok(Edit::RemoveSample { id })
            }
            Edit::RemoveSample { id } => {
                if !self.samples.contains_key(&id) {
                    return Err(CommandError::UnknownSample { id });
                }
                if self.sample_in_use(id) {
                    return Err(CommandError::SampleInUse { id });
                }
                let sample = self
                    .samples
                    .remove(&id)
                    .ok_or(CommandError::UnknownSample { id })?;
                Ok(Edit::AddSample { id, sample })
            }
            Edit::SetInstrumentSample { id, sample } => {
                let node = self
                    .instrument_node(id)
                    .ok_or(CommandError::UnknownDevice { id })?;
                let current = self
                    .node(node)
                    .and_then(|n| n.instrument(id))
                    .map(|(_, i)| i.clone())
                    .ok_or(CommandError::UnknownDevice { id })?;
                self.check_instrument_sample(
                    id,
                    &Instrument {
                        sample,
                        ..current.clone()
                    },
                )?;
                if current.kind != InstrumentKind::SingleSampler {
                    return Err(CommandError::NotASampler { id });
                }
                let target = self.node_or_err(node)?;
                let Some((_, instrument)) = target.instruments.iter_mut().find(|(i, _)| *i == id)
                else {
                    return Err(CommandError::UnknownDevice { id });
                };
                let previous = std::mem::replace(&mut instrument.sample, sample);
                Ok(Edit::SetInstrumentSample {
                    id,
                    sample: previous,
                })
            }
            Edit::SetInstrumentNote { id, note } => {
                if note > 127 {
                    return Err(CommandError::NoteOutOfRange { note });
                }
                let node = self
                    .instrument_node(id)
                    .ok_or(CommandError::UnknownDevice { id })?;
                let target = self.node_or_err(node)?;
                let Some((_, instrument)) = target.instruments.iter_mut().find(|(i, _)| *i == id)
                else {
                    return Err(CommandError::UnknownDevice { id });
                };
                if instrument.kind != InstrumentKind::SingleSampler {
                    return Err(CommandError::NotASampler { id });
                }
                let previous = instrument
                    .note
                    .replace(note)
                    .unwrap_or(daw_protocol::DEFAULT_TRIGGER_NOTE);
                Ok(Edit::SetInstrumentNote { id, note: previous })
            }
            Edit::SetDeviceEnabled { id, enabled } => {
                let node = self
                    .effect_node(id)
                    .or_else(|| self.instrument_node(id))
                    .ok_or(CommandError::UnknownDevice { id })?;
                let target = self.node_or_err(node)?;
                let switch = if let Some((_, e)) = target.effects.iter_mut().find(|(e, _)| *e == id)
                {
                    &mut e.enabled
                } else if let Some((_, i)) = target.instruments.iter_mut().find(|(i, _)| *i == id) {
                    &mut i.enabled
                } else {
                    return Err(CommandError::UnknownDevice { id });
                };
                let previous = std::mem::replace(switch, enabled);
                Ok(Edit::SetDeviceEnabled {
                    id,
                    enabled: previous,
                })
            }
            Edit::AddLayer {
                param,
                id,
                index,
                mut layer,
            } => {
                let stack_len = self
                    .param(param)
                    .ok_or(CommandError::UnknownParam { param })?
                    .layers
                    .len();
                check_index(index, stack_len)?;
                prepare_layer(id, &mut layer, &mut self.used_ids(), &self.signals)?;
                self.param_mut(param)?.layers.insert(index, (id, layer));
                Ok(Edit::RemoveLayer { id })
            }
            Edit::RemoveLayer { id } => {
                let (param, index) = self
                    .find_layer(id)
                    .ok_or(CommandError::UnknownLayer { id })?;
                let (_, layer) = self.param_mut(param)?.layers.remove(index);
                Ok(Edit::AddLayer {
                    param,
                    id,
                    index,
                    layer,
                })
            }
            Edit::MoveLayer { id, index } => {
                let (param, from) = self
                    .find_layer(id)
                    .ok_or(CommandError::UnknownLayer { id })?;
                let stack = &mut self.param_mut(param)?.layers;
                if index >= stack.len() {
                    return Err(CommandError::IndexOutOfRange {
                        index,
                        len: stack.len(),
                    });
                }
                let layer = stack.remove(from);
                stack.insert(index, layer);
                Ok(Edit::MoveLayer { id, index: from })
            }
            Edit::RenameLayer { id, name } => {
                let name = clean_name(&name)?;
                let layer = self.layer_mut(id)?;
                let previous = std::mem::replace(&mut layer.name, name);
                Ok(Edit::RenameLayer { id, name: previous })
            }
            Edit::SetLayerMix { id, mix } => {
                check_normalized(mix)?;
                let layer = self.layer_mut(id)?;
                let previous = std::mem::replace(&mut layer.mix, mix);
                Ok(Edit::SetLayerMix { id, mix: previous })
            }
            Edit::SetLayerSignal { id, signal } => {
                check_signal(signal, &self.signals)?;
                let layer = self.layer_mut(id)?;
                let previous = std::mem::replace(&mut layer.signal, signal);
                Ok(Edit::SetLayerSignal {
                    id,
                    signal: previous,
                })
            }
            Edit::SetLayerOptions { id, mode, enabled } => {
                let layer = self.layer_mut(id)?;
                let previous = Edit::SetLayerOptions {
                    id,
                    mode: layer.mode,
                    enabled: layer.enabled,
                };
                layer.mode = mode;
                layer.enabled = enabled;
                Ok(previous)
            }
            Edit::AddBlock { layer, id, block } => {
                if self.find_layer(layer).is_none() {
                    return Err(CommandError::UnknownLayer { id: layer });
                }
                self.used_ids().claim_block(id)?;
                check_block(id, &block)?;
                let target = self.layer_mut(layer)?;
                check_no_overlap(&target.blocks, id, &block)?;
                insert_sorted(&mut target.blocks, id, block);
                Ok(Edit::RemoveBlock { id })
            }
            Edit::RemoveBlock { id } => {
                if let Some((param, layer_pos, layer, block_pos)) = self.find_block(id) {
                    let state = self.param_mut(param)?;
                    let (_, block) = state.layers[layer_pos].1.blocks.remove(block_pos);
                    return Ok(Edit::AddBlock { layer, id, block });
                }
                let (signal, position) = find_signal_block(&self.signals, id)
                    .ok_or(CommandError::UnknownBlock { id })?;
                let (_, block) = drawn_blocks(&mut self.signals, signal)?.remove(position);
                Ok(Edit::AddSignalBlock { signal, id, block })
            }
            Edit::SetBlock { id, block } => {
                check_block(id, &block)?;
                let (blocks, position) =
                    if let Some((param, layer_pos, _, position)) = self.find_block(id) {
                        (
                            &mut self.param_mut(param)?.layers[layer_pos].1.blocks,
                            position,
                        )
                    } else {
                        let (signal, position) = find_signal_block(&self.signals, id)
                            .ok_or(CommandError::UnknownBlock { id })?;
                        (drawn_blocks(&mut self.signals, signal)?, position)
                    };
                check_no_overlap(blocks, id, &block)?;
                let (_, previous) = blocks.remove(position);
                insert_sorted(blocks, id, block);
                Ok(Edit::SetBlock {
                    id,
                    block: previous,
                })
            }
            Edit::AddSignal {
                id,
                mut signal,
                listeners,
            } => {
                if self.signals.contains_key(&id) {
                    return Err(CommandError::DuplicateSignal { id });
                }
                signal.name = clean_name(&signal.name)?;
                check_color(signal.color)?;
                match &mut signal.source {
                    SignalSource::Drawn(blocks) => {
                        let mut used = self.used_ids();
                        blocks.sort_by_key(|(_, b)| b.start);
                        for (i, (block_id, block)) in blocks.iter().enumerate() {
                            used.claim_block(*block_id)?;
                            check_block(*block_id, block)?;
                            check_no_overlap(&blocks[..i], *block_id, block)?;
                        }
                    }
                    SignalSource::Notes(source) => validate_notes_source(source)
                        .map_err(|reason| CommandError::InvalidSignal { id, reason })?,
                }
                for &layer in &listeners {
                    let (param, position) = self
                        .find_layer(layer)
                        .ok_or(CommandError::UnknownLayer { id: layer })?;
                    let state = self
                        .param(param)
                        .ok_or(CommandError::UnknownParam { param })?;
                    if state.layers[position].1.signal.is_some() {
                        return Err(CommandError::AlreadyListening { layer });
                    }
                }
                for layer in listeners {
                    self.layer_mut(layer)?.signal = Some(id);
                }
                self.signals.insert(id, signal);
                Ok(Edit::RemoveSignal { id })
            }
            Edit::RemoveSignal { id } => {
                let signal = self
                    .signals
                    .remove(&id)
                    .ok_or(CommandError::UnknownSignal { id })?;
                // Listening layers go back to drawing their blocks.
                let listeners: Vec<LayerId> = self
                    .listeners()
                    .into_iter()
                    .filter(|(_, signal)| *signal == id)
                    .map(|(used, _)| used.layer)
                    .collect();
                for &layer in &listeners {
                    self.layer_mut(layer)?.signal = None;
                }
                Ok(Edit::AddSignal {
                    id,
                    signal,
                    listeners,
                })
            }
            Edit::RenameSignal { id, name } => {
                let name = clean_name(&name)?;
                let signal = self
                    .signals
                    .get_mut(&id)
                    .ok_or(CommandError::UnknownSignal { id })?;
                let previous = std::mem::replace(&mut signal.name, name);
                Ok(Edit::RenameSignal { id, name: previous })
            }
            Edit::SetSignalColor { id, color } => {
                check_color(color)?;
                let signal = self
                    .signals
                    .get_mut(&id)
                    .ok_or(CommandError::UnknownSignal { id })?;
                let previous = std::mem::replace(&mut signal.color, color);
                Ok(Edit::SetSignalColor {
                    id,
                    color: previous,
                })
            }
            Edit::SetNotesSource { id, source } => {
                let signal = self
                    .signals
                    .get_mut(&id)
                    .ok_or(CommandError::UnknownSignal { id })?;
                let SignalSource::Notes(current) = &mut signal.source else {
                    return Err(CommandError::WrongSignalKind { id });
                };
                validate_notes_source(&source)
                    .map_err(|reason| CommandError::InvalidSignal { id, reason })?;
                let previous = std::mem::replace(current, source);
                Ok(Edit::SetNotesSource {
                    id,
                    source: previous,
                })
            }
            Edit::AddSignalBlock { signal, id, block } => {
                drawn_blocks(&mut self.signals, signal)?;
                self.used_ids().claim_block(id)?;
                check_block(id, &block)?;
                let blocks = drawn_blocks(&mut self.signals, signal)?;
                check_no_overlap(blocks, id, &block)?;
                insert_sorted(blocks, id, block);
                Ok(Edit::RemoveBlock { id })
            }
        }
    }
}

/// Validates an incoming node and everything under it, claims its ids, and
/// fills empty parameter lists with defaults.
fn prepare_node(
    id: NodeId,
    node: &mut Node,
    used: &mut UsedIds,
    signals: &BTreeMap<SignalId, Signal>,
    samples: &BTreeMap<SampleId, SampleRef>,
) -> Result<(), CommandError> {
    used.claim_node(id)?;
    node.name = clean_name(&node.name)?;
    check_color(node.color)?;
    if node.instruments.len() > MAX_INSTRUMENTS {
        return Err(CommandError::TooManyInstruments {
            max: MAX_INSTRUMENTS,
        });
    }
    for (instrument_id, instrument) in &mut node.instruments {
        used.claim_device(*instrument_id)?;
        prepare_params(
            &mut instrument.params,
            instrument_params(instrument.kind),
            used,
            signals,
        )?;
    }
    prepare_params(&mut node.channel, &CHANNEL_PARAMS, used, signals)?;
    if node.effects.len() > MAX_EFFECTS {
        return Err(CommandError::TooManyEffects { max: MAX_EFFECTS });
    }
    for (effect_id, effect) in &mut node.effects {
        used.claim_device(*effect_id)?;
        prepare_params(
            &mut effect.params,
            effect_params(effect.kind),
            used,
            signals,
        )?;
    }
    if node.kind == NodeKind::Audio && !node.instruments.is_empty() {
        return Err(CommandError::InstrumentOnAudio { node: id });
    }
    if node.kind == NodeKind::Audio && !node.clips.is_empty() {
        return Err(CommandError::NotMidi { node: id });
    }
    if node.kind == NodeKind::Midi && !node.audio_clips.is_empty() {
        return Err(CommandError::NotAudio { node: id });
    }
    for (clip_id, clip) in &node.audio_clips {
        used.claim_clip(*clip_id)?;
        validate_audio_clip(clip).map_err(|reason| CommandError::InvalidClip { reason })?;
        if !samples.contains_key(&clip.sample) {
            return Err(CommandError::UnknownSample { id: clip.sample });
        }
    }
    for (clip_id, clip) in &node.clips {
        used.claim_clip(*clip_id)?;
        validate_clip(clip).map_err(|reason| CommandError::InvalidClip { reason })?;
    }
    for (child_id, child) in &mut node.children {
        prepare_node(*child_id, child, used, signals, samples)?;
    }
    Ok(())
}

/// Validates a device's incoming parameters (empty means defaults) and
/// claims their layer and block ids.
fn prepare_params(
    params: &mut Vec<ParamState>,
    infos: &[ParamInfo],
    used: &mut UsedIds,
    signals: &BTreeMap<SignalId, Signal>,
) -> Result<(), CommandError> {
    if params.is_empty() {
        *params = default_params(infos);
    }
    if params.len() != infos.len() {
        return Err(CommandError::WrongParamCount {
            expected: infos.len(),
            found: params.len(),
        });
    }
    for state in params {
        check_normalized(state.base)?;
        for (layer_id, layer) in &mut state.layers {
            prepare_layer(*layer_id, layer, used, signals)?;
        }
    }
    Ok(())
}

/// A name with surrounding whitespace removed; empty names are rejected.
fn clean_name(name: &str) -> Result<String, CommandError> {
    let name = name.trim();
    if name.is_empty() {
        Err(CommandError::EmptyName)
    } else {
        Ok(name.to_owned())
    }
}

fn check_normalized(value: f64) -> Result<(), CommandError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(CommandError::ValueOutOfRange { value })
    }
}

fn check_index(index: usize, len: usize) -> Result<(), CommandError> {
    if index <= len {
        Ok(())
    } else {
        Err(CommandError::IndexOutOfRange { index, len })
    }
}

fn check_block(id: BlockId, block: &Block) -> Result<(), CommandError> {
    validate_block(block).map_err(|reason| CommandError::InvalidBlock { id, reason })
}

/// Checks that `block` (with id `id`) would not overlap any other block in
/// `blocks`. A block with the same id is ignored, since it is being replaced.
fn check_no_overlap(
    blocks: &[(BlockId, Block)],
    id: BlockId,
    block: &Block,
) -> Result<(), CommandError> {
    match blocks.iter().find(|(other_id, other)| {
        *other_id != id && other.start < block.end() && block.start < other.end()
    }) {
        Some((other, _)) => Err(CommandError::OverlappingBlocks { id, other: *other }),
        None => Ok(()),
    }
}

fn insert_sorted(blocks: &mut Vec<(BlockId, Block)>, id: BlockId, block: Block) {
    let index = blocks.partition_point(|(_, b)| b.start < block.start);
    blocks.insert(index, (id, block));
}

/// Validates an incoming layer, claims its ids, and sorts its blocks.
fn prepare_layer(
    id: LayerId,
    layer: &mut Layer,
    used: &mut UsedIds,
    signals: &BTreeMap<SignalId, Signal>,
) -> Result<(), CommandError> {
    used.claim_layer(id)?;
    check_normalized(layer.mix)?;
    check_signal(layer.signal, signals)?;
    layer.blocks.sort_by_key(|(_, b)| b.start);
    for (i, (block_id, block)) in layer.blocks.iter().enumerate() {
        used.claim_block(*block_id)?;
        check_block(*block_id, block)?;
        check_no_overlap(&layer.blocks[..i], *block_id, block)?;
    }
    Ok(())
}

fn validate_audio_clip(clip: &AudioClip) -> Result<(), InvalidClip> {
    if clip.start.0 < 0 {
        return Err(InvalidClip::NegativeStart);
    }
    if clip.length.0 <= 0 {
        return Err(InvalidClip::EmptyLength);
    }
    if !(clip.offset >= 0.0 && clip.offset.is_finite()) {
        return Err(InvalidClip::Offset);
    }
    if !(MIN_CLIP_BPM..=MAX_CLIP_BPM).contains(&clip.bpm) {
        return Err(InvalidClip::Bpm);
    }
    if !(MIN_CLIP_GAIN_DB..=MAX_CLIP_GAIN_DB).contains(&clip.gain_db) {
        return Err(InvalidClip::Gain);
    }
    if clip.fade_in.0 < 0 || clip.fade_out.0 < 0 || clip.fade_in + clip.fade_out > clip.length {
        return Err(InvalidClip::Fades);
    }
    validate_loop(clip.looping)
}

/// A loop needs a cycle, and a phase inside it.
fn validate_loop(looping: Option<daw_protocol::ClipLoop>) -> Result<(), InvalidClip> {
    match looping {
        Some(l) if l.cycle.0 <= 0 || l.phase.0 < 0 || l.phase >= l.cycle => Err(InvalidClip::Loop),
        _ => Ok(()),
    }
}

/// Whether `edit` can put clips somewhere (so overlaps need checking).
fn places_clips(edit: &Edit) -> bool {
    match edit {
        Edit::Batch(edits) => edits.iter().any(places_clips),
        Edit::AddClip { .. }
        | Edit::SetClip { .. }
        | Edit::AddAudioClip { .. }
        | Edit::SetAudioClip { .. }
        | Edit::AddNode { .. } => true,
        _ => false,
    }
}

/// Fails if two clips (MIDI or audio: a node holds one kind) on any node
/// from `node` down overlap.
fn check_unstacked(id: NodeId, node: &Node) -> Result<(), CommandError> {
    let mut spans: Vec<(daw_protocol::Ticks, daw_protocol::Ticks, ClipId)> = node
        .clips
        .iter()
        .map(|(clip_id, c)| (c.start, c.start + c.length, *clip_id))
        .chain(
            node.audio_clips
                .iter()
                .map(|(clip_id, c)| (c.start, c.start + c.length, *clip_id)),
        )
        .collect();
    spans.sort();
    for pair in spans.windows(2) {
        let ((_, end, clip), (start, _, over)) = (pair[0], pair[1]);
        if start < end {
            return Err(CommandError::StackedClips {
                node: id,
                clip,
                over,
            });
        }
    }
    for (child_id, child) in &node.children {
        check_unstacked(*child_id, child)?;
    }
    Ok(())
}

fn validate_clip(clip: &MidiClip) -> Result<(), InvalidClip> {
    if clip.start.0 < 0 {
        return Err(InvalidClip::NegativeStart);
    }
    if clip.length.0 <= 0 {
        return Err(InvalidClip::EmptyLength);
    }
    for (index, note) in clip.notes.iter().enumerate() {
        if note.pitch > 127 {
            return Err(InvalidClip::NotePitch { index });
        }
        if !(1..=127).contains(&note.velocity) {
            return Err(InvalidClip::NoteVelocity { index });
        }
        if note.length.0 <= 0 {
            return Err(InvalidClip::NoteLength { index });
        }
    }
    validate_loop(clip.looping)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{channel, filter, instrument};
    use daw_protocol::{
        BlendMode, EffectKind, InvalidBlock, InvalidSignal, MidiNote, NotesSource, Shape, Ticks,
    };

    const CUTOFF: ParamId = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Instrument(FIRST_INSTRUMENT),
        index: instrument::CUTOFF,
    };
    const VOLUME: ParamId = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Channel,
        index: channel::VOLUME,
    };

    /// The node a new project starts with.
    fn synth(project: &Project) -> &Node {
        project.node(FIRST_NODE).unwrap()
    }

    fn clip(notes: Vec<MidiNote>) -> MidiClip {
        MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_bars(1),
            notes,
            looping: None,
        }
    }

    fn note(pitch: u8, velocity: u8, length: i64) -> MidiNote {
        MidiNote {
            start: Ticks::ZERO,
            length: Ticks(length),
            pitch,
            velocity,
        }
    }

    fn block(start: i64, length: i64) -> Block {
        Block::new(
            Ticks::from_beats(start),
            Ticks::from_beats(length),
            Shape::ramp(0.0, 1.0),
        )
    }

    fn add_layer(param: ParamId, id: u64) -> Edit {
        Edit::AddLayer {
            param,
            id: LayerId(id),
            index: 0,
            layer: Layer::new(BlendMode::Add),
        }
    }

    fn add_block(layer: u64, id: u64, block: Block) -> Edit {
        Edit::AddBlock {
            layer: LayerId(layer),
            id: BlockId(id),
            block,
        }
    }

    /// A project with a filter effect (device 1) and a layer (id 1) on the
    /// synth cutoff holding one block (id 1) over beats 0..4.
    fn populated() -> Project {
        let mut project = Project::default();
        for edit in [
            Edit::AddEffect {
                node: FIRST_NODE,
                id: DeviceId(1),
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            },
            add_layer(CUTOFF, 1),
            add_block(1, 1, block(0, 4)),
        ] {
            project.apply(edit).unwrap();
        }
        project
    }

    /// Applies `edit` to `project`, checks it changed something, then checks
    /// its inverse restores the original.
    fn assert_round_trip(mut project: Project, edit: Edit) {
        let original = project.clone();
        let inverse = project.apply(edit).unwrap();
        assert_ne!(project, original);
        project.apply(inverse).unwrap();
        assert_eq!(project, original);
    }

    fn assert_rejected(project: &mut Project, edit: Edit) -> CommandError {
        let before = project.clone();
        let err = project.apply(edit).unwrap_err();
        assert_eq!(*project, before, "rejected edit changed the project");
        err
    }

    const DUCK: SignalId = SignalId(1);
    const SWEEP: SignalId = SignalId(2);

    fn kick_envelope() -> NotesSource {
        NotesSource {
            pitch_range: true,
            lowest: 36,
            highest: 36,
            ..NotesSource::new(FIRST_NODE)
        }
    }

    fn add_signal(id: SignalId, signal: Signal) -> Edit {
        Edit::AddSignal {
            id,
            signal,
            listeners: Vec::new(),
        }
    }

    fn listens_to(project: &Project, layer: u64) -> Option<SignalId> {
        let (param, position) = project.find_layer(LayerId(layer)).unwrap();
        project.param(param).unwrap().layers[position].1.signal
    }

    fn notes_signal() -> Signal {
        Signal {
            name: "Kick duck".to_owned(),
            color: 0,
            source: SignalSource::Notes(kick_envelope()),
        }
    }

    /// A drawn signal holding block 20 over beats 0..4.
    fn drawn_signal() -> Signal {
        Signal {
            name: "Sweep".to_owned(),
            color: 5,
            source: SignalSource::Drawn(vec![(BlockId(20), block(0, 4))]),
        }
    }

    /// `populated()` plus a notes signal (1), a drawn signal (2), and layer
    /// 2 on the volume listening to the notes signal.
    fn with_signals() -> Project {
        let mut project = populated();
        for edit in [
            add_signal(DUCK, notes_signal()),
            add_signal(SWEEP, drawn_signal()),
            Edit::AddLayer {
                param: VOLUME,
                id: LayerId(2),
                index: 0,
                layer: Layer::new(BlendMode::Multiply).listening(DUCK),
            },
            add_block(2, 2, block(8, 4)),
        ] {
            project.apply(edit).unwrap();
        }
        project
    }

    #[test]
    fn layer_mix_stays_within_range() {
        let mut project = populated();
        let err = assert_rejected(
            &mut project,
            Edit::SetLayerMix {
                id: LayerId(1),
                mix: 1.5,
            },
        );
        assert_eq!(err, CommandError::ValueOutOfRange { value: 1.5 });
        let mut layer = Layer::new(BlendMode::Add);
        layer.mix = -0.1;
        let err = assert_rejected(
            &mut project,
            Edit::AddLayer {
                param: VOLUME,
                id: LayerId(9),
                index: 0,
                layer,
            },
        );
        assert_eq!(err, CommandError::ValueOutOfRange { value: -0.1 });
    }

    #[test]
    fn every_signal_edit_is_reverted_by_its_inverse() {
        let edits = [
            add_signal(SignalId(3), notes_signal()),
            Edit::AddSignal {
                id: SignalId(3),
                signal: drawn_signal_without_blocks(),
                listeners: vec![LayerId(1)],
            },
            Edit::RemoveSignal { id: SWEEP },
            Edit::RemoveSignal { id: DUCK },
            Edit::RenameSignal {
                id: DUCK,
                name: "Pump".to_owned(),
            },
            Edit::SetSignalColor { id: DUCK, color: 8 },
            Edit::SetNotesSource {
                id: DUCK,
                source: NotesSource {
                    decay: Ticks(960),
                    ..kick_envelope()
                },
            },
            Edit::AddSignalBlock {
                signal: SWEEP,
                id: BlockId(21),
                block: block(8, 2),
            },
            Edit::SetBlock {
                id: BlockId(20),
                block: block(1, 2),
            },
            Edit::RemoveBlock { id: BlockId(20) },
            Edit::SetLayerSignal {
                id: LayerId(1),
                signal: Some(SWEEP),
            },
            Edit::SetLayerSignal {
                id: LayerId(2),
                signal: None,
            },
        ];
        for edit in edits {
            assert_round_trip(with_signals(), edit);
        }
    }

    fn drawn_signal_without_blocks() -> Signal {
        Signal {
            source: SignalSource::Drawn(Vec::new()),
            ..drawn_signal()
        }
    }

    #[test]
    fn removing_a_signal_sends_its_listeners_back_to_drawing() {
        let mut project = with_signals();
        let inverse = project.apply(Edit::RemoveSignal { id: DUCK }).unwrap();
        assert_eq!(listens_to(&project, 2), None);
        let (param, position) = project.find_layer(LayerId(2)).unwrap();
        let layer = &project.param(param).unwrap().layers[position].1;
        assert_eq!(layer.blocks.len(), 1, "blocks kept");
        project.apply(inverse).unwrap();
        assert_eq!(listens_to(&project, 2), Some(DUCK), "undo restores it");
    }

    #[test]
    fn layers_listen_only_to_existing_signals() {
        let mut project = with_signals();
        let missing = SignalId(9);
        let err = assert_rejected(
            &mut project,
            Edit::SetLayerSignal {
                id: LayerId(1),
                signal: Some(missing),
            },
        );
        assert_eq!(err, CommandError::UnknownSignal { id: missing });
        let err = assert_rejected(
            &mut project,
            Edit::AddLayer {
                param: VOLUME,
                id: LayerId(30),
                index: 0,
                layer: Layer::new(BlendMode::Add).listening(missing),
            },
        );
        assert_eq!(err, CommandError::UnknownSignal { id: missing });
        let err = assert_rejected(
            &mut project,
            Edit::AddSignal {
                id: SignalId(3),
                signal: notes_signal(),
                listeners: vec![LayerId(1), LayerId(99)],
            },
        );
        assert_eq!(err, CommandError::UnknownLayer { id: LayerId(99) });
        // Taking a layer from another signal would lose that on undo.
        let err = assert_rejected(
            &mut project,
            Edit::AddSignal {
                id: SignalId(3),
                signal: notes_signal(),
                listeners: vec![LayerId(2)],
            },
        );
        assert_eq!(err, CommandError::AlreadyListening { layer: LayerId(2) });
    }

    #[test]
    fn switching_a_layer_keeps_its_blocks() {
        let mut project = with_signals();
        project
            .apply(Edit::SetLayerSignal {
                id: LayerId(1),
                signal: Some(SWEEP),
            })
            .unwrap();
        assert_eq!(listens_to(&project, 1), Some(SWEEP));
        let layer =
            &synth(&project).instruments[0].1.params[instrument::CUTOFF as usize].layers[0].1;
        assert_eq!(layer.blocks[0].1, block(0, 4));
    }

    #[test]
    fn signal_edits_check_the_kind_of_signal() {
        let mut project = with_signals();
        let err = assert_rejected(
            &mut project,
            Edit::AddSignalBlock {
                signal: DUCK,
                id: BlockId(21),
                block: block(0, 1),
            },
        );
        assert_eq!(err, CommandError::WrongSignalKind { id: DUCK });
        let err = assert_rejected(
            &mut project,
            Edit::SetNotesSource {
                id: SWEEP,
                source: kick_envelope(),
            },
        );
        assert_eq!(err, CommandError::WrongSignalKind { id: SWEEP });
    }

    #[test]
    fn signal_blocks_share_the_block_id_space_and_never_overlap() {
        let mut project = with_signals();
        let err = assert_rejected(&mut project, add_block(1, 20, block(30, 1)));
        assert_eq!(err, CommandError::DuplicateBlock { id: BlockId(20) });
        let err = assert_rejected(
            &mut project,
            Edit::AddSignalBlock {
                signal: SWEEP,
                id: BlockId(1),
                block: block(10, 1),
            },
        );
        assert_eq!(err, CommandError::DuplicateBlock { id: BlockId(1) });
        let err = assert_rejected(
            &mut project,
            Edit::AddSignalBlock {
                signal: SWEEP,
                id: BlockId(21),
                block: block(2, 4),
            },
        );
        assert_eq!(
            err,
            CommandError::OverlappingBlocks {
                id: BlockId(21),
                other: BlockId(20)
            }
        );
    }

    #[test]
    fn bad_signals_are_rejected() {
        let mut project = with_signals();
        let err = assert_rejected(&mut project, add_signal(DUCK, notes_signal()));
        assert_eq!(err, CommandError::DuplicateSignal { id: DUCK });
        let mut bad = notes_signal();
        bad.color = 9;
        let err = assert_rejected(&mut project, add_signal(SignalId(3), bad));
        assert_eq!(err, CommandError::ColorOutOfRange { color: 9 });
        let mut bad = notes_signal();
        bad.name = "  ".to_owned();
        let err = assert_rejected(&mut project, add_signal(SignalId(3), bad));
        assert_eq!(err, CommandError::EmptyName);
        let err = assert_rejected(
            &mut project,
            Edit::SetNotesSource {
                id: DUCK,
                source: NotesSource {
                    sustain: 2.0,
                    ..kick_envelope()
                },
            },
        );
        assert_eq!(
            err,
            CommandError::InvalidSignal {
                id: DUCK,
                reason: InvalidSignal::EnvelopeLevel
            }
        );
        let mut bad = block(20, 1);
        bad.depth = -1.0;
        let err = assert_rejected(
            &mut project,
            Edit::AddSignalBlock {
                signal: SWEEP,
                id: BlockId(21),
                block: bad,
            },
        );
        assert!(matches!(
            err,
            CommandError::InvalidBlock {
                reason: InvalidBlock::Depth,
                ..
            }
        ));
    }

    #[test]
    fn listeners_list_every_listening_layer() {
        let mut project = with_signals();
        project
            .apply(Edit::SetLayerSignal {
                id: LayerId(1),
                signal: Some(DUCK),
            })
            .unwrap();
        let listeners: Vec<_> = project.listeners();
        assert_eq!(
            listeners,
            [
                (
                    SignalUse {
                        param: CUTOFF,
                        layer: LayerId(1),
                    },
                    DUCK
                ),
                (
                    SignalUse {
                        param: VOLUME,
                        layer: LayerId(2),
                    },
                    DUCK
                )
            ]
        );
    }

    #[test]
    fn every_edit_is_reverted_by_its_inverse() {
        let edits = [
            Edit::SetTempo { bpm: 140.0 },
            Edit::RenameSong {
                name: "Night Drive".to_owned(),
            },
            Edit::RenameNode {
                node: FIRST_NODE,
                name: "Lead".to_owned(),
            },
            Edit::SetMute {
                node: FIRST_NODE,
                muted: true,
            },
            Edit::SetSolo {
                node: FIRST_NODE,
                soloed: true,
            },
            Edit::AddClip {
                node: FIRST_NODE,
                id: ClipId(1),
                clip: clip(vec![note(60, 100, 240)]),
            },
            Edit::SetParam {
                param: VOLUME,
                value: 0.5,
            },
            Edit::SetParam {
                param: ParamId {
                    node: FIRST_NODE,
                    device: DeviceRef::Effect(DeviceId(1)),
                    index: filter::CUTOFF,
                },
                value: 0.2,
            },
            Edit::AddEffect {
                node: FIRST_NODE,
                id: DeviceId(2),
                index: 1,
                effect: Effect::new(EffectKind::Filter),
            },
            Edit::RemoveEffect { id: DeviceId(1) },
            add_layer(VOLUME, 2),
            Edit::RemoveLayer { id: LayerId(1) },
            Edit::SetLayerOptions {
                id: LayerId(1),
                mode: BlendMode::Multiply,
                enabled: false,
            },
            Edit::SetLayerMix {
                id: LayerId(1),
                mix: 0.25,
            },
            Edit::RenameLayer {
                id: LayerId(1),
                name: "Wobble".to_owned(),
            },
            add_block(1, 2, block(4, 2)),
            Edit::RemoveBlock { id: BlockId(1) },
            Edit::SetBlock {
                id: BlockId(1),
                block: block(8, 1),
            },
        ];
        for edit in edits {
            assert_round_trip(populated(), edit);
        }
    }

    #[test]
    fn removing_an_effect_takes_its_layers_and_undo_brings_them_back() {
        let mut project = populated();
        let filter_cutoff = ParamId {
            node: FIRST_NODE,
            device: DeviceRef::Effect(DeviceId(1)),
            index: filter::CUTOFF,
        };
        project.apply(add_layer(filter_cutoff, 9)).unwrap();
        let inverse = project
            .apply(Edit::RemoveEffect { id: DeviceId(1) })
            .unwrap();
        assert_eq!(project.find_layer(LayerId(9)), None);
        project.apply(inverse).unwrap();
        assert_eq!(project.find_layer(LayerId(9)), Some((filter_cutoff, 0)));
    }

    #[test]
    fn new_effects_start_at_their_defaults() {
        let project = populated();
        let (_, effect) = &synth(&project).effects[0];
        assert_eq!(effect.params.len(), 3);
        assert!(effect.params[filter::CUTOFF as usize].base > 0.99);
    }

    #[test]
    fn layers_insert_at_their_index() {
        let mut project = populated();
        project
            .apply(Edit::AddLayer {
                param: CUTOFF,
                id: LayerId(2),
                index: 1,
                layer: Layer::new(BlendMode::Add),
            })
            .unwrap();
        let ids: Vec<_> = synth(&project).instruments[0].1.params[instrument::CUTOFF as usize]
            .layers
            .iter()
            .map(|(id, _)| id.0)
            .collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn blocks_stay_sorted_by_start() {
        let mut project = populated();
        project.apply(add_block(1, 2, block(10, 1))).unwrap();
        project.apply(add_block(1, 3, block(5, 1))).unwrap();
        project
            .apply(Edit::SetBlock {
                id: BlockId(1),
                block: block(20, 1),
            })
            .unwrap();
        let layer =
            &synth(&project).instruments[0].1.params[instrument::CUTOFF as usize].layers[0].1;
        let starts: Vec<_> = layer.blocks.iter().map(|(_, b)| b.start).collect();
        assert_eq!(
            starts,
            [
                Ticks::from_beats(5),
                Ticks::from_beats(10),
                Ticks::from_beats(20)
            ]
        );
    }

    #[test]
    fn blocks_in_a_layer_cannot_overlap() {
        let mut project = populated();
        assert_eq!(
            assert_rejected(&mut project, add_block(1, 2, block(3, 2))),
            CommandError::OverlappingBlocks {
                id: BlockId(2),
                other: BlockId(1)
            }
        );
        // Touching end to start is fine.
        project.apply(add_block(1, 2, block(4, 2))).unwrap();
        // Moving a block onto itself is fine; onto another is not.
        project
            .apply(Edit::SetBlock {
                id: BlockId(1),
                block: block(1, 3),
            })
            .unwrap();
        assert!(matches!(
            assert_rejected(
                &mut project,
                Edit::SetBlock {
                    id: BlockId(1),
                    block: block(1, 4),
                }
            ),
            CommandError::OverlappingBlocks { .. }
        ));
    }

    #[test]
    fn incoming_layers_are_validated_whole() {
        let mut project = populated();
        let mut layer = Layer::new(BlendMode::Add);
        layer.blocks = vec![(BlockId(7), block(0, 2)), (BlockId(8), block(1, 2))];
        assert!(matches!(
            assert_rejected(
                &mut project,
                Edit::AddLayer {
                    param: VOLUME,
                    id: LayerId(5),
                    index: 0,
                    layer: layer.clone(),
                }
            ),
            CommandError::OverlappingBlocks { .. }
        ));
        layer.blocks[1] = (BlockId(1), block(4, 1));
        assert_eq!(
            assert_rejected(
                &mut project,
                Edit::AddLayer {
                    param: VOLUME,
                    id: LayerId(5),
                    index: 0,
                    layer,
                }
            ),
            CommandError::DuplicateBlock { id: BlockId(1) }
        );
    }

    #[test]
    fn ids_must_be_unique_and_known() {
        let mut project = populated();
        let cases = [
            (
                add_layer(VOLUME, 1),
                CommandError::DuplicateLayer { id: LayerId(1) },
            ),
            (
                add_block(1, 1, block(9, 1)),
                CommandError::DuplicateBlock { id: BlockId(1) },
            ),
            (
                add_block(99, 2, block(9, 1)),
                CommandError::UnknownLayer { id: LayerId(99) },
            ),
            (
                Edit::RemoveBlock { id: BlockId(99) },
                CommandError::UnknownBlock { id: BlockId(99) },
            ),
            (
                Edit::RemoveLayer { id: LayerId(99) },
                CommandError::UnknownLayer { id: LayerId(99) },
            ),
            (
                Edit::RemoveEffect { id: DeviceId(99) },
                CommandError::UnknownDevice { id: DeviceId(99) },
            ),
            (
                Edit::AddEffect {
                    node: FIRST_NODE,
                    id: DeviceId(1),
                    index: 0,
                    effect: Effect::new(EffectKind::Filter),
                },
                CommandError::DuplicateDevice { id: DeviceId(1) },
            ),
        ];
        for (edit, expected) in cases {
            assert_eq!(assert_rejected(&mut project, edit), expected);
        }
    }

    #[test]
    fn parameters_must_exist() {
        let mut project = populated();
        for param in [
            ParamId {
                node: FIRST_NODE,
                device: DeviceRef::Instrument(FIRST_INSTRUMENT),
                index: 99,
            },
            ParamId {
                node: FIRST_NODE,
                device: DeviceRef::Effect(DeviceId(99)),
                index: 0,
            },
        ] {
            assert_eq!(
                assert_rejected(&mut project, Edit::SetParam { param, value: 0.5 }),
                CommandError::UnknownParam { param }
            );
            assert_eq!(
                assert_rejected(&mut project, add_layer(param, 50)),
                CommandError::UnknownParam { param }
            );
        }
    }

    #[test]
    fn values_and_positions_are_range_checked() {
        let mut project = populated();
        for value in [-0.1, 1.1, f64::NAN] {
            assert!(matches!(
                assert_rejected(
                    &mut project,
                    Edit::SetParam {
                        param: VOLUME,
                        value
                    }
                ),
                CommandError::ValueOutOfRange { .. }
            ));
        }
        assert_eq!(
            assert_rejected(
                &mut project,
                Edit::AddLayer {
                    param: CUTOFF,
                    id: LayerId(5),
                    index: 2,
                    layer: Layer::new(BlendMode::Add),
                }
            ),
            CommandError::IndexOutOfRange { index: 2, len: 1 }
        );
        let mut bad = block(10, 1);
        bad.depth = 2.0;
        assert_eq!(
            assert_rejected(&mut project, add_block(1, 2, bad)),
            CommandError::InvalidBlock {
                id: BlockId(2),
                reason: InvalidBlock::Depth
            }
        );
    }

    #[test]
    fn effect_chain_has_a_limit() {
        let mut project = Project::default();
        for i in 1..=MAX_EFFECTS as u64 {
            project
                .apply(Edit::AddEffect {
                    node: FIRST_NODE,
                    id: DeviceId(i),
                    index: 0,
                    effect: Effect::new(EffectKind::Filter),
                })
                .unwrap();
        }
        assert_eq!(
            assert_rejected(
                &mut project,
                Edit::AddEffect {
                    node: FIRST_NODE,
                    id: DeviceId(99),
                    index: 0,
                    effect: Effect::new(EffectKind::Filter),
                }
            ),
            CommandError::TooManyEffects { max: MAX_EFFECTS }
        );
    }

    #[test]
    fn out_of_range_tempo_is_rejected() {
        let mut project = Project::default();
        for bpm in [0.0, 19.9, 1000.0, f64::NAN, f64::INFINITY] {
            assert_rejected(&mut project, Edit::SetTempo { bpm });
        }
    }

    #[test]
    fn clip_ids_must_be_unique_and_known() {
        let mut project = Project::default();
        let add = Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: clip(vec![]),
        };
        project.apply(add.clone()).unwrap();
        assert_eq!(
            assert_rejected(&mut project, add),
            CommandError::DuplicateClip { id: ClipId(1) }
        );
        assert_eq!(
            assert_rejected(&mut project, Edit::RemoveClip { id: ClipId(2) }),
            CommandError::UnknownClip { id: ClipId(2) }
        );
    }

    #[test]
    fn midi_clips_on_a_node_never_overlap() {
        let mut project = Project::default();
        let at = |bar: i64| MidiClip {
            start: Ticks::from_bars(bar),
            ..clip(vec![])
        };
        let add = |id: u64, bar: i64| Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(id),
            clip: at(bar),
        };
        project.apply(add(1, 0)).unwrap();
        project.apply(add(2, 1)).unwrap();
        let stacked = Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(3),
            clip: MidiClip {
                start: Ticks::from_beats(2),
                ..clip(vec![])
            },
        };
        assert_eq!(
            assert_rejected(&mut project, stacked),
            CommandError::StackedClips {
                node: FIRST_NODE,
                clip: ClipId(1),
                over: ClipId(3),
            },
            "a clip may touch another but not overlap it"
        );
        // Swapping the two in one batch overlaps halfway, which is fine:
        // only the whole edit counts.
        let swap = Edit::Batch(vec![
            Edit::SetClip {
                id: ClipId(1),
                node: FIRST_NODE,
                clip: at(1),
            },
            Edit::SetClip {
                id: ClipId(2),
                node: FIRST_NODE,
                clip: at(0),
            },
        ]);
        project.apply(swap).unwrap();
        assert_eq!(
            project.root.children[0].1.clips[&ClipId(1)].start,
            Ticks::from_bars(1)
        );
    }

    #[test]
    fn invalid_clips_are_rejected_with_a_reason() {
        let cases = [
            (
                MidiClip {
                    start: Ticks(-1),
                    ..clip(vec![])
                },
                InvalidClip::NegativeStart,
            ),
            (
                MidiClip {
                    length: Ticks::ZERO,
                    ..clip(vec![])
                },
                InvalidClip::EmptyLength,
            ),
            (
                clip(vec![note(60, 100, 10), note(128, 100, 10)]),
                InvalidClip::NotePitch { index: 1 },
            ),
            (
                clip(vec![note(60, 0, 10)]),
                InvalidClip::NoteVelocity { index: 0 },
            ),
            (
                clip(vec![note(60, 100, 0)]),
                InvalidClip::NoteLength { index: 0 },
            ),
        ];
        for (clip, reason) in cases {
            let mut project = Project::default();
            let err = assert_rejected(
                &mut project,
                Edit::AddClip {
                    node: FIRST_NODE,
                    id: ClipId(1),
                    clip,
                },
            );
            assert_eq!(err, CommandError::InvalidClip { reason });
        }
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn names_are_trimmed() {
        let mut project = Project::default();
        project
            .apply(Edit::RenameSong {
                name: "  Night Drive ".to_owned(),
            })
            .unwrap();
        assert_eq!(project.name, "Night Drive");
    }

    #[test]
    fn empty_names_are_rejected() {
        let mut project = Project::default();
        for name in ["", "   "] {
            assert_eq!(
                project.apply(Edit::RenameNode {
                    node: FIRST_NODE,
                    name: name.to_owned()
                }),
                Err(CommandError::EmptyName)
            );
        }
        assert_eq!(project.node(FIRST_NODE).unwrap().name, "Synth");
    }
}

#[cfg(test)]
mod move_layer_tests {
    use super::*;
    use daw_protocol::BlendMode;

    const VOLUME: ParamId = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Channel,
        index: 0,
    };

    fn stack(project: &Project) -> Vec<u64> {
        project.node(FIRST_NODE).unwrap().channel[0]
            .layers
            .iter()
            .map(|(id, _)| id.0)
            .collect()
    }

    fn with_three_layers() -> Project {
        let mut project = Project::default();
        for id in 1..=3 {
            project
                .apply(Edit::AddLayer {
                    param: VOLUME,
                    id: LayerId(id),
                    index: (id - 1) as usize,
                    layer: Layer::new(BlendMode::Add),
                })
                .unwrap();
        }
        project
    }

    #[test]
    fn moving_a_layer_reorders_its_stack_and_undoes() {
        let mut project = with_three_layers();
        let inverse = project
            .apply(Edit::MoveLayer {
                id: LayerId(1),
                index: 2,
            })
            .unwrap();
        assert_eq!(stack(&project), [2, 3, 1]);
        project.apply(inverse).unwrap();
        assert_eq!(stack(&project), [1, 2, 3]);
    }

    #[test]
    fn moves_must_stay_inside_the_stack() {
        let mut project = with_three_layers();
        assert_eq!(
            project.apply(Edit::MoveLayer {
                id: LayerId(1),
                index: 3
            }),
            Err(CommandError::IndexOutOfRange { index: 3, len: 3 })
        );
        assert_eq!(stack(&project), [1, 2, 3]);
    }
}

#[cfg(test)]
mod node_tests {
    use super::*;
    use daw_protocol::{EffectKind, Ticks};

    const DRUMS: NodeId = NodeId(2);
    const KICK: NodeId = NodeId(3);

    /// Song › [Synth, Drums › [Kick]].
    fn tree() -> Project {
        let mut project = Project::default();
        let mut drums = Node::new("Drums", 0);
        drums
            .children
            .push((KICK, Node::new("Kick", 0).with_instrument(DeviceId(101))));
        project
            .apply(Edit::AddNode {
                id: DRUMS,
                parent: NodeId::ROOT,
                index: 1,
                node: drums,
            })
            .unwrap();
        project
    }

    fn order(project: &Project) -> Vec<(u64, usize)> {
        project.nodes().iter().map(|e| (e.id.0, e.depth)).collect()
    }

    fn round_trip(edit: Edit) {
        round_trip_on(tree(), edit);
    }

    fn round_trip_on(mut project: Project, edit: Edit) {
        let original = project.clone();
        let inverse = project.apply(edit).unwrap();
        assert_ne!(project, original);
        project.apply(inverse).unwrap();
        assert_eq!(project, original);
    }

    #[test]
    fn nodes_are_walked_parent_first() {
        assert_eq!(order(&tree()), [(0, 0), (1, 1), (2, 1), (3, 2)]);
        assert_eq!(tree().parent_of(KICK), Some((DRUMS, 0)));
    }

    #[test]
    fn added_nodes_get_default_parameters() {
        let project = tree();
        let kick = project.node(KICK).unwrap();
        assert_eq!(kick.channel.len(), CHANNEL_PARAMS.len());
        assert_eq!(
            kick.instruments.first().map(|(_, i)| i.params.len()),
            Some(INSTRUMENT_PARAMS.len())
        );
        assert!(project.node(DRUMS).unwrap().instruments.is_empty());
    }

    #[test]
    fn every_node_edit_is_reverted_by_its_inverse() {
        round_trip(Edit::RemoveNode { id: DRUMS });
        round_trip(Edit::MoveNode {
            id: FIRST_NODE,
            parent: DRUMS,
            index: 1,
        });
        round_trip(Edit::MoveNode {
            id: DRUMS,
            parent: NodeId::ROOT,
            index: 0,
        });
        round_trip(Edit::SetNodeColor {
            node: KICK,
            color: 5,
        });
        round_trip(Edit::SetMute {
            node: DRUMS,
            muted: true,
        });
    }

    #[test]
    fn moves_count_positions_after_taking_the_node_out() {
        let mut project = tree();
        project
            .apply(Edit::MoveNode {
                id: FIRST_NODE,
                parent: NodeId::ROOT,
                index: 1,
            })
            .unwrap();
        assert_eq!(order(&project), [(0, 0), (2, 1), (3, 2), (1, 1)]);
    }

    #[test]
    fn the_song_stays_put_and_nodes_never_move_inside_themselves() {
        let mut project = tree();
        let before = project.clone();
        assert_eq!(
            project.apply(Edit::RemoveNode { id: NodeId::ROOT }),
            Err(CommandError::RootNode)
        );
        assert_eq!(
            project.apply(Edit::MoveNode {
                id: DRUMS,
                parent: KICK,
                index: 0
            }),
            Err(CommandError::NodeInsideItself { id: DRUMS })
        );
        assert_eq!(
            project.apply(Edit::MoveNode {
                id: DRUMS,
                parent: DRUMS,
                index: 0
            }),
            Err(CommandError::NodeInsideItself { id: DRUMS })
        );
        assert_eq!(
            project.apply(Edit::MoveNode {
                id: KICK,
                parent: DRUMS,
                index: 1
            }),
            Err(CommandError::IndexOutOfRange { index: 1, len: 0 })
        );
        assert_eq!(project, before);
    }

    #[test]
    fn ids_are_unique_across_the_whole_tree() {
        let mut project = tree();
        let clip = MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_beats(1),
            notes: Vec::new(),
            looping: None,
        };
        project
            .apply(Edit::AddClip {
                node: FIRST_NODE,
                id: ClipId(1),
                clip: clip.clone(),
            })
            .unwrap();
        assert_eq!(
            project.apply(Edit::AddClip {
                node: KICK,
                id: ClipId(1),
                clip
            }),
            Err(CommandError::DuplicateClip { id: ClipId(1) })
        );
        project
            .apply(Edit::AddEffect {
                node: KICK,
                id: DeviceId(4),
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            })
            .unwrap();
        let mut copy = Node::new("Copy", 0);
        copy.effects
            .push((DeviceId(4), Effect::new(EffectKind::Filter)));
        assert_eq!(
            project.apply(Edit::AddNode {
                id: NodeId(9),
                parent: NodeId::ROOT,
                index: 0,
                node: copy
            }),
            Err(CommandError::DuplicateDevice { id: DeviceId(4) })
        );
        assert_eq!(
            project.apply(Edit::AddNode {
                id: KICK,
                parent: NodeId::ROOT,
                index: 0,
                node: Node::new("Again", 0)
            }),
            Err(CommandError::DuplicateNode { id: KICK })
        );
    }

    #[test]
    fn a_batch_is_one_step_and_all_or_nothing() {
        // Group the Synth into a new folder: add the folder, move it in.
        let group = Edit::Batch(vec![
            Edit::AddNode {
                id: NodeId(7),
                parent: NodeId::ROOT,
                index: 0,
                node: Node::new("Folder", 0),
            },
            Edit::MoveNode {
                id: FIRST_NODE,
                parent: NodeId(7),
                index: 0,
            },
        ]);
        round_trip(group);
        let mut project = tree();
        project
            .apply(Edit::Batch(vec![Edit::Batch(Vec::new())]))
            .unwrap();
        let before = project.clone();
        let failing = Edit::Batch(vec![
            Edit::RemoveNode { id: KICK },
            Edit::RemoveNode { id: KICK },
        ]);
        assert_eq!(
            project.apply(failing),
            Err(CommandError::UnknownNode { id: KICK })
        );
        assert_eq!(project, before, "the first removal was undone");
    }

    fn one_beat() -> MidiClip {
        MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_beats(1),
            notes: Vec::new(),
            looping: None,
        }
    }

    #[test]
    fn clips_move_between_nodes_and_back() {
        let mut project = tree();
        project
            .apply(Edit::AddClip {
                node: FIRST_NODE,
                id: ClipId(1),
                clip: one_beat(),
            })
            .unwrap();
        let moved = MidiClip {
            start: Ticks::from_beats(4),
            ..one_beat()
        };
        round_trip_on(
            project.clone(),
            Edit::SetClip {
                id: ClipId(1),
                node: KICK,
                clip: moved.clone(),
            },
        );
        project
            .apply(Edit::SetClip {
                id: ClipId(1),
                node: KICK,
                clip: moved.clone(),
            })
            .unwrap();
        assert_eq!(project.clip_node(ClipId(1)), Some(KICK));
        assert_eq!(project.node(KICK).unwrap().clips[&ClipId(1)], moved);
        assert!(project.node(FIRST_NODE).unwrap().clips.is_empty());
    }

    #[test]
    fn midi_clips_go_on_midi_nodes_with_or_without_an_instrument() {
        let mut project = tree();
        project
            .apply(Edit::AddClip {
                node: DRUMS,
                id: ClipId(1),
                clip: one_beat(),
            })
            .unwrap();
        let before = project.clone();
        assert_eq!(
            project.apply(Edit::AddClip {
                node: NodeId::ROOT,
                id: ClipId(2),
                clip: one_beat()
            }),
            Err(CommandError::NotMidi { node: NodeId::ROOT })
        );
        let mut audio = Node::new("Vocals", 0).with_kind(NodeKind::Audio);
        audio.clips.insert(ClipId(2), one_beat());
        assert_eq!(
            project.apply(Edit::AddNode {
                id: NodeId(8),
                parent: NodeId::ROOT,
                index: 0,
                node: audio
            }),
            Err(CommandError::NotMidi { node: NodeId(8) })
        );
        assert_eq!(
            project.apply(Edit::AddNode {
                id: NodeId(8),
                parent: NodeId::ROOT,
                index: 0,
                node: Node::new("Vocals", 0)
                    .with_kind(NodeKind::Audio)
                    .with_instrument(DeviceId(102))
            }),
            Err(CommandError::InstrumentOnAudio { node: NodeId(8) })
        );
        assert_eq!(project, before);
    }

    #[test]
    fn effects_switch_off_and_back() {
        let mut project = tree();
        project
            .apply(Edit::AddEffect {
                node: KICK,
                id: DeviceId(3),
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            })
            .unwrap();
        round_trip_on(
            project,
            Edit::SetDeviceEnabled {
                id: DeviceId(3),
                enabled: false,
            },
        );
    }

    #[test]
    fn the_instrument_is_only_on_nodes_that_have_one() {
        let mut project = tree();
        let param = ParamId {
            node: DRUMS,
            device: DeviceRef::Instrument(FIRST_INSTRUMENT),
            index: 0,
        };
        assert_eq!(
            project.apply(Edit::SetParam { param, value: 0.5 }),
            Err(CommandError::UnknownParam { param })
        );
    }
}
