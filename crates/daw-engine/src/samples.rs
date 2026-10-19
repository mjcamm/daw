//! Audio samples: decoding files on the control side, and the pool of
//! decoded audio the render state shares with the audio thread.
//!
//! The project names its samples by a path inside the project folder (see
//! `docs/design/samples.md`). The pool reads each the first time the
//! project refers to it and keeps it for the session, so undo and redo
//! never read files again. The audio thread only reads the decoded audio
//! through `Arc`s owned by render states, which are always freed here.

use daw_dsp::sampler::SampleView;
use daw_protocol::{Peaks, SampleId, SampleRef, SampleStatus};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Decoded audio: one or two channels at the file's own rate.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleData {
    pub sample_rate: u32,
    pub left: Box<[f32]>,
    /// `None` for a mono sample.
    pub right: Option<Box<[f32]>>,
}

impl SampleData {
    pub fn mono(sample_rate: u32, samples: Vec<f32>) -> Self {
        Self {
            sample_rate,
            left: samples.into_boxed_slice(),
            right: None,
        }
    }

    pub fn stereo(sample_rate: u32, left: Vec<f32>, right: Vec<f32>) -> Self {
        Self {
            sample_rate,
            left: left.into_boxed_slice(),
            right: Some(right.into_boxed_slice()),
        }
    }

    pub fn channels(&self) -> u16 {
        if self.right.is_some() { 2 } else { 1 }
    }

    pub fn frames(&self) -> usize {
        self.left.len()
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate.max(1))
    }

    /// The waveform overview, for drawing.
    pub fn peaks(&self) -> Peaks {
        let right = self.right.as_deref();
        let channels: Vec<&[f32]> = std::iter::once(&*self.left).chain(right).collect();
        Peaks::compute(&channels, self.sample_rate)
    }

    /// The audio as the sampler reads it. Real-time safe.
    pub fn view(&self) -> SampleView<'_> {
        SampleView {
            left: &self.left,
            right: self.right.as_deref().unwrap_or(&self.left),
            sample_rate: self.sample_rate as f32,
        }
    }
}

/// Reads a WAV file: 8, 16, 24 or 32-bit integer or 32-bit float, any rate.
/// Mono stays mono; from more than two channels the first two are kept.
pub fn decode_wav(path: &Path) -> Result<SampleData, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let full_scale = (1_i64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|s| s as f32 / full_scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    if interleaved.len() < channels {
        return Err("the file holds no audio".to_owned());
    }
    let frames = interleaved.len() / channels;
    let channel = |c: usize| -> Vec<f32> {
        interleaved
            .chunks_exact(channels)
            .take(frames)
            .map(|frame| frame[c])
            .collect()
    };
    Ok(if channels == 1 {
        SampleData::mono(spec.sample_rate, channel(0))
    } else {
        SampleData::stereo(spec.sample_rate, channel(0), channel(1))
    })
}

/// The audio converted to `rate` (band-limited, so nothing aliases), the
/// same length in time. Audio clips play their sample at the engine's rate,
/// converted once here, so the stretcher works at one rate. Control side
/// only: it allocates and takes a while for long files.
pub fn resampled(data: &SampleData, rate: u32) -> SampleData {
    use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
    use rubato::{Fft, FixedSync, Resampler};

    if data.sample_rate == rate || data.frames() == 0 {
        return data.clone();
    }
    let channels: Vec<Vec<f32>> = std::iter::once(&data.left)
        .chain(&data.right)
        .map(|c| c.to_vec())
        .collect();
    let frames = data.frames();
    let converted = SequentialSliceOfVecs::new(&channels, channels.len(), frames)
        .ok()
        .and_then(|input| {
            let mut resampler = Fft::<f32>::new(
                data.sample_rate as usize,
                rate as usize,
                1024,
                channels.len(),
                FixedSync::Both,
            )
            .ok()?;
            resampler.process_all(&input, frames, None).ok()
        });
    let Some(converted) = converted else {
        // Rates the converter cannot handle are not offered by devices or
        // files; fall back to stepping through the audio.
        return stepped(data, rate);
    };
    let count = channels.len();
    let interleaved = converted.take_data();
    // Exactly as long in time, to the nearest frame.
    let wanted = (frames as f64 * f64::from(rate) / f64::from(data.sample_rate)).round() as usize;
    let channel = |c: usize| -> Vec<f32> {
        let mut out: Vec<f32> = interleaved
            .chunks_exact(count)
            .map(|frame| frame[c])
            .collect();
        out.resize(wanted, 0.0);
        out
    };
    if count == 1 {
        SampleData::mono(rate, channel(0))
    } else {
        SampleData::stereo(rate, channel(0), channel(1))
    }
}

