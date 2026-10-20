//! How long frames take to build on a full-size song, headless: render,
//! layout and paint of the real views, without the GPU or font shaping.
//!
//! Two kinds of frame per scene:
//! - **steady**: what happens every frame while the song plays (the view
//!   asks for the next frame); after step 3 of the speed work, only what
//!   moves is rebuilt.
//! - **full**: everything rebuilt, as after an edit.
//!
//! Timings only mean something in release builds (`./scripts/full-test.sh`
//! runs them that way), so debug runs skip them. Each test prints its
//! numbers; budgets fail the run when a change makes frames slower.

use super::Harness;
use crate::project::Page;
use daw_engine::{
    AudioClip, BlendMode, Block, BlockId, ClipId, Command, DeviceId, DeviceRef, Edit, Layer,
    LayerId, MidiClip, MidiNote, Node, NodeId, NodeKind, NotesSource, ParamId, SampleId, SampleRef,
    Shape, Signal, SignalId, SignalSource, Ticks,
};
use std::time::{Duration, Instant};

/// Groups, nodes per group, bars, notes per bar.
const GROUPS: u64 = 4;
const PER_GROUP: u64 = 6;
const BARS: i64 = 64;
const NOTES_PER_BAR: i64 = 8;
/// Frames timed per measurement, after a few to warm up.
const FRAMES: usize = 40;

/// Audio stems in the full-size song: nodes, and seconds of audio each.
const STEMS: u64 = 6;
const STEM_SECONDS: f64 = 15.;

/// Writes a mono 16-bit WAV of a decaying tone every half second, the
/// stems' audio.
fn write_tone(path: &std::path::Path, seconds: f64) {
    let rate = 48_000u32;
    let frames = (seconds * f64::from(rate)) as u32;
    let mut bytes = Vec::with_capacity(44 + 2 * frames as usize);
    let data_len = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let since_hit = t % 0.5;
        let v = (t * 220. * std::f64::consts::TAU).sin() * (-since_hit * 8.).exp() * 0.8;
        bytes.extend_from_slice(&((v * f64::from(i16::MAX)) as i16).to_le_bytes());
    }
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("audio folder");
    std::fs::write(path, bytes).expect("written");
}

