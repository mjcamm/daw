//! Audio signals: published from an audio node, their line worked out in
//! the background, their follower set in the inspector.

use super::Harness;
use crate::project::Page;
use crate::session::ImportTarget;
use daw_automation::CompiledSignal;
use daw_engine::{AudioSource, NodeId, SignalId, SignalSource};
use gpui::{point, px};
use std::path::PathBuf;

/// Half a second (a beat at 120 BPM) at half scale (-6 dB), as a WAV file
/// outside the project, to import.
fn half_beat_wav(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-ui-audio-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp folder");
    let path = dir.join("hit.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("wav");
    for _ in 0..24_000 {
        writer.write_sample(0.5f32).expect("sample");
    }
    writer.finalize().expect("wav written");
    path
}

/// The starter song with the hit imported on a new audio node at the
/// start; returns the node.
fn with_hit(h: &mut Harness, test: &str) -> NodeId {
    let wav = half_beat_wav(test);
    h.view.update(h.cx, |v, _| {
        v.session
            .import_audio(
                &[wav],
                ImportTarget::NewNodes {
                    parent: NodeId::ROOT,
                    index: 1,
                },
            )
            .expect("imported");
    });
    let node = h
        .snapshot()
        .nodes
        .iter()
        .find(|n| !n.audio_clips.is_empty())
        .expect("the audio node")
        .id;
    h.settle_rows();
    node
}

fn publish(h: &mut Harness, node: NodeId) -> SignalId {
    let button: &'static str =
        Box::leak(format!("arrange-node-signals-{}", node.0).into_boxed_str());
    let at = h.bounds(button).center();
    h.click(at, 1);
    h.redraw_all();
    let at = h.bounds("arrange-publish-signal").center();
    h.click(at, 1);
    let snapshot = h.snapshot();
    let signal = snapshot.signals.last().expect("a signal");
    assert_eq!(
        signal.source,
        SignalSource::Audio(AudioSource::new(node)),
        "following the node, with a new signal's settings"
    );
    signal.id
}

/// docs/behaviours.md: "Signal button: ... Publish Audio Signal on an
/// audio node" and "An audio signal's line is worked out in the
/// background".
#[test]
fn an_audio_nodes_signal_draws_its_loudness_once_worked_out() {
    let mut h = Harness::new("audio-signal-line");
    let node = with_hit(&mut h, "line");
    let id = publish(&mut h, node);
    h.view.update(h.cx, |v, _| v.session.finish_lines());
    let line = h
        .view
        .read_with(h.cx, |v, _| v.compiled_signals().get(id).cloned());
    let Some(line @ CompiledSignal::Recorded { .. }) = line else {
        panic!("no line worked out: {line:?}");
    };
    // -6 dB between -48 and 0 dB is seven eighths of the way up.
    let loud = line.sample(0.5).0;
    assert!((loud - 0.9375).abs() < 0.01, "loud in the hit: {loud}");
    assert_eq!(line.sample(6.).0, 0.5, "the centre line long after it");
}

/// docs/behaviours.md: "Audio signal: Attack, Release, Floor and Ceiling
/// on sliders ...".
#[test]
fn an_audio_signals_follower_is_set_in_the_inspector() {
    let mut h = Harness::new("audio-signal-inspector");
    let node = with_hit(&mut h, "inspector");
    let id = publish(&mut h, node);
    h.view.update(h.cx, |v, _| {
        v.page = Page::Signals;
        v.selected_signal = Some(id);
        v.inspector_open = true;
    });
    h.redraw_all();
    let source = |h: &mut Harness| match &h.snapshot().signals[0].source {
        SignalSource::Audio(source) => source.clone(),
        other => panic!("{other:?}"),
    };

    // Attack to the slider's left end: no attack, one undo step.
    let attack = h.bounds("follow-attack");
    let from = attack.center();
    h.drag(from, point(attack.left() - px(40.), from.y), 4);
    assert_eq!(source(&mut h).attack_ms, 0.);
    h.keys("ctrl-z");
    assert_eq!(source(&mut h).attack_ms, 5., "one undo step");

    // The floor up to the ceiling's end stops a decibel below it.
    let floor = h.bounds("follow-floor");
    h.drag(
        floor.center(),
        point(floor.right() + px(40.), floor.center().y),
        4,
    );
    assert_eq!(source(&mut h).floor_db, -1.);

    // Double-click puts it back.
    h.redraw_all();
    let floor = h.bounds("follow-floor").center();
    h.click(floor, 2);
    assert_eq!(source(&mut h).floor_db, -48.);
}
