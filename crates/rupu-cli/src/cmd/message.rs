//! `rupu message`: the operator's side of a run's message space (spec F1b
//! §3.4), and the hidden `rupu __message` an SSH coordinator runs on a remote
//! host (§3.6). Both are thin over `rupu_runtime::services::operator`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context as _};
use clap::{Args, Subcommand};
use rupu_fleet::{Address, OperatorMessage, OperatorVia, SendReceipt, Undelivered};
use rupu_runtime::services::operator::{
    parse_kinds, Liveness, MessageError, MessageErrorCode, MessageErrorReply, MessageRoot,
    MessageView, MessagesView, OperatorSendError, OperatorSendReply, ParticipantView, ViewQuery,
};

use crate::output::formats::{self, OutputFormat};
use crate::output::palette;
use crate::output::report as output_report;
use crate::paths;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Show what was said in a run's message space.
    ///
    /// One line per message (`time from → to [kind] body`), then, indented,
    /// whom it could not reach and who saw it on which turn. `--follow` keeps
    /// printing new messages and receipts until the run stops, then says why;
    /// Ctrl-C leaves the run going. `--json` prints the whole view as one
    /// object or, with `--follow`, one message per line (printed again when
    /// its receipts change).
    List(ListArgs),
    /// Send a message into a running run's message space, as its operator.
    ///
    /// It goes to `--to` (a participant, a codename, `role:<agent>`,
    /// `step:<id>`, `lead` or `broadcast`); by default to an agentiflow's lead,
    /// and to everyone in any other run. Each recipient gets it before its
    /// next model call. `--urgent` puts it first, and ends an agentiflow
    /// lead's round early. A run that isn't running takes no message.
    Send(SendArgs),
    /// List the participants in a run's message space.
    Participants {
        /// Run id: a workflow run, an agentiflow (`af_...`), a session
        /// (`ses_...`) or a standalone run; the full id, the compact form a
        /// list prints, or a unique prefix / suffix of it.
        #[arg(value_name = "RUN_ID")]
        id: String,
        /// Print the participants as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Run id: a workflow run, an agentiflow (`af_...`), a session
    /// (`ses_...`) or a standalone run; the full id, the compact form a list
    /// prints, or a unique prefix / suffix of it.
    #[arg(value_name = "RUN_ID")]
    pub id: String,
    /// Only the messages after this one (`msg_...`).
    #[arg(long, value_name = "MSG_ID")]
    pub after: Option<String>,
    /// Keep printing new messages and receipts until the run stops.
    #[arg(long)]
    pub follow: bool,
    /// Only these kinds, comma-separated: post, direct, broadcast, directive,
    /// retract, operator.
    #[arg(long, value_name = "KINDS")]
    pub kind: Option<String>,
    /// Print JSON: the whole view, or with `--follow` one message per line.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SendArgs {
    /// Run id: a workflow run, an agentiflow (`af_...`), a session
    /// (`ses_...`) or a standalone run; the full id, the compact form a list
    /// prints, or a unique prefix / suffix of it.
    #[arg(value_name = "RUN_ID")]
    pub id: String,
    /// Who the message is for: a participant, a codename, `role:<agent>`,
    /// `step:<id>`, `lead` or `broadcast`.
    #[arg(long, value_name = "ADDRESS")]
    pub to: Option<String>,
    /// Put the message first; an agentiflow lead ends its round early.
    #[arg(long)]
    pub urgent: bool,
    /// The message (its words are joined with spaces).
    #[arg(required = true, num_args = 1.., value_name = "TEXT")]
    pub text: Vec<String>,
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List(_) => ("message list", output_report::TABLE_JSON),
        Action::Send(_) => ("message send", output_report::TABLE_ONLY),
        Action::Participants { .. } => ("message participants", output_report::TABLE_JSON),
    };
    formats::ensure_supported(command_name, format, supported)
}

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let format_json = global_format == Some(OutputFormat::Json);
    let result = match action {
        Action::List(args) => {
            let json = args.json || format_json;
            list_cmd(args, json).await
        }
        Action::Send(args) => send_cmd(&args),
        Action::Participants { id, json } => {
            participants_cmd(&id, json || format_json, absolute, all_columns)
        }
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(format!("{e:#}")),
    }
}

