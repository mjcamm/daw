//! The project file: a versioned, human-readable JSON document.
//!
//! Saving writes the project out as plain data. Loading never trusts that
//! data: it rebuilds the project by applying the same validated edits the UI
//! uses, so a hand-edited or damaged file can never produce an invalid
//! project. Undo history and transport position are not saved.

use crate::devices::{CHANNEL_PARAMS, INSTRUMENT_PARAMS};
use crate::project::{PALETTE_SIZE, Project};
use daw_protocol::{
    ClipId, CommandError, DeviceId, DeviceRef, Edit, Effect, LoopRange, MidiClip, ParamId,
    ParamState, Signal, SignalId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifies the file type, so other JSON files are rejected clearly.
pub const FORMAT: &str = "daw-project";
/// Bumped whenever the layout changes; older versions are converted on load.
/// Version 2 added motion signals; version 3 centred the blend modes;
/// version 4 moved listening to signals onto layers and made notes signals
/// ADSR hits.
pub const VERSION: u32 = 6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub tempo_bpm: f64,
    pub loop_range: Option<LoopRange>,
    /// The project's nodes. Exactly one for now.
    pub nodes: Vec<NodeFile>,
    /// Motion signals. Missing in version 1 files.
    #[serde(default)]
    pub signals: Vec<SignalFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalFile {
    pub id: SignalId,
    #[serde(flatten)]
    pub signal: Signal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeFile {
    pub name: String,
    pub color: u8,
    pub muted: bool,
    pub soloed: bool,
    pub instrument: InstrumentFile,
    pub effects: Vec<EffectFile>,
    /// Volume and pan, in parameter order.
    pub channel: Vec<ParamState>,
    pub clips: Vec<ClipFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstrumentKind {
    Synth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentFile {
    pub kind: InstrumentKind,
    /// In parameter order.
    pub params: Vec<ParamState>,
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
    /// This version supports exactly one node.
    NodeCount(usize),
    /// A device's parameter list does not match the device.
    ParamCount {
        device: &'static str,
        expected: usize,
        found: usize,
    },
    BadColor(u8),
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
            Self::NodeCount(n) => write!(f, "expected one node, found {n}"),
            Self::ParamCount {
                device,
                expected,
                found,
            } => write!(f, "{device} has {found} parameters, expected {expected}"),
            Self::BadColor(c) => write!(f, "colour {c} is not in the palette"),
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

impl ProjectFile {
    /// Captures a project and its loop region.
    pub fn new(project: &Project, loop_range: Option<LoopRange>) -> Self {
        let node = &project.node;
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            name: project.name.clone(),
            tempo_bpm: project.tempo_bpm,
            loop_range,
            nodes: vec![NodeFile {
                name: node.name.clone(),
                color: node.color,
                muted: node.muted,
                soloed: node.soloed,
                instrument: InstrumentFile {
                    kind: InstrumentKind::Synth,
                    params: node.instrument.clone(),
                },
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
            }],
            signals: project
                .signals
                .iter()
                .map(|(id, signal)| SignalFile {
                    id: *id,
                    signal: signal.clone(),
                })
                .collect(),
        }
    }

    pub fn to_json(&self) -> String {
        // Plain data with string keys: serialising cannot fail.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Result<Self, LoadError> {
        // Check the header first, so a newer or foreign file gets a clear
        // message instead of a confusing field error.
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
            crate::upgrade::notes_switches(&mut value);
            return serde_json::from_value(value).map_err(LoadError::Json);
        }
        serde_json::from_str(json).map_err(LoadError::Json)
    }

    /// Rebuilds the project through validated edits, returning it with its
    /// loop region.
    pub fn into_project(self) -> Result<(Project, Option<LoopRange>), LoadError> {
        let [node]: [NodeFile; 1] = self
            .nodes
            .try_into()
            .map_err(|nodes: Vec<NodeFile>| LoadError::NodeCount(nodes.len()))?;
        check_count(
            "Synth",
            INSTRUMENT_PARAMS.len(),
            node.instrument.params.len(),
        )?;
        check_count("Channel", CHANNEL_PARAMS.len(), node.channel.len())?;
        if node.color >= PALETTE_SIZE {
            return Err(LoadError::BadColor(node.color));
        }
        if let Some(range) = self.loop_range {
            crate::check_loop(range)?;
        }

        let mut project = Project::default();
        // Signals first, so the layers that listen to them resolve.
        let mut edits: Vec<Edit> = self
            .signals
            .into_iter()
            .map(|s| Edit::AddSignal {
                id: s.id,
                signal: s.signal,
                listeners: Vec::new(),
            })
            .collect();
        edits.extend([
            Edit::RenameSong { name: self.name },
            Edit::SetTempo {
                bpm: self.tempo_bpm,
            },
            Edit::RenameNode { name: node.name },
            Edit::SetMute { muted: node.muted },
            Edit::SetSolo {
                soloed: node.soloed,
            },
        ]);
        edits.extend(param_edits(DeviceRef::Instrument, node.instrument.params));
        edits.extend(param_edits(DeviceRef::Channel, node.channel));
        edits.extend(
            node.effects
                .into_iter()
                .enumerate()
                .map(|(index, e)| Edit::AddEffect {
                    id: e.id,
                    index,
                    effect: e.effect,
                }),
        );
        edits.extend(node.clips.into_iter().map(|c| Edit::AddClip {
            id: c.id,
            clip: c.clip,
        }));
        for edit in edits {
            project.apply(edit)?;
        }
        project.node.color = node.color;
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
fn param_edits(device: DeviceRef, params: Vec<ParamState>) -> impl Iterator<Item = Edit> {
    params
        .into_iter()
        .enumerate()
        .flat_map(move |(index, state)| {
            let param = ParamId {
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
                name: "Lead".to_owned(),
            },
            Edit::SetMute { muted: true },
            Edit::AddEffect {
                id: DeviceId(3),
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            },
            Edit::SetParam {
                param: ParamId {
                    device: DeviceRef::Effect(DeviceId(3)),
                    index: filter::RESONANCE,
                },
                value: 0.4,
            },
            Edit::AddLayer {
                param: ParamId {
                    device: DeviceRef::Instrument,
                    index: instrument::CUTOFF,
                },
                id: LayerId(5),
                index: 0,
                layer,
            },
            Edit::AddLayer {
                param: ParamId {
                    device: DeviceRef::Channel,
                    index: crate::devices::channel::VOLUME,
                },
                id: LayerId(6),
                index: 0,
                layer: ducking,
            },
            Edit::AddClip {
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
        project.node.color = 3;
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
            load(&file_json(|v| v["nodes"][0]["name"] = " ".into())),
            Err(LoadError::Invalid(CommandError::EmptyName))
        ));
        assert!(matches!(
            load(&file_json(|v| v["nodes"][0]["color"] = 42.into())),
            Err(LoadError::BadColor(42))
        ));
        assert!(matches!(
            load(&file_json(|v| v["nodes"] = serde_json::json!([]))),
            Err(LoadError::NodeCount(0))
        ));
        assert!(matches!(
            load(&file_json(|v| {
                v["nodes"][0]["channel"].as_array_mut().unwrap().pop();
            })),
            Err(LoadError::ParamCount { .. })
        ));
        // A duplicate clip id.
        assert!(matches!(
            load(&file_json(|v| {
                let clips = v["nodes"][0]["clips"].as_array_mut().unwrap();
                let copy = clips[0].clone();
                clips.push(copy);
            })),
            Err(LoadError::Invalid(CommandError::DuplicateClip { .. }))
        ));
        // A block shape value out of range.
        assert!(matches!(
            load(&file_json(|v| {
                v["nodes"][0]["instrument"]["params"][0]["layers"][0][1]["blocks"][0][1]["shape"]
                    ["points"][0]["value"] = 3.into();
            })),
            Err(LoadError::Invalid(CommandError::InvalidBlock { .. }))
        ));
    }

    #[test]
    fn version_1_files_without_signals_still_load() {
        let json = file_json(|v| {
            v["version"] = 1.into();
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
        let json = file_json(|v| {
            v["version"] = 3.into();
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
        let volume = &project.node.channel[crate::devices::channel::VOLUME as usize];
        assert_eq!(volume.layers[0].1.signal, Some(SignalId(2)));
        let SignalSource::Notes(source) = &project.signals[&SignalId(2)].source else {
            panic!("expected a notes signal");
        };
        assert_eq!(source.decay, Ticks(480));
    }

    #[test]
    fn older_files_load_with_their_blend_modes_centred() {
        let json = file_json(|v| {
            v["version"] = 2.into();
            let layer = &mut v["nodes"][0]["instrument"]["params"][0]["layers"][0][1];
            layer["mode"] = "Modulate".into();
        });
        let (project, _) = load(&json).unwrap();
        // Modulate was already centred: it becomes Add with the same values
        // (invert folded into the points).
        let cutoff = |p: &Project| {
            daw_automation::Automation::compile(&p.node.instrument[instrument::CUTOFF as usize])
        };
        let (loaded, expected) = (cutoff(&project), cutoff(&rich_project()));
        for beat in [2.0, 2.1, 3.3, 5.9] {
            assert!((loaded.value_at(beat) - expected.value_at(beat)).abs() < 1e-9);
        }
        assert!(
            project.node.instrument[0].layers[0]
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
