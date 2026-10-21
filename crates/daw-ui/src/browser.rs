//! The device browser: one modal for adding an instrument or an effect,
//! with the built-in devices and plugins side by side, searchable
//! (`docs/design/vst3-hosting.md`, The browser).
//!
//! The list is worked out by plain functions ([`rows`]); the view shows
//! only the rows on screen.

use crate::icons::{Icon, icon};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;
use daw_engine::devices::{
    EFFECTS, INSTRUMENTS, effect_description, effect_name, instrument_description, instrument_name,
};
use daw_engine::{EffectKind, InstrumentKind, NodeId};
use daw_plugins::scan::{PluginInfo, Progress};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

actions!(device_browser, [Up, Down]);

/// The key context the browser sets (arrows move the highlight).
pub const KEY_CONTEXT: &str = "DeviceBrowser";

const WIDTH: f32 = 560.;
const HEIGHT: f32 = 520.;
const ROW_HEIGHT: f32 = 26.;
/// How often an open browser shows a running scan's progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// What the browser adds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Instrument(NodeId),
    Effect(NodeId),
}

impl Target {
    pub fn node(self) -> NodeId {
        match self {
            Self::Instrument(node) | Self::Effect(node) => node,
        }
    }
}

/// Built-in devices, or plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    #[default]
    BuiltIn,
    Plugins,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Self::BuiltIn => Self::Plugins,
            Self::Plugins => Self::BuiltIn,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::BuiltIn => "Built-in",
            Self::Plugins => "Plugins",
        }
    }
}

/// The plugins the browser offers: the catalog's, by vendor then name, and
/// a scan's progress while one runs.
#[derive(Debug, Clone, Default)]
pub struct PluginShelf {
    pub plugins: Arc<Vec<PluginInfo>>,
    pub scanning: Option<Arc<Progress>>,
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Instrument(InstrumentKind),
    Effect(EffectKind),
    Plugin(PluginInfo),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BrowserEvent {
    Chose(Choice),
    Close,
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A vendor's name, above their plugins.
    Heading(SharedString),
    Item {
        choice: Choice,
        name: SharedString,
        /// A built-in device's description; a plugin's type.
        detail: SharedString,
        /// False for what can't be added yet (plugin instruments).
        enabled: bool,
    },
}

impl Row {
    fn enabled(&self) -> bool {
        matches!(self, Self::Item { enabled: true, .. })
    }
}

/// A plugin's type, from its subcategories ("Fx|Delay" is "Delay").
pub fn plugin_type(info: &PluginInfo) -> String {
    let named: Vec<&str> = info
        .subcategories
        .iter()
        .map(String::as_str)
        .filter(|s| !matches!(*s, "Fx" | "Instrument"))
        .collect();
    if !named.is_empty() {
        named.join(" · ")
    } else if info.is_instrument() {
        "Instrument".into()
    } else {
        "Effect".into()
    }
}

/// Whether every word of `query` is in one of `fields` (ignoring case).
fn matches(query: &str, fields: &[&str]) -> bool {
    let fields: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| fields.iter().any(|f| f.contains(word)))
}

