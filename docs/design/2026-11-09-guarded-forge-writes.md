# Guarded operator forge writes — source contract

Status: **root approved source implementation; no provider writes**. Source base is exact `a42e3beaa5736998fabcf9513976545044e326de`. G1 bounded issue reads and G3 bounded review/account transport are integration prerequisites; reuse their reviewed authority helpers when composing rather than fork a weaker copy.

## Supported actions and deliberate boundaries

The first contract exposes `review.comment`, `review.create`, `issue.comment`, `issue.create`, and `issue.set_state`. GitHub/GitLab support review actions; the four configured issue providers support issue actions where a concrete provider action can be prepared. No approval, merge, thread-resolution, push, branch creation, label creation or credential/grant mutation is implied.

Repository actions originate from an exact registered local project. The owning checkout's origin and configured account rules determine the adapter; request account/origin fields are equality guards. Initial PR/MR creation is within that repository, from an existing remote source branch to an existing remote target branch; forks need a separate explicit source/target-repository contract. The native editor may not infer account/project from a displayed name or recorded trigger.

Issue actions use an explicit configured tracker/account/API origin and exact tracker project, independent of whichever workspace happens to be selected. G1 current issue identity supplies existing-item context. Creating an issue first resolves and returns the immutable provider project/team identifier; that identity must be reviewed. Jira issue type and Jira/Linear state choices are exact provider IDs and visible names, not an invisible first-match heuristic. Multiple eligible choices require an explicit selection before an executable intent can exist. Labels are bounded explicit existing names/IDs; preparation does not create them.

## Three-step operator contract

Feature: `forge.operator_write.v1`, conditional on an installed owning writer port. Routes are local-owner only and excluded from the model MCP operation catalog/guard allowlist. Existing HTTP bearer/cookie policy remains intact. These routes do not grant models new forge permissions.

1. `GET /api/forge/write-capabilities?host=local` returns configured safe account/origin/action support; preparation returns the action-specific preconditions and limitations. Configuration presence does not claim provider authorization. Repository rules still resolve the account from the registered project when preparing.
2. `POST /api/forge/intents` prepares an immutable reviewed intent, making bounded provider **reads only**. Request: `{host:"local", action, target, expected, input}` with unknown fields rejected. Response includes `{host_id,intent_id,digest,created_at,expires_at,review,execute_supported,preconditions_atomic:false,automatic_retry:false}`; `review` contains `{request,authority,observed,preconditions,limitations,generation}`. Exact submitted text is returned without silent rewriting; absent provider choices produce a typed choice response, not an executable intent.
3. `POST /api/forge/intents/:intent_id/execute` accepts only `{operation_id,expected_digest,confirm:true}`. It cannot replace authority, action, expected identity or text. The client reserves its operation ID durably before this request. The CP also durably reserves before any mutation and permits at most one dispatch for an intent.

`GET /api/forge/intents/:id` restores the exact review, and `GET /api/forge/operations/:operation_id` returns a local receipt without contacting the provider or retrying. Repeating execute with the same operation/intent/digest only returns that operation's existing state. Rebinding an operation ID or reusing an intent under another operation is a conflict. A stale/expired intent requires fresh preparation and review; it never changes into a new write automatically.

### Authority and expected identity

All IDs are opaque strings or canonical decimal strings, avoiding native JSON floating-point conversion. `authority` includes `{host_id,instance_id,provider,account,api_origin,project_ref,provider_project_id}`. Repository actions additionally include `{ws_id,checkout_root_identity,repo_ref,origin_url_identity}`. No credential or token-derived digest is exposed. A nonsecret adapter/credential generation binds the prepared intent to the same owning configuration.

Existing-review expectations include provider review ID/number, target/source repository identities, source/target branch names, head/base SHAs and captured review state/version. Existing-issue expectations include immutable provider issue ID, full key/number, project/team ID and a hash of fetched bounded current fields. Create-review expectations additionally bind the checked source/target remote branch tips and the selected local checkout branch/HEAD. Creating an issue binds exact project/team, issue type if required, and prepared label IDs. State changes bind current state and the explicit target state/transition ID.

Execution rereads identity and required state immediately before dispatch and refuses changed selection. **This is a client-side precondition check, not provider CAS.** The forge can change between that read and the write. Comments target the reviewed issue/review, not an immutable commit. PR/MR creation follows the provider's branch-name semantics and therefore may incorporate a branch move during that window. These limits are included in the reviewed response and UI, not hidden in implementation notes.

## Mutation transport and response semantics

