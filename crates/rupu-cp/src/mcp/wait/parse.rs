//! The `wait` arguments: conditions, cursor and timeout, and the tool's schema.

use std::collections::BTreeMap;
use std::time::Duration;

use rupu_fleet::MessageKind;
use rupu_orchestrator::executor::Event;
use serde_json::{json, Map, Value};

use super::clamp_timeout;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Kind {
    Settled,
    Gate,
    Events { types: Option<Vec<String>> },
    Messages { kinds: Option<Vec<MessageKind>> },
}

impl Kind {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Kind::Settled => "run_settled",
            Kind::Gate => "run_gate",
            Kind::Events { .. } => "run_events",
            Kind::Messages { .. } => "messages",
        }
    }

    /// Whether it collects lines (rather than watching run state).
    pub(super) fn collects(&self) -> bool {
        matches!(self, Kind::Events { .. } | Kind::Messages { .. })
    }
}

/// One source's cursor: a line number (the SSE streams' `id:`), or the id of
/// a message in a remote space.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum CursorValue {
    Line(u64),
    Id(String),
}

#[derive(Debug, Clone)]
pub(super) struct Condition {
    pub(super) kind: Kind,
    /// The run id, or the message root id.
    pub(super) target: String,
    /// `run:<id>` or `messages:<root>`.
    pub(super) source: String,
}

pub(super) struct Request {
    pub(super) watch: Vec<Condition>,
    pub(super) cursor: BTreeMap<String, CursorValue>,
    pub(super) timeout: Duration,
}

pub(super) fn input_schema() -> Value {
    let host = json!({"type": "string", "description": "`local` (the default); remote hosts are not served yet."});
    let run = json!({"type": "string", "description": "A workflow run id."});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["watch"],
        "properties": {
            "watch": {
                "type": "array",
                "minItems": 1,
                "description": "What to wait for. The call returns when any condition fires.",
                "items": {"oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "run"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["run_settled", "run_gate"],
                                "description": "`run_settled`: the run ended and nothing will write to it again. `run_gate`: the run has a gate waiting for approval."},
                            "run": run,
                            "host": host,
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "run"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["run_events"],
                                "description": "New lines of the run's event log."},
                            "run": run,
                            "types": {"type": "array", "items": {"type": "string"},
                                "description": "Only these event types (`step_started`, `run_completed`, …)."},
                            "host": host,
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "root"],
                        "properties": {
                            "kind": {"type": "string", "enum": ["messages"],
                                "description": "New messages in a message space."},
                            "root": {"type": "string",
                                "description": "A run, session or agentiflow id."},
                            "kinds": {"type": "array", "items": {"type": "string",
                                "enum": ["post", "direct", "broadcast", "directive", "retract", "operator"]},
                                "description": "Only these message kinds."},
                            "host": host,
                        }
                    }
                ]}
            },
            "cursor": {
                "type": "object",
                "additionalProperties": {"type": ["integer", "string"], "minimum": 0},
                "description": "The `cursor` a previous call returned: per source, the last line id already seen (the id the SSE streams use; a remote message space's ids are strings)."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "How long to wait (default 30000; a longer one is clamped to 55000). A timeout returns `timed_out: true`, not an error."
            }
        }
    })
}

/// An object's entries without `null`s (agents send `null` for "absent"),
/// refusing a key outside `allowed`.
fn entries(v: Value, allowed: &[&str], what: &str) -> Result<Map<String, Value>, String> {
    let mut o = match v {
        Value::Object(o) => o,
        Value::Null => Map::new(),
        _ if what.is_empty() => return Err("arguments must be an object".into()),
        _ => return Err(format!("{what} must be an object")),
    };
    o.retain(|_, v| !v.is_null());
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(if what.is_empty() {
            format!("unknown argument `{k}`")
        } else {
            format!("{what}: unknown argument `{k}`")
        });
    }
    Ok(o)
}

fn strings(v: Value, key: &str, at: &str) -> Result<Vec<String>, String> {
    let Value::Array(items) = v else {
        return Err(format!("{at}: `{key}` must be an array of strings"));
    };
    items
        .into_iter()
        .map(|i| match i {
            Value::String(s) => Ok(s),
            _ => Err(format!("{at}: `{key}` must be an array of strings")),
        })
        .collect()
}

pub(super) fn parse(args: Value) -> Result<Request, String> {
    let mut o = entries(args, &["watch", "cursor", "timeout_ms"], "")?;
    let watch = match o.remove("watch") {
        Some(Value::Array(w)) if !w.is_empty() => w,
        _ => return Err("`watch` must be a non-empty array of conditions".into()),
    };
    let watch = watch
        .into_iter()
        .enumerate()
        .map(|(i, c)| condition(i, c))
        .collect::<Result<Vec<_>, _>>()?;
    let cursor = match o.remove("cursor") {
        None => BTreeMap::new(),
        Some(Value::Object(c)) => c
            .into_iter()
            .map(|(k, v)| match (&v, v.as_u64()) {
                (_, Some(n)) => Ok((k, CursorValue::Line(n))),
                (Value::String(s), _) => Ok((k, CursorValue::Id(s.clone()))),
                _ => Err(format!(
                    "`cursor.{k}` must be a non-negative integer or a string"
                )),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`cursor` must be an object".into()),
    };
    let timeout = match o.remove("timeout_ms") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .ok_or("`timeout_ms` must be a non-negative integer")?,
        ),
    };
    Ok(Request {
        watch,
        cursor,
        timeout: clamp_timeout(timeout),
    })
}

