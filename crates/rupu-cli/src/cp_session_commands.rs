//! Executes only registry-approved read commands through this binary's Clap
//! parser and a fixed executable. No shell and no process-wide cwd changes.
use clap::Parser;
use rupu_cp::session_commands::{CommandError, CommandOutput, MAX_OUTPUT_BYTES};
use rupu_runtime::{session_commands::AttachCommand, sessions::SessionRecord};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

fn invalid(e: impl std::fmt::Display) -> CommandError {
    CommandError::Invalid(e.to_string())
}
fn failed(e: impl std::fmt::Display) -> CommandError {
    CommandError::Failed(e.to_string())
}

pub(crate) fn argv(
    command: AttachCommand,
    session: &SessionRecord,
) -> Result<Vec<String>, CommandError> {
    let mut argv = match command {
        AttachCommand::Routed(c) => {
            if !rupu_runtime::session_commands::inline_command_is_allowed(&c.root, &c.args) {
                return Err(invalid("only registry-approved read commands are allowed"));
            }
            let mut a = vec![c.root];
            a.extend(c.args);
            a
        }
        AttachCommand::Transcript => vec!["transcript".into(), "show".into()],
        AttachCommand::Status => vec!["session".into(), "show".into(), session.session_id.clone()],
        _ => return Err(invalid("not a routed read command")),
    };
    let parse = |a: &[String]| {
        crate::Cli::try_parse_from(std::iter::once("rupu".to_owned()).chain(a.iter().cloned()))
    };
    let session_alias = argv[0] == "session" && argv[1] == "show";
    let run_alias = (argv[0] == "transcript" && argv[1] == "show")
        || (argv[0] == "workflow" && argv[1] == "show-run");
    let default_id = || -> Result<String, CommandError> {
        if session_alias {
            Ok(session.session_id.clone())
        } else {
            session
                .active_run_id
                .as_ref()
                .or(session.last_run_id.as_ref())
                .cloned()
                .ok_or_else(|| invalid("session has no current or last run"))
        }
    };
    match parse(&argv) {
        Ok(cli) => {
            let id = match cli.command {
                crate::Cmd::Session {
                    action: crate::cmd::session::Action::Show { session_id, .. },
                } => Some(session_id),
                crate::Cmd::Transcript {
                    action: crate::cmd::transcript::Action::Show { run_id, .. },
                }
                | crate::Cmd::Workflow {
                    action: crate::cmd::workflow::Action::ShowRun { run_id, .. },
                } => Some(run_id),
                _ => None,
            };
            if let Some(id) = id.filter(|id| id == "current" || run_alias && id == "last") {
                // These show commands have exactly one string positional; their
                // other value options are typed view/format/numeric values.
                if let Some(value) = argv.iter_mut().skip(2).find(|v| **v == id) {
                    *value = default_id()?;
                }
            }
        }
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return Ok(argv)
        }
        Err(e)
            if (session_alias || run_alias)
                && e.kind() == clap::error::ErrorKind::MissingRequiredArgument =>
        {
            argv.insert(2, default_id()?);
        }
        Err(e) => return Err(invalid(e)),
    }
    // Validate before spawn with the exact CLI grammar (flags, arity, enums,
    // conflicts and supported global output formats), then force noninteractive output.
    parse(&argv).map_err(invalid)?;
    if matches!(argv[1].as_str(), "show" | "show-run") {
        let delimiter = argv.iter().position(|v| v == "--").unwrap_or(argv.len());
        let no_pager = argv[..delimiter].iter().any(|v| v == "--no-pager");
        argv = argv
            .into_iter()
            .enumerate()
            .filter_map(|(i, v)| (!(i < delimiter && v == "--pager")).then_some(v))
            .collect();
        if !no_pager {
            argv.insert(2, "--no-pager".into());
        }
    }
    parse(&argv).map_err(invalid)?;
    Ok(argv)
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> std::io::Result<(String, bool)> {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(MAX_OUTPUT_BYTES.saturating_sub(kept.len()));
        kept.extend_from_slice(&buf[..keep]);
        truncated |= keep < n;
    }
    Ok((String::from_utf8_lossy(&kept).into_owned(), truncated))
}

