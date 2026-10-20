//! `rupu cp serve --no-web` serves the operator MCP over HTTP, and the
//! deprecated `rupu mcp serve` is the same server. Both spawn the real
//! binary with an env of its own, so nothing here mutates this process.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// One HTTP/1.1 request over a plain socket (`Connection: close`): the
/// status and the body. `base` is `http://host:port`.
fn http(
    method: &str,
    base: &str,
    path: &str,
    body: Option<&Value>,
) -> std::io::Result<(u16, String)> {
    let addr = base.trim_start_matches("http://");
    let mut sock = std::net::TcpStream::connect(addr)?;
    sock.set_read_timeout(Some(Duration::from_secs(10)))?;
    let body = body.map(Value::to_string).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(req.as_bytes())?;
    let mut raw = String::new();
    sock.read_to_string(&mut raw)?;
    let (head, rest) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Chunked bodies: keep only the JSON object between the first `{` and
    // the last `}`.
    let json = match (rest.find('{'), rest.rfind('}')) {
        (Some(a), Some(b)) if a < b => rest[a..=b].to_string(),
        _ => rest.to_string(),
    };
    Ok((status, json))
}

async fn wait_healthy(base: &str, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok((200, _)) = http("GET", base, "/healthz", None) {
            return;
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("server exited early: {status}");
        }
        assert!(Instant::now() < deadline, "server not up within 20 s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn rpc(base: &str, id: u64, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let (status, text) = http("POST", base, "/mcp", Some(&body)).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{status} {text}: {e}"))
}

async fn tool_names(base: &str) -> Vec<String> {
    let init = rpc(
        base,
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "smoke", "version": "0"}}),
    );
    assert!(init["result"].is_object(), "{init}");
    let list = rpc(base, 2, "tools/list", json!({}));
    list["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools in {list}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn cp_serve_no_web_serves_mcp_tools() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args([
            "cp",
            "serve",
            "--no-web",
            "--bind",
            &format!("127.0.0.1:{port}"),
        ])
        .env("RUPU_HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rupu cp serve");
    let mut guard = Guard(child);
    let base = format!("http://127.0.0.1:{port}");
    wait_healthy(&base, &mut guard.0).await;

    let names = tool_names(&base).await;
    for want in ["runs.approve", "wait", "scm.repos.list", "read_file"] {
        assert!(names.iter().any(|n| n == want), "missing {want}: {names:?}");
    }
    assert!(
        !names.iter().any(|n| n == "findings.query"),
        "findings.query is the operator's: {names:?}"
    );
    let (status, _) = http("GET", &base, "/", None).unwrap();
    assert_eq!(status, 404, "--no-web serves no page");
}

#[tokio::test]
async fn cp_serve_refuses_no_web_with_no_mcp_and_tools_with_no_mcp() {
    let home = tempfile::tempdir().unwrap();
    for (args, says) in [
        (
            &["cp", "serve", "--no-web", "--no-mcp"][..],
            "nothing to serve",
        ),
        (
            &["cp", "serve", "--no-mcp", "--tools", "read_file"][..],
            "--tools needs the MCP; drop --no-mcp",
        ),
        (
            &["cp", "serve", "--no-web", "--tools", "nope.nothing"][..],
            "nope",
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_rupu"))
            .args(args)
            .args(["--bind", &format!("127.0.0.1:{}", free_port())])
            .env("RUPU_HOME", home.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?} should fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(says),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[tokio::test]
async fn mcp_serve_alias_prints_the_deprecation_and_serves_http() {
    // The alias binds the default port; never touch a server already there.
    if std::net::TcpStream::connect("127.0.0.1:7878").is_ok() {
        eprintln!("skipping: 127.0.0.1:7878 is taken (a real `rupu cp serve`?)");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(["mcp", "serve", "--transport", "stdio"])
        .env("RUPU_HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rupu mcp serve");
    let mut guard = Guard(child);
    let mut stderr = guard.0.stderr.take().unwrap();
    let base = "http://127.0.0.1:7878";
    wait_healthy(base, &mut guard.0).await;
    let names = tool_names(base).await;
    assert!(names.iter().any(|n| n == "scm.repos.list"), "{names:?}");
    let _ = guard.0.kill();
    let _ = guard.0.wait();
    let mut text = String::new();
    let _ = stderr.read_to_string(&mut text);
    assert!(text.contains("deprecated"), "{text}");
    assert!(text.contains("ignoring --transport"), "{text}");
    assert!(
        text.contains("claude mcp add --transport http rupu http://127.0.0.1:7878/mcp"),
        "{text}"
    );
}