/// Linear interpolation, for rates the band-limited converter refuses.
fn stepped(data: &SampleData, rate: u32) -> SampleData {
    let step = f64::from(data.sample_rate) / f64::from(rate);
    let frames = (data.frames() as f64 / step).round() as usize;
    let read = |c: &[f32]| -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let at = i as f64 * step;
                let j = at as usize;
                let t = (at - j as f64) as f32;
                let a = c.get(j).copied().unwrap_or(0.0);
                let b = c.get(j + 1).copied().unwrap_or(a);
                a + (b - a) * t
            })
            .collect()
    };
    match &data.right {
        Some(right) => SampleData::stereo(rate, read(&data.left), read(right)),
        None => SampleData::mono(rate, read(&data.left)),
    }
}

/// Decoded samples by id, read from the project folder as needed.
#[derive(Debug, Default)]
pub struct SamplePool {
    dir: Option<PathBuf>,
    /// Each sample with the file it was read from, so a different file
    /// under the same id is read again.
    loaded: BTreeMap<SampleId, (String, Arc<SampleData>)>,
    failed: BTreeMap<SampleId, (String, String)>,
    /// Audio clips' samples converted to the engine's rate, with the file
    /// and rate they were made for.
    converted: BTreeMap<SampleId, (String, u32, Arc<SampleData>)>,
    /// Each read sample's waveform overview, made once when it is read.
    peaks: BTreeMap<SampleId, Arc<Peaks>>,
}

impl SamplePool {
    /// Where sample files are found. Samples that failed are tried again.
    pub fn set_dir(&mut self, dir: Option<PathBuf>) {
        self.dir = dir;
        self.failed.clear();
    }

    /// Puts decoded audio in the pool directly, as if read from `file`.
    pub fn insert(&mut self, id: SampleId, file: &str, data: SampleData) {
        self.failed.remove(&id);
        self.converted.remove(&id);
        self.peaks.insert(id, Arc::new(data.peaks()));
        self.loaded.insert(id, (file.to_owned(), Arc::new(data)));
    }

    /// Reads every sample in `samples` not read yet (or named by another
    /// file than before). Failures are remembered until the folder changes.
    pub fn refresh(&mut self, samples: &BTreeMap<SampleId, SampleRef>) {
        for (id, sample) in samples {
            let current = self.loaded.get(id).is_some_and(|(f, _)| *f == sample.file);
            let failed = self.failed.get(id).is_some_and(|(f, _)| *f == sample.file);
            if current || failed {
                continue;
            }
            let result = match &self.dir {
                Some(dir) => decode_wav(&dir.join(&sample.file)),
                None => Err("the project has no folder".to_owned()),
            };
            self.converted.remove(id);
            self.peaks.remove(id);
            match result {
                Ok(data) => {
                    self.peaks.insert(*id, Arc::new(data.peaks()));
                    self.loaded
                        .insert(*id, (sample.file.clone(), Arc::new(data)));
                }
                Err(reason) => {
                    self.loaded.remove(id);
                    self.failed.insert(*id, (sample.file.clone(), reason));
                }
            }
        }
    }

