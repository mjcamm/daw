//! Audio signals' lines (`docs/design/signals.md`, From audio).
//!
//! An audio signal follows a node's sound as the song plays, so its line is
//! only known from a render. [`analyse`] renders a copy of the song in an
//! engine of its own, as an export does, and keeps each followed node's
//! [`Loudness`]: its peak (before its effects) over every control period.
//! [`Loudness::line`] then gives the line an audio signal draws for its
//! follower settings, so changing them never needs another render. The
//! loudness is kept beside the song ([`save`], [`load`]) so a reopened song
//! draws its lines at once.

use crate::export::{ExportError, Progress, song_end};
use daw_dsp::follower::Follower;
use daw_engine::{AudioSource, CONTROL_FRAMES, Command, Engine, EngineConfig, NodeId, ProjectFile};
use daw_plugins::client::HostCommand;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Renders in blocks of this many frames: one chunk each, on the control
/// grid, so every chunk's control points come back.
const BLOCK_FRAMES: usize = 1024;
/// Where the loudness is kept, in the project folder.
const FILE: &str = "signal-loudness.bin";
const MAGIC: &[u8; 4] = b"DAWL";
const FILE_VERSION: u32 = 2;
/// Stored as 0 for silence, else half-dB steps up from this.
const STORED_FLOOR_DB: f32 = -103.0;

/// Everything an analysis needs; it owns a copy of the song, so it can run
/// on another thread.
#[derive(Debug, Clone)]
pub struct AnalysisJob {
    pub song: ProjectFile,
    /// The project folder, where the song's samples are.
    pub sample_dir: Option<PathBuf>,
    pub sample_rate: u32,
    pub threads: usize,
    /// How to start a plugin host, if the song may have plugins.
    pub plugin_host: Option<HostCommand>,
}

/// Each followed node's sound before its effects over the song: its peak
/// over every control period from the song's start, `peaks[i]` over the
/// period ending at control point `i`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loudness {
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    pub nodes: BTreeMap<NodeId, Arc<[f32]>>,
}

impl Loudness {
    /// How long one control period is, in beats.
    pub fn step_beats(&self) -> f64 {
        CONTROL_FRAMES as f64 / f64::from(self.sample_rate.max(1)) * self.tempo_bpm / 60.0
    }

    /// The line an audio signal draws: its value at each control point
    /// (`0.5` is the centre line), as its follower finds it there. `None`
    /// until its node has been rendered.
    pub fn line(&self, source: &AudioSource) -> Option<Arc<[f32]>> {
        let peaks = self.nodes.get(&source.node)?;
        let rate = self.sample_rate as f32;
        let mut follower = Follower::new(
            source.attack_ms as f32,
            source.release_ms as f32,
            source.floor_db as f32,
            source.ceiling_db as f32,
            rate,
        );
        Some(
            peaks
                .iter()
                .map(|&peak| {
                    follower.process_held(peak, CONTROL_FRAMES as u32);
                    0.5 + 0.5 * follower.level()
                })
                .collect(),
        )
    }
}