/// The root `fragment` names: an exact id, else the compact form, a prefix or
/// a suffix of exactly one.
fn resolve_root(global: &Path, fragment: &str) -> anyhow::Result<MessageRoot> {
    use crate::output::ids::{self, Resolution};
    if let Ok(root) = MessageRoot::resolve(global, fragment) {
        return Ok(root);
    }
    match ids::resolve(&MessageRoot::candidates(global), fragment) {
        Resolution::Unique(id) => Ok(MessageRoot::resolve(global, &id)?),
        Resolution::NotFound => Err(anyhow!(
            "no run matches '{fragment}' (no workflow run, agentiflow, session or run under {})",
            global.display()
        )),
        Resolution::Ambiguous(matches) => Err(anyhow!(
            "`{fragment}` matches more than one run: {}",
            matches.join(", ")
        )),
    }
}

/// The message-space caps from the layered config, read the way a listing
/// reads it (a malformed file degrades with a warning).
pub(crate) fn messaging_caps(global: &Path) -> anyhow::Result<rupu_fleet::BusCaps> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = paths::load_config_for_display(global, project_root.as_deref(), &pwd, true);
    Ok(rupu_runtime::services::messaging::caps(&cfg))
}

/// One `not delivered to <x>: <reason>` line per recipient `receipt` missed.
pub(crate) fn undelivered_lines(receipt: &SendReceipt) -> Vec<String> {
    receipt
        .delivery()
        .undelivered
        .iter()
        .map(undelivered_line)
        .collect()
}

fn undelivered_line(u: &Undelivered) -> String {
    let reason = serde_json::to_value(u.reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    format!("not delivered to {}: {reason}", u.to)
}

// ---- `send` -----------------------------------------------------------------

fn send(global: &Path, args: &SendArgs, caps: rupu_fleet::BusCaps) -> anyhow::Result<String> {
    let root = resolve_root(global, &args.id)?;
    let to = match &args.to {
        Some(to) => Address::parse(to)?,
        None => root.default_recipient(),
    };
    let msg = OperatorMessage {
        to: to.clone(),
        body: args.text.join(" "),
        urgent: args.urgent,
        via: OperatorVia::Cli,
        viewer: None,
        client: None,
    };
    let receipt = root
        .operator_send(global, msg, caps, &rupu_orchestrator::runs::pid_is_running)
        .map_err(|e| match e {
            OperatorSendError::Invalid(why) => anyhow!(why),
            OperatorSendError::NotLive(why) => anyhow!("{why}: it takes no message"),
            e => anyhow!(e).context(format!("send to {to} in {}", root.id)),
        })?;
    let mut lines = vec![format!("sent {} to {to}", receipt.id())];
    lines.extend(undelivered_lines(&receipt));
    Ok(lines.join("\n"))
}

fn send_cmd(args: &SendArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let caps = messaging_caps(&global)?;
    println!("{}", send(&global, args, caps)?);
    Ok(())
}

// ---- `list` -----------------------------------------------------------------

/// How often `list --follow` looks at the log again: pure filesystem polling.
const FOLLOW_POLL: Duration = Duration::from_millis(250);

async fn list_cmd(args: ListArgs, json: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let root = resolve_root(&global, &args.id)?;
    let kinds = args
        .kind
        .as_deref()
        .map(parse_kinds)
        .transpose()
        .map_err(|e| anyhow!(e))?;
    let query = ViewQuery {
        after: args.after.clone(),
        limit: usize::MAX,
        kinds,
    };
    let view = |root: &MessageRoot| {
        root.view(&query)
            .with_context(|| format!("read the messages of {}", root.id))
    };
    if !args.follow {
        let view = view(&root)?;
        if json {
            return formats::print_json(&view);
        }
        print!("{}", render_view(&view));
        return Ok(());
    }

    let mut follow = Follow::default();
    let mut stamp = None;
    let mut first = true;
    loop {
        // Liveness first: a message logged before the root stopped is still
        // read below, so the last one is never lost.
        let live = root.liveness(&global, &rupu_orchestrator::runs::pid_is_running);
        let now = log_stamp(&root.log_path());
        if first || now != stamp {
            let chunk = follow.next(&view(&root)?, json)?;
            let mut out = std::io::stdout().lock();
            out.write_all(chunk.as_bytes())?;
            out.flush()?;
            stamp = now;
            first = false;
        }
        if let Liveness::NotLive(why) = live {
            eprintln!("{why}");
            return Ok(());
        }
        tokio::time::sleep(FOLLOW_POLL).await;
    }
}

/// The log's size and mtime: `--follow` reads the log again only when it
/// changed.
fn log_stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// What `--follow` has printed: each message's id and how many of its
/// receipts.
#[derive(Default)]
struct Follow {
    printed: HashMap<String, usize>,
}

impl Follow {
    /// What `view` adds to what was printed: new messages in full, and new
    /// receipts of earlier ones (as JSON, the message again).
    fn next(&mut self, view: &MessagesView, json: bool) -> anyhow::Result<String> {
        let mut out = String::new();
        for m in &view.messages {
            let seen = m.seen.len();
            match self.printed.get(&m.message.id).copied() {
                None => {
                    if json {
                        writeln!(out, "{}", serde_json::to_string(m)?)?;
                    } else {
                        render_message(m, &mut out);
                    }
                }
                Some(n) if seen > n => {
                    if json {
                        writeln!(out, "{}", serde_json::to_string(m)?)?;
                    } else {
                        for s in &m.seen[n..] {
                            dim_line(
                                &mut out,
                                &format!("seen by {} (turn {}): {}", s.by, s.turn, m.message.id),
                            );
                        }
                    }
                }
                Some(_) => continue,
            }
            self.printed.insert(m.message.id.clone(), seen);
        }
        Ok(out)
    }
}

fn render_view(view: &MessagesView) -> String {
    let mut out = String::new();
    if view.legacy {
        dim_line(
            &mut out,
            "(an agentiflow from before the message log: its board posts and directives)",
        );
    }
    if view.messages.is_empty() {
        out.push_str("(no messages)\n");
    }
    for m in &view.messages {
        render_message(m, &mut out);
    }
    out
}

/// `HH:MM:SS from → to [kind][ URGENT] body`, a multi-line body's other lines
/// indented, then (dimmed, indented) whom it didn't reach and who saw it.
fn render_message(m: &MessageView, out: &mut String) {
    let msg = &m.message;
    let to = match (&msg.to, &msg.refs.retracts) {
        (Some(to), _) => to.clone(),
        (None, Some(id)) => format!("retracts {id}"),
        (None, None) => "board".to_string(),
    };
    let _ = palette::write_colored(out, &clock(&msg.ts), palette::DIM);
    let _ = write!(out, " {} → {to} [{}]", msg.from, msg.kind.as_str());
    if msg.urgent {
        out.push(' ');
        let _ = palette::write_bold_colored(out, "URGENT", palette::AWAITING);
    }
    let mut lines = msg.body.lines();
    if let Some(first) = lines.next() {
        let _ = write!(out, " {first}");
    }
    out.push('\n');
    for line in lines {
        let _ = writeln!(out, "    {line}");
    }
    for u in &msg.delivery.undelivered {
        dim_line(out, &undelivered_line(u));
    }
    for s in &m.seen {
        dim_line(out, &format!("seen by {} (turn {})", s.by, s.turn));
    }
}

fn dim_line(out: &mut String, text: &str) {
    out.push_str("    ");
    let _ = palette::write_colored(out, text, palette::DIM);
    out.push('\n');
}

/// A log timestamp as local `HH:MM:SS`; the raw value when it doesn't parse.
fn clock(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| ts.to_string())
}

