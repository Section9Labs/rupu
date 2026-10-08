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

- **Node protocol.** `RunSpec` (`rupu-cp/src/node/protocol.rs:87`) gains optional `run_id` and `codename` (`#[serde(default, skip_serializing_if)]`). A node that knows them **honours** a supplied run id and codename. Nodes advertise this as the capability `run.supplied_identity`, in tunnel `Hello.capabilities` and bucket node markers, following the existing `agent.findings_profile` pattern.
  - The coordinator relies on its pre-minted id only when the capability is advertised. Otherwise it keeps today's behaviour (learn the id from the node's reply). That choice is logged, never silent.
- **HTTP host.** `POST /api/agents/:name/run` accepts optional `run_id` / `codename` behind the same feature in `/api/host/info`.
- **SSH.** Already honours both (`ssh.rs:3258`). The `__features` list gains `run.codename_flag`. An older remote gets the env var form, chosen by `RunArgv::for_peer`.
- `honours_supplied_run_id` (SSH-only today) becomes a `HostConnector` method answered from each connector's advertised features.

### 3.4 L12

`build_dispatcher_if_needed` wires a `SubprocessAgentLauncher`, built from `spawn_detached` and `RunArgv`, into the local connector, so `host: local` works from the CLI.

## 4. Files

| File | Change |
|---|---|
| `rupu-runtime/src/{argv,spawn}.rs` | **new** |
| `rupu-cli/src/{cp_agent_launcher,cp_launcher,cp_session_starter,cp_session_sender}.rs` | use `RunArgv` + `spawn_detached` |
| `rupu-cli/src/cmd/{cp,node,run,workflow,session}.rs` | `build_resume_argv`/`build_argv` deleted; `--codename` flag; session worker spawn |
| `rupu-cp/src/host/{ssh,http,tunnel,bucket}.rs`, `node/protocol.rs` | `to_shell`; `RunSpec.{run_id,codename}`; capability advertisement |
| `rupu-agentiflow/src/subprocess.rs` | `rupu_run_argv` deleted; `SpawnSpec { keep_child: true }` |
| `rupu-cli/src/fleet_unit_dispatcher.rs` | the L12 wiring |

## 5. Deleted

`build_agent_argv`, `build_run_argv`, `agent_argv`/`workflow_argv` (bodies, not the SSH wrapper), `node::build_argv`, `rupu_run_argv`/`rupu_agent_argv`/`rupu_workflow_argv`, `build_resume_argv`, and the six inline `Command::new(..).process_group(0)` blocks.

## 6. Tests

1. **`argv_roundtrip`** (`rupu-cli` it, because the clap types live there): for a generated set of `RunArgv` values, `to_args()` → the real clap `Cli::try_parse_from` → convert back → equal. This makes the argv and the CLI impossible to drift apart. A prompt beginning with `-` and containing newlines and quotes is included.
2. **`argv_shell_safe`** (`rupu-runtime`): `to_shell` output, run through `sh -c 'printf %s\\n "$@"' _ …`, reproduces `to_args` element for element.
3. **`for_peer_downgrades`**: a peer without `run.codename_flag` gets the env form, with a recorded `Downgrade`.
4. **`node_honours_supplied_identity`** (`rupu-cli` it, node executor): with the capability, the spawned run's id and codename equal the supplied ones.
5. **`host_local_works`** (`rupu-cli` serial): a workflow with `host: local` dispatches through the subprocess launcher.

## 7. Acceptance

- `grep -rn 'process_group(0)' crates/` hits only `rupu-runtime/src/spawn.rs` (and tests).
- `grep -rn '"--run-id"' crates/` hits only `argv.rs` (and tests).
