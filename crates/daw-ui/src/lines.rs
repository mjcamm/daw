//! Audio signals' lines, worked out in the background
//! (`docs/design/signals.md`, From audio).
//!
//! An audio signal's line is only known from a render of the song, so once
//! edits settle the song is rendered on a thread of its own
//! (`daw_host::analysis`), and the followed nodes' loudness comes back to
//! draw from. A newer edit cancels a render still running. The loudness is
//! kept beside the song when it is saved, so a reopened song draws its
//! lines at once.

use daw_engine::{Engine, ProjectFile, SignalSource};
use daw_host::analysis::{self, AnalysisJob, Loudness};
use daw_host::export::{ExportError, Progress};
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the song must stay unchanged before it is rendered.
const SETTLE: Duration = Duration::from_millis(400);

/// A render running for the song at a content revision.
struct Running {
    revision: u64,
    progress: Arc<Progress>,
    thread: JoinHandle<Result<Loudness, ExportError>>,
}

pub struct Lines {
    loudness: Arc<Loudness>,
    /// The content revision `loudness` was found for.
    analysed: Option<u64>,
    running: Option<Running>,
    /// The content revision last seen, and when it was first seen.
    seen: (u64, Instant),
    /// Bumped whenever `loudness` changes, so views redraw.
    version: u64,
}

impl Lines {
    /// For a song just opened or made (`song`, at content revision
    /// `revision`), with the loudness kept beside it if it is this song's.
    pub fn new(song: &ProjectFile, revision: u64, folder: &Path) -> Self {
        let kept = analysis::load(folder, analysis::fingerprint(song));
        Self {
            analysed: kept.is_some().then_some(revision),
            loudness: Arc::new(kept.unwrap_or_default()),
            running: None,
            seen: (revision, Instant::now()),
            version: 0,
        }
    }

    /// Each followed node's loudness, as last found.
    pub fn loudness(&self) -> &Arc<Loudness> {
        &self.loudness
    }

    /// Changes whenever the loudness does.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Takes a finished render's loudness, and starts a render once the
    /// song has settled after a change, if it has audio signals.
    pub fn tick(&mut self, engine: &mut Engine, folder: &Path) {
        if self
            .running
            .as_ref()
            .is_some_and(|r| r.thread.is_finished())
            && let Some(done) = self.running.take()
            && let Ok(Ok(loudness)) = done.thread.join()
        {
            self.loudness = Arc::new(loudness);
            self.analysed = Some(done.revision);
            self.version += 1;
        }
        let revision = engine.content_revision();
        if revision != self.seen.0 {
            self.seen = (revision, Instant::now());
        }
        let followed = engine
            .project()
            .signals
            .values()
            .any(|s| matches!(s.source, SignalSource::Audio(_)));
        let wanted = followed
            && self.analysed != Some(revision)
            && self.running.as_ref().is_none_or(|r| r.revision != revision)
            && self.seen.1.elapsed() >= SETTLE;
        if wanted {
            self.start(engine, folder, revision);
        }
    }

    fn start(&mut self, engine: &mut Engine, folder: &Path, revision: u64) {
        if let Some(old) = self.running.take() {
            // It stops at its next block; its thread is left to end.
            old.progress.cancel();
        }
        let config = engine.config();
        let job = AnalysisJob {
            song: engine.to_file(),
            sample_dir: Some(folder.to_owned()),
            sample_rate: config.sample_rate,
            threads: config.threads,
            // Tests run no plugins (their binary can't be a plugin host).
            plugin_host: daw_host::plugins::host_command().filter(|_| !cfg!(test)),
        };
        let progress = Arc::new(Progress::default());
        let watched = Arc::clone(&progress);
        let thread = std::thread::Builder::new()
            .name("signal lines".into())
            .spawn(move || analysis::analyse(job, &watched));
        match thread {
            Ok(thread) => {
                self.running = Some(Running {
                    revision,
                    progress,
                    thread,
                });
            }
            Err(e) => eprintln!("could not start working out signal lines: {e}"),
        }
    }

    /// Keeps the loudness beside `song`, just saved at content revision
    /// `revision`, if it is the song's.
    pub fn save(&self, song: &ProjectFile, revision: u64, folder: &Path) {
        if self.analysed != Some(revision) {
            return;
        }
        if let Err(e) = analysis::save(folder, analysis::fingerprint(song), &self.loudness) {
            eprintln!("could not keep the signal lines: {e}");
        }
    }

    /// Waits for a render to start and finish, for tests.
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn finish(&mut self, engine: &mut Engine, folder: &Path) {
        self.seen.1 = Instant::now().checked_sub(SETTLE).unwrap_or(self.seen.1);
        self.tick(engine, folder);
        if let Some(running) = &self.running {
            while !running.thread.is_finished() {
                std::thread::yield_now();
            }
        }
        self.tick(engine, folder);
    }
}