// ---- `participants` ---------------------------------------------------------

fn participants_cmd(
    fragment: &str,
    json: bool,
    absolute: bool,
    all_columns: bool,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let root = resolve_root(&global, fragment)?;
    let view = root
        .view(&ViewQuery {
            after: None,
            limit: 1,
            kinds: None,
        })
        .with_context(|| format!("read the participants of {}", root.id))?;
    if json {
        return formats::print_json(&view.participants);
    }
    if view.participants.is_empty() {
        println!(
            "(no participants: nothing in {} has joined its message space)",
            root.id
        );
        return Ok(());
    }
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = paths::load_config_for_display(&global, project_root.as_deref(), &pwd, true);
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None, None)
        .with_table_flags(absolute, all_columns);
    println!("{}", render_participants(&view.participants, &prefs));
    Ok(())
}

fn render_participants(rows: &[ParticipantView], prefs: &crate::cmd::ui::UiPrefs) -> String {
    use crate::output::entity_table::{CellValue, EntityTable};
    let mut table = EntityTable::new(
        prefs,
        prefs.render_opts(),
        vec!["ADDRESS", "AGENT", "KIND", "STEP/UNIT", "LIVE"],
    )
    .with_summary("participant");
    for p in rows {
        let place = match (&p.step_id, &p.unit) {
            (Some(step), Some(unit)) => Some(format!("{step}/{unit}")),
            (Some(step), None) => Some(step.clone()),
            (None, Some(unit)) => Some(unit.clone()),
            (None, None) => None,
        };
        table = table.row(vec![
            CellValue::Name(p.address.clone()),
            CellValue::Name(p.agent.clone()),
            CellValue::Text(p.kind.clone()),
            place.map(CellValue::Text).unwrap_or(CellValue::Missing),
            CellValue::Text(if p.live { "yes" } else { "left" }.into()),
        ]);
    }
    table.render(chrono::Utc::now())
}

