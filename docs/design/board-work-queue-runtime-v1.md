# Board work Queue runtime bindings

This supplements the shared v1 contract. It records supported runtime placements,
not a claim of deployment or real-provider acceptance.

The runtime owns the Queue actor. Every assembled supported execution receives a
new execution ID, exact run ID, recording anchor/path, and explicit lead round
where applicable. A child inherits only its parent's typed Queue owner binding;
its own launch supplies its recording and new execution ID. Tools cannot submit
an actor, executor, recording path or operator identity.

| Placement | Binding |
| --- | --- |
| Root session turn | Explicit session ID and active session directory |
| Workflow step | Explicit workflow run ID under the configured global run root |
| Agentiflow lead | Explicit flow ID/directory and launch-site round |
| Agentiflow unit | Explicit flow ID/directory |
| Standalone run with its own space | Explicit run ID under the configured global run root |
| In-process subagent | Typed owner copied through the launcher; exact parent/child transcript path |
| Legacy process child carrying only message-space path/address | Unsupported; Queue tools unavailable |
| Legacy continuation whose shared space is established only by a symlink | Unsupported; a symlink confers no Queue authority |
| Missing owner, unsupported recording path, disabled messaging | Queue tools unavailable with a recorded reason |

Recording paths are relative to the exact owner, global `transcripts`, the
project's `.rupu/transcripts` with workspace ID, or global `runs`. Global `runs`
is supported only for a typed in-process child whose trusted launch names exactly
`<parent_run_id>/sub/<child_run_id>/transcript.jsonl`. The control plane must
verify the recorded descendant chain and transcript identity before returning an
available conversation link. A path prefix alone is not ownership evidence.

After `RunStart`, the runner writes a `Notice` with kind `work_queue_execution`
and the JSON serialization of the trusted `WorkActor`. Source resolution can
therefore distinguish executions that reused a run ID or recording path. An
unsupported placement receives `work_queue_unavailable` instead. A Queue receipt
does not prove that the subsequent transcript output was written.

The runner stamps turn index and the call's position in the full model tool-call
list, including earlier denied calls. Provider call IDs may repeat. Fresh writes
require both the current registered execution and a live runtime execution guard;
the guard is revoked on return, error, cancellation or unwind. Historical receipt
replay remains separate from new-write authority.

Every mutation takes the retention lifecycle lease before the Bus lock and
storage transaction. Owner directory identity and immutable owner-record identity
are checked again under the guard. Mutable session history/status saves do not
change owner identity. Archive, replacement and termination cannot silently
transfer authority.

The collector reads at most eight queued items and four active items per state
(claimed, working, blocked). It reports source-scoped counts and pagination;
its compact observation is not a complete Queue dump. It performs no mutation,
does not wake an idle session, and never selects or claims work automatically.
Queue tools are explicit bookkeeping operations and preserve existing readonly,
workspace, connector and spawn permissions. A later session execution or lead
round must ask the operator to requeue an abandoned attempt.

Focused checks use isolated production stores and the actual session record,
assembler, runner and Queue tool paths. Scripted providers exercise invocation
ordering without a network call. They are not application display data and do
not establish live provider or native UI acceptance.
