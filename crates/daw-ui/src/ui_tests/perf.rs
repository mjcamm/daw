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
                            looping: None,
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

/// The device browser open over the mixer, on the Plugins side with 150
/// plugins (the user's machine has 148).
#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_browser() {
    use crate::browser::{PluginShelf, Target};
    let mut h = Harness::new("perf-browser");
    heavy_song(&mut h);
    let plugins = (0..150)
        .map(|i| daw_plugins::scan::PluginInfo {
            bundle: format!("C:/VST3/Plugin {i}.vst3").into(),
            cid: format!("{i:032X}"),
            name: format!("Plugin {i}"),
            vendor: format!("Vendor {}", i / 6),
            version: "1.0".into(),
            subcategories: vec!["Fx".into(), "Delay".into()],
        })
        .collect();
    let shelf = PluginShelf {
        plugins: std::sync::Arc::new(plugins),
        scanning: None,
    };
    h.view.update_in(h.cx, |view, window, cx| {
        view.page = Page::Mixer;
        view.set_plugins(shelf, cx);
        view.open_browser(Target::Effect(daw_engine::FIRST_NODE), window, cx);
    });
    h.redraw();
    let plugins = h.bounds("browser-side-Plugins").center();
    h.click(plugins, 1);
    let t = measure(&mut h, "browser");
    check("browser", t, 6., 20.);
}

/// The export dialog open over Arrange.
#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_export() {
    let mut h = Harness::new("perf-export");
    heavy_song(&mut h);
    h.keys("ctrl-e");
    let t = measure(&mut h, "export");
    check("export", t, 2., 6.);
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

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_param_picker() {
    use daw_engine::{DeviceId, Effect, PluginParamInfo, PluginSpec, TestPlugin};
    let mut h = Harness::new("perf-param-picker");
    heavy_song(&mut h);
    // A stem's few rows keep its effects on screen.
    let stem = h.view.read_with(h.cx, |view, _| {
        view.session()
            .snapshot()
            .nodes
            .iter()
            .find(|n| n.name == "Stem 0")
            .map(|n| n.id)
            .expect("a stem")
    });
    let plugin = DeviceId(9_000);
    // As many parameters as a big synth, in groups of 40.
    let params = (0..600)
        .map(|i| PluginParamInfo {
            id: i,
            name: format!("Parameter {i}"),
            group: format!("Group {}", i / 40),
            unit: String::new(),
            steps: 0,
            value: 0.5,
            bypass: false,
        })
        .collect();
    h.view.update(h.cx, |view, _| {
        view.session.apply_or_panic(Command::Edit(Edit::AddEffect {
            node: stem,
            id: plugin,
            index: 0,
            effect: Effect::plugin(PluginSpec::Test(TestPlugin::Gain)),
        }));
        view.session.show_plugin_params(plugin, params);
        view.selected_node = Some(stem);
        view.page = Page::Parameters;
    });
    h.redraw_all();
    let add = h.bounds("add-param-effect-9000").center();
    h.click(add, 1);
    h.bounds("param-picker");
    let t = measure(&mut h, "parameters + plugin list");
    check("parameters + plugin list", t, 3., 10.);
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

/// The Signals page with an audio signal following the stems, its line
/// worked out over the whole song.
#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_signals_with_audio() {
    let mut h = Harness::new("perf-signals-audio");
    heavy_song(&mut h);
    h.view.update(h.cx, |view, _| {
        let stems = view
            .session()
            .snapshot()
            .nodes
            .iter()
            .find(|n| n.name == "Stems")
            .expect("the stems")
            .id;
        view.session.apply_or_panic(Command::Edit(Edit::AddSignal {
            id: SignalId(9_000),
            signal: Signal {
                name: "Stems audio".into(),
                color: 2,
                source: SignalSource::Audio(daw_engine::AudioSource::new(stems)),
            },
            listeners: Vec::new(),
        }));
        view.session.finish_lines();
        view.page = Page::Signals;
    });
    h.redraw();
    let t = measure(&mut h, "signals with audio");
    check("signals with audio", t, 6., 15.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_signals_with_shape_panel() {
    let mut h = Harness::new("perf-shape-panel");
    heavy_song(&mut h);
    h.view.update(h.cx, |view, _| {
        let snapshot = view.session().snapshot();
        let signal = snapshot
            .signals
            .iter()
            .find(|s| s.name == "Notes")
            .expect("a notes signal");
        let SignalSource::Notes(source) = &signal.source else {
            panic!("a notes signal");
        };
        // A dense shape: a triangle in every 1/16 of a 4-bar window.
        let mut points = vec![daw_engine::CurvePoint::corner(0., 0.)];
        for cell in 0..64 {
            let at = |part: f64| (f64::from(cell) + part) / 64.;
            points.push(daw_engine::CurvePoint::corner(at(0.5), 1.));
            points.push(daw_engine::CurvePoint::corner(at(1.), 0.));
        }
        let source = NotesSource {
            window: Ticks::from_bars(4),
            shape: daw_engine::Curve { points },
            ..source.clone()
        };
        let id = signal.id;
        view.session
            .apply_or_panic(Command::Edit(Edit::SetNotesSource { id, source }));
        view.page = Page::Signals;
        view.select_signal(id);
        if let Some(editor) = &mut view.shape_editor {
            editor.opened -= std::time::Duration::from_secs(1);
            editor.selected = Some(1);
        }
    });
    h.redraw();
    let t = measure(&mut h, "signals + shape panel");
    check("signals + shape panel", t, 6., 15.);
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

/// A frame while the pointer drags in Arrange's lanes: only the pane the
/// drag began in redraws (`ProjectView::render`).
#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_arrange_lanes_redraw() {
    let mut h = Harness::new("perf-lanes");
    heavy_song(&mut h);
    let main = h
        .view
        .read_with(h.cx, |view, _| view.pane(crate::live::PaneKind::Main));
    let t = time(|| {
        main.update(h.cx, |_, cx| cx.notify());
        h.cx.run_until_parked();
    });
    eprintln!("perf {:<22} lanes pane: {t}", "arrange lanes redraw");
    assert!(t.median <= ms(10.), "arrange lanes redraw: {t}");
}

/// What hearing a drag costs a frame on the full-size song: the song
/// compiled again for the audio side (`Command::Preview`), here for a
/// fader. The frame around it is the page's own cost while dragging.
#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_drag_preview() {
    let mut h = Harness::new("perf-drag-preview");
    heavy_song(&mut h);
    let param = crate::mixer::volume_id(NodeId::ROOT);
    let mut down = false;
    let preview = time(|| {
        down = !down;
        let value = if down { 0.5 } else { 0.6 };
        h.view.update(h.cx, |view, _| {
            view.session.preview(Some(Edit::SetParam { param, value }));
        });
    });
    eprintln!("perf {:<22} preview: {preview}", "drag preview");
    assert!(
        preview.median <= ms(1.),
        "hearing a drag takes {preview} a frame, over the 1 ms budget"
    );
}
