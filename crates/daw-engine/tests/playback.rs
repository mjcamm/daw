//! A node playing MIDI clips through its instrument, in song time.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::devices::{CHANNEL_PARAMS, channel};
use daw_engine::{
    AudioProcessor, ClipId, Command, CommandError, DeviceRef, Edit, Engine, EngineConfig,
    FIRST_NODE, LoopRange, MidiClip, MidiNote, ParamId, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, assert_reference, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;

fn engine() -> (Engine, AudioProcessor) {
    Engine::new(EngineConfig::default())
}

fn beats(n: f64) -> Ticks {
    Ticks::from_beats_f64(n)
}

/// A note given as (start beat, length in beats, pitch).
fn note((start, length, pitch): (f64, f64, u8)) -> MidiNote {
    MidiNote {
        start: beats(start),
        length: beats(length),
        pitch,
        velocity: 100,
    }
}

fn add_clip(engine: &mut Engine, id: u64, start: f64, length: f64, notes: &[(f64, f64, u8)]) {
    let clip = MidiClip {
        start: beats(start),
        length: beats(length),
        notes: notes.iter().copied().map(note).collect(),
    };
    engine
        .apply(Command::Edit(Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(id),
            clip,
        }))
        .unwrap();
}

fn first_sound(rendered: &Rendered) -> Option<usize> {
    rendered
        .samples
        .iter()
        .position(|&s| s != 0.0)
        .map(|i| i / usize::from(rendered.channels))
}

fn frames(rendered: &Rendered, range: std::ops::Range<usize>) -> &[f32] {
    let channels = usize::from(rendered.channels);
    &rendered.samples[range.start * channels..range.end * channels]
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |p, s| p.max(s.abs()))
}

fn assert_silent(samples: &[f32]) {
    assert!(
        samples.iter().all(|&s| s == 0.0),
        "expected silence, peak {}",
        peak(samples)
    );
}

/// The first sample of a band-limited saw is exactly zero, so a note is heard
/// on its own frame or the one after.
fn assert_starts_at(rendered: &Rendered, frame: usize) {
    let start = first_sound(rendered).expect("no sound at all");
    assert!(
        (frame..=frame + 1).contains(&start),
        "sound started at frame {start}, expected {frame}"
    );
}

#[test]
fn note_starts_on_its_exact_frame() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(1.0, 1.0, 60)]);
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 30_000, VARIED_BLOCKS);
    assert_starts_at(&rendered, FRAMES_PER_BEAT);
}

#[test]
fn clip_position_offsets_its_notes() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 2.0, 4.0, &[(0.0, 1.0, 60)]);
    engine.apply(Command::Play).unwrap();
    let rendered = render(
        &mut engine,
        &mut processor,
        3 * FRAMES_PER_BEAT,
        VARIED_BLOCKS,
    );
    assert_starts_at(&rendered, 2 * FRAMES_PER_BEAT);
}

#[test]
fn tempo_sets_note_timing() {
    let (mut engine, mut processor) = engine();
    engine
        .apply(Command::Edit(Edit::SetTempo { bpm: 60.0 }))
        .unwrap();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(1.0, 1.0, 60)]);
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 50_000, VARIED_BLOCKS);
    assert_starts_at(&rendered, 48_000);
}

#[test]
fn tempo_change_while_playing_keeps_musical_position() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(1.0, 1.0, 60)]);
    engine.apply(Command::Play).unwrap();
    // Half a beat at 120 BPM, then the remaining half at 60 BPM (24,000 frames).
    let before = render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    assert_silent(&before.samples);
    engine
        .apply(Command::Edit(Edit::SetTempo { bpm: 60.0 }))
        .unwrap();
    let after = render(&mut engine, &mut processor, 30_000, VARIED_BLOCKS);
    assert_starts_at(&after, 24_000);
}

