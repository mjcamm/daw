//! Audio signals' lines from a render of the song in the background
//! (`docs/design/signals.md`, From audio).

#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_engine::{
    AudioClip, AudioSource, ClipId, Command, Edit, Engine, EngineConfig, Node, NodeId, NodeKind,
    ProjectFile, SampleId, SampleRef, Signal, SignalId, SignalSource, Ticks,
};
use daw_host::analysis::{self, AnalysisJob, Loudness};
use daw_host::export::Progress;
use std::path::PathBuf;

const RATE: u32 = 48_000;
/// 120 BPM: a beat is 24 000 frames, 750 control periods.
const BEAT: usize = 24_000;
const PERIODS_PER_BEAT: usize = BEAT / 32;
const DRUMS: NodeId = NodeId(50);

/// A fresh project folder holding `hit.wav`: one beat at full scale.
fn folder(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-analysis-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.join("hit.wav"), spec).unwrap();
    for _ in 0..BEAT {
        writer.write_sample(1.0f32).unwrap();
    }
    writer.finalize().unwrap();
    dir
}

/// The drums play the hit on beat 2 (from beat 1); with `published`, an
/// audio signal follows them, instantly.
fn song(published: bool) -> ProjectFile {
    let (mut engine, _processor) = Engine::new(EngineConfig::default());
    let mut edits = vec![
        Edit::SetTempo { bpm: 120.0 },
        Edit::AddNode {
            id: DRUMS,
            parent: NodeId::ROOT,
            index: 0,
            node: Node::new("Drums", 0).with_kind(NodeKind::Audio),
        },
        Edit::AddSample {
            id: SampleId(1),
            sample: SampleRef {
                name: "hit".to_owned(),
                file: "hit.wav".to_owned(),
            },
        },
        Edit::AddAudioClip {
            node: DRUMS,
            id: ClipId(1),
            clip: AudioClip::whole_file(SampleId(1), Ticks::from_beats(1), 0.5, 120.0),
        },
    ];
    if published {
        edits.push(Edit::AddSignal {
            id: SignalId(1),
            signal: Signal {
                name: "Drums audio".to_owned(),
                color: 0,
                source: SignalSource::Audio(instant()),
            },
            listeners: Vec::new(),
        });
    }
    for edit in edits {
        engine.apply(Command::Edit(edit)).unwrap();
    }
    engine.to_file()
}

fn instant() -> AudioSource {
    AudioSource {
        attack_ms: 0.0,
        release_ms: 0.0,
        ..AudioSource::new(DRUMS)
    }
}

fn job(song: ProjectFile, folder: PathBuf) -> AnalysisJob {
    AnalysisJob {
        song,
        sample_dir: Some(folder),
        sample_rate: RATE,
        threads: 2,
        plugin_host: None,
    }
}

fn analysed(test: &str) -> Loudness {
    let folder = folder(test);
    analysis::analyse(job(song(true), folder), &Progress::default()).unwrap()
}

#[test]
fn a_render_finds_the_followed_nodes_loudness_over_the_song() {
    let loudness = analysed("loudness");
    let peaks = &loudness.nodes[&DRUMS];
    // The song ends with the hit, at beat 2.
    assert!(peaks.len() >= 2 * PERIODS_PER_BEAT);
    assert_eq!(peaks[PERIODS_PER_BEAT], 0.0, "silent up to beat 1");
    assert_eq!(peaks[PERIODS_PER_BEAT + 1], 1.0, "the hit");
    assert_eq!(peaks[2 * PERIODS_PER_BEAT], 1.0, "to its end");
    assert!((loudness.step_beats() - 1.0 / PERIODS_PER_BEAT as f64).abs() < 1e-12);
}

#[test]
fn the_line_follows_the_signals_own_settings() {
    let loudness = analysed("line");
    let line = loudness.line(&instant()).unwrap();
    assert_eq!(
        line[PERIODS_PER_BEAT / 2],
        0.5,
        "the centre line in silence"
    );
    assert_eq!(line[PERIODS_PER_BEAT + 10], 1.0, "the top while it is loud");
    // A slow attack takes its time to rise.
    let slow = AudioSource {
        attack_ms: 100.0,
        ..instant()
    };
    let rising = loudness.line(&slow).unwrap();
    let early = rising[PERIODS_PER_BEAT + 10];
    assert!(early > 0.5 && early < 1.0, "{early}");
    // Floor and ceiling: a hit at 0 dB with the ceiling raised to +12 dB
    // and the floor at -12 dB sits half way up.
    let raised = AudioSource {
        floor_db: -12.0,
        ceiling_db: 12.0,
        ..instant()
    };
    let half = loudness.line(&raised).unwrap()[PERIODS_PER_BEAT + 10];
    assert!((half - 0.75).abs() < 1e-3, "{half}");
}

#[test]
fn a_song_without_audio_signals_renders_nothing() {
    let folder = folder("none");
    let loudness = analysis::analyse(job(song(false), folder), &Progress::default()).unwrap();
    assert!(loudness.nodes.is_empty());
}

#[test]
fn kept_loudness_comes_back_only_for_its_own_song() {
    let loudness = analysed("kept");
    let folder = folder("kept-files");
    let print = analysis::fingerprint(&song(true));
    analysis::save(&folder, print, &loudness).unwrap();
    let back = analysis::load(&folder, print).unwrap();
    assert_eq!(back.sample_rate, RATE);
    let (kept, original) = (&back.nodes[&DRUMS], &loudness.nodes[&DRUMS]);
    assert_eq!(kept.len(), original.len());
    for (a, b) in kept.iter().zip(original.iter()) {
        // Kept in half-dB steps.
        let db = |p: f32| 20.0 * p.max(1e-6).log10();
        assert!((db(*a) - db(*b)).abs() <= 0.25 || (*a == 0.0 && *b == 0.0));
    }
    assert!(analysis::load(&folder, print ^ 1).is_none(), "another song");
}