    /// Makes sure the sample's audio is held at `rate` too (for audio
    /// clips), converting it once if it was read at another rate.
    pub fn convert(&mut self, id: SampleId, file: &str, rate: u32) {
        if self.at_rate(id, file, rate).is_some() {
            return;
        }
        let Some(data) = self.get(id, file) else {
            return;
        };
        let data = if data.sample_rate == rate {
            Arc::clone(data)
        } else {
            Arc::new(resampled(data, rate))
        };
        self.converted.insert(id, (file.to_owned(), rate, data));
    }

    /// Converts every sample an audio clip plays to `rate`.
    pub fn convert_clips(&mut self, project: &crate::project::Project, rate: u32) {
        let mut wanted = Vec::new();
        for entry in project.nodes() {
            for clip in entry.node.audio_clips.values() {
                if let Some(sample) = project.samples.get(&clip.sample) {
                    wanted.push((clip.sample, sample.file.clone()));
                }
            }
        }
        for (id, file) in wanted {
            self.convert(id, &file, rate);
        }
    }

    /// The sample's audio at `rate`, if [`Self::convert`] made it.
    pub fn at_rate(&self, id: SampleId, file: &str, rate: u32) -> Option<&Arc<SampleData>> {
        self.converted
            .get(&id)
            .filter(|(f, r, _)| f == file && *r == rate)
            .map(|(_, _, data)| data)
    }

    /// The waveform overview of a sample named by `file`, if it was read.
    pub fn peaks(&self, id: SampleId, file: &str) -> Option<&Arc<Peaks>> {
        self.get(id, file)?;
        self.peaks.get(&id)
    }

    /// The decoded audio of a sample named by `file`, if it was read.
    pub fn get(&self, id: SampleId, file: &str) -> Option<&Arc<SampleData>> {
        self.loaded
            .get(&id)
            .filter(|(f, _)| f == file)
            .map(|(_, data)| data)
    }