/// The list for one side: built-in devices, or plugins under vendor
/// headings, narrowed to those matching `query`.
pub fn rows(target: Target, side: Side, query: &str, plugins: &[PluginInfo]) -> Vec<Row> {
    match side {
        Side::BuiltIn => {
            let items: Vec<(Choice, &str, &str)> = match target {
                Target::Instrument(_) => INSTRUMENTS
                    .iter()
                    .map(|&k| {
                        (
                            Choice::Instrument(k),
                            instrument_name(k),
                            instrument_description(k),
                        )
                    })
                    .collect(),
                Target::Effect(_) => EFFECTS
                    .iter()
                    .map(|&k| (Choice::Effect(k), effect_name(k), effect_description(k)))
                    .collect(),
            };
            items
                .into_iter()
                .filter(|(_, name, detail)| matches(query, &[name, detail]))
                .map(|(choice, name, detail)| Row::Item {
                    choice,
                    name: name.into(),
                    detail: detail.into(),
                    enabled: true,
                })
                .collect()
        }
        Side::Plugins => {
            let instruments = matches!(target, Target::Instrument(_));
            let mut rows = Vec::new();
            let mut vendor: Option<&str> = None;
            for info in plugins.iter().filter(|p| p.is_instrument() == instruments) {
                let kind = plugin_type(info);
                if !matches(query, &[&info.name, &info.vendor, &kind]) {
                    continue;
                }
                if vendor != Some(info.vendor.as_str()) {
                    vendor = Some(&info.vendor);
                    let heading = if info.vendor.is_empty() {
                        "Unknown vendor"
                    } else {
                        &info.vendor
                    };
                    rows.push(Row::Heading(heading.to_owned().into()));
                }
                rows.push(Row::Item {
                    choice: Choice::Plugin(info.clone()),
                    name: info.name.clone().into(),
                    detail: kind.into(),
                    // Plugin instruments come with Phase 8 step 4.
                    enabled: !instruments,
                });
            }
            rows
        }
    }
}

/// How many items a side's list holds.
fn item_count(rows: &[Row]) -> usize {
    rows.iter()
        .filter(|r| matches!(r, Row::Item { .. }))
        .count()
}

pub struct DeviceBrowser {
    target: Target,
    side: Side,
    shelf: PluginShelf,
    search: gpui::Entity<TextInput>,
    query: String,
    rows: Vec<Row>,
    /// Items on the other side matching the search.
    other_matches: usize,
    /// The row Enter adds (always an enabled item).
    highlighted: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _search_events: Subscription,
}

impl EventEmitter<BrowserEvent> for DeviceBrowser {}

impl Focusable for DeviceBrowser {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DeviceBrowser {
    pub fn new(
        target: Target,
        side: Side,
        shelf: PluginShelf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| TextInput::new("", "Search", cx).plain());
        window.focus(&search.read(cx).focus_handle(cx));
        let subscription = cx.subscribe(&search, |this, _, event: &TextInputEvent, cx| {
            match event {
                TextInputEvent::Changed(text) => {
                    this.query.clone_from(text);
                    this.refresh_rows();
                }
                TextInputEvent::Confirm(_) => this.choose_highlighted(cx),
                TextInputEvent::Cancel => cx.emit(BrowserEvent::Close),
            }
            cx.notify();
        });
        let mut browser = Self {
            target,
            side,
            shelf: PluginShelf::default(),
            search,
            query: String::new(),
            rows: Vec::new(),
            other_matches: 0,
            highlighted: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _search_events: subscription,
        };
        browser.set_shelf(shelf, cx);
        browser
    }

