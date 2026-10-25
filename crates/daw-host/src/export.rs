//! Exporting a song to a WAV or MP3 file.
//!
//! [`export`] loads a copy of the song into an engine of its own and renders
//! it offline through the same path as live play, so the live song plays on
//! untouched and the file sounds exactly as the song plays. The file starts
//! the song's plugin latency in (it lines up with the song), stops on the
//! song's last frame, and may let the sound ring out until it is silent.
//! It is written beside the target and renamed into place only once it is
//! complete: a cancelled or failed export leaves nothing behind.

use daw_engine::{
    AudioProcessor, Command, Engine, EngineConfig, LoadError, Project, ProjectFile, Ticks,
};
use daw_plugins::client::HostCommand;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Exports render in blocks of this many frames (any size sounds the same).
const BLOCK_FRAMES: usize = 1024;
/// The longest a tail rings out after the song.
pub const MAX_TAIL_SECONDS: f64 = 10.0;
/// Quieter than this (about -96 dBFS) counts as silent.
const SILENCE: f32 = 1.6e-5;
/// A tail ends after this long silent.
const SILENT_SECONDS: f64 = 0.5;
/// Exports are stereo.
const CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavDepth {
    /// With dither.
    Int16,
    Int24,
    Float32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Bitrate {
    Kbps192,
    Kbps256,
    Kbps320,
}

impl Mp3Bitrate {
    pub fn kbps(self) -> u32 {
        match self {
            Self::Kbps192 => 192,
            Self::Kbps256 => 256,
            Self::Kbps320 => 320,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Wav(WavDepth),
    Mp3(Mp3Bitrate),
}

impl Format {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav(_) => "wav",
            Self::Mp3(_) => "mp3",
        }
    }
}

/// Whether this build can write MP3 (the `mp3` feature, which links LAME).
pub const MP3_AVAILABLE: bool = cfg!(feature = "mp3");

/// What part of the song to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// From the start to the end of the last clip.
    Song,
    /// The loop region.
    Loop,
}

/// What follows the range's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    /// The file ends with the range.
    Off,
    /// Releases and effects ring out until silent, up to
    /// [`MAX_TAIL_SECONDS`].
    UntilSilent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportSettings {
    pub format: Format,
    pub range: Range,
    pub tail: Tail,
}

/// Everything an export needs; it owns a copy of the song, so it can run on
/// another thread.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub song: ProjectFile,
    /// The project folder, where the song's samples are.
    pub sample_dir: Option<PathBuf>,
    pub sample_rate: u32,
    pub threads: usize,
    /// How to start a plugin host, if the song may have plugins.
    pub plugin_host: Option<HostCommand>,
    pub settings: ExportSettings,
    pub path: PathBuf,
}

/// How far an export has got, and a way to cancel it. Shared between the
/// thread exporting and whoever watches.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    /// 0 to 1.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32
    }

    /// Asks the export to stop; it returns [`ExportError::Cancelled`].
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn set(&self, done: u64, total: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
    }
}

/// A finished export.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exported {
    pub seconds: f64,
    /// The loudest sample, 1.0 being full scale.
    pub peak: f32,
}