Do not call the existing retrying write helpers directly: GitHub wraps create/comment in retry loops, and GitLab's generic authenticated transport may repeat after 401. Use an operator-specific single-dispatch path with the owning connector's normal credential source, no redirects, no transient retry, no 401 replay and no automatic reconciliation writes. Normal pre-request OAuth refresh is allowed before final authority capture; credential changes invalidate the prepared selection. Credentials remain in memory inside the owning adapter and never enter review or receipt files.

Prepare the exact authenticated request privately before the durable dispatch marker, then send once. Network/transport error classification never assumes that a failed send means the provider did nothing. Returned resource identities must be validated against the intended provider/project/parent before reporting success. State-changing actions that return an empty successful response are `provider_acknowledged`; they do not become a verified persisted-state claim. A subsequent current read is a separate observation.

Receipt: `{host_id,operation_id,intent_id,digest,review,phase,outcome,reason,created_at,updated_at,receipt_version,durability,writer_active,result,preconditions_atomic:false,automatic_retry:false}`. Outcomes:

- `not_submitted`: preparation/preflight/refusal happened before the dispatch marker; no forge mutation was attempted.
- `accepted`: provider acknowledgement was validated (including returned identity when the action returns it) and the terminal receipt is durable. `result` states whether an object was returned or only acknowledgement was observed.
- `rejected`: an action-specific, documented definitive rejection was received; no automatic replay.
- `unknown`: dispatch may have occurred, but no trustworthy durable terminal result exists. Includes timeout, disconnection, 5xx, malformed/oversized response, uncertain GraphQL partial errors, cancellation after dispatch or process crash.

A lost receipt write after a valid provider acknowledgement leaves the durable uncertainty marker intact. The immediate response can retain bounded acknowledgement evidence with `durability:"unconfirmed"`; it must not advertise durable success. GET status never infers success from a matching title/body or from the desired current state: another actor could have produced it.

## Durable store and cancellation

Use a private CP-owned directory with descriptor-relative no-symlink traversal, owner/mode checks, regular-file and single-link checks, create-new immutable intent files, atomic receipt replacement, file/directory fsync and a cross-process nonblocking lease. Review files contain user-submitted text and are private (0600/0700). A failed or uncertain reservation prevents dispatch.

Order: acquire lease → verify immutable intent/digest and unused intent → create/fsync reservation → bounded preflight reads → fsync `dispatching` marker → one provider mutation → persist terminal receipt. Work runs in a CP-owned task so an HTTP client disconnect cannot cancel the receipt writer. Timeout after dispatch records unknown. A restart never resumes a mutation; leftover dispatching becomes unknown, and leftover reserved may be shown as not submitted only because all mutation paths require the durable dispatching marker first.

An intent is permanently bound to one operation. An unresolved operation also blocks a newly prepared identical action/authority/payload digest, preventing a fresh ID from silently bypassing uncertainty. Initial recovery is inspection and status only; acknowledging uncertainty does not authorize replay. Any later manual uncertainty-resolution/override needs its own explicit contract and must preserve the original unknown evidence.

Known terminal receipts and intent-use tombstones remain durable; expiry removes only unreferenced, unused review bodies. Capacity exhaustion is an explicit refusal, never permission to delete uncertainty or weaken deduplication. These are initial implementation bounds, not permanent product quotas. A follow-up must archive proven terminal bodies and compact them into immutable identity/idempotency tombstones while retaining uncertain evidence. Capacity refusal remains a documented limitation until that compaction is reviewed. Initial limits: 16 pending intents, 4096 operation receipts and 64 MiB aggregate journal, 64 KiB submitted body, title512B, 32 labels, 256 KiB stored intent/receipt, 2 MiB per upstream response, 512 KiB public response, 30s preparation/preflight deadline and 30s single-dispatch deadline, one cross-process writer at a time. Provider choice catalogs are bounded and reject incomplete ambiguous choice sets rather than silently selecting.

## Implementation slices and validation

After design approval: (1) durable intent/operation store and pure state machine; (2) owning writer port, routes and MCP exclusions; (3) per-provider exact preparation/single-dispatch adapters; (4) frozen DTO to native owner and compact explicit review/receipt UI. Keep these source slices independently reviewable. No unsupported operation gets an enabled capability.

Prepare fixtures for exact owner/account/origin; changed branch/head/project/issue/state/credential generation; cross-project returned resources; body/response/choice bounds; redirect refusal; single dispatch under 401/429/5xx/timeouts; every durable publication boundary; concurrent/repeated operation IDs; restart in each phase; lost accepted receipt; sticky unknown and digest reuse; and MCP exclusion. Run no live provider writes. Builds/tests/services remain in the root's validation lane after independent source review.

