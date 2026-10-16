//! Converting project files saved by older versions, before they are read.
//!
//! Version 3 centred the blend modes: a layer's lane has a centre line with
//! no effect, the top pushes up and the bottom pulls down. The old modes
//! (Replace, Add, Subtract, Multiply, Modulate) all become Add or Multiply.
//! Each old mode is a straight-line map of the shape value onto the new
//! ones, so points, curves and link ranges convert exactly. Invert is folded
//! into the map.
//!
//! The one approximation is Replace, which depended on the layers below it:
//! it converts exactly when nothing below it is acting, which is how layers
//! were used in practice.
//!
//! Version 4 moved listening to signals from blocks to layers and made notes
//! signals ADSR hits. The first-draft signals never reached the UI, so they
//! convert as closely as the new model allows: a layer whose blocks all link
//! to the same signal listens to it (link ranges are dropped); other links
//! are dropped; an old notes envelope becomes a hit that decays over its
//! length.
//!
//! Version 5 replaced the velocity amount (how much velocity scaled a hit's
//! peak) with a clamp: the peak is the note's velocity held between a
//! minimum and a maximum. Amount `a` becomes the clamp `1 - a ..= 1`, exact
//! at 0 (every hit full) and 1 (the peak is the velocity), and close
//! between: soft notes are lifted to the minimum rather than scaled.
//!
//! Version 6 added switches for the pitch range and the velocity clamp. A
//! range or clamp that does something is switched on; one that lets
//! everything through (all pitches, a clamp of `0..=1`) is switched off.
//!
//! Version 7 made the nodes a tree under the Song. The single node of an
//! older file becomes the Song's first child (id 1), and every notes signal
//! reads its notes. The Song is a plain bus at 0 dB, centred, so old songs
//! sound exactly as before.

use serde_json::Value;

