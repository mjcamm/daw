//! Exporting a song to a file: the same sound as playing it, cut to the
//! song (or the loop), in the format asked for.

#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_engine::{
    ClipId, Command, Edit, Engine, EngineConfig, FIRST_NODE, LoopRange, MidiClip, MidiNote,
    ProjectFile, Ticks,
};
use daw_host::export::{
    self, ExportError, ExportJob, ExportSettings, Format, Mp3Bitrate, Progress, Range, Tail,
    WavDepth,
};
use daw_testkit::render;
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;
/// 120 BPM: a beat is 24 000 frames.
const BEAT: usize = 24_000;

/// A song of two bars at 120 BPM: a held chord through the first bar and
/// a note on the last beat, with a loop over beats 2 to 4.
fn song() -> ProjectFile {
    let (mut engine, _processor) = Engine::new(EngineConfig::default());
    let note = |start, length, pitch| MidiNote {
        start,
        length,
        pitch,
        velocity: 100,
    };
    let notes = vec![
        note(Ticks::ZERO, Ticks::from_bars(1), 60),
        note(Ticks::ZERO, Ticks::from_bars(1), 64),
        note(Ticks::from_beats(7), Ticks::from_beats(1), 67),
    ];
    for command in [
        Command::Edit(Edit::SetTempo { bpm: 120.0 }),
        Command::Edit(Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_bars(2),
                notes,
                looping: None,
            },
        }),
        Command::SetLoop(Some(LoopRange {
            start: Ticks::from_beats(2),
            end: Ticks::from_beats(4),
        })),
    ] {
        engine.apply(command).unwrap();
    }
    engine.to_file()
}

/// The song played from `from` for `frames` frames, as the live path
/// renders it.
fn played(from: Ticks, frames: usize) -> Vec<f32> {
    let config = EngineConfig {
        sample_rate: RATE,
        channels: 2,
        threads: 1,
    };
    let (mut engine, mut processor) = Engine::new(config);
    engine.load(song()).unwrap();
    engine.apply(Command::SetLoopEnabled(false)).unwrap();
    engine.apply(Command::Locate { position: from }).unwrap();
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, frames, &[512]).samples
}

fn job(path: &Path, format: Format, range: Range, tail: Tail) -> ExportJob {
    ExportJob {
        song: song(),
        sample_dir: None,
        sample_rate: RATE,
        threads: 2,
        plugin_host: None,
        settings: ExportSettings {
            format,
            range,
            tail,
        },
        path: path.to_owned(),
    }
}

fn out_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-export-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_wav(path: &Path) -> (hound::WavSpec, Vec<f32>) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let scale = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.unwrap() as f32 / scale)
                .collect()
        }
    };
    (spec, samples)
}

#[test]
fn a_float_wav_is_the_song_exactly_as_played() {
    let dir = out_dir("float");
    let path = dir.join("song.wav");
    let done = export::export(
        job(
            &path,
            Format::Wav(WavDepth::Float32),
            Range::Song,
            Tail::Off,
        ),
        &Progress::default(),
    )
    .unwrap();

    let (spec, samples) = read_wav(&path);
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, RATE);
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(samples.len(), 8 * BEAT * 2, "two bars, no tail");
    assert!(
        samples == played(Ticks::ZERO, 8 * BEAT),
        "same as playing it"
    );
    assert!(done.peak > 0.01, "it is not silent");
    assert!((done.seconds - 4.0).abs() < 1e-9);
}

#[test]
fn a_tail_rings_out_until_silent() {
    let dir = out_dir("tail");
    let path = dir.join("song.wav");
    export::export(
        job(
            &path,
            Format::Wav(WavDepth::Float32),
            Range::Song,
            Tail::UntilSilent,
        ),
        &Progress::default(),
    )
    .unwrap();

    let (_, samples) = read_wav(&path);
    let frames = samples.len() / 2;
    assert!(frames > 8 * BEAT, "longer than the song: the release rings");
    assert!(
        frames <= 8 * BEAT + export::MAX_TAIL_SECONDS as usize * RATE as usize,
        "never longer than the longest tail"
    );
    assert!(
        samples[..8 * BEAT * 2] == played(Ticks::ZERO, 8 * BEAT)[..],
        "the song part is unchanged"
    );
    let last = samples[samples.len() - 2..]
        .iter()
        .fold(0.0_f32, |p, s| p.max(s.abs()));
    assert!(last > 0.0, "trailing silence is cut");
}

#[test]
fn the_loop_range_exports_only_the_loop() {
    let dir = out_dir("loop");
    let path = dir.join("loop.wav");
    export::export(
        job(
            &path,
            Format::Wav(WavDepth::Float32),
            Range::Loop,
            Tail::Off,
        ),
        &Progress::default(),
    )
    .unwrap();

    let (_, samples) = read_wav(&path);
    assert_eq!(samples.len(), 2 * BEAT * 2, "beats 2 to 4");
    assert!(samples == played(Ticks::from_beats(2), 2 * BEAT));
}

