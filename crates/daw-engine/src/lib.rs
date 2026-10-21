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
mod clips;
pub mod devices;
mod file;
mod history;
mod parallel;
mod plugins;
mod project;
pub mod samples;
mod schedule;
mod transport;
mod upgrade;

pub use audio::{AudioProcessor, CONTROL_FRAMES};
pub use daw_protocol::{
    AudioClip, BlendMode, Block, BlockId, ClipId, ClipLoop, Command, CommandError,
    DEFAULT_TRIGGER_NOTE, DeviceId, DeviceRef, DeviceSnapshot, Edit, Effect, EffectKind,
    Instrument, InstrumentKind, InvalidBlock, InvalidClip, InvalidSignal, Layer, LayerId,
    LoopRange, MAX_CLIP_BPM, MAX_CLIP_GAIN_DB, MIN_CLIP_BPM, MIN_CLIP_GAIN_DB, Meter, MidiClip,
    MidiNote, MinMax, Node, NodeId, NodeKind, NodeSnapshot, NoteSet, NotesSource, ParamId,
    ParamSnapshot, ParamState, Peaks, Piece, PluginData, PluginParam, PluginParamInfo,
    PluginSnapshot, PluginSpec, PluginState, SEAM_FADE, SampleId, SampleRef, SampleSnapshot,
    SampleStatus, Scale, Segment, Shape, ShapePoint, Signal, SignalId, SignalSnapshot,
    SignalSource, SignalUse, Snapshot, TICKS_PER_BEAT, TestPlugin, Ticks,
};
pub use file::{LoadError, ProjectFile};
pub use project::{
    DEFAULT_TEMPO_BPM, DeviceParams, FIRST_INSTRUMENT, FIRST_NODE, MAX_EFFECTS, MAX_INSTRUMENTS,
    MAX_NODES, MAX_PLUGIN_PARAMS, MAX_TEMPO_BPM, MIN_TEMPO_BPM, NodeExt, Project, song, synth_node,
};
pub use schedule::{EffectState, NodeRender, RenderState, ScheduledNote};

