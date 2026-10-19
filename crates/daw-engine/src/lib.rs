//! The audio engine.
//!
//! The engine is the single source of truth for project state and can run
//! without any UI. It is split in two:
//!
//! - [`Engine`] lives on a control thread. It applies [`Command`]s, keeps undo
//!   history and produces [`Snapshot`]s.
//! - [`AudioProcessor`] lives on the audio thread (or in an offline render
//!   loop) and renders sound. It never allocates, frees, locks or blocks.
//!
//! They talk only through lock-free queues. Call [`Engine::maintain`]
//! regularly so queued messages are delivered and retired state is freed.

mod audio;
pub mod devices;
mod file;
mod history;
mod parallel;
mod project;
pub mod samples;
mod schedule;
mod transport;
mod upgrade;

pub use audio::{AudioProcessor, CONTROL_FRAMES};
pub use daw_protocol::{
    BlendMode, Block, BlockId, ClipId, Command, CommandError, DEFAULT_TRIGGER_NOTE, DeviceId,
    DeviceRef, DeviceSnapshot, Edit, Effect, EffectKind, Instrument, InstrumentKind, InvalidBlock,
    InvalidClip, InvalidSignal, Layer, LayerId, LoopRange, Meter, MidiClip, MidiNote, Node, NodeId,
    NodeKind, NodeSnapshot, NoteSet, NotesSource, ParamId, ParamSnapshot, ParamState, SampleId,
    SampleRef, SampleSnapshot, SampleStatus, Scale, Segment, Shape, ShapePoint, Signal, SignalId,
    SignalSnapshot, SignalSource, SignalUse, Snapshot, TICKS_PER_BEAT, Ticks,
};
pub use file::{LoadError, ProjectFile};
pub use project::{
    DEFAULT_TEMPO_BPM, FIRST_INSTRUMENT, FIRST_NODE, MAX_EFFECTS, MAX_INSTRUMENTS, MAX_NODES,
    MAX_TEMPO_BPM, MIN_TEMPO_BPM, NodeExt, Project, song, synth_node,
};
pub use schedule::{EffectState, NodeRender, RenderState, ScheduledNote};

use audio::{AudioMsg, SharedStatus};
use daw_automation::{Automation, Signals};
use devices::{ParamInfo, effect_name};
use history::History;
use samples::{SampleData, SamplePool};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// Capacity of each control/audio queue. Generous, because a full queue only
/// delays delivery until the next [`Engine::maintain`].
const QUEUE_CAPACITY: usize = 1024;

/// Shortest allowed loop: a 64th note.
pub const MIN_LOOP_LENGTH: Ticks = Ticks(TICKS_PER_BEAT / 16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    pub sample_rate: u32,
    pub channels: u16,
    /// Threads that render, the audio thread included: 1 renders every
    /// node on the audio thread; more render the nodes of the tree in
    /// parallel. The result is the same either way.
    pub threads: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            threads: 1,
        }
    }
}

/// How many threads to render with when the user leaves it to the app: one
/// per core, keeping one core free for the rest of the system, at most 8.
pub fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get().saturating_sub(1))
        .clamp(1, 8)
}

#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
    project: Project,
    history: History,
    revision: u64,
    /// Like `revision`, but only for changes that are saved in the project
    /// file (not play, stop or locate).
    content_revision: u64,
    playing: bool,
    loop_range: Option<LoopRange>,
    outbox: VecDeque<AudioMsg>,
    to_audio: rtrb::Producer<AudioMsg>,
    garbage: rtrb::Consumer<Box<RenderState>>,
    status: Arc<SharedStatus>,
    /// Decoded sample audio, read from the project folder as needed.
    samples: SamplePool,
}