## Exact prerequisite provenance

- Original source: `a42e3beaa5736998fabcf9513976545044e326de`.
- G1 operator issue patch SHA256 `5846e73359a24cf9d66d9083284d804a31202d4f175eaa0a57434d50d1b0ae43`.
- G1 MCP inventory correction SHA256 `b798409ac972e04a6a40be589c03504a4ced904876ee53d3aa0b2df2779ee1f1`.
- G3 review-pages patch SHA256 `955d39d0b1be1b3f6b4eebe45a5192a6a44dd8709c8c4a595c47c5b793447bf0` (includes its transport input).
- Composed prerequisite commit: `16cfcae7f8f4a5ccf9fe5b180c273f36ffe0b81a`. Three overlap conflicts preserved both host feature gates, both GitHub client fields, and both GitLab transports. This composition is source lineage, not independent union or runtime acceptance.

- G1 HTTP error conversion correction SHA256 `003ac8c23ea73e92d3eb00a87f8351450acee4e7c27722e2ba5422c716c714b8`, imported as `05d54135` after the first warm compiler check.
- G3 pinned MCP route inventory correction SHA256 `d37e4ce8d25a120a1c76eb39354693fd57fad4fbfd9e1f91ca6d945660f6382d`, composed as the exact one-line addition after G1's inventory insertions (`d16fe6dd1240d0ff763f8c440706a2a3db255124`). This is the current G4 incremental export base.

## HTTP DTO refinement for native implementation

Preparation request remains `{host,action,target,expected,input}`. `target` is `{provider,account,api_origin,project_ref,number?,ws_id?}`; numbers are canonical decimal strings. Repository actions require `ws_id`; issue actions reject it. `input` has only action-appropriate `title,body,labels,head,base,draft,state,choice_id`; unknown or extraneous fields fail validation. Provider choice responses contain bounded `choices:[{id,name,kind}]` and no executable intent.

The initial/restore review response is `{host_id,intent_id,digest,created_at,expires_at,review,execute_supported,preconditions_atomic:false,automatic_retry:false}`. `review` is the complete immutable PreparedWrite: `{request,authority,observed,preconditions,limitations,generation}`. Times are Unix seconds. `generation` is a nonsecret equality guard; the UI should keep it in technical details, not its main consequence text.

Receipts are `{host_id,operation_id,intent_id,digest,review,phase,outcome,reason,created_at,updated_at,receipt_version,durability,writer_active,result,preconditions_atomic:false,automatic_retry:false}`. `receipt_version` is a decimal string; phase is reserved/dispatching/terminal. In-progress observations use outcome `in_progress`. Successful store publication uses durability `durable`; a later read without an active writer is `observed`; a writer-active or publication-failure observation is `unconfirmed`. Even visible terminal-file evidence remains in_progress/unconfirmed until the writer's publication boundary has completed. The native journal must retain its uncertainty frontier and must not turn `observed` into a claim of a new durable publication.

Unknown deduplication uses canonical provider/account/API origin plus immutable provider project/object IDs, action and exact input (labels sorted only for key computation). Display names, workspace aliases, captured versions and adapter epochs cannot turn the same unresolved action into a new action. The reviewed payload preserves submitted text and order.

Terminal archive follow-up: under the same exclusive lease, immutably publish/fsync a proven terminal full snapshot, then a compact lookup tombstone binding operation/intent IDs, digest, key, terminal outcome and archive SHA before reclaiming redundant active bytes. Add bounded segmented lookup/paging and verify snapshots on restoration. Never prune unresolved evidence or idempotency identity to escape capacity limits. The initial implementation only removes expired unused and redundant standalone terminal intent files; it keeps complete operation reviews and all tombstones.

The CP adds `authority.instance_id`, a nonsecret hash of its canonical global directory and filesystem identity, to the reviewed payload and rechecks it before provider preflight. The private store remains the owner of durable intent/operation IDs. This field is separate from provider credential generation.

## Initial provider qualifications

Review comments, like the current project review inspector, require the same repository on both sides of the PR/MR and the selected owning checkout branch/HEAD. Fork-review targeting is not introduced by this slice. Creating a review also requires existing remote branches in the same repository; publishing/pushing a branch remains a separate action.

