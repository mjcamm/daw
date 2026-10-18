//! The root view: the menu bar over the launch page or an open project, and
//! the preferences modal (Audio and Appearance; see
//! `docs/design/audio-settings.md`).

use crate::icons::{Icon, icon};
use crate::launch::{LaunchEvent, LaunchView};
use crate::menu;
use crate::project::{ProjectEvent, ProjectView};
use crate::session::Session;
use crate::theme::{Theme, UI_FONT};
use crate::tooltip;
use daw_host::preferences::{Appearance, AudioSettings, StoredPreferences};
use daw_host::projects::{self, RecentProjects};
use daw_host::{DeviceChoices, device_choices, drivers};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Subscription, Window, actions, div, prelude::*,
    px,
};

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

/// A section of the preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Audio,
    Appearance,
}

/// A drop-down list in the audio settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Driver,
    Device,
    Rate,
    Buffer,
    Threads,
}

enum Screen {
    Launch(Entity<LaunchView>),
    Project(Entity<ProjectView>),
}

pub struct AppView {
    screen: Screen,
    recent: RecentProjects,
    preferences: StoredPreferences,
    file_menu_open: bool,
    /// The current press closed the File menu by landing outside it (see
    /// `ProjectView::toggle_menu`).
    file_menu_dismissed: bool,
    preferences_open: bool,
    section: Section,
    /// The audio drop-down that is open, if any.
    open_choice: Option<Choice>,
    /// What the audio settings can choose; read when the preferences open
    /// and when the driver or device changes (asking the system is slow).
    choices: DeviceChoices,
    drivers: Vec<String>,
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
            file_menu_dismissed: false,
            preferences_open: false,
            section: Section::Audio,
            open_choice: None,
            choices: DeviceChoices::default(),
            drivers: Vec::new(),
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
            LaunchEvent::Open(folder) => {
                Session::open(folder.clone(), &self.preferences.preferences.audio)
            }
            LaunchEvent::Create(name) => {
                projects::create_folder(&projects::default_projects_dir(), name)
                    .map_err(|e| format!("Could not create the project folder: {e}"))
                    .and_then(|folder| {
                        Session::create(folder, name, &self.preferences.preferences.audio)
                    })
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
        self.open_choice = None;
        self.drivers = drivers();
        self.choices = device_choices(&self.preferences.preferences.audio);
        cx.notify();
    }

    /// Saves new audio settings and switches the open project to them.
    fn set_audio(&mut self, settings: AudioSettings, cx: &mut Context<Self>) {
        let old = std::mem::replace(&mut self.preferences.preferences.audio, settings.clone());
        if old.driver != settings.driver
            || old.device != settings.device
            || old.sample_rate != settings.sample_rate
        {
            self.choices = device_choices(&settings);
        }
        if let Err(e) = self.preferences.save() {
            eprintln!("could not save preferences: {e}");
        }
        self.open_choice = None;
        if let Screen::Project(project) = &self.screen {
            project.update(cx, |project, cx| {
                project.session.apply_audio(&settings);
                cx.notify();
            });
        }
        cx.notify();
    }

