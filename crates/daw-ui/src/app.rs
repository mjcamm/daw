//! The root view: the launch page, or an open project.

use crate::launch::{LaunchEvent, LaunchView};
use crate::project::{ProjectEvent, ProjectView};
use crate::session::Session;
use crate::theme::{Theme, UI_FONT};
use daw_host::projects::{self, RecentProjects};
use gpui::{Context, Entity, Subscription, Window, div, prelude::*, px};

enum Screen {
    Launch(Entity<LaunchView>),
    Project(Entity<ProjectView>),
}

pub struct AppView {
    screen: Screen,
    recent: RecentProjects,
    _screen_events: Subscription,
    _appearance: Subscription,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Theme::for_appearance(window.appearance()));
            cx.notify();
        });
        let recent = RecentProjects::load_default();
        let (screen, events) = launch_screen(&recent, window, cx);
        Self {
            screen,
            recent,
            _screen_events: events,
            _appearance: appearance,
        }
    }

    fn show_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (screen, events) = launch_screen(&self.recent, window, cx);
        self.screen = screen;
        self._screen_events = events;
        window.set_window_title("DAW");
        cx.notify();
    }

    fn handle_launch(&mut self, event: &LaunchEvent, window: &mut Window, cx: &mut Context<Self>) {
        let result = match event {
            LaunchEvent::Open(folder) => Session::open(folder.clone()),
            LaunchEvent::Create(name) => {
                projects::create_folder(&projects::default_projects_dir(), name)
                    .map_err(|e| format!("Could not create the project folder: {e}"))
                    .and_then(|folder| Session::create(folder, name))
            }
        };
        match result {
            Ok(session) => self.show_project(session, window, cx),
            Err(error) => {
                if let Screen::Launch(launch) = &self.screen {
                    launch.update(cx, |launch, cx| launch.show_error(error, cx));
                }
            }
        }
    }

    fn show_project(&mut self, session: Session, window: &mut Window, cx: &mut Context<Self>) {
        self.remember(&session);
        let view = cx.new(|cx| ProjectView::new(session, window, cx));
        self._screen_events = cx.subscribe_in(
            &view,
            window,
            |this, view, event: &ProjectEvent, window, cx| match event {
                ProjectEvent::Close => {
                    let session = view.read(cx).session();
                    let (folder, name) = (
                        session.folder().to_owned(),
                        session.snapshot().song_name.clone(),
                    );
                    if let Err(e) = this.recent.touch(&folder, &name) {
                        eprintln!("could not update recent projects: {e}");
                    }
                    this.show_launch(window, cx);
                }
            },
        );
        self.screen = Screen::Project(view);
        cx.notify();
    }

    /// Puts a project at the top of the recent list.
    fn remember(&mut self, session: &Session) {
        let name = session.snapshot().song_name.clone();
        if let Err(e) = self.recent.touch(session.folder(), &name) {
            eprintln!("could not update recent projects: {e}");
        }
    }
}

fn launch_screen(
    recent: &RecentProjects,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> (Screen, Subscription) {
    let projects = recent.existing().cloned().collect();
    let launch = cx.new(|_| LaunchView::new(projects, None));
    let events = cx.subscribe_in(
        &launch,
        window,
        |this, _, event: &LaunchEvent, window, cx| {
            this.handle_launch(event, window, cx);
        },
    );
    (Screen::Launch(launch), events)
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let content = match &self.screen {
            Screen::Launch(view) => view.clone().into_any_element(),
            Screen::Project(view) => view.clone().into_any_element(),
        };
        div()
            .size_full()
            .bg(theme.window)
            .text_color(theme.text)
            .font_family(UI_FONT)
            .text_size(px(12.))
            .child(content)
    }
}
