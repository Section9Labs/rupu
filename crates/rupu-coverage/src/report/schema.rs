//! The finding-report JSON Schema, embedded in the binary.
//!
//! `canonical_schema()` is the contract (what `rupu findings schema` prints
//! and what the lockstep test checks the validator against).
//! `advertised_schema()` is the copy put in a tool definition: providers
//! differ in which JSON Schema keywords their tool-calling accepts, so it
//! keeps only the widely supported subset. Dropping keywords there loses
//! nothing — `validate_report` enforces the full contract on every write.
//! It also omits the fields rupu fills in itself (`verification`, each
//! claim's `sha256`, and every artifact field but `path`): stored records
//! carry them, but an agent is never asked for them.

use serde_json::Value;

pub const FINDING_REPORT_SCHEMA: &str = include_str!("../../schema/finding_report.schema.json");

pub fn canonical_schema() -> Value {
    serde_json::from_str(FINDING_REPORT_SCHEMA)
        .expect("embedded finding report schema is valid JSON")
}

pub fn advertised_schema() -> Value {
    let mut v = canonical_schema();
    simplify_schema(&mut v);
    strip_rupu_owned_fields(&mut v);
    v
}

/// Remove the fields rupu sets at write time (or a later verification run
/// sets) from the copy an agent sees. The canonical schema keeps them,
/// because stored records carry them.
fn strip_rupu_owned_fields(schema: &mut Value) {
    let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    props.remove("verification");
    if let Some(claim) = props
        .get_mut("evidence")
        .and_then(|e| e.get_mut("items"))
        .and_then(|i| i.get_mut("properties"))
        .and_then(Value::as_object_mut)
    {
        claim.remove("sha256");
    }
    if let Some(artifact) = props
        .get_mut("artifacts")
        .and_then(|a| a.get_mut("items"))
        .and_then(|i| i.get_mut("properties"))
        .and_then(Value::as_object_mut)
    {
        artifact.retain(|k, _| k == "path");
    }
}

/// Keywords removed from the advertised copy.
const DROP: &[&str] = &[
    "$schema",
    "$id",
    "pattern",
    "minLength",
    "additionalProperties",
];

/// Walk a schema node. Only schema *keywords* are rewritten; the keys of a
/// `properties` map are property names and are left alone.
fn simplify_schema(node: &mut Value) {
    let Value::Object(map) = node else { return };
    for k in DROP {
        map.remove(*k);
    }
    if let Some(one_of) = map.remove("oneOf") {
        map.insert("anyOf".to_string(), one_of);
    }
    if let Some(Value::Object(props)) = map.get_mut("properties") {
        for child in props.values_mut() {
            simplify_schema(child);
        }
    }
    if let Some(items) = map.get_mut("items") {
        simplify_schema(items);
    }
    if let Some(Value::Array(alts)) = map.get_mut("anyOf") {
        for alt in alts.iter_mut() {
            simplify_schema(alt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk_keys(v: &serde_json::Value, found: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, child) in m {
                    found.push(k.clone());
                    walk_keys(child, found);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|c| walk_keys(c, found)),
            _ => {}
        }
    }

    #[test]
    fn canonical_schema_parses() {
        let s = canonical_schema();
        assert_eq!(s["type"], "object");
        assert!(s["required"].as_array().unwrap().len() >= 20);
    }

    #[test]
    fn advertised_schema_drops_provider_unsafe_keywords() {
        let mut keys = Vec::new();
        walk_keys(&advertised_schema(), &mut keys);
        for banned in [
            "oneOf",
            "pattern",
            "minLength",
            "additionalProperties",
            "$schema",
            "$id",
        ] {
            assert!(!keys.iter().any(|k| k == banned), "{banned} survived");
        }
        assert!(keys.iter().any(|k| k == "anyOf"), "oneOf must become anyOf");
    }

    #[test]
    fn advertised_schema_omits_fields_rupu_sets() {
        let s = advertised_schema();
        assert!(s["properties"].get("verification").is_none());
        assert!(s["properties"]["evidence"]["items"]["properties"]
            .get("sha256")
            .is_none());
        let artifact: Vec<&String> = s["properties"]["artifacts"]["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(artifact, vec!["path"]);
        // The canonical schema still describes stored records in full.
        let c = canonical_schema();
        assert!(c["properties"]["verification"].is_object());
        assert!(c["properties"]["evidence"]["items"]["properties"]["sha256"].is_object());
        assert!(c["properties"]["artifacts"]["items"]["properties"]["stored"].is_object());
    }

    #[test]
    fn advertised_schema_keeps_property_names() {
        // `title` is both a JSON Schema keyword and a report property; the
        // property must survive.
        let s = advertised_schema();
        assert!(s["properties"]["title"].is_object());
        assert!(s["properties"]["regression_test"]["anyOf"].is_array());
    }
}
