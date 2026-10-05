//! Fleet coordination tools for the agentiflow lead: board claim / release /
//! post / read and mailbox send, as [`rupu_tools::Tool`] impls over run-scoped
//! `rupu-fleet` handles.
//!
//! Error discipline: a failed operation on a well-formed call is
//! `Ok(ToolOutput { error: Some(..) })` so the model sees it and can react.
//! `Err(ToolError::InvalidInput)` is reserved for arguments that cannot be
//! parsed -- including an unrecognized `board.post` `kind` (fail closed, never
//! a silent `note`). The runner surfaces such an `Err` to the model as an error
//! `ToolResult` and continues the turn; it does not abort the run.

use async_trait::async_trait;
use rupu_fleet::{Board, BoardPost, ClaimGuard, ClaimOutcome, FleetMessage, Mailbox, PostKind};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Lease a `board.claim` takes on a work unit.
const DEFAULT_CLAIM_TTL: Duration = Duration::from_secs(3600);
/// Per-participant inbox cap for `msg.send`.
const DEFAULT_MSG_CAP: usize = 256;
/// Posts `board.read` returns when `limit` is omitted.
const DEFAULT_READ_LIMIT: usize = 50;
/// Hard ceiling on `board.read`'s `limit`, so one call cannot flood the context.
const MAX_READ_LIMIT: usize = 500;

/// Run-scoped handles the five fleet tools share.
///
/// `ClaimGuard` releases its claim on drop, so a stateless tool call cannot
/// simply return it: granted guards are retained in `claims` (keyed by work
/// unit) for as long as the claim should stand. `board.release` — or dropping
/// the whole context at run end — is what releases them.
pub struct FleetToolCtx {
    board: Arc<Board>,
    mailbox: Arc<Mailbox>,
    participant: String,
    claims: Arc<Mutex<HashMap<String, ClaimGuard>>>,
    msg_cap: usize,
}

impl FleetToolCtx {
    pub fn new(board: Arc<Board>, mailbox: Arc<Mailbox>, participant: impl Into<String>) -> Self {
        Self {
            board,
            mailbox,
            participant: participant.into(),
            claims: Arc::new(Mutex::new(HashMap::new())),
            msg_cap: DEFAULT_MSG_CAP,
        }
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// The claim map. A poisoned lock is recovered: the map only ever holds
    /// guards, and a panic elsewhere must not wedge every later claim.
    fn claims(&self) -> MutexGuard<'_, HashMap<String, ClaimGuard>> {
        self.claims.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The five fleet coordination tools, each holding a clone of `ctx`.
pub fn fleet_tools(ctx: Arc<FleetToolCtx>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BoardClaim(ctx.clone())),
        Arc::new(BoardRelease(ctx.clone())),
        Arc::new(BoardPostTool(ctx.clone())),
        Arc::new(BoardRead(ctx.clone())),
        Arc::new(MsgSend(ctx)),
    ]
}

// ---- argument + output helpers ---------------------------------------------

/// A successful result.
fn done(stdout: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: stdout.into(),
        error: None,
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A store failure the model should see (not a run-aborting `Err`).
fn failed(msg: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg.into()),
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A required, non-blank string argument.
fn req_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match input.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(ToolError::InvalidInput(format!(
            "{key} (non-empty string) required"
        ))),
    }
}

/// An optional string argument: absent, `null` and blank are all `None`; any
/// other non-string value is an error rather than silently ignored.
fn opt_str(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            Ok((!s.is_empty()).then(|| s.to_string()))
        }
        Some(_) => Err(ToolError::InvalidInput(format!(
            "{key} must be a string when given"
        ))),
    }
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

fn kind_str(k: PostKind) -> &'static str {
    match k {
        PostKind::Observation => "observation",
        PostKind::Question => "question",
        PostKind::Answer => "answer",
        PostKind::Vote => "vote",
        PostKind::Note => "note",
    }
}

// ---- board.claim -----------------------------------------------------------

/// `board.claim { work_unit }` -> `granted` | `denied ... held by <holder>`.
struct BoardClaim(Arc<FleetToolCtx>);

