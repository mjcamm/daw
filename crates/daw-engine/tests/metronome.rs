//! The metronome: a click on every beat while playing, on its exact frame,
//! over the output after the Song's channel.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::devices::channel;
use daw_engine::{
    ClickSound, Command, DeviceRef, Edit, Engine, EngineConfig, LoopRange, Metronome, NodeId,
    ParamId, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const BEAT: usize = 24_000;

fn engine_with(metronome: Metronome) -> (Engine, daw_engine::AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    engine.apply(Command::SetMetronome(metronome)).unwrap();
    (engine, processor)
}

fn on() -> Metronome {
    Metronome {
        on: true,
        ..Metronome::default()
    }
}

fn play(metronome: Metronome, frames: usize, blocks: &[usize]) -> Rendered {
    let (mut engine, mut processor) = engine_with(metronome);
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, frames, blocks)
}

/// The left channel's peak over frames `from..to`.
fn peak(rendered: &Rendered, from: usize, to: usize) -> f32 {
    let channels = usize::from(rendered.channels);
    rendered.samples[from * channels..to * channels]
        .iter()
        .step_by(channels)
        .fold(0., |m, s| m.max(s.abs()))
}

/// The first frame that sounds at or after `from`.
fn onset(rendered: &Rendered, from: usize) -> usize {
    let channels = usize::from(rendered.channels);
    (from..rendered.frames())
        .find(|f| rendered.samples[f * channels].abs() > 1e-6)
        .expect("a click")
}

#[test]
fn it_clicks_on_every_beat_while_playing() {
    let out = play(on(), BEAT * 3 + 100, VARIED_BLOCKS);
    for beat in 0..3 {
        let at = beat * BEAT;
        assert!(peak(&out, at, at + 2_000) > 0.05, "a click at beat {beat}");
        // It starts on the beat's own frame, rising from nothing, so its
        // first sound is the frame after.
        assert_eq!(onset(&out, at), at + 1, "on the beat's own frame");
        if beat > 0 {
            assert_eq!(peak(&out, at - 2_000, at), 0., "nothing before it");
        }
        assert_eq!(peak(&out, at + 8_000, at + BEAT), 0., "silent between");
    }
}

#[test]
fn a_bars_first_beat_is_accented_unless_switched_off() {
    let out = play(on(), BEAT * 5, VARIED_BLOCKS);
    let first = peak(&out, 0, 2_000);
    let second = peak(&out, BEAT, BEAT + 2_000);
    let fifth = peak(&out, BEAT * 4, BEAT * 4 + 2_000);
    assert!(first > second * 1.2, "beat 1 stands out");
    assert!((fifth - first).abs() < 1e-6, "every bar's beat 1");
    let plain = play(
        Metronome {
            accent: false,
            ..on()
        },
        BEAT * 2,
        &[512],
    );
    assert!((peak(&plain, 0, 2_000) - peak(&plain, BEAT, BEAT + 2_000)).abs() < 1e-6);
}

#[test]
fn off_stopped_or_at_no_volume_it_is_silent() {
    assert!(
        play(Metronome::default(), BEAT * 2, &[512]).is_silent(),
        "off"
    );
    let quiet = Metronome { volume: 0, ..on() };
    assert!(play(quiet, BEAT * 2, &[512]).is_silent(), "no volume");
    let (mut engine, mut processor) = engine_with(on());
    assert!(
        render(&mut engine, &mut processor, BEAT * 2, &[512]).is_silent(),
        "stopped"
    );
}

#[test]
fn clicks_are_the_same_for_any_block_size() {
    let a = play(on(), BEAT * 3, VARIED_BLOCKS);
    let b = play(on(), BEAT * 3, &[4096]);
    assert_eq!(a.samples, b.samples);
}

#[test]
fn every_sound_and_pitch_clicks() {
    for sound in ClickSound::ALL {
        for pitch in [-12, 0, 12] {
            let out = play(
                Metronome {
                    sound,
                    pitch,
                    ..on()
                },
                BEAT,
                &[512],
            );
            assert!(peak(&out, 0, 2_000) > 0.02, "{sound:?} at {pitch}");
        }
    }
    let low = play(Metronome { pitch: -12, ..on() }, 2_000, &[512]);
    let high = play(Metronome { pitch: 12, ..on() }, 2_000, &[512]);
    assert_ne!(low.samples, high.samples);
}

#[test]
fn it_follows_the_loop_back_to_its_start() {
    let (mut engine, mut processor) = engine_with(on());
    // A two-beat loop: clicks at 0 and 1, then 0 (accented) again.
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: Ticks::ZERO,
            end: Ticks::from_beats(2),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    let out = render(&mut engine, &mut processor, BEAT * 5, VARIED_BLOCKS);
    for beat in 0..5 {
        let at = beat * BEAT;
        assert_eq!(onset(&out, at), at + 1, "a click at {beat}");
    }
    let accent = peak(&out, 0, 2_000);
    assert!((peak(&out, BEAT * 2, BEAT * 2 + 2_000) - accent).abs() < 1e-6);
    assert!((peak(&out, BEAT * 4, BEAT * 4 + 2_000) - accent).abs() < 1e-6);
}

#[test]
fn it_is_heard_over_the_songs_channel_not_through_it() {
    let (mut engine, mut processor) = engine_with(on());
    engine
        .apply(Command::Edit(Edit::SetParam {
            param: ParamId {
                node: NodeId::ROOT,
                device: DeviceRef::Channel,
                index: channel::VOLUME,
            },
            value: 0.0,
        }))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    let out = render(&mut engine, &mut processor, 4_000, &[512]);
    assert!(
        peak(&out, 0, 2_000) > 0.05,
        "the Song's fader leaves it alone"
    );
}

#[test]
fn a_preview_click_sounds_while_stopped() {
    let (mut engine, mut processor) = engine_with(Metronome::default());
    engine.apply(Command::PreviewClick).unwrap();
    let out = render(&mut engine, &mut processor, 4_000, &[512]);
    assert!(peak(&out, 0, 2_000) > 0.05);
}

#[test]
fn the_setting_shows_and_is_not_part_of_the_song() {
    let (mut engine, _processor) = engine_with(Metronome {
        volume: 200,
        pitch: -40,
        ..on()
    });
    let shown = engine.snapshot().metronome;
    assert!(shown.on);
    assert_eq!((shown.volume, shown.pitch), (100, -12), "kept in range");
    assert!(!engine.snapshot().can_undo, "not an edit");
    // A song saved and opened again (as an export does) has no click.
    let file = engine.to_file();
    let (mut fresh, _processor) = Engine::new(EngineConfig::default());
    fresh.load(file).unwrap();
    assert!(!fresh.snapshot().metronome.on);
}
