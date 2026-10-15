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

pub use audio::{AudioProcessor, RenderState};
pub use daw_protocol::{Command, CommandError, Edit, Snapshot};
pub use project::{DEFAULT_TEMPO_BPM, MAX_TEMPO_BPM, MIN_TEMPO_BPM, Project};

use audio::{AudioMsg, SharedStatus};
use history::History;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Capacity of each control/audio queue. Generous, because a full queue only
/// delays delivery until the next [`Engine::maintain`].
const QUEUE_CAPACITY: usize = 1024;

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
    /// If `config.channels` is zero.
    pub fn new(config: EngineConfig) -> (Self, AudioProcessor) {
        assert!(config.channels > 0, "engine needs at least one channel");
        let project = Project::default();
        let (to_audio, from_control) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        // Every message carries at most one state, so the audio side can never
        // hold more retired states than this while the control side keeps up.
        let (garbage_tx, garbage) = rtrb::RingBuffer::new(QUEUE_CAPACITY);
        let status = Arc::new(SharedStatus::default());
        let processor = AudioProcessor::new(
            usize::from(config.channels),
            Box::new(render_state(&project)),
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
            Command::Play => self.set_playing(true),
            Command::Stop => self.set_playing(false),
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
        Snapshot {
            revision: self.revision,
            tempo_bpm: self.project.tempo_bpm,
            playing: self.playing,
            position_frames: self.status.position_frames.load(Ordering::Relaxed),
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

    fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.outbox.push_back(AudioMsg::SetPlaying(playing));
    }

    fn publish_state(&mut self) {
        // Only the newest state matters; drop any that were never delivered.
        self.outbox
            .retain(|msg| !matches!(msg, AudioMsg::SwapState(_)));
        let state = Box::new(render_state(&self.project));
        self.outbox.push_back(AudioMsg::SwapState(state));
    }
}

fn render_state(project: &Project) -> RenderState {
    RenderState {
        tempo_bpm: project.tempo_bpm,
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;
