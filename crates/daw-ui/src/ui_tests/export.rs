//! File → Export…: the song to a WAV or MP3 file.

use super::Harness;
use std::time::{Duration, Instant};

fn exporting(h: &mut Harness) -> bool {
    h.view.read_with(h.cx, |v, _| v.export.is_some())
}

/// Lets a running export finish, showing its progress as it goes.
fn finish(h: &mut Harness) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        h.cx.executor().advance_clock(Duration::from_millis(50));
        h.redraw();
        let running = h.view.read_with(h.cx, |v, cx| {
            v.export
                .as_ref()
                .is_some_and(|(dialog, _)| dialog.read(cx).is_running())
        });
        if !running {
            break;
        }
        assert!(Instant::now() < deadline, "the export never finished");
        std::hint::spin_loop();
    }
}

/// docs/behaviours.md: "File → Export… (Ctrl+E): format, quality, range
/// and tail, then a save dialog starting in the project's Exports folder".
#[test]
fn ctrl_e_exports_the_song_to_the_chosen_file() {
    let mut h = Harness::new("export");
    h.keys("ctrl-e");
    assert!(exporting(&mut h), "Ctrl+E opens the dialog");
    // 32-bit float, no tail.
    let depth = h.bounds("export-depth-2").center();
    h.click(depth, 1);
    let tail = h.bounds("export-tail-1").center();
    h.click(tail, 1);

    let start = h.bounds("export-start").center();
    h.click(start, 1);
    assert!(h.cx.did_prompt_for_new_path(), "asks where to save");
    let exports = h.folder().join(crate::export_dialog::EXPORTS_FOLDER);
    assert!(exports.is_dir(), "the dialog starts in the Exports folder");
    let target = exports.join("mix");
    let chosen = target.clone();
    h.cx.simulate_new_path_selection(move |_| Some(chosen));
    h.redraw();
    finish(&mut h);

    let file = exports.join("mix.wav");
    let (path, exported) = h.view.read_with(h.cx, |v, cx| {
        let (dialog, _) = v.export.as_ref().expect("still open");
        let dialog = dialog.read(cx);
        assert_eq!(dialog.failure(), None);
        let (path, exported) = dialog.exported().expect("finished");
        (path.to_owned(), exported)
    });
    assert_eq!(path, file, "the format's extension is added");
    let reader = hound::WavReader::open(&file).expect("a WAV file");
    assert_eq!(reader.spec().bits_per_sample, 32);
    assert!(reader.duration() > 0);
    assert!(exported.peak > 0.0, "the starter song is not silent");
    h.bounds("export-done");

    let done = h.bounds("export-finish").center();
    h.click(done, 1);
    assert!(!exporting(&mut h), "Done closes it");
}

/// docs/behaviours.md: "Export closes with Cancel, Escape or a click
/// outside".
#[test]
fn the_export_dialog_closes_with_cancel_escape_or_a_click_outside() {
    let mut h = Harness::new("export-close");
    h.keys("ctrl-e");
    let cancel = h.bounds("export-close").center();
    h.click(cancel, 1);
    assert!(!exporting(&mut h), "Cancel");

    h.keys("ctrl-e");
    h.keys("escape");
    assert!(!exporting(&mut h), "Escape");

    h.keys("ctrl-e");
    let panel = h.bounds("export-panel");
    let outside = gpui::point(panel.left() - gpui::px(40.), panel.center().y);
    h.click(outside, 1);
    assert!(!exporting(&mut h), "a click outside");
}
