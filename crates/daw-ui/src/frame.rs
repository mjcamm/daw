//! The frame around a timeline page's scrolling body: a vertical scrollbar
//! down the right with the vertical zoom slider at its bottom end, and a
//! horizontal scrollbar along the bottom with the horizontal zoom slider at
//! its right end.
//!
//! ```text
//! [ body                                  ][v-scroll]
//! [                                       ][        ]
//! [                                       ][v-zoom  ]
//! [ (left column) ][ h-scroll   ][ h-zoom ][corner  ]
//! ```
//!
//! Each timeline page (Arrange, Parameters, Signals) keeps its own
//! zoom and scroll position.

use crate::layout::{MIN_GRID_SPACING, grid_bar_step};
use crate::project::ProjectView;
use crate::scroll::{self, Extent, ScrollDrag};
use crate::slider::Axis;
use crate::theme::Theme;
use crate::zoom::{self, Zoom, ZoomDrag, keep_anchor, zoom_anchor};
use gpui::{
    AnyElement, Bounds, Context, Div, MouseButton, MouseDownEvent, Pixels, Point, ScrollHandle,
    canvas, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;

/// The timeline's vertical grid across `bars` bars: bar lines stronger than
/// the beat lines between them, beat lines only when there is room, bar
/// lines thinned out when zoomed far out.
pub fn grid_lines(zoom: Zoom, bars: u32, theme: &Theme) -> std::iter::Once<gpui::AnyElement> {
    let bar_step = grid_bar_step(zoom.pixels_per_beat);
    let show_beats = zoom.pixels_per_beat >= MIN_GRID_SPACING;
    let (bar_line, beat_line) = (theme.hairline, theme.grid);
    let lines = (0..bars * 4)
        .filter(move |beat| {
            if beat % 4 == 0 {
                (beat / 4) % bar_step == 0
            } else {
                show_beats
            }
        })
        .map(move |beat| crate::paint::Bar {
            x: zoom.beat_to_x(f64::from(beat)),
            y: 0.,
            w: 1.,
            // As tall as any lane area; the canvas clips it.
            h: 100_000.,
            color: if beat % 4 == 0 { bar_line } else { beat_line },
            radius: 0.,
        })
        .collect();
    // One drawing for every line; only those on screen are painted.
    std::iter::once(
        crate::paint::bars(lines)
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .into_any_element(),
    )
}

/// A page with a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeline {
    Arrange,
    Parameters,
    Signals,
    /// The piano roll in the bottom panel.
    Notes,
}

/// A timeline page's view state: zoom, scroll areas and where its
/// scrollbars were drawn.
pub struct TimelineView {
    pub zoom: Zoom,
    /// The sideways-scrolling lanes.
    pub x: ScrollHandle,
    /// The vertically scrolling body.
    pub y: ScrollHandle,
    track_x: Rc<Cell<Bounds<Pixels>>>,
    track_y: Rc<Cell<Bounds<Pixels>>>,
}

impl TimelineView {
    pub fn new(zoom: Zoom) -> Self {
        Self {
            zoom,
            x: ScrollHandle::new(),
            y: ScrollHandle::new(),
            track_x: Rc::default(),
            track_y: Rc::default(),
        }
    }

    /// One scroll area for both axes (the piano roll's grid).
    pub fn shared(zoom: Zoom) -> Self {
        let scroll = ScrollHandle::new();
        Self {
            zoom,
            x: scroll.clone(),
            y: scroll,
            track_x: Rc::default(),
            track_y: Rc::default(),
        }
    }

    pub fn scroll(&self, axis: Axis) -> &ScrollHandle {
        match axis {
            Axis::Horizontal => &self.x,
            Axis::Vertical => &self.y,
        }
    }

    fn track(&self, axis: Axis) -> f32 {
        let bounds = match axis {
            Axis::Horizontal => self.track_x.get(),
            Axis::Vertical => self.track_y.get(),
        };
        match axis {
            Axis::Horizontal => bounds.size.width / px(1.),
            Axis::Vertical => bounds.size.height / px(1.),
        }
    }
}

/// A drag on a timeline page's frame: a scrollbar thumb or a zoom slider;
/// or a nesting map grabbed and pulled.
#[derive(Debug, Clone, Copy)]
pub enum FrameDrag {
    Scroll(Timeline, ScrollDrag),
    Zoom(Timeline, ZoomDrag),
    Pan(MapPan),
}

/// A nesting map grabbed and pulled: Arrange's sideways, the mixer's up and
/// down. The map follows the pointer, like a page pulled by hand.
#[derive(Debug, Clone, Copy)]
pub struct MapPan {
    pub mixer: bool,
    pub origin: Point<Pixels>,
    /// How far the map was scrolled when grabbed.
    pub scrolled: f32,
}

