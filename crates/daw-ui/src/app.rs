//! The root view: header and the current page.

use crate::session::Session;
use crate::theme::{Theme, UI_FONT};
use daw_engine::{Command, LoopRange};
use gpui::{
    Bounds, Context, FocusHandle, Pixels, Subscription, Window, actions, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;

actions!(daw, [TogglePlay]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Arrange,
    Parameters,
    Signals,
    Mixer,
}

impl Page {
    pub const ALL: [Self; 4] = [Self::Arrange, Self::Parameters, Self::Signals, Self::Mixer];

    pub fn label(self) -> &'static str {
        match self {
            Self::Arrange => "Arrange",
            Self::Parameters => "Parameters",
            Self::Signals => "Signals",
            Self::Mixer => "Mixer",
        }
    }
}

pub struct AppView {
    pub(crate) session: Session,
    pub(crate) page: Page,
    /// The loop region to restore when looping is switched back on.
    pub(crate) saved_loop: Option<LoopRange>,
    /// Where the timeline ruler was last drawn, for turning clicks into
    /// song positions.
    pub(crate) ruler_bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    _appearance: Subscription,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Theme::for_appearance(window.appearance()));
            cx.notify();
        });
        let session = Session::start();
        let saved_loop = session.snapshot().loop_range;
        Self {
            session,
            page: Page::Arrange,
            saved_loop,
            ruler_bounds: Rc::default(),
            focus,
            _appearance: appearance,
        }
    }

    pub(crate) fn toggle_play(&mut self) {
        let command = if self.session.snapshot().playing {
            Command::Stop
        } else {
            Command::Play
        };
        self.session.apply(command);
    }

    pub(crate) fn toggle_loop(&mut self) {
        let current = self.session.snapshot().loop_range;
        if current.is_some() {
            self.saved_loop = current;
            self.session.apply(Command::SetLoop(None));
        } else if let Some(range) = self.saved_loop {
            self.session.apply(Command::SetLoop(Some(range)));
        }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Redraw every frame: the playhead and meters move continuously.
        window.request_animation_frame();
        self.session.tick();
        let theme = Theme::get(cx);

        let page = match self.page {
            Page::Arrange => self.render_arrange(&theme, cx).into_any_element(),
            other => placeholder(&theme, other).into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window)
            .text_color(theme.text)
            .font_family(UI_FONT)
            .text_size(px(12.))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _, _| this.toggle_play()))
            .child(self.render_header(&theme, cx))
            .children(self.session.audio_error.as_ref().map(|e| {
                div()
                    .px(px(18.))
                    .py(px(6.))
                    .bg(theme.warning)
                    .text_color(theme.on_warning)
                    .child(format!("No audio output: {e}"))
            }))
            .child(page)
    }
}

fn placeholder(theme: &Theme, page: Page) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.text_tertiary)
        .text_size(px(13.))
        .child(format!("{} page: not built yet", page.label()))
}