GitLab labels containing commas are refused because its comma-separated label transport cannot preserve those names unambiguously. A GitLab draft creation requires the exact reviewed title to begin with `Draft: `; submitted content is never silently rewritten. Jira issue-type/transition and Linear state choices use explicit IDs even when only one eligible choice is returned.

Provider preparation/preflight has at most six logical reads for supported action/provider combinations, including the two G1 issue-version brackets; each response is capped at 2 MiB before decoding (at most 12 MiB decoded upstream). Owning OAuth refresh and G1 authentication replacement behavior occur only during preparation, before the dispatch marker. The mutation uses an already frozen credential, explicitly disables reqwest protocol retry, refuses redirects, makes one API attempt, and caps its reply at 2 MiB. Errors and ambiguous responses remain bounded unknown observations.

GitLab descriptions/comments can execute platform quick actions, including operations outside this contract. This editor refuses submitted GitLab body lines beginning with `/` after leading whitespace; it does not rewrite them. The rule also covers carriage-return and Unicode line boundaries. Source: [GitLab quick actions](https://docs.gitlab.com/user/project/quick_actions/) and [GitLab MCP tools](https://docs.gitlab.com/user/model_context_protocol/mcp_server_tools/). The current [Notes API](https://docs.gitlab.com/api/notes/) documents project and parent identifiers, so strict comment acknowledgement identity validation is retained. These were public documentation reads, not provider API calls.

## Validation handoff

This source slice has independent store, HTTP lifecycle/CLI, and provider-transport reviews. It has not been compiled, tested, deployed, or exercised against a provider in this lane. Forty authored fixtures comprise 18 store, 7 HTTP lifecycle/classification, and 15 provider/CLI/token fixtures. The owning validation lane should run `rupu-cp` library filters `forge_write_store` and `api::forge_writes`, its integration `mcp_routes` filter, `rupu-scm` library filters `forge_write` and `token::tests`, and `rupu-cli` library filter `cp_forge_writer`. The token filter intentionally includes existing refresh regression tests because the atomic snapshot refactors its common token path. Build both default and supported no-default feature configurations in that lane. Public documentation research is separate from provider API acceptance.

## Native request field reference

`expected` is a map of exact string values. Its allowed keys depend on action; omitted optional keys are captured by preparation, then every captured observation is checked at execution. Existing issue actions require the G1 current-read `version`; review comments require the currently inspected `head_sha`.

| Action | Input | Allowed expected keys |
| --- | --- | --- |
| `review.comment` | required nonempty `body` | `provider_project_id`, `object_id`, required `head_sha`, `base_sha`, `state` |
| `review.create` | required `title`, `head`, `base`; optional `body`, `draft` | `provider_project_id`, `head_sha`, `base_sha` |
| `issue.comment` | required nonempty `body` | required `version`, `provider_project_id`, `object_id`, `state` |
| `issue.create` | required `title`; optional `body`, `labels`; Jira requires an explicit `choice_id` issue type | `provider_project_id` |
| `issue.set_state` | required `state` (`open` or `closed`); Jira/Linear require exact `choice_id` | required `version`, `provider_project_id`, `object_id`, `state` |

Other action-inappropriate input fields are rejected. `labels` defaults to an empty array; optional inputs can be omitted or null. Provider names serialize as `github`, `gitlab`, `jira`, `linear`. Existing-item number is a canonical positive decimal string even for Jira/Linear; the selected exact project ref carries the key prefix. The explicit account catalog is capped at 64 rows. Choice responses use HTTP409 `{code:"choice_required",error,choices:[{id,name,kind}]}` and contain no intent; choosing a provider ID triggers a new preparation. Other failures use `{code,error,choices?}`. HTTP400 indicates invalid input (including unsupported GitLab quick-action text), 409 changed selection/conflict, 501 unsupported connector action, and 503 busy/unavailable/capacity. Never interpret an HTTP failure after execute as proof of non-submission: retain the operation ID and read its local status.

Intent IDs are server generated. Operation IDs must be generated and durably retained by the native client before execute; they must contain 1–80 ASCII alphanumeric, hyphen or underscore characters (UUID strings are valid). `execute_supported` indicates that the response supports this protocol, not that an old/restored intent is unused, unexpired, or still current. Execution enforces those properties. Treat the complete prepared `review` as immutable, show its exact target/consequence/text, and send only its digest with the operation ID and explicit confirmation.

## Guarded GitHub Actions dispatch (additive source slice)

`workflow.dispatch` is a repository action advertised only by the GitHub owning Forge adapter. It reuses the existing prepare, durable single-dispatch reservation, preflight and operation lookup lifecycle. It never calls the older retrying `GithubExtras.workflows_dispatch`. The exact registered project, checkout identity/origin and configured account remain mandatory; a workflow's display name or recorded trigger cannot supply that authority. No new MCP tool, route or model grant is added.

Input fields are `workflow`, `ref_name`, `ref_kind` and `workflow_inputs`. Other action fields are refused. `workflow` is a canonical positive UInt64 decimal ID or an ASCII basename of at most255 bytes using letters, numbers, underscore, dot and hyphen, ending in `.yml` or `.yaml`; leading dot and `..` are refused. `ref_kind` is explicitly `branch` or `tag`. `ref_name` is at most255 bytes and uses the same literal Git ref-name guard as branch creation: no `HEAD`, `@`, `refs/` prefix, whitespace/control, revision expressions, empty path components, dot-leading components or `.lock` suffix. There is no branch/tag guessing. `workflow_inputs` is a string map (omitted means empty; null and nonstring values are invalid): at most25 keys, each1–128 ASCII letters/numbers/underscore/hyphen, each value at most8192 UTF-8 bytes without NUL, and at most65536 key-plus-value bytes total. Values are preserved without coercion or Unicode normalization. The provider validates its declared input schema when dispatching; preparation does not claim to have parsed workflow file inputs or confirmed a `workflow_dispatch` trigger.

Optional expected keys are `provider_project_id`, `workflow_id` and `ref_sha`. Preparation performs four bounded reads: exact repository, selected workflow, selected branch/tag ref, and opposite-namespace absence. The repository identity and workflow API URL are verified. A numeric selector must match the returned immutable workflow ID; a filename must match `.github/workflows/<filename>`. The workflow must be active. The ref URL and `refs/heads/...` or `refs/tags/...` identity must match. An existing or unavailable opposite namespace refuses preparation, rather than silently choosing precedence.

Observed proof keys are `provider_project_id`, `workflow_id`, `workflow_path`, `workflow_state:"active"`, `ref_name`, `ref_kind`, `ref_sha`, `ref_object_type`. Branches require `commit`; tags allow `commit` or `tag`. An annotated tag records its tag-object SHA without recursive peeling and does not claim that SHA is a commit. `observed.object_id` and `authority.provider_object_id` are the same immutable workflow ID. Execution re-reads the full observation before dispatch. The forge can still move a ref or workflow after that check: the request submits a ref name, not a commit CAS.

The Actions read and mutation explicitly pin `X-GitHub-Api-Version: 2022-11-28`; the mutation uses the resolved numeric workflow ID and sends `{ref,inputs,return_run_details:false}` exactly once. Under this pinned contract only HTTP204 with no body bytes exposed by the HTTP client is accepted. JSON `null` is not treated as an empty received body. This does not claim to inspect bytes suppressed by the HTTP implementation for a204 response. HTTP200 with run details, malformed/oversized responses, transport loss and5xx remain unknown; this slice does not infer a run ID or query by name/time to manufacture one. Result contains the eight proof fields plus `effect:"provider_acknowledged"` and `verification:"not_performed"`, with no `resource_id` or `run_id`. Provider acknowledgement does not establish eventual run creation, completion or success. API reference: [GitHub workflow dispatch, version2022-11-28](https://docs.github.com/en/enterprise-cloud%40latest/rest/actions/workflows?apiVersion=2022-11-28#create-a-workflow-dispatch-event).

Unknown-operation deduplication requires immutable workflow authority and removes only the submitted workflow filename/ID alias from its key. Thus the same workflow selected by filename or numeric ID cannot evade unresolved evidence. Exact ref kind/name and inputs remain key components; the reviewed request retains the original selector. The new optional input fields are omitted during serialization when absent, preserving old saved intent shapes and digests.

This isolated source patch is based on `fa41010cb5fe1bd092dd0c325875d499d96c752e`. Thirteen new fixture declarations cover typed bounds/foreign providers, old input serialization, exact reads and204 proof, changed workflow/ref/namespace, inactive or mismatched workflow, annotated tag semantics, numeric identity, no mutation retries/credential rotation, received empty bytes versus JSON null, contradictory acknowledgement/rejection carrying run evidence, exact owner and alias-stable deduplication. They are authored and source-reviewed only; no build, test execution, provider request, service, GUI or real dispatch occurred. Later owning validation should run `rupu-scm` Forge write tests, `rupu-cp` Forge routes/store tests and the CLI Forge writer tests with the normal feature configuration, preserving existing lifecycle and account-selection coverage.
