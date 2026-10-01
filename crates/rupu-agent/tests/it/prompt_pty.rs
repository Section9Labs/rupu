use rupu_agent::permission::{PermissionDecision, PermissionPrompt};
use std::io::{IsTerminal, Read, Write};
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// In-memory test: simulate the operator typing "y\n" — should yield Allow.
#[test]
fn allow_on_y() {
    let input = b"y\n".to_vec();
    let mut output: Vec<u8> = Vec::new();
    let mut prompt = PermissionPrompt::new_in_memory(&input[..], &mut output);
    let d = prompt
        .ask("bash", &serde_json::json!({"command": "ls"}), "/tmp/ws")
        .unwrap();
    assert_eq!(d, PermissionDecision::Allow);
    let s = String::from_utf8(output).unwrap();
    assert!(s.contains("bash"), "prompt should mention tool name: {s}");
    assert!(
        s.contains("/tmp/ws"),
        "prompt should mention workspace: {s}"
    );
}

#[test]
fn deny_on_n() {
    let input = b"n\n".to_vec();
    let mut output: Vec<u8> = Vec::new();
    let mut prompt = PermissionPrompt::new_in_memory(&input[..], &mut output);
    let d = prompt
        .ask("bash", &serde_json::json!({}), "/tmp/ws")
        .unwrap();
    assert_eq!(d, PermissionDecision::Deny);
}

#[test]
fn always_on_a() {
    let input = b"a\n".to_vec();
    let mut output: Vec<u8> = Vec::new();
    let mut prompt = PermissionPrompt::new_in_memory(&input[..], &mut output);
    let d = prompt
        .ask("bash", &serde_json::json!({}), "/tmp/ws")
        .unwrap();
    assert_eq!(d, PermissionDecision::AllowAlwaysForToolThisRun);
}

#[test]
fn stop_on_s() {
    let input = b"s\n".to_vec();
    let mut output: Vec<u8> = Vec::new();
    let mut prompt = PermissionPrompt::new_in_memory(&input[..], &mut output);
    let d = prompt
        .ask("bash", &serde_json::json!({}), "/tmp/ws")
        .unwrap();
    assert_eq!(d, PermissionDecision::StopRun);
}

#[test]
fn invalid_input_re_prompts_then_decides() {
    let input = b"q\nfoo\ny\n".to_vec();
    let mut output: Vec<u8> = Vec::new();
    let mut prompt = PermissionPrompt::new_in_memory(&input[..], &mut output);
    let d = prompt
        .ask("bash", &serde_json::json!({}), "/tmp/ws")
        .unwrap();
    assert_eq!(d, PermissionDecision::Allow);
}

#[test]
fn long_input_truncated_to_200_chars_with_more_marker() {
    let huge = "x".repeat(500);
    let input = b"y\n".to_vec();
    let mut output: Vec<u8> = Vec::new();
    let mut prompt = PermissionPrompt::new_in_memory(&input[..], &mut output);
    prompt
        .ask("bash", &serde_json::json!({"command": huge}), "/tmp/ws")
        .unwrap();
    let s = String::from_utf8(output).unwrap();
    assert!(s.contains("(more)"), "expected truncation marker, got: {s}");
    // Sanity: the full 500-char string should not appear in full.
    assert!(!s.contains(&"x".repeat(500)));
}

/// Set on the child [`pty_real_terminal_round_trip`] spawns: the
/// re-executed test binary then plays the prompting process instead of
/// the driver.
const PTY_CHILD_ENV: &str = "RUPU_AGENT_PTY_CHILD";
/// The child prints this, then its decision, once `ask` returns.
const DECISION_MARKER: &str = "rupu-pty-decision=";
/// Upper bound on each wait for the child, so a broken prompt fails the
/// test instead of hanging it.
const PTY_WAIT: Duration = Duration::from_secs(30);