/// Renders the job's song and keeps its followed nodes' loudness. Blocks
/// until done; run it on its own thread and watch `progress`. A song with
/// no audio signals renders nothing.
pub fn analyse(job: AnalysisJob, progress: &Progress) -> Result<Loudness, ExportError> {
    let config = EngineConfig {
        sample_rate: job.sample_rate,
        channels: 2,
        threads: job.threads.max(1),
    };
    let (mut engine, mut processor) = Engine::new_offline(config);
    if let Some(command) = job.plugin_host {
        engine.set_plugin_host(command);
    }
    engine.set_sample_dir(job.sample_dir);
    engine.load(job.song).map_err(ExportError::Load)?;
    let project = engine.project();
    let mut loudness = Loudness {
        sample_rate: job.sample_rate,
        tempo_bpm: project.tempo_bpm,
        nodes: BTreeMap::new(),
    };
    let followed = project
        .signals
        .values()
        .any(|s| matches!(s.source, daw_engine::SignalSource::Audio(_)));
    if !followed {
        return Ok(loudness);
    }
    let seconds = song_end(project).as_beats() * 60.0 / project.tempo_bpm;
    let frames = (seconds * f64::from(job.sample_rate)).ceil() as usize;
    for command in [
        Command::SetLoopEnabled(false),
        Command::Locate {
            position: daw_engine::Ticks::ZERO,
        },
        Command::Play,
    ] {
        engine
            .apply(command)
            .map_err(|e| ExportError::Encode(e.to_string()))?;
    }
    let mut nodes: BTreeMap<NodeId, Vec<f32>> = BTreeMap::new();
    let mut block = vec![0.0; BLOCK_FRAMES * 2];
    let mut done = 0;
    while done < frames {
        if progress.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        engine.maintain();
        processor.process(&mut block);
        for (node, peaks) in processor.followed_peaks() {
            nodes.entry(node).or_default().extend(peaks);
        }
        done += BLOCK_FRAMES;
        progress.set(done as u64, frames as u64);
    }
    loudness.nodes = nodes.into_iter().map(|(n, p)| (n, p.into())).collect();
    Ok(loudness)
}

/// A fingerprint of a song, to tell whether kept loudness belongs to it
/// (FNV-1a, stable across builds).
pub fn fingerprint(song: &ProjectFile) -> u64 {
    song.to_json()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// Keeps `loudness` beside the song whose fingerprint is `song`.
pub fn save(folder: &Path, song: u64, loudness: &Loudness) -> std::io::Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&song.to_le_bytes());
    out.extend_from_slice(&loudness.sample_rate.to_le_bytes());
    out.extend_from_slice(&loudness.tempo_bpm.to_le_bytes());
    out.extend_from_slice(&(loudness.nodes.len() as u32).to_le_bytes());
    for (node, peaks) in &loudness.nodes {
        out.extend_from_slice(&node.0.to_le_bytes());
        out.extend_from_slice(&(peaks.len() as u32).to_le_bytes());
        out.extend(peaks.iter().map(|&p| quantize(p)));
    }
    let path = folder.join(FILE);
    let partial = folder.join(format!("{FILE}.partial"));
    std::fs::write(&partial, out)?;
    std::fs::rename(partial, path)
}

/// The loudness kept beside the song, if it belongs to the song whose
/// fingerprint is `song`.
pub fn load(folder: &Path, song: u64) -> Option<Loudness> {
    let bytes = std::fs::read(folder.join(FILE)).ok()?;
    let mut reader = Reader(&bytes);
    if reader.take(4)? != MAGIC || reader.u32()? != FILE_VERSION || reader.u64()? != song {
        return None;
    }
    let sample_rate = reader.u32()?;
    let tempo_bpm = f64::from_le_bytes(reader.take(8)?.try_into().ok()?);
    let count = reader.u32()?;
    let mut nodes = BTreeMap::new();
    for _ in 0..count {
        let node = NodeId(reader.u64()?);
        let len = reader.u32()? as usize;
        let peaks: Arc<[f32]> = reader.take(len)?.iter().map(|&q| unquantize(q)).collect();
        nodes.insert(node, peaks);
    }
    Some(Loudness {
        sample_rate,
        tempo_bpm,
        nodes,
    })
}

/// A peak in half-dB steps: 0 is silence, 1 is -103 dB, 255 is +24 dB.
fn quantize(peak: f32) -> u8 {
    if peak.is_nan() || peak <= 0.0 {
        return 0;
    }
    let db = 20.0 * peak.log10();
    ((db - STORED_FLOOR_DB) * 2.0 + 1.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn unquantize(q: u8) -> f32 {
    if q == 0 {
        return 0.0;
    }
    let db = STORED_FLOOR_DB + f32::from(q - 1) / 2.0;
    10f32.powf(db / 20.0)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}
