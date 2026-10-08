# W6: One typed argv, one detached spawn

- **Card:** W6 · **Depends on:** nothing (it can run in parallel with W1–W5) · **Blocks:** W7
- **Principles:** P1 applied to processes, P10 · **Decisions:** D18
- **Fixes:** L6, L7, L8, L12

## 1. Goal

Every place that starts a `rupu` child process builds its command line from **one typed value**, `RunArgv`, and starts it through **one** `spawn_detached`. Remote connectors (SSH, node tunnel/bucket) render the same value. Prompt quoting, flag spelling, codename passing and run-id honouring then stop differing by launcher.

## 2. Today

There are six argv builders. Each one encodes its own idea of `rupu run` / `rupu workflow run`:

| Builder | Location | Quirks |
|---|---|---|
| CP local agent | `rupu-cli/src/cp_agent_launcher.rs:20 build_agent_argv` | `--prompt p` (two args), `--tmp` when targeted, repeats `--engagement-profile`, **drops `req.codename`** |
| CP local workflow | `rupu-cli/src/cp_launcher.rs build_run_argv` | `--plain`, `--input k=v` |
| SSH agent/workflow | `rupu-cp/src/host/ssh.rs:1839 agent_argv` / `workflow_argv` | `env RUPU_CODENAME=… rupu run …`, shell-escaped, `setsid`/`nohup` |
| node (tunnel/bucket executor) | `rupu-cli/src/cmd/node.rs:902 build_argv` | **mints its own run id**, ignores codename |
| agentiflow units | `rupu-agentiflow/src/subprocess.rs:86 rupu_run_argv` | `--prompt=p` (one arg), comma-joins engagement ids, `--fleet-run-dir/--fleet-participant` |
| resume/approve workers | `rupu-cli/src/cmd/cp.rs:887 build_resume_argv` | `workflow approve|resume [--if-unfinished]` |

There are also six hand-rolled detached spawns: CP agent launcher, CP workflow launcher, `node spawn_run` / `spawn_control`, `SubprocessUnitLauncher` (the only one that keeps the `Child`), agentiflow `spawn_detached`, and the session worker.

Separately, `build_dispatcher_if_needed` (`rupu-cli/src/fleet_unit_dispatcher.rs:754`) constructs `LocalHostConnector::new(None, None, None, None, …)`, so `host: local` fails with "no agent launcher configured" (L12).

## 3. Design

### 3.1 `RunArgv` (`crates/rupu-runtime/src/argv.rs`, new)

```rust
pub enum RunArgv {
    Agent(AgentRun),
    Workflow(WorkflowRun),
    WorkflowControl(WorkflowControl),   // approve | resume (+ if_unfinished)
    SessionStart(SessionStart),
    SessionSend(SessionSend),
}

pub struct AgentRun {
    pub agent: String,
    pub target: Option<String>,
    pub run_id: String,
    pub codename: Option<String>,
    pub mode: Option<PermissionMode>,
    pub prompt: Option<String>,
    pub findings_profile: Option<FindingProfile>,
    pub engagement_profiles: Vec<String>,
    pub tmp_clone: bool,                // today's "--tmp when targeted" rule, explicit
    pub flow: Option<FlowAttach>,       // { run_dir, participant } → --fleet-run-dir/--fleet-participant
    pub launch: Option<LaunchLink>,     // W7: { depth, ceiling, parent_run, usage_root } (hidden flags)
}
pub struct WorkflowRun {
    pub workflow: WorkflowRef,          // Name(String) | File(PathBuf)
    pub target: Option<String>, pub run_id: String, pub mode: Option<PermissionMode>,
    pub inputs: BTreeMap<String, String>, pub plain: bool,
    pub engagement_profiles: Vec<String>, pub flow: Option<FlowAttach>, pub launch: Option<LaunchLink>,
}

impl RunArgv {
    /// Canonical rendering: one spelling per flag. Prompt as `--prompt=<p>` (one argv
    /// element, so a prompt starting with '-' can't become a flag); engagement ids
    /// repeated; inputs `--input k=v` in key order.
    pub fn to_args(&self) -> Vec<OsString>;
    /// The same args, POSIX-shell-escaped, for SSH command strings.
    pub fn to_shell(&self, program: &str) -> String;
    /// For a peer that predates a flag (see 3.3): drop or downgrade the flags its
    /// advertised features don't cover, returning what was downgraded.
    pub fn for_peer(&self, features: &FeatureSet) -> (RunArgv, Vec<Downgrade>);
}
```

**`--codename` hidden flag (L8).** `rupu run` and `rupu workflow run` accept `--codename <c>`, replacing the `RUPU_CODENAME` environment variable. The variable keeps being read for one release, so in-flight detached runs keep their names. The codename is honoured whenever it is given, not only together with `--run-id` (the `run.rs:1926` condition is removed).

### 3.2 `spawn_detached` (`crates/rupu-runtime/src/spawn.rs`, new)

```rust
pub struct SpawnSpec {
    pub program: PathBuf,               // current_exe() by default; config override (`[cp].rupu_exe`) if present
    pub argv: RunArgv,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>, // RUPU_HOME etc.; never secrets the child can resolve itself
    pub stdio: SpawnStdio,              // Null | Log(PathBuf)
    pub keep_child: bool,               // agentiflow units need the Child for try_wait
}
pub struct Spawned { pub pid: u32, pub pgid: u32, pub child: Option<std::process::Child> }
pub fn spawn_detached(spec: SpawnSpec) -> io::Result<Spawned>;   // process_group(0), stdio per spec
```

