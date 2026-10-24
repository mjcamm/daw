//! The metronome: its switch and volume in the transport, and its page in
//! the preferences (kept in the preferences file, sent to the engine).

use super::Harness;
use crate::app::{AppView, Section};
use crate::project::ProjectView;
use crate::session::Session;
use crate::theme::Theme;
use daw_engine::{ClickSound, Metronome};
use daw_host::preferences::StoredPreferences;
use daw_host::projects::RecentProjects;
use gpui::{
    Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, TestAppContext, VisualTestContext, WindowAppearance, point, px, size,
};
use std::path::PathBuf;

/// The slider's thumb: its travel is its length less this.
const THUMB: f32 = 9.;

fn metronome(h: &mut Harness) -> Metronome {
    h.snapshot().metronome
}

/// Whether the popover shows. (GPUI's test `debug_bounds` keeps every
/// element ever drawn, so it can only show one appearing: the view says
/// whether it is open now.)
fn popover_open(h: &mut Harness) -> bool {
    h.view.read_with(h.cx, |v, _| v.volume_open())
}

/// docs/behaviours.md: "The metronome button beside the transport switches
/// the click on and off".
#[test]
fn the_metronome_button_switches_the_click() {
    let mut h = Harness::new("metronome-switch");
    assert!(!metronome(&mut h).on, "off at first");
    let at = h.bounds("transport-metronome").center();
    h.click(at, 1);
    assert!(metronome(&mut h).on);
    h.view.read_with(h.cx, |v, _| {
        assert!(v.metronome_to_keep.is_none(), "handed to the app to keep");
    });
    h.click(at, 1);
    assert!(!metronome(&mut h).on);
}

/// docs/behaviours.md: "Hovering the volume button opens a vertical slider
/// for the click's level ...".
#[test]
fn hovering_the_volume_button_opens_its_slider() {
    let mut h = Harness::new("metronome-volume");
    let button = h.bounds("transport-metronome-volume");
    assert!(!popover_open(&mut h));
    assert!(h.cx.debug_bounds("metronome-volume-popover").is_none());
    h.hover(button.center());
    assert!(popover_open(&mut h), "opens on hover");
    assert!(
        h.cx.debug_bounds("metronome-volume-popover").is_some(),
        "and is drawn"
    );
    // Down into the popover: it stays.
    let slider = h.bounds("metronome-volume-slider");
    h.hover(point(button.center().x, button.bottom() + px(2.)));
    assert!(popover_open(&mut h), "the gap below the button keeps it");
    h.hover(slider.center());
    assert!(popover_open(&mut h), "over the slider");

    // Up a quarter of the travel from 70%: 95%, the thumb under the pointer.
    let before = metronome(&mut h).volume;
    let travel = slider.size.height / px(1.) - THUMB;
    let from = slider.center();
    h.press(from);
    let to = point(from.x, from.y - px(travel / 4.));
    h.move_held(to);
    assert_eq!(metronome(&mut h).volume, before + 25, "heard as it moves");
    // Off the popover while held: it stays until let go.
    h.move_held(point(from.x + px(300.), to.y));
    assert!(popover_open(&mut h), "held open while dragging");
    h.move_held(to);
    h.release(to);
    assert_eq!(metronome(&mut h).volume, 95);

    // Away from both: it closes.
    h.hover(point(px(600.), px(400.)));
    assert!(!popover_open(&mut h), "closes when the pointer leaves");
}

/// The whole app in a test window, its preferences and recent projects in
/// a folder of their own, with a project open.
struct AppHarness {
    app: Entity<AppView>,
    cx: &'static mut VisualTestContext,
    folder: PathBuf,
}

