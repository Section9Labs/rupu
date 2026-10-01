//! SIGTERM (what a workflow-run cancel sends the run's `runner_pid`) goes
//! through rupu's handler thread, which drains in-flight credential writes
//! (bounded) and then restores the default disposition and re-raises the
//! signal. With nothing pending the process therefore dies by SIGTERM at
//! once — a cancel, the shell and `systemctl stop` see a signal death, not
//! an exit code — exactly as an unhandled SIGTERM would. With a write
//! pending, the process still dies by SIGTERM once the write lands, never
//! by the command's own exit code.

use std::io::BufRead;
use std::sync::mpsc::{channel, Receiver};

/// Kills the child on drop, so a failed assertion never leaves a
/// `cp serve` (or anything else) behind.
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A `rupu` subprocess with its stdout and stderr delivered line by line.
/// `RUPU_LOG=info` so the SIGTERM handler's own log line is observable.
fn spawn_rupu(
    args: &[&str],
    home: &std::path::Path,
    extra_env: &[(&str, &str)],
) -> (ChildGuard, Receiver<String>, Receiver<String>) {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("rupu"));
    cmd.args(args)
        .env("RUPU_HOME", home)
        .env("RUPU_LOG", "info")
        .current_dir(home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn rupu");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (out_tx, out_rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            let _ = out_tx.send(line);
        }
    });
    let (err_tx, err_rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = err_tx.send(line);
        }
    });
    (ChildGuard(child), out_rx, err_rx)
}

/// Blocks until a line containing `needle` arrives, panicking after 30s.
fn wait_for_line(rx: &Receiver<String>, needle: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) if line.contains(needle) => return,
            Ok(_) => {}
            Err(e) => panic!("no line containing {needle:?} arrived: {e}"),
        }
    }
}

fn sigterm(child: &ChildGuard) {
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &child.0.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
}

fn wait_exit(child: &mut ChildGuard) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "rupu did not exit after SIGTERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Everything the process wrote to stderr, once it has exited.
fn drain(rx: Receiver<String>) -> String {
    rx.iter().collect::<Vec<_>>().join("\n")
}

#[cfg(unix)]
#[test]
fn sigterm_with_nothing_pending_kills_by_the_signal() {
    use std::os::unix::process::ExitStatusExt;
    let home = tempfile::tempdir().unwrap();
    let (mut child, stdout, stderr) = spawn_rupu(
        &["cp", "serve", "--bind", "127.0.0.1:0", "--no-open"],
        home.path(),
        &[],
    );
    // Wait until it is serving: the handler is installed before that.
    wait_for_line(&stdout, "127.0.0.1");
    sigterm(&child);
    let status = wait_exit(&mut child);
    assert_eq!(
        status.signal(),
        Some(15),
        "died by SIGTERM (nothing was pending), not by an exit code: {status:?}"
    );
    assert_eq!(status.code(), None);
    // The handler's own trace: without it installed, SIGTERM's default
    // disposition would kill the process identically — this is what tells
    // the two apart.
    let stderr = drain(stderr);
    assert!(
        stderr.contains("SIGTERM received"),
        "the handler logged the signal: {stderr}"
    );
}

/// A credential write still pending when SIGTERM arrives holds the exit
/// until it lands (at most 10s), and the process then still dies by SIGTERM
/// — also when the command had already returned and `main` was the one
/// waiting for the write: `main` hands off to the signal path instead of
/// exiting with the command's code.
#[cfg(unix)]
#[test]
fn sigterm_with_a_pending_write_still_kills_by_the_signal() {
    use std::os::unix::process::ExitStatusExt;
    let home = tempfile::tempdir().unwrap();
    // A quick command with one tracked write held open for 3s: the command
    // returns at once and `main` waits for the write.
    let (mut child, _stdout, stderr) = spawn_rupu(
        &["agent", "list"],
        home.path(),
        &[("RUPU_TEST_HOLD_CREDENTIAL_WRITE_MS", "3000")],
    );
    wait_for_line(&stderr, "waiting for 1 credential write");
    let signalled_at = std::time::Instant::now();
    sigterm(&child);
    let status = wait_exit(&mut child);
    assert!(
        signalled_at.elapsed() >= std::time::Duration::from_millis(500),
        "the pending write held the exit ({:?})",
        signalled_at.elapsed()
    );
    assert_eq!(
        status.signal(),
        Some(15),
        "died by SIGTERM once the write landed, not by the command's exit code: {status:?}"
    );
    assert_eq!(status.code(), None);
    let stderr = drain(stderr);
    assert!(
        stderr.contains("SIGTERM received"),
        "the handler logged the signal: {stderr}"
    );
}
