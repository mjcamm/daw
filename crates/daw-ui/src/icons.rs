//! Icons from the Lucide set (ISC licence, see `assets/icons/LICENSE`),
//! embedded in the binary and served to GPUI as assets.
//!
//! To add an icon: download it from the pinned `lucide-static` version into
//! `assets/icons/`, then add a variant below.

use gpui::{AssetSource, Hsla, SharedString, Styled, Svg, px, svg};
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Stop,
    ToStart,
    Loop,
    ChevronDown,
    Eye,
    EyeOff,
    Close,
    Plus,
    GripVertical,
    Magnet,
    Move,
    GoTo,
    Pencil,
    Inbound,
    ChevronLeft,
    ChevronRight,
    Signal,
    Panel,
}

/// Every embedded icon: its variant, asset path and file contents.
const ICONS: [(Icon, &str, &[u8]); 19] = [
    (
        Icon::Play,
        "icons/play.svg",
        include_bytes!("../assets/icons/play.svg"),
    ),
    (
        Icon::Stop,
        "icons/square.svg",
        include_bytes!("../assets/icons/square.svg"),
    ),
    (
        Icon::ToStart,
        "icons/skip-back.svg",
        include_bytes!("../assets/icons/skip-back.svg"),
    ),
    (
        Icon::Loop,
        "icons/repeat.svg",
        include_bytes!("../assets/icons/repeat.svg"),
    ),
    (
        Icon::ChevronDown,
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (
        Icon::Eye,
        "icons/eye.svg",
        include_bytes!("../assets/icons/eye.svg"),
    ),
    (
        Icon::EyeOff,
        "icons/eye-off.svg",
        include_bytes!("../assets/icons/eye-off.svg"),
    ),
    (
        Icon::Close,
        "icons/x.svg",
        include_bytes!("../assets/icons/x.svg"),
    ),
    (
        Icon::Plus,
        "icons/plus.svg",
        include_bytes!("../assets/icons/plus.svg"),
    ),
    (
        Icon::GripVertical,
        "icons/grip-vertical.svg",
        include_bytes!("../assets/icons/grip-vertical.svg"),
    ),
    (
        Icon::Magnet,
        "icons/magnet.svg",
        include_bytes!("../assets/icons/magnet.svg"),
    ),
    (
        Icon::Move,
        "icons/move.svg",
        include_bytes!("../assets/icons/move.svg"),
    ),
    (
        Icon::GoTo,
        "icons/arrow-up-right.svg",
        include_bytes!("../assets/icons/arrow-up-right.svg"),
    ),
    (
        Icon::Pencil,
        "icons/pencil.svg",
        include_bytes!("../assets/icons/pencil.svg"),
    ),
    (
        Icon::Inbound,
        "icons/arrow-down-left.svg",
        include_bytes!("../assets/icons/arrow-down-left.svg"),
    ),
    (
        Icon::ChevronLeft,
        "icons/chevron-left.svg",
        include_bytes!("../assets/icons/chevron-left.svg"),
    ),
    (
        Icon::ChevronRight,
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        Icon::Signal,
        "icons/radio.svg",
        include_bytes!("../assets/icons/radio.svg"),
    ),
    (
        Icon::Panel,
        "icons/panel-right.svg",
        include_bytes!("../assets/icons/panel-right.svg"),
    ),
];

impl Icon {
    fn path(self) -> &'static str {
        ICONS
            .iter()
            .find(|(icon, _, _)| *icon == self)
            .map_or("", |(_, path, _)| path)
    }
}

/// An icon drawn in `color`, `size` pixels square.
pub fn icon(kind: Icon, color: Hsla, size: f32) -> Svg {
    svg().path(kind.path()).size(px(size)).text_color(color)
}

/// Serves the embedded icons to GPUI.
#[derive(Debug, Clone, Copy)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(_, p, _)| *p == path)
            .map(|(_, _, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(_, p, _)| p.starts_with(path))
            .map(|(_, p, _)| SharedString::from(*p))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_embedded_and_loadable() {
        for (icon, path, _) in ICONS {
            assert_eq!(icon.path(), path);
            let bytes = Assets.load(path).unwrap().unwrap();
            assert!(bytes.starts_with(b"<!-- @license lucide"), "{path}");
        }
    }
}