/// A busy pattern exercising overlapping notes, a loop and a mid-render edit.
fn render_busy_pattern(blocks: &[usize]) -> Rendered {
    let (mut engine, mut processor) = engine();
    let mut notes = Vec::new();
    for step in 0..32 {
        let start = f64::from(step) * 0.25;
        notes.push((start, 0.2, 48 + (step * 7 % 24) as u8));
        if step % 4 == 0 {
            notes.push((start, 1.5, 60)); // overlaps the following notes
        }
    }
    // A note off the grid (clips on a node never overlap, so it is in the
    // same clip rather than a second one stacked on it).
    notes.push((1.0 / 3.0, 1.0 / 7.0, 72));
    add_clip(&mut engine, 1, 0.0, 8.0, &notes);
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: beats(1.0),
            end: beats(5.5),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    let mut first = render(&mut engine, &mut processor, 100_000, blocks);
    engine
        .apply(Command::Edit(Edit::SetTempo { bpm: 137.0 }))
        .unwrap();
    let second = render(&mut engine, &mut processor, 200_000, blocks);
    first.samples.extend(second.samples);
    first
}

#[test]
fn output_is_identical_for_any_block_size() {
    // This is what guarantees live playback matches offline renders: a host
    // may call with any block size and the result must not change.
    let reference = render_busy_pattern(&[480]);
    assert!(!reference.is_silent());
    for blocks in [&[1][..], VARIED_BLOCKS, &[4096], &[1023, 1025]] {
        let other = render_busy_pattern(blocks);
        assert!(other == reference, "blocks {blocks:?} changed the output");
    }
}

#[test]
fn busy_pattern_matches_reference_recording() {
    assert_reference!("busy-pattern", &render_busy_pattern(VARIED_BLOCKS));
}

#[test]
fn loop_repeats_identically() {
    let (mut engine, mut processor) = engine();
    // The note and its release tail fit well inside the one-bar loop.
    add_clip(&mut engine, 1, 0.0, 4.0, &[(0.0, 0.5, 60), (2.0, 0.25, 67)]);
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: Ticks::ZERO,
            end: Ticks::from_bars(1),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    let bar = 4 * FRAMES_PER_BEAT;
    let rendered = render(&mut engine, &mut processor, 3 * bar, VARIED_BLOCKS);
    let first = frames(&rendered, 0..bar);
    assert!(!rendered.is_silent());
    assert!(first == frames(&rendered, bar..2 * bar));
    assert!(first == frames(&rendered, 2 * bar..3 * bar));
    assert!(engine.snapshot().position_beats < 4.0);
}

#[test]
fn stop_releases_sounding_notes() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 8.0, &[(0.0, 8.0, 60)]);
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    engine.apply(Command::Stop).unwrap();
    let tail = render(&mut engine, &mut processor, 48_000, VARIED_BLOCKS);
    assert!(peak(frames(&tail, 0..100)) > 0.0, "release should ring out");
    assert_silent(frames(&tail, 47_000..48_000));
}

#[test]
fn removing_a_clip_mid_note_still_ends_the_note() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(0.0, 2.0, 60)]);
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 6_000, VARIED_BLOCKS);
    engine
        .apply(Command::Edit(Edit::RemoveClip { id: ClipId(1) }))
        .unwrap();
    let rest = render(
        &mut engine,
        &mut processor,
        3 * FRAMES_PER_BEAT,
        VARIED_BLOCKS,
    );
    // The note ends at beat 2 (frame 42,000 of this render); half a beat
    // later its release has finished.
    assert!(
        peak(frames(&rest, 40_000..42_000)) > 0.0,
        "note ended early"
    );
    assert_silent(frames(&rest, 54_000..3 * FRAMES_PER_BEAT));
}

#[test]
fn undoing_a_clip_silences_it() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(0.0, 1.0, 60)]);
    engine.apply(Command::Undo).unwrap();
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, FRAMES_PER_BEAT, VARIED_BLOCKS);
    assert!(rendered.is_silent());
}

#[test]
fn locate_moves_where_playback_starts() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(2.0, 1.0, 60)]);
    engine
        .apply(Command::Locate {
            position: beats(1.5),
        })
        .unwrap();
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, FRAMES_PER_BEAT, VARIED_BLOCKS);
    assert_starts_at(&rendered, FRAMES_PER_BEAT / 2);
}

fn channel_param(index: u32, plain: f64) -> Edit {
    let info = &CHANNEL_PARAMS[index as usize];
    Edit::SetParam {
        param: ParamId {
            node: FIRST_NODE,
            device: DeviceRef::Channel,
            index,
        },
        value: info.scale.to_normalized(plain),
    }
}