impl Engine {
    /// Creates an engine with an empty project, and the processor that renders it.
    ///
    /// # Panics
    ///
    /// If `config.channels` or `config.sample_rate` is zero.
    pub fn new(config: EngineConfig) -> (Self, AudioProcessor) {
        assert!(config.channels > 0, "engine needs at least one channel");
        assert!(config.sample_rate > 0, "engine needs a sample rate");
        let project = Project::default();
        let (to_audio, from_control) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (garbage_tx, garbage) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let status = Arc::new(SharedStatus::default());
        let processor = AudioProcessor::new(
            config.sample_rate,
            usize::from(config.channels),
            config.threads.max(1),
            Box::new(RenderState::compile(&project)),
            from_control,
            garbage_tx,
            Arc::clone(&status),
        );
        let engine = Self {
            config,
            project,
            history: History::default(),
            revision: 0,
            content_revision: 0,
            playing: false,
            loop_range: None,
            outbox: VecDeque::new(),
            to_audio,
            garbage,
            status,
            samples: SamplePool::default(),
        };
        (engine, processor)
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Makes a new processor for another device setup (sample rate,
    /// channels, threads), carrying over the song, undo history, transport
    /// position, playing and the loop. The old processor receives nothing
    /// more: stop its stream and drop it.
    pub fn reconnect(&mut self, config: EngineConfig) -> AudioProcessor {
        assert!(config.channels > 0, "engine needs at least one channel");
        assert!(config.sample_rate > 0, "engine needs a sample rate");
        let position = self.status.position_beats();
        let (to_audio, from_control) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (garbage_tx, garbage) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let status = Arc::new(SharedStatus::default());
        status.set_position_beats(position);
        let processor = AudioProcessor::new(
            config.sample_rate,
            usize::from(config.channels),
            config.threads.max(1),
            Box::new(RenderState::compile_with(&self.project, &self.samples)),
            from_control,
            garbage_tx,
            Arc::clone(&status),
        );
        while let Ok(retired) = self.garbage.pop() {
            drop(retired);
        }
        self.to_audio = to_audio;
        self.garbage = garbage;
        self.status = status;
        self.config = config;
        // Whatever the old processor had not received is moot now.
        self.outbox.clear();
        self.outbox.push_back(AudioMsg::Locate(position));
        let loop_range = self.loop_range;
        self.set_loop(loop_range);
        if self.playing {
            self.outbox.push_back(AudioMsg::SetPlaying(true));
        }
        self.revision += 1;
        self.maintain();
        processor
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Applies a command. A rejected command changes nothing.
    pub fn apply(&mut self, command: Command) -> Result<(), CommandError> {
        match command {
            Command::Play => {
                self.playing = true;
                self.outbox.push_back(AudioMsg::SetPlaying(true));
            }
            Command::Stop => {
                self.playing = false;
                self.outbox.push_back(AudioMsg::SetPlaying(false));
            }
            Command::Locate { position } => {
                if position < Ticks::ZERO {
                    return Err(CommandError::NegativePosition { position });
                }
                self.outbox.push_back(AudioMsg::Locate(position.as_beats()));
            }
            Command::PreviewNote {
                node,
                pitch,
                velocity,
                on,
            } => {
                if self.project.node(node).is_none() {
                    return Err(CommandError::UnknownNode { id: node });
                }
                self.outbox.push_back(AudioMsg::Preview {
                    node,
                    pitch: pitch.min(127),
                    velocity: velocity.clamp(1, 127),
                    on,
                });
            }
            Command::TestTone => self.outbox.push_back(AudioMsg::TestTone),
            Command::SetLoop(range) => {
                if let Some(range) = range {
                    check_loop(range)?;
                }
                self.set_loop(range);
                self.content_revision += 1;
            }
            Command::Edit(edit) => {
                self.history.edit(&mut self.project, edit)?;
                self.publish_state();
                self.content_revision += 1;
            }
            Command::Undo => {
                self.history.undo(&mut self.project)?;
                self.publish_state();
                self.content_revision += 1;
            }
            Command::Redo => {
                self.history.redo(&mut self.project)?;
                self.publish_state();
                self.content_revision += 1;
            }
        }
        self.revision += 1;
        self.maintain();
        Ok(())
    }

    /// The project folder, where sample files are found. Samples that could
    /// not be read are tried again.
    pub fn set_sample_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.samples.set_dir(dir);
        self.publish_state();
        self.revision += 1;
        self.maintain();
    }

    /// Puts decoded audio in the sample pool as if read from `file` (for
    /// generated sounds and tests).
    pub fn insert_sample_data(&mut self, id: SampleId, file: &str, data: SampleData) {
        self.samples.insert(id, file, data);
        self.publish_state();
        self.revision += 1;
        self.maintain();
    }

    pub fn snapshot(&self) -> Snapshot {
        let position_beats = self.status.position_beats();
        let signals = schedule::compile_signals(&self.project);
        let listeners = self.project.listeners();
        Snapshot {
            revision: self.revision,
            song_name: self.project.name.clone(),
            tempo_bpm: self.project.tempo_bpm,
            playing: self.playing,
            position_beats,
            loop_range: self.loop_range,
            nodes: self
                .project
                .nodes()
                .into_iter()
                .map(|entry| node_snapshot(entry, &signals, position_beats))
                .collect(),
            signals: self
                .project
                .signals
                .iter()
                .map(|(&id, signal)| SignalSnapshot {
                    id,
                    name: signal.name.clone(),
                    color: signal.color,
                    source: signal.source.clone(),
                    used_by: listeners
                        .iter()
                        .filter(|(_, listened)| *listened == id)
                        .map(|(used, _)| *used)
                        .collect(),
                })
                .collect(),
            samples: self
                .project
                .samples
                .iter()
                .map(|(&id, sample)| SampleSnapshot {
                    id,
                    name: sample.name.clone(),
                    file: sample.file.clone(),
                    status: self.samples.status(id, &sample.file),
                })
                .collect(),
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
        }
    }

    /// Increases every time a command changes engine state; a front end
    /// only needs a new [`Snapshot`] when this changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Changes whenever saved content changes: edits, undo, redo, the loop
    /// region, or loading. Compare with a stored value to know whether the
    /// project has unsaved changes.
    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    /// Transport position in beats, as last reported by the audio side.
    /// Cheap enough to call every UI frame.
    pub fn position_beats(&self) -> f64 {
        self.status.position_beats()
    }

    /// Peak output levels since the last call. Call once per UI frame.
    pub fn take_meter(&self) -> Meter {
        let (left, right) = self.status.take_peaks();
        Meter { left, right }
    }

    /// Each node's peak output levels since the last call. Call once per UI
    /// frame.
    pub fn take_meters(&self) -> BTreeMap<NodeId, Meter> {
        self.status
            .take_node_peaks()
            .into_iter()
            .map(|(node, left, right)| (node, Meter { left, right }))
            .collect()
    }

    /// Delivers queued messages to the audio side and frees retired state.
    /// Call regularly from the control thread.
    pub fn maintain(&mut self) {
        while let Ok(retired) = self.garbage.pop() {
            drop(retired);
        }
        while let Some(msg) = self.outbox.pop_front() {
            if let Err(rtrb::PushError::Full(msg)) = self.to_audio.push(msg) {
                self.outbox.push_front(msg);
                break;
            }
        }
    }

    /// Forgets undo history, so the current state becomes the starting
    /// point. Used after setting up a new project.
    pub fn clear_history(&mut self) {
        self.history = History::default();
        self.revision += 1;
    }

    /// The project as a file, ready to save.
    pub fn to_file(&self) -> ProjectFile {
        ProjectFile::new(&self.project, self.loop_range)
    }

    /// Replaces the project with one loaded from a file. The transport stops
    /// at the start and undo history is cleared. A file that fails to load
    /// changes nothing.
    pub fn load(&mut self, file: ProjectFile) -> Result<(), LoadError> {
        let (project, loop_range) = file.into_project()?;
        self.project = project;
        self.history = History::default();
        self.playing = false;
        self.outbox.push_back(AudioMsg::SetPlaying(false));
        self.outbox.push_back(AudioMsg::Locate(0.0));
        self.set_loop(loop_range);
        self.publish_state();
        self.revision += 1;
        self.content_revision += 1;
        self.maintain();
        Ok(())
    }

    fn set_loop(&mut self, range: Option<LoopRange>) {
        self.loop_range = range;
        let beats = range.map(|r| (r.start.as_beats(), r.end.as_beats()));
        self.outbox.push_back(AudioMsg::SetLoop(beats));
    }

    fn publish_state(&mut self) {
        // Only the newest state matters; drop any that were never delivered.
        self.outbox
            .retain(|msg| !matches!(msg, AudioMsg::SwapState(_)));
        self.samples.refresh(&self.project.samples);
        let state = Box::new(RenderState::compile_with(&self.project, &self.samples));
        self.outbox.push_back(AudioMsg::SwapState(state));
    }
}

fn node_snapshot(
    entry: project::NodeEntry<'_>,
    signals: &Signals,
    position_beats: f64,
) -> NodeSnapshot {
    let node = entry.node;
    NodeSnapshot {
        id: entry.id,
        kind: node.kind,
        parent: entry.parent,
        depth: entry.depth,
        children: node.children.iter().map(|(id, _)| *id).collect(),
        name: node.name.clone(),
        color: node.color,
        muted: node.muted,
        soloed: node.soloed,
        clips: node
            .clips
            .iter()
            .map(|(&id, clip)| (id, clip.clone()))
            .collect(),
        devices: node
            .devices()
            .map(|(device, params)| {
                let infos = node.param_infos(device).unwrap_or(&[]);
                DeviceSnapshot {
                    device,
                    effect: match device {
                        DeviceRef::Effect(id) => node.effect(id).map(|(_, e)| e.kind),
                        _ => None,
                    },
                    instrument: match device {
                        DeviceRef::Instrument(id) => node.instrument(id).map(|(_, i)| i.kind),
                        _ => None,
                    },
                    sample: match device {
                        DeviceRef::Instrument(id) => {
                            node.instrument(id).and_then(|(_, i)| i.sample)
                        }
                        _ => None,
                    },
                    plays: match device {
                        DeviceRef::Instrument(id) => node
                            .instrument(id)
                            .map_or(daw_protocol::NoteSet::NONE, |(_, i)| i.plays()),
                        _ => daw_protocol::NoteSet::NONE,
                    },
                    trigger: match device {
                        DeviceRef::Instrument(id) => node
                            .instrument(id)
                            .filter(|(_, i)| i.kind == InstrumentKind::SingleSampler)
                            .map(|(_, i)| i.note.unwrap_or(daw_protocol::DEFAULT_TRIGGER_NOTE)),
                        _ => None,
                    },
                    enabled: match device {
                        DeviceRef::Effect(id) => node.effect(id).is_none_or(|(_, e)| e.enabled),
                        DeviceRef::Instrument(id) => {
                            node.instrument(id).is_none_or(|(_, i)| i.enabled)
                        }
                        DeviceRef::Channel => true,
                    },
                    name: device_name(node, device).to_owned(),
                    params: params
                        .iter()
                        .zip(infos)
                        .enumerate()
                        .map(|(index, (state, info))| {
                            let id = ParamId {
                                node: entry.id,
                                device,
                                index: index as u32,
                            };
                            param_snapshot(id, state, info, signals, position_beats)
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

fn device_name(node: &Node, device: DeviceRef) -> &'static str {
    match device {
        DeviceRef::Instrument(id) => node
            .instrument(id)
            .map_or("Instrument", |(_, i)| devices::instrument_name(i.kind)),
        DeviceRef::Channel => "Channel",
        DeviceRef::Effect(id) => node
            .effects
            .iter()
            .find(|(e, _)| *e == id)
            .map_or("Effect", |(_, effect)| effect_name(effect.kind)),
    }
}

fn param_snapshot(
    id: ParamId,
    state: &ParamState,
    info: &ParamInfo,
    signals: &Signals,
    position_beats: f64,
) -> ParamSnapshot {
    let value = Automation::compile_with(state, signals).value_at(position_beats);
    ParamSnapshot {
        id,
        name: info.name.to_owned(),
        scale: info.scale,
        unit: info.unit.to_owned(),
        choices: info.choices.iter().map(|&c| c.to_owned()).collect(),
        base: state.base,
        value,
        display: info.format(info.scale.to_plain(value)),
        layers: state.layers.clone(),
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

/// Checks a loop region: it must start at or after the song start and be at
/// least [`MIN_LOOP_LENGTH`] long.
pub(crate) fn check_loop(range: LoopRange) -> Result<(), CommandError> {
    if range.start < Ticks::ZERO || range.end - range.start < MIN_LOOP_LENGTH {
        return Err(CommandError::InvalidLoop {
            range,
            min_length: MIN_LOOP_LENGTH,
        });
    }
    Ok(())
}
