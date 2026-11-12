# Explicit continuation list options v1

Source-only implementation based on fa41010cb5fe1bd092dd0c325875d499d96c752e. No service, provider, build or API acceptance performed.

## Native contract

Positive `agent.continue_options.v1` appears in `/api/host/info` only when the actual installed AgentLauncher implements `supports_continuation_options()`. The default trait implementation is false and its new operation refuses. Existing AgentLaunchRequest literals and the legacy continuation route retain their contract.

`POST /api/runs/agents/:id/continue-options?host=local` is a new strict local route. It requires a JSON object with `lists`, plus optional existing `prompt`, `model`, `provider`, `mode`, `findings_profile`, and `messaging` fields. Unknown body/query fields are rejected. Legacy top-level skills/preload_skills/engagement_profiles fields are not accepted, even if a corresponding nested field is absent. There is no fallback to `/continue`. This new operator HTTP review route is intentionally excluded from the MCP operation catalog; it adds no model-callable tool or permission grant.

`lists` accepts only `skills`, `preload_skills`, and `engagement_profiles`, each an array of strings. At least one key must be present. Missing inherits the corresponding original launch list. `[]` clears that list's launch contribution. Nonempty replaces that list. Null is rejected. Each list has at most 256 entries, each at most 1024 UTF-8 bytes without control characters or blank values; the JSON selection is bounded at 256 KiB. Skill grant syntax is validated before dispatch; profile IDs follow existing trimming behavior. Catalog existence remains the existing worker's resolution responsibility.

Success is the unchanged 202 receipt `{run_id,continues,host_id:"local"}`. It acknowledges one dispatch, not successful provider execution. Malformed JSON/shape produces extraction 400/422; semantically invalid input 400; unsupported adapter or remote host 501; missing run 404; current liveness/directory restrictions 409. The route reuses existing local run lookup, prologue/workspace resolution, liveness checks and one-launch dispatch. It adds no filesystem or revision CAS claim and no idempotency/retry promise.

## CLI and recording

The local adapter carries a typed `ContinuationListOverrides` in runtime `AgentRun`, rendered as hidden `--continue-options=<JSON>`. It requires `--continue` and conflicts with legacy list flags, preventing ambiguous precedence. This request-only action is consumed after recorded command restoration and pre-command transcript skill carrying. The original agent's declared skills, shipped skill bundle, provider/mode selection and earlier conversation are untouched.

The resolved three launch lists are recorded as hidden `--launch-lists=<JSON>` alongside ordinary nonempty list flags. Empty lists therefore remain explicit in `RunStart.launch_args`. A later continuation hydrates the recorded marker before ordinary explicit-flag merging. New command-line list flags win over the old normalized marker, and the next marker records that resolved state. The one-shot `--continue-options` and `--continue` action are not carried into recorded launch arguments.

For old transcripts without launch_args, existing skill carrying runs before these independent list overrides. Clearing one list does not prevent the other from inheriting. A fresh CLI launch without this feature remains unchanged. Legacy `/continue` still treats empty lists as inherit.

Clearing skills/preloads does not erase previously loaded skill bodies or messages from the rebuilt transcript. It does not remove agent declarations or shipped bundle content. Empty engagement selection uses the existing native/default code path; it is not a declaration that findings have no schema.

## Prepared validation

New runtime shape/tri-state/roundtrip tests, CP opt-in capability and strict-route dispatch/refusal tests, local adapter argv tests, and CLI restore/legacy-carry/repeated-continuation fixtures are authored but unrun. Existing argv roundtrip fixture coverage includes explicit list JSON. Independent source review and scheduled combined compilation/tests are required before runtime support or native acceptance is claimed.
