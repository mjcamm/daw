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
