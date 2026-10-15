//! The project file: a versioned, human-readable JSON document.
//!
//! Saving writes the project out as plain data. Loading never trusts that
//! data: it rebuilds the project by applying the same validated edits the UI
//! uses, so a hand-edited or damaged file can never produce an invalid
//! project. Undo history and transport position are not saved.

use crate::devices::{CHANNEL_PARAMS, INSTRUMENT_PARAMS};
use crate::project::Project;
use daw_protocol::{
    ClipId, CommandError, DeviceId, DeviceRef, Edit, Effect, LoopRange, MidiClip, ParamId,
    ParamState,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifies the file type, so other JSON files are rejected clearly.
pub const FORMAT: &str = "daw-project";
/// Bumped whenever the layout changes; older versions are converted on load.
pub const VERSION: u32 = 1;
/// Number of colours in the content palette.
const PALETTE_SIZE: u8 = 9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub tempo_bpm: f64,
    pub loop_range: Option<LoopRange>,
    /// The project's nodes. Version 1 holds exactly one.
    pub nodes: Vec<NodeFile>,
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
        let mut edits = vec![
            Edit::RenameSong { name: self.name },
            Edit::SetTempo {
                bpm: self.tempo_bpm,
            },
            Edit::RenameNode { name: node.name },
            Edit::SetMute { muted: node.muted },
            Edit::SetSolo {
                soloed: node.soloed,
            },
        ];
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
        BlendMode, Block, BlockId, EffectKind, Layer, LayerId, MidiNote, Segment, Shape,
        ShapePoint, Ticks,
    };

    /// A project using every kind of content.
    fn rich_project() -> Project {
        let mut project = Project::default();
        let mut layer = Layer::new(BlendMode::Modulate);
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
        let edits = [
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
        assert!(json.contains("\"mode\": \"Modulate\""));
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
    fn loop_regions_are_validated() {
        assert!(matches!(
            load(&file_json(|v| {
                v["loop_range"] = serde_json::json!({ "start": 960, "end": 960 });
            })),
            Err(LoadError::Invalid(CommandError::InvalidLoop { .. }))
        ));
    }
}
