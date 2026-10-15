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
        let source: daw_protocol::NotesSource =
            serde_json::from_value(file["signals"][0]["source"]["Notes"].clone()).unwrap();
        assert_eq!(source.decay, daw_protocol::Ticks(480));
        assert_eq!((source.lowest, source.highest), (36, 36));
        assert_eq!(source.attack, daw_protocol::Ticks::ZERO);
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