    pub fn target(&self) -> Target {
        self.target
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// New plugins (a scan finished) or a scan started.
    pub fn set_shelf(&mut self, shelf: PluginShelf, cx: &mut Context<Self>) {
        let scanning = shelf.scanning.is_some();
        self.shelf = shelf;
        self.refresh_rows();
        if scanning {
            // Shows the scan's progress until it ends (a new shelf).
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(PROGRESS_EVERY).await;
                    let more = this.update(cx, |this, cx| {
                        let more = this.shelf.scanning.is_some();
                        if more && this.side == Side::Plugins {
                            cx.notify();
                        }
                        more
                    });
                    if !matches!(more, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn set_side(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        self.side = side;
        self.refresh_rows();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        window.focus(&self.search.read(cx).focus_handle(cx));
        cx.notify();
    }

    /// Works out the list again, highlighting its first item.
    fn refresh_rows(&mut self) {
        let plugins = &self.shelf.plugins;
        self.rows = rows(self.target, self.side, &self.query, plugins);
        self.other_matches = if self.query.trim().is_empty() {
            0
        } else {
            item_count(&rows(self.target, self.side.other(), &self.query, plugins))
        };
        self.highlighted = self.rows.iter().position(Row::enabled);
    }

    /// Moves the highlight to the next enabled item up or down.
    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let enabled: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.rows[i].enabled())
            .collect();
        let at = self
            .highlighted
            .and_then(|h| enabled.iter().position(|&i| i == h));
        let next = match (at, down) {
            (None, _) => enabled.first(),
            (Some(i), true) => enabled.get(i + 1).or(enabled.last()),
            (Some(i), false) => enabled.get(i.saturating_sub(1)),
        };
        if let Some(&row) = next {
            self.highlighted = Some(row);
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
            cx.notify();
        }
    }

    fn choose_highlighted(&mut self, cx: &mut Context<Self>) {
        if let Some(Row::Item {
            choice,
            enabled: true,
            ..
        }) = self.highlighted.and_then(|i| self.rows.get(i))
        {
            cx.emit(BrowserEvent::Chose(choice.clone()));
        }
    }

    fn render_row(&self, index: usize, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let base = div()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px(px(10.))
            .flex()
            .items_center();
        match &self.rows[index] {
            Row::Heading(vendor) => base
                .pt(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_tertiary)
                .child(vendor.clone())
                .into_any_element(),
            Row::Item {
                choice,
                name,
                detail,
                enabled,
            } => {
                let highlighted = self.highlighted == Some(index);
                let hover = theme.hover;
                let row = base
                    .id(("browser-row", index))
                    .debug_selector(move || format!("browser-row-{index}"))
                    .justify_between()
                    .gap(px(16.))
                    .rounded(px(5.))
                    .when(highlighted, |d| d.bg(theme.control_selected))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(if *enabled {
                                theme.text
                            } else {
                                theme.text_tertiary
                            })
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme.text_tertiary)
                            .child(detail.clone()),
                    );
                if !enabled {
                    return row.into_any_element();
                }
                let choice = choice.clone();
                row.when(!highlighted, |d| d.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(BrowserEvent::Chose(choice.clone()));
                    }))
                    .into_any_element()
            }
        }
    }

    /// What shows instead of (or above) the list: an empty search, no
    /// plugins yet, a scan running, plugin instruments to come.
    fn notes(&self, theme: &Theme) -> Vec<gpui::AnyElement> {
        let note = |text: String| {
            div()
                .px(px(10.))
                .py(px(4.))
                .text_size(px(12.))
                .text_color(theme.text_secondary)
                .child(text)
                .into_any_element()
        };
        let mut notes = Vec::new();
        if self.side == Side::Plugins {
            if let Some(progress) = &self.shelf.scanning {
                let done = progress.done.load(Ordering::Relaxed);
                let total = progress.total.load(Ordering::Relaxed);
                notes.push(note(if total == 0 {
                    "Looking for plugins…".into()
                } else {
                    format!("Scanning {done} of {total}…")
                }));
            }
            if matches!(self.target, Target::Instrument(_)) && !self.rows.is_empty() {
                notes.push(note(
                    "Plugin instruments can be added once instrument hosting lands.".into(),
                ));
            }
        }
        if self.rows.is_empty() {
            let searching = !self.query.trim().is_empty();
            let mut text = if searching {
                format!("Nothing here matches “{}”.", self.query.trim())
            } else if self.side == Side::Plugins && self.shelf.scanning.is_none() {
                "No plugins found. Add folders in Preferences → Plugins.".into()
            } else {
                String::new()
            };
            if searching && self.other_matches > 0 {
                text.push_str(&format!(
                    " {} has {}.",
                    self.side.other().name(),
                    self.other_matches
                ));
            }
            if !text.is_empty() {
                notes.push(note(text));
            }
        }
        notes
    }

    fn segment(&self, side: Side, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.side == side;
        let hover = theme.hover;
        div()
            .id(side.name())
            .debug_selector(move || format!("browser-side-{}", side.name()))
            .px(px(12.))
            .py(px(3.))
            .rounded(px(5.))
            .when(selected, |d| {
                d.bg(theme.control_selected).text_color(theme.text)
            })
            .when(!selected, |d| {
                d.text_color(theme.text_secondary)
                    .hover(move |s| s.bg(hover))
            })
            .child(side.name())
            .on_click(cx.listener(move |this, _, window, cx| this.set_side(side, window, cx)))
    }
}