    pub fn status(&self, id: SampleId, file: &str) -> SampleStatus {
        match self.get(id, file) {
            Some(data) => SampleStatus::Loaded {
                seconds: data.seconds(),
                channels: data.channels(),
                sample_rate: data.sample_rate,
            },
            None => SampleStatus::Missing {
                reason: self
                    .failed
                    .get(&id)
                    .map_or_else(|| "not read yet".to_owned(), |(_, r)| r.clone()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-samples-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_wav(path: &Path, spec: hound::WavSpec, samples: &[i32]) {
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn wav_files_decode_to_full_scale_floats() {
        let dir = temp_dir("decode");
        let mono = dir.join("mono.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        write_wav(&mono, spec, &[0, 16_384, -32_768]);
        let data = decode_wav(&mono).unwrap();
        assert_eq!(data.sample_rate, 44_100);
        assert_eq!(data.channels(), 1);
        assert_eq!(&*data.left, &[0.0, 0.5, -1.0]);

        let stereo = dir.join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            bits_per_sample: 24,
            ..spec
        };
        write_wav(&stereo, spec, &[4_194_304, -4_194_304, 0, 8_388_607]);
        let data = decode_wav(&stereo).unwrap();
        assert_eq!(data.channels(), 2);
        assert_eq!(&*data.left, &[0.5, 0.0]);
        assert_eq!(data.right.as_deref().unwrap()[0], -0.5);
        std::fs::remove_dir_all(dir).ok();
    }

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<f32> {
        let frames = (f64::from(rate) * seconds) as usize;
        (0..frames)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32 * 0.5)
            .collect()
    }

    /// Upward zero crossings per second, over the middle of the audio.
    fn frequency(samples: &[f32], rate: u32) -> f64 {
        let (from, to) = (samples.len() / 4, samples.len() * 3 / 4);
        let ups = samples[from..to]
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        ups as f64 * f64::from(rate) / (to - from) as f64
    }

    #[test]
    fn resampling_keeps_the_length_in_time_and_the_pitch() {
        let data = SampleData::stereo(44_100, sine(44_100, 1_000.0, 2.0), sine(44_100, 500.0, 2.0));
        let out = resampled(&data, 48_000);
        assert_eq!(out.sample_rate, 48_000);
        assert_eq!(out.frames(), 96_000, "two seconds at the new rate");
        assert!((frequency(&out.left, 48_000) - 1_000.0).abs() < 2.0);
        let right = out.right.as_deref().unwrap();
        assert!((frequency(right, 48_000) - 500.0).abs() < 2.0);
        // No real delay: in the middle it matches the same sine made at the
        // new rate to within a sample (the converter leaves about half a
        // sample, 0.01 ms; a sample's shift is at most 0.5 · 2π · 1000/48000).
        let ideal = sine(48_000, 1_000.0, 2.0);
        let worst = (24_000..72_000)
            .map(|i| (out.left[i] - ideal[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.065, "differs from the ideal by {worst}");
        let peak = out.left.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "level kept: {peak}");
    }

    #[test]
    fn the_pool_keeps_clip_audio_at_the_engine_rate() {
        let mut pool = SamplePool::default();
        let file = "audio/loop.wav";
        pool.insert(
            SampleId(1),
            file,
            SampleData::mono(44_100, sine(44_100, 100.0, 0.5)),
        );
        assert!(pool.at_rate(SampleId(1), file, 48_000).is_none());
        pool.convert(SampleId(1), file, 48_000);
        let converted = Arc::clone(pool.at_rate(SampleId(1), file, 48_000).unwrap());
        assert_eq!(
            (converted.sample_rate, converted.frames()),
            (48_000, 24_000)
        );
        pool.convert(SampleId(1), file, 48_000);
        assert!(
            Arc::ptr_eq(&converted, pool.at_rate(SampleId(1), file, 48_000).unwrap()),
            "converted once"
        );
        assert!(
            pool.at_rate(SampleId(1), file, 96_000).is_none(),
            "another rate"
        );

        // Already at the rate: shared, not copied.
        pool.insert(SampleId(2), file, SampleData::mono(48_000, vec![0.5; 10]));
        pool.convert(SampleId(2), file, 48_000);
        assert!(Arc::ptr_eq(
            pool.get(SampleId(2), file).unwrap(),
            pool.at_rate(SampleId(2), file, 48_000).unwrap()
        ));
    }

    #[test]
    fn resampling_to_the_same_rate_changes_nothing() {
        let data = SampleData::mono(48_000, sine(48_000, 440.0, 0.1));
        assert_eq!(resampled(&data, 48_000), data);
    }

    #[test]
    fn the_pool_reads_from_the_project_folder_and_reports_what_failed() {
        let dir = temp_dir("pool");
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        write_wav(&dir.join("audio/kick.wav"), spec, &[100, 200]);
        let mut samples = BTreeMap::new();
        let kick = SampleRef {
            name: "kick".to_owned(),
            file: "audio/kick.wav".to_owned(),
        };
        samples.insert(SampleId(1), kick.clone());
        samples.insert(
            SampleId(2),
            SampleRef {
                name: "gone".to_owned(),
                file: "audio/gone.wav".to_owned(),
            },
        );

        let mut pool = SamplePool::default();
        pool.refresh(&samples);
        assert!(
            matches!(
                pool.status(SampleId(1), &kick.file),
                SampleStatus::Missing { .. }
            ),
            "no folder yet"
        );
        pool.set_dir(Some(dir.clone()));
        pool.refresh(&samples);
        assert_eq!(pool.get(SampleId(1), &kick.file).unwrap().frames(), 2);
        assert!(matches!(
            pool.status(SampleId(1), &kick.file),
            SampleStatus::Loaded {
                channels: 1,
                sample_rate: 48_000,
                ..
            }
        ));
        assert!(matches!(
            pool.status(SampleId(2), "audio/gone.wav"),
            SampleStatus::Missing { .. }
        ));
        std::fs::remove_dir_all(dir).ok();
    }
}
