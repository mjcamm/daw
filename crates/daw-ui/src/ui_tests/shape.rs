//! The shape panel on the Signals page (`docs/design/curve-editor.md`).

use super::Harness;
use crate::curve_editor::CurveArea;
use crate::project::Page;
use daw_engine::{
    Command, Curve, CurvePoint, Edit, FIRST_NODE, HitMode, NotesSource, PointKind, Signal,
    SignalId, SignalSource, Ticks,
};
use gpui::{Pixels, Point, point, px};

const KICK: SignalId = SignalId(1);
const DRAWN: SignalId = SignalId(2);

/// The starter song with a notes signal from its node, a 1-bar window
/// (16 cells on the 1/16 grid) and a triangle, and a drawn signal, on the
/// Signals page.
fn signals_page(name: &str) -> Harness {
    let mut h = Harness::new(name);
    h.view.update(h.cx, |view, _| {
        let source = NotesSource {
            window: Ticks(3840),
            shape: Curve {
                points: vec![
                    CurvePoint::corner(0., 0.5),
                    CurvePoint::corner(0.5, 1.),
                    CurvePoint::corner(1., 0.5),
                ],
            },
            ..NotesSource::new(FIRST_NODE)
        };
        for (id, name, source) in [
            (KICK, "Kick", SignalSource::Notes(source)),
            (DRAWN, "Sweep", SignalSource::Drawn(Vec::new())),
        ] {
            view.session.apply_or_panic(Command::Edit(Edit::AddSignal {
                id,
                signal: Signal {
                    name: name.into(),
                    color: 2,
                    source,
                },
                listeners: Vec::new(),
            }));
        }
        view.page = Page::Signals;
    });
    h.redraw_all();
    h
}

fn row(h: &mut Harness, id: SignalId) -> Point<Pixels> {
    let name: &'static str = Box::leak(format!("signal-row-{}", id.0).into_boxed_str());
    h.bounds(name).center()
}

/// Opens the kick's shape, already slid up.
fn open_shape(h: &mut Harness) {
    let at = row(h, KICK);
    h.click(at, 1);
    h.view.update(h.cx, |view, _| {
        if let Some(editor) = &mut view.shape_editor {
            editor.opened -= std::time::Duration::from_secs(1);
        }
    });
    h.redraw_all();
    h.redraw();
}

fn shape(h: &mut Harness) -> NotesSource {
    let snapshot = h.snapshot();
    match &snapshot
        .signals
        .iter()
        .find(|s| s.id == KICK)
        .expect("the kick signal")
        .source
    {
        SignalSource::Notes(source) => source.clone(),
        SignalSource::Drawn(_) => panic!("a notes signal"),
    }
}

/// Where the curve position `at`, `value` is on screen.
fn on_curve(h: &mut Harness, at: f64, value: f64) -> Point<Pixels> {
    let area = CurveArea::new(h.bounds("shape-curve"));
    let (x, y) = area.screen_at(at, value);
    point(px(x), px(y))
}

/// docs/behaviours.md: "Click a notes signal opens its shape in the bottom
/// panel ...".
#[test]
fn clicking_a_notes_signal_opens_its_shape_and_a_drawn_one_closes_it() {
    let mut h = signals_page("shape-open");
    assert!(h.view.read_with(h.cx, |v, _| v.shape_editor.is_none()));
    open_shape(&mut h);
    assert!(h.view.read_with(h.cx, |v, _| v.shape_editor.is_some()));
    assert!(
        h.cx.debug_bounds("shape-curve").is_some(),
        "the panel shows"
    );

    let close = h.bounds("shape-close").center();
    h.click(close, 1);
    assert!(h.view.read_with(h.cx, |v, _| v.shape_editor.is_none()));

    open_shape(&mut h);
    let drawn = row(&mut h, DRAWN);
    h.click(drawn, 1);
    assert!(
        h.view.read_with(h.cx, |v, _| v.shape_editor.is_none()),
        "a drawn signal has no shape panel"
    );
}

/// docs/behaviours.md: "... double-click adds a point, double-click a
/// point removes it ...".
#[test]
fn double_clicks_add_and_remove_points_on_the_grid() {
    let mut h = signals_page("shape-points");
    open_shape(&mut h);
    // Near a quarter of the way, three quarters up: snapped to both.
    let at = on_curve(&mut h, 0.26, 0.74);
    h.click(at, 2);
    let points = shape(&mut h).shape.points;
    assert_eq!(points.len(), 4);
    assert_eq!((points[1].at, points[1].value), (0.25, 0.75));
    assert_eq!(
        h.view.read_with(h.cx, |v, _| v
            .shape_editor
            .as_ref()
            .and_then(|e| e.selected)),
        Some(1),
        "the new point is selected"
    );

    let at = on_curve(&mut h, 0.25, 0.75);
    h.click(at, 2);
    assert_eq!(shape(&mut h).shape.points.len(), 3, "removed");

    let end = on_curve(&mut h, 1., 0.5);
    h.click(end, 2);
    assert_eq!(shape(&mut h).shape.points.len(), 3, "the ends stay");
}

