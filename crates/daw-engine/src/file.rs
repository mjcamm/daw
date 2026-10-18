//! The project file: a versioned, human-readable JSON document.
//!
//! Saving writes the project out as plain data. Loading never trusts that
//! data: it rebuilds the project by applying the same validated edits the UI
//! uses, so a hand-edited or damaged file can never produce an invalid
//! project. Undo history and transport position are not saved.

use crate::devices::CHANNEL_PARAMS;
use crate::project::{Project, song};
use daw_protocol::{
    ClipId, CommandError, DeviceId, DeviceRef, Edit, Effect, Instrument, LoopRange, MidiClip, Node,
    NodeId, NodeKind, ParamId, ParamState, SampleId, SampleRef, Signal, SignalId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifies the file type, so other JSON files are rejected clearly.
pub const FORMAT: &str = "daw-project";
/// Bumped whenever the layout changes; older versions are converted on load.
/// Version 2 added motion signals; version 3 centred the blend modes;
/// version 4 moved listening to signals onto layers and made notes signals
/// ADSR hits; version 5 made velocity a clamp; version 6 added switches for
/// the pitch range and the clamp; version 7 made the nodes a tree under the
/// Song; version 8 let a node layer several instruments; version 9 added
/// the sample table and the SingleSampler's sample.
pub const VERSION: u32 = 9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub tempo_bpm: f64,
    pub loop_range: Option<LoopRange>,
    /// The Song and everything under it.
    pub root: NodeFile,
    /// Motion signals. Missing in version 1 files.
    #[serde(default)]
    pub signals: Vec<SignalFile>,
    /// Audio files the project uses. Missing before version 9.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<SampleFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleFile {
    pub id: SampleId,
    #[serde(flatten)]
    pub sample: SampleRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalFile {
    pub id: SignalId,
    #[serde(flatten)]
    pub signal: Signal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeFile {
    pub id: NodeId,
    pub name: String,
    /// Missing before node kinds existed: those nodes are all MIDI.
    #[serde(default)]
    pub kind: NodeKind,
    pub color: u8,
    pub muted: bool,
    pub soloed: bool,
    /// Layered instruments (MIDI nodes only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instruments: Vec<InstrumentFile>,
    pub effects: Vec<EffectFile>,
    /// Volume and pan, in parameter order.
    pub channel: Vec<ParamState>,
    pub clips: Vec<ClipFile>,
    #[serde(default)]
    pub children: Vec<NodeFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentFile {
    pub id: DeviceId,
    #[serde(flatten)]
    pub instrument: Instrument,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectFile {
    pub id: DeviceId,
    #[serde(flatten)]
    pub effect: Effect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipFile {
    pub id: ClipId,
    #[serde(flatten)]
    pub clip: MidiClip,
}

/// Why a project file could not be opened.
#[derive(Debug)]
pub enum LoadError {
    /// Not valid JSON, or not shaped like a project file.
    Json(serde_json::Error),
    /// Valid JSON, but some other kind of file.
    WrongFormat(String),
    /// Saved by a newer version of the app.
    UnsupportedVersion(u32),
    /// The Song must have id 0.
    RootId(NodeId),
    /// A device's parameter list does not match the device.
    ParamCount {
        device: &'static str,
        expected: usize,
        found: usize,
    },
    /// The content breaks a project rule (overlapping blocks, bad ids, ...).
    Invalid(CommandError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "not a readable project file: {e}"),
            Self::WrongFormat(format) => {
                write!(f, "not a project file (format is \"{format}\")")
            }
            Self::UnsupportedVersion(v) => write!(
                f,
                "saved by a newer version of the app (file version {v}, this app reads up to {VERSION})"
            ),
            Self::RootId(id) => write!(f, "the Song has id {}, expected 0", id.0),
            Self::ParamCount {
                device,
                expected,
                found,
            } => write!(f, "{device} has {found} parameters, expected {expected}"),
            Self::Invalid(e) => write!(f, "the project is damaged: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<CommandError> for LoadError {
    fn from(e: CommandError) -> Self {
        Self::Invalid(e)
    }
}

fn node_file(id: NodeId, node: &Node) -> NodeFile {
    NodeFile {
        id,
        name: node.name.clone(),
        kind: node.kind,
        color: node.color,
        muted: node.muted,
        soloed: node.soloed,
        instruments: node
            .instruments
            .iter()
            .map(|(id, instrument)| InstrumentFile {
                id: *id,
                instrument: instrument.clone(),
            })
            .collect(),
        effects: node
            .effects
            .iter()
            .map(|(id, effect)| EffectFile {
                id: *id,
                effect: effect.clone(),
            })
            .collect(),
        channel: node.channel.clone(),
        clips: node
            .clips
            .iter()
            .map(|(id, clip)| ClipFile {
                id: *id,
                clip: clip.clone(),
            })
            .collect(),
        children: node
            .children
            .iter()
            .map(|(id, child)| node_file(*id, child))
            .collect(),
    }
}

/// A node from the file, ready for [`Edit::AddNode`], which checks the
/// rest. Clip ids are checked here, since a map would merge duplicates.
fn file_node(file: NodeFile) -> Result<(NodeId, Node), LoadError> {
    let mut clips = std::collections::BTreeMap::new();
    for c in file.clips {
        if clips.insert(c.id, c.clip).is_some() {
            return Err(CommandError::DuplicateClip { id: c.id }.into());
        }
    }
    let node = Node {
        name: file.name,
        kind: file.kind,
        color: file.color,
        muted: file.muted,
        soloed: file.soloed,
        instruments: file
            .instruments
            .into_iter()
            .map(|i| (i.id, i.instrument))
            .collect(),
        effects: file.effects.into_iter().map(|e| (e.id, e.effect)).collect(),
        channel: file.channel,
        clips,
        children: file
            .children
            .into_iter()
            .map(file_node)
            .collect::<Result<_, _>>()?,
    };
    Ok((file.id, node))
}

impl ProjectFile {
    /// Captures a project and its loop region.
    pub fn new(project: &Project, loop_range: Option<LoopRange>) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            name: project.name.clone(),
            tempo_bpm: project.tempo_bpm,
            loop_range,
            root: node_file(NodeId::ROOT, &project.root),
            signals: project
                .signals
                .iter()
                .map(|(id, signal)| SignalFile {
                    id: *id,
                    signal: signal.clone(),
                })
                .collect(),
            samples: project
                .samples
                .iter()
                .map(|(id, sample)| SampleFile {
                    id: *id,
                    sample: sample.clone(),
                })
                .collect(),
        }
    }

    pub fn to_json(&self) -> String {
        // Plain data with string keys: serialising cannot fail.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Reads a file, converting older versions first.
    pub fn from_json(json: &str) -> Result<Self, LoadError> {
        #[derive(Deserialize)]
        struct Header {
            format: String,
            version: u32,
        }
        let header: Header = serde_json::from_str(json).map_err(LoadError::Json)?;
        if header.format != FORMAT {
            return Err(LoadError::WrongFormat(header.format));
        }
        if header.version > VERSION {
            return Err(LoadError::UnsupportedVersion(header.version));
        }
        if header.version < VERSION {
            let mut value: serde_json::Value =
                serde_json::from_str(json).map_err(LoadError::Json)?;
            if header.version < 3 {
                crate::upgrade::centre_blend_modes(&mut value);
            }
            if header.version < 4 {
                crate::upgrade::listen_to_signals(&mut value);
            }
            if header.version < 5 {
                crate::upgrade::clamp_velocity(&mut value);
            }
            if header.version < 6 {
                crate::upgrade::notes_switches(&mut value);
            }
            if header.version < 7 {
                crate::upgrade::nest_under_song(&mut value);
            }
            if header.version < 8 {
                crate::upgrade::instrument_lists(&mut value);
            }
            return serde_json::from_value(value).map_err(LoadError::Json);
        }
        serde_json::from_str(json).map_err(LoadError::Json)
    }

    /// Rebuilds the project through validated edits, returning it with its
    /// loop region.
    pub fn into_project(self) -> Result<(Project, Option<LoopRange>), LoadError> {
        let root = self.root;
        if root.id != NodeId::ROOT {
            return Err(LoadError::RootId(root.id));
        }
        check_count("Channel", CHANNEL_PARAMS.len(), root.channel.len())?;
        if let Some(range) = self.loop_range {
            crate::check_loop(range)?;
        }

        let mut project = Project {
            root: song(),
            ..Project::default()
        };
        let song = NodeId::ROOT;
        // Samples and signals first, so the instruments that play them and
        // the layers that listen to them resolve.
        let mut edits: Vec<Edit> = self
            .samples
            .into_iter()
            .map(|s| Edit::AddSample {
                id: s.id,
                sample: s.sample,
            })
            .collect();
        edits.extend(self.signals.into_iter().map(|s| Edit::AddSignal {
            id: s.id,
            signal: s.signal,
            listeners: Vec::new(),
        }));
        edits.extend([
            Edit::RenameSong { name: self.name },
            Edit::SetTempo {
                bpm: self.tempo_bpm,
            },
            Edit::RenameNode {
                node: song,
                name: root.name,
            },
            Edit::SetNodeColor {
                node: song,
                color: root.color,
            },
            Edit::SetMute {
                node: song,
                muted: root.muted,
            },
            Edit::SetSolo {
                node: song,
                soloed: root.soloed,
            },
        ]);
        // The Song is an audio node, so instruments on it are refused.
        edits.extend(root.instruments.into_iter().enumerate().map(|(index, i)| {
            Edit::AddInstrument {
                node: song,
                id: i.id,
                index,
                instrument: i.instrument,
            }
        }));
        edits.extend(param_edits(song, DeviceRef::Channel, root.channel));
        edits.extend(
            root.effects
                .into_iter()
                .enumerate()
                .map(|(index, e)| Edit::AddEffect {
                    node: song,
                    id: e.id,
                    index,
                    effect: e.effect,
                }),
        );
        edits.extend(root.clips.into_iter().map(|c| Edit::AddClip {
            node: song,
            id: c.id,
            clip: c.clip,
        }));
        for (index, child) in root.children.into_iter().enumerate() {
            let (id, node) = file_node(child)?;
            edits.push(Edit::AddNode {
                id,
                parent: song,
                index,
                node,
            });
        }
        for edit in edits {
            project.apply(edit)?;
        }
        Ok((project, self.loop_range))
    }
}

fn check_count(device: &'static str, expected: usize, found: usize) -> Result<(), LoadError> {
    if expected == found {
        Ok(())
    } else {
        Err(LoadError::ParamCount {
            device,
            expected,
            found,
        })
    }
}

/// Edits that set each parameter's default state and rebuild its layers.
fn param_edits(
    node: NodeId,
    device: DeviceRef,
    params: Vec<ParamState>,
) -> impl Iterator<Item = Edit> {
    params
        .into_iter()
        .enumerate()
        .flat_map(move |(index, state)| {
            let param = ParamId {
                node,
                device,
                index: index as u32,
            };
            std::iter::once(Edit::SetParam {
                param,
                value: state.base,
            })
            .chain(state.layers.into_iter().enumerate().map(
                move |(position, (id, layer))| Edit::AddLayer {
                    param,
                    id,
                    index: position,
                    layer,
                },
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{filter, instrument};
    use crate::project::{FIRST_INSTRUMENT, FIRST_NODE};
    use daw_protocol::{
        BlendMode, Block, BlockId, EffectKind, Layer, LayerId, MidiNote, NotesSource, Segment,
        Shape, ShapePoint, SignalSource, Ticks,
    };
    use serde_json::json;

    /// A project using every kind of content.
    fn rich_project() -> Project {
        let mut project = Project::default();
        let mut layer = Layer::new(BlendMode::Add);
        layer.mix = 0.65;
        let mut block = Block::new(
            Ticks::from_beats(2),
            Ticks::from_beats(4),
            Shape {
                points: vec![
                    ShapePoint::new(0.0, 0.2).with_segment(Segment::Curve(-0.4)),
                    ShapePoint::new(0.5, 0.9).with_segment(Segment::Hold),
                    ShapePoint::new(1.0, 0.1),
                ],
            },
        );
        block.cycle = Some(Ticks(240));
        block.depth = 0.6;
        block.invert = true;
        block.nudge = Ticks(-30);
        block.fade_in = Ticks(120);
        layer.blocks.push((BlockId(7), block));
        let mut ducking = Layer::new(BlendMode::Multiply).listening(SignalId(2));
        let mut window = Block::new(
            Ticks::from_beats(8),
            Ticks::from_beats(4),
            Shape::constant(0.5),
        );
        window.invert = true;
        ducking.blocks.push((BlockId(8), window));
        let edits = [
            Edit::AddSignal {
                id: SignalId(2),
                signal: Signal {
                    name: "Kick duck".to_owned(),
                    color: 4,
                    source: SignalSource::Notes(NotesSource {
                        node: FIRST_NODE,
                        pitch_range: true,
                        velocity_clamp: true,
                        lowest: 36,
                        highest: 37,
                        attack: Ticks(10),
                        decay: Ticks(480),
                        sustain: 0.25,
                        release: Ticks(120),
                        velocity_min: 0.5,
                        velocity_max: 0.9,
                    }),
                },
                listeners: Vec::new(),
            },
            Edit::AddSignal {
                id: SignalId(5),
                signal: Signal {
                    name: "Sweep".to_owned(),
                    color: 1,
                    source: SignalSource::Drawn(vec![(
                        BlockId(11),
                        Block::new(Ticks::ZERO, Ticks::from_bars(1), Shape::ramp(0.2, 0.9)),
                    )]),
                },
                listeners: Vec::new(),
            },
            Edit::RenameSong {
                name: "Night Drive".to_owned(),
            },
            Edit::SetTempo { bpm: 128.5 },
            Edit::RenameNode {
                node: FIRST_NODE,
                name: "Lead".to_owned(),
            },
            Edit::SetMute {
                node: FIRST_NODE,
                muted: true,
            },
            Edit::AddEffect {
                node: FIRST_NODE,
                id: DeviceId(3),
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            },
            Edit::SetParam {
                param: ParamId {
                    node: FIRST_NODE,
                    device: DeviceRef::Effect(DeviceId(3)),
                    index: filter::RESONANCE,
                },
                value: 0.4,
            },
            Edit::AddLayer {
                param: ParamId {
                    node: FIRST_NODE,
                    device: DeviceRef::Instrument(FIRST_INSTRUMENT),
                    index: instrument::CUTOFF,
                },
                id: LayerId(5),
                index: 0,
                layer,
            },
            Edit::AddLayer {
                param: ParamId {
                    node: FIRST_NODE,
                    device: DeviceRef::Channel,
                    index: crate::devices::channel::VOLUME,
                },
                id: LayerId(6),
                index: 0,
                layer: ducking,
            },
            Edit::AddClip {
                node: FIRST_NODE,
                id: ClipId(9),
                clip: MidiClip {
                    start: Ticks::from_bars(1),
                    length: Ticks::from_bars(2),
                    notes: vec![MidiNote {
                        start: Ticks(120),
                        length: Ticks(480),
                        pitch: 64,
                        velocity: 90,
                    }],
                },
            },
        ];
        for edit in edits {
            project.apply(edit).unwrap();
        }
        project.node_mut(FIRST_NODE).unwrap().color = 3;
        // A folder holding a muted node that plays notes through an effect.
        let mut inner = Node::new("Hats", 1).with_instrument(DeviceId(103));
        inner.muted = true;
        inner
            .effects
            .push((DeviceId(12), Effect::new(EffectKind::Filter)));
        let mut folder = Node::new("Drums", 0);
        folder.children.push((NodeId(21), inner));
        project
            .apply(Edit::AddNode {
                id: NodeId(20),
                parent: NodeId::ROOT,
                index: 1,
                node: folder,
            })
            .unwrap();
        project
    }

    fn loop_range() -> Option<LoopRange> {
        Some(LoopRange {
            start: Ticks::from_bars(1),
            end: Ticks::from_bars(3),
        })
    }

    #[test]
    fn projects_survive_a_round_trip_exactly() {
        let project = rich_project();
        let json = ProjectFile::new(&project, loop_range()).to_json();
        let (loaded, loaded_loop) = ProjectFile::from_json(&json)
            .unwrap()
            .into_project()
            .unwrap();
        assert_eq!(loaded, project);
        assert_eq!(loaded_loop, loop_range());
        // Saving again produces the identical file.
        assert_eq!(ProjectFile::new(&loaded, loaded_loop).to_json(), json);
    }

    #[test]
    fn files_are_readable_json() {
        let json = ProjectFile::new(&rich_project(), None).to_json();
        assert!(json.contains("\"format\": \"daw-project\""));
        assert!(json.contains("\"name\": \"Night Drive\""));
        assert!(json.contains("\"mode\": \"Add\""));
    }

    fn file_json(change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut value: serde_json::Value =
            serde_json::from_str(&ProjectFile::new(&rich_project(), None).to_json()).unwrap();
        change(&mut value);
        value.to_string()
    }

    /// The rich project as a file from before version 7: one node in a
    /// `nodes` list, and notes signals without a source node.
    fn old_file_json(version: u32, change: impl FnOnce(&mut serde_json::Value)) -> String {
        file_json(|v| {
            v["version"] = version.into();
            let mut node = v["root"]["children"][0].clone();
            let node_object = node.as_object_mut().unwrap();
            node_object.remove("id");
            node_object.remove("children");
            // One instrument, without an id or switch.
            let mut instrument = node_object["instruments"][0].clone();
            let instrument_object = instrument.as_object_mut().unwrap();
            instrument_object.remove("id");
            instrument_object.remove("enabled");
            node_object.remove("instruments");
            node_object.insert("instrument".to_owned(), instrument);
            let object = v.as_object_mut().unwrap();
            object.remove("root");
            object.insert("nodes".to_owned(), json!([node]));
            for signal in v["signals"].as_array_mut().unwrap() {
                if let Some(notes) = signal["source"].get_mut("Notes") {
                    notes.as_object_mut().unwrap().remove("node");
                }
            }
            change(v);
        })
    }

    fn load(json: &str) -> Result<(Project, Option<LoopRange>), LoadError> {
        ProjectFile::from_json(json)?.into_project()
    }

    #[test]
    fn foreign_and_newer_files_are_rejected_clearly() {
        assert!(matches!(load("not json"), Err(LoadError::Json(_))));
        assert!(matches!(
            load(&file_json(|v| v["format"] = "something-else".into())),
            Err(LoadError::WrongFormat(_))
        ));
        assert!(matches!(
            load(&file_json(|v| v["version"] = 99.into())),
            Err(LoadError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn damaged_content_is_rejected() {
        assert!(matches!(
            load(&file_json(|v| v["tempo_bpm"] = 5.into())),
            Err(LoadError::Invalid(CommandError::TempoOutOfRange { .. }))
        ));
        assert!(matches!(
            load(&file_json(|v| v["root"]["children"][0]["name"] = " ".into())),
            Err(LoadError::Invalid(CommandError::EmptyName))
        ));
        assert!(matches!(
            load(&file_json(|v| v["root"]["children"][0]["color"] = 42.into())),
            Err(LoadError::Invalid(CommandError::ColorOutOfRange {
                color: 42
            }))
        ));
        assert!(matches!(
            load(&file_json(|v| v["root"]["id"] = 5.into())),
            Err(LoadError::RootId(NodeId(5)))
        ));
        assert!(matches!(
            load(&file_json(|v| {
                v["root"]["children"][0]["channel"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            })),
            Err(LoadError::Invalid(CommandError::WrongParamCount { .. }))
        ));
        // A duplicate clip id.
        assert!(matches!(
            load(&file_json(|v| {
                let clips = v["root"]["children"][0]["clips"].as_array_mut().unwrap();
                let copy = clips[0].clone();
                clips.push(copy);
            })),
            Err(LoadError::Invalid(CommandError::DuplicateClip { .. }))
        ));
        // A block shape value out of range.
        assert!(matches!(
            load(&file_json(|v| {
                v["root"]["children"][0]["instruments"][0]["params"][0]["layers"][0][1]["blocks"]
                    [0][1]["shape"]["points"][0]["value"] = 3.into();
            })),
            Err(LoadError::Invalid(CommandError::InvalidBlock { .. }))
        ));
    }

    #[test]
    fn version_1_files_without_signals_still_load() {
        let json = old_file_json(1, |v| {
            v.as_object_mut().unwrap().remove("signals");
            // Version 1 had no listening layers either.
            let volume = crate::devices::channel::VOLUME as usize;
            v["nodes"][0]["channel"][volume]["layers"] = json!([]);
        });
        let (project, _) = load(&json).unwrap();
        assert!(project.signals.is_empty());
    }

    #[test]
    fn version_3_links_load_as_listening_layers() {
        let json = old_file_json(3, |v| {
            let volume = crate::devices::channel::VOLUME as usize;
            let layer = &mut v["nodes"][0]["channel"][volume]["layers"][0][1];
            layer.as_object_mut().unwrap().remove("signal");
            layer["blocks"][0][1]["link"] = json!({ "signal": 2, "low": 0.0, "high": 0.5 });
            v["signals"][0]["source"]["Notes"] = json!({
                "lowest": 36, "highest": 37, "length": 480,
                "shape": { "points": [{ "at": 0.0, "value": 0.0, "segment": "Linear" }] }
            });
        });
        let (project, _) = load(&json).unwrap();
        let volume =
            &project.node(FIRST_NODE).unwrap().channel[crate::devices::channel::VOLUME as usize];
        assert_eq!(volume.layers[0].1.signal, Some(SignalId(2)));
        let SignalSource::Notes(source) = &project.signals[&SignalId(2)].source else {
            panic!("expected a notes signal");
        };
        assert_eq!(source.decay, Ticks(480));
    }

    #[test]
    fn older_files_load_with_their_blend_modes_centred() {
        let json = old_file_json(2, |v| {
            let layer = &mut v["nodes"][0]["instrument"]["params"][0]["layers"][0][1];
            layer["mode"] = "Modulate".into();
        });
        let (project, _) = load(&json).unwrap();
        // Modulate was already centred: it becomes Add with the same values
        // (invert folded into the points).
        let cutoff = |p: &Project| {
            daw_automation::Automation::compile(
                &p.node(FIRST_NODE).unwrap().instruments[0].1.params[instrument::CUTOFF as usize],
            )
        };
        let (loaded, expected) = (cutoff(&project), cutoff(&rich_project()));
        for beat in [2.0, 2.1, 3.3, 5.9] {
            assert!((loaded.value_at(beat) - expected.value_at(beat)).abs() < 1e-9);
        }
        assert!(
            project.node(FIRST_NODE).unwrap().instruments[0].1.params[0].layers[0]
                .1
                .blocks
                .iter()
                .all(|(_, b)| !b.invert)
        );
    }

    #[test]
    fn links_to_missing_signals_are_rejected() {
        assert!(matches!(
            load(&file_json(|v| {
                v["signals"].as_array_mut().unwrap().remove(0);
            })),
            Err(LoadError::Invalid(CommandError::UnknownSignal { .. }))
        ));
    }

    #[test]
    fn loop_regions_are_validated() {
        assert!(matches!(
            load(&file_json(|v| {
                v["loop_range"] = serde_json::json!({ "start": 960, "end": 960 });
            })),
            Err(LoadError::Invalid(CommandError::InvalidLoop { .. }))
        ));
    }
}
