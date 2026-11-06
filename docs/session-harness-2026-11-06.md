# Built-in Rupu session harness

The project session entry point creates one shipped, provider-independent
`rupu-session` root. It needs no project agent file, stock fleet installation,
or specialist selection. Rupu's existing runtime supplies its tools, ledgers,
children, transcripts, configuration, and credentials.

## Client contract

Check `GET /api/host/info` for `features` containing `session.builtin_harness`.
It is advertised only by a writable CP whose session adapter preserves the
first-turn selection and project identity.

```http
POST /api/projects/:ws_id/sessions
Content-Type: application/json

{"prompt":"Inspect this project", "provider":"openai", "model":"selected-model", "mode":"readonly"}
```

`prompt` must contain text for a normal start. `provider`, `model`, and `mode` are optional. A
provider override requires an explicit model; model alone uses the instance's
default provider. Omitting both uses normal Rupu configuration resolution,
including the project's optional customer layer. A model ID does not have to
appear in a discovered model catalog. Identifiers are validated before launch.
No credential, endpoint, agent, target, or working-directory field is accepted.
Unknown body and query fields are errors. The exact registered project is the
execution/configuration root, even if it has no `.rupu` directory.

The mode defaults to `readonly`. `bypass` must be selected explicitly. `ask`
returns HTTP 400 because detached workers have no approval channel. Readonly
allows reads and Rupu bookkeeping, denies workspace writes and external effects,
and caps children at readonly. A strict-approval marker travels from the built-in
root through its in-process and local process descendants. A headless child in
that tree whose effective mode is Ask fails assembly instead of permitting writes
without approval. Existing unrelated launches retain their established behavior;
interactive children with a real prompter retain that approval path.

Explicit operator workflow launches linked to a built-in session also inherit
its mode when the request omits one and refuse Ask before reserving the workflow
association. An explicit readonly/bypass choice remains an independent operator
decision; legacy session-linked workflow behavior is preserved. A durable
run-owned approval policy makes local workflow steps and descendants strict
across resume, without manufacturing parent/child ancestry. Host/distribute
workflow steps are refused on this path until that transport can carry the
policy; the request is rejected before association or execution.

Execution belongs to the directly connected instance. `?host=local` is accepted;
remote host routing is HTTP 501. Connect directly to the intended host before
creating the session. The legacy remote adapter refuses first-turn selections or
project identity fields it cannot carry.

A successful HTTP 200 returns:

```json
{"session_id":"ses_...","host_id":"local","agent":"rupu-session","provider":"openai","model":"selected-model"}
```

A normal start acknowledges creation/enqueue, not successful provider execution. Follow the
existing session detail and stream routes for actual progress and failures. A
transport failure after creation is ambiguous; reload before retrying. Invalid
input/configuration is HTTP 400, unknown project is 404, and a deployment without
the required adapter is 501. Errors use the CP's normal `{"error":"..."}` body.
The operator MCP exposes the same route as `projects.start_session`.

For opening recorded work, check the additional `session.idle_harness` capability
and send `{"idle":true,"mode":"readonly"}` with optional provider/model. A prompt
is forbidden with `idle:true`; the backend creates the real empty session and
settings without enqueueing a turn, launching a worker, or issuing a model
request. The CLI equivalent is `rupu session start rupu-session --idle`. Legacy
starts without a prompt retain their prior behavior when idle is false. The
parent's historical workflow attachment contract is documented separately in
`docs/harness-2026-11-06/historical-workflow-contract.md`.

Recorded active or archived workflow runs can be attached to an active session
in the same registered project. Attachment preserves original provenance and
does not execute the run. On its next real turn, the built-in harness receives
bounded reference metadata from the canonical records: run ID, workflow name,
origin, recorded status, and storage location. Raw inputs, events, and source
paths are excluded. The context is refreshed per turn and does not change the
stored transcript. Unix/macOS reads traverse directory descriptors with
`O_NOFOLLOW`; unsupported hosts refuse attached context rather than reading
through an unsafe fallback.

The CLI equivalent is `rupu session start rupu-session --provider ACCOUNT
--model MODEL --mode readonly --prompt OBJECTIVE`. Provider/model overrides are
frozen into the session record and its revisioned settings before the first turn
is enqueued. The HTTP route pins its resolved defaults before invoking the CLI.
A hidden expected-workspace argument verifies ownership again at creation. Later
settings changes keep the project's and agent's identity immutable.

## Runtime behavior

The root definition is compiled into `rupu-agent` and reserved lookup precedes
all project/global agent files, including malformed or colliding files. Its
provider, model, and authentication mechanism are unpinned. Explicit empty
fallbacks keep the selected model from moving to a configured fallback chain.
Generic disk-definition listings remain disk listings; creation does not depend
on finding the root there. Skills remain available through explicit operator
invocation (`/skill-name`, `--skill`, or the existing session skill controls).
The root does not grant all installed skill layers automatically, so duplicate
global/project skill names cannot block basic conversation creation.