impl MapPan {
    pub fn axis(self) -> Axis {
        if self.mixer {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// How far the map is scrolled with the pointer at `position`, kept
    /// within `0..=max`.
    pub fn scrolled_at(self, position: Point<Pixels>, max: f32) -> f32 {
        let moved = match self.axis() {
            Axis::Horizontal => position.x - self.origin.x,
            Axis::Vertical => position.y - self.origin.y,
        } / px(1.);
        (self.scrolled - moved).clamp(0., max.max(0.))
    }
}

impl ProjectView {
    pub(crate) fn timeline(&self, timeline: Timeline) -> &TimelineView {
        match timeline {
            Timeline::Arrange => &self.arrange_view,
            Timeline::Parameters => &self.parameters_view,
            Timeline::Signals => &self.signals_view,
            Timeline::Notes => &self.notes_view,
        }
    }

    fn timeline_mut(&mut self, timeline: Timeline) -> &mut TimelineView {
        match timeline {
            Timeline::Arrange => &mut self.arrange_view,
            Timeline::Parameters => &mut self.parameters_view,
            Timeline::Signals => &mut self.signals_view,
            Timeline::Notes => &mut self.notes_view,
        }
    }

    /// Frames `body` (a page's scrolling rows) with scrollbars and zoom
    /// sliders. `left_width` is the width of the page's fixed left column,
    /// which the horizontal scrollbar does not run under.
    pub(crate) fn timeline_frame(
        &self,
        timeline: Timeline,
        body: AnyElement,
        left_width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let bar = scroll::THICKNESS;
        let vertical = div()
            .w(px(bar))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.hairline)
            .child(self.scrollbar(timeline, Axis::Vertical, theme, cx))
            .child(self.zoom_slider(timeline, Axis::Vertical, theme, cx));
        let horizontal = div()
            .h(px(bar))
            .flex_none()
            .flex()
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.hairline)
            .child(div().w(px(left_width)).flex_none())
            .child(self.scrollbar(timeline, Axis::Horizontal, theme, cx))
            .child(self.zoom_slider(timeline, Axis::Horizontal, theme, cx))
            .child(div().w(px(bar)).flex_none());
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().flex().flex_col().child(body))
                    .child(vertical),
            )
            .child(horizontal)
    }

    fn scrollbar(
        &self,
        timeline: Timeline,
        axis: Axis,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = self.timeline(timeline);
        let extent = Extent::of(view.scroll(axis), axis);
        let active = matches!(
            self.frame_drag,
            Some(FrameDrag::Scroll(t, drag)) if t == timeline && drag.axis == axis
        );
        let track = match axis {
            Axis::Horizontal => view.track_x.clone(),
            Axis::Vertical => view.track_y.clone(),
        };
        let id = match (timeline, axis) {
            (Timeline::Arrange, Axis::Horizontal) => "arrange-scroll-x",
            (Timeline::Arrange, Axis::Vertical) => "arrange-scroll-y",
            (Timeline::Parameters, Axis::Horizontal) => "parameters-scroll-x",
            (Timeline::Parameters, Axis::Vertical) => "parameters-scroll-y",
            (Timeline::Signals, Axis::Horizontal) => "signals-scroll-x",
            (Timeline::Signals, Axis::Vertical) => "signals-scroll-y",
            (Timeline::Notes, Axis::Horizontal) => "notes-scroll-x",
            (Timeline::Notes, Axis::Vertical) => "notes-scroll-y",
        };
        scroll::scrollbar(id, axis, extent, active, theme)
            .child(
                // Records the track's size, to turn thumb drags into scrolling.
                canvas(move |bounds, _, _| track.set(bounds), |_, (), _, _| {})
                    .absolute()
                    .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    let view = this.timeline(timeline);
                    this.frame_drag = Some(FrameDrag::Scroll(
                        timeline,
                        ScrollDrag {
                            axis,
                            origin: event.position,
                            extent: Extent::of(view.scroll(axis), axis),
                            track: view.track(axis),
                        },
                    ));
                }),
            )
    }

    fn zoom_slider(
        &self,
        timeline: Timeline,
        axis: Axis,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let zoom_axis = match axis {
            Axis::Horizontal => zoom::Axis::Horizontal,
            Axis::Vertical => zoom::Axis::Vertical,
        };
        let active = matches!(
            self.frame_drag,
            Some(FrameDrag::Zoom(t, drag)) if t == timeline && drag.axis == zoom_axis
        );
        let id = match (timeline, axis) {
            (Timeline::Arrange, Axis::Horizontal) => "arrange-zoom-x",
            (Timeline::Arrange, Axis::Vertical) => "arrange-zoom-y",
            (Timeline::Parameters, Axis::Horizontal) => "parameters-zoom-x",
            (Timeline::Parameters, Axis::Vertical) => "parameters-zoom-y",
            (Timeline::Signals, Axis::Horizontal) => "signals-zoom-x",
            (Timeline::Signals, Axis::Vertical) => "signals-zoom-y",
            (Timeline::Notes, Axis::Horizontal) => "notes-zoom-x",
            (Timeline::Notes, Axis::Vertical) => "notes-zoom-y",
        };
        let position = self.timeline(timeline).zoom.position(zoom_axis);
        zoom::slider(id, zoom_axis, position, active, theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.frame_drag = Some(FrameDrag::Zoom(
                    timeline,
                    ZoomDrag {
                        axis: zoom_axis,
                        origin: event.position,
                        original: this.timeline(timeline).zoom,
                    },
                ));
            }),
        )
    }

    /// Follows a frame drag (scrollbar or zoom); false if none is in
    /// progress. Horizontal zoom holds the playhead still if it is in view,
    /// otherwise the middle of the view.
    /// The scroll area of a nesting map.
    fn map_scroll(&self, mixer: bool) -> &ScrollHandle {
        if mixer {
            &self.mixer_view.map_y
        } else {
            &self.node_map_x
        }
    }

    /// Grabs a nesting map to pull it (not from its chevrons, which stop
    /// the press).
    pub(crate) fn grab_map(&mut self, mixer: bool, origin: Point<Pixels>) {
        let pan = MapPan {
            mixer,
            origin,
            scrolled: 0.,
        };
        let scrolled = Extent::of(self.map_scroll(mixer), pan.axis()).scrolled;
        self.frame_drag = Some(FrameDrag::Pan(MapPan { scrolled, ..pan }));
    }

    pub(crate) fn drag_frame(&mut self, position: Point<Pixels>) -> bool {
        let playhead_beats = self.session.position_beats() as f32;
        // The piano roll's time starts at its clip.
        let clip_start = self.note_editor_start().unwrap_or(0.);
        match self.frame_drag {
            None => false,
            Some(FrameDrag::Pan(pan)) => {
                let handle = self.map_scroll(pan.mixer);
                let extent = Extent::of(handle, pan.axis());
                let max = extent.content - extent.view;
                scroll::scroll_to(handle, pan.axis(), pan.scrolled_at(position, max));
                true
            }
            Some(FrameDrag::Scroll(timeline, drag)) => {
                let view = self.timeline(timeline);
                scroll::scroll_to(
                    view.scroll(drag.axis),
                    drag.axis,
                    drag.scrolled_at(position),
                );
                true
            }
            Some(FrameDrag::Zoom(timeline, drag)) => {
                let zoom = drag.zoom_at(position);
                let playhead_beats = if timeline == Timeline::Notes {
                    playhead_beats - clip_start
                } else {
                    playhead_beats
                };
                let view = self.timeline_mut(timeline);
                // The piano roll's keys zoom about the middle of the view.
                let old_key = view.zoom.row_height;
                if timeline == Timeline::Notes && zoom.row_height != old_key {
                    let extent = Extent::of(&view.y, Axis::Vertical);
                    let middle = extent.view / 2.;
                    let scrolled = keep_anchor(extent.scrolled, middle, old_key, zoom.row_height);
                    scroll::scroll_to(&view.y, Axis::Vertical, scrolled);
                }
                let old = view.zoom.pixels_per_beat;
                if zoom.pixels_per_beat != old {
                    let extent = Extent::of(&view.x, Axis::Horizontal);
                    let anchor = zoom_anchor(playhead_beats * old, extent.scrolled, extent.view);
                    let scrolled = keep_anchor(extent.scrolled, anchor, old, zoom.pixels_per_beat);
                    scroll::scroll_to(&view.x, Axis::Horizontal, scrolled);
                }
                view.zoom = zoom;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    #[test]
    fn a_pulled_map_follows_the_pointer_within_its_range() {
        let pan = MapPan {
            mixer: false,
            origin: point(px(100.), px(50.)),
            scrolled: 20.,
        };
        // Pulling left reveals what is further right.
        assert_eq!(pan.scrolled_at(point(px(90.), px(80.)), 100.), 30.);
        assert_eq!(pan.scrolled_at(point(px(150.), px(50.)), 100.), 0.);
        assert_eq!(pan.scrolled_at(point(px(-200.), px(50.)), 100.), 100.);
        let mixer = MapPan { mixer: true, ..pan };
        assert_eq!(mixer.scrolled_at(point(px(100.), px(40.)), 100.), 30.);
    }
}
