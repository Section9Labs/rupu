//! SSH transport: dispatch/observe/control runs on a host reachable over `ssh`.
//!
//! Auth is delegated entirely to the system `ssh` (ssh-agent / `~/.ssh/config`
//! / default keys); rupu stores no key material. Every remote argument is
//! shell-escaped before being joined into the remote command, because `ssh`
//! re-parses remote args through the remote login shell.

use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::stream::Stream;

// ── Pure builder functions ────────────────────────────────────────────────────

/// POSIX single-quote escaping: wrap in single quotes, replacing each embedded
/// `'` with `'\''`.
pub(crate) fn shell_escape(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Join an argv into a single shell command string with each token escaped.
pub(crate) fn build_remote_command(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shell_escape(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the args (after the `ssh` program) to run `remote_command` on `host`.
///
/// Flags emitted:
/// - `-o BatchMode=yes`  — fail fast on missing key rather than prompting
/// - `-o ConnectTimeout=10` — don't hang indefinitely on unreachable hosts
/// - `-i <identity_file>` — if provided
/// - `-p <port>` — if provided
/// - `<host>` — always present
/// - `<remote_command>` — always last
pub(crate) fn ssh_argv(
    host: &str,
    port: Option<u16>,
    identity_file: Option<&Path>,
    remote_command: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
    ];
    if let Some(id) = identity_file {
        argv.push("-i".to_string());
        argv.push(id.to_string_lossy().into_owned());
    }
    if let Some(p) = port {
        argv.push("-p".to_string());
        argv.push(p.to_string());
    }
    argv.push(host.to_string());
    argv.push(remote_command.to_string());
    argv
}

/// If `line` is a `tail` file-header (`==> <path> <==`), return the path.
pub(crate) fn parse_tail_marker(line: &str) -> Option<&str> {
    let t = line.trim();
    let inner = t.strip_prefix("==> ")?.strip_suffix(" <==")?;
    if inner.is_empty() {
        None
    } else {
        Some(inner)
    }
}

// ── RemoteExec trait + types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct RemoteOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteExecError {
    #[error("ssh spawn failed: {0}")]
    Spawn(String),
}

/// A pinned, boxed stream of lines from a remote command.
pub(crate) type LineStream = Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>>;

/// Port: run a command on the remote host.
///
/// The real impl (`SshExec`) shells out to the system `ssh`; tests inject a fake.
#[async_trait::async_trait]
pub(crate) trait RemoteExec: Send + Sync {
    /// Run `remote_command` to completion and collect its output.
    async fn run(&self, remote_command: &str) -> Result<RemoteOutput, RemoteExecError>;

    /// Spawn `remote_command` and return a stream of its stdout lines.
    ///
    /// The ssh child is kept alive for the stream's duration. When the stream
    /// is dropped the child is killed via `kill_on_drop(true)`.
    fn spawn_lines(&self, remote_command: &str) -> Result<LineStream, RemoteExecError>;
}

// ── Internal stream wrapper ───────────────────────────────────────────────────

/// Owns both the ssh `Child` and the `LinesStream` so the child process is
/// killed when this stream is dropped.
///
/// No `async-stream` macro is used. Both fields are `Unpin`, so the wrapper is
/// `Unpin` too and `poll_next` can delegate to the inner stream without unsafe.
struct SshLineStream {
    /// Kept for its `Drop` impl: `kill_on_drop(true)` kills the child when this
    /// field is dropped.
    _child: tokio::process::Child,
    /// The actual line producer. Stored boxed so `SshLineStream` stays `Unpin`.
    inner: Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>>,
}

impl Stream for SshLineStream {
    type Item = std::io::Result<String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Both Self and inner are Unpin, so no pin projection needed.
        self.inner.as_mut().poll_next(cx)
    }
}

// ── SshExec real implementation ───────────────────────────────────────────────

pub(crate) struct SshExec {
    pub host: String,
    pub port: Option<u16>,
    pub identity_file: Option<std::path::PathBuf>,
}

#[async_trait::async_trait]
impl RemoteExec for SshExec {
    async fn run(&self, remote_command: &str) -> Result<RemoteOutput, RemoteExecError> {
        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
        );
        let out = tokio::process::Command::new("ssh")
            .args(&argv)
            .output()
            .await
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        Ok(RemoteOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        })
    }

    fn spawn_lines(&self, remote_command: &str) -> Result<LineStream, RemoteExecError> {
        use tokio::io::AsyncBufReadExt as _;

        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
        );
        let mut child = tokio::process::Command::new("ssh")
            .args(&argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RemoteExecError::Spawn("no stdout pipe".into()))?;
        let reader = tokio::io::BufReader::new(stdout);
        let lines = tokio_stream::wrappers::LinesStream::new(reader.lines());

        // Wrap `_child` + `inner` together so the child is killed when the
        // stream is dropped. No async-stream or unsafe needed.
        let stream = SshLineStream {
            _child: child,
            inner: Box::pin(lines),
        };
        Ok(Box::pin(stream))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_wraps_and_escapes_quotes() {
        assert_eq!(shell_escape("plain"), "'plain'");
        assert_eq!(shell_escape("a b"), "'a b'");
        assert_eq!(shell_escape("it's"), r#"'it'\''s'"#);
        assert_eq!(shell_escape("a;rm -rf /"), "'a;rm -rf /'");
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
    }

    #[test]
    fn build_remote_command_escapes_each_token() {
        let argv = vec![
            "rupu".to_string(),
            "workflow".to_string(),
            "run".to_string(),
            "my workflow".to_string(),
            "--run-id".to_string(),
            "run_1".to_string(),
        ];
        assert_eq!(
            build_remote_command(&argv),
            "'rupu' 'workflow' 'run' 'my workflow' '--run-id' 'run_1'"
        );
    }

    #[test]
    fn ssh_argv_includes_flags_in_order() {
        let argv = ssh_argv(
            "deploy@edge",
            Some(2222),
            Some(std::path::Path::new("/k/id")),
            "'true'",
        );
        // BatchMode present as two args: -o BatchMode=yes
        assert!(argv
            .windows(2)
            .any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(argv.iter().any(|a| a == "-i") && argv.iter().any(|a| a == "/k/id"));
        assert!(argv.iter().any(|a| a == "-p") && argv.iter().any(|a| a == "2222"));
        assert_eq!(argv.last().unwrap(), "'true'");
        let pos_host = argv.iter().position(|a| a == "deploy@edge").unwrap();
        let pos_cmd = argv.len() - 1;
        assert!(pos_host < pos_cmd, "host must precede the remote command");
    }

    #[test]
    fn ssh_argv_omits_optional_flags() {
        let argv = ssh_argv("edge", None, None, "'true'");
        assert!(!argv.iter().any(|a| a == "-i"));
        assert!(!argv.iter().any(|a| a == "-p"));
        assert!(argv.iter().any(|a| a == "edge"));
    }

    #[test]
    fn parse_tail_marker_extracts_path() {
        assert_eq!(
            parse_tail_marker("==> /r/run_1/events.jsonl <=="),
            Some("/r/run_1/events.jsonl")
        );
        assert_eq!(parse_tail_marker(r#"{"some":"json"}"#), None);
        assert_eq!(parse_tail_marker(""), None);
    }
}