fn volume_db(db: f64) -> Edit {
    channel_param(channel::VOLUME, db)
}

fn pan(pan: f64) -> Edit {
    channel_param(channel::PAN, pan)
}

/// Renders a held note with volume and pan applied before playback, skipping
/// the first moments.
fn render_held_note(edits: &[Edit], channels: u16) -> Rendered {
    let (mut engine, mut processor) = Engine::new(EngineConfig {
        channels,
        ..EngineConfig::default()
    });
    add_clip(&mut engine, 1, 0.0, 4.0, &[(0.0, 4.0, 60)]);
    for edit in edits {
        engine.apply(Command::Edit(edit.clone())).unwrap();
    }
    engine.apply(Command::Play).unwrap();
    let mut rendered = render(&mut engine, &mut processor, 24_000, VARIED_BLOCKS);
    let skip = 1_000 * usize::from(channels);
    rendered.samples.drain(..skip);
    rendered
}

fn channel(rendered: &Rendered, index: usize) -> Vec<f32> {
    rendered
        .samples
        .iter()
        .skip(index)
        .step_by(usize::from(rendered.channels))
        .copied()
        .collect()
}

#[test]
fn volume_scales_the_output() {
    let full = render_held_note(&[], 2);
    let half = render_held_note(&[volume_db(-6.0206)], 2);
    let ratio = peak(&half.samples) / peak(&full.samples);
    assert!((ratio - 0.5).abs() < 1e-3, "{ratio}");

    let muted = render_held_note(&[volume_db(f64::NEG_INFINITY)], 2);
    assert!(muted.is_silent());
}

#[test]
fn pan_moves_sound_between_channels() {
    let left = render_held_note(&[pan(-1.0)], 2);
    assert!(peak(&channel(&left, 0)) > 0.01);
    assert_silent(&channel(&left, 1));

    let centre = render_held_note(&[], 2);
    assert_eq!(channel(&centre, 0), channel(&centre, 1));
}

#[test]
fn mono_output_keeps_panned_sound() {
    let mono = render_held_note(&[pan(1.0)], 1);
    assert!(peak(&mono.samples) > 0.01);
}

#[test]
fn extra_output_channels_are_silent() {
    let quad = render_held_note(&[], 4);
    assert!(peak(&channel(&quad, 0)) > 0.01);
    assert_silent(&channel(&quad, 2));
    assert_silent(&channel(&quad, 3));
}

#[test]
fn invalid_transport_commands_are_rejected() {
    let (mut engine, _processor) = engine();
    let before = engine.snapshot();
    assert!(matches!(
        engine.apply(Command::Locate {
            position: Ticks(-1)
        }),
        Err(CommandError::NegativePosition { .. })
    ));
    for (start, end) in [(-1, 960), (960, 960), (960, 0), (0, 59)] {
        let range = LoopRange {
            start: Ticks(start),
            end: Ticks(end),
        };
        assert!(
            matches!(
                engine.apply(Command::SetLoop(Some(range))),
                Err(CommandError::InvalidLoop { .. })
            ),
            "{range:?}"
        );
    }
    assert_eq!(engine.snapshot(), before);
}

#[test]
fn snapshot_lists_clips_and_mix_settings() {
    let (mut engine, _processor) = engine();
    add_clip(&mut engine, 5, 4.0, 4.0, &[(0.0, 1.0, 60), (1.0, 1.0, 62)]);
    engine.apply(Command::Edit(pan(0.25))).unwrap();
    let range = LoopRange {
        start: Ticks::ZERO,
        end: Ticks::from_bars(2),
    };
    engine.apply(Command::SetLoop(Some(range))).unwrap();
    let snapshot = engine.snapshot();
    let channel = snapshot.node(FIRST_NODE).unwrap().devices.last().unwrap();
    assert_eq!(channel.device, DeviceRef::Channel);
    assert_eq!(channel.params[1].display, "R25");
    assert_eq!(snapshot.loop_range, Some(range));
    assert_eq!(snapshot.node(FIRST_NODE).unwrap().clips.len(), 1);
    assert_eq!(snapshot.node(FIRST_NODE).unwrap().clips[0].0, ClipId(5));
    assert_eq!(
        snapshot.node(FIRST_NODE).unwrap().clips[0].1.start,
        beats(4.0)
    );
    assert_eq!(snapshot.node(FIRST_NODE).unwrap().clips[0].1.notes.len(), 2);
}