impl Render for DeviceBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let title = match self.target {
            Target::Instrument(_) => "Add Instrument",
            Target::Effect(_) => "Add Effect",
        };
        let switch = div()
            .flex_none()
            .flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(theme.control)
            .child(self.segment(Side::BuiltIn, &theme, cx))
            .child(self.segment(Side::Plugins, &theme, cx));
        let search = div()
            .flex_1()
            .h(px(28.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(7.))
            .bg(theme.control)
            .child(icon(Icon::Search, theme.text_tertiary, 13.))
            .child(div().flex_1().child(self.search.clone()));
        let count = self.rows.len();
        let theme_rows = theme;
        let list = uniform_list(
            "browser-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|i| this.render_row(i, &theme_rows, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.scroll.clone())
        .flex_1();
        let panel = div()
            .id("browser-panel")
            .debug_selector(|| "browser-panel".into())
            .occlude()
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_lg()
            .text_size(px(13.))
            .text_color(theme.text)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &Up, _, cx| this.step(false, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.step(true, cx)))
            // Typing always goes to the search field.
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.search.read(cx).focus_handle(cx));
            }))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(switch)
                    .child(search),
            )
            .children(self.notes(&theme))
            .child(list);
        // The dimmed backdrop: a press outside the panel closes it.
        div()
            .id("browser-scrim")
            .debug_selector(|| "browser-scrim".into())
            .track_focus(&self.focus)
            .occlude()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.scrim)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserEvent::Close)))
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugin(vendor: &str, name: &str, subcategories: &[&str]) -> PluginInfo {
        PluginInfo {
            bundle: PathBuf::from(format!("{name}.vst3")),
            cid: name.into(),
            name: name.into(),
            vendor: vendor.into(),
            version: "1".into(),
            subcategories: subcategories.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn catalog() -> Vec<PluginInfo> {
        vec![
            plugin("Valhalla DSP", "ValhallaDelay", &["Fx", "Delay"]),
            plugin("Valhalla DSP", "ValhallaRoom", &["Fx", "Reverb"]),
            plugin("Xfer Records", "OTT", &["Fx"]),
            plugin("Xfer Records", "Serum 2", &["Instrument", "Synth"]),
        ]
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Heading(v) => format!("# {v}"),
                Row::Item { name, detail, .. } => format!("{name} ({detail})"),
            })
            .collect()
    }

    const FX: Target = Target::Effect(NodeId(1));
    const INSTRUMENT: Target = Target::Instrument(NodeId(1));

    #[test]
    fn plugin_effects_are_grouped_by_vendor_with_their_type() {
        assert_eq!(
            names(&rows(FX, Side::Plugins, "", &catalog())),
            [
                "# Valhalla DSP",
                "ValhallaDelay (Delay)",
                "ValhallaRoom (Reverb)",
                "# Xfer Records",
                "OTT (Effect)",
            ]
        );
    }

    #[test]
    fn plugin_instruments_are_listed_for_instruments_but_not_yet_addable() {
        let rows = rows(INSTRUMENT, Side::Plugins, "", &catalog());
        assert_eq!(names(&rows), ["# Xfer Records", "Serum 2 (Synth)"]);
        assert!(!rows[1].enabled());
    }

    #[test]
    fn search_matches_every_word_in_name_vendor_or_type() {
        assert_eq!(
            names(&rows(FX, Side::Plugins, "valhalla REV", &catalog())),
            ["# Valhalla DSP", "ValhallaRoom (Reverb)"]
        );
        assert_eq!(
            names(&rows(FX, Side::Plugins, "xfer", &catalog())),
            ["# Xfer Records", "OTT (Effect)"]
        );
        assert!(rows(FX, Side::Plugins, "zzz", &catalog()).is_empty());
    }

    #[test]
    fn built_in_devices_with_their_descriptions() {
        assert_eq!(
            names(&rows(FX, Side::BuiltIn, "", &catalog())),
            ["Filter (Resonant low-, high- or band-pass filter)"]
        );
        assert_eq!(
            names(&rows(INSTRUMENT, Side::BuiltIn, "sample", &catalog())),
            ["SingleSampler (Plays one audio sample for every note)"]
        );
    }
}
