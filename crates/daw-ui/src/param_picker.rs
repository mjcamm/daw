//! Picking one of a plugin's parameters to give layers: the dropdown under
//! "+ Add Parameter" for a plugin, searchable, in the plugin's groups
//! (`docs/design/vst3-hosting.md`, Parameters).
//!
//! The list is worked out by a plain function ([`rows`]); the view shows
//! only the rows on screen.

use crate::browser::{Down, Up, matches};
use crate::icons::{Icon, icon};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;
use daw_engine::{DeviceRef, PluginParamInfo};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use std::collections::HashSet;
use std::sync::Arc;

/// The key context the picker sets (arrows move the highlight).
pub const KEY_CONTEXT: &str = "ParamPicker";

const WIDTH: f32 = 300.;
const ROW_HEIGHT: f32 = 26.;
/// Rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 12;

#[derive(Debug, Clone, PartialEq)]
pub enum PickerEvent {
    /// The plugin's id for the chosen parameter.
    Chose(u32),
    Close,
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A group's name, above its parameters.
    Heading(SharedString),
    Item {
        id: u32,
        name: SharedString,
    },
}

/// The parameters to offer, minus `taken` (those that already have
/// layers): ungrouped first, then each group in the order the plugin first
/// names it, narrowed to those matching every word of `query` in their
/// name or group.
pub fn rows(params: &[PluginParamInfo], taken: &HashSet<u32>, query: &str) -> Vec<Row> {
    let offered: Vec<&PluginParamInfo> = params
        .iter()
        .filter(|p| !taken.contains(&p.id) && matches(query, &[&p.name, &p.group]))
        .collect();
    let mut groups: Vec<&str> = Vec::new();
    for p in &offered {
        if !groups.contains(&p.group.as_str()) {
            groups.push(&p.group);
        }
    }
    groups.sort_by_key(|g| !g.is_empty());
    let mut rows = Vec::new();
    for group in groups {
        if !group.is_empty() {
            rows.push(Row::Heading(group.to_owned().into()));
        }
        rows.extend(
            offered
                .iter()
                .filter(|p| p.group == group)
                .map(|p| Row::Item {
                    id: p.id,
                    name: p.name.clone().into(),
                }),
        );
    }
    rows
}

pub struct ParamPicker {
    device: DeviceRef,
    params: Arc<Vec<PluginParamInfo>>,
    taken: HashSet<u32>,
    search: gpui::Entity<TextInput>,
    query: String,
    rows: Vec<Row>,
    /// The row Enter picks (always an item).
    highlighted: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _search_events: Subscription,
}

impl EventEmitter<PickerEvent> for ParamPicker {}

impl Focusable for ParamPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ParamPicker {
    pub fn new(
        device: DeviceRef,
        params: Arc<Vec<PluginParamInfo>>,
        taken: HashSet<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| TextInput::new("", "Search parameters", cx).plain());
        window.focus(&search.read(cx).focus_handle(cx));
        let subscription = cx.subscribe(&search, |this, _, event: &TextInputEvent, cx| {
            match event {
                TextInputEvent::Changed(text) => {
                    this.query.clone_from(text);
                    this.refresh_rows();
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                TextInputEvent::Confirm(_) => this.choose_highlighted(cx),
                TextInputEvent::Cancel => cx.emit(PickerEvent::Close),
            }
            cx.notify();
        });
        let mut picker = Self {
            device,
            params,
            taken,
            search,
            query: String::new(),
            rows: Vec::new(),
            highlighted: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _search_events: subscription,
        };
        picker.refresh_rows();
        picker
    }

    pub fn device(&self) -> DeviceRef {
        self.device
    }

    /// Works out the list again, highlighting its first item.
    fn refresh_rows(&mut self) {
        self.rows = rows(&self.params, &self.taken, &self.query);
        self.highlighted = self.rows.iter().position(|r| matches!(r, Row::Item { .. }));
    }

