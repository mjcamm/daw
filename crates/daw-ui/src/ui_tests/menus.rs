//! Menus: they open on press, like native ones, and their trigger toggles.

use super::Harness;
use crate::menu::OpenMenu;

/// docs/behaviours.md: "Menus open on press".
#[test]
fn a_menu_opens_on_press_and_its_trigger_closes_it() {
    let mut h = Harness::new("menu-press");
    let at = h.bounds("song-name").center();
    h.press(at);
    let open = h.view.read_with(h.cx, |v, _| v.open_menu);
    assert_eq!(open, Some(OpenMenu::Song), "open before the release");
    h.release(at);
    h.click(at, 1);
    let open = h.view.read_with(h.cx, |v, _| v.open_menu);
    assert_eq!(open, None, "pressing the trigger again closes it");
}
