# Session runtime controls

Companion backend for the RupuMac session workspace. Based on origin/main 74384e23.

## API

`GET /api/sessions/:id/settings?host=local` returns `session_id`, `host_id`, `defaults` (provider/model/tools), `settings` (revision/provider/model/tools), `permission_mode`, the shared `tools` catalog, and `applies: next_turn`.

`PUT` to the same path accepts exactly the settings object. Supply the revision from GET. Null inherits the agent values frozen when the session started. An empty tools array means no declared tools; normal ambient grants and permission-mode checks still apply. A provider override requires a model identifier. Catalog tool names and aliases are validated by the existing ToolCatalog.

Writes require a writable `rupu cp serve` deployment, matching the existing lifecycle controls. Successful writes increment the revision. Concurrent stale writes return a conflict; reload before retrying. Settings are atomically replaced in `settings.json` under a dedicated lock. The worker's session history writer does not own this file. Archived sessions and remote proxy targets are refused; connect directly to the owning control plane.

## Worker semantics

The worker reads a settings snapshot at the start of each turn, overlays a temporary AgentSpec, and uses the existing RunAssembler. The actual provider/model and effective grants retain the normal transcript provenance. No agent definition or original session defaults are rewritten. Provider-specific authentication/options are cleared on provider changes. Model changes invalidate inherited limits, effort and context pins; override limits never replace the default model's cached limits. Explicit model/provider selections disable the agent/config fallback chain, including selecting the original model explicitly. Tools-only edits preserve that chain.

Queued messages use request version 3 after settings have been written. Old workers reject that version rather than ignoring it; normal stale-binary detection restarts workers on enqueue. Deploy the new control plane and worker together. Saving settings alone does not replace an already running turn's provider.

Manual compaction after a model change requires an explicit window and uses the configured provider/model. Its threshold uses that window and the agent's compaction percentage, without the original model's cached output headroom or learned limits. Worker compact pseudo-turns without that contract refuse. Automatic per-turn compaction continues through the normal assembler.

## Validation and limits

Tests cover atomic/revisioned persistence, preserved history/defaults, invalid grants, host/path affinity, read-only refusal, API conflicts, and a mock-provider turn that records the overridden model while retaining the original model's limits. Model identifiers and authentication are finally checked by the normal provider setup; a settings receipt does not assert a model can be reached.

This branch adds local session configuration. It does not add remote proxy configuration, arbitrary CLI slash-command dispatch, attachment upload, queue editing, checkpoint restore, or Agentiflow-specific services to standalone sessions. No installed production CLI or user session is changed by the implementation tests.

## Verification receipt

- `cargo check -p rupu-runtime -p rupu-cp -p rupu-cli --no-default-features` passed.
- Runtime settings tests: 4 passed (revision/atomic replacement, validation, isolated AgentSpec overrides, and explicit original-model selection disabling fallback while tools-only edits preserve it). The new fallback regression failed before the fix.
- API settings test: passed (history preservation, host/path checks, stale-write conflicts, invalid grants, read-only refusal, and archived-session refusal).
- Mock worker/compaction tests: 2 passed; verify actual model/provider, effective tool grant, preserved defaults/history/limits, request version 3 for configured turns, explicit compaction windows without stale output headroom, and refusal of compaction paths that lack a window after a model change.
- Existing compaction-to-next-turn history regression: passed.
- Strict Clippy is not clean on this checkout/toolchain. The dependency-inclusive invocation stops on existing `async_trait`/`double_must_use` diagnostics in `rupu-netflow`. `--no-deps` reaches `rupu-cp` but stops on 88 existing macro-generated `double_must_use` diagnostics and one existing `result_large_err` in `api/findings.rs:890`. Those files were not changed by this work.
- Default PDF-feature builds and real provider authentication/switching have not been exercised. The test provider is mocked; no installed CLI or live session was modified.

## Integration

The command-parity and orchestration branches started from foundation commit `1624fe99`. Integrate the final `codex/session-runtime-controls` branch before either dependent branch; those branches must include the review follow-up for fallback and compaction correctness. The `GET`/`PUT /api/sessions/:id/settings?host=local` request and response contract is unchanged by that follow-up. Native callers do not need a schema change.
