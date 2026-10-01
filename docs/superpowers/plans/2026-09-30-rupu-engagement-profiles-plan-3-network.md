# Engagement profiles — Plan 3 (network engagement + RoE enforcement) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `network` engagement as data on the Plan 1/2 ABI, render its evidence properly, and add **real** scope / rules-of-engagement (RoE) enforcement — honest about its boundary: enforced in typed network tools, fail-closed for `bash`/opaque tools, with a full egress sandbox deferred.

**Architecture:** An operator-authored, immutable `ScopePolicy` is compiled once at launch from a scope root (never the agent-writable asset graph). It rides on `ToolContext` (and the MCP dispatcher, like `FindingsContext`) so both tool-authorization chokepoints — the agent runner loop and the MCP dispatcher — can consult it. Typed network tools resolve their target (DNS for hostnames/URLs) and check it against the policy, failing closed via a new `ToolError::ScopeDenied`. Under an enforcing engagement, `bash` and other opaque tools are refused (they cannot be scoped in-band) unless the operator explicitly opts out, which records a loud `roe_unenforced` audit marker. `network` composes with `web` as `pentest`, exercising Plan 2's per-origin routing.

**Tech Stack:** Rust 2021, serde, toml, tokio; `ipnet` (CIDR) + `hickory-resolver`/std DNS for host resolution (confirm workspace deps). TDD with `#[test]` + mock-provider integration.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md` — **this plan amends it** (Task 0: network:scope hostname policy + `[scope]` as a first-class profile field).
**Builds on:** Plan 1 (contract, #687) and Plan 2 (wiring) — **both merged to `main` first.** Plan 3 assumes `FindingWriteOptions.engagement`, per-origin routing, `assets.jsonl`, and profile-aware `report_finding` exist.

## Global Constraints

- MSRV pinned; `#![deny(clippy::all)]`/`#![forbid(unsafe_code)]`; workspace deps only (add `ipnet`, a DNS resolver to the root `[workspace.dependencies]` if absent — do not pin in a crate; if a resolver is already vendored, prefer it).
- **No silent no-ops (standing rule).** Scope enforcement must be real where claimed and refused where it cannot be real. `bash`/opaque tools under an enforcing engagement are denied (or stripped) — never silently allowed through. Any operator opt-out is loud: a `roe_unenforced` audit event per run.
- **Trust boundary.** `ScopePolicy` is compiled once at launch from an **operator-authored** scope source, immutable thereafter. Agent-declared/discovered assets (`network:host`, `network:service`) are *children under* the scope, never authorization. An agent cannot widen its own scope.
- **Fail closed everywhere:** malformed scope input → launch error; an unresolved hostname → deny; a live-touch tool with no `ScopePolicy` under an enforcing engagement → deny; a scope-denial reported accurately as `blocked` in the audit.
- **Additive & backwards-compatible:** `ScopeSpec` on `EngagementProfile`, the `ToolContext`/dispatcher scope field, and `ToolError::ScopeDenied` are additive; non-network engagements (`code`/`binary`) carry no scope and behave exactly as after Plan 2.
- **Deferred (refuse/defer, do not fake):** a real egress sandbox for arbitrary subprocesses (the microVM/netfilter arc the netflow spec already defers); a full scanner suite (Plan 3 ships the typed-tool *pattern* + one tool); remote/placed network engagements; the CP web scope editor.
- No assessment-derived data (public repo); all scope/target fixtures are synthetic (RFC-5737 `192.0.2.0/24`, `example.com`).
- Gate before each commit: touched-crate `cargo test` + `cargo clippy -p <crate> --all-targets -- -D warnings`; workspace `--locked` check at the end.

---

## File Structure

