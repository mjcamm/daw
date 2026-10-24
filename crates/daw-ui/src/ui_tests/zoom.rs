//! Zoom on every timeline: the sliders, and Ctrl+wheel over the page.

use super::Harness;
use crate::frame::Timeline;
use crate::project::Page;
use crate::zoom::{WHEEL_STEP, Zoom};
use daw_engine::FIRST_NODE;
use gpui::{Modifiers, Pixels, Point, point, px};

fn zoom_of(h: &mut Harness, timeline: Timeline) -> Zoom {
    h.view.read_with(h.cx, |v, _| v.timeline(timeline).zoom)
}

/// The beat under `at` on `timeline`'s lanes.
fn beat_at(h: &mut Harness, timeline: Timeline, at: Point<Pixels>) -> f32 {
    h.view.read_with(h.cx, |v, _| {
        let view = v.timeline(timeline);
        let scrolled = -view.x.offset().x / px(1.);
        ((at.x - view.x.bounds().left()) / px(1.) + scrolled) / view.zoom.pixels_per_beat
    })
}

/// Shows `timeline`'s page (or panel) and returns a point over its lanes.
fn show(h: &mut Harness, timeline: Timeline) -> Point<Pixels> {
    h.view.update(h.cx, |v, _| {
        v.selected_node = Some(FIRST_NODE);
        match timeline {
            Timeline::Arrange => v.page = Page::Arrange,
            Timeline::Parameters => v.page = Page::Parameters,
            Timeline::Signals => v.page = Page::Signals,
            Timeline::Notes => {
                v.page = Page::Arrange;
                v.open_node_editor(FIRST_NODE, 0.);
                if let Some(editor) = &mut v.note_editor {
                    editor.opened = std::time::Instant::now()
                        .checked_sub(std::time::Duration::from_secs(1))
                        .unwrap_or(editor.opened);
                }
            }
        }
    });
    h.redraw_all();
    h.redraw_all();
    let strip = match timeline {
        Timeline::Notes => h.bounds("loop-strip-notes"),
        _ => h.bounds("loop-strip-main"),
    };
    // Below the ruler, a third of the way along: over the lanes.
    let x = strip.left() + px(300.);
    point(x, strip.bottom() + px(60.))
}

const ALL: [Timeline; 4] = [
    Timeline::Arrange,
    Timeline::Parameters,
    Timeline::Signals,
    Timeline::Notes,
];

/// docs/behaviours.md: "Ctrl+wheel zooms rows and Ctrl+Shift+wheel zooms
/// time on every timeline, about the pointer".
#[test]
fn ctrl_wheel_zooms_rows_and_ctrl_shift_wheel_zooms_time_everywhere() {
    for timeline in ALL {
        let mut h = Harness::new(&format!("wheel-zoom-{timeline:?}"));
        let at = show(&mut h, timeline);
        let before = zoom_of(&mut h, timeline);
        let ctrl = Modifiers::secondary_key();

        h.wheel(at, 1., ctrl);
        let rows = zoom_of(&mut h, timeline);
        assert_eq!(
            rows.row_height,
            (before.row_height * WHEEL_STEP).round(),
            "{timeline:?}: Ctrl+wheel zooms rows"
        );
        assert_eq!(rows.pixels_per_beat, before.pixels_per_beat, "{timeline:?}");

        let beat = beat_at(&mut h, timeline, at);
        let shift = Modifiers {
            shift: true,
            ..ctrl
        };
        h.wheel(at, 1., shift);
        let time = zoom_of(&mut h, timeline);
        assert!(
            (time.pixels_per_beat - before.pixels_per_beat * WHEEL_STEP).abs() < 1e-3,
            "{timeline:?}: Ctrl+Shift+wheel zooms time"
        );
        assert_eq!(time.row_height, rows.row_height, "{timeline:?}");
        let held = beat_at(&mut h, timeline, at);
        assert!(
            (held - beat).abs() < 0.05,
            "{timeline:?}: the beat under the pointer stays ({beat} → {held})"
        );

        h.wheel(at, -1., shift);
        let back = zoom_of(&mut h, timeline);
        assert!(
            (back.pixels_per_beat - before.pixels_per_beat).abs() < 1e-2,
            "{timeline:?}: a notch back"
        );

        h.wheel(at, 1., Modifiers::none());
        assert_eq!(
            zoom_of(&mut h, timeline),
            back,
            "{timeline:?}: a plain wheel scrolls, never zooms"
        );
    }
}
