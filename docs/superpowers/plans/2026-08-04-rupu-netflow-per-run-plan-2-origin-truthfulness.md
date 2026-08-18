# Netflow Plan 2 — remove the `Origin` variants that describe nothing

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete `Origin::Mcp` and `Origin::Webhook`, so the enum enumerates egress that can actually occur.

**Architecture:** Pure subtraction. Neither `rupu-mcp` nor `rupu-webhook` makes outbound HTTP, so neither variant can ever be constructed. Removing them makes the enum a truthful list and retires the standing rule that CP filter chips must not be built from it.

**Tech Stack:** Rust 2021; TypeScript for the mirrored CP types.

**Spec:** `docs/superpowers/specs/2026-08-04-rupu-netflow-per-run-ledger-design.md` §7
**Depends on:** nothing in Plan 1 — this plan is independent and can land in either order.

## Why deletion rather than construction

Verified against the code, not assumed:

- **`rupu-mcp` has no `reqwest` dependency at all.** It dispatches tool calls into `rupu-scm`'s `Registry`; those connectors make the requests and already tag `Origin::Scm`. Re-tagging them `Origin::Mcp` would be *less* accurate — the call really is to GitHub's or GitLab's API, which is what an operator auditing egress needs to see. MCP is dispatch, not transport.
- **`rupu-webhook` is an inbound axum server** serving `/webhook/{github,gitlab,linear,jira}`. It makes no outbound calls; its `reqwest` dependency is used only by its test files.

## Global Constraints

- **Workspace deps only.** Versions pinned in the root `Cargo.toml`.
- `#![deny(clippy::all)]` and `unsafe_code = "forbid"` workspace-wide.
- **Backward compatibility on read.** Existing ledgers and transcripts on disk may contain `{"kind":"mcp"}` or `{"kind":"webhook"}` only if something once wrote them. Nothing ever did — but a reader must still not panic on unknown input. Verify the deserialize path degrades rather than crashing, and say what it does.
- **Format ONLY files you touch, with `rustfmt --edition 2021 <path>`.** Never `cargo fmt`; never `rustfmt` a crate root or `mod.rs`.
- **Never use `git stash`** — the stash stack is shared across worktrees.
- **Run tests in the FOREGROUND.** No `pgrep -f` wait loops.

---

### Task 1: Remove the variants and mirror the change in the CP types

**Files:**
- Modify: `crates/rupu-netflow/src/ctx.rs`
- Modify: `crates/rupu-cp/web/src/lib/netflow.ts`
- Test: `crates/rupu-netflow/src/ctx.rs` test module

**Interfaces:**
- Consumes: nothing.
- Produces: `Origin` without `Mcp` or `Webhook`. The TypeScript `Origin['kind']` union mirrors it.

- [ ] **Step 1: Confirm the premise before deleting anything**

Run these and paste the output into your report. If either returns a production hit, **stop and report** — the premise is wrong and this plan should not proceed.

```bash
grep -rn "Origin::Mcp\|Origin::Webhook" crates --include "*.rs" | grep -v "netflow/src/ctx.rs"
grep -rn "reqwest" crates/rupu-mcp/Cargo.toml crates/rupu-mcp/src
```

Expected: no production constructor for either variant; no `reqwest` in `rupu-mcp` at all.

- [ ] **Step 2: Write the failing test**

Add to `crates/rupu-netflow/src/ctx.rs`'s test module:

```rust
    #[test]
    fn origin_enumerates_only_egress_that_can_occur() {
        // The enum is a claim about what this subsystem can capture.
        // `Mcp` and `Webhook` were removed because neither crate makes
        // outbound HTTP: rupu-mcp dispatches into rupu-scm's connectors
        // (already tagged Scm), and rupu-webhook is an inbound server.
        // A variant that can never be constructed is a false claim.
        for json in [
            r#"{"kind":"provider","name":"anthropic"}"#,
            r#"{"kind":"scm","name":"github"}"#,
            r#"{"kind":"update"}"#,
            r#"{"kind":"cp"}"#,
            r#"{"kind":"system"}"#,
        ] {
            serde_json::from_str::<Origin>(json).expect("known variant must parse");
        }

        // Retired variants must no longer deserialize.
        assert!(serde_json::from_str::<Origin>(r#"{"kind":"mcp","name":"x"}"#).is_err());
        assert!(serde_json::from_str::<Origin>(r#"{"kind":"webhook"}"#).is_err());
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p rupu-netflow origin_enumerates`
Expected: FAIL — the two retired variants still deserialize successfully.

- [ ] **Step 4: Delete the variants**

In `crates/rupu-netflow/src/ctx.rs`, remove the `Mcp(String)` and `Webhook` arms from `Origin`, and update the enum's doc comment to say what it now claims:

```rust
/// Which subsystem opened the connection.
///
/// This enum is a claim about what the subsystem can capture, so it lists
/// only egress that can actually occur. `Mcp` and `Webhook` were removed
/// deliberately: `rupu-mcp` makes no outbound HTTP (it dispatches into
/// `rupu-scm`'s connectors, which tag their own calls `Scm`), and
/// `rupu-webhook` is an inbound server. A variant nothing can construct
/// is a promise of coverage that does not exist.
```

- [ ] **Step 5: Mirror it in the CP types**

In `crates/rupu-cp/web/src/lib/netflow.ts`, remove `'mcp'` and `'webhook'` from `Origin['kind']`, and delete any comment that told readers not to build filter chips from the enum — that caveat existed only because the enum was untruthful.

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p rupu-netflow`, then `cargo build --workspace`, then `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: PASS. If any production code fails to compile, Step 1's premise was wrong — stop and report.

- [ ] **Step 7: Confirm the read path degrades rather than panicking**

An unknown `kind` on disk now fails to deserialize. Confirm what a reader does with that: `read_flows` skips malformed lines by design (`crates/rupu-netflow/src/ledger/views.rs`), so a legacy line would be skipped, not fatal. Verify that is actually true by reading the code, and record it in your report.

- [ ] **Step 8: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/ctx.rs
git status --porcelain
git add crates/rupu-netflow/src/ctx.rs crates/rupu-cp/web/src/lib/netflow.ts
git commit -m "refactor(netflow): drop Origin::Mcp and Origin::Webhook — neither can occur"
```

---

## Done when

- `cargo test --workspace` passes and `npx tsc --noEmit` is clean.
- `grep -rn "Origin::Mcp\|Origin::Webhook" crates --include "*.rs"` returns nothing.
- The `Origin` enum's doc states why the two are absent, so nobody re-adds them speculatively.
