//! Drawing the loop strip on each ruler and turning presses on it into loop
//! drags. The rules are in `loop_strip.rs`.

use crate::block_edit::Snap;
use crate::clip_edit::bar_beat;
use crate::loop_strip::{Grab, HANDLE_WIDTH, LoopDrag, LoopRuler, STRIP_HEIGHT, dragged, grab_at};
use crate::project::ProjectView;
use crate::theme::{MONO_FONT, Theme};
use daw_engine::{Command, Ticks};
use gpui::{
    Bounds, Context, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, canvas, div,
    prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;

/// Where each ruler's strip was last drawn, for turning presses into song
/// positions.
#[derive(Debug, Default)]
pub struct StripBounds {
    main: Rc<Cell<Bounds<Pixels>>>,
    notes: Rc<Cell<Bounds<Pixels>>>,
    audio: Rc<Cell<Bounds<Pixels>>>,
}

impl StripBounds {
    fn of(&self, ruler: LoopRuler) -> &Rc<Cell<Bounds<Pixels>>> {
        match ruler {
            LoopRuler::Main => &self.main,
            LoopRuler::Notes => &self.notes,
            LoopRuler::Audio => &self.audio,
        }
    }
}

impl ProjectView {
    /// The loop strip along the top of a ruler whose content starts at the
    /// song start and is `width` pixels wide.
    pub(crate) fn render_loop_strip(
        &self,
        ruler: LoopRuler,
        pixels_per_beat: f32,
        width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let drag = self.loop_drag.filter(|d| d.ruler == ruler);
        let shown = drag.map_or(snapshot.loop_range, |d| d.shown());
        // A fresh loop plays once released; a moved end keeps the state.
        let on = snapshot.loop_enabled
            || drag.is_some_and(|d| d.grab == Grab::New && d.preview.is_some());

        let region = shown.map(|range| {
            let start = range.start.as_beats() as f32 * pixels_per_beat;
            let end = range.end.as_beats() as f32 * pixels_per_beat;
            let span = (end - start).max(2.);
            let handle_width = HANDLE_WIDTH.min(span / 2.);
            let (mut fill, handle) = if on {
                (theme.accent, theme.accent)
            } else {
                (theme.text_secondary, theme.text_tertiary)
            };
            fill.a = if on { 0.28 } else { 0.16 };
            let handle_at = |left: bool| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(px(handle_width))
                    .bg(handle)
                    .cursor_ew_resize()
                    .when(left, |d| d.left_0().rounded_l(px(3.)))
                    .when(!left, |d| d.right_0().rounded_r(px(3.)))
            };
            // While dragging, where the loop runs (its end is where the
            // next bar or beat starts).
            let label = drag.map(|_| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(handle_width + 4.))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .font_family(MONO_FONT)
                    .text_size(px(10.))
                    .text_color(theme.text)
                    .child(format!(
                        "{} – {}",
                        bar_beat(range.start),
                        bar_beat(range.end)
                    ))
            });
            div()
                .absolute()
                .top_0()
                .left(px(start))
                .w(px(span))
                .h(px(STRIP_HEIGHT))
                .rounded(px(3.))
                .bg(fill)
                .child(handle_at(true))
                .child(handle_at(false))
                .children(label)
        });

        let cell = self.loop_strip_bounds.of(ruler).clone();
        let (id, selector) = match ruler {
            LoopRuler::Main => ("loop-strip-main", "loop-strip-main"),
            LoopRuler::Notes => ("loop-strip-notes", "loop-strip-notes"),
            LoopRuler::Audio => ("loop-strip-audio", "loop-strip-audio"),
        };
        div()
            .id(id)
            .debug_selector(move || selector.into())
            .absolute()
            .top_0()
            .left_0()
            .w(px(width))
            .h(px(STRIP_HEIGHT))
            .child(
                canvas(move |bounds, _, _| cell.set(bounds), |_, (), _, _| {})
                    .absolute()
                    .size_full(),
            )
            .children(region)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.start_loop_drag(ruler, event.position, pixels_per_beat);
                    // Not a locate: the ruler below this strip does that.
                    cx.stop_propagation();
                }),
            )
    }

    /// A press on a ruler below its loop strip: the playhead goes there,
    /// to the nearest quarter beat. Every ruler (Arrange, Parameters,
    /// Signals, piano roll, audio panel) places it this way; their content
    /// starts at the song start, where the loop strip does.
    pub(crate) fn locate_on_ruler(
        &mut self,
        ruler: LoopRuler,
        at: Point<Pixels>,
        pixels_per_beat: f32,
    ) {
        let origin = self.loop_strip_bounds.of(ruler).get().origin.x;
        let x = (at.x - origin) / px(1.);
        self.session.apply(Command::Locate {
            position: crate::layout::x_to_snapped_position(x, pixels_per_beat),
        });
    }

    fn start_loop_drag(&mut self, ruler: LoopRuler, at: Point<Pixels>, pixels_per_beat: f32) {
        let origin = self.loop_strip_bounds.of(ruler).get().origin.x;
        let x = (at.x - origin) / px(1.);
        let original = self.session.snapshot().loop_range;
        let grab = grab_at(original, x, pixels_per_beat);
        self.loop_drag = Some(LoopDrag {
            ruler,
            grab,
            press: Ticks::from_beats_f64(f64::from(x / pixels_per_beat).max(0.)),
            pixels_per_beat,
            original,
            preview: if grab == Grab::New { None } else { original },
        });
    }

    /// Follows a loop drag; false if there is none.
    pub(crate) fn drag_loop(&mut self, at: Point<Pixels>, modifiers: Modifiers) -> bool {
        let Some(drag) = self.loop_drag else {
            return false;
        };
        let snap: Snap = match drag.ruler {
            LoopRuler::Notes => self.note_grid,
            LoopRuler::Main | LoopRuler::Audio => self.snap,
        };
        let snap = if modifiers.alt { snap.inverted() } else { snap };
        let origin = self.loop_strip_bounds.of(drag.ruler).get().origin.x;
        let beat = f64::from((at.x - origin) / px(1.) / drag.pixels_per_beat);
        let preview = dragged(
            drag.grab,
            drag.press,
            Ticks::from_beats_f64(beat),
            drag.original,
            snap,
        );
        self.loop_drag = Some(LoopDrag { preview, ..drag });
        true
    }

    /// Sets the loop a drag made. A fresh loop plays; moving an end keeps
    /// looping on or off as it was.
    pub(crate) fn finish_loop_drag(&mut self) {
        let Some(drag) = self.loop_drag.take() else {
            return;
        };
        let Some(range) = drag
            .preview
            .filter(|r| Some(*r) != drag.original || drag.grab == Grab::New)
        else {
            return;
        };
        let was_on = self.session.snapshot().loop_enabled;
        self.session.apply(Command::SetLoop(Some(range)));
        if drag.grab != Grab::New && !was_on {
            self.session.apply(Command::SetLoopEnabled(false));
        }
    }
}