/// A full-size song: 4 groups of 6 synth nodes, each with a 4-bar clip
/// every 4 bars across 64 bars (8 notes a bar, about 12,000 notes); a
/// group of 6 audio stems, each with a 15-second clip every 8 bars; the
/// first synth's parameters layered with ramps; a notes signal and a drawn
/// signal with a block every 2 bars.
fn heavy_song(h: &mut Harness) -> NodeId {
    write_tone(&h.folder().join("audio/tone.wav"), STEM_SECONDS);
    let mut next = 1000u64;
    let mut id = || {
        next += 1;
        next
    };
    let mut first_synth = None;
    h.view.update(h.cx, |view, _| {
        let session = &mut view.session;
        for g in 0..GROUPS {
            let group = NodeId(id());
            session.apply_or_panic(Command::Edit(Edit::AddNode {
                id: group,
                parent: NodeId::ROOT,
                index: g as usize + 1,
                node: Node::new(&format!("Group {g}"), (g % 9) as u8),
            }));
            for n in 0..PER_GROUP {
                let node = NodeId(id());
                let device = DeviceId(id());
                first_synth.get_or_insert((node, device));
                session.apply_or_panic(Command::Edit(Edit::AddNode {
                    id: node,
                    parent: group,
                    index: n as usize,
                    node: Node::new(&format!("Synth {g}.{n}"), ((g + n) % 9) as u8)
                        .with_instrument(device),
                }));
                for bar in (0..BARS).step_by(4) {
                    let notes = (0..4 * NOTES_PER_BAR)
                        .map(|i| MidiNote {
                            start: Ticks(Ticks::from_beats(4).0 * i / NOTES_PER_BAR),
                            length: Ticks(Ticks::from_beats(4).0 / NOTES_PER_BAR / 2),
                            pitch: 48 + ((i * 7 + n as i64) % 24) as u8,
                            velocity: 100,
                        })
                        .collect();
                    session.apply_or_panic(Command::Edit(Edit::AddClip {
                        node,
                        id: ClipId(id()),
                        clip: MidiClip {
                            start: Ticks::from_bars(bar),
                            length: Ticks::from_bars(4),
                            notes,
                        },
                    }));
                }
            }
        }
        // Audio stems.
        let sample = SampleId(1);
        session.apply_or_panic(Command::Edit(Edit::AddSample {
            id: sample,
            sample: SampleRef {
                name: "tone".into(),
                file: "audio/tone.wav".into(),
            },
        }));
        let stems = NodeId(id());
        session.apply_or_panic(Command::Edit(Edit::AddNode {
            id: stems,
            parent: NodeId::ROOT,
            index: GROUPS as usize + 1,
            node: Node::new("Stems", 5).with_kind(NodeKind::Audio),
        }));
        for n in 0..STEMS {
            let node = NodeId(id());
            session.apply_or_panic(Command::Edit(Edit::AddNode {
                id: node,
                parent: stems,
                index: n as usize,
                node: Node::new(&format!("Stem {n}"), (n % 9) as u8).with_kind(NodeKind::Audio),
            }));
            for bar in (0..BARS).step_by(8) {
                session.apply_or_panic(Command::Edit(Edit::AddAudioClip {
                    node,
                    id: ClipId(id()),
                    clip: AudioClip::whole_file(sample, Ticks::from_bars(bar), STEM_SECONDS, 124.),
                }));
            }
        }
        // Signals: one from a synth's notes, one drawn.
        let (node, device) = first_synth.expect("a synth");
        session.apply_or_panic(Command::Edit(Edit::AddSignal {
            id: SignalId(id()),
            signal: Signal {
                name: "Notes".into(),
                color: 3,
                source: SignalSource::Notes(NotesSource::new(node)),
            },
            listeners: Vec::new(),
        }));
        let drawn = SignalId(id());
        session.apply_or_panic(Command::Edit(Edit::AddSignal {
            id: drawn,
            signal: Signal {
                name: "Drawn".into(),
                color: 6,
                source: SignalSource::Drawn(Vec::new()),
            },
            listeners: Vec::new(),
        }));
        for bar in (0..BARS).step_by(2) {
            session.apply_or_panic(Command::Edit(Edit::AddSignalBlock {
                signal: drawn,
                id: BlockId(id()),
                block: Block::new(
                    Ticks::from_bars(bar),
                    Ticks::from_bars(1),
                    Shape::ramp(0.2, 0.8),
                ),
            }));
        }
        // Automation to draw on the Parameters page.
        for index in 0..4 {
            let param = ParamId {
                node,
                device: DeviceRef::Instrument(device),
                index,
            };
            for l in 0..3 {
                let layer = LayerId(id());
                session.apply_or_panic(Command::Edit(Edit::AddLayer {
                    param,
                    id: layer,
                    index: l,
                    layer: Layer::named(&format!("Layer {l}"), BlendMode::Add),
                }));
                for bar in (0..BARS).step_by(2) {
                    session.apply_or_panic(Command::Edit(Edit::AddBlock {
                        layer,
                        id: BlockId(id()),
                        block: Block::new(
                            Ticks::from_bars(bar),
                            Ticks::from_bars(1),
                            Shape::ramp(0.2, 0.8),
                        ),
                    }));
                }
            }
        }
    });
    h.redraw();
    first_synth.expect("a synth").0
}

/// Median and 90th percentile of `frames` frames.
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// The fastest frame: what a frame costs with nothing else in the way
    /// (other programs make the rest noisy). For comparing changes.
    best: Duration,
    median: Duration,
    p90: Duration,
}

impl std::fmt::Display for Timing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "best {:>6.2} ms, median {:>6.2} ms, p90 {:>6.2} ms",
            self.best.as_secs_f64() * 1000.,
            self.median.as_secs_f64() * 1000.,
            self.p90.as_secs_f64() * 1000.
        )
    }
}

fn time(mut frame: impl FnMut()) -> Timing {
    for _ in 0..5 {
        frame();
    }
    let mut times: Vec<Duration> = (0..FRAMES)
        .map(|_| {
            let start = Instant::now();
            frame();
            start.elapsed()
        })
        .collect();
    times.sort();
    Timing {
        best: times[0],
        median: times[FRAMES / 2],
        p90: times[FRAMES * 9 / 10],
    }
}

