//! The operator's side of a message space (spec F1b): which run an id
//! names, whether it can take a message now, what was said in it, and
//! sending into it. The CLI (`rupu message`), the CP and the agentiflow
//! shorthand all come through here, so they read and refuse alike.
//!
//! Liveness reads minimal projections of each root's own record
//! (`agentiflow.json`, `run.json`, `session.json`); the crates that write
//! those records pin the projections in lockstep tests.

use std::path::{Path, PathBuf};

use rupu_fleet::{
    address, Address, Bus, BusCaps, BusError, DirectiveEvent, LogLine, LogMessage, MessageKind,
    OperatorMessage, Refs, Roster, SendReceipt,
};
use serde::{Deserialize, Serialize};

/// The kind of run a message space belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    Agentiflow,
    Workflow,
    Session,
    Standalone,
}

/// A run that owns a message space, as the operator addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRoot {
    pub kind: RootKind,
    pub id: String,
    /// The space's directory: an agentiflow's run dir, else the run's or
    /// session's `messages/`.
    pub space: PathBuf,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RootError {
    #[error("no workflow run, agentiflow, session or run `{0}`")]
    NotFound(String),
}

/// Whether a root can take an operator message now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    Live,
    /// Why not, for the operator.
    NotLive(String),
}

#[derive(Debug, thiserror::Error)]
pub enum OperatorSendError {
    /// The root isn't running: nothing would read the message.
    #[error("{0}")]
    NotLive(String),
    #[error(transparent)]
    Bus(#[from] BusError),
}

/// Which page of a space's messages to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQuery {
    /// Start after this message (from the start when it isn't in the log).
    pub after: Option<String>,
    pub limit: usize,
    /// Only these kinds; every kind when `None`.
    pub kinds: Option<Vec<MessageKind>>,
}

