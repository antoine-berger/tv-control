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
        for (name, constraint) in properties {
            if name != "type" {
                candidates.entry(name.clone()).or_default().push(constraint.clone());
            }
        }
    }
    let common: Map<String, Value> = candidates
        .into_iter()
        .filter_map(|(name, constraints)| {
            let first = constraints.first()?;
            constraints.iter().all(|c| c == first).then(|| (name, first.clone()))
        })
        .collect();
    for variant in variants.iter_mut() {
        variant.as_object_mut().unwrap().remove("type");
        let properties = variant["properties"].as_object().unwrap();
        let required = variant["required"].as_array();
        if required.is_some_and(|required| {
            required.len() == properties.len()
                && required.iter().all(|name| {
                    name.as_str().is_some_and(|name| properties.contains_key(name))
                })
        }) {
            // A closed shape with N possible keys and at least N keys requires
            // every key, exactly like the full required list. No value or
            // discriminator constraint is removed.
            let count = properties.len();
            variant.as_object_mut().unwrap().remove("required");
            variant["minProperties"] = json!(count);
        }
        // Presence/absence is still decided by each variant's properties,
        // required and additionalProperties:false. The parent validates the
        // value of a present factored field, including in single-use shapes.
        for (name, constraint) in variant["properties"].as_object_mut().unwrap() {
            if common.contains_key(name) {
                *constraint = json!({});
            }
        }
    }
    action["type"] = json!("object");
    action["properties"] = Value::Object(common);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_values_move_but_each_variant_keeps_its_closed_presence_contract() {
        let mut value = json!({"oneOf":[
            {"type":"object","properties":{"type":{"const":"first"},"id":{"type":"string","minLength":1},"index":{"minimum":0}},"required":["type","id","index"],"additionalProperties":false},
            {"type":"object","properties":{"type":{"const":"second"},"id":{"type":"string","minLength":1},"index":{"minimum":-1}},"required":["type","id"],"additionalProperties":false}
        ]});
        let original = value.clone();
        compact(&mut value);
        assert_eq!(value["type"], "object");
        assert_eq!(value["properties"]["id"], original["oneOf"][0]["properties"]["id"]);
        assert!(value["properties"].get("index").is_none());
        for (i, branch) in value["oneOf"].as_array().unwrap().iter().enumerate() {
            assert_eq!(branch["properties"]["id"], json!({}));
            assert_eq!(branch["additionalProperties"], false);
            if i == 0 {
                assert!(branch.get("required").is_none());
                assert_eq!(branch["minProperties"], 3);
            } else {
                assert_eq!(branch["required"], original["oneOf"][i]["required"]);
            }
            assert_eq!(branch["properties"]["index"], original["oneOf"][i]["properties"]["index"]);
        }
    }

    #[test]
    fn unfamiliar_shape_is_left_unchanged() {
        let mut value = json!({"oneOf":[{"type":"object","additionalProperties":true}]});
        let original = value.clone();
        compact(&mut value);
        assert_eq!(value, original);
    }
}