#[derive(Debug)]
pub enum ExportError {
    Cancelled,
    /// The song has no clips.
    Empty,
    /// Asked for the loop, but the song has no loop region.
    NoLoop,
    Load(LoadError),
    Io(std::io::Error),
    Encode(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the export was cancelled"),
            Self::Empty => f.write_str("the song has nothing to export yet"),
            Self::NoLoop => f.write_str("the song has no loop region"),
            Self::Load(e) => write!(f, "the song could not be loaded: {e}"),
            Self::Io(e) => write!(f, "the file could not be written: {e}"),
            Self::Encode(e) => write!(f, "the audio could not be encoded: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<hound::Error> for ExportError {
    fn from(e: hound::Error) -> Self {
        match e {
            hound::Error::IoError(e) => Self::Io(e),
            e => Self::Encode(e.to_string()),
        }
    }
}

/// Where the song ends: the end of its last clip.
pub fn song_end(project: &Project) -> Ticks {
    project
        .nodes()
        .iter()
        .flat_map(|entry| {
            let node = entry.node;
            let midi = node.clips.values().map(|c| c.start + c.length);
            let audio = node.audio_clips.values().map(|c| c.start + c.length);
            midi.chain(audio)
        })
        .max()
        .unwrap_or(Ticks::ZERO)
}

/// The frames from song position `from` to `to` at `bpm`.
fn frames_between(from: Ticks, to: Ticks, bpm: f64, sample_rate: u32) -> usize {
    let seconds = (to.as_beats() - from.as_beats()) * 60.0 / bpm;
    (seconds * f64::from(sample_rate)).round().max(0.0) as usize
}

/// Renders the job's song to its file. Blocks until done; run it on its own
/// thread and watch `progress`.
pub fn export(job: ExportJob, progress: &Progress) -> Result<Exported, ExportError> {
    let loop_range = job.song.loop_range;
    let config = EngineConfig {
        sample_rate: job.sample_rate,
        channels: CHANNELS,
        threads: job.threads.max(1),
    };
    let (mut engine, mut processor) = Engine::new_offline(config);
    if let Some(command) = job.plugin_host.clone() {
        engine.set_plugin_host(command);
    }
    engine.set_sample_dir(job.sample_dir.clone());
    engine.load(job.song).map_err(ExportError::Load)?;

    let (start, end) = match job.settings.range {
        Range::Song => (Ticks::ZERO, song_end(engine.project())),
        Range::Loop => {
            let range = loop_range.ok_or(ExportError::NoLoop)?;
            (range.start, range.end)
        }
    };
    if end <= start {
        return Err(ExportError::Empty);
    }
    let frames = frames_between(start, end, engine.project().tempo_bpm, job.sample_rate);
    if progress.is_cancelled() {
        return Err(ExportError::Cancelled);
    }

    let commands = [
        Command::SetLoopEnabled(false),
        Command::Locate { position: start },
        Command::Play,
    ];
    for command in commands {
        engine
            .apply(command)
            .map_err(|e| ExportError::Encode(e.to_string()))?;
    }
    let partial = partial_path(&job.path);
    let result = File::create(&partial)
        .map_err(ExportError::from)
        .and_then(|file| {
            let sink = Sink::new(job.settings.format, job.sample_rate, file)?;
            let mut render = Render {
                engine: &mut engine,
                processor: &mut processor,
                sink,
                block: vec![0.0; BLOCK_FRAMES * usize::from(CHANNELS)],
                skipped: 0,
                peak: 0.0,
                written: 0,
            };
            render.song(frames, progress)?;
            if job.settings.tail == Tail::UntilSilent {
                render.tail(job.sample_rate, progress)?;
            }
            let (written, peak) = (render.written, render.peak);
            render.sink.finish()?;
            Ok(Exported {
                seconds: written as f64 / f64::from(job.sample_rate),
                peak,
            })
        });
    match result {
        Ok(done) => {
            std::fs::rename(&partial, &job.path)?;
            progress.set(1, 1);
            Ok(done)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// Beside the target, hidden from a listing of exports by its name.
fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".partial");
    path.with_file_name(name)
}

struct Render<'a> {
    engine: &'a mut Engine,
    processor: &'a mut AudioProcessor,
    sink: Sink,
    block: Vec<f32>,
    /// Frames skipped so far for the song's latency.
    skipped: usize,
    peak: f32,
    /// Frames written.
    written: usize,
}

impl Render<'_> {
    /// Renders one block of `frames` into `self.block`.
    fn process(&mut self, frames: usize) -> &[f32] {
        let samples = &mut self.block[..frames * usize::from(CHANNELS)];
        self.engine.maintain();
        self.processor.process(samples);
        samples
    }

    fn write(&mut self, frames: usize) -> Result<(), ExportError> {
        let channels = usize::from(CHANNELS);
        let samples = &self.block[..frames * channels];
        self.peak = samples.iter().fold(self.peak, |p, s| p.max(s.abs()));
        self.sink.write(samples)?;
        self.written += frames;
        Ok(())
    }

    /// Renders `frames` of song, after skipping the song's latency, then
    /// stops the transport.
    fn song(&mut self, frames: usize, progress: &Progress) -> Result<(), ExportError> {
        loop {
            // Known once the first block has run.
            let latency = self.processor.latency() as usize;
            let remaining = latency + frames - (self.skipped + self.written);
            if remaining == 0 {
                break;
            }
            if progress.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let n = remaining.min(BLOCK_FRAMES);
            self.process(n);
            let latency = self.processor.latency() as usize;
            let skip = latency.saturating_sub(self.skipped).min(n);
            self.skipped += skip;
            if skip < n {
                let channels = usize::from(CHANNELS);
                self.block.copy_within(skip * channels..n * channels, 0);
                self.write(n - skip)?;
            }
            progress.set(self.written as u64, frames as u64);
        }
        self.engine
            .apply(Command::Stop)
            .map_err(|e| ExportError::Encode(e.to_string()))
    }

    /// Lets the sound ring out until silent, then writes it without the
    /// silence at its end.
    fn tail(&mut self, sample_rate: u32, progress: &Progress) -> Result<(), ExportError> {
        let channels = usize::from(CHANNELS);
        let max = (MAX_TAIL_SECONDS * f64::from(sample_rate)) as usize;
        let quiet = (SILENT_SECONDS * f64::from(sample_rate)) as usize;
        let mut tail = Vec::new();
        // Frames up to the last one that is not silent.
        let mut loud = 0;
        let mut rendered = 0;
        while rendered < max && rendered - loud < quiet {
            if progress.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let n = (max - rendered).min(BLOCK_FRAMES);
            let samples = self.process(n);
            if let Some(last) = samples.iter().rposition(|s| s.abs() > SILENCE) {
                loud = rendered + last / channels + 1;
            }
            tail.extend_from_slice(samples);
            rendered += n;
        }
        for chunk in tail[..loud * channels].chunks(BLOCK_FRAMES * channels) {
            self.block[..chunk.len()].copy_from_slice(chunk);
            self.write(chunk.len() / channels)?;
        }
        Ok(())
    }
}

/// Where rendered audio goes.
enum Sink {
    Wav {
        writer: hound::WavWriter<BufWriter<File>>,
        depth: WavDepth,
        dither: Dither,
    },
    #[cfg(feature = "mp3")]
    Mp3(mp3::Mp3Sink),
}

impl Sink {
    fn new(format: Format, sample_rate: u32, file: File) -> Result<Self, ExportError> {
        match format {
            Format::Wav(depth) => {
                let (bits_per_sample, sample_format) = match depth {
                    WavDepth::Int16 => (16, hound::SampleFormat::Int),
                    WavDepth::Int24 => (24, hound::SampleFormat::Int),
                    WavDepth::Float32 => (32, hound::SampleFormat::Float),
                };
                let spec = hound::WavSpec {
                    channels: CHANNELS,
                    sample_rate,
                    bits_per_sample,
                    sample_format,
                };
                Ok(Self::Wav {
                    writer: hound::WavWriter::new(BufWriter::new(file), spec)?,
                    depth,
                    dither: Dither::default(),
                })
            }
            #[cfg(feature = "mp3")]
            Format::Mp3(bitrate) => Ok(Self::Mp3(mp3::Mp3Sink::new(bitrate, sample_rate, file)?)),
            #[cfg(not(feature = "mp3"))]
            Format::Mp3(_) => Err(ExportError::Encode(
                "this build has no MP3 encoder".to_owned(),
            )),
        }
    }