The root gets native file tools, `agents.*`, workflow catalog reads,
`catalog.search`, `dispatch`/`join`, `coverage.*`, `findings.*`, and the SCM/issue
connector namespaces. Granted tools are resolved against actual runtime services
and the ordinary permission policy. It does not attach the unrestricted operator
MCP facade. Missing connector credentials remain normal connector errors.

At each turn the runtime resolves installed catalog names into the root's launch
allowlists. Normal child caps, depth limits, connector limits, and lifecycle rules
still apply. Delegated work remains ordinary Rupu runs. The existing launcher
refuses gated or host/distribute workflows as model-dispatched children; this
change does not claim remote workflow parity. Broken catalog entries cannot
replace the root or force specialist setup.

Three bundled coding concerns cover correctness, verification, and security/data
boundaries. They provide a real session-owned coverage ledger and findings
services without a project concern catalog. Their evidence describes the
session's work; it does not establish whole-project assessment coverage.

A persisted `builtin_harness` marker distinguishes new harness sessions from
older custom sessions with the same name. Only marked sessions acquire automatic
catalog launch scope. Such records require version 4 so an older binary cannot
silently discard the marker. Legacy records retain their prior minimum versions,
frozen prompts/tools, history, and fallback lookup. Existing agent-based HTTP
creation remains available.

## Validation and handoff

The component checks below use the isolated checkout's target directory with
`CARGO_BUILD_JOBS=4`, `CARGO_PROFILE_DEV_DEBUG=0`, and
`CARGO_PROFILE_TEST_DEBUG=0`.

| Command | Result |
| --- | --- |
| `cargo test -p rupu-agent --test it builtin_session` | 2 passed |
| `cargo test -p rupu-runtime --test it` | 133 passed |
| `cargo test -p rupu-runtime --lib` | 162 passed; one existing live-network test ignored |
| `cargo test -p rupu-orchestrator --test it workflow_runtime::` | 4 passed |
| `cargo test -p rupu-launch --test it` | 83 passed |
| `cargo test -p rupu-runtime --lib strict_approval::` | 1 passed on the final durable-marker implementation |
| `cargo test -p rupu-cp --no-default-features --lib project_session_start` | 7 passed |
| `cargo test -p rupu-cp --no-default-features --lib session_workflow` | 23 passed |
| `cargo test -p rupu-cli --no-default-features --lib session_harness_tests` | 6 passed |
| `cargo test -p rupu-cli --no-default-features --lib workflow_context::tests` | 6 passed |
| `cargo test -p rupu-cli --no-default-features --lib cp_session_starter` | 9 passed |
| `cargo test -p rupu-cli --no-default-features --test it argv_roundtrip` | 4 passed |
| `cargo test -p rupu-cp --no-default-features --test it mcp_ops_projects` | 4 passed |
| `cargo test -p rupu-cp --no-default-features --test it mcp_ops_sessions` | 3 passed |
| `cargo test -p rupu-cp --no-default-features --test it first_turn_session_selection` | 1 passed |
| `cargo test -p rupu-cp --no-default-features --test it mcp_routes` | 5 passed; all 193 operator tools pinned, including the four new session/history operations |
| `cargo test -p rupu-cp --no-default-features --lib scoped_revision` | 6 passed after the parent's test-only Clippy correction |
| `cargo build -p rupu-cli --no-default-features --bin rupu` | Passed; resulting binary reports `rupu 0.87.0 (dev)` |

Provider discovery changed after the published snapshot: the initial public
catalog fallback passed 30 mocked tests but failed the parent's live acceptance
because the selected token lacked the required scope. Parent commit `cec4cefd`
replaced that fallback with a current Codex catalog client version. Its owner
reports all 27 corrected provider catalog tests passing. The updated contract,
direct probe evidence, and remaining acceptance are in
`docs/harness-2026-11-06/provider-model-discovery.md`. The binary build above
predates this correction; the parent owns the rebuilt runtime, cache refresh,
and deployment acceptance. Catalog discovery does not
establish successful model completion.

Combined CP/CLI test logs and machine-readable results are under
`target/harness-validation/`. The first operator-tool run stopped because the
sandbox denied its `127.0.0.1:0` mock listener; the rerun with localhost networking
allowed passed. Project creation was rerun successfully after the parent's
provider discovery change settled. Apple's linker emitted a nonfatal
compact-unwind size warning for the large integration-test binaries.

The implementation snapshot is commit `1f3da72a`, published by the parent in
draft Rupu PR #946. The MCP fixture/count correction and this completed validation
record are a separate follow-up. Parent commit `cec4cefd` contains the provider
correction, test-only Clippy changes, and the CLI newer-record fixture update
from hardcoded version 4 to `SessionRecord::VERSION + 1`. The full `cli_session::` regression
filter for that last update remains with the parent; it is not included in the
focused results above. This workstream has released the isolated Cargo target.

No real provider work, installation, control-plane restart, push, or merge is
part of this backend workstream. Native UI integration, a successful live
provider journey, final combined permission acceptance, packaging and deployment
remain with the parent workstream. Mock provider and disposable filesystem tests
are component evidence, not live integration acceptance.

AI-assisted implementation: OpenAI Codex.