async fn subprocess(
    exe: &Path,
    global: &Path,
    cwd: &Path,
    argv: &[String],
) -> Result<CommandOutput, CommandError> {
    let future = async {
        let mut child = tokio::process::Command::new(exe)
            .args(argv)
            .current_dir(cwd)
            .env("RUPU_HOME", global)
            .env("NO_COLOR", "1")
            .env("RUPU_SESSION_READ_ONLY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(failed)?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (out, err, status) =
            tokio::try_join!(read_bounded(stdout), read_bounded(stderr), child.wait())
                .map_err(failed)?;
        let mut receipt = CommandOutput {
            output: out.0,
            stderr: err.0,
            exit_code: status.code().unwrap_or(-1),
            truncated: out.1 || err.1,
            data: None,
        };
        receipt.bound();
        Ok(receipt)
    };
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .map_err(|_| failed("session command exceeded 30 second deadline"))?
}

enum Prepared {
    Output(CommandOutput),
    Child {
        argv: Vec<String>,
        cwd: std::path::PathBuf,
    },
}
pub async fn execute(
    exe: &Path,
    global: &Path,
    id: &str,
    command: AttachCommand,
) -> Result<CommandOutput, CommandError> {
    let (home, session_id) = (global.to_owned(), id.to_owned());
    let future = async {
        let prepared = tokio::task::spawn_blocking(move || prepare(&home, &session_id, command))
            .await
            .map_err(failed)??;
        match prepared {
            Prepared::Output(output) => Ok(output),
            Prepared::Child { argv, cwd } => subprocess(exe, global, &cwd, &argv).await,
        }
    };
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .map_err(|_| failed("session command exceeded 30 second deadline"))?
}
fn prepare(global: &Path, id: &str, command: AttachCommand) -> Result<Prepared, CommandError> {
    let bytes =
        std::fs::read(global.join("sessions").join(id).join("session.json")).map_err(failed)?;
    let session = rupu_runtime::sessions::parse_session_record(&bytes).map_err(failed)?;
    if session.session_id != id {
        return Err(invalid("session record identity mismatch"));
    }
    let mut result = match command {
        AttachCommand::History | AttachCommand::Runs => {
            let history = matches!(command, AttachCommand::History);
            let rows: Vec<_> = session.runs.iter().rev().take(5).map(|r| {
                if history { serde_json::json!({"run_id":r.run_id,"prompt":r.prompt}) }
                else { serde_json::json!({"run_id":r.run_id,"status":r.status,"started_at":r.started_at,"completed_at":r.completed_at}) }
            }).collect();
            let mut out = CommandOutput::text(if rows.is_empty() {
                "no prior turns".into()
            } else {
                serde_json::to_string_pretty(&rows).map_err(failed)?
            });
            out.data = Some(serde_json::json!(rows));
            out
        }
        AttachCommand::Coverage => {
            if session.concerns.is_none() {
                return Err(invalid("this session's agent doesn't declare concerns"));
            }
            let target = rupu_coverage::target_id(&session.workspace_path, &session.session_id);
            let paths = rupu_coverage::CoveragePaths::new(&session.workspace_path, &target);
            if !paths.catalog.exists() {
                return Err(invalid("no ledger yet for this session (run a turn first)"));
            }
            let report = rupu_coverage::run_audit(&paths).map_err(failed)?;
            let mut out = CommandOutput::text(format!(
                "{}/{} concerns complete · {} gap files",
                report.complete_concerns, report.total_concerns, report.total_gap_files
            ));
            let mut concerns: Vec<_> = report.concerns.iter().collect();
            concerns.sort_by_key(|c| std::cmp::Reverse(c.gap_files.len()));
            for c in concerns.into_iter().filter(|c| !c.is_complete()).take(5) {
                out.output.push_str(&format!(
                    "\nGAP · {} · {} of {} files unasserted",
                    c.concern_id,
                    c.gap_files.len(),
                    c.in_scope_files.len()
                ));
            }
            out.data = Some(serde_json::to_value(report).map_err(failed)?);
            out
        }
        command => {
            let argv = argv(command, &session)?;
            return Ok(Prepared::Child {
                argv,
                cwd: session.workspace_path,
            });
        }
    };
    result.bound();
    Ok(Prepared::Output(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) -> SessionRecord {
        rupu_runtime::sessions::parse_session_record(&serde_json::to_vec(&serde_json::json!({
            "version":1,"session_id":"ses_test","agent_name":"test","provider_name":"mock","model":"test","agent_system_prompt":"","max_turns":5,"permission_mode":"readonly","no_stream":true,"workspace_id":"ws_test","workspace_path":root,"transcripts_dir":root.join("transcripts"),"created_at":"2026-11-04T00:00:00Z","updated_at":"2026-11-04T00:00:00Z","status":"idle","active_run_id":"run_current","last_run_id":"run_previous","runs":[]
        })).unwrap()).unwrap()
    }
    fn parsed(s: &str) -> AttachCommand {
        rupu_runtime::session_commands::parse_attach_command(s).unwrap()
    }
    #[test]
    fn uses_cli_validation_and_session_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let session = fixture(tmp.path());
        for command in [
            "/workflow list",
            "/workflow show --help",
            "/issues list --help",
            "/workflow show audit --view full",
            "/workflow show-run current --view full",
            "/workflow runs --limit 3 --status running",
            "/issues list --repo github:example/repo --label 'needs review' --limit 3",
            "/issues show 42",
            "/session show current",
            "/session show --view full",
            "/session show --view full ses_other",
            "/session show --view full current",
            "/workflow show -- --pager",
            "/session list --all",
            "/transcript",
            "/transcript list --limit 2",
        ] {
            let a = argv(parsed(command), &session).unwrap_or_else(|e| panic!("{command}: {e}"));
            assert!(!a.iter().any(|v| v == "current"));
        }
        assert!(argv(parsed("/session show current"), &session)
            .unwrap()
            .iter()
            .any(|a| a == "ses_test"));
        assert!(argv(parsed("/workflow show-run last"), &session)
            .unwrap()
            .iter()
            .any(|a| a == "run_current"));
        for command in [
            "/issues list --bogus",
            "/issues show",
            "/session list extra",
            "/workflow show audit --view wrong",
            "/issues list --limit NaN",
        ] {
            assert!(argv(parsed(command), &session).is_err(), "{command}");
        }
        let malicious =
            AttachCommand::Routed(rupu_runtime::session_commands::RoutedAttachCommand {
                root: "session".into(),
                args: vec!["delete".into(), "ses_test".into(), "--force".into()],
                display: String::new(),
            });
        assert!(argv(malicious, &session).is_err());
    }
    #[tokio::test]
    async fn adapter_uses_fixed_executable_literal_args_selected_workspace_and_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let dir = home.join("sessions/ses_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_vec(&fixture(&cwd)).unwrap(),
        )
        .unwrap();
        let exe = tmp.path().join("fixture-rupu");
        crate::test_support::exec_ready_script(&exe,"printf '%s\\n' \"$PWD\" \"$RUPU_HOME\" \"$RUPU_SESSION_READ_ONLY\" \"$@\"\nprintf 'fixture error' >&2\nexit 7\n");
        let out = execute(
            &exe,
            &home,
            "ses_test",
            parsed(
                "/issues list --label 'a; $(touch should-not-exist)' --repo github:example/repo",
            ),
        )
        .await
        .unwrap();
        assert_eq!(out.exit_code, 7);
        assert_eq!(out.stderr, "fixture error");
        assert!(out.output.contains(cwd.to_str().unwrap()));
        assert!(out.output.contains(home.to_str().unwrap()));
        assert!(out.output.contains("a; $(touch should-not-exist)"));
        assert!(!cwd.join("should-not-exist").exists());
    }
    #[tokio::test]
    async fn local_inspection_uses_the_selected_session_ledger_without_a_child() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions/ses_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut session = fixture(tmp.path());
        session.concerns = Some(Default::default());
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
        let target = rupu_coverage::target_id(tmp.path(), "ses_test");
        let paths = rupu_coverage::CoveragePaths::new(tmp.path(), &target);
        paths.ensure_dir().unwrap();
        std::fs::write(&paths.catalog, "concerns: []\nsources: {}\n").unwrap();
        let out = execute(
            Path::new("/does-not-exist"),
            tmp.path(),
            "ses_test",
            parsed("/coverage"),
        )
        .await
        .unwrap();
        assert!(out.output.contains("0/0 concerns complete"));
        assert_eq!(out.data.unwrap()["target_id"], target);
        for command in ["/history", "/runs"] {
            let out = execute(
                Path::new("/does-not-exist"),
                tmp.path(),
                "ses_test",
                parsed(command),
            )
            .await
            .unwrap();
            assert_eq!(out.output, "no prior turns");
        }
    }

    #[tokio::test]
    async fn output_capture_drains_but_does_not_retain_unbounded_streams() {
        let input = vec![b'x'; MAX_OUTPUT_BYTES * 3];
        let (text, truncated) = read_bounded(&input[..]).await.unwrap();
        assert_eq!(text.len(), MAX_OUTPUT_BYTES);
        assert!(truncated);
    }
}