    /// Interleaved stereo.
    fn write(&mut self, samples: &[f32]) -> Result<(), ExportError> {
        match self {
            Self::Wav {
                writer,
                depth,
                dither,
            } => {
                for &sample in samples {
                    match depth {
                        WavDepth::Float32 => writer.write_sample(sample)?,
                        WavDepth::Int24 => writer.write_sample(quantize(sample, 24, 0.0))?,
                        WavDepth::Int16 => {
                            writer.write_sample(quantize(sample, 16, dither.next()) as i16)?;
                        }
                    }
                }
                Ok(())
            }
            #[cfg(feature = "mp3")]
            Self::Mp3(sink) => sink.write(samples),
        }
    }

    fn finish(self) -> Result<(), ExportError> {
        match self {
            Self::Wav { writer, .. } => {
                writer.finalize()?;
                Ok(())
            }
            #[cfg(feature = "mp3")]
            Self::Mp3(sink) => sink.finish(),
        }
    }
}

/// `sample` as a `bits`-bit integer, with `dither` steps added.
fn quantize(sample: f32, bits: u32, dither: f32) -> i32 {
    let scale = (1_i64 << (bits - 1)) as f64;
    let value = (f64::from(sample) * scale + f64::from(dither)).round();
    value.clamp(-scale, scale - 1.0) as i32
}

/// Triangular dither of up to one step either way, the same every export.
#[derive(Debug)]
struct Dither(u32);

impl Default for Dither {
    fn default() -> Self {
        Self(0x9E37_79B9)
    }
}

impl Dither {
    fn uniform(&mut self) -> f32 {
        // xorshift32
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as f32 / u32::MAX as f32
    }