impl AppHarness {
    fn new(name: &str) -> Self {
        let folder =
            std::env::temp_dir().join(format!("daw-ui-test-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("song")).expect("test folder");
        let preferences = StoredPreferences::load(folder.join("preferences.json"));
        let recent = RecentProjects::load(folder.join("recent.json"));
        let session = Session::headless(folder.join("song")).expect("headless session");
        let test_app = Box::leak(Box::new(TestAppContext::single()));
        test_app.update(|cx| cx.set_global(Theme::for_appearance(WindowAppearance::Dark)));
        let (app, cx) =
            test_app.add_window_view(|window, cx| AppView::with(preferences, recent, window, cx));
        cx.simulate_resize(size(px(1280.), px(800.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.show_project(session, window, cx));
        });
        let mut h = Self { app, cx, folder };
        h.redraw();
        h
    }

    fn redraw(&mut self) {
        for _ in 0..2 {
            self.cx.update(|window, _| window.refresh());
            self.cx.run_until_parked();
        }
    }

    fn bounds(&mut self, selector: &'static str) -> Bounds<Pixels> {
        self.cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("nothing on screen is tagged {selector:?}"))
    }

    fn click(&mut self, at: Point<Pixels>) {
        self.cx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        self.cx.simulate_event(MouseUpEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
        });
        self.redraw();
    }

    fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>) {
        self.cx.simulate_event(MouseDownEvent {
            position: from,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        self.redraw();
        for at in [point((from.x + to.x) / 2., from.y), to] {
            self.cx.simulate_event(MouseMoveEvent {
                position: at,
                modifiers: Modifiers::none(),
                pressed_button: Some(MouseButton::Left),
            });
            self.redraw();
        }
        self.cx.simulate_event(MouseUpEvent {
            position: to,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 1,
        });
        self.redraw();
    }

    fn project(&mut self) -> Entity<ProjectView> {
        self.app.read_with(self.cx, |app, _| match &app.screen {
            crate::app::Screen::Project(project) => project.clone(),
            crate::app::Screen::Launch(_) => panic!("no project open"),
        })
    }

    /// The metronome as the engine plays it.
    fn heard(&mut self) -> Metronome {
        let project = self.project();
        self.redraw();
        project.read_with(self.cx, |p, _| p.session().snapshot().metronome)
    }

    /// The metronome as the preferences file keeps it.
    fn saved(&self) -> Metronome {
        StoredPreferences::load(self.folder.join("preferences.json"))
            .preferences
            .metronome
    }

    fn open_metronome_page(&mut self) {
        self.app.update(self.cx, |app, cx| {
            app.open_preferences(cx);
            app.section = Section::Metronome;
        });
        self.redraw();
    }
}

impl Drop for AppHarness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// docs/behaviours.md: "Preferences → Metronome: sound (picking one plays
/// it), accent on beat 1, pitch and volume; kept for every song".
#[test]
fn the_metronome_page_sets_the_click_and_keeps_it() {
    let mut h = AppHarness::new("metronome-page");
    h.open_metronome_page();
    let cowbell = h.bounds("metronome-sound-Cowbell").center();
    h.click(cowbell);
    assert_eq!(h.heard().sound, ClickSound::Cowbell, "the engine plays it");
    assert_eq!(
        h.saved().sound,
        ClickSound::Cowbell,
        "the preferences keep it"
    );

    let accent = h.bounds("metronome-accent").center();
    h.click(accent);
    assert!(!h.heard().accent);
    assert!(!h.saved().accent);

    // Pitch: from the middle, a quarter of the travel right is +6.
    let pitch = h.bounds("metronome-pitch");
    let travel = pitch.size.width / px(1.) - THUMB;
    let from = point(
        pitch.left() + px(THUMB / 2. + travel / 2.),
        pitch.center().y,
    );
    h.drag(from, point(from.x + px(travel / 4.), from.y));
    assert_eq!(h.heard().pitch, 6);
    assert_eq!(h.saved().pitch, 6, "saved when let go");
}

/// docs/behaviours.md: "The metronome button ..." and "... kept for every
/// song": the transport's changes reach the preferences too.
#[test]
fn the_transport_switch_is_kept_in_the_preferences() {
    let mut h = AppHarness::new("metronome-kept");
    assert!(!h.saved().on);
    let at = h.bounds("transport-metronome").center();
    h.click(at);
    assert!(h.heard().on);
    assert!(h.saved().on, "kept for the next song");
}
