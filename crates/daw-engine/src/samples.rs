//! Audio samples: decoding files on the control side, and the pool of
//! decoded audio the render state shares with the audio thread.
//!
//! The project names its samples by a path inside the project folder (see
//! `docs/design/samples.md`). The pool reads each the first time the
//! project refers to it and keeps it for the session, so undo and redo
//! never read files again. The audio thread only reads the decoded audio
//! through `Arc`s owned by render states, which are always freed here.

use daw_dsp::sampler::SampleView;
use daw_protocol::{SampleId, SampleRef, SampleStatus};
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

/// Decoded samples by id, read from the project folder as needed.
#[derive(Debug, Default)]
pub struct SamplePool {
    dir: Option<PathBuf>,
    /// Each sample with the file it was read from, so a different file
    /// under the same id is read again.
    loaded: BTreeMap<SampleId, (String, Arc<SampleData>)>,
    failed: BTreeMap<SampleId, (String, String)>,
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
            match result {
                Ok(data) => {
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