#[test]
fn integer_wavs_are_the_song_to_within_their_resolution() {
    for (depth, bits) in [(WavDepth::Int16, 16), (WavDepth::Int24, 24)] {
        let dir = out_dir(&format!("int{bits}"));
        let path = dir.join("song.wav");
        export::export(
            job(&path, Format::Wav(depth), Range::Song, Tail::Off),
            &Progress::default(),
        )
        .unwrap();
        let (spec, samples) = read_wav(&path);
        assert_eq!(spec.bits_per_sample, bits);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let expected = played(Ticks::ZERO, 8 * BEAT);
        assert_eq!(samples.len(), expected.len());
        // Dither adds up to one step either way, rounding half a step.
        let step = 1.0 / (1_i64 << (bits - 1)) as f32;
        let worst = samples
            .iter()
            .zip(&expected)
            .fold(0.0_f32, |w, (a, b)| w.max((a - b).abs()));
        assert!(worst <= 2.0 * step, "{bits}-bit: off by {worst}");
    }
}

#[test]
fn sixteen_bit_dither_is_the_same_every_time() {
    let dir = out_dir("dither");
    let paths = [dir.join("a.wav"), dir.join("b.wav")];
    for path in &paths {
        export::export(
            job(path, Format::Wav(WavDepth::Int16), Range::Song, Tail::Off),
            &Progress::default(),
        )
        .unwrap();
    }
    assert!(std::fs::read(&paths[0]).unwrap() == std::fs::read(&paths[1]).unwrap());
}

#[cfg(feature = "mp3")]
#[test]
fn an_mp3_is_written_at_its_bitrate() {
    let dir = out_dir("mp3");
    let path = dir.join("song.mp3");
    export::export(
        job(
            &path,
            Format::Mp3(Mp3Bitrate::Kbps320),
            Range::Song,
            Tail::Off,
        ),
        &Progress::default(),
    )
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    // An MPEG-1 Layer III frame header: sync bits, then the layer.
    let first = bytes
        .windows(2)
        .position(|w| w[0] == 0xFF && w[1] & 0xFE == 0xFA)
        .unwrap();
    assert!(first < 1024, "frames start near the top");
    // 4 s at 320 kbit/s is 160 000 bytes.
    let expected = 4.0 * 320_000.0 / 8.0;
    let size = bytes.len() as f64;
    assert!(
        (size - expected).abs() < expected * 0.1,
        "{size} bytes, about {expected} expected"
    );
}

#[test]
fn a_cancelled_export_leaves_no_file() {
    let dir = out_dir("cancel");
    let path = dir.join("song.wav");
    let progress = Progress::default();
    progress.cancel();
    let result = export::export(
        job(
            &path,
            Format::Wav(WavDepth::Float32),
            Range::Song,
            Tail::Off,
        ),
        &progress,
    );
    assert!(matches!(result, Err(ExportError::Cancelled)));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing left");
}

#[test]
fn an_existing_file_is_kept_until_the_export_succeeds() {
    let dir = out_dir("replace");
    let path = dir.join("song.wav");
    std::fs::write(&path, b"old").unwrap();
    let progress = Progress::default();
    progress.cancel();
    let _ = export::export(
        job(
            &path,
            Format::Wav(WavDepth::Float32),
            Range::Song,
            Tail::Off,
        ),
        &progress,
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"old");

    export::export(
        job(
            &path,
            Format::Wav(WavDepth::Float32),
            Range::Song,
            Tail::Off,
        ),
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(read_wav(&path).1.len(), 8 * BEAT * 2, "replaced");
}

#[test]
fn progress_reaches_the_end() {
    let dir = out_dir("progress");
    let progress = Progress::default();
    assert_eq!(progress.fraction(), 0.0);
    export::export(
        job(
            &dir.join("song.wav"),
            Format::Wav(WavDepth::Float32),
            Range::Song,
            Tail::Off,
        ),
        &progress,
    )
    .unwrap();
    assert_eq!(progress.fraction(), 1.0);
}

#[test]
fn an_empty_song_or_missing_loop_is_refused() {
    let dir = out_dir("empty");
    let (mut engine, _processor) = Engine::new(EngineConfig::default());
    let mut empty = job(
        &dir.join("song.wav"),
        Format::Wav(WavDepth::Float32),
        Range::Song,
        Tail::Off,
    );
    empty.song = engine.to_file();
    assert!(matches!(
        export::export(empty, &Progress::default()),
        Err(ExportError::Empty)
    ));

    let mut no_loop = job(
        &dir.join("loop.wav"),
        Format::Wav(WavDepth::Float32),
        Range::Loop,
        Tail::Off,
    );
    no_loop.song.loop_range = None;
    assert!(matches!(
        export::export(no_loop, &Progress::default()),
        Err(ExportError::NoLoop)
    ));
}

#[test]
fn the_song_ends_at_its_last_clip() {
    let file = song();
    let (project, _) = file.into_project().unwrap();
    assert_eq!(export::song_end(&project), Ticks::from_bars(2));
}

#[test]
fn exporting_runs_on_another_thread() {
    let dir = out_dir("thread");
    let job = job(
        &dir.join("song.wav"),
        Format::Wav(WavDepth::Float32),
        Range::Song,
        Tail::Off,
    );
    std::thread::spawn(move || export::export(job, &Progress::default()))
        .join()
        .unwrap()
        .unwrap();
}

#[test]
fn formats_name_their_files() {
    assert_eq!(Format::Wav(WavDepth::Int24).extension(), "wav");
    assert_eq!(Format::Mp3(Mp3Bitrate::Kbps320).extension(), "mp3");
}
