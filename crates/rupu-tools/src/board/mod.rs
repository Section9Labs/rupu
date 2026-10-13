//! `board.*`: the flow's shared board — claim / release a work unit, post
//! and read, and write or lift a standing directive. They act on the run's
//! [`Bus`] ([`ToolServices::message_bus`](crate::ToolServices::message_bus)):
//! the author of what a run posts is its own participant id.
//!
//! Error discipline: a failed operation on a well-formed call is
//! `Ok(ToolOutput { error: Some(..) })` so the model sees it and can react.
//! `Err(ToolError::InvalidInput)` is reserved for arguments that cannot be
//! parsed — including an unrecognized `board.post` `kind` (fail closed, never
//! a silent `note`).

use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::output::{deny_unknown, failed, ok, ok_json, opt_str, req_str};
use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_fleet::{Address, Bus, BusError, ClaimOutcome, PostKind, SendReceipt};
use serde_json::{json, Value};
use std::sync::Arc;

/// Hard ceiling on `board.read`'s `limit`, so one call cannot flood the context.
pub const MAX_READ_LIMIT: usize = 500;
/// Posts `board.read` returns when `limit` is omitted.
const DEFAULT_READ_LIMIT: usize = 50;

pub static CLAIM: ToolDescriptor = ToolDescriptor {
    name: "board.claim",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    wildcard: true,
    description: "Atomically claim a work unit so no other participant duplicates it. \
     Returns granted, or the current holder when it is already claimed.",
    input_schema: board_claim_schema,
};

fn board_claim_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["work_unit"],
        "properties": {
            "work_unit": {
                "type": "string",
                "description": "Identifier of the unit of work to claim, e.g. host:1.1.2.2"
            }
        }
    })
}

pub static RELEASE: ToolDescriptor = ToolDescriptor {
    name: "board.release",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    wildcard: true,
    description: "Release a work unit you previously claimed with board.claim, so another \
     participant can take it.",
    input_schema: board_release_schema,
};

fn board_release_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["work_unit"],
        "properties": {
            "work_unit": {
                "type": "string",
                "description": "Identifier of the claimed unit of work to release"
            }
        }
    })
}

pub static POST: ToolDescriptor = ToolDescriptor {
    name: "board.post",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    wildcard: true,
    description: "Post to the shared board every participant can read: an observation, a \
     question, an answer, a vote, or a note. Optionally address it to a \
     participant (lynx#1), role:<agent>, step:<id>, parent or lead. Returns the \
     post's `id` and who it was resolved to.",
    input_schema: board_post_schema,
};

fn board_post_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "body"],
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["observation", "question", "answer", "vote", "note"]
            },
            "body": { "type": "string" },
            "addressed_to": {
                "type": "string",
                "description": "Who this post is for: a participant (lynx#1), role:<agent>, step:<id>, parent or lead; omit for everyone"
            }
        }
    })
}

pub static READ: ToolDescriptor = ToolDescriptor {
    name: "board.read",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::MessageBus],
    uses: &[],
    wildcard: true,
    description: "Read the shared board's posts, oldest first, newest last. Returns the \
     most recent posts (default 50); filter to those addressed to a \
     participant id or role with addressed_to.",
    input_schema: board_read_schema,
};

fn board_read_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "addressed_to": {
                "type": "string",
                "description": "Only posts addressed to this participant id or role"
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_READ_LIMIT,
                "description": "Maximum posts to return (the most recent ones); default 50"
            }
        }
    })
}

pub static DIRECTIVE: ToolDescriptor = ToolDescriptor {
    name: "board.directive",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    // Authority over other agents (D22): never granted by `*`.
    wildcard: false,
    description: "Steer the fleet: write a standing directive to the board. Units see it on \
     every turn until you lift it, so post sparingly and keep each short and \
     actionable. Returns the directive's `id`: pass it to `board.retract` once \
     the instruction is obsolete. Omit `addressed_to` to address everyone; name \
     a unit or role to address only it.",
    input_schema: board_directive_schema,
};

fn board_directive_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["body"],
        "properties": {
            "body": { "type": "string", "description": "The directive text" },
            "addressed_to": {
                "type": "string",
                "description": "A participant (lynx#1), role:<agent> or step:<id> to address; omit for everyone"
            }
        }
    })
}

pub static RETRACT: ToolDescriptor = ToolDescriptor {
    name: "board.retract",
    aliases: &[],
    effect: Effect::Record,
    needs: &[Service::MessageBus],
    uses: &[],
    // Authority over other agents (D22): never granted by `*`.
    wildcard: false,
    description: "Lift a standing directive: pass the `id` that `board.directive` returned \
     (it is also shown in brackets on each standing directive you see each \
     turn). Units stop seeing it from their next turn. An unknown or already \
     retracted id is reported and changes nothing.",
    input_schema: board_retract_schema,
};

fn board_retract_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["id"],
        "properties": {
            "id": {
                "type": "string",
                "description": "The `id` `board.directive` returned for the directive to lift"
            }
        }
    })
}