#[async_trait]
impl Tool for BoardClaim {
    fn name(&self) -> &'static str {
        "board.claim"
    }

    fn description(&self) -> &'static str {
        "Atomically claim a work unit so no other participant duplicates it. \
         Returns granted, or the current holder when it is already claimed."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["work_unit"],
            "properties": {
                "work_unit": {
                    "type": "string",
                    "description": "Identifier of the unit of work to claim, e.g. host:1.1.2.2"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let key = req_str(&input, "work_unit")?;
        let c = &self.0;
        // Hold the map lock across the claim so two concurrent calls for the
        // same key by this participant cannot both reach the store: the second
        // would be Denied by its own first claim.
        let mut held = c.claims();
        if held.contains_key(key) {
            return Ok(done(format!("granted (already held by you): {key}")));
        }
        match c.board.claim(key, &c.participant, DEFAULT_CLAIM_TTL) {
            Ok(ClaimOutcome::Granted(guard)) => {
                held.insert(key.to_string(), guard);
                Ok(done(format!("granted: {key}")))
            }
            Ok(ClaimOutcome::Denied { holder }) => {
                Ok(done(format!("denied: {key} held by {holder}")))
            }
            Err(e) => Ok(failed(format!("board.claim failed: {e}"))),
        }
    }
}

// ---- board.release ---------------------------------------------------------

/// `board.release { work_unit }` -> drops the retained guard, freeing the claim.
struct BoardRelease(Arc<FleetToolCtx>);

#[async_trait]
impl Tool for BoardRelease {
    fn name(&self) -> &'static str {
        "board.release"
    }

    fn description(&self) -> &'static str {
        "Release a work unit you previously claimed with board.claim, so another \
         participant can take it."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["work_unit"],
            "properties": {
                "work_unit": {
                    "type": "string",
                    "description": "Identifier of the claimed unit of work to release"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let key = req_str(&input, "work_unit")?;
        // Remove under the lock, drop after it is released: dropping the guard
        // is what deletes the claim file.
        let removed = self.0.claims().remove(key);
        Ok(match removed {
            Some(guard) => {
                drop(guard);
                done(format!("released: {key}"))
            }
            None => done(format!("not held by you: {key} (nothing to release)")),
        })
    }
}

// ---- board.post ------------------------------------------------------------

/// `board.post { kind, body, addressed_to? }` -> appends to the board.
struct BoardPostTool(Arc<FleetToolCtx>);

#[async_trait]
impl Tool for BoardPostTool {
    fn name(&self) -> &'static str {
        "board.post"
    }

    fn description(&self) -> &'static str {
        "Post to the shared board every participant can read: an observation, a \
         question, an answer, a vote, or a note. Optionally address it to a \
         participant id or role."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["kind", "body"],
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["observation", "question", "answer", "vote", "note"]
                },
                "body": { "type": "string" },
                "addressed_to": {
                    "type": "string",
                    "description": "Participant id or role this post is for; omit for everyone"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let kind = parse_kind(req_str(&input, "kind")?)?;
        let body = req_str(&input, "body")?.to_string();
        let addressed_to = opt_str(&input, "addressed_to")?;
        let c = &self.0;
        let post = BoardPost {
            author: c.participant.clone(),
            ts: FleetToolCtx::now(),
            kind,
            body,
            addressed_to,
        };
        Ok(match c.board.post(&post) {
            Ok(()) => done(format!("posted [{}]", kind_str(kind))),
            Err(e) => failed(format!("board.post failed: {e}")),
        })
    }
}

// ---- board.read ------------------------------------------------------------

/// `board.read { addressed_to?, limit? }` -> posts, oldest first.
struct BoardRead(Arc<FleetToolCtx>);

#[async_trait]
impl Tool for BoardRead {
    fn name(&self) -> &'static str {
        "board.read"
    }

    fn description(&self) -> &'static str {
        "Read the shared board's posts, oldest first, newest last. Returns the \
         most recent posts (default 50); filter to those addressed to a \
         participant id or role with addressed_to."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
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

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
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
        let mut posts = match self.0.board.read_posts() {
            Ok(p) => p,
            Err(e) => return Ok(failed(format!("board.read failed: {e}"))),
        };
        if let Some(to) = &addressed_to {
            posts.retain(|p| p.addressed_to.as_deref() == Some(to.as_str()));
        }
        if posts.is_empty() {
            return Ok(done("(no posts)"));
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
                    kind_str(p.kind),
                    to,
                    p.body
                )
            })
            .collect();
        Ok(done(lines.join("\n")))
    }
}

// ---- msg.send --------------------------------------------------------------

/// `msg.send { to, body }` -> appends to the recipient's inbox.
struct MsgSend(Arc<FleetToolCtx>);