#[test]
fn constant_editing_during_playback_stays_realtime_safe() {
    // The realtime guard aborts this test if the audio side allocates or frees.
    let (mut engine, mut processor) = engine();
    engine.apply(Command::Play).unwrap();
    // Sixteen quarter-beat slots, each holding at most one clip: clips on a
    // node never overlap, so a slot's clip goes before another takes it.
    let mut slots: [Option<u64>; 16] = [None; 16];
    for i in 0..500_u64 {
        let slot = (i % 16) as usize;
        let start = slot as f64 * 0.25;
        if let Some(old) = slots[slot].take() {
            engine
                .apply(Command::Edit(Edit::RemoveClip { id: ClipId(old) }))
                .unwrap();
        }
        add_clip(
            &mut engine,
            i,
            start,
            0.25,
            &[(0.0, 0.2, 40 + (i % 40) as u8)],
        );
        if i % 3 == 0 {
            engine
                .apply(Command::Edit(Edit::RemoveClip { id: ClipId(i) }))
                .unwrap();
        } else {
            slots[slot] = Some(i);
        }
        if i % 50 == 0 {
            engine
                .apply(Command::Locate {
                    position: Ticks::ZERO,
                })
                .unwrap();
        }
        render(&mut engine, &mut processor, 97, &[13, 31]);
    }
}

#[test]
fn muting_silences_the_node_and_undo_brings_it_back() {
    let muted = render_held_note(
        &[Edit::SetMute {
            node: FIRST_NODE,
            muted: true,
        }],
        2,
    );
    assert!(muted.is_silent());

    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(0.0, 4.0, 60)]);
    engine
        .apply(Command::Edit(Edit::SetMute {
            node: FIRST_NODE,
            muted: true,
        }))
        .unwrap();
    engine.apply(Command::Undo).unwrap();
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    assert!(peak(&rendered.samples) > 0.01);
    assert!(!engine.snapshot().node(FIRST_NODE).unwrap().muted);
}

#[test]
fn soloing_the_only_node_keeps_it_audible() {
    let soloed = render_held_note(
        &[Edit::SetSolo {
            node: FIRST_NODE,
            soloed: true,
        }],
        2,
    );
    let normal = render_held_note(&[], 2);
    assert!(soloed == normal);
}

#[test]
fn meter_reports_output_peaks_since_last_read() {
    let (mut engine, mut processor) = engine();
    add_clip(&mut engine, 1, 0.0, 4.0, &[(0.0, 4.0, 60)]);
    engine.apply(Command::Edit(pan(-0.5))).unwrap();
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    let meter = engine.take_meter();
    assert_eq!(meter.left, peak(&channel(&rendered, 0)));
    assert_eq!(meter.right, peak(&channel(&rendered, 1)));
    assert!(meter.left > meter.right, "panned left");
    // Reading resets the peaks.
    assert_eq!(engine.take_meter(), daw_engine::Meter::default());
}

#[test]
fn snapshot_describes_the_node() {
    let (engine, _processor) = engine();
    let snapshot = engine.snapshot();
    let node = snapshot.node(FIRST_NODE).unwrap();
    assert_eq!(node.name, "Synth");
    assert!(!node.muted && !node.soloed);
    assert_eq!(node.parent, Some(daw_engine::NodeId::ROOT));
    assert_eq!(snapshot.root().children, [FIRST_NODE]);
}

#[test]
fn the_test_tone_plays_one_second_at_minus_12_db() {
    let (mut engine, mut processor) = engine();
    engine.apply(Command::TestTone).unwrap();
    let rendered = render(&mut engine, &mut processor, 60_000, VARIED_BLOCKS);
    let tone = frames(&rendered, 0..48_000);
    assert!((peak(tone) - 0.251).abs() < 0.01, "peak {}", peak(tone));
    assert_silent(frames(&rendered, 48_000..60_000));
}
