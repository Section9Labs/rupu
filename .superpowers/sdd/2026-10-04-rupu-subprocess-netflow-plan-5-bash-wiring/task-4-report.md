# Task 4 report — end-to-end live test

Approach: full `rupu run` path. `crates/rupu-cli/tests/serial/netflow_subprocess_live.rs`
(macOS-only, `#[ignore]`, holds ENV_LOCK) spawns the real `rupu` binary
(assert_cmd) in a temp project with `[netflow] subprocess_capture = true`, a
pre-created `.rupu/netflow/`, and a mock-provider script: one `bash` tool_use
(id `call_live_curl`) then a closing text turn. Running as a child process gives
the real macOS backend (`net_capture::shared`, fresh OnceLock) and no env leaks.
The test polls `<project>/.rupu/netflow/run_netflow_live.jsonl` (up to 8s) via
`rupu_netflow::ledger::read_flows`.

Command: `curl -s -o /dev/null --limit-rate 300 --max-time 5 http://example.com || true`.
A bare curl finishes in ~100ms and the ntstat watcher misses it (first attempt
captured only the `capture active (macos-ntstat)` line, zero flows); `--limit-rate`
holds the socket established, as the netwatch live test does.

Reproduce: `cargo test -p rupu-cli --test serial -- --ignored a_bash_curl_flow_is_captured_and_attributed --nocapture < /dev/null`

Captured flow (3/3 runs passed, ~6s each):
- fidelity Socket, origin Subprocess("curl"), process curl pid 45627
- ctx.run_id run_netflow_live, agent fetcher, tool_call_id call_live_curl
- scheme tcp, host/peer 104.20.23.154, port 80, local 10.9.8.151:63415
- outcome Ok, body_complete true (SocketComplete folded), bytes_in 936, bytes_out 74, duration_ms 1781
- error note: "observation ended with the run; socket still open"

Asserts: Socket flow exists; port 80 + tool_call_id match; origin curl; run_id;
body_complete; bytes_in>0; bytes_out>0; duration present.

Flakiness: depends on example.com reachability and on the connection staying up
across a poll (hence --limit-rate). Short connections are not reliably captured
(known backend limitation, not a wiring bug). Clippy: my file is clean;
`cargo clippy -p rupu-cli --tests` fails only on a pre-existing
`question_mark` lint in src/cmd/completers.rs:127 (local toolchain drift, untouched).
