//! The transport: back, play/stop, loop, the position and the tempo.

use super::Harness;

/// docs/behaviours.md: "Tempo beside the transport: click to type, Enter
/// applies (one undo step)".
#[test]
fn typing_a_tempo_sets_the_song_tempo() {
    let mut h = Harness::new("tempo-type");
    let before = h.snapshot().tempo_bpm;
    let at = h.bounds("transport-tempo").center();
    h.click(at, 1);
    h.type_text("128.5");
    h.keys("enter");
    assert_eq!(h.snapshot().tempo_bpm, 128.5);
    assert!(
        h.view.read_with(h.cx, |v, _| v.tempo_input.is_none()),
        "the field closes"
    );
    h.keys("ctrl-z");
    assert_eq!(h.snapshot().tempo_bpm, before, "one undo step");
}

/// docs/behaviours.md: "Escape, or a tempo outside 20 to 999, leaves it".
#[test]
fn escape_or_an_impossible_tempo_leaves_the_tempo() {
    let mut h = Harness::new("tempo-cancel");
    let before = h.snapshot().tempo_bpm;
    let at = h.bounds("transport-tempo").center();

    h.click(at, 1);
    h.type_text("140");
    h.keys("escape");
    assert_eq!(h.snapshot().tempo_bpm, before, "Escape cancels");
    assert!(h.view.read_with(h.cx, |v, _| v.tempo_input.is_none()));

    h.click(at, 1);
    h.type_text("5");
    h.keys("enter");
    assert_eq!(h.snapshot().tempo_bpm, before, "below 20 BPM is refused");
    assert!(h.view.read_with(h.cx, |v, _| v.tempo_input.is_none()));
}

/// docs/behaviours.md: "Shift+Space plays from the loop's start (the song's
/// start without a loop), from stopped or while playing".
#[test]
fn shift_space_plays_from_the_loop_start() {
    use daw_engine::{Command, LoopRange, Ticks};
    let mut h = Harness::new("play-from-loop");
    let located = |h: &mut Harness| h.view.read_with(h.cx, |v, _| v.session().located);
    // No loop: from the song's start.
    h.view.update(h.cx, |v, _| {
        v.session.apply_or_panic(Command::SetLoop(None))
    });
    // The view reads the song as last drawn, as when a key comes.
    h.redraw();
    h.keys("shift-space");
    assert!(h.snapshot().playing, "plays");
    assert_eq!(located(&mut h), Some(Ticks::ZERO));

    // A loop over bars 3 to 5, switched off: still from its start, and
    // again while playing.
    let range = LoopRange {
        start: Ticks::from_bars(2),
        end: Ticks::from_bars(4),
    };
    h.view.update(h.cx, |v, _| {
        v.session.apply_or_panic(Command::SetLoop(Some(range)));
        v.session.apply_or_panic(Command::SetLoopEnabled(false));
    });
    h.redraw();
    h.keys("shift-space");
    assert!(h.snapshot().playing, "still playing");
    assert_eq!(located(&mut h), Some(range.start));

    h.keys("space");
    assert!(!h.snapshot().playing, "Space still stops");
}
