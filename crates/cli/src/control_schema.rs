//! Factor identical field constraints without opening an action's closed shape.
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub(super) fn compact(action: &mut Value) {
    let Some(variants) = action.get_mut("oneOf").and_then(Value::as_array_mut) else {
        return;
    };
    let mut candidates: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for variant in variants.iter() {
        // Only the known object/closed-shape form may be factored. Keep any
        // future schema construct intact until its semantics are considered.
        if variant["type"] != "object" || variant["additionalProperties"] != false {
            return;
        }
        let Some(properties) = variant["properties"].as_object() else {
            return;
        };
        let Some(required) = variant["required"].as_array() else {
            return;
        };
        if !properties.contains_key("type")
            || required.len() != properties.len()
            || !properties
                .keys()
                .all(|name| required.iter().any(|field| field.as_str() == Some(name)))
        {
            return;
        }
        for (name, constraint) in properties {
            if name != "type" {
                candidates
                    .entry(name.clone())
                    .or_default()
                    .push(constraint.clone());
            }
        }
    }
    let common: Map<String, Value> = candidates
        .into_iter()
        .filter_map(|(name, constraints)| {
            let first = constraints.first()?;
            constraints
                .iter()
                .all(|c| c == first)
                .then(|| (name, first.clone()))
        })
        .collect();
    for variant in variants.iter_mut() {
        variant.as_object_mut().unwrap().remove("type");
        let count = variant["properties"].as_object().unwrap().len();
        // N required names plus a maximum of N properties is exactly a closed
        // shape. The parent requires the discriminator; branches require every
        // other name. No spare slot remains for an unknown or foreign field.
        variant["required"]
            .as_array_mut()
            .unwrap()
            .retain(|name| name != "type");
        if variant["required"].as_array().unwrap().is_empty() {
            variant.as_object_mut().unwrap().remove("required");
        }
        variant["maxProperties"] = json!(count);
        variant
            .as_object_mut()
            .unwrap()
            .remove("additionalProperties");
        variant["properties"]
            .as_object_mut()
            .unwrap()
            .retain(|name, _| !common.contains_key(name));
    }
    action["type"] = json!("object");
    action["required"] = json!(["type"]);
    action["properties"] = Value::Object(common);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_values_move_but_each_variant_keeps_its_closed_presence_contract() {
        let mut value = json!({"oneOf":[
            {"type":"object","properties":{"type":{"const":"first"},"id":{"type":"string","minLength":1},"index":{"minimum":0}},"required":["type","id","index"],"additionalProperties":false},
            {"type":"object","properties":{"type":{"const":"second"},"id":{"type":"string","minLength":1},"index":{"minimum":-1}},"required":["type","id","index"],"additionalProperties":false}
        ]});
        let original = value.clone();
        compact(&mut value);
        assert_eq!(value["type"], "object");
        assert_eq!(
            value["properties"]["id"],
            original["oneOf"][0]["properties"]["id"]
        );
        assert!(value["properties"].get("index").is_none());
        assert_eq!(value["required"], json!(["type"]));
        for (i, branch) in value["oneOf"].as_array().unwrap().iter().enumerate() {
            assert!(branch["properties"].get("id").is_none());
            assert_eq!(branch["required"], json!(["id", "index"]));
            assert_eq!(branch["maxProperties"], 3);
            assert_eq!(
                branch["properties"]["index"],
                original["oneOf"][i]["properties"]["index"]
            );
        }
    }

    #[test]
    fn unfamiliar_shape_is_left_unchanged() {
        let mut value = json!({"oneOf":[{"type":"object","additionalProperties":true}]});
        let original = value.clone();
        compact(&mut value);
        assert_eq!(value, original);
        let mut optional = json!({"oneOf":[{"type":"object","properties":{"type":{"const":"x"},"optional":{"type":"string"}},"required":["type"],"additionalProperties":false}]});
        let original = optional.clone();
        compact(&mut optional);
        assert_eq!(optional, original);
    }
}
