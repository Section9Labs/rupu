//! Pure executor for `run:` steps. Templates are already rendered by the
//! caller; this module turns a fully-resolved command into a captured
//! result. No shell is involved at any point.
//!
//! Keeping this separate from `runner.rs` is deliberate: given a
//! [`ResolvedRun`] it produces a [`RunStepOutput`] with no knowledge of
//! runs, steps, or contexts, so the security-critical argv handling is
//! testable in isolation.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::workflow::ParseMode;

/// A `run:` step with every template already rendered.
#[derive(Debug, Clone)]
pub struct ResolvedRun {
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub parse: ParseMode,
    pub timeout: Option<Duration>,
    pub allow_exit_codes: Vec<i32>,
}

/// Captured result of one command execution.
#[derive(Debug, Clone)]
pub struct RunStepOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    /// True when `exit_code` is in `allow_exit_codes`.
    pub success: bool,
    /// stdout interpreted per [`ParseMode`]. `Raw` ⇒ a JSON string.
    pub parsed: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum RunStepError {
    #[error("failed to spawn `{cmd}`: {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{cmd}` exceeded its {timeout_secs}s timeout and was killed")]
    Timeout { cmd: String, timeout_secs: u64 },
    #[error("`{cmd}` stdout is not valid JSON (parse: json): {source}")]
    ParseJson {
        cmd: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Execute a resolved command.
///
/// Never invokes a shell: `cmd` and `args` go to the OS as an argv
/// vector, so shell metacharacters in an argument are inert literal
/// text. This is the property that lets a workflow interpolate an
/// arbitrary fixture path or task id into `args` without it becoming a
/// code-execution vector.
pub async fn execute(r: &ResolvedRun) -> Result<RunStepOutput, RunStepError> {
    let started = Instant::now();

    let mut command = tokio::process::Command::new(&r.cmd);
    command
        .args(&r.args)
        .current_dir(&r.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in &r.env {
        command.env(k, v);
    }

    let child = command.spawn().map_err(|source| RunStepError::Spawn {
        cmd: r.cmd.clone(),
        source,
    })?;

    let output = match r.timeout {
        Some(t) => match tokio::time::timeout(t, child.wait_with_output()).await {
            Ok(res) => res.map_err(|source| RunStepError::Spawn {
                cmd: r.cmd.clone(),
                source,
            })?,
            // `kill_on_drop(true)` reaps the child when the future drops.
            Err(_) => {
                return Err(RunStepError::Timeout {
                    cmd: r.cmd.clone(),
                    timeout_secs: t.as_secs(),
                })
            }
        },
        None => child
            .wait_with_output()
            .await
            .map_err(|source| RunStepError::Spawn {
                cmd: r.cmd.clone(),
                source,
            })?,
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // A signal-terminated child has no code; -1 is distinguishable from
    // any real exit status and is never in a sane allow-list.
    let exit_code = output.status.code().unwrap_or(-1);
    let success = r.allow_exit_codes.contains(&exit_code);

    let parsed = match r.parse {
        ParseMode::Raw => serde_json::Value::String(stdout.clone()),
        ParseMode::Json => {
            serde_json::from_str(stdout.trim()).map_err(|source| RunStepError::ParseJson {
                cmd: r.cmd.clone(),
                source,
            })?
        }
        ParseMode::Lines => serde_json::Value::Array(
            stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| serde_json::Value::String(l.to_string()))
                .collect(),
        ),
    };

    Ok(RunStepOutput {
        stdout,
        stderr,
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
        success,
        parsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn resolved(cmd: &str, args: &[&str]) -> ResolvedRun {
        ResolvedRun {
            cmd: cmd.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            parse: ParseMode::Raw,
            timeout: None,
            allow_exit_codes: vec![0],
        }
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let out = execute(&resolved("echo", &["hello"])).await.unwrap();
        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.exit_code, 0);
        assert!(out.success);
    }

    #[tokio::test]
    async fn argv_is_never_shell_interpreted() {
        // THE security property. A metacharacter-laden argument is passed
        // as literal text, not executed. If this regresses, a
        // template-rendered fixture path becomes a code-execution vector.
        let marker = std::env::temp_dir().join("rupu_pwned_marker_run_step");
        let _ = std::fs::remove_file(&marker);
        let evil = format!("; touch {}", marker.display());

        let out = execute(&resolved("echo", &[&evil])).await.unwrap();

        assert_eq!(out.stdout.trim(), evil);
        assert!(
            !marker.exists(),
            "shell metacharacters must not be interpreted"
        );
    }

    #[tokio::test]
    async fn disallowed_exit_code_is_not_success() {
        let out = execute(&resolved("false", &[])).await.unwrap();
        assert_eq!(out.exit_code, 1);
        assert!(!out.success, "exit 1 is not in allow_exit_codes [0]");
    }

    #[tokio::test]
    async fn custom_allow_exit_codes_accepts_nonzero() {
        let mut r = resolved("false", &[]);
        r.allow_exit_codes = vec![0, 1];
        let out = execute(&r).await.unwrap();
        assert!(out.success);
    }

    #[tokio::test]
    async fn timeout_kills_the_child() {
        let mut r = resolved("sleep", &["30"]);
        r.timeout = Some(Duration::from_millis(200));
        let started = Instant::now();

        let err = execute(&r).await.unwrap_err();

        assert!(matches!(err, RunStepError::Timeout { .. }), "got {err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the child must be killed, not waited out"
        );
    }

    #[tokio::test]
    async fn json_parse_mode_binds_a_value() {
        let mut r = resolved("echo", &[r#"{"score": 91}"#]);
        r.parse = ParseMode::Json;
        let out = execute(&r).await.unwrap();
        assert_eq!(out.parsed["score"], 91);
    }

    #[tokio::test]
    async fn json_parse_mode_fails_loudly_on_bad_json() {
        // Binding a garbage string as "the output" would let a broken
        // scorer silently produce a plausible-looking report.
        let mut r = resolved("echo", &["not json"]);
        r.parse = ParseMode::Json;
        assert!(
            matches!(
                execute(&r).await.unwrap_err(),
                RunStepError::ParseJson { .. }
            ),
            "bad JSON under parse: json must be an error"
        );
    }

    #[tokio::test]
    async fn lines_parse_mode_drops_empty_lines() {
        let mut r = resolved("printf", &["a\nb\n"]);
        r.parse = ParseMode::Lines;
        let out = execute(&r).await.unwrap();
        assert_eq!(out.parsed, serde_json::json!(["a", "b"]));
    }

    #[tokio::test]
    async fn missing_executable_is_a_spawn_error() {
        let err = execute(&resolved("rupu_no_such_binary_xyz", &[]))
            .await
            .unwrap_err();
        assert!(matches!(err, RunStepError::Spawn { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn env_is_passed_to_the_child() {
        let mut r = resolved("sh", &["-c", "printf %s \"$RUPU_TEST_VAR\""]);
        r.env.insert("RUPU_TEST_VAR".into(), "sentinel".into());
        let out = execute(&r).await.unwrap();
        assert_eq!(out.stdout.trim(), "sentinel");
    }

    #[tokio::test]
    async fn cwd_is_honoured() {
        let dir = std::env::temp_dir();
        let mut r = resolved("pwd", &[]);
        r.cwd = dir.clone();
        let out = execute(&r).await.unwrap();
        // macOS reports /private/var for /var, so compare canonical forms.
        let got = std::fs::canonicalize(out.stdout.trim()).unwrap();
        let want = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(got, want);
    }
}
