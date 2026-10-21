//! The device browser: adding instruments and effects, built-in or plugins.

use super::Harness;
use crate::browser::{PluginShelf, Side, Target};
use crate::project::Page;
use daw_engine::{EffectKind, FIRST_NODE, InstrumentKind, PluginSpec};
use daw_plugins::scan::PluginInfo;
use gpui::point;
use std::sync::Arc;

fn plugin(vendor: &str, name: &str, subcategories: &[&str]) -> PluginInfo {
    PluginInfo {
        bundle: format!("C:/VST3/{name}.vst3").into(),
        cid: format!("{name}-cid"),
        name: name.into(),
        vendor: vendor.into(),
        version: "1".into(),
        subcategories: subcategories.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// The mixer, with a few plugins in the catalog.
fn mixer_with_plugins(name: &str) -> Harness {
    let mut h = Harness::new(name);
    let shelf = PluginShelf {
        plugins: Arc::new(vec![
            plugin("Valhalla DSP", "ValhallaDelay", &["Fx", "Delay"]),
            plugin("Valhalla DSP", "ValhallaRoom", &["Fx", "Reverb"]),
            plugin("Xfer Records", "Serum 2", &["Instrument", "Synth"]),
        ]),
        scanning: None,
    };
    h.view.update(h.cx, |view, cx| {
        view.page = Page::Mixer;
        view.set_plugins(shelf, cx);
    });
    h.redraw_all();
    h
}

fn selector(name: &str) -> &'static str {
    Box::leak(format!("{name}-{}", FIRST_NODE.0).into_boxed_str())
}

fn browsing(h: &mut Harness) -> Option<(Target, Side)> {
    h.view.read_with(h.cx, |v, cx| {
        v.browser.as_ref().map(|(b, _)| {
            let b = b.read(cx);
            (b.target(), b.side())
        })
    })
}

/// docs/behaviours.md: "+ Effect opens the device browser; search and
/// Enter add a plugin".
#[test]
fn a_plugin_effect_is_found_by_search_and_added_with_enter() {
    let mut h = mixer_with_plugins("browser-plugin");
    let add = h.bounds(selector("slot-add")).center();
    h.click(add, 1);
    assert_eq!(
        browsing(&mut h),
        Some((Target::Effect(FIRST_NODE), Side::BuiltIn)),
        "opens on Built-in the first time"
    );
    let plugins = h.bounds("browser-side-Plugins").center();
    h.click(plugins, 1);
    h.type_text("room");
    h.keys("enter");

    assert_eq!(browsing(&mut h), None, "adding closes it");
    let snapshot = h.snapshot();
    let node = snapshot.node(FIRST_NODE).unwrap();
    let added = node.devices.iter().find_map(|d| d.plugin.as_ref());
    assert!(
        matches!(
            added.map(|p| &p.spec),
            Some(PluginSpec::Vst3 { name, vendor, .. })
                if name == "ValhallaRoom" && vendor == "Valhalla DSP"
        ),
        "the plugin found is added: {added:?}"
    );
}

/// docs/behaviours.md: "The device browser opens on the side used last;
/// arrows and Enter pick; Escape or a click outside closes".
#[test]
fn the_browser_remembers_its_side_and_works_from_the_keys() {
    let mut h = mixer_with_plugins("browser-keys");
    let add_effect = h.bounds(selector("slot-add")).center();
    h.click(add_effect, 1);
    let plugins = h.bounds("browser-side-Plugins").center();
    h.click(plugins, 1);
    h.keys("escape");
    assert_eq!(browsing(&mut h), None, "Escape closes it");

    let add_instrument = h.bounds(selector("instrument-add")).center();
    h.click(add_instrument, 1);
    assert_eq!(
        browsing(&mut h),
        Some((Target::Instrument(FIRST_NODE), Side::Plugins)),
        "opens on the side used last"
    );
    h.click(point(gpui::px(5.), gpui::px(5.)), 1);
    assert_eq!(browsing(&mut h), None, "a click outside closes it");

    h.click(add_instrument, 1);
    let built_in = h.bounds("browser-side-Built-in").center();
    h.click(built_in, 1);
    let before = h.snapshot().node(FIRST_NODE).unwrap().devices.len();
    h.keys("down");
    h.keys("enter");
    let snapshot = h.snapshot();
    let node = snapshot.node(FIRST_NODE).unwrap();
    assert_eq!(node.devices.len(), before + 1);
    assert!(
        node.devices
            .iter()
            .any(|d| d.instrument == Some(InstrumentKind::SingleSampler)),
        "Down moved from Synth to SingleSampler"
    );
    assert!(
        !node
            .devices
            .iter()
            .any(|d| d.effect == Some(EffectKind::Plugin)),
        "nothing else added"
    );
}