/// What was said in a space: the one wire shape of the CP's messages
/// endpoints, `rupu message list --json` and `rupu __message list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagesView {
    pub participants: Vec<ParticipantView>,
    pub messages: Vec<MessageView>,
    /// An agentiflow from before the log: its board posts and directives,
    /// in the log's shape.
    pub legacy: bool,
    pub space: SpacePresence,
    /// The `after` of the next page; `None` at the end.
    pub next: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpacePresence {
    Present,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantView {
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    pub agent: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub kind: String,
    pub live: bool,
}

/// A log message with the receipts of everyone who saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    #[serde(flatten)]
    pub message: LogMessage,
    #[serde(default)]
    pub seen: Vec<SeenView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenView {
    pub by: String,
    pub turn: u32,
    pub at: String,
}

const AGENTIFLOWS: &str = "agentiflows";
const RUNS: &str = "runs";
/// `RunStore::archive_root`.
const RUNS_ARCHIVE: &str = "runs-archive";
/// `rupu-cli`'s `paths::sessions_dir` / `paths::archived_sessions_dir`.
const SESSIONS: &str = "sessions";
const SESSIONS_ARCHIVE: &str = "sessions-archive";
/// `rupu_orchestrator::AGENT_RUN_PREFIX`: the `workflow_name` a standalone
/// `rupu run` records.
const AGENT_RUN_PREFIX: &str = "agent:";

impl MessageRoot {
    /// The root `id` names exactly: an agentiflow run, a session (active,
    /// else archived), or a run (active, else archived) — a workflow run
    /// when its `run.json` records a workflow, else a standalone `rupu run`
    /// (which writes its record only as it ends).
    pub fn resolve(global: &Path, id: &str) -> Result<Self, RootError> {
        let not_found = || RootError::NotFound(id.to_string());
        if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\']) {
            return Err(not_found());
        }
        let root = |kind, space: PathBuf| MessageRoot {
            kind,
            id: id.to_string(),
            space,
        };
        let flow = global.join(AGENTIFLOWS).join(id);
        if flow.is_dir() {
            return Ok(root(RootKind::Agentiflow, flow));
        }
        for sessions in [SESSIONS, SESSIONS_ARCHIVE] {
            let dir = global.join(sessions).join(id);
            if dir.join("session.json").is_file() {
                return Ok(root(RootKind::Session, dir.join("messages")));
            }
        }
        let run = [RUNS, RUNS_ARCHIVE]
            .iter()
            .map(|runs| global.join(runs).join(id))
            .find(|dir| dir.is_dir())
            .ok_or_else(not_found)?;
        let kind = match read_json::<RunProjection>(&run.join("run.json")) {
            Some(r) if !r.workflow_name.starts_with(AGENT_RUN_PREFIX) => RootKind::Workflow,
            _ => RootKind::Standalone,
        };
        Ok(root(kind, run.join("messages")))
    }

    /// Every id [`MessageRoot::resolve`] could accept: the directory names
    /// under `agentiflows/`, `sessions/`, `runs/` and their archives.
    pub fn candidates(global: &Path) -> Vec<String> {
        [AGENTIFLOWS, SESSIONS, SESSIONS_ARCHIVE, RUNS, RUNS_ARCHIVE]
            .iter()
            .filter_map(|d| std::fs::read_dir(global.join(d)).ok())
            .flat_map(|entries| entries.flatten())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    /// Who a message with no `to` goes to: an agentiflow's lead, everyone
    /// in any other root.
    pub fn default_recipient(&self) -> Address {
        match self.kind {
            RootKind::Agentiflow => Address::Lead,
            _ => Address::Broadcast,
        }
    }

    /// The root's own directory.
    fn dir(&self) -> &Path {
        match self.kind {
            RootKind::Agentiflow => &self.space,
            _ => self.space.parent().unwrap_or(&self.space),
        }
    }

    /// Whether the root is running, so something will read a message sent
    /// now: a running agentiflow or workflow run whose runner is alive (a
    /// record naming no runner counts as alive, as the steer endpoint has
    /// always read it), an active session that isn't stopped, or a
    /// standalone run with no finished record whose own process is alive.
    /// `pid_alive` is the caller's liveness test
    /// (`rupu_orchestrator::runs::pid_is_running`).
    pub fn liveness(&self, global: &Path, pid_alive: &dyn Fn(u32) -> bool) -> Liveness {
        let not = |why: String| Liveness::NotLive(why);
        let id = &self.id;
        match self.kind {
            RootKind::Agentiflow => {
                let Some(r) = read_json::<FlowProjection>(&self.dir().join("agentiflow.json"))
                else {
                    return not(format!("agentiflow `{id}` has no readable record"));
                };
                if r.status != "running" {
                    return not(format!("agentiflow `{id}` is not running ({})", r.status));
                }
                runner(r.runner_pid, pid_alive, || {
                    format!("agentiflow `{id}`'s coordinator")
                })
            }
            RootKind::Workflow => {
                let Some(r) = read_json::<RunProjection>(&self.dir().join("run.json")) else {
                    return not(format!("workflow run `{id}` has no readable record"));
                };
                if !matches!(r.status.as_str(), "running" | "pending") {
                    return not(format!("workflow run `{id}` is not running ({})", r.status));
                }
                runner(r.runner_pid, pid_alive, || {
                    format!("workflow run `{id}`'s runner")
                })
            }
            RootKind::Session => {
                if !self.dir().starts_with(global.join(SESSIONS)) {
                    return not(format!("session `{id}` is archived"));
                }
                match read_json::<SessionProjection>(&self.dir().join("session.json")) {
                    None => not(format!("session `{id}` has no readable record")),
                    Some(s) if s.status == "stopped" => not(format!("session `{id}` is stopped")),
                    Some(_) => Liveness::Live,
                }
            }
            RootKind::Standalone => {
                if let Some(r) = read_json::<RunProjection>(&self.dir().join("run.json")) {
                    if FINISHED.contains(&r.status.as_str()) {
                        return not(format!("run `{id}` has finished ({})", r.status));
                    }
                }
                let roster = Roster::read(&self.space.join("participants.jsonl"));
                let running = roster.ok().is_some_and(|roster| {
                    roster.participants().any(|p| {
                        p.live && p.member.run_id == *id && p.member.pid.is_some_and(pid_alive)
                    })
                });
                if running {
                    Liveness::Live
                } else {
                    not(format!("run `{id}` is not running"))
                }
            }
        }
    }

    /// The space's messages, `q`'s page of them, each with its receipts,
    /// and its participants.
    pub fn view(&self, q: &ViewQuery) -> Result<MessagesView, BusError> {
        let bus = Bus::open(&self.space, address::OPERATOR);
        let has_log = bus.log_path().exists();
        let board = self.space.join("board");
        let legacy = self.kind == RootKind::Agentiflow
            && !has_log
            && (board.join("posts.jsonl").exists() || board.join("directives.jsonl").exists());
        let all = if legacy {
            legacy_messages(&bus)?
        } else {
            fold(bus.read_log(None, usize::MAX)?.lines)
        };
        let participants = bus
            .roster()?
            .participants()
            .map(|p| ParticipantView {
                address: p.address.clone(),
                codename: p.member.codename.clone(),
                agent: p.member.agent.clone(),
                run_id: p.member.run_id.clone(),
                step_id: p.member.step_id.clone(),
                unit: p.member.unit.clone(),
                kind: p.member.kind.clone(),
                live: p.live,
            })
            .collect();
        let present = has_log || legacy || bus.participants_path().exists();
        let (messages, next) = page(all, q);
        Ok(MessagesView {
            participants,
            messages,
            legacy,
            space: if present {
                SpacePresence::Present
            } else {
                SpacePresence::Absent
            },
            next,
        })
    }

    /// Send `msg` into the space, when the root is live
    /// ([`MessageRoot::liveness`]).
    pub fn operator_send(
        &self,
        global: &Path,
        msg: OperatorMessage,
        caps: BusCaps,
        pid_alive: &dyn Fn(u32) -> bool,
    ) -> Result<SendReceipt, OperatorSendError> {
        if let Liveness::NotLive(why) = self.liveness(global, pid_alive) {
            return Err(OperatorSendError::NotLive(why));
        }
        Ok(Bus::open(&self.space, address::OPERATOR)
            .with_caps(caps)
            .operator_send(msg)?)
    }
}

/// `run.json`'s terminal statuses (`RunStatus`).
const FINISHED: [&str; 4] = ["completed", "failed", "rejected", "cancelled"];

/// A recorded runner pid: alive, or none recorded.
fn runner(pid: Option<u32>, pid_alive: &dyn Fn(u32) -> bool, who: impl Fn() -> String) -> Liveness {
    match pid {
        Some(pid) if !pid_alive(pid) => {
            Liveness::NotLive(format!("{} (pid {pid}) is no longer running", who()))
        }
        _ => Liveness::Live,
    }
}

/// What liveness reads of a `run.json` (`rupu_orchestrator::RunRecord`).
#[derive(Deserialize)]
struct RunProjection {
    #[serde(default)]
    workflow_name: String,
    status: String,
    #[serde(default)]
    runner_pid: Option<u32>,
}

/// What liveness reads of an `agentiflow.json`
/// (`rupu_agentiflow::AgentiflowRecord`).
#[derive(Deserialize)]
struct FlowProjection {
    status: String,
    #[serde(default)]
    runner_pid: Option<u32>,
}

/// What liveness reads of a `session.json` (`rupu-cli`'s `SessionRecord`).
#[derive(Deserialize)]
struct SessionProjection {
    status: String,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The log's messages in order, each with the receipts that name it.
fn fold(lines: Vec<LogLine>) -> Vec<MessageView> {
    let mut out: Vec<MessageView> = Vec::new();
    let mut at: std::collections::HashMap<String, usize> = Default::default();
    for line in lines {
        match line {
            LogLine::Msg(message) => {
                at.insert(message.id.clone(), out.len());
                out.push(MessageView {
                    message,
                    seen: Vec::new(),
                });
            }
            LogLine::Seen {
                id,
                by,
                turn,
                at: ts,
            } => {
                if let Some(&i) = at.get(&id) {
                    out[i].seen.push(SeenView { by, turn, at: ts });
                }
            }
        }
    }
    out
}

/// A pre-F1 agentiflow's board as log messages: its posts, and every
/// directive put and retraction, in time order. A line written before ids
/// existed gets one from its position; a retraction (which records neither
/// author nor time) is its directive's author's, at the time of the line
/// before it.
fn legacy_messages(bus: &Bus) -> Result<Vec<MessageView>, BusError> {
    let message = |id: String, ts: String, kind, from: String, to, body: String| MessageView {
        message: LogMessage {
            id,
            ts,
            kind,
            from,
            from_codename: None,
            to,
            post_kind: None,
            body,
            urgent: false,
            refs: Refs::default(),
            delivery: Default::default(),
            via: None,
            viewer: None,
        },
        seen: Vec::new(),
    };
    let mut out: Vec<MessageView> = Vec::new();
    for (n, p) in bus.board.read_posts()?.into_iter().enumerate() {
        let id = p.id.unwrap_or_else(|| format!("legacy_post_{n}"));
        let mut m = message(
            id,
            p.ts,
            MessageKind::Post,
            p.author,
            p.addressed_to,
            p.body,
        );
        m.message.post_kind = Some(p.kind);
        out.push(m);
    }
    let mut authors: std::collections::HashMap<String, String> = Default::default();
    let mut last_ts = String::new();
    for (n, event) in bus.board.directive_events()?.into_iter().enumerate() {
        match event {
            DirectiveEvent::Put(d) => {
                let id = if d.id.trim().is_empty() {
                    format!("legacy_directive_{n}")
                } else {
                    d.id
                };
                authors.insert(id.clone(), d.author.clone());
                last_ts = d.ts.clone();
                out.push(message(
                    id,
                    d.ts,
                    MessageKind::Directive,
                    d.author,
                    d.addressed_to,
                    d.body,
                ));
            }
            DirectiveEvent::Retract { retract } => {
                let from = authors.get(&retract).cloned().unwrap_or_default();
                let mut m = message(
                    format!("legacy_retract_{n}"),
                    last_ts.clone(),
                    MessageKind::Retract,
                    from,
                    None,
                    String::new(),
                );
                m.message.refs.retracts = Some(retract);
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| a.message.ts.cmp(&b.message.ts));
    Ok(out)
}

/// `q`'s page of `all`: the messages of `q`'s kinds after `q.after`, at
/// most `q.limit`, and the `after` of the page that follows.
fn page(all: Vec<MessageView>, q: &ViewQuery) -> (Vec<MessageView>, Option<String>) {
    let start = q
        .after
        .as_deref()
        .and_then(|after| all.iter().position(|m| m.message.id == after))
        .map_or(0, |i| i + 1);
    let mut matching = all
        .into_iter()
        .skip(start)
        .filter(|m| q.kinds.as_ref().is_none_or(|k| k.contains(&m.message.kind)));
    let limit = q.limit.max(1);
    let page: Vec<MessageView> = matching.by_ref().take(limit).collect();
    let next = matching
        .next()
        .and_then(|_| page.last().map(|m| m.message.id.clone()));
    (page, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_fleet::{Member, OperatorVia, PostKind};
    use serde_json::json;

    const ALIVE: u32 = 4242;
    fn alive(pid: u32) -> bool {
        pid == ALIVE
    }

    fn write(path: &Path, value: serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn member(run_id: &str, pid: Option<u32>) -> Member {
        Member {
            agent: "recon".into(),
            run_id: run_id.into(),
            kind: "standalone".into(),
            pid,
            ..Default::default()
        }
    }

    fn live(root: &MessageRoot, g: &Path) -> Result<(), String> {
        match root.liveness(g, &alive) {
            Liveness::Live => Ok(()),
            Liveness::NotLive(why) => Err(why),
        }
    }

    fn to(s: &str, body: &str) -> OperatorMessage {
        OperatorMessage {
            to: Address::parse(s).unwrap(),
            body: body.into(),
            urgent: false,
            via: OperatorVia::Cli,
            viewer: None,
        }
    }

    #[test]
    fn resolve_names_each_kind_of_root() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        std::fs::create_dir_all(g.join("agentiflows/af_1")).unwrap();
        std::fs::write(g.join("agentiflows/flow.toml"), "").unwrap();
        write(
            &g.join("sessions/ses_1/session.json"),
            json!({"status": "idle"}),
        );
        write(
            &g.join("sessions-archive/ses_2/session.json"),
            json!({"status": "idle"}),
        );
        write(
            &g.join("runs/run_wf/run.json"),
            json!({"workflow_name": "review", "status": "running"}),
        );
        write(
            &g.join("runs/run_agent/run.json"),
            json!({"workflow_name": "agent:recon", "status": "running"}),
        );
        std::fs::create_dir_all(g.join("runs/run_bare")).unwrap();
        write(
            &g.join("runs-archive/run_old/run.json"),
            json!({"workflow_name": "review", "status": "completed"}),
        );

        let got = |id: &str| {
            MessageRoot::resolve(g, id).map(|r| {
                (
                    r.kind,
                    r.space.strip_prefix(g).unwrap().display().to_string(),
                )
            })
        };
        assert_eq!(
            got("af_1"),
            Ok((RootKind::Agentiflow, "agentiflows/af_1".into()))
        );
        assert_eq!(
            got("ses_1"),
            Ok((RootKind::Session, "sessions/ses_1/messages".into()))
        );
        assert_eq!(
            got("ses_2"),
            Ok((RootKind::Session, "sessions-archive/ses_2/messages".into()))
        );
        assert_eq!(
            got("run_wf"),
            Ok((RootKind::Workflow, "runs/run_wf/messages".into()))
        );
        assert_eq!(
            got("run_agent"),
            Ok((RootKind::Standalone, "runs/run_agent/messages".into()))
        );
        assert_eq!(
            got("run_bare"),
            Ok((RootKind::Standalone, "runs/run_bare/messages".into()))
        );
        assert_eq!(
            got("run_old"),
            Ok((RootKind::Workflow, "runs-archive/run_old/messages".into()))
        );
        for bad in ["run_none", "", "..", "../runs/run_wf", "flow.toml"] {
            assert_eq!(got(bad), Err(RootError::NotFound(bad.into())), "{bad:?}");
        }
        let mut ids = MessageRoot::candidates(g);
        ids.sort();
        assert_eq!(
            ids,
            [
                "af_1",
                "run_agent",
                "run_bare",
                "run_old",
                "run_wf",
                "ses_1",
                "ses_2"
            ]
        );
        assert_eq!(
            MessageRoot::resolve(g, "af_1").unwrap().default_recipient(),
            Address::Lead
        );
        assert_eq!(
            MessageRoot::resolve(g, "run_wf")
                .unwrap()
                .default_recipient(),
            Address::Broadcast
        );
    }

    #[test]
    fn liveness_of_an_agentiflow_and_a_workflow_run() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        let flow = g.join("agentiflows/af_1/agentiflow.json");
        let run = g.join("runs/run_1/run.json");
        std::fs::create_dir_all(flow.parent().unwrap()).unwrap();
        let af = MessageRoot::resolve(g, "af_1").unwrap();
        assert!(live(&af, g).unwrap_err().contains("no readable record"));
        for (record, expected) in [
            (json!({"status": "running", "runner_pid": ALIVE}), Ok(())),
            (json!({"status": "running", "runner_pid": null}), Ok(())),
            (
                json!({"status": "running", "runner_pid": 7}),
                Err("agentiflow `af_1`'s coordinator (pid 7) is no longer running"),
            ),
            (
                json!({"status": "completed", "runner_pid": ALIVE}),
                Err("agentiflow `af_1` is not running (completed)"),
            ),
        ] {
            write(&flow, record.clone());
            assert_eq!(live(&af, g), expected.map_err(String::from), "{record}");
        }

        for (record, expected) in [
            (
                json!({"workflow_name": "w", "status": "running", "runner_pid": ALIVE}),
                Ok(()),
            ),
            (json!({"workflow_name": "w", "status": "pending"}), Ok(())),
            (
                json!({"workflow_name": "w", "status": "running", "runner_pid": 7}),
                Err("workflow run `run_1`'s runner (pid 7) is no longer running"),
            ),
            (
                json!({"workflow_name": "w", "status": "awaiting_approval"}),
                Err("workflow run `run_1` is not running (awaiting_approval)"),
            ),
        ] {
            write(&run, record.clone());
            let wf = MessageRoot::resolve(g, "run_1").unwrap();
            assert_eq!(wf.kind, RootKind::Workflow);
            assert_eq!(live(&wf, g), expected.map_err(String::from), "{record}");
        }
    }

    #[test]
    fn liveness_of_a_session() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        let record = g.join("sessions/ses_1/session.json");
        for (status, expected) in [
            ("idle", Ok(())),
            ("running", Ok(())),
            ("failed", Ok(())),
            ("stopped", Err("session `ses_1` is stopped")),
        ] {
            write(&record, json!({"status": status}));
            let s = MessageRoot::resolve(g, "ses_1").unwrap();
            assert_eq!(live(&s, g), expected.map_err(String::from), "{status}");
        }
        write(
            &g.join("sessions-archive/ses_2/session.json"),
            json!({"status": "idle"}),
        );
        let archived = MessageRoot::resolve(g, "ses_2").unwrap();
        assert_eq!(
            live(&archived, g),
            Err("session `ses_2` is archived".into())
        );
    }

    #[test]
    fn liveness_of_a_standalone_run() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        std::fs::create_dir_all(g.join("runs/run_1")).unwrap();
        let root = MessageRoot::resolve(g, "run_1").unwrap();
        assert_eq!(live(&root, g), Err("run `run_1` is not running".into()));

        let bus = Bus::open(&root.space, "heron");
        bus.join(member("run_1", Some(ALIVE))).unwrap();
        assert_eq!(live(&root, g), Ok(()));
        // A stub record a launcher wrote ahead of the run doesn't end it.
        write(
            &g.join("runs/run_1/run.json"),
            json!({"workflow_name": "agent:recon", "status": "running"}),
        );
        assert_eq!(live(&root, g), Ok(()));
        write(
            &g.join("runs/run_1/run.json"),
            json!({"workflow_name": "agent:recon", "status": "completed"}),
        );
        assert_eq!(
            live(&root, g),
            Err("run `run_1` has finished (completed)".into())
        );

        std::fs::create_dir_all(g.join("runs/run_2")).unwrap();
        let dead = MessageRoot::resolve(g, "run_2").unwrap();
        Bus::open(&dead.space, "otter")
            .join(member("run_2", Some(7)))
            .unwrap();
        assert!(live(&dead, g).is_err(), "its process is gone");
        Bus::open(&dead.space, "otter")
            .join(member("run_2", None))
            .unwrap();
        assert!(live(&dead, g).is_err(), "no process recorded");
        let left = Bus::open(&dead.space, "otter");
        left.join(member("run_2", Some(ALIVE))).unwrap();
        left.leave("done").unwrap();
        assert!(live(&dead, g).is_err(), "it left");
    }

    #[test]
    fn a_send_to_a_root_that_is_not_live_is_refused_and_not_logged() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        write(
            &g.join("runs/run_1/run.json"),
            json!({"workflow_name": "w", "status": "completed"}),
        );
        let root = MessageRoot::resolve(g, "run_1").unwrap();
        let err = root
            .operator_send(g, to("broadcast", "hi"), BusCaps::default(), &alive)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "workflow run `run_1` is not running (completed)"
        );
        assert!(!root.space.exists());
    }

    #[test]
    fn view_folds_receipts_and_filters_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        write(
            &g.join("runs/run_1/run.json"),
            json!({"workflow_name": "w", "status": "running"}),
        );
        let root = MessageRoot::resolve(g, "run_1").unwrap();
        let q = |kinds: Option<Vec<MessageKind>>, after: Option<String>, limit| ViewQuery {
            after,
            limit,
            kinds,
        };
        let empty = root.view(&q(None, None, 50)).unwrap();
        assert_eq!(empty.space, SpacePresence::Absent);
        assert!(empty.messages.is_empty() && empty.participants.is_empty());

        let lynx = Bus::open(&root.space, "lynx#1");
        lynx.join(Member {
            step_id: Some("scan".into()),
            ..member("r_lynx", Some(ALIVE))
        })
        .unwrap();
        let op = root
            .operator_send(
                g,
                to("lynx#1", "look at 10.0.0.5"),
                BusCaps::default(),
                &alive,
            )
            .unwrap();
        let post = lynx.post(PostKind::Note, None, "on it").unwrap();
        lynx.mark_seen(&[op.id().to_string()], "lynx#1", 3).unwrap();
        let bcast = lynx.send(&Address::Broadcast, "done").unwrap();

        let v = root.view(&q(None, None, 50)).unwrap();
        assert_eq!(v.space, SpacePresence::Present);
        assert!(!v.legacy);
        assert_eq!(v.next, None);
        let ids: Vec<&str> = v.messages.iter().map(|m| m.message.id.as_str()).collect();
        assert_eq!(ids, [op.id(), post.id(), bcast.id()]);
        assert_eq!(v.messages[0].message.kind, MessageKind::Operator);
        assert_eq!(v.messages[0].message.via, Some(OperatorVia::Cli));
        let seen: Vec<(&str, u32)> = v.messages[0]
            .seen
            .iter()
            .map(|s| (s.by.as_str(), s.turn))
            .collect();
        assert_eq!(seen, [("lynx#1", 3)]);
        assert!(v.messages[1].seen.is_empty());
        assert_eq!(
            v.participants,
            vec![ParticipantView {
                address: "lynx#1".into(),
                codename: None,
                agent: "recon".into(),
                run_id: "r_lynx".into(),
                step_id: Some("scan".into()),
                unit: None,
                kind: "standalone".into(),
                live: true,
            }]
        );

        let only = root
            .view(&q(
                Some(vec![MessageKind::Operator, MessageKind::Broadcast]),
                None,
                1,
            ))
            .unwrap();
        assert_eq!(only.messages.len(), 1);
        assert_eq!(only.messages[0].message.id, op.id());
        assert_eq!(only.next.as_deref(), Some(op.id()));
        let rest = root
            .view(&q(
                Some(vec![MessageKind::Operator, MessageKind::Broadcast]),
                only.next,
                1,
            ))
            .unwrap();
        assert_eq!(rest.messages[0].message.id, bcast.id());
        assert_eq!(rest.next, None);

        // One wire shape: a message is its log line plus its receipts.
        let wire = serde_json::to_value(&v).unwrap();
        assert_eq!(wire["space"], "present");
        assert_eq!(wire["messages"][0]["kind"], "operator");
        assert_eq!(wire["messages"][0]["from"], "operator");
        assert_eq!(wire["messages"][0]["seen"][0]["by"], "lynx#1");
        let back: MessagesView = serde_json::from_value(wire).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn a_pre_f1_agentiflow_reads_as_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        let board = g.join("agentiflows/af_old/board");
        std::fs::create_dir_all(&board).unwrap();
        std::fs::write(
            board.join("posts.jsonl"),
            "{\"author\":\"recon#1\",\"ts\":\"2026-01-01T00:00:02Z\",\"kind\":\"observation\",\"body\":\"port 22\"}\n",
        )
        .unwrap();
        std::fs::write(
            board.join("directives.jsonl"),
            "{\"author\":\"lead\",\"ts\":\"2026-01-01T00:00:01Z\",\"body\":\"old rule\"}\n\
             {\"id\":\"01DIR\",\"author\":\"lead\",\"ts\":\"2026-01-01T00:00:03Z\",\"body\":\"hold\",\"addressed_to\":\"recon#1\"}\n\
             {\"retract\":\"01DIR\"}\n",
        )
        .unwrap();
        let root = MessageRoot::resolve(g, "af_old").unwrap();
        let v = root
            .view(&ViewQuery {
                after: None,
                limit: 50,
                kinds: None,
            })
            .unwrap();
        assert!(v.legacy);
        assert_eq!(v.space, SpacePresence::Present);
        let got: Vec<(String, MessageKind, &str, Option<&str>, &str)> = v
            .messages
            .iter()
            .map(|m| {
                let m = &m.message;
                (
                    m.id.clone(),
                    m.kind,
                    m.from.as_str(),
                    m.to.as_deref(),
                    m.body.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "legacy_directive_0".into(),
                    MessageKind::Directive,
                    "lead",
                    None,
                    "old rule"
                ),
                (
                    "legacy_post_0".into(),
                    MessageKind::Post,
                    "recon#1",
                    None,
                    "port 22"
                ),
                (
                    "01DIR".into(),
                    MessageKind::Directive,
                    "lead",
                    Some("recon#1"),
                    "hold"
                ),
                (
                    "legacy_retract_2".into(),
                    MessageKind::Retract,
                    "lead",
                    None,
                    ""
                ),
            ]
        );
        assert_eq!(v.messages[1].message.post_kind, Some(PostKind::Observation));
        assert_eq!(
            v.messages[3].message.refs.retracts.as_deref(),
            Some("01DIR")
        );
    }
}
