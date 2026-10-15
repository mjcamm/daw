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

use serde_json::Value;

/// Rewrites every layer in a version 1 or 2 file for version 3.
pub fn centre_blend_modes(file: &mut Value) {
    let Some(nodes) = file.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    for node in nodes {
        let mut params: Vec<&mut Value> = Vec::new();
        let node = match node.as_object_mut() {
            Some(node) => node,
            None => continue,
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
        for param in params {
            let base = param.get("base").and_then(Value::as_f64).unwrap_or(0.0);
            let layers = param.get_mut("layers").and_then(Value::as_array_mut);
            for pair in layers.into_iter().flatten() {
                if let Some(layer) = pair.get_mut(1) {
                    centre_layer(layer, base);
                }
            }
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
                    let (x, w) = block.sample(beat);
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
