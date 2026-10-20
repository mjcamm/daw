//! Panes: parts of the project view that GPUI reuses from the last frame
//! until they change (`docs/design/ui-speed.md`).
//!
//! All state stays in `ProjectView`; a pane only marks out a part of the
//! window for caching and renders it through `ProjectView::render_pane`.
//! The project view tells a pane to redraw (`ProjectView::invalidate`) when
//! the song changes and on input; a pane asks for its own next frame while
//! one of its animations runs. What moves every frame is drawn by the live
//! layer (`live.rs`), not by panes.

use crate::live::PaneKind;
use crate::project::ProjectView;
use gpui::{Context, IntoElement, Render, WeakEntity, Window, div, prelude::*};

pub struct Pane {
    pub project: WeakEntity<ProjectView>,
    pub kind: PaneKind,
    /// Whether this render is the follow-up to the last one (below).
    settling: bool,
}

impl Pane {
    pub fn new(project: WeakEntity<ProjectView>, kind: PaneKind) -> Self {
        Self {
            project,
            kind,
            settling: false,
        }
    }
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        let Some(project) = self.project.upgrade() else {
            return div().into_any_element();
        };
        let (element, animating) = project.update(cx, |project, cx| {
            project.animating.set(false);
            project.live.clear(kind);
            let element = project.render_pane(kind, window, cx);
            (element, project.animating.get())
        });
        // Some parts measure themselves as they are laid out and use the
        // size the next time they draw (fader lengths, scrollbar thumbs).
        // So a pane that redraws draws once more next frame, with what
        // this frame measured. Animations keep asking until they end.
        // (`request_animation_frame` redraws the view being rendered: this
        // pane.)
        let settle = !self.settling;
        self.settling = settle;
        if settle || animating {
            window.request_animation_frame();
        }
        let content = match kind {
            // The inspector sits beside the page and fills its height.
            PaneKind::Inspector => div().size_full().flex(),
            PaneKind::Main | PaneKind::Bottom => div().size_full().flex().flex_col(),
        };
        content.child(element).into_any_element()
    }
}
