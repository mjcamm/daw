//! A plugin's parameters on the Parameters page: picked from a searchable
//! list in the plugin's groups, then given layers like any other.

use super::Harness;
use crate::project::Page;
use daw_engine::{
    Command, DeviceId, DeviceRef, Edit, Effect, FIRST_NODE, PluginParamInfo, PluginSpec, TestPlugin,
};

const PLUGIN: DeviceId = DeviceId(900);

fn param(id: u32, name: &str, group: &str, value: f64) -> PluginParamInfo {
    PluginParamInfo {
        id,
        name: name.into(),
        group: group.into(),
        unit: String::new(),
        steps: 0,
        value,
    }
}

/// The Parameters page of a node with a plugin effect listing four
/// parameters in two groups.
fn page_with_plugin(name: &str) -> Harness {
    let mut h = Harness::new(name);
    h.view.update(h.cx, |view, _| {
        view.session.apply_or_panic(Command::Edit(Edit::AddEffect {
            node: FIRST_NODE,
            id: PLUGIN,
            index: 0,
            effect: Effect::plugin(PluginSpec::Test(TestPlugin::Gain)),
        }));
        view.session.show_plugin_params(
            PLUGIN,
            vec![
                param(1, "Mix", "", 1.0),
                param(10, "Cutoff", "Filter", 0.3),
                param(11, "Resonance", "Filter", 0.0),
                param(20, "Drive", "Saturation", 0.5),
            ],
        );
        view.selected_node = Some(FIRST_NODE);
        view.page = Page::Parameters;
    });
    h.redraw_all();
    h
}

fn picking(h: &mut Harness) -> bool {
    h.view.read_with(h.cx, |v, _| v.param_picker.is_some())
}

/// docs/behaviours.md: "+ Add Parameter on a plugin lists its parameters
/// in its groups; search and Enter pick one, then its first layer's type".
#[test]
fn a_plugin_parameter_is_found_by_search_and_gets_a_layer_in_one_undo() {
    let mut h = page_with_plugin("plugin-params");
    let add = h.bounds("add-param-effect-900").center();
    h.click(add, 1);
    assert!(picking(&mut h), "the plugin's list opens");
    h.bounds("param-row-0");
    h.type_text("filter res");

    h.keys("enter");
    assert!(!picking(&mut h), "picking closes the list");
    let drawn = h.bounds("layer-kind-drawn").center();
    h.click(drawn, 1);

    let snapshot = h.snapshot();
    let device = snapshot
        .node(FIRST_NODE)
        .unwrap()
        .devices
        .iter()
        .find(|d| d.device == DeviceRef::Effect(PLUGIN));
    let params: Vec<_> = device
        .unwrap()
        .params
        .iter()
        .map(|p| (p.id.index, p.name.as_str(), p.base, p.layers.len()))
        .collect();
    assert_eq!(params, [(11, "Resonance", 0.0, 1)], "joins with its layer");

    h.view
        .update(h.cx, |view, _| view.session.apply_or_panic(Command::Undo));
    let snapshot = h.snapshot();
    let device = snapshot
        .node(FIRST_NODE)
        .unwrap()
        .devices
        .iter()
        .find(|d| d.device == DeviceRef::Effect(PLUGIN));
    assert!(
        device.unwrap().params.is_empty(),
        "one undo takes both back"
    );
}

/// docs/behaviours.md: "A plugin's parameter list: arrows move, Escape or
/// a click outside closes".
#[test]
fn the_plugin_parameter_list_works_from_the_keys_and_closes() {
    let mut h = page_with_plugin("plugin-params-keys");
    let add = h.bounds("add-param-effect-900").center();
    h.click(add, 1);
    h.keys("escape");
    assert!(!picking(&mut h), "Escape closes it");
    h.view.read_with(h.cx, |v, _| assert_eq!(v.open_menu, None));
    h.click(add, 1);
    h.click(gpui::point(gpui::px(5.), gpui::px(5.)), 1);
    assert!(!picking(&mut h), "a click outside closes it");
    h.view.read_with(h.cx, |v, _| assert_eq!(v.open_menu, None));

    h.click(add, 1);
    // Mix, then (under Filter) Cutoff.
    h.keys("down");
    h.keys("enter");
    let drawn = h.bounds("layer-kind-drawn").center();
    h.click(drawn, 1);
    let snapshot = h.snapshot();
    let device = snapshot
        .node(FIRST_NODE)
        .unwrap()
        .devices
        .iter()
        .find(|d| d.device == DeviceRef::Effect(PLUGIN));
    let param = &device.unwrap().params[0];
    assert_eq!(
        (param.id.index, param.base),
        (10, 0.3),
        "rests where the plugin is"
    );

    // A parameter with layers is not offered again (the button moved down
    // below the new layer).
    let add = h.bounds("add-param-effect-900").center();
    h.click(add, 1);
    h.type_text("cutoff");
    h.keys("enter");
    assert!(picking(&mut h), "nothing to pick: the list stays open");
    h.view.read_with(h.cx, |v, _| {
        assert_eq!(v.pending_param, None, "nothing picked")
    });
}