    fn test_tone(&mut self, cx: &mut Context<Self>) {
        if let Screen::Project(project) = &self.screen {
            project.update(cx, |project, _| project.session.test_tone());
        }
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
                if !this.file_menu_dismissed {
                    this.file_menu_open = !this.file_menu_open;
                }
                cx.notify();
            }));
        let dropdown = open.then(|| {
            menu::float(
                menu::panel(theme)
                    .min_w(px(220.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.file_menu_dismissed = this.file_menu_open;
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
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(file)
                    .children(dropdown),
            )
    }

    /// The preferences modal: sections down the left, the chosen section's
    /// settings on the right.
    fn render_preferences(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = theme.hover;
        let nav = |section: Section, label: &'static str| {
            let selected = self.section == section;
            div()
                .id(label)
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .when(selected, |d| d.bg(theme.control_selected))
                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_secondary
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.section = section;
                    this.open_choice = None;
                    cx.notify();
                }))
        };
        let sidebar = div()
            .w(px(150.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(10.))
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.hairline)
            .child(
                div()
                    .px(px(10.))
                    .pt(px(4.))
                    .pb(px(10.))
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Preferences"),
            )
            .child(nav(Section::Audio, "Audio"))
            .child(nav(Section::Appearance, "Appearance"));
        let body = match self.section {
            Section::Audio => self.render_audio_settings(theme, cx).into_any_element(),
            Section::Appearance => self.render_appearance(theme, cx).into_any_element(),
        };
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
            .id("preferences-panel")
            .occlude()
            .w(px(640.))
            .h(px(440.))
            .flex()
            .rounded(px(10.))
            .overflow_hidden()
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_lg()
            .text_size(px(13.))
            // Clicks inside close an open drop-down, not the window.
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.open_choice.take().is_some() {
                    cx.notify();
                }
            }))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .p(px(18.))
                    .child(div().flex_1().min_h_0().child(body))
                    .child(div().flex().justify_end().child(close)),
            );
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

    fn render_appearance(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
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
        setting_row(
            theme,
            "Theme",
            div()
                .w(px(240.))
                .flex()
                .p(px(2.))
                .gap(px(2.))
                .rounded(px(7.))
                .bg(theme.control)
                .text_size(px(12.))
                .children(segments)
                .into_any_element(),
        )
    }

    /// Driver, output device, sample rate, buffer size and processing
    /// threads, what is running now, and a test tone.
    fn render_audio_settings(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.preferences.preferences.audio.clone();
        let choices = &self.choices;
        let project = match &self.screen {
            Screen::Project(project) => Some(project.read(cx)),
            Screen::Launch(_) => None,
        };
        let running = project.and_then(|p| p.session.audio().cloned());
        let device_buffer = project.and_then(|p| p.session.device_buffer());
        let error = project.and_then(|p| p.session.audio_error.clone());
        let rate_for_latency = running
            .as_ref()
            .map(|r| r.sample_rate)
            .or(settings.sample_rate)
            .or(choices.default_rate)
            .unwrap_or(48_000);
        let khz = |rate: u32| format!("{} kHz", f64::from(rate) / 1000.);
        let latency = |frames: u32| {
            format!(
                "{frames} samples · {:.1} ms",
                f64::from(frames) / f64::from(rate_for_latency) * 1000.
            )
        };
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());

        // Driver.
        let driver_value = settings.driver.clone().unwrap_or_else(|| {
            self.drivers
                .first()
                .map_or_else(|| "System default".to_owned(), |d| format!("{d} (default)"))
        });
        let driver_options: Vec<(String, bool, AudioSettings)> = self
            .drivers
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let chosen = settings.driver.as_deref() == Some(name)
                    || (settings.driver.is_none() && i == 0);
                (
                    if i == 0 {
                        format!("{name} (default)")
                    } else {
                        name.clone()
                    },
                    chosen,
                    AudioSettings {
                        // A new driver starts from its own defaults.
                        driver: (i != 0).then(|| name.clone()),
                        device: None,
                        sample_rate: None,
                        buffer_frames: None,
                        ..settings.clone()
                    },
                )
            })
            .collect();

        // Output device.
        let device_value = settings
            .device
            .as_ref()
            .and_then(|id| choices.devices.iter().find(|d| &d.id == id))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| {
                choices.devices.iter().find(|d| d.is_default).map_or_else(
                    || "System default".to_owned(),
                    |d| format!("{} (default)", d.name),
                )
            });
        let mut device_options = vec![(
            "System default".to_owned(),
            settings.device.is_none(),
            AudioSettings {
                device: None,
                sample_rate: None,
                buffer_frames: None,
                ..settings.clone()
            },
        )];
        device_options.extend(choices.devices.iter().map(|d| {
            (
                d.name.clone(),
                settings.device.as_deref() == Some(&d.id),
                AudioSettings {
                    device: Some(d.id.clone()),
                    sample_rate: None,
                    buffer_frames: None,
                    ..settings.clone()
                },
            )
        }));

        // Sample rate.
        let rate_value = settings.sample_rate.map_or_else(
            || {
                choices.default_rate.map_or_else(
                    || "Device default".to_owned(),
                    |r| format!("{} (default)", khz(r)),
                )
            },
            khz,
        );
        let mut rate_options = vec![(
            "Device default".to_owned(),
            settings.sample_rate.is_none(),
            AudioSettings {
                sample_rate: None,
                ..settings.clone()
            },
        )];
        rate_options.extend(choices.sample_rates.iter().map(|&rate| {
            (
                khz(rate),
                settings.sample_rate == Some(rate),
                AudioSettings {
                    sample_rate: Some(rate),
                    ..settings.clone()
                },
            )
        }));

        // Buffer size.
        let buffer_value = settings
            .buffer_frames
            .map_or_else(|| "Device default".to_owned(), latency);
        let mut buffer_options = vec![(
            "Device default".to_owned(),
            settings.buffer_frames.is_none(),
            AudioSettings {
                buffer_frames: None,
                ..settings.clone()
            },
        )];
        buffer_options.extend(choices.buffer_sizes.iter().map(|&frames| {
            (
                latency(frames),
                settings.buffer_frames == Some(frames),
                AudioSettings {
                    buffer_frames: Some(frames),
                    ..settings.clone()
                },
            )
        }));

        // Processing threads.
        let auto = daw_engine::auto_threads();
        let threads_value = settings
            .threads
            .map_or_else(|| format!("Auto ({auto})"), |n| n.to_string());
        let mut thread_options = vec![(
            format!("Auto ({auto})"),
            settings.threads.is_none(),
            AudioSettings {
                threads: None,
                ..settings.clone()
            },
        )];
        thread_options.extend((1..=cores.min(16)).map(|n| {
            (
                n.to_string(),
                settings.threads == Some(n),
                AudioSettings {
                    threads: Some(n),
                    ..settings.clone()
                },
            )
        }));

        let status: AnyElement = match (&running, &error) {
            (_, Some(error)) => div()
                .text_color(theme.record)
                .child(format!("Could not start: {error}"))
                .into_any_element(),
            (Some(r), None) => div()
                .text_color(theme.text_secondary)
                .child(format!(
                    "Running: {} · {} · {} · {} · {} thread{}",
                    r.driver,
                    r.device,
                    khz(r.sample_rate),
                    r.buffer_frames
                        .or(device_buffer)
                        .map_or_else(|| "device buffer".to_owned(), latency),
                    r.threads,
                    if r.threads == 1 { "" } else { "s" }
                ))
                .into_any_element(),
            (None, None) => div()
                .text_color(theme.text_tertiary)
                .child("Starts when a project is open.")
                .into_any_element(),
        };
        let hover = theme.hover;
        let can_test = running.is_some();
        let test = div()
            .id("test-tone")
            .px(px(12.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(theme.control)
            .when(can_test, |d| d.hover(move |s| s.bg(hover)))
            .when(!can_test, |d| d.text_color(theme.text_tertiary))
            .child("Test tone")
            .tooltip(tooltip::text(if can_test {
                "Plays one second of A4 (440 Hz) through the output"
            } else {
                "Open a project to play a test tone"
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if can_test {
                    this.test_tone(cx);
                }
            }));

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .pb(px(4.))
                    .child("Audio"),
            )
            .child(self.choice_row(
                theme,
                cx,
                Choice::Driver,
                "Driver",
                driver_value,
                driver_options,
            ))
            .child(self.choice_row(
                theme,
                cx,
                Choice::Device,
                "Output device",
                device_value,
                device_options,
            ))
            .child(self.choice_row(
                theme,
                cx,
                Choice::Rate,
                "Sample rate",
                rate_value,
                rate_options,
            ))
            .child(match choices.fixed_buffer {
                // The driver decides: show its size, not a choice.
                Some(frames) => setting_row(
                    theme,
                    "Buffer size",
                    div()
                        .w(px(280.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().px(px(8.)).child(latency(frames)))
                        .child(
                            div()
                                .px(px(8.))
                                .text_size(px(11.))
                                .text_color(theme.text_tertiary)
                                .child(format!(
                                    "Set by {}. A smaller buffer needs a driver that allows it, such as ASIO.",
                                    choices.driver
                                )),
                        )
                        .into_any_element(),
                )
                .into_any_element(),
                None => self
                    .choice_row(theme, cx, Choice::Buffer, "Buffer size", buffer_value, buffer_options)
                    .into_any_element(),
            })
            .child(self.choice_row(
                theme,
                cx,
                Choice::Threads,
                "Processing threads",
                threads_value,
                thread_options,
            ))
            .child(div().h(px(1.)).bg(theme.hairline))
            .child(status)
            .child(div().flex().child(test))
    }

    /// A labelled drop-down: the current value, and when open the options
    /// (with a tick on the chosen one); picking one applies it.
    fn choice_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
        choice: Choice,
        label: &'static str,
        value: String,
        options: Vec<(String, bool, AudioSettings)>,
    ) -> impl IntoElement {
        let open = self.open_choice == Some(choice);
        let hover = theme.hover;
        let button = div()
            .id(label)
            .w(px(280.))
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .bg(theme.control)
            .hover(move |s| s.bg(hover))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(value),
            )
            .child(icon(Icon::ChevronDown, theme.text_tertiary, 11.))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_choice = if this.open_choice == Some(choice) {
                    None
                } else {
                    Some(choice)
                };
                cx.notify();
            }));
        let list = open.then(|| {
            let items = options
                .into_iter()
                .enumerate()
                .map(|(i, (text, chosen, settings))| {
                    menu::item(theme, (label, i), text, if chosen { "✓" } else { "" }).on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_audio(settings.clone(), cx);
                        }),
                    )
                });
            menu::float(
                menu::panel(theme)
                    .w(px(280.))
                    .max_h(px(260.))
                    .children(items.collect::<Vec<_>>()),
            )
        });
        setting_row(
            theme,
            label,
            div()
                .relative()
                .flex()
                .flex_col()
                .child(button)
                .children(list)
                .into_any_element(),
        )
    }

    /// Puts a project at the top of the recent list.
    fn remember(&mut self, session: &Session) {
        let name = session.snapshot().song_name.clone();
        if let Err(e) = self.recent.touch(session.folder(), &name) {
            eprintln!("could not update recent projects: {e}");
        }
    }
}

/// A setting: its name on the left, its control on the right.
fn setting_row(theme: &Theme, label: &'static str, control: AnyElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .child(
            div()
                .w(px(130.))
                .flex_none()
                .text_color(theme.text_secondary)
                .child(label),
        )
        .child(control)
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
            // Runs before the File menu's press-outside handler.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.file_menu_dismissed = false))
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