// ---- `__message` ------------------------------------------------------------

/// The hidden `rupu __message` (spec F1b §3.6): what an SSH coordinator runs
/// on a remote host to read and write a run's message space. Its stdout is a
/// contract with `rupu-cp`'s SSH connector:
///
/// - `list <id> [--after <msg-id>] [--limit <n>] [--kind a,b]` prints one
///   JSON object, the `MessagesView` (every message after `--after` when
///   `--limit` is absent);
/// - `send <id> --body=<text> [--to <address>] [--urgent] [--viewer <v>]`
///   sends as the operator (`via: cp`; `--to` defaults as `rupu message send`
///   does) and prints `{"id", "delivery"}` (`OperatorSendReply`).
///
/// `<id>` is exact (no fragment resolution). Any failure prints
/// `{"error": {"code", "message"}}` (`MessageErrorReply`; code `not_found`,
/// `not_live`, `invalid`, `log_full` or `internal`) and exits 3. Nothing
/// else is ever printed on stdout.
#[derive(Subcommand, Debug)]
pub enum HelperAction {
    List {
        id: String,
        #[arg(long)]
        after: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        kind: Option<String>,
    },
    Send {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        body: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        urgent: bool,
        #[arg(long)]
        viewer: Option<String>,
    },
}

/// The exit status of a failed hidden helper (`rupu __message`, `rupu
/// __board`).
const HELPER_FAILED: u8 = 3;

/// A hidden helper's reply on stdout — the wire contract the SSH connector's
/// `helper_call` parses: `r`'s JSON and exit 0, or `{"error": {"code",
/// "message"}}` (`MessageErrorReply`) and exit [`HELPER_FAILED`]. Nothing
/// else is printed.
pub(crate) fn emit_helper_reply<T: serde::Serialize>(r: Result<T, MessageError>) -> ExitCode {
    let reply = r.and_then(|v| {
        serde_json::to_string(&v)
            .map_err(|e| MessageError::new(MessageErrorCode::Internal, e.to_string()))
    });
    match reply {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            match serde_json::to_string(&MessageErrorReply { error }) {
                Ok(json) => println!("{json}"),
                Err(_) => println!(r#"{{"error":{{"code":"internal","message":"serialize"}}}}"#),
            }
            ExitCode::from(HELPER_FAILED)
        }
    }
}

pub fn handle_helper(action: HelperAction) -> ExitCode {
    emit_helper_reply(
        paths::global_dir()
            .map_err(|e| MessageError::new(MessageErrorCode::Internal, e.to_string()))
            .and_then(|global| helper(&global, action)),
    )
}