/// Times both kinds of frame on what `h` shows now.
fn measure(h: &mut Harness, scene: &str) -> (Timing, Timing) {
    let steady = time(|| h.animation_frame());
    let full = time(|| h.redraw_all());
    eprintln!("perf {scene:<22} steady: {steady}   full: {full}");
    (steady, full)
}

fn ms(n: f64) -> Duration {
    Duration::from_secs_f64(n / 1000.)
}

/// Budgets, release build, median ms, set just above what the app measures
/// (2026-10-20, after steps 3 and 4 of `docs/design/ui-speed.md`). Tighten
/// them as the speed work goes on; never loosen one without the user.
fn check(scene: &str, (steady, full): (Timing, Timing), steady_ms: f64, full_ms: f64) {
    assert!(
        steady.median <= ms(steady_ms),
        "{scene}: a steady frame takes {steady}, over the {steady_ms} ms budget"
    );
    assert!(
        full.median <= ms(full_ms),
        "{scene}: a full frame takes {full}, over the {full_ms} ms budget"
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_arrange() {
    let mut h = Harness::new("perf-arrange");
    heavy_song(&mut h);
    let t = measure(&mut h, "arrange");
    check("arrange", t, 6., 20.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_arrange_with_piano_roll() {
    let mut h = Harness::new("perf-roll");
    let synth = heavy_song(&mut h);
    h.view
        .update(h.cx, |view, _| view.open_node_editor(synth, 0.));
    h.redraw();
    let t = measure(&mut h, "arrange + piano roll");
    check("arrange + piano roll", t, 6., 30.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_mixer() {
    let mut h = Harness::new("perf-mixer");
    heavy_song(&mut h);
    h.view.update(h.cx, |view, _| view.page = Page::Mixer);
    h.redraw();
    let t = measure(&mut h, "mixer");
    check("mixer", t, 6., 20.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_parameters() {
    let mut h = Harness::new("perf-parameters");
    let synth = heavy_song(&mut h);
    h.view.update(h.cx, |view, _| {
        view.selected_node = Some(synth);
        view.page = Page::Parameters;
    });
    h.redraw();
    let t = measure(&mut h, "parameters");
    check("parameters", t, 6., 25.);
}

/// Saves the full-size song as a project, to try speed by hand in the app:
/// `DAW_HEAVY_SONG=<folder> cargo test -p daw-ui --features ui-tests
/// save_heavy_song -- --ignored`.
#[test]
#[ignore = "writes a project; run by hand"]
fn save_heavy_song() {
    let folder = std::env::var_os("DAW_HEAVY_SONG").expect("DAW_HEAVY_SONG: where to save it");
    let mut h = Harness::in_folder(folder.into(), true);
    heavy_song(&mut h);
    h.view.update(h.cx, |view, _| {
        view.session.apply_or_panic(Command::Edit(Edit::RenameSong {
            name: "Heavy Song".into(),
        }));
        view.session.save().expect("saved");
    });
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_signals() {
    let mut h = Harness::new("perf-signals");
    heavy_song(&mut h);
    h.view.update(h.cx, |view, _| view.page = Page::Signals);
    h.redraw();
    let t = measure(&mut h, "signals");
    check("signals", t, 6., 15.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_audio_panel() {
    let mut h = Harness::new("perf-audio-panel");
    heavy_song(&mut h);
    let stem = h.view.read_with(h.cx, |view, _| {
        view.session()
            .snapshot()
            .nodes
            .iter()
            .find(|n| n.name == "Stem 0")
            .map(|n| n.id)
            .expect("a stem")
    });
    h.view.update(h.cx, |view, _| {
        view.selected_node = Some(stem);
        view.open_audio_editor(stem, 0.);
    });
    h.redraw();
    let t = measure(&mut h, "arrange + audio panel");
    check("arrange + audio panel", t, 6., 30.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_arrange_stems() {
    let mut h = Harness::new("perf-stems");
    heavy_song(&mut h);
    // Fold the synth groups so the stems' waveforms are on screen.
    h.view.update(h.cx, |view, _| {
        let groups: Vec<NodeId> = view
            .session()
            .snapshot()
            .nodes
            .iter()
            .filter(|n| n.name.starts_with("Group"))
            .map(|n| n.id)
            .collect();
        view.folded.extend(groups);
    });
    h.redraw();
    let t = measure(&mut h, "arrange, stems");
    check("arrange, stems", t, 6., 30.);
}