/// Every parameter on every node: instrument, channel and effects.
fn params_mut(file: &mut Value) -> Vec<&mut Value> {
    let mut params: Vec<&mut Value> = Vec::new();
    let Some(nodes) = file.get_mut("nodes").and_then(Value::as_array_mut) else {
        return params;
    };
    for node in nodes {
        let Some(node) = node.as_object_mut() else {
            continue;
        };
        for (key, value) in node.iter_mut() {
            match key.as_str() {
                "instrument" => {
                    if let Some(list) = value.get_mut("params").and_then(Value::as_array_mut) {
                        params.extend(list.iter_mut());
                    }
                }
                "channel" => {
                    if let Some(list) = value.as_array_mut() {
                        params.extend(list.iter_mut());
                    }
                }
                "effects" => {
                    for effect in value.as_array_mut().into_iter().flatten() {
                        if let Some(list) = effect.get_mut("params").and_then(Value::as_array_mut) {
                            params.extend(list.iter_mut());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    params
}

/// Every layer of a parameter.
fn layers_mut(param: &mut Value) -> impl Iterator<Item = &mut Value> {
    let layers = param.get_mut("layers").and_then(Value::as_array_mut);
    layers
        .into_iter()
        .flatten()
        .filter_map(|pair| pair.get_mut(1))
}

/// Rewrites every layer in a version 1 or 2 file for version 3.
pub fn centre_blend_modes(file: &mut Value) {
    for param in params_mut(file) {
        let base = param.get("base").and_then(Value::as_f64).unwrap_or(0.0);
        for layer in layers_mut(param) {
            centre_layer(layer, base);
        }
    }
}

/// Rewrites block links and notes envelopes in a file before version 4.
pub fn listen_to_signals(file: &mut Value) {
    for param in params_mut(file) {
        for layer in layers_mut(param) {
            listen_through_layer(layer);
        }
    }
    let signals = file.get_mut("signals").and_then(Value::as_array_mut);
    for signal in signals.into_iter().flatten() {
        if let Some(notes) = signal.get_mut("source").and_then(|s| s.get_mut("Notes")) {
            hit_from_envelope(notes);
        }
    }
}

/// A layer whose blocks all link to one signal listens to it; every link is
/// removed.
fn listen_through_layer(layer: &mut Value) {
    let blocks = layer.get_mut("blocks").and_then(Value::as_array_mut);
    let mut linked: Vec<Option<Value>> = Vec::new();
    for pair in blocks.into_iter().flatten() {
        let Some(block) = pair.get_mut(1).and_then(Value::as_object_mut) else {
            continue;
        };
        let link = block.remove("link").filter(Value::is_object);
        linked.push(link.and_then(|l| l.get("signal").cloned()));
    }
    if let Some(Some(first)) = linked.first()
        && linked.iter().all(|s| s.as_ref() == Some(first))
    {
        layer["signal"] = first.clone();
    }
}

/// An old notes envelope (a shape over a fixed length) becomes a hit that
/// decays over that length.
fn hit_from_envelope(notes: &mut Value) {
    let Some(notes) = notes.as_object_mut() else {
        return;
    };
    let Some(length) = notes.remove("length") else {
        return;
    };
    notes.remove("shape");
    notes.insert("attack".to_owned(), 0.into());
    notes.insert("decay".to_owned(), length);
    notes.insert("sustain".to_owned(), 0.0.into());
    notes.insert("release".to_owned(), 0.into());
    notes.insert("velocity".to_owned(), 0.0.into());
}

/// Rewrites notes signals in a file before version 5: the velocity amount
/// becomes a clamp.
pub fn clamp_velocity(file: &mut Value) {
    let signals = file.get_mut("signals").and_then(Value::as_array_mut);
    for signal in signals.into_iter().flatten() {
        let notes = signal
            .get_mut("source")
            .and_then(|s| s.get_mut("Notes"))
            .and_then(Value::as_object_mut);
        let Some(notes) = notes else {
            continue;
        };
        let amount = notes
            .remove("velocity")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        notes.insert("velocity_min".to_owned(), (1.0 - amount).into());
        notes.insert("velocity_max".to_owned(), 1.0.into());
    }
}

/// Adds the version 6 switches to every notes signal: on where the range
/// or clamp has an effect.
pub fn notes_switches(file: &mut Value) {
    let signals = file.get_mut("signals").and_then(Value::as_array_mut);
    for signal in signals.into_iter().flatten() {
        let notes = signal
            .get_mut("source")
            .and_then(|s| s.get_mut("Notes"))
            .and_then(Value::as_object_mut);
        let Some(notes) = notes else {
            continue;
        };
        let number =
            |key: &str, default: f64| notes.get(key).and_then(Value::as_f64).unwrap_or(default);
        let ranged = number("lowest", 0.0) > 0.0 || number("highest", 127.0) < 127.0;
        let clamped = number("velocity_min", 0.0) > 0.0 || number("velocity_max", 1.0) < 1.0;
        notes.insert("pitch_range".to_owned(), ranged.into());
        notes.insert("velocity_clamp".to_owned(), clamped.into());
    }
}

/// Turns each node's single `instrument` (before version 8) into a list of
/// one, with an id one past the highest device id in the file.
pub fn instrument_lists(file: &mut Value) {
    fn walk(node: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
        visit(node);
        let children = node.get_mut("children").and_then(Value::as_array_mut);
        for child in children.into_iter().flatten() {
            walk(child, visit);
        }
    }
    let Some(root) = file.get_mut("root") else {
        return;
    };
    let mut highest = 0;
    walk(root, &mut |node| {
        let effects = node.get("effects").and_then(Value::as_array);
        for effect in effects.into_iter().flatten() {
            highest = highest.max(effect.get("id").and_then(Value::as_u64).unwrap_or(0));
        }
    });
    let mut next = highest + 1;
    walk(root, &mut |node| {
        let Some(node) = node.as_object_mut() else {
            return;
        };
        match node.remove("instrument") {
            Some(Value::Object(mut instrument)) => {
                instrument.insert("id".to_owned(), next.into());
                instrument.entry("enabled").or_insert(true.into());
                next += 1;
                node.insert(
                    "instruments".to_owned(),
                    Value::Array(vec![instrument.into()]),
                );
            }
            _ => {
                node.entry("instruments")
                    .or_insert(Value::Array(Vec::new()));
            }
        }
    });
}

/// Puts an older file's nodes under a new Song, and points every notes
/// signal at the first of them.
pub fn nest_under_song(file: &mut Value) {
    let Some(object) = file.as_object_mut() else {
        return;
    };
    let nodes = match object.remove("nodes") {
        Some(Value::Array(nodes)) => nodes,
        _ => Vec::new(),
    };
    let children: Vec<Value> = nodes
        .into_iter()
        .enumerate()
        .map(|(i, mut node)| {
            if let Some(node) = node.as_object_mut() {
                node.insert("id".to_owned(), (i as u64 + 1).into());
            }
            node
        })
        .collect();
    let channel: Vec<Value> = crate::devices::CHANNEL_PARAMS
        .iter()
        .map(|info| serde_json::json!({ "base": info.default_normalized(), "layers": [] }))
        .collect();
    object.insert(
        "root".to_owned(),
        serde_json::json!({
            "id": 0, "name": "Song", "color": crate::project::SONG_COLOR,
            "muted": false, "soloed": false,
            "effects": [], "channel": channel, "clips": [], "children": children
        }),
    );
    let signals = object.get_mut("signals").and_then(Value::as_array_mut);
    for signal in signals.into_iter().flatten() {
        let notes = signal
            .get_mut("source")
            .and_then(|s| s.get_mut("Notes"))
            .and_then(Value::as_object_mut);
        if let Some(notes) = notes {
            notes.entry("node").or_insert(1.into());
        }
    }
}

/// Converts one layer (on a parameter resting at `base`) to a centred mode.
fn centre_layer(layer: &mut Value, base: f64) {
    // New stored value = a · (old shape value, after invert) + b.
    let (a, b, mode) = match layer.get("mode").and_then(Value::as_str) {
        Some("Add") => (0.5, 0.5, "Add"),
        Some("Subtract") => (-0.5, 0.5, "Add"),
        Some("Modulate") => (1.0, 0.0, "Add"),
        Some("Multiply") => (0.5, 0.0, "Multiply"),
        Some("Replace") => (0.5, (1.0 - base) / 2.0, "Add"),
        // Unknown: leave it for the reader to reject.
        _ => return,
    };
    layer["mode"] = mode.into();
    let blocks = layer.get_mut("blocks").and_then(Value::as_array_mut);
    for pair in blocks.into_iter().flatten() {
        let Some(block) = pair.get_mut(1) else {
            continue;
        };
        let invert = block
            .get("invert")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let map = |v: f64| a * (if invert { 1.0 - v } else { v }) + b;
        let points = block
            .get_mut("shape")
            .and_then(|s| s.get_mut("points"))
            .and_then(Value::as_array_mut);
        for point in points.into_iter().flatten() {
            if let Some(v) = point.get("value").and_then(Value::as_f64) {
                point["value"] = map(v).into();
            }
        }
        if let Some(link) = block.get_mut("link").filter(|l| l.is_object()) {
            for end in ["low", "high"] {
                if let Some(v) = link.get(end).and_then(Value::as_f64) {
                    link[end] = map(v).into();
                }
            }
        }
        if block.get("invert").is_some() {
            block["invert"] = false.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_automation::Automation;
    use daw_protocol::{Layer, LayerId};
    use serde_json::json;

    /// What the old blend modes did, for comparison.
    fn old_blend(mode: &str, v: f64, x: f64, w: f64) -> f64 {
        match mode {
            "Replace" => v + (x - v) * w,
            "Add" => v + x * w,
            "Subtract" => v - x * w,
            "Multiply" => v * (1.0 + (x - 1.0) * w),
            "Modulate" => v + (2.0 * x - 1.0) * w,
            _ => unreachable!(),
        }
    }

    /// An old layer: one block over beats 0..4 (3840 ticks) ramping from
    /// 0.2 to 0.9 with a curve, at depth 0.6.
    fn old_layer(mode: &str, invert: bool) -> Value {
        json!({
            "name": "Old",
            "mode": mode,
            "enabled": true,
            "blocks": [[1, {
                "start": 0, "length": 3840, "cycle": null,
                "shape": { "points": [
                    { "at": 0.0, "value": 0.2, "segment": { "Curve": 0.4 } },
                    { "at": 1.0, "value": 0.9, "segment": "Linear" }
                ]},
                "depth": 0.6, "invert": invert, "nudge": 0,
                "fade_in": 0, "fade_out": 0
            }]]
        })
    }

    #[test]
    fn every_old_mode_converts_to_the_same_values() {
        let base = 0.35;
        for mode in ["Replace", "Add", "Subtract", "Multiply", "Modulate"] {
            for invert in [false, true] {
                let old: Layer = serde_json::from_value(
                    old_layer(mode, invert)
                        .as_object()
                        .map(|o| {
                            let mut o = o.clone();
                            o["mode"] = "Add".into();
                            Value::Object(o)
                        })
                        .unwrap(),
                )
                .unwrap();
                let mut converted = old_layer(mode, invert);
                centre_layer(&mut converted, base);
                let new: Layer = serde_json::from_value(converted).unwrap();
                assert!(!new.blocks[0].1.invert);
                let automation = Automation::compile_layers(base, &[(LayerId(1), new)]);
                // The old shape read directly: same block, compiled as-is.
                let block = daw_automation::CompiledBlock::compile(&old.blocks[0].1);
                for beat in [0.0, 0.7, 1.9, 3.2] {
                    let (x, w) = block.sample(beat, None);
                    let expected = old_blend(mode, base, x, w).clamp(0.0, 1.0);
                    let actual = automation.value_at(beat);
                    assert!(
                        (expected - actual).abs() < 1e-9,
                        "{mode} invert {invert} beat {beat}: {expected} vs {actual}"
                    );
                }
            }
        }
    }

    #[test]
    fn link_ranges_convert_with_the_shape() {
        let mut layer = old_layer("Multiply", true);
        layer["blocks"][0][1]["link"] = json!({ "signal": 1, "low": 0.2, "high": 1.0 });
        centre_layer(&mut layer, 0.5);
        let link = &layer["blocks"][0][1]["link"];
        // Inverted, then halved: 1 - 0.2 = 0.8 -> 0.4, and 1 - 1 = 0 -> 0.
        assert!((link["low"].as_f64().unwrap() - 0.4).abs() < 1e-12);
        assert!((link["high"].as_f64().unwrap() - 0.0).abs() < 1e-12);
    }

    fn linked_layer(links: &[Option<u64>]) -> Value {
        let mut layer = old_layer("Add", false);
        let block = layer["blocks"][0].clone();
        layer["blocks"] = json!([]);
        for (i, link) in links.iter().enumerate() {
            let mut pair = block.clone();
            pair[0] = i.into();
            if let Some(signal) = link {
                pair[1]["link"] = json!({ "signal": signal, "low": 0.0, "high": 0.5 });
            }
            layer["blocks"].as_array_mut().unwrap().push(pair);
        }
        layer
    }

    #[test]
    fn a_layer_linked_to_one_signal_listens_to_it() {
        let mut layer = linked_layer(&[Some(4), Some(4)]);
        listen_through_layer(&mut layer);
        assert_eq!(layer["signal"], 4);
        let layer: Layer = serde_json::from_value(layer).unwrap();
        assert_eq!(layer.blocks.len(), 2);
    }

    #[test]
    fn mixed_links_are_dropped() {
        for links in [&[Some(4), None][..], &[Some(4), Some(5)], &[None]] {
            let mut layer = linked_layer(links);
            listen_through_layer(&mut layer);
            assert!(layer.get("signal").is_none(), "{links:?}");
            assert!(layer["blocks"][0][1].get("link").is_none());
            serde_json::from_value::<Layer>(layer).unwrap();
        }
    }

    #[test]
    fn old_notes_envelopes_become_decaying_hits() {
        let mut file = json!({
            "nodes": [],
            "signals": [{
                "id": 1, "name": "Kick", "color": 0,
                "source": { "Notes": {
                    "lowest": 36, "highest": 36, "length": 480,
                    "shape": { "points": [{ "at": 0.0, "value": 0.0, "segment": "Linear" }] }
                }}
            }]
        });
        listen_to_signals(&mut file);
        clamp_velocity(&mut file);
        notes_switches(&mut file);
        nest_under_song(&mut file);
        let source: daw_protocol::NotesSource =
            serde_json::from_value(file["signals"][0]["source"]["Notes"].clone()).unwrap();
        assert_eq!(source.decay, daw_protocol::Ticks(480));
        assert_eq!((source.lowest, source.highest), (36, 36));
        assert_eq!(source.attack, daw_protocol::Ticks::ZERO);
    }

    #[test]
    fn velocity_amounts_become_a_clamp() {
        let notes = |velocity: f64| {
            let mut file = json!({
                "nodes": [],
                "signals": [{
                    "id": 1, "name": "Kick", "color": 0,
                    "source": { "Notes": {
                        "lowest": 36, "highest": 36, "attack": 0, "decay": 480,
                        "sustain": 0.0, "release": 0, "velocity": velocity
                    }}
                }]
            });
            clamp_velocity(&mut file);
            notes_switches(&mut file);
            nest_under_song(&mut file);
            let source: daw_protocol::NotesSource =
                serde_json::from_value(file["signals"][0]["source"]["Notes"].clone()).unwrap();
            (source.velocity_min, source.velocity_max)
        };
        assert_eq!(notes(0.0), (1.0, 1.0), "every hit full");
        assert_eq!(notes(1.0), (0.0, 1.0), "follows velocity");
        assert_eq!(notes(0.25), (0.75, 1.0), "softest hits lifted");
    }

    #[test]
    fn switches_are_on_where_they_had_an_effect() {
        let switches = |lowest: u8, highest: u8, min: f64, max: f64| {
            let mut file = json!({
                "nodes": [],
                "signals": [{
                    "id": 1, "name": "Kick", "color": 0,
                    "source": { "Notes": {
                        "lowest": lowest, "highest": highest, "attack": 0, "decay": 480,
                        "sustain": 0.0, "release": 0,
                        "velocity_min": min, "velocity_max": max
                    }}
                }]
            });
            notes_switches(&mut file);
            nest_under_song(&mut file);
            let source: daw_protocol::NotesSource =
                serde_json::from_value(file["signals"][0]["source"]["Notes"].clone()).unwrap();
            (source.pitch_range, source.velocity_clamp)
        };
        assert_eq!(switches(0, 127, 0.0, 1.0), (false, false));
        assert_eq!(switches(36, 36, 0.0, 1.0), (true, false));
        assert_eq!(switches(0, 127, 1.0, 1.0), (false, true));
    }

    #[test]
    fn old_nodes_go_under_a_new_song() {
        let mut file = json!({
            "nodes": [{ "name": "Lead", "color": 3 }],
            "signals": [{ "id": 1, "name": "Kick", "color": 0, "source": { "Notes": { "lowest": 36 } } }]
        });
        nest_under_song(&mut file);
        assert!(file.get("nodes").is_none());
        let root = &file["root"];
        assert_eq!(
            (root["id"].clone(), root["name"].clone()),
            (json!(0), json!("Song"))
        );
        assert_eq!(root["children"][0]["id"], 1);
        assert_eq!(root["children"][0]["name"], "Lead");
        assert_eq!(root["channel"].as_array().unwrap().len(), 2);
        assert_eq!(file["signals"][0]["source"]["Notes"]["node"], 1);
    }

    #[test]
    fn single_instruments_become_lists_with_fresh_ids() {
        let mut file = json!({
            "root": {
                "id": 0, "effects": [],
                "children": [
                    { "id": 1, "effects": [{ "id": 4 }], "instrument": { "kind": "Synth", "params": [] } },
                    { "id": 2, "effects": [], "children": [] }
                ]
            }
        });
        instrument_lists(&mut file);
        let synth = &file["root"]["children"][0];
        assert!(synth.get("instrument").is_none());
        assert_eq!(
            synth["instruments"][0]["id"], 5,
            "after the highest device id"
        );
        assert_eq!(synth["instruments"][0]["kind"], "Synth");
        assert_eq!(file["root"]["children"][1]["instruments"], json!([]));
    }

    #[test]
    fn layers_are_found_on_every_device() {
        let mut file = json!({
            "nodes": [{
                "instrument": { "params": [{ "base": 0.5, "layers": [[1, old_layer("Replace", false)]] }] },
                "channel": [{ "base": 0.5, "layers": [[2, old_layer("Subtract", false)]] }],
                "effects": [{ "params": [{ "base": 0.5, "layers": [[3, old_layer("Modulate", false)]] }] }]
            }]
        });
        centre_blend_modes(&mut file);
        let node = &file["nodes"][0];
        assert_eq!(
            node["instrument"]["params"][0]["layers"][0][1]["mode"],
            "Add"
        );
        assert_eq!(node["channel"][0]["layers"][0][1]["mode"], "Add");
        assert_eq!(
            node["effects"][0]["params"][0]["layers"][0][1]["mode"],
            "Add"
        );
    }
}