/// The run's message bus. A run without one is never offered these tools
/// (they need [`Service::MessageBus`]).
pub(crate) fn bus(ctx: &ToolContext) -> Result<&Arc<Bus>, ToolError> {
    ctx.services.message_bus.as_ref().ok_or_else(|| {
        ToolError::Execution("this run has no message bus, so board/msg tools can't run".into())
    })
}

/// The address in `input[key]`, if any. A malformed one is invalid input.
pub(crate) fn opt_address(input: &Value, key: &str) -> Result<Option<Address>, ToolError> {
    opt_str(input, key)?
        .map(|s| Address::parse(&s).map_err(|e| ToolError::InvalidInput(e.to_string())))
        .transpose()
}

/// Run a bus write off the async runtime: it holds the space's lock while
/// it reads the roster and the recipients' inboxes.
pub(crate) async fn on_bus<T: Send + 'static>(
    bus: &Arc<Bus>,
    write: impl FnOnce(&Bus) -> Result<T, BusError> + Send + 'static,
) -> Result<T, BusError> {
    let bus = Arc::clone(bus);
    tokio::task::spawn_blocking(move || write(&bus))
        .await
        .unwrap_or_else(|e| {
            Err(BusError::Io {
                action: "message space write".into(),
                source: std::io::Error::other(e.to_string()),
            })
        })
}

/// A write's result: `fields` plus the message's `id` and delivery, and the
/// `message_sent` event the agent loop records. A message that reached none
/// of its recipients is also an error the model sees, though it is logged.
pub(crate) fn sent(receipt: SendReceipt, mut fields: Value) -> ToolOutput {
    let m = receipt.message;
    let missed: Vec<String> = m
        .delivery
        .undelivered
        .iter()
        .map(|u| {
            let reason = serde_json::to_value(u.reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            format!("{} ({reason})", u.to)
        })
        .collect();
    let nobody = m.delivery.resolved.is_empty() && !missed.is_empty();
    let delivery = serde_json::to_value(&m.delivery).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut fields {
        map.insert("id".into(), json!(m.id));
        if let Some(to) = &m.to {
            map.insert("to".into(), json!(to));
        }
        map.insert("delivery".into(), delivery.clone());
    }
    let mut out = ok_json(fields);
    if nobody {
        out.error = Some(format!(
            "{} {} was logged but delivered to nobody: {}",
            m.kind.as_str(),
            m.id,
            missed.join(", ")
        ));
    }
    out.derived = Some(DerivedEvent::MessageSent {
        id: m.id,
        from: m.from,
        from_codename: m.from_codename,
        to: m.to,
        kind: m.kind.as_str().to_string(),
        body: m.body,
        delivery,
    });
    out
}

/// A write that failed. A full log also leaves a `messaging_full` notice on
/// the sender's transcript.
pub(crate) fn not_sent(tool: &str, e: BusError) -> ToolOutput {
    let full = matches!(e, BusError::LogFull { .. });
    let mut out = failed(format!("{tool} failed: {e}"));
    if full {
        out.derived = Some(DerivedEvent::Notice {
            kind: "messaging_full".into(),
            message: e.to_string(),
        });
    }
    out
}

fn parse_kind(s: &str) -> Result<PostKind, ToolError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "observation" => Ok(PostKind::Observation),
        "question" => Ok(PostKind::Question),
        "answer" => Ok(PostKind::Answer),
        "vote" => Ok(PostKind::Vote),
        "note" => Ok(PostKind::Note),
        other => Err(ToolError::InvalidInput(format!(
            "unknown kind `{other}` (expected observation | question | answer | vote | note)"
        ))),
    }
}

/// `board.claim { work_unit }` -> `granted` | `denied ... held by <holder>`.
pub struct BoardClaimTool;

#[async_trait]
impl Tool for BoardClaimTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CLAIM
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["work_unit"])?;
        let key = req_str(&input, "work_unit")?;
        let bus = bus(ctx)?;
        // Hold the table's lock across the claim so two concurrent calls for
        // the same key by this participant cannot both reach the store: the
        // second would be Denied by its own first claim.
        let mut held = bus.claims.lock();
        if held.contains_key(key) {
            return Ok(ok(format!("granted (already held by you): {key}")));
        }
        match bus.board.claim(key, &bus.participant, bus.caps.claim_ttl) {
            Ok(ClaimOutcome::Granted(guard)) => {
                held.insert(key.to_string(), guard);
                Ok(ok(format!("granted: {key}")))
            }
            Ok(ClaimOutcome::Denied { holder }) => {
                Ok(ok(format!("denied: {key} held by {holder}")))
            }
            Err(e) => Ok(failed(format!("board.claim failed: {e}"))),
        }
    }
}

/// `board.release { work_unit }` -> drops the held guard, freeing the claim.
pub struct BoardReleaseTool;

