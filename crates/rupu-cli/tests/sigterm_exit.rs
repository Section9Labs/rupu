//! SIGTERM (what a workflow-run cancel sends the run's `runner_pid`) goes
//! through rupu's handler thread, which drains in-flight credential writes
//! (bounded) and then restores the default disposition and re-raises the
//! signal. With nothing pending the process therefore dies by SIGTERM at
//! once — a cancel, the shell and `systemctl stop` see a signal death, not
//! an exit code — exactly as an unhandled SIGTERM would.

/// Kills the child on drop, so a failed assertion never leaves a
/// `cp serve` behind.
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
#[test]
fn sigterm_with_nothing_pending_kills_by_the_signal() {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;
    let home = tempfile::tempdir().unwrap();
    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("rupu"))
        .args(["cp", "serve", "--bind", "127.0.0.1:0", "--no-open"])
        .env("RUPU_HOME", home.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn rupu cp serve");
    let mut child = ChildGuard(child);
    // Wait until it is serving: the handler is installed before that.
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if line.contains("127.0.0.1") {
                let _ = tx.send(());
            }
        }
    });
    rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("cp serve came up");
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &child.0.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "rupu did not exit after SIGTERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(
        status.signal(),
        Some(15),
        "died by SIGTERM (nothing was pending), not by an exit code: {status:?}"
    );
    assert_eq!(status.code(), None);
}