/// docs/behaviours.md: "... drag a point to move it; one undo step ...".
#[test]
fn dragging_a_point_moves_it_heard_as_it_moves_and_undoes_in_one_step() {
    let mut h = signals_page("shape-drag");
    open_shape(&mut h);
    let from = on_curve(&mut h, 0.5, 1.);
    let to = on_curve(&mut h, 0.75, 0.25);
    h.press(from);
    let middle = on_curve(&mut h, 0.625, 0.5);
    h.move_held(middle);
    let heard = h.view.read_with(h.cx, |v, _| v.drag_edit());
    assert!(
        matches!(heard, Some(Edit::SetNotesSource { id: KICK, .. })),
        "heard while dragging"
    );
    h.move_held(to);
    h.release(to);
    let p = shape(&mut h).shape.points[1];
    assert_eq!((p.at, p.value), (0.75, 0.25));
    h.keys("ctrl-z");
    let p = shape(&mut h).shape.points[1];
    assert_eq!((p.at, p.value), (0.5, 1.), "one undo step");
}

/// docs/behaviours.md: "Corner | Smooth | Broken for the selected point;
/// drag a handle to bend ...".
#[test]
fn a_selected_point_turns_smooth_or_broken_and_its_handles_bend_the_curve() {
    let mut h = signals_page("shape-kinds");
    open_shape(&mut h);
    let peak = on_curve(&mut h, 0.5, 1.);
    h.click(peak, 1);
    let smooth = h.bounds("shape-kind-1").center();
    h.click(smooth, 1);
    let p = shape(&mut h).shape.points[1];
    assert_eq!(p.kind, PointKind::Smooth);
    assert!(p.handle_out.at > 0., "handles pulled out");

    // Drag the out-handle down: a Smooth point's in-handle swings up.
    let handle = on_curve(&mut h, 0.5 + p.handle_out.at, 1. + p.handle_out.value);
    let down = point(handle.x, handle.y + px(30.));
    h.drag(handle, down, 3);
    let bent = shape(&mut h).shape.points[1];
    assert!(
        bent.handle_out.value < 0. && bent.handle_in.value > 0.,
        "{bent:?}"
    );

    let broken = h.bounds("shape-kind-2").center();
    h.click(broken, 1);
    assert_eq!(shape(&mut h).shape.points[1].kind, PointKind::Broken);
}

/// docs/behaviours.md: "Trigger | Loop; the window ...".
#[test]
fn the_toolbar_sets_the_mode_and_the_window() {
    let mut h = signals_page("shape-toolbar");
    open_shape(&mut h);
    let looped = h.bounds("shape-mode-1").center();
    h.click(looped, 1);
    assert_eq!(shape(&mut h).mode, HitMode::Loop);
    let quarter = h.bounds("shape-window-1").center();
    h.click(quarter, 1);
    assert_eq!(shape(&mut h).window, NotesSource::WINDOWS[1]);
}

/// docs/behaviours.md: "A stamp card fills the window once per grid cell;
/// Save as stamp ...; a saved stamp's × deletes it".
#[test]
fn stamps_fill_the_window_and_saved_ones_can_be_deleted() {
    let mut h = signals_page("shape-stamps");
    open_shape(&mut h);
    // A 1/4 grid: four ramps across the bar.
    h.view.update(h.cx, |v, _| {
        v.shape_grid.grid = crate::block_edit::Grid::Quarter;
    });
    h.redraw_all();
    let ramp = h.bounds("stamp-0").center();
    h.click(ramp, 1);
    let ramps = crate::curve_edit::stamp(&Curve::line(0., 1.), 4);
    assert_eq!(shape(&mut h).shape, ramps);

    let save = h.bounds("shape-save-stamp").center();
    h.click(save, 1);
    let saved = h.view.read_with(h.cx, |v, _| v.saved_stamps.clone());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, "Stamp 1");
    assert_eq!(saved[0].curve, ramps);

    h.redraw_all();
    let delete = h.bounds("stamp-delete-0").center();
    h.click(delete, 1);
    assert!(h.view.read_with(h.cx, |v, _| v.saved_stamps.is_empty()));
    assert_eq!(
        shape(&mut h).shape,
        ramps,
        "deleting a stamp leaves the song"
    );
}
