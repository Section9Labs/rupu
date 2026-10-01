//! SIGTERM (what a workflow-run cancel sends the run's `runner_pid`) must go
//! through rupu's handler — drain in-flight credential writes, bounded, then
//! exit 143 — rather than kill the process outright, which would cancel a
//! token refresh mid-persist.

#[cfg(unix)]
#[test]
fn sigterm_exits_through_the_drain_with_143() {
    use std::io::BufRead;
    let home = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("rupu"))
        .args(["cp", "serve", "--bind", "127.0.0.1:0", "--no-open"])
        .env("RUPU_HOME", home.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn rupu cp serve");
    // Wait until it is serving: the handler is installed before that.
    let stdout = child.stdout.take().unwrap();
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
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "rupu did not exit after SIGTERM"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(
        status.code(),
        Some(143),
        "exited through the handler, not killed by the signal: {status:?}"
    );
}
