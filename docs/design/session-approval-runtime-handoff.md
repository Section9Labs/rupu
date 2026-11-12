# Session approval runtime handoff

This isolated source slice wires the authenticated CP broker into builtin session turns. It does not change the default permission mode or enable the CP feature by itself. Source baseline: `a42e3beaa5736998fabcf9513976545044e326de`.

## Authority and ordering

The CLI starter/sender passes a private Unix socket as the start/send command's stdin. The hidden `--approval-channel` flag carries no credential. Start/send and worker entry points immediately duplicate that descriptor with `F_DUPFD_CLOEXEC` and replace stdin with `/dev/null`. Only the explicit session-worker subprocess receives the socket again. Ordinary tool subprocesses cannot inherit it through stdin or an open executable descriptor.

The long-lived worker registers its exact session and PID, then freezes each turn's session settings, coordination revision, root run, project path, and optional trusted MCP registry fingerprint. A private owner round trip precedes SCM/provider construction; run assembly repeats it before creating the run. In-process subagents carry the same authorizer with their actual parent/root identities and must register their own ownership before provider work.

The runner resolves the existing tool grant before asking. Only already-granted Write/External calls under Ask enter the asynchronous approval boundary. Read, Record, Spawn ceilings, explicit readonly/bypass, and TTY behavior keep their existing semantics. The synchronous policy refuses an attached asynchronous channel rather than falling through to unattended AllowDegraded or AllowAlways.

Requests bind actual session/root/run/parent/project, turn index, call ID plus call sequence, canonical/called tool names, effect, exact input, descriptor hash, and grant hash. The private client checks broker instance, epoch, worker generation, owner/call/tool/effect fields before publishing Pending. A reservation must match that Pending approval ID and intent fingerprint. Tool dispatch consumes a noncloneable permit. Completion is published only after the broker durably acknowledges it.

The client serial mutex covers authorization through handler completion. Cancellation, malformed replies, lost completion acknowledgement, or dropping a reserved permit shuts down the actual socket before releasing its mutex, even when other Arc handles survive. A sibling cannot silently dispatch after uncertainty. The agent records the already-observed tool result and stops if its completion receipt is unconfirmed.

## Boundaries that remain explicit

- Process agent/workflow children cannot inherit this private transport in v1. An Ask parent with this authorizer receives `approval_channel_unsupported` before process spawn; it is not silently downgraded. In-process subagents are supported. This does not claim Agentiflow process execution parity.
- A warm Ask worker retains its original channel across turns. A CP broker restart closes that channel; the next ownership check fails closed. After preserving the claimed turn's evidence, an unusable worker retires. A later explicit send creates a new worker/channel; the failed or uncertain claimed request is never requeued. There is no automatic retry of an uncertain call.
- The host MCP authority fingerprint covers the registry and credential-store status metadata (binding, revision, presence), never credential values. It is checked before and after client assembly and again by the broker before reservation. Registry-wide credential rotation conservatively invalidates the Ask turn. Environment bearer configurations are refused for this path because they have no versioned revocation contract.
- Typed transcript events are historical evidence at `type=tool_approval`, `data.record`. Current approval eligibility still requires a fresh owning-broker read. Exact reviewed arguments stay out of transcript events.
- The private socket is an operating-system process capability, not protection from another process already running with equivalent access to the same user's storage/processes.

## Prepared validation

No build, test execution, provider call, service mutation, live HTTP request, or real credential access occurred in this slice. Rustfmt parsed the owned changed sources, and `git diff --check` passed.

Prepared fixtures:

- `session_approvals/client_tests.rs`: abort after observed Pending closes actual peer despite surviving handles; mismatched generation/epoch/run/root/parent/tool/effect never reaches observer; dropped reservation blocks queued sibling; valid Recorded releases serial channel; wrong reservation fingerprint refuses dispatch; oversized/unknown-field protocol frames reject.
- `rupu-agent/src/approval_tests.rs`: exact call and remote binding, no grant expansion, project drift/read effect/oversized input rejection, canonical hashing, and no synchronous AllowAlways fallback.
- `rupu-transcript/src/event.rs`: exact typed event envelope roundtrip without review arguments.
- `rupu-cli/src/cmd/session_harness_tests.rs`: explicit Ask preparation refuses without the private-channel bit and succeeds with it before writing or starting a session.
- Runner cancellation fixture: a fake authorizer cancels in the same poll that returns a ready permit; the real write handler never runs, and the permit drops.
- Fresh-channel fixture: a broken worker capability remains permanently closed while a distinct explicit registration can work; no old request crosses to it.

Coordinated validation still must compile the full union, run these and the broker/store tests, exercise stdin handoff with a disposable CLI subprocess, and verify actual provider/tool approval behavior before enabling the advertised feature/default in a release.