fn helper(global: &Path, action: HelperAction) -> Result<serde_json::Value, MessageError> {
    let internal =
        |e: &dyn std::fmt::Display| MessageError::new(MessageErrorCode::Internal, e.to_string());
    let invalid =
        |e: &dyn std::fmt::Display| MessageError::new(MessageErrorCode::Invalid, e.to_string());
    match action {
        HelperAction::List {
            id,
            after,
            limit,
            kind,
        } => {
            let root = MessageRoot::resolve(global, &id).map_err(|e| MessageError::from(&e))?;
            let kinds = kind
                .as_deref()
                .map(parse_kinds)
                .transpose()
                .map_err(|e| invalid(&e))?;
            let view = root
                .view(&ViewQuery {
                    after,
                    limit: limit.unwrap_or(usize::MAX),
                    kinds,
                })
                .map_err(|e| internal(&e))?;
            serde_json::to_value(view).map_err(|e| internal(&e))
        }
        HelperAction::Send {
            id,
            body,
            to,
            urgent,
            viewer,
        } => {
            let root = MessageRoot::resolve(global, &id).map_err(|e| MessageError::from(&e))?;
            let to = match to {
                Some(to) => Address::parse(&to).map_err(|e| invalid(&e))?,
                None => root.default_recipient(),
            };
            let caps = messaging_caps(global).map_err(|e| internal(&e))?;
            let msg = OperatorMessage {
                to,
                body,
                urgent,
                via: OperatorVia::Cp,
                viewer,
                client: None,
            };
            let receipt = root
                .operator_send(global, msg, caps, &rupu_orchestrator::runs::pid_is_running)
                .map_err(|e| MessageError::from(&e))?;
            serde_json::to_value(OperatorSendReply::from(&receipt)).map_err(|e| internal(&e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_fleet::{Bus, Member};
    use serde_json::json;

    /// A running workflow run `run_01ALPHA…` whose runner is this process,
    /// with `lynx#1` joined in its space.
    fn live_run(g: &Path, id: &str) -> Bus {
        let dir = g.join("runs").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("run.json"),
            serde_json::to_vec(&json!({
                "workflow_name": "review",
                "status": "running",
                "runner_pid": std::process::id(),
            }))
            .unwrap(),
        )
        .unwrap();
        let bus = Bus::open(&dir.join("messages"), "lynx#1");
        bus.join(Member {
            agent: "recon".into(),
            run_id: format!("{id}_a"),
            step_id: Some("scan".into()),
            kind: "workflow_step".into(),
            ..Default::default()
        })
        .unwrap();
        bus
    }

    fn args(id: &str, to: Option<&str>, text: &[&str]) -> SendArgs {
        SendArgs {
            id: id.into(),
            to: to.map(str::to_string),
            urgent: false,
            text: text.iter().map(|t| t.to_string()).collect(),
        }
    }

    const ID: &str = "run_01ALPHAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn send_reaches_the_address_and_says_who_it_missed() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        let lynx = live_run(g, ID);
        let caps = rupu_fleet::BusCaps::default();

        // The compact fragment a listing prints resolves.
        let line = send(
            g,
            &args("run_01ALPHAA…AAAA", Some("lynx#1"), &["look", "here"]),
            caps,
        )
        .unwrap();
        let view = MessageRoot::resolve(g, ID).unwrap().view(&ViewQuery {
            after: None,
            limit: 10,
            kinds: None,
        });
        let msgs = view.unwrap().messages;
        assert_eq!(line, format!("sent {} to lynx#1", msgs[0].message.id));
        assert_eq!(msgs[0].message.body, "look here");
        assert_eq!(msgs[0].message.via, Some(OperatorVia::Cli));
        assert_eq!(lynx.mailbox.drain("lynx#1").unwrap().len(), 1);

        // No `--to`: everyone, in a workflow run.
        let line = send(g, &args(ID, None, &["all"]), caps).unwrap();
        assert!(line.ends_with(" to broadcast"), "{line}");

        let line = send(g, &args(ID, Some("otter#9"), &["hi"]), caps).unwrap();
        let (_, missed) = line.split_once('\n').expect(&line);
        assert_eq!(missed, "not delivered to otter#9: unknown_participant");

        // Everyone, when nobody is left to hear it.
        lynx.leave("done").unwrap();
        let line = send(g, &args(ID, None, &["anyone?"]), caps).unwrap();
        assert_eq!(
            line.lines().nth(1),
            Some("not delivered to broadcast: no_live_participant"),
            "{line}"
        );
    }

    #[test]
    fn send_refuses_a_run_that_is_not_running_and_a_bad_address() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        live_run(g, ID);
        let caps = rupu_fleet::BusCaps::default();
        let err = send(g, &args(ID, Some("two words"), &["hi"]), caps).unwrap_err();
        assert!(
            err.to_string().starts_with("invalid address `two words`"),
            "{err}"
        );

        let run_json = g.join("runs").join(ID).join("run.json");
        std::fs::write(
            &run_json,
            r#"{"workflow_name":"review","status":"completed"}"#,
        )
        .unwrap();
        let err = send(g, &args(ID, None, &["hi"]), caps).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("workflow run `{ID}` is not running (completed): it takes no message")
        );
    }

    #[test]
    fn a_fragment_that_names_no_run_or_two_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        live_run(g, ID);
        live_run(g, "run_01ALPHAAAAAAAAAAAAAAAAAAB");
        let err = resolve_root(g, "run_01ALPHA").unwrap_err().to_string();
        assert!(
            err.starts_with("`run_01ALPHA` matches more than one run: "),
            "{err}"
        );
        let err = resolve_root(g, "ses_nothing").unwrap_err().to_string();
        assert!(err.starts_with("no run matches 'ses_nothing'"), "{err}");
    }

    #[test]
    fn follow_prints_each_message_once_then_only_new_receipts() {
        crate::output::palette::disable_color();
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        let lynx = live_run(g, ID);
        let caps = rupu_fleet::BusCaps::default();
        send(g, &args(ID, Some("lynx#1"), &["first\nsecond line"]), caps).unwrap();
        let root = MessageRoot::resolve(g, ID).unwrap();
        let q = ViewQuery {
            after: None,
            limit: usize::MAX,
            kinds: None,
        };
        let mut follow = Follow::default();
        let out = follow.next(&root.view(&q).unwrap(), false).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert!(
            lines[0].ends_with(" operator → lynx#1 [operator] first"),
            "{out}"
        );
        assert_eq!(lines[1], "    second line");
        assert_eq!(follow.next(&root.view(&q).unwrap(), false).unwrap(), "");

        let id = root.view(&q).unwrap().messages[0].message.id.clone();
        lynx.mark_seen(std::slice::from_ref(&id), "lynx#1", 3)
            .unwrap();
        let out = follow.next(&root.view(&q).unwrap(), false).unwrap();
        assert_eq!(out, format!("    seen by lynx#1 (turn 3): {id}\n"));

        // As JSON lines, a message comes again when its receipts change.
        let mut json = Follow::default();
        let line = json.next(&root.view(&q).unwrap(), true).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["id"], json!(id));
        assert_eq!(v["seen"][0]["by"], json!("lynx#1"));
    }

    #[test]
    fn a_message_line_shows_urgency_misses_and_receipts() {
        crate::output::palette::disable_color();
        let view: MessageView = serde_json::from_value(json!({
            "id": "msg_1", "ts": "2026-10-13T12:00:00Z", "kind": "operator",
            "from": "operator", "to": "role:recon", "body": "stop scanning",
            "urgent": true,
            "delivery": {"resolved": ["lynx#1"], "undelivered": [
                {"to": "lynx#2", "reason": "participant_left"}
            ]},
            "via": "cli",
            "seen": [{"by": "lynx#1", "turn": 4, "at": "2026-10-13T12:00:01Z"}],
        }))
        .unwrap();
        let mut out = String::new();
        render_message(&view, &mut out);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            &lines[0][8..],
            " operator → role:recon [operator] URGENT stop scanning"
        );
        assert_eq!(
            &lines[1..],
            [
                "    not delivered to lynx#2: participant_left",
                "    seen by lynx#1 (turn 4)"
            ]
        );
    }

    #[test]
    fn the_helper_sends_lists_and_answers_failures_with_a_code() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        live_run(g, ID);
        let sent = helper(
            g,
            HelperAction::Send {
                id: ID.into(),
                body: "-hi".into(),
                to: Some("step:scan".into()),
                urgent: true,
                viewer: Some("matt".into()),
            },
        )
        .unwrap();
        let reply: OperatorSendReply = serde_json::from_value(sent).unwrap();
        assert_eq!(reply.delivery.resolved, ["lynx#1"]);

        let listed = helper(
            g,
            HelperAction::List {
                id: ID.into(),
                after: None,
                limit: Some(5),
                kind: Some("operator".into()),
            },
        )
        .unwrap();
        let view: MessagesView = serde_json::from_value(listed).unwrap();
        let m = &view.messages[0].message;
        assert_eq!(m.id, reply.id);
        assert_eq!(
            (m.via, m.viewer.as_deref(), m.urgent, m.body.as_str()),
            (Some(OperatorVia::Cp), Some("matt"), true, "-hi")
        );

        let code = |action| helper(g, action).unwrap_err().code;
        let send_to = |id: &str, to: Option<&str>, body: &str| HelperAction::Send {
            id: id.into(),
            body: body.into(),
            to: to.map(str::to_string),
            urgent: false,
            viewer: None,
        };
        // An exact id only: a fragment is not found.
        assert_eq!(
            code(send_to("run_01ALPHA", None, "x")),
            MessageErrorCode::NotFound
        );
        assert_eq!(
            code(send_to(ID, Some("operator"), "x")),
            MessageErrorCode::Invalid
        );
        assert_eq!(code(send_to(ID, None, " ")), MessageErrorCode::Invalid);
        let list_kind = HelperAction::List {
            id: ID.into(),
            after: None,
            limit: None,
            kind: Some("shout".into()),
        };
        assert_eq!(code(list_kind), MessageErrorCode::Invalid);
        std::fs::write(
            g.join("runs").join(ID).join("run.json"),
            r#"{"workflow_name":"review","status":"failed"}"#,
        )
        .unwrap();
        assert_eq!(code(send_to(ID, None, "x")), MessageErrorCode::NotLive);
    }
}
