//! Codename helpers for DTOs: stored name wins, legacy records get a derived
//! one (spec §6) flagged with `codename_derived`.

/// Stored codename, or a derived one for a legacy record. Returns (name, derived).
pub fn named(stored: Option<&str>, id: &str, agent: Option<&str>) -> (String, bool) {
    match stored.filter(|s| !s.is_empty()) {
        Some(s) => (s.to_string(), false),
        None => (rupu_codename::derive_legacy(id, agent), true),
    }
}

/// For pass-through JSON objects: if `obj["codename"]` is absent/null, set it to
/// `derive_legacy(id, agent)` and set `codename_derived: true`; else set
/// `codename_derived: false`. No-op on non-objects.
pub fn inject_codename(obj: &mut serde_json::Value, id: &str, agent: Option<&str>) {
    let Some(map) = obj.as_object_mut() else {
        return;
    };
    let stored = map.get("codename").and_then(|v| v.as_str());
    // A row that already carries both keys (a newer remote, or one of our own
    // rows that was itself derived) is authoritative: keep its flag.
    if stored.is_some() && map.get("codename_derived").is_some_and(|v| v.is_boolean()) {
        return;
    }
    let (name, derived) = named(stored, id, agent);
    map.insert("codename".into(), serde_json::Value::String(name));
    map.insert("codename_derived".into(), serde_json::Value::Bool(derived));
}

/// [`inject_codename`] for a row that carries its own id (a row from a possibly
/// older remote). `id_key` names the id field; `agent_key` a field holding the
/// agent name.
pub fn inject_codename_row(obj: &mut serde_json::Value, id_key: &str, agent_key: Option<&str>) {
    let Some(id) = obj.get(id_key).and_then(|v| v.as_str()).map(str::to_string) else {
        return;
    };
    let agent = agent_key.and_then(|k| obj.get(k).and_then(|v| v.as_str()).map(str::to_string));
    inject_codename(obj, &id, agent.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stored_wins_else_derived() {
        assert_eq!(
            named(Some("cobalt-harbor"), "run_X", None),
            ("cobalt-harbor".into(), false)
        );
        assert_eq!(
            named(None, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None),
            ("jade-reef".into(), true)
        );
        assert_eq!(
            named(None, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", Some("triage")),
            ("jade-reef/numbat".into(), true)
        );
    }
    #[test]
    fn inject_fills_only_missing() {
        let mut a = serde_json::json!({"id": "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"});
        inject_codename(&mut a, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None);
        assert_eq!(a["codename"], "jade-reef");
        assert_eq!(a["codename_derived"], true);
        let mut b = serde_json::json!({"codename": "cobalt-harbor"});
        inject_codename(&mut b, "run_X", None);
        assert_eq!(b["codename"], "cobalt-harbor");
        assert_eq!(b["codename_derived"], false);
        // Already-flagged rows keep their flag (a derived name stays derived).
        let mut d = serde_json::json!({"codename": "jade-reef", "codename_derived": true});
        inject_codename(&mut d, "run_X", None);
        assert_eq!(d["codename_derived"], true);
        let mut n = serde_json::json!("x");
        inject_codename(&mut n, "run_X", None);
        assert_eq!(n, "x");
    }
    #[test]
    fn row_variant_reads_id() {
        let mut a = serde_json::json!({"id": "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"});
        inject_codename_row(&mut a, "id", None);
        assert_eq!(a["codename"], "jade-reef");
    }
}