SSH keeps its remote `setsid nohup … &` wrapper, but takes its command string from `RunArgv::to_shell`. The node executor (`cmd/node.rs spawn_run`) becomes `spawn_detached` + `RunArgv`.

### 3.3 Run id and codename across peers (L8)

*(Corrected 2026-10-08. The first draft wrongly put `run_id` into `RunSpec` behind a capability.)*

**Run id: no protocol change and no capability.** The id already travels outside `RunSpec` (tunnel `Frame::Run { run_id, spec }`, the bucket `jobs/<run_id>.json` key), and every node since #414 runs `rupu run --run-id <that id>`. The bug is on the coordinator side: the tunnel and bucket connectors mint a fresh id instead of using `AgentLaunchRequest.run_id`.
- **Fix:** `req.run_id.clone().unwrap_or_else(mint)` in both connectors.
- `honours_supplied_run_id` returns `true` for tunnel and bucket, unconditionally.
- **HTTP is the exception.** `POST /api/agents/:name/run` mints on the peer. Add an optional `run_id` body field. The peer honours it only if it advertises `run.supplied_run_id` in `/api/host/info`, and the HTTP connector's `honours_supplied_run_id` answers from that feature. The connector keeps reading the returned id, so an older peer that ignores the field still reports the id it really used.

**Codename: best-effort, never a launch refusal.** A codename is display identity. Unlike `findings_profile` / `engagement_profiles`, it changes nothing about how the run behaves, so a peer that can't take it must not block the launch.
- `RunSpec` gains an optional `codename` field (`#[serde(default, skip_serializing_if)]`; `RunSpec` has no `deny_unknown_fields`, so an old node ignores it). Nodes that pass it on as `--codename` advertise `run.codename`. The HTTP body gets the same field under the same feature.
- **When the peer lacks the capability:**
  - the launch proceeds;
  - the remote run derives its own standalone codename;
  - the coordinator's own records (the `UnitDispatch` / `AgentStarted` / run records) keep the coordinator-minted name, as they do today;
  - the connector records the downgrade: one `tracing::warn!` per launch naming the host and its version, and a `Downgrade` returned from `RunArgv::for_peer`.

  The result is a codename mismatch between the coordinator's record and the remote transcript's `RunStart`. That is visible and cosmetic, never silent breakage.
- SSH keeps its existing env-var path for remotes without `run.codename_flag` (`for_peer`).

### 3.4 L12

`build_dispatcher_if_needed` wires a `SubprocessAgentLauncher`, built from `spawn_detached` and `RunArgv`, into the local connector, so `host: local` works from the CLI.

## 4. Files

| File | Change |
|---|---|
| `rupu-runtime/src/{argv,spawn}.rs` | **new** |
| `rupu-cli/src/{cp_agent_launcher,cp_launcher,cp_session_starter,cp_session_sender}.rs` | use `RunArgv` + `spawn_detached` |
| `rupu-cli/src/cmd/{cp,node,run,workflow,session}.rs` | `build_resume_argv`/`build_argv` deleted; `--codename` flag; session worker spawn |
| `rupu-cp/src/host/{ssh,http,tunnel,bucket}.rs`, `node/protocol.rs` | `to_shell`; `RunSpec.codename` + `run.codename` / `run.supplied_run_id` capability advertisement; tunnel/bucket connectors honour `req.run_id` |
| `rupu-agentiflow/src/subprocess.rs` | `rupu_run_argv` deleted; `SpawnSpec { keep_child: true }` |
| `rupu-cli/src/fleet_unit_dispatcher.rs` | the L12 wiring |

## 5. Deleted

`build_agent_argv`, `build_run_argv`, `agent_argv`/`workflow_argv` (bodies, not the SSH wrapper), `node::build_argv`, `rupu_run_argv`/`rupu_agent_argv`/`rupu_workflow_argv`, `build_resume_argv`, and the six inline `Command::new(..).process_group(0)` blocks.

## 6. Tests

1. **`argv_roundtrip`** (`rupu-cli` it, because the clap types live there): for a generated set of `RunArgv` values, `to_args()` → the real clap `Cli::try_parse_from` → convert back → equal. This makes the argv and the CLI impossible to drift apart. A prompt beginning with `-` and containing newlines and quotes is included.
2. **`argv_shell_safe`** (`rupu-runtime`): `to_shell` output, run through `sh -c 'printf %s\\n "$@"' _ …`, reproduces `to_args` element for element.
3. **`for_peer_downgrades`**: a peer without `run.codename_flag` gets the env form, with a recorded `Downgrade`.
4. **`tunnel_bucket_use_supplied_run_id`** (`rupu-cp` it): both connectors launch under `req.run_id` when it is set. **`codename_best_effort`**: a node with `run.codename` runs under the supplied codename; one without it still launches, and the downgrade is logged.
5. **`host_local_works`** (`rupu-cli` serial): a workflow with `host: local` dispatches through the subprocess launcher.

## 7. Acceptance

- `grep -rn 'process_group(0)' crates/` hits only `rupu-runtime/src/spawn.rs` (and tests).
- `grep -rn '"--run-id"' crates/` hits only `argv.rs` (and tests).
