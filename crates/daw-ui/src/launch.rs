//! The launch page: start a new project, open one, or pick a recent one.

use crate::layout::format_age;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;
use daw_host::projects::RecentProject;
use gpui::{
    Context, Entity, EventEmitter, Focusable, FontWeight, PathPromptOptions, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub enum LaunchEvent {
    /// Open the project in this folder.
    Open(PathBuf),
    /// Create a new project with this name.
    Create(String),
}

pub struct LaunchView {
    recent: Vec<RecentProject>,
    /// The name field, while a new project is being named.
    new_name: Option<(Entity<TextInput>, Subscription)>,
    error: Option<String>,
}

impl EventEmitter<LaunchEvent> for LaunchView {}

impl LaunchView {
    pub fn new(recent: Vec<RecentProject>, error: Option<String>) -> Self {
        Self {
            recent,
            new_name: None,
            error,
        }
    }

    pub fn show_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    fn start_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("", "Song name", cx));
        window.focus(&input.read(cx).focus_handle(cx));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, _, cx| {
                match event {
                    TextInputEvent::Confirm(name) => this.create(name.clone(), cx),
                    TextInputEvent::Cancel => this.new_name = None,
                    TextInputEvent::Changed(_) => return,
                }
                cx.notify();
            });
        self.new_name = Some((input, subscription));
        self.error = None;
    }

    fn create(&mut self, name: String, cx: &mut Context<Self>) {
        if name.trim().is_empty() {
            self.error = Some("Give the song a name first.".to_owned());
            return;
        }
        cx.emit(LaunchEvent::Create(name.trim().to_owned()));
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(folder) = paths.into_iter().next()
            {
                let _ = this.update(cx, |_, cx| cx.emit(LaunchEvent::Open(folder)));
            }
        })
        .detach();
    }
}

impl Render for LaunchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        let button = |id: &'static str, label: &'static str, primary: bool| {
            let hover = theme.hover;
            div()
                .id(id)
                .px(px(14.))
                .py(px(7.))
                .rounded(px(7.))
                .text_size(px(13.))
                .bg(if primary { theme.accent } else { theme.control })
                .text_color(if primary { theme.on_accent } else { theme.text })
                .when(!primary, |d| d.hover(move |s| s.bg(hover)))
                .child(label)
        };

        let new_project: gpui::AnyElement = match &self.new_name {
            Some((input, _)) => {
                let field = input.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().text_size(px(13.)).child(input.clone()))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(button("create", "Create", true).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let name = field.read(cx).text().to_owned();
                                    this.create(name, cx);
                                },
                            )))
                            .child(
                                button("cancel", "Cancel", false)
                                    .on_click(cx.listener(|this, _, _, _| this.new_name = None)),
                            ),
                    )
                    .into_any_element()
            }
            None => button("new", "New project", true)
                .on_click(cx.listener(|this, _, window, cx| this.start_new_project(window, cx)))
                .into_any_element(),
        };

        let actions = div()
            .w(px(320.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(32.))
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .child(
                div()
                    .text_size(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("DAW"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.text_secondary)
                    .mb(px(16.))
                    .child("Draw a movement once, use it everywhere."),
            )
            .child(new_project)
            .child(
                button("open", "Open…", false)
                    .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
            )
            .children(self.error.clone().map(|e| {
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .text_color(theme.record)
                    .child(e)
            }));

        let hover = theme.hover;
        let rows = self.recent.iter().enumerate().map(|(i, project)| {
            let folder = project.folder.clone();
            div()
                .id(("recent", i))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(7.))
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(LaunchEvent::Open(folder.clone()));
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(13.)).child(project.name.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text_secondary)
                                .child(SharedString::from(project.folder.display().to_string())),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_tertiary)
                        .child(format_age(now, project.last_opened)),
                )
        });

        let recent = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(32.))
            .child(
                div()
                    .px(px(12.))
                    .pb(px(8.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("RECENT"),
            )
            .children(rows)
            .when(self.recent.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(12.))
                        .text_color(theme.text_tertiary)
                        .child("No recent projects yet."),
                )
            });

        div().size_full().flex().child(actions).child(recent)
    }
}