/// PTY round-trip for `rupu run`'s `Ask` mode: re-run this test binary as a
/// child whose stdin/stdout/stderr are the slave side of a real pty, have it
/// prompt through [`PermissionPrompt::for_stdio`] (the constructor the CLI
/// uses), type `y` + Enter once the prompt reaches the terminal, and assert
/// the child decided `Allow`. The in-memory tests above cover `ask`'s
/// parsing; this covers what they can't: the prompt being flushed to a real
/// terminal before the read blocks, and the answer arriving through the tty
/// line discipline.
#[test]
fn pty_real_terminal_round_trip() {
    if std::env::var_os(PTY_CHILD_ENV).is_some() {
        prompt_as_pty_child();
        return;
    }

    let (pty, pts) = pty_process::blocking::open().expect("allocate pty");
    let exe = std::env::current_exe().expect("locate this test binary");
    let child = pty_process::blocking::Command::new(exe)
        .args([
            "--exact",
            &self_test_name(),
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PTY_CHILD_ENV, "1")
        .spawn(pts)
        .expect("spawn child on the pty");
    let mut child = KillOnDrop(child);

    // Reads on the pty block, so pump them on a thread. The channel
    // disconnects once the child exits and the slave side closes.
    let pty = Arc::new(pty);
    let (tx, rx) = mpsc::channel();
    let reader = Arc::clone(&pty);
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match (&*reader).read(&mut buf) {
                // EOF, or EIO on Linux once the slave side closes.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut seen = Vec::new();

    assert!(
        wait_until(&rx, &mut seen, |t| t.contains("[y/n/a/s]: ")),
        "prompt never reached the terminal:\n{}",
        String::from_utf8_lossy(&seen)
    );
    // Enter sends CR; the tty's ICRNL turns it into the newline `read_line`
    // is waiting for.
    (&*pty).write_all(b"y\r").expect("type the answer");
    assert!(
        wait_until(&rx, &mut seen, |t| decision(t).is_some()),
        "child never reported a decision:\n{}",
        String::from_utf8_lossy(&seen)
    );
    let text = String::from_utf8_lossy(&seen).into_owned();
    assert_eq!(
        decision(&text),
        Some("Allow"),
        "terminal transcript:\n{text}"
    );

    let status =
        wait_for_exit(&mut child.0).unwrap_or_else(|| panic!("child never exited:\n{text}"));
    seen.extend(rx.try_iter().flatten());
    assert!(
        status.success(),
        "child failed ({status}):\n{}",
        String::from_utf8_lossy(&seen)
    );
}

/// The re-executed side of [`pty_real_terminal_round_trip`]: prompt the way
/// `rupu run` does in `Ask` mode and report the decision on stdout.
fn prompt_as_pty_child() {
    assert!(std::io::stdin().is_terminal(), "child stdin is not the pty");
    assert!(
        std::io::stderr().is_terminal(),
        "child stderr is not the pty"
    );
    let mut stderr = std::io::stderr();
    let d = PermissionPrompt::for_stdio(&mut stderr)
        .ask("bash", &serde_json::json!({"command": "ls"}), "/tmp/ws")
        .expect("prompt on the pty");
    println!("{DECISION_MARKER}{d:?}");
}

/// This test's libtest name: bare when this file is its own test binary,
/// module-qualified when it is a module of a combined `tests/it` binary.
fn self_test_name() -> String {
    let test = "pty_real_terminal_round_trip";
    match module_path!().split_once("::") {
        Some((_, module)) => format!("{module}::{test}"),
        None => test.to_string(),
    }
}

/// Pull pty output into `seen` until `done` holds for it. False when the
/// child closed the pty first or [`PTY_WAIT`] ran out.
fn wait_until(rx: &Receiver<Vec<u8>>, seen: &mut Vec<u8>, done: impl Fn(&str) -> bool) -> bool {
    let deadline = Instant::now() + PTY_WAIT;
    while !done(&String::from_utf8_lossy(seen)) {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(chunk) => seen.extend(chunk),
            Err(_) => return false,
        }
    }
    true
}

/// The decision the child reported, once its whole line has arrived.
fn decision(text: &str) -> Option<&str> {
    let rest = &text[text.find(DECISION_MARKER)? + DECISION_MARKER.len()..];
    rest.find(['\r', '\n']).map(|end| &rest[..end])
}

fn wait_for_exit(child: &mut Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + PTY_WAIT;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Kills and reaps the child even when an assertion fails first, so a
/// broken prompt can't leave a process blocked on the pty.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
