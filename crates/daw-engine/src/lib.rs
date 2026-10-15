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
mod history;
mod project;
mod schedule;
mod transport;

pub use audio::AudioProcessor;
pub use daw_protocol::{
    ClipId, ClipSummary, Command, CommandError, Edit, InvalidClip, LoopRange, MidiClip, MidiNote,
    Snapshot, TICKS_PER_BEAT, Ticks,
};
pub use project::{DEFAULT_TEMPO_BPM, MAX_TEMPO_BPM, MAX_VOLUME_DB, MIN_TEMPO_BPM, Node, Project};
pub use schedule::{RenderState, ScheduledNote};

use audio::{AudioMsg, SharedStatus};
use history::History;
use std::collections::VecDeque;
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
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
        }
    }
}

#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
    project: Project,
    history: History,
    revision: u64,
    playing: bool,
    loop_range: Option<LoopRange>,
    outbox: VecDeque<AudioMsg>,
    to_audio: rtrb::Producer<AudioMsg>,
    garbage: rtrb::Consumer<Box<RenderState>>,
    status: Arc<SharedStatus>,
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
            playing: false,
            loop_range: None,
            outbox: VecDeque::new(),
            to_audio,
            garbage,
            status,
        };
        (engine, processor)
    }

    pub fn config(&self) -> EngineConfig {
        self.config
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
            Command::SetLoop(range) => {
                if let Some(range) = range
                    && (range.start < Ticks::ZERO || range.end - range.start < MIN_LOOP_LENGTH)
                {
                    return Err(CommandError::InvalidLoop {
                        range,
                        min_length: MIN_LOOP_LENGTH,
                    });
                }
                self.loop_range = range;
                let beats = range.map(|r| (r.start.as_beats(), r.end.as_beats()));
                self.outbox.push_back(AudioMsg::SetLoop(beats));
            }
            Command::Edit(edit) => {
                self.history.edit(&mut self.project, edit)?;
                self.publish_state();
            }
            Command::Undo => {
                self.history.undo(&mut self.project)?;
                self.publish_state();
            }
            Command::Redo => {
                self.history.redo(&mut self.project)?;
                self.publish_state();
            }
        }
        self.revision += 1;
        self.maintain();
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        let node = &self.project.node;
        Snapshot {
            revision: self.revision,
            tempo_bpm: self.project.tempo_bpm,
            playing: self.playing,
            position_beats: self.status.position_beats(),
            loop_range: self.loop_range,
            volume_db: node.volume_db,
            pan: node.pan,
            clips: node
                .clips
                .iter()
                .map(|(&id, clip)| ClipSummary {
                    id,
                    start: clip.start,
                    length: clip.length,
                    note_count: clip.notes.len(),
                })
                .collect(),
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
        }
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

    fn publish_state(&mut self) {
        // Only the newest state matters; drop any that were never delivered.
        self.outbox
            .retain(|msg| !matches!(msg, AudioMsg::SwapState(_)));
        let state = Box::new(RenderState::compile(&self.project));
        self.outbox.push_back(AudioMsg::SwapState(state));
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;