#[async_trait]
impl Tool for BoardReleaseTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &RELEASE
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["work_unit"])?;
        let key = req_str(&input, "work_unit")?;
        // Remove under the lock, drop after it is released: dropping the guard
        // is what deletes the claim file.
        let removed = bus(ctx)?.claims.lock().remove(key);
        Ok(match removed {
            Some(guard) => {
                drop(guard);
                ok(format!("released: {key}"))
            }
            None => ok(format!("not held by you: {key} (nothing to release)")),
        })
    }
}

/// `board.post { kind, body, addressed_to? }` -> `{posted, id, to?, delivery}`.
pub struct BoardPostTool;

#[async_trait]
impl Tool for BoardPostTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &POST
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["kind", "body", "addressed_to"])?;
        let kind = parse_kind(req_str(&input, "kind")?)?;
        let body = req_str(&input, "body")?.to_string();
        let to = opt_address(&input, "addressed_to")?;
        Ok(
            match on_bus(bus(ctx)?, move |b| b.post(kind, to.as_ref(), &body)).await {
                Ok(r) => sent(r, json!({ "posted": kind.as_str() })),
                Err(e) => not_sent("board.post", e),
            },
        )
    }
}

/// `board.read { addressed_to?, limit? }` -> posts, oldest first.
pub struct BoardReadTool;

#[async_trait]
impl Tool for BoardReadTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &READ
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["addressed_to", "limit"])?;
        let addressed_to = opt_str(&input, "addressed_to")?;
        let limit = match input.get("limit") {
            None | Some(Value::Null) => DEFAULT_READ_LIMIT,
            Some(v) => v
                .as_u64()
                .ok_or_else(|| {
                    ToolError::InvalidInput("limit must be a positive integer when given".into())
                })?
                .clamp(1, MAX_READ_LIMIT as u64) as usize,
        };
        let mut posts = match bus(ctx)?.board.read_posts() {
            Ok(p) => p,
            Err(e) => return Ok(failed(format!("board.read failed: {e}"))),
        };
        if let Some(to) = &addressed_to {
            posts.retain(|p| p.addressed_to.as_deref() == Some(to.as_str()));
        }
        if posts.is_empty() {
            return Ok(ok("(no posts)"));
        }
        let skip = posts.len().saturating_sub(limit);
        let lines: Vec<String> = posts
            .iter()
            .skip(skip)
            .map(|p| {
                let to = p
                    .addressed_to
                    .as_deref()
                    .map(|t| format!(" -> {t}"))
                    .unwrap_or_default();
                format!(
                    "{} {} [{}]{}: {}",
                    p.ts,
                    p.author,
                    p.kind.as_str(),
                    to,
                    p.body
                )
            })
            .collect();
        Ok(ok(lines.join("\n")))
    }
}

/// `board.directive { body, addressed_to? }` -> `{ok, id, addressed_to, delivery}`:
/// write a standing directive authored by the caller's participant. The `id`
/// is what `board.retract` takes to lift it again.
pub struct BoardDirectiveTool;

#[async_trait]
impl Tool for BoardDirectiveTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DIRECTIVE
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["body", "addressed_to"])?;
        let body = req_str(&input, "body")?.to_string();
        let to = opt_address(&input, "addressed_to")?;
        let addressed_to = to.as_ref().map(ToString::to_string);
        Ok(
            match on_bus(bus(ctx)?, move |b| b.directive(to.as_ref(), &body)).await {
                Ok(r) => sent(r, json!({ "ok": true, "addressed_to": addressed_to })),
                Err(e) => not_sent("board.directive", e),
            },
        )
    }
}

/// `board.retract { id }` -> `{ok, retracted, id}`: lift a standing
/// directive the caller wrote from the live set. The board log is
/// append-only, so this appends a retraction; an id with no live directive
/// behind it, or a directive another participant wrote, is reported and
/// appends nothing.
pub struct BoardRetractTool;

#[async_trait]
impl Tool for BoardRetractTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &RETRACT
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        deny_unknown(&input, &["id"])?;
        let id = req_str(&input, "id")?.to_string();
        let bus = bus(ctx)?;
        let live = match bus.board.read_directives() {
            Ok(live) => live,
            Err(e) => return Ok(failed(format!("could not read directives: {e}"))),
        };
        let Some(directive) = live.iter().find(|d| d.id == id) else {
            return Ok(failed(format!(
                "no live directive has id {id}: it is unknown or already retracted"
            )));
        };
        // A participant lifts only its own directives: another's standing
        // instruction is not its to withdraw (D22).
        if directive.author != bus.participant {
            return Ok(failed(format!(
                "not the author of directive {id}: it was written by {}, and only its \
                 author can retract it",
                directive.author
            )));
        }
        let lift = id.clone();
        Ok(match on_bus(bus, move |b| b.retract(&lift)).await {
            Ok(r) => sent(r, json!({ "ok": true, "retracted": id })),
            Err(e) => not_sent("board.retract", e),
        })
    }
}