**`crates/rupu-coverage/`**
- `src/profile/package.rs` — `ScopeSpec` field on `EngagementProfile`; consider `deny_unknown_fields`.
- `src/profile/loader.rs` — merge `scope` in `expand_includes` (`enforce_in_scope` OR'd).
- `src/profile/mod.rs` — `builtin_profiles()` registers `network`; `ActiveSet` scope accessor.
- `src/profile/builtin/network.toml` — the network profile (with `[scope]` incl. hostname policy).
- Create `src/scope/mod.rs` — `ScopePolicy`, `Target`, `scope_denied` reasons; CIDR + hostname matching; `ScopePolicy::compile(scope_root_toml) -> Result<ScopePolicy, ScopeError>`.
- `src/tools/report_finding.rs` — reject a `host`/`endpoint`-scoped finding whose target is out of policy (mis-filing guard).

**`crates/rupu-tools/`**
- `src/tool.rs` — `ToolContext.scope: Option<Arc<ScopePolicy>>` + `Default`; `ToolError::ScopeDenied { target, reason }`; a `Tool::live_touch(&self) -> LiveTouch` classifier (`None | Targets(fn(&Value)->Vec<Target>) | Opaque`).
- Create `src/net_http.rs` — a typed `http_request` tool (method/url/headers/body) that resolves + scope-checks before connecting; registered only when a policy exists.

**`crates/rupu-agent/`** — `src/runner.rs` (gate at ~1525-1604 consults scope + ScopeDenied→`blocked`; registry build at ~942 strips `bash`/opaque under enforcement; dispatcher `.with_scope` at ~1121), `src/mcp_tool.rs` (forward scope), `src/tool_registry.rs` (register `http_request`).

**`crates/rupu-mcp/`** — `src/dispatcher.rs` (`with_scope` + check before `match`), `src/tools/mod.rs` (`ToolSpec` live-touch/target metadata), bless snapshot; `rupu-cp/src/api/tools.rs` mirror.

**`crates/rupu-cli/` + `rupu-orchestrator/`** — thread `ScopePolicy` through every `ToolContext`/dispatcher construction site (run/session/dispatch/step_factory/workflow/resume/mcp serve), compiled from the operator scope source; children inherit via the dispatcher struct.

**`crates/rupu-findings-report/`** — `src/blocks.rs` (http_exchange labels + header redaction, scan ANSI strip, artifact resolution) + tests.

**Docs/spec/samples** — spec amendment; `docs/coverage.md`/`workflow-format.md`; `.rupu/engagements/<name>/scope.toml` example; a `pentest` composite profile; `.rupu/workflows/network-assessment.yaml`.

---

## Task 0: Spec amendment — scope as first-class + hostname policy

**Files:** Modify `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`. No code.

The spec's `network` profile shows `[scope]` with `enforce_in_scope`, but (a) `EngagementProfile` has no `scope` field so it is silently dropped today, and (b) scope is modeled as CIDRs while real targets are hostnames/URLs. Amend the spec to make `[scope]` a first-class `EngagementProfile` section and to define the **hostname policy**: a scope carries `cidrs`, `hosts` (exact/suffix host patterns), `out_of_scope`, and `window`; hostname/URL targets are resolved at connect time and every resolved IP must fall in `cidrs` (or the host match `hosts`), failing closed on unresolved names, with a note on redirects/rebinding. Record the operator-authored, immutable-at-launch trust boundary.

- [ ] **Step 1:** write the amendment (scope schema + hostname policy + trust boundary + the honest enforcement boundary: typed tools enforced, bash deferred).
- [ ] **Step 2:** commit `docs(spec): scope as first-class profile section + hostname/RoE policy`.

---

## Task 1: `ScopeSpec` on EngagementProfile (stop dropping `[scope]`)

**Files:** Modify `rupu-coverage/src/profile/package.rs`, `src/profile/loader.rs`. Test: inline.

**Interfaces:**
- Produces: `ScopeSpec { #[serde(default)] enforce_in_scope: bool, #[serde(default)] cidrs: Vec<String>, #[serde(default)] hosts: Vec<String>, #[serde(default)] out_of_scope: Vec<String>, #[serde(default)] window: Option<String> }`; `EngagementProfile.scope: Option<ScopeSpec>` (`#[serde(default)]`). `expand_includes` merges `scope` from includes: union `cidrs`/`hosts`/`out_of_scope`, OR `enforce_in_scope`.

- [ ] **Step 1: failing test** — a profile TOML with `[scope] enforce_in_scope=true, cidrs=["192.0.2.0/24"]` parses into `Some(ScopeSpec{..})` (today it's dropped); a composite `includes=[network]` inherits `enforce_in_scope=true`.
- [ ] **Step 2: run, expect fail** (field dropped today).
- [ ] **Step 3: implement** the struct + field + merge in `expand_includes`. Consider adding `#[serde(deny_unknown_fields)]` to `EngagementProfile` so a future dropped section is a loud error (verify it doesn't break the existing built-ins + discovered profiles first; if risky, defer deny_unknown_fields with a note).
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): first-class ScopeSpec on EngagementProfile (+ include merge)`

---

## Task 2: `ScopePolicy` compile + matching

**Files:** Create `rupu-coverage/src/scope/mod.rs`; export from `lib.rs`. Test: inline.

**Interfaces:**
- Consumes: `ScopeSpec` (T1).
- Produces: `Target` (`Ip(IpAddr) | Host(String) | Url(String)`); `ScopeError`; `ScopeDecision` (`InScope | OutOfScope(reason) | Unresolvable(reason)`); `ScopePolicy { enforce: bool, nets: Vec<IpNet>, host_patterns: Vec<HostPattern>, out_of_scope: Vec<IpNet/HostPattern>, window: Option<Window> }`; `ScopePolicy::compile(&ScopeSpec) -> Result<ScopePolicy, ScopeError>` (parse CIDRs/hosts, fail closed on malformed); `ScopePolicy::check_ip(IpAddr) -> ScopeDecision` and `check_host(&str, resolved: &[IpAddr]) -> ScopeDecision` (host must match `hosts` OR every resolved IP in `nets`, none in `out_of_scope`); DNS resolution is done by the *caller* (the tool) and passed in, so this module stays pure/sync/testable.

- [ ] **Step 1: failing tests** — compile a policy from `cidrs=["192.0.2.0/24"], out_of_scope=["192.0.2.1/32"]`; `check_ip(192.0.2.5)=InScope`, `check_ip(192.0.2.1)=OutOfScope`, `check_ip(198.51.100.1)=OutOfScope`; `check_host("example.com", &[192.0.2.5])=InScope`, same host with an out-of-scope resolved IP = OutOfScope; malformed CIDR → `compile` errors.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** with `ipnet` (confirm/add workspace dep). Keep it pure (no DNS here).
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): ScopePolicy — CIDR/host matching, fail-closed compile`

---

## Task 3: `network.toml` built-in profile + registration

**Files:** Create `rupu-coverage/src/profile/builtin/network.toml`; modify `src/profile/mod.rs` (`builtin_profiles()` + `ActiveSet` scope accessor). Test: inline.

**Interfaces:**
- Produces: the `network` profile as data — kinds `scope`(root)/`host`/`service`, coordinates host/port/url, evidence_blocks text/table/scan_output/http_exchange/pcap_ref, classification_systems CVE/CWE/CAPEC/ATT&CK, completeness (service pinned to host+port via `locator_has_coordinate`, scored), coverage ladder discovered→enumerated→tested→exploited, `[scope] enforce_in_scope=true` (operator fills cidrs/hosts per engagement). `builtin_registry()` registers it; `ActiveSet::scope_spec() -> Option<&ScopeSpec>` returns the active engagement's scope (from the scope-owning profile).

- [ ] **Step 1: failing test** — `builtin_registry().active_set(["network"])` resolves, `scope_spec()` is `Some` with `enforce_in_scope=true`; `profile_for_kind("network:service")` routes to `network`.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the TOML + registration + accessor.
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): built-in network engagement profile`

---

## Task 4: `ToolError::ScopeDenied` + ToolContext scope + accurate `blocked`

**Files:** Modify `rupu-tools/src/tool.rs` (`ToolContext.scope`, `Default`, `ToolError::ScopeDenied`, `Tool::live_touch`); `rupu-agent/src/runner.rs` (gate derivation at ~1604). Test: inline.

**Interfaces:**
- Produces: `ToolContext.scope: Option<std::sync::Arc<rupu_coverage::scope::ScopePolicy>>`; `ToolError::ScopeDenied { target: String, reason: String }`; `enum LiveTouch { None, Targets, Opaque }` + `Tool::live_touch(&self) -> LiveTouch` (default `None`). In the runner gate, a returned `ToolError::ScopeDenied` sets the audit `blocked = true` (today only `PermissionDenied` does).

- [ ] **Step 1: failing test** — a stub tool returning `ScopeDenied` is audited as `blocked:true`; `ToolContext::default().scope` is `None`; `live_touch()` defaults to `None`.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the field/variant/classifier + extend the `blocked` derivation (runner.rs:1604) to include `ScopeDenied`.
- [ ] **Step 4: run, expect pass** `cargo test -p rupu-tools -p rupu-agent`.
- [ ] **Step 5: commit** `feat(tools): ScopeDenied error + scope on ToolContext + live-touch classifier`

---

## Task 5: typed `http_request` tool with in-tool scope enforcement

**Files:** Create `rupu-tools/src/net_http.rs`; register in `rupu-agent/src/tool_registry.rs` (only when a policy is present). Test: inline (policy check) + a gated live test (ignored by default).

**Interfaces:**
- Consumes: `ScopePolicy` (T2), `ToolContext.scope` (T4).
- Produces: `HttpRequestTool` with `Input { method, url, #[serde(default)] headers, #[serde(default)] body }`, `live_touch()=Targets`. On invoke: parse the URL host, resolve it (DNS), `scope.check_host(host, &resolved)`; `OutOfScope`/`Unresolvable` → `Err(ToolError::ScopeDenied{..})` BEFORE any connection; `InScope` → perform the request and return status/headers/body as an `http_exchange`-shaped result. If `ctx.scope` is `None` under an enforcing engagement, refuse (the registry only adds this tool when a policy exists, so the `None` path is defensive).

- [ ] **Step 1: failing tests** — with a policy allowing `192.0.2.0/24`: a request to a host resolving outside the range returns `ScopeDenied` and makes NO connection (inject a fake resolver in the test); an in-scope host passes the check. Unresolvable host → `ScopeDenied`.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** (resolver injectable for tests; real DNS in prod). Register in `default_tool_registry` guarded on policy presence.
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(tools): http_request typed tool with pre-connect scope enforcement`

---

## Task 6: fail-closed `bash`/opaque under an enforcing engagement

**Files:** Modify `rupu-agent/src/runner.rs` (registry build ~942 + gate). Test: inline.

**Interfaces:**
- Produces: when the active engagement `enforce_in_scope` is true, `bash` and any `Tool::live_touch()==Opaque` tool are removed from the registry at build time (so the agent cannot call them), UNLESS an explicit operator opt-out (`[engagement].allow_unscoped_tools=true` in config / a run flag) is set — in which case they remain but the run emits one loud `roe_unenforced` audit event naming the unscoped tools. Never silently allow.

- [ ] **Step 1: failing tests** — under an enforcing policy, the built registry excludes `bash`; with the opt-out set, `bash` is present AND a `roe_unenforced` event is recorded once.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the strip + opt-out + audit marker.
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(agent): fail-closed bash/opaque tools under enforcing engagement (loud opt-out)`

---

## Task 7: scope threading (all construction sites + child dispatch + MCP)

**Files:** Modify `rupu-agent/src/runner.rs` (dispatcher `.with_scope`), `src/mcp_tool.rs` (forward); `rupu-mcp/src/dispatcher.rs` (`with_scope` + check before `match`), `src/tools/mod.rs` (`ToolSpec` live-touch/target metadata) + bless snapshot + `rupu-cp/src/api/tools.rs` mirror; `rupu-cli/src/cmd/{run,session,dispatch,mcp}.rs`, `rupu-orchestrator/src/{step_factory,runner}.rs`, `rupu-cli/src/{resume,workflow}.rs`. Test: inline + an integration check.

**Interfaces:**
- Produces: a `ScopePolicy` compiled once at launch from the operator scope source (`.rupu/engagements/<name>/scope.toml`, or the operator-authored scope root of the selected engagement — NOT the asset graph) and installed on every `ToolContext` (run.rs:848, session.rs:7632, step_factory.rs:434) and on the MCP dispatcher (workflow.rs:3278/4857, resume.rs:333) via `with_scope`, mirroring `findings`/`FindingsContext`. **Children** carry it on `CliAgentDispatcher`/the dispatcher struct (dispatch.rs), never relying on `ToolContext` inheritance. `rupu mcp serve` with no engagement → live-touch tools denied.

- [ ] **Step 1: failing test** — an integration/unit check that a child agent (`dispatch_agent`) under an enforcing engagement has a non-`None` scope (today a fresh child ctx would be `None`); the MCP dispatcher denies a live-touch catalog tool when its target is out of scope.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the `with_scope` plumbing across the sites (the recon lists each), compiling the policy at launch; bless the MCP snapshot.
- [ ] **Step 4: run, expect pass** across the touched crates.
- [ ] **Step 5: commit** `feat: thread ScopePolicy through run/session/dispatch/workflow/mcp (children included)`

---

## Task 8: out-of-scope finding-recording guard

**Files:** Modify `rupu-coverage/src/tools/report_finding.rs`. Test: inline.

**Interfaces:**
- Produces: when `opts.scope` is enforcing and a finding's asset/`scope` targets a host/endpoint, reject (add to `problems`) if that target is out of policy — a cheap guard against mis-filing an out-of-scope finding, independent of live-touch enforcement.

- [ ] **Step 1: failing test** — a `network:service` finding whose host is out of policy is rejected; an in-scope one passes.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** (reuse `ScopePolicy::check_*`; extract host/port from the asset locator).
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): reject out-of-scope host/endpoint findings under enforcing engagement`

---

## Task 9: evidence-block renderer polish + redaction

**Files:** Modify `rupu-findings-report/src/blocks.rs` (and the `evidence_block_to_blocks` signature if artifact resolution needs `&[ArtifactRef]`). Test: inline across md (+ html if cheap).

**Interfaces:**
- Produces: `HttpExchange` renders labeled Request/Response with **redacted** `Authorization`/`Cookie`/`Set-Cookie` headers; `ScanOutput` strips ANSI escapes; `PcapRef`/`Image`/`Hexdump` resolve their `artifact` against `report.artifacts` for path/size (new `&[ArtifactRef]` param threaded from `full_blocks`). No new `EvidenceBlock` variants.

- [ ] **Step 1: failing tests** — an `http_exchange` with an `Authorization: Bearer x` header renders with it redacted and Request/Response labels; a `scan_output` with ANSI codes renders clean.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the labeled layout + redaction list + ANSI strip + artifact resolution.
- [ ] **Step 4: run, expect pass** `cargo test -p rupu-findings-report`.
- [ ] **Step 5: commit** `feat(findings-report): labeled+redacted http_exchange, clean scan_output, artifact resolution`

---

## Task 10: `pentest` composite + end-to-end network run

**Files:** Create `.rupu/engagements/example/scope.toml` (synthetic), a `pentest` built-in composite (`profile/builtin/pentest.toml` = `includes=[network, web]`), `.rupu/workflows/network-assessment.yaml`; an integration test. Test: the integration test IS the deliverable.

**Interfaces:**
- Consumes: the whole stack + Plan 2 per-origin routing.
- Produces: a `pentest` composite that activates `network`+`web`; an end-to-end test (MockProvider) under an enforcing synthetic scope asserting: an in-scope `http_request` succeeds (fake resolver) and its finding routes to `web` (a `web:route` finding) while a `network:service` finding routes to `network` — **per-origin routing**, each validated against its own completeness; an out-of-scope `http_request` is `ScopeDenied`; `bash` is absent from the registry; the asset graph holds the scope root + discovered host/service/route.

- [ ] **Step 1: write the failing integration test.**
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: author the composite + scope + workflow; wire any missing launch-time scope-source discovery.**
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `test: end-to-end pentest (network+web) — per-origin routing + scope enforcement`

---

## Task 11: Docs

**Files:** Modify `docs/coverage.md`, `docs/workflow-format.md`, `CLAUDE.md`. No tests.

Document the `network` profile, authoring `[scope]` + the operator scope source, the hostname/DNS policy, the typed-tool enforcement boundary + the fail-closed bash handling + the loud opt-out, and the deferrals (egress sandbox, scanner suite, remote).

- [ ] **Step 1:** write docs referencing the real fields/flags/paths.
- [ ] **Step 2:** commit `docs: network engagement + scope/RoE authoring and boundaries`.

---

## Self-Review

**Spec coverage:** network profile as data (T3) ✔; scope first-class, no longer dropped (T0/T1) ✔; ScopePolicy operator-authored + immutable + fail-closed (T2/T7) ✔; real enforcement in typed tools (T5) ✔; honest fail-closed for bash/opaque, loud opt-out (T6) ✔; accurate audit (T4) ✔; renderer polish + redaction (T9) ✔; per-origin routing exercised by pentest (T10) ✔; out-of-scope mis-filing guard (T8) ✔.
**Honest boundaries (stated, not faked):** egress sandbox for arbitrary subprocesses deferred (microVM/netfilter arc); only one typed network tool ships (pattern for the scanner suite); remote network engagements + CP scope editor deferred.
**Type consistency:** `ScopeSpec` (T1)→`ScopePolicy::compile` (T2)→`ToolContext.scope` (T4)→`http_request` (T5)/threading (T7)/report_finding guard (T8); `ScopeDenied`/`LiveTouch` (T4) used by T5/T6/T7; `ActiveSet::scope_spec` (T3) used by T7.
**Dependency note:** requires Plan 2 merged (engagement selection + per-origin routing + asset persistence). If Plan 2 slips, T7's scope source still works standalone via `.rupu/engagements/<name>/scope.toml`, but selection/routing come from Plan 2.

## Follow-on (deferred, tracked)

- Egress sandbox (microVM/netfilter) for opaque subprocesses — the only way to enforce scope for `bash`-launched tooling.
- A full typed scanner suite (nmap/nuclei/httpx/tls) on the `http_request` pattern.
- Remote/placed network engagements (connector + capability) and the CP web scope editor.
