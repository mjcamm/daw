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
    BlendMode, Block, BlockId, ClipId, Command, DeviceId, DeviceRef, Edit, Layer, LayerId,
    MidiClip, MidiNote, Node, NodeId, ParamId, Shape, Ticks,
};
use std::time::{Duration, Instant};

/// Groups, nodes per group, bars, notes per bar.
const GROUPS: u64 = 4;
const PER_GROUP: u64 = 6;
const BARS: i64 = 64;
const NOTES_PER_BAR: i64 = 8;
/// Frames timed per measurement, after a few to warm up.
const FRAMES: usize = 40;

/// A full-size song: 4 groups of 6 synth nodes, each with a 4-bar clip
/// every 4 bars across 64 bars (8 notes a bar, about 12,000 notes), and
/// the first synth's parameters layered with ramps.
fn heavy_song(h: &mut Harness) -> NodeId {
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
        // Automation to draw on the Parameters page.
        let (node, device) = first_synth.expect("a synth");
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
    median: Duration,
    p90: Duration,
}

impl std::fmt::Display for Timing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "median {:>6.2} ms, p90 {:>6.2} ms",
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

/// Budgets, release build, median ms. They start just above what the app
/// measured before the speed work (2026-10-20) and tighten as each step
/// lands, towards the targets in `docs/design/ui-speed.md`: a steady
/// (playback) frame under 1 ms, a full frame under 5 ms.
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
    check("arrange", t, 330., 350.);
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
    check("arrange + piano roll", t, 360., 350.);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timings need a release build")]
fn perf_mixer() {
    let mut h = Harness::new("perf-mixer");
    heavy_song(&mut h);
    h.view.update(h.cx, |view, _| view.page = Page::Mixer);
    h.redraw();
    let t = measure(&mut h, "mixer");
    check("mixer", t, 20., 20.);
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
    check("parameters", t, 95., 85.);
}