    fn next(&mut self) -> f32 {
        self.uniform() - self.uniform()
    }
}

#[cfg(feature = "mp3")]
mod mp3 {
    use super::{CHANNELS, ExportError, Mp3Bitrate};
    use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, Quality};
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::num::NonZeroU32;

    /// The highest rate MP3 holds; faster songs are resampled to it.
    const MAX_RATE: u32 = 48_000;

    pub(super) struct Mp3Sink {
        encoder: mp3lame_encoder::Encoder,
        out: BufWriter<File>,
        buffer: Vec<u8>,
    }

    fn failed(e: impl std::fmt::Debug) -> ExportError {
        ExportError::Encode(format!("MP3 encoder: {e:?}"))
    }

    impl Mp3Sink {
        pub(super) fn new(
            bitrate: Mp3Bitrate,
            sample_rate: u32,
            file: File,
        ) -> Result<Self, ExportError> {
            let mut builder = Builder::new().ok_or_else(|| failed("could not start"))?;
            builder.set_num_channels(CHANNELS as u8).map_err(failed)?;
            builder.set_sample_rate(sample_rate).map_err(failed)?;
            builder
                .set_output_sample_rate(NonZeroU32::new(sample_rate.min(MAX_RATE)))
                .map_err(failed)?;
            builder
                .set_brate(match bitrate {
                    Mp3Bitrate::Kbps192 => Bitrate::Kbps192,
                    Mp3Bitrate::Kbps256 => Bitrate::Kbps256,
                    Mp3Bitrate::Kbps320 => Bitrate::Kbps320,
                })
                .map_err(failed)?;
            builder.set_quality(Quality::Best).map_err(failed)?;
            builder.set_to_write_vbr_tag(false).map_err(failed)?;
            Ok(Self {
                encoder: builder.build().map_err(failed)?,
                out: BufWriter::new(file),
                buffer: Vec::new(),
            })
        }

        pub(super) fn write(&mut self, samples: &[f32]) -> Result<(), ExportError> {
            let frames = samples.len() / usize::from(CHANNELS);
            self.buffer.clear();
            self.buffer
                .reserve(mp3lame_encoder::max_required_buffer_size(frames));
            self.encoder
                .encode_to_vec(InterleavedPcm(samples), &mut self.buffer)
                .map_err(failed)?;
            self.out.write_all(&self.buffer)?;
            Ok(())
        }

        pub(super) fn finish(mut self) -> Result<(), ExportError> {
            self.buffer.clear();
            self.buffer.reserve(7200);
            self.encoder
                .flush_to_vec::<FlushNoGap>(&mut self.buffer)
                .map_err(failed)?;
            self.out.write_all(&self.buffer)?;
            self.out.flush()?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantizing_rounds_and_clamps() {
        assert_eq!(quantize(0.0, 16, 0.0), 0);
        assert_eq!(quantize(0.5, 16, 0.0), 16_384);
        assert_eq!(quantize(1.0, 16, 0.0), 32_767, "full scale clamps");
        assert_eq!(quantize(-1.0, 16, 0.0), -32_768);
        assert_eq!(quantize(-2.0, 24, 0.0), -8_388_608);
        assert_eq!(quantize(0.0, 16, 0.9), 1, "dither moves by steps");
    }

    #[test]
    fn dither_is_triangular_within_a_step() {
        let mut dither = Dither::default();
        let values: Vec<f32> = (0..10_000).map(|_| dither.next()).collect();
        assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean.abs() < 0.02, "centred: {mean}");
        let near = values.iter().filter(|v| v.abs() < 0.5).count();
        assert!(near > 7_000, "most near zero, as a triangle: {near}");
    }

    #[test]
    fn frames_follow_the_tempo() {
        assert_eq!(
            frames_between(Ticks::ZERO, Ticks::from_bars(1), 120.0, 48_000),
            96_000
        );
        assert_eq!(
            frames_between(Ticks::from_beats(1), Ticks::from_beats(2), 90.0, 44_100),
            29_400
        );
    }

    #[test]
    fn a_partial_file_sits_beside_its_target() {
        assert_eq!(
            partial_path(Path::new("out/song.wav")),
            Path::new("out/song.wav.partial")
        );
    }
}
