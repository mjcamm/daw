//! The root view: the menu bar over the launch page or an open project, and
//! the preferences modal.

use crate::launch::{LaunchEvent, LaunchView};
use crate::menu;
use crate::project::{ProjectEvent, ProjectView};
use crate::session::Session;
use crate::theme::{Theme, UI_FONT};
use daw_host::preferences::{Appearance, StoredPreferences};
use daw_host::projects::{self, RecentProjects};
use gpui::{App, Context, Entity, FontWeight, Subscription, Window, actions, div, prelude::*, px};

actions!(app, [Quit, OpenPreferences]);

/// Height of the menu bar across the top of the window.
const MENU_BAR_HEIGHT: f32 = 26.;

/// The quit and preferences shortcuts as the platform writes them.
const QUIT_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘Q"
} else {
    "Ctrl+Q"
};
const PREFERENCES_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘,"
} else {
    "Ctrl+,"
};

enum Screen {
    Launch(Entity<LaunchView>),
    Project(Entity<ProjectView>),
}

pub struct AppView {
    screen: Screen,
    recent: RecentProjects,
    preferences: StoredPreferences,
    file_menu_open: bool,
    preferences_open: bool,
    _screen_events: Subscription,
    _appearance: Subscription,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
            this.apply_theme(window, cx);
        });
        let recent = RecentProjects::load_default();
        let preferences = StoredPreferences::load_default();
        cx.set_global(Theme::for_preference(
            preferences.preferences.appearance,
            window.appearance(),
        ));
        let (screen, events) = launch_screen(&recent, window, cx);
        Self {
            screen,
            recent,
            preferences,
            file_menu_open: false,
            preferences_open: false,
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

    /// Sets the theme from the appearance preference (and, for System, the
    /// window's appearance), and redraws.
    fn apply_theme(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.set_global(Theme::for_preference(
            self.preferences.preferences.appearance,
            window.appearance(),
        ));
        window.refresh();
        cx.notify();
    }

    fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.preferences.appearance = appearance;
        if let Err(e) = self.preferences.save() {
            eprintln!("could not save preferences: {e}");
        }
        self.apply_theme(window, cx);
    }

    fn open_preferences(&mut self, cx: &mut Context<Self>) {
        self.file_menu_open = false;
        self.preferences_open = true;
        cx.notify();
    }

    /// Quits, first saving an open project with unsaved changes (as going
    /// back to the projects page does).
    fn quit(&mut self, cx: &mut Context<Self>) {
        if let Screen::Project(project) = &self.screen {
            let saved = project.update(cx, |project, _| project.save_if_edited());
            if !saved {
                // The project shows why; stay open rather than lose work.
                self.file_menu_open = false;
                return;
            }
        }
        cx.quit();
    }

    /// The menu bar across the top: File.
    fn render_menu_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = theme.hover;
        let open = self.file_menu_open;
        let file = div()
            .id("menu-file")
            .px(px(8.))
            .py(px(2.))
            .rounded(px(4.))
            .when(open, |d| d.bg(theme.control_selected))
            .hover(move |s| s.bg(hover))
            .child("File")
            .on_click(cx.listener(|this, _, _, cx| {
                this.file_menu_open = !this.file_menu_open;
                cx.notify();
            }));
        let dropdown = open.then(|| {
            menu::float(
                menu::panel(theme)
                    .min_w(px(220.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.file_menu_open = false;
                        cx.notify();
                    }))
                    .child(
                        menu::item(
                            theme,
                            "menu-preferences",
                            "Preferences…",
                            PREFERENCES_SHORTCUT,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_preferences(cx))),
                    )
                    .child(menu::separator(theme))
                    .child(
                        menu::item(theme, "menu-quit", "Quit", QUIT_SHORTCUT)
                            .on_click(cx.listener(|this, _, _, cx| this.quit(cx))),
                    ),
            )
        });
        div()
            .h(px(MENU_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(px(6.))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .text_size(px(12.))
            .child(div().flex().flex_col().child(file).children(dropdown))
    }

    /// The preferences modal: a centred panel over a dimmed page.
    fn render_preferences(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.preferences.preferences.appearance;
        let segments = Appearance::ALL.map(|appearance| {
            let selected = appearance == current;
            div()
                .id(appearance.label())
                .flex_1()
                .flex()
                .justify_center()
                .py(px(3.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(theme.control_selected))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_secondary
                })
                .child(appearance.label())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_appearance(appearance, window, cx);
                }))
        });
        let hover = theme.hover;
        let close = div()
            .id("preferences-close")
            .px(px(14.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(theme.control)
            .hover(move |s| s.bg(hover))
            .child("Close")
            .on_click(cx.listener(|this, _, _, cx| {
                this.preferences_open = false;
                cx.notify();
            }));
        let panel = div()
            .occlude()
            .w(px(380.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(18.))
            .rounded(px(10.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_lg()
            .text_size(px(13.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Preferences"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        div()
                            .w(px(90.))
                            .text_color(theme.text_secondary)
                            .child("Appearance"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .p(px(2.))
                            .gap(px(2.))
                            .rounded(px(7.))
                            .bg(theme.control)
                            .text_size(px(12.))
                            .children(segments),
                    ),
            )
            .child(div().flex().justify_end().child(close));
        div()
            .id("preferences-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.scrim)
            .occlude()
            // Clicking outside the panel closes it.
            .on_click(cx.listener(|this, _, _, cx| {
                this.preferences_open = false;
                cx.notify();
            }))
            .child(panel)
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
            .relative()
            .flex()
            .flex_col()
            .bg(theme.window)
            .text_color(theme.text)
            .font_family(UI_FONT)
            .text_size(px(12.))
            .on_action(cx.listener(|this, _: &Quit, _, cx| this.quit(cx)))
            .on_action(cx.listener(|this, _: &OpenPreferences, _, cx| this.open_preferences(cx)))
            .child(self.render_menu_bar(&theme, cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(content))
            .when(self.preferences_open, |d| {
                d.child(self.render_preferences(&theme, cx))
            })
    }
}

/// Quits from anywhere, even when no view has focus. The root view handles
/// the action first when it can, saving an open project.
pub fn register_global_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
}
