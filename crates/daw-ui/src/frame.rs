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
//! Each timeline page (Arrange, Parameters, later Signals) keeps its own
//! zoom and scroll position.

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

/// A page with a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeline {
    Arrange,
    Parameters,
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

/// A drag on a timeline page's frame: a scrollbar thumb or a zoom slider.
#[derive(Debug, Clone, Copy)]
pub enum FrameDrag {
    Scroll(Timeline, ScrollDrag),
    Zoom(Timeline, ZoomDrag),
}

impl ProjectView {
    pub(crate) fn timeline(&self, timeline: Timeline) -> &TimelineView {
        match timeline {
            Timeline::Arrange => &self.arrange_view,
            Timeline::Parameters => &self.parameters_view,
        }
    }

    fn timeline_mut(&mut self, timeline: Timeline) -> &mut TimelineView {
        match timeline {
            Timeline::Arrange => &mut self.arrange_view,
            Timeline::Parameters => &mut self.parameters_view,
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
    pub(crate) fn drag_frame(&mut self, position: Point<Pixels>) -> bool {
        let playhead_beats = self.session.position_beats() as f32;
        match self.frame_drag {
            None => false,
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
                let view = self.timeline_mut(timeline);
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