    /// Moves the highlight to the next item up or down.
    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let items: Vec<usize> = (0..self.rows.len())
            .filter(|&i| matches!(self.rows[i], Row::Item { .. }))
            .collect();
        let at = self
            .highlighted
            .and_then(|h| items.iter().position(|&i| i == h));
        let next = match (at, down) {
            (None, _) => items.first(),
            (Some(i), true) => items.get(i + 1).or(items.last()),
            (Some(i), false) => items.get(i.saturating_sub(1)),
        };
        if let Some(&row) = next {
            self.highlighted = Some(row);
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
            cx.notify();
        }
    }

    fn choose_highlighted(&mut self, cx: &mut Context<Self>) {
        if let Some(Row::Item { id, .. }) = self.highlighted.and_then(|i| self.rows.get(i)) {
            cx.emit(PickerEvent::Chose(*id));
        }
    }

    fn render_row(&self, index: usize, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let base = div()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px(px(10.))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap();
        match &self.rows[index] {
            Row::Heading(group) => base
                .pt(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_tertiary)
                .child(group.clone())
                .into_any_element(),
            Row::Item { id, name } => {
                let highlighted = self.highlighted == Some(index);
                let hover = theme.hover;
                let id = *id;
                base.id(("param-row", index))
                    .debug_selector(move || format!("param-row-{index}"))
                    .rounded(px(5.))
                    .when(highlighted, |d| d.bg(theme.control_selected))
                    .when(!highlighted, |d| d.hover(move |s| s.bg(hover)))
                    .child(name.clone())
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Chose(id))))
                    .into_any_element()
            }
        }
    }

    /// Why the list is empty.
    fn empty_note(&self) -> Option<String> {
        if !self.rows.is_empty() {
            return None;
        }
        Some(if !self.query.trim().is_empty() {
            format!("Nothing matches “{}”.", self.query.trim())
        } else if self.params.is_empty() {
            "No parameters to show: the plugin isn't running, or has none to automate.".into()
        } else {
            "Every parameter has layers.".into()
        })
    }
}

impl Render for ParamPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let search = div()
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
            "param-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|i| this.render_row(i, &theme_rows, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.scroll.clone())
        .h(px(count.min(VISIBLE_ROWS) as f32 * ROW_HEIGHT));
        let note = self.empty_note().map(|text| {
            div()
                .px(px(10.))
                .py(px(4.))
                .text_size(px(12.))
                .text_color(theme.text_secondary)
                .child(text)
        });
        div()
            .id("param-picker")
            .debug_selector(|| "param-picker".into())
            .track_focus(&self.focus)
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap(px(6.))
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &Up, _, cx| this.step(false, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.step(true, cx)))
            .child(search)
            .children(note)
            .when(count > 0, |d| d.child(list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(id: u32, name: &str, group: &str) -> PluginParamInfo {
        PluginParamInfo {
            id,
            name: name.into(),
            group: group.into(),
            unit: String::new(),
            steps: 0,
            value: 0.0,
            bypass: false,
        }
    }

    fn plugin() -> Vec<PluginParamInfo> {
        vec![
            param(10, "Cutoff", "Filter"),
            param(1, "Master", ""),
            param(20, "Level", "Osc A"),
            param(11, "Resonance", "Filter"),
            param(2, "Bypass", ""),
        ]
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Heading(g) => format!("# {g}"),
                Row::Item { name, .. } => name.to_string(),
            })
            .collect()
    }

    #[test]
    fn parameters_are_listed_in_the_plugins_groups_ungrouped_first() {
        assert_eq!(
            names(&rows(&plugin(), &HashSet::new(), "")),
            [
                "Master",
                "Bypass",
                "# Filter",
                "Cutoff",
                "Resonance",
                "# Osc A",
                "Level"
            ]
        );
    }

    #[test]
    fn search_matches_name_or_group_and_layered_ones_are_left_out() {
        let taken = HashSet::from([10]);
        assert_eq!(
            names(&rows(&plugin(), &taken, "filter")),
            ["# Filter", "Resonance"]
        );
        assert_eq!(
            names(&rows(&plugin(), &taken, "osc lev")),
            ["# Osc A", "Level"]
        );
        assert!(rows(&plugin(), &taken, "zzz").is_empty());
    }
}
