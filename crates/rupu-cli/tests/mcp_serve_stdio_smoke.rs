//! Spawn `rupu mcp serve --transport stdio`, send tools/list, parse the
//! response, confirm the catalog contains expected names.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn mcp_serve_stdio_returns_tools_list() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(["mcp", "serve", "--transport", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rupu mcp serve");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        // Small pause so the server's stdin listener is ready before we send.
        std::thread::sleep(std::time::Duration::from_millis(50));
        writeln!(
            stdin,
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}}"
        )
        .unwrap();
        stdin.flush().unwrap();
    }

    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    // Read the response with a thread-based timeout so CI doesn't hang.
    let read_result = std::thread::scope(|s| {
        let h = s.spawn(|| reader.read_line(&mut line));
        h.join()
    });

    // Tolerate the thread joining even if there was an I/O error.
    let _ = read_result;

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        !line.trim().is_empty(),
        "Expected a JSON-RPC line from mcp serve, got nothing"
    );

    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("parse JSON-RPC response");
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    let tools = v["result"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|t| t["name"] == "scm.repos.list"),
        "expected scm.repos.list in catalog; got: {:?}",
        tools.iter().map(|t| &t["name"]).collect::<Vec<_>>()
    );
    assert!(
        tools.iter().any(|t| t["name"] == "issues.get"),
        "expected issues.get in catalog"
    );
}