fn condition(i: usize, c: Value) -> Result<Condition, String> {
    let at = format!("watch[{i}]");
    let kind = c
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!("{at}: `kind` must be one of run_settled, run_gate, run_events, messages")
        })?
        .to_string();
    let allowed: &[&str] = match kind.as_str() {
        "run_settled" | "run_gate" => &["kind", "run", "host"],
        "run_events" => &["kind", "run", "types", "host"],
        "messages" => &["kind", "root", "kinds", "host"],
        other => {
            return Err(format!(
                "{at}: unknown kind `{other}` (run_settled, run_gate, run_events, messages)"
            ))
        }
    };
    let what = format!("{at} (`{kind}`)");
    let mut o = entries(c, allowed, &what)?;
    match o.remove("host") {
        None => {}
        Some(Value::String(h)) if h == "local" => {}
        Some(Value::String(_)) => {
            return Err(format!(
                "{what}: wait on a remote host is not supported yet"
            ));
        }
        Some(_) => return Err(format!("{what}: `host` must be a string")),
    }
    let target_key = if kind == "messages" { "root" } else { "run" };
    let target = match o.remove(target_key) {
        Some(Value::String(s)) if !s.is_empty() => s,
        _ => return Err(format!("{what}: missing required argument `{target_key}`")),
    };
    if let Some(why) = crate::path_guard::unsafe_segment(&target) {
        return Err(format!("{what}: `{target}` is not a valid id ({why})"));
    }
    let kind = match kind.as_str() {
        "run_settled" => Kind::Settled,
        "run_gate" => Kind::Gate,
        "run_events" => Kind::Events {
            types: o
                .remove("types")
                .map(|v| {
                    let types = strings(v, "types", &what)?;
                    match types.iter().find(|t| !is_event_type(t)) {
                        Some(t) => Err(format!("{what}: unknown event type `{t}`")),
                        None => Ok(types),
                    }
                })
                .transpose()?,
        },
        _ => Kind::Messages {
            kinds: o
                .remove("kinds")
                .map(|v| {
                    strings(v, "kinds", &what)?
                        .into_iter()
                        .map(|k| {
                            serde_json::from_value::<MessageKind>(Value::String(k.clone()))
                                .map_err(|_| format!("{what}: unknown message kind `{k}`"))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
        },
    };
    let source = match kind {
        Kind::Messages { .. } => format!("messages:{target}"),
        _ => format!("run:{target}"),
    };
    Ok(Condition {
        kind,
        target,
        source,
    })
}

/// Whether `t` is one of [`Event`]'s `type` tags: an unknown tag reads as
/// [`Event::Unknown`], a known one as its variant (or fails on its missing
/// fields).
fn is_event_type(t: &str) -> bool {
    !matches!(
        serde_json::from_value::<Event>(json!({ "type": t })),
        Ok(Event::Unknown)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::wait::DEFAULT_TIMEOUT;

    #[test]
    fn a_condition_names_its_source() {
        let r = parse(json!({"watch": [
            {"kind": "run_gate", "run": "run_a"},
            {"kind": "messages", "root": "ses_b", "kinds": ["operator", "direct"]}
        ]}))
        .unwrap();
        assert_eq!(r.watch[0].source, "run:run_a");
        assert_eq!(r.watch[1].source, "messages:ses_b");
        assert_eq!(
            r.watch[1].kind,
            Kind::Messages {
                kinds: Some(vec![MessageKind::Operator, MessageKind::Direct])
            }
        );
        assert_eq!(r.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn the_schema_leaves_the_timeout_cap_to_the_server() {
        let schema = input_schema();
        let t = &schema["properties"]["timeout_ms"];
        assert_eq!(t["type"], "integer");
        assert!(t.get("maximum").is_none(), "{t}");
    }

    #[test]
    fn event_types_are_events_own_tags() {
        for t in [
            "run_started",
            "step_started",
            "run_completed",
            "attempt_resumed",
        ] {
            assert!(is_event_type(t), "{t}");
        }
        for t in ["step_warped", "unknown", "StepStarted", ""] {
            assert!(!is_event_type(t), "{t}");
        }
    }

    #[test]
    fn null_fields_are_absent() {
        let r = parse(json!({"watch": [{"kind": "run_events", "run": "run_a",
            "types": null, "host": null}], "cursor": null, "timeout_ms": null}))
        .unwrap();
        assert_eq!(r.watch[0].kind, Kind::Events { types: None });
        assert!(r.cursor.is_empty());
    }

    #[test]
    fn a_cursor_value_is_a_line_number_or_an_id() {
        let r = parse(json!({"watch": [{"kind": "run_gate", "run": "run_a"}],
            "cursor": {"run:run_a": 7, "messages:x": "msg_01"}}))
        .unwrap();
        assert_eq!(r.cursor["run:run_a"], CursorValue::Line(7));
        assert_eq!(r.cursor["messages:x"], CursorValue::Id("msg_01".into()));
        assert!(
            parse(json!({"watch": [{"kind": "run_gate", "run": "run_a"}],
            "cursor": {"run:run_a": -1}}))
            .is_err()
        );
    }
}