use audio::{AudioMsg, AudioQueues, SharedStatus};
use daw_automation::{Automation, Signals};
use devices::{ParamInfo, effect_name};
use history::History;
use samples::{SampleData, SamplePool};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// Capacity of each control/audio queue. Generous, because a full queue only
/// delays delivery until the next [`Engine::maintain`].
const QUEUE_CAPACITY: usize = 1024;
/// Room for every plugin channel there can be, so returning one never fails.
const RETIRED_CAPACITY: usize = MAX_NODES * MAX_EFFECTS;

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
    /// Whether playback loops over `loop_range` (never without one).
    loop_enabled: bool,
    outbox: VecDeque<AudioMsg>,
    to_audio: rtrb::Producer<AudioMsg>,
    garbage: rtrb::Consumer<Box<RenderState>>,
    status: Arc<SharedStatus>,
    /// Decoded sample audio, read from the project folder as needed.
    samples: SamplePool,
    /// Plugin effects' instances, in host processes.
    plugins: plugins::Plugins,
    /// Plugin channels the audio side let go of, to free here.
    retired: rtrb::Consumer<Box<daw_plugins::block::DawBlock>>,
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
        let (retired_tx, retired) = rtrb::RingBuffer::new(RETIRED_CAPACITY);
        let status = Arc::new(SharedStatus::default());
        let processor = AudioProcessor::new(
            config.sample_rate,
            usize::from(config.channels),
            config.threads.max(1),
            Box::new(RenderState::compile(&project)),
            AudioQueues {
                from_control,
                garbage: garbage_tx,
                retired: retired_tx,
            },
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
            loop_enabled: false,
            outbox: VecDeque::new(),
            to_audio,
            garbage,
            status,
            samples: SamplePool::default(),
            plugins: plugins::Plugins::default(),
            retired,
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
        self.samples
            .convert_clips(&self.project, config.sample_rate);
        let (to_audio, from_control) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (garbage_tx, garbage) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let (retired_tx, retired) = rtrb::RingBuffer::new(RETIRED_CAPACITY);
        let status = Arc::new(SharedStatus::default());
        status.set_position_beats(position);
        let processor = AudioProcessor::new(
            config.sample_rate,
            usize::from(config.channels),
            config.threads.max(1),
            Box::new(RenderState::compile_with(
                &self.project,
                &self.samples,
                config.sample_rate,
            )),
            AudioQueues {
                from_control,
                garbage: garbage_tx,
                retired: retired_tx,
            },
            Arc::clone(&status),
        );
        while let Ok(retired) = self.garbage.pop() {
            drop(retired);
        }
        while let Ok(block) = self.retired.pop() {
            self.plugins.retired(block);
        }
        self.retired = retired;
        self.to_audio = to_audio;
        self.garbage = garbage;
        self.status = status;
        self.config = config;
        // Whatever the old processor had not received is moot now.
        self.outbox.clear();
        self.outbox.push_back(AudioMsg::Locate(position));
        self.send_loop();
        if self.playing {
            self.outbox.push_back(AudioMsg::SetPlaying(true));
        }
        self.plugins
            .reactivate(&self.project, config.sample_rate, &mut self.outbox);
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
            Command::ReloadPlugins => {
                self.plugins
                    .reload(&self.project, self.config.sample_rate, &mut self.outbox);
                self.revision += 1;
            }
            Command::SetLoop(range) => {
                if let Some(range) = range {
                    check_loop(range)?;
                }
                self.set_loop(range, range.is_some());
                self.content_revision += 1;
            }
            Command::SetLoopEnabled(on) => {
                let range = self.loop_range;
                self.set_loop(range, on);
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

    /// How to start a plugin host process (the app runs itself in
    /// plugin-host mode). Without one, plugin effects are silent and say so.
    /// For a front end's tests, which run no plugin host: the parameters
    /// a plugin effect would list if it were running.
    #[doc(hidden)]
    pub fn show_plugin_params(&mut self, effect: DeviceId, params: Vec<PluginParamInfo>) {
        self.plugins.show_params(effect, params);
        self.revision += 1;
    }

    pub fn set_plugin_host(&mut self, command: daw_plugins::client::HostCommand) {
        self.plugins.set_command(command);
        self.publish_state();
        self.revision += 1;
        self.maintain();
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
        let mut snapshot = Snapshot {
            revision: self.revision,
            song_name: self.project.name.clone(),
            tempo_bpm: self.project.tempo_bpm,
            playing: self.playing,
            position_beats,
            loop_range: self.loop_range,
            loop_enabled: self.loop_enabled,
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
                    peaks: self.samples.peaks(id, &sample.file).cloned(),
                })
                .collect(),
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
        };
        for node in &mut snapshot.nodes {
            let Some(project_node) = self.project.node(node.id) else {
                continue;
            };
            for device in &mut node.devices {
                if let DeviceRef::Effect(id) = device.device
                    && let Some((_, effect)) = project_node.effect(id)
                    && let Some(spec) = &effect.plugin
                {
                    device.plugin = Some(PluginSnapshot {
                        spec: spec.clone(),
                        state: self
                            .plugins
                            .state(id)
                            .unwrap_or_else(|| PluginState::Failed("not started".into())),
                        params: self.plugins.params(id),
                    });
                }
            }
        }
        snapshot
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
        while let Ok(block) = self.retired.pop() {
            self.plugins.retired(block);
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

    /// The project as a file, ready to save. Running plugins are asked for
    /// their state (a call to their host process each).
    pub fn to_file(&mut self) -> ProjectFile {
        let mut file = ProjectFile::new(&self.project, self.loop_range);
        file.loop_enabled = self.loop_enabled;
        file.set_plugin_states(&self.plugins.states(&self.project));
        file
    }

    /// Replaces the project with one loaded from a file. The transport stops
    /// at the start and undo history is cleared. A file that fails to load
    /// changes nothing.
    pub fn load(&mut self, file: ProjectFile) -> Result<(), LoadError> {
        let loop_enabled = file.loop_enabled;
        let states = file.plugin_states();
        let (project, loop_range) = file.into_project()?;
        self.project = project;
        // Before the new state, so the old song's plugins go quiet first.
        self.plugins.replace_song(states, &mut self.outbox);
        self.history = History::default();
        self.playing = false;
        self.outbox.push_back(AudioMsg::SetPlaying(false));
        self.outbox.push_back(AudioMsg::Locate(0.0));
        self.set_loop(loop_range, loop_enabled);
        self.publish_state();
        self.revision += 1;
        self.content_revision += 1;
        self.maintain();
        Ok(())
    }

    fn set_loop(&mut self, range: Option<LoopRange>, enabled: bool) {
        self.loop_range = range;
        self.loop_enabled = enabled && range.is_some();
        self.send_loop();
    }

    /// Tells the audio side the loop that plays: the region, if looping.
    fn send_loop(&mut self) {
        let beats = self
            .loop_range
            .filter(|_| self.loop_enabled)
            .map(|r| (r.start.as_beats(), r.end.as_beats()));
        self.outbox.push_back(AudioMsg::SetLoop(beats));
    }

    fn publish_state(&mut self) {
        // Only the newest state matters; drop any that were never delivered.
        self.outbox
            .retain(|msg| !matches!(msg, AudioMsg::SwapState(_)));
        self.samples.refresh(&self.project.samples);
        self.samples
            .convert_clips(&self.project, self.config.sample_rate);
        let state = Box::new(RenderState::compile_with(
            &self.project,
            &self.samples,
            self.config.sample_rate,
        ));
        self.outbox.push_back(AudioMsg::SwapState(state));
        // After the state: its plugin effects exist when their channels
        // arrive.
        self.plugins
            .sync(&self.project, self.config.sample_rate, &mut self.outbox);
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
        audio_clips: node
            .audio_clips
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
                    plugin: None,
                    name: device_name(node, device),
                    params: match params {
                        DeviceParams::Indexed(states) => states
                            .iter()
                            .zip(infos)
                            .enumerate()
                            .map(|(index, (state, info))| {
                                let id = ParamId {
                                    node: entry.id,
                                    device,
                                    index: index as u32,
                                };
                                param_snapshot(id, state, info, info.name, signals, position_beats)
                            })
                            .collect(),
                        DeviceParams::Plugin(params) => params
                            .iter()
                            .map(|param| {
                                let id = ParamId {
                                    node: entry.id,
                                    device,
                                    index: param.id,
                                };
                                param_snapshot(
                                    id,
                                    &param.state,
                                    &devices::PLUGIN_PARAM,
                                    &param.name,
                                    signals,
                                    position_beats,
                                )
                            })
                            .collect(),
                    },
                }
            })
            .collect(),
    }
}

/// A device's name; a plugin's own name for a plugin effect.
fn device_name(node: &Node, device: DeviceRef) -> String {
    match device {
        DeviceRef::Instrument(id) => node
            .instrument(id)
            .map_or("Instrument", |(_, i)| devices::instrument_name(i.kind))
            .to_owned(),
        DeviceRef::Channel => "Channel".to_owned(),
        DeviceRef::Effect(id) => node.effects.iter().find(|(e, _)| *e == id).map_or(
            "Effect".to_owned(),
            |(_, effect)| match &effect.plugin {
                Some(spec) => spec.name().to_owned(),
                None => effect_name(effect.kind).to_owned(),
            },
        ),
    }
}

fn param_snapshot(
    id: ParamId,
    state: &ParamState,
    info: &ParamInfo,
    name: &str,
    signals: &Signals,
    position_beats: f64,
) -> ParamSnapshot {
    let value = Automation::compile_with(state, signals).value_at(position_beats);
    ParamSnapshot {
        id,
        name: name.to_owned(),
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