#[async_trait]
impl Tool for MsgSend {
    fn name(&self) -> &'static str {
        "msg.send"
    }

    fn description(&self) -> &'static str {
        "Send a direct message to one participant's inbox. `to` is a participant \
         id, a role, \"parent\", \"lead\", or \"broadcast\"; it is delivered to \
         exactly the inbox named."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["to", "body"],
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient inbox: participant id, role, parent, lead or broadcast"
                },
                "body": { "type": "string" }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let to = req_str(&input, "to")?;
        let body = req_str(&input, "body")?.to_string();
        let c = &self.0;
        let msg = FleetMessage {
            from: c.participant.clone(),
            ts: FleetToolCtx::now(),
            body,
        };
        Ok(match c.mailbox.send(to, &msg, c.msg_cap) {
            Ok(()) => done(format!("sent to {to}")),
            Err(e) => failed(format!("msg.send failed: {e}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stores(dir: &tempfile::TempDir) -> (Arc<Board>, Arc<Mailbox>) {
        (
            Arc::new(Board::new(dir.path().join("board"))),
            Arc::new(Mailbox::new(dir.path().join("mailboxes"))),
        )
    }

    fn tool(tools: &[Arc<dyn Tool>], name: &str) -> Arc<dyn Tool> {
        tools
            .iter()
            .find(|t| t.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
            .clone()
    }

    #[tokio::test]
    async fn board_post_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(FleetToolCtx::new(
            Arc::new(Board::new(dir.path().join("board"))),
            Arc::new(Mailbox::new(dir.path().join("mailboxes"))),
            "lead",
        ));
        let tools = fleet_tools(ctx.clone());
        let post = tools.iter().find(|t| t.name() == "board.post").unwrap();
        let read = tools.iter().find(|t| t.name() == "board.read").unwrap();
        let tc = rupu_tools::ToolContext::default();
        post.invoke(
            serde_json::json!({"kind":"observation","body":"found an open port"}),
            &tc,
        )
        .await
        .unwrap();
        let out = read.invoke(serde_json::json!({}), &tc).await.unwrap();
        assert!(out.error.is_none(), "{out:?}");
        assert!(out.stdout.contains("found an open port"), "{}", out.stdout);
        assert!(out.stdout.contains("lead"), "author shown: {}", out.stdout);
    }

    #[tokio::test]
    async fn board_claim_is_exclusive_and_releasable() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let first = fleet_tools(Arc::new(FleetToolCtx::new(
            board.clone(),
            mailbox.clone(),
            "lead",
        )));
        let second = fleet_tools(Arc::new(FleetToolCtx::new(board, mailbox, "worker-1")));
        let tc = rupu_tools::ToolContext::default();
        let args = serde_json::json!({"work_unit": "host:1.1.2.2"});

        let out = tool(&first, "board.claim")
            .invoke(args.clone(), &tc)
            .await
            .unwrap();
        assert!(out.error.is_none(), "{out:?}");
        assert!(out.stdout.starts_with("granted"), "{}", out.stdout);

        // The same participant claiming again is told it already holds it.
        let again = tool(&first, "board.claim")
            .invoke(args.clone(), &tc)
            .await
            .unwrap();
        assert!(again.stdout.contains("already held"), "{}", again.stdout);

        // Another participant is denied, and told who holds it.
        let denied = tool(&second, "board.claim")
            .invoke(args.clone(), &tc)
            .await
            .unwrap();
        assert!(denied.error.is_none(), "{denied:?}");
        assert!(denied.stdout.starts_with("denied"), "{}", denied.stdout);
        assert!(denied.stdout.contains("lead"), "{}", denied.stdout);

        // Releasing frees it for the other participant.
        let released = tool(&first, "board.release")
            .invoke(args.clone(), &tc)
            .await
            .unwrap();
        assert!(
            released.stdout.starts_with("released"),
            "{}",
            released.stdout
        );
        let granted = tool(&second, "board.claim")
            .invoke(args.clone(), &tc)
            .await
            .unwrap();
        assert!(granted.stdout.starts_with("granted"), "{}", granted.stdout);

        // Releasing something this participant does not hold is a no-op that
        // must not free the real holder's claim.
        let noop = tool(&first, "board.release")
            .invoke(args.clone(), &tc)
            .await
            .unwrap();
        assert!(noop.stdout.starts_with("not held"), "{}", noop.stdout);
        let still_denied = tool(&first, "board.claim").invoke(args, &tc).await.unwrap();
        assert!(
            still_denied.stdout.starts_with("denied") && still_denied.stdout.contains("worker-1"),
            "{}",
            still_denied.stdout
        );
    }

    #[tokio::test]
    async fn msg_send_then_drain() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let tools = fleet_tools(Arc::new(FleetToolCtx::new(board, mailbox.clone(), "lead")));
        let tc = rupu_tools::ToolContext::default();
        let out = tool(&tools, "msg.send")
            .invoke(
                serde_json::json!({"to":"worker-1","body":"scan the subnet"}),
                &tc,
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let got = mailbox.drain("worker-1").unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].from, "lead");
        assert_eq!(got[0].body, "scan the subnet");
        assert!(!got[0].ts.is_empty());
    }

    #[tokio::test]
    async fn board_post_rejects_unknown_kind() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let tools = fleet_tools(Arc::new(FleetToolCtx::new(board.clone(), mailbox, "lead")));
        let tc = rupu_tools::ToolContext::default();
        let err = tool(&tools, "board.post")
            .invoke(serde_json::json!({"kind":"bogus","body":"x"}), &tc)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "{err:?}");
        // Fail closed: nothing was posted under a defaulted kind.
        assert!(board.read_posts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_arguments_are_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let tools = fleet_tools(Arc::new(FleetToolCtx::new(board, mailbox, "lead")));
        let tc = rupu_tools::ToolContext::default();
        for (name, args) in [
            ("board.claim", serde_json::json!({})),
            ("board.claim", serde_json::json!({"work_unit": "  "})),
            ("board.release", serde_json::json!({"work_unit": 7})),
            ("board.post", serde_json::json!({"body": "x"})),
            ("board.post", serde_json::json!({"kind": "note"})),
            (
                "board.post",
                serde_json::json!({"kind":"note","body":"x","addressed_to":3}),
            ),
            ("board.read", serde_json::json!({"limit": "ten"})),
            ("msg.send", serde_json::json!({"to": "w"})),
            ("msg.send", serde_json::json!({"body": "hi"})),
        ] {
            let err = tool(&tools, name)
                .invoke(args.clone(), &tc)
                .await
                .unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidInput(_)),
                "{name} {args}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn board_read_filters_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let tools = fleet_tools(Arc::new(FleetToolCtx::new(board, mailbox, "lead")));
        let tc = rupu_tools::ToolContext::default();
        let post = tool(&tools, "board.post");
        let read = tool(&tools, "board.read");

        let empty = read.invoke(serde_json::json!({}), &tc).await.unwrap();
        assert_eq!(empty.stdout, "(no posts)");

        for (body, to) in [("one", None), ("two", Some("worker-1")), ("three", None)] {
            let mut args = serde_json::json!({"kind":"note","body":body});
            if let Some(to) = to {
                args["addressed_to"] = serde_json::json!(to);
            }
            post.invoke(args, &tc).await.unwrap();
        }

        let addressed = read
            .invoke(serde_json::json!({"addressed_to":"worker-1"}), &tc)
            .await
            .unwrap();
        assert!(
            addressed.stdout.contains("two") && !addressed.stdout.contains("one"),
            "{}",
            addressed.stdout
        );

        // `limit` keeps the newest posts, still oldest-first.
        let last_two = read
            .invoke(serde_json::json!({"limit": 2}), &tc)
            .await
            .unwrap();
        assert!(!last_two.stdout.contains("one"), "{}", last_two.stdout);
        let (two_at, three_at) = (
            last_two.stdout.find("two").unwrap(),
            last_two.stdout.find("three").unwrap(),
        );
        assert!(two_at < three_at, "{}", last_two.stdout);
    }

    #[tokio::test]
    async fn msg_send_to_full_inbox_is_a_visible_error_not_a_failed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let mut raw = FleetToolCtx::new(board, mailbox, "lead");
        raw.msg_cap = 1;
        let tools = fleet_tools(Arc::new(raw));
        let tc = rupu_tools::ToolContext::default();
        let send = tool(&tools, "msg.send");
        let args = serde_json::json!({"to":"worker-1","body":"hi"});
        assert!(send
            .invoke(args.clone(), &tc)
            .await
            .unwrap()
            .error
            .is_none());
        let full = send.invoke(args, &tc).await.unwrap();
        let msg = full.error.expect("a full inbox is reported to the model");
        assert!(msg.contains("full"), "{msg}");
    }

    #[test]
    fn tool_names_are_stable() {
        let dir = tempfile::tempdir().unwrap();
        let (board, mailbox) = stores(&dir);
        let tools = fleet_tools(Arc::new(FleetToolCtx::new(board, mailbox, "lead")));
        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            [
                "board.claim",
                "board.release",
                "board.post",
                "board.read",
                "msg.send"
            ]
        );
        for t in &tools {
            assert_eq!(t.input_schema()["type"], "object", "{}", t.name());
        }
    }
}
