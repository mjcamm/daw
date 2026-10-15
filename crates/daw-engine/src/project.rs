//! Authoritative project state and the edits that change it.

use crate::devices::{
    CHANNEL_PARAMS, INSTRUMENT_PARAMS, ParamInfo, effect_params, fixed_device_params,
};
use daw_automation::validate_block;
use daw_protocol::{
    Block, BlockId, ClipId, CommandError, DeviceId, DeviceRef, Edit, Effect, InvalidClip, Layer,
    LayerId, MidiClip, ParamId, ParamState,
};
use std::collections::{BTreeMap, HashSet};

pub const MIN_TEMPO_BPM: f64 = 20.0;
pub const MAX_TEMPO_BPM: f64 = 999.0;
pub const DEFAULT_TEMPO_BPM: f64 = 120.0;
/// Most effects in one node's chain.
pub const MAX_EFFECTS: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub tempo_bpm: f64,
    pub node: Node,
}

/// A channel: clips playing through an instrument, then its effect chain,
/// then its own volume and pan. Every device holds one [`ParamState`] per
/// parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub instrument: Vec<ParamState>,
    pub effects: Vec<(DeviceId, Effect)>,
    pub channel: Vec<ParamState>,
    pub clips: BTreeMap<ClipId, MidiClip>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            tempo_bpm: DEFAULT_TEMPO_BPM,
            node: Node::default(),
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        Self {
            instrument: default_params(&INSTRUMENT_PARAMS),
            effects: Vec::new(),
            channel: default_params(&CHANNEL_PARAMS),
            clips: BTreeMap::new(),
        }
    }
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

impl Node {
    /// Every device on the node, in signal order, with its parameters.
    pub fn devices(&self) -> impl Iterator<Item = (DeviceRef, &[ParamState])> {
        std::iter::once((DeviceRef::Instrument, self.instrument.as_slice()))
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

    pub fn param_infos(&self, device: DeviceRef) -> Option<&'static [ParamInfo]> {
        match device {
            DeviceRef::Effect(id) => self.effect(id).map(|(_, e)| effect_params(e.kind)),
            fixed => fixed_device_params(fixed),
        }
    }

    pub fn param(&self, param: ParamId) -> Option<&ParamState> {
        let params = match param.device {
            DeviceRef::Instrument => &self.instrument,
            DeviceRef::Channel => &self.channel,
            DeviceRef::Effect(id) => &self.effect(id)?.1.params,
        };
        params.get(param.index as usize)
    }

    fn param_mut(&mut self, param: ParamId) -> Result<&mut ParamState, CommandError> {
        let params = match param.device {
            DeviceRef::Instrument => &mut self.instrument,
            DeviceRef::Channel => &mut self.channel,
            DeviceRef::Effect(id) => match self.effects.iter_mut().find(|(e, _)| *e == id) {
                Some((_, effect)) => &mut effect.params,
                None => return Err(CommandError::UnknownParam { param }),
            },
        };
        params
            .get_mut(param.index as usize)
            .ok_or(CommandError::UnknownParam { param })
    }

    fn effect(&self, id: DeviceId) -> Option<(usize, &Effect)> {
        self.effects
            .iter()
            .enumerate()
            .find(|(_, (e, _))| *e == id)
            .map(|(i, (_, effect))| (i, effect))
    }

    /// Where a layer lives: its parameter and position in the stack.
    fn find_layer(&self, id: LayerId) -> Option<(ParamId, usize)> {
        self.devices().find_map(|(device, params)| {
            params.iter().enumerate().find_map(|(index, state)| {
                let position = state.layers.iter().position(|(l, _)| *l == id)?;
                let param = ParamId {
                    device,
                    index: index as u32,
                };
                Some((param, position))
            })
        })
    }

    /// Where a block lives: its layer's parameter, the layer's position in
    /// the stack, the layer id and the block's position in the layer.
    fn find_block(&self, id: BlockId) -> Option<(ParamId, usize, LayerId, usize)> {
        self.devices().find_map(|(device, params)| {
            params.iter().enumerate().find_map(|(index, state)| {
                state
                    .layers
                    .iter()
                    .enumerate()
                    .find_map(|(layer_pos, (layer_id, layer))| {
                        let block_pos = layer.blocks.iter().position(|(b, _)| *b == id)?;
                        let param = ParamId {
                            device,
                            index: index as u32,
                        };
                        Some((param, layer_pos, *layer_id, block_pos))
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

    /// Every layer and block id in use.
    fn used_ids(&self) -> UsedIds {
        let mut used = UsedIds::default();
        for (_, params) in self.devices() {
            for state in params {
                for (layer_id, layer) in &state.layers {
                    used.layers.insert(*layer_id);
                    used.blocks.extend(layer.blocks.iter().map(|(b, _)| *b));
                }
            }
        }
        used
    }
}

#[derive(Debug, Default)]
struct UsedIds {
    layers: HashSet<LayerId>,
    blocks: HashSet<BlockId>,
}

impl UsedIds {
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
    /// A rejected edit leaves the project unchanged: every check happens
    /// before anything is modified.
    pub fn apply(&mut self, edit: Edit) -> Result<Edit, CommandError> {
        let node = &mut self.node;
        match edit {
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
            Edit::AddClip { id, clip } => {
                if node.clips.contains_key(&id) {
                    return Err(CommandError::DuplicateClip { id });
                }
                validate_clip(&clip).map_err(|reason| CommandError::InvalidClip { reason })?;
                node.clips.insert(id, clip);
                Ok(Edit::RemoveClip { id })
            }
            Edit::RemoveClip { id } => {
                let clip = node
                    .clips
                    .remove(&id)
                    .ok_or(CommandError::UnknownClip { id })?;
                Ok(Edit::AddClip { id, clip })
            }
            Edit::SetParam { param, value } => {
                check_normalized(value)?;
                let state = node.param_mut(param)?;
                let previous = std::mem::replace(&mut state.base, value);
                Ok(Edit::SetParam {
                    param,
                    value: previous,
                })
            }
            Edit::AddEffect {
                id,
                index,
                mut effect,
            } => {
                if node.effect(id).is_some() {
                    return Err(CommandError::DuplicateDevice { id });
                }
                if node.effects.len() >= MAX_EFFECTS {
                    return Err(CommandError::TooManyEffects { max: MAX_EFFECTS });
                }
                check_index(index, node.effects.len())?;
                let infos = effect_params(effect.kind);
                if effect.params.is_empty() {
                    effect.params = default_params(infos);
                }
                if effect.params.len() != infos.len() {
                    return Err(CommandError::WrongParamCount {
                        expected: infos.len(),
                        found: effect.params.len(),
                    });
                }
                let mut used = node.used_ids();
                for state in &mut effect.params {
                    check_normalized(state.base)?;
                    for (layer_id, layer) in &mut state.layers {
                        prepare_layer(*layer_id, layer, &mut used)?;
                    }
                }
                node.effects.insert(index, (id, effect));
                Ok(Edit::RemoveEffect { id })
            }
            Edit::RemoveEffect { id } => {
                let (index, _) = node.effect(id).ok_or(CommandError::UnknownDevice { id })?;
                let (_, effect) = node.effects.remove(index);
                Ok(Edit::AddEffect { id, index, effect })
            }
            Edit::AddLayer {
                param,
                id,
                index,
                mut layer,
            } => {
                let stack_len = node
                    .param(param)
                    .ok_or(CommandError::UnknownParam { param })?
                    .layers
                    .len();
                check_index(index, stack_len)?;
                prepare_layer(id, &mut layer, &mut node.used_ids())?;
                node.param_mut(param)?.layers.insert(index, (id, layer));
                Ok(Edit::RemoveLayer { id })
            }
            Edit::RemoveLayer { id } => {
                let (param, index) = node
                    .find_layer(id)
                    .ok_or(CommandError::UnknownLayer { id })?;
                let (_, layer) = node.param_mut(param)?.layers.remove(index);
                Ok(Edit::AddLayer {
                    param,
                    id,
                    index,
                    layer,
                })
            }
            Edit::SetLayerOptions { id, mode, enabled } => {
                let layer = node.layer_mut(id)?;
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
                if node.find_layer(layer).is_none() {
                    return Err(CommandError::UnknownLayer { id: layer });
                }
                node.used_ids().claim_block(id)?;
                check_block(id, &block)?;
                let target = node.layer_mut(layer)?;
                check_no_overlap(&target.blocks, id, &block)?;
                insert_sorted(&mut target.blocks, id, block);
                Ok(Edit::RemoveBlock { id })
            }
            Edit::RemoveBlock { id } => {
                let (param, layer_pos, layer, block_pos) = node
                    .find_block(id)
                    .ok_or(CommandError::UnknownBlock { id })?;
                let state = node.param_mut(param)?;
                let (_, block) = state.layers[layer_pos].1.blocks.remove(block_pos);
                Ok(Edit::AddBlock { layer, id, block })
            }
            Edit::SetBlock { id, block } => {
                let (param, layer_pos, _, block_pos) = node
                    .find_block(id)
                    .ok_or(CommandError::UnknownBlock { id })?;
                check_block(id, &block)?;
                let blocks = &mut node.param_mut(param)?.layers[layer_pos].1.blocks;
                check_no_overlap(blocks, id, &block)?;
                let (_, previous) = blocks.remove(block_pos);
                insert_sorted(blocks, id, block);
                Ok(Edit::SetBlock {
                    id,
                    block: previous,
                })
            }
        }
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
fn prepare_layer(id: LayerId, layer: &mut Layer, used: &mut UsedIds) -> Result<(), CommandError> {
    used.claim_layer(id)?;
    layer.blocks.sort_by_key(|(_, b)| b.start);
    for (i, (block_id, block)) in layer.blocks.iter().enumerate() {
        used.claim_block(*block_id)?;
        check_block(*block_id, block)?;
        check_no_overlap(&layer.blocks[..i], *block_id, block)?;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{channel, filter, instrument};
    use daw_protocol::{BlendMode, EffectKind, InvalidBlock, MidiNote, Shape, Ticks};

    const CUTOFF: ParamId = ParamId {
        device: DeviceRef::Instrument,
        index: instrument::CUTOFF,
    };
    const VOLUME: ParamId = ParamId {
        device: DeviceRef::Channel,
        index: channel::VOLUME,
    };

    fn clip(notes: Vec<MidiNote>) -> MidiClip {
        MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_bars(1),
            notes,
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
            layer: Layer::new(BlendMode::Replace),
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

    #[test]
    fn every_edit_is_reverted_by_its_inverse() {
        let edits = [
            Edit::SetTempo { bpm: 140.0 },
            Edit::AddClip {
                id: ClipId(1),
                clip: clip(vec![note(60, 100, 240)]),
            },
            Edit::SetParam {
                param: VOLUME,
                value: 0.5,
            },
            Edit::SetParam {
                param: ParamId {
                    device: DeviceRef::Effect(DeviceId(1)),
                    index: filter::CUTOFF,
                },
                value: 0.2,
            },
            Edit::AddEffect {
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
            device: DeviceRef::Effect(DeviceId(1)),
            index: filter::CUTOFF,
        };
        project.apply(add_layer(filter_cutoff, 9)).unwrap();
        let inverse = project
            .apply(Edit::RemoveEffect { id: DeviceId(1) })
            .unwrap();
        assert_eq!(project.node.find_layer(LayerId(9)), None);
        project.apply(inverse).unwrap();
        assert_eq!(
            project.node.find_layer(LayerId(9)),
            Some((filter_cutoff, 0))
        );
    }

    #[test]
    fn new_effects_start_at_their_defaults() {
        let project = populated();
        let (_, effect) = &project.node.effects[0];
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
        let ids: Vec<_> = project.node.instrument[instrument::CUTOFF as usize]
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
        let layer = &project.node.instrument[instrument::CUTOFF as usize].layers[0].1;
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
                device: DeviceRef::Instrument,
                index: 99,
            },
            ParamId {
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
        for i in 0..MAX_EFFECTS as u64 {
            project
                .apply(Edit::AddEffect {
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
                    id: ClipId(1),
                    clip,
                },
            );
            assert_eq!(err, CommandError::InvalidClip { reason });
        }
    }
}
