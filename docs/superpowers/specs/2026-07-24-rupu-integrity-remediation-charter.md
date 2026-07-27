# rupu — Integrity Remediation Charter

**Date:** 2026-07-24
**Status:** approved (matt, 2026-07-24)
**Type:** program charter — defines the tracking system and the arc decomposition. Each arc gets its own spec + plan.

## 1. Why this exists

A four-sweep audit of the repo (specs, plans, code, user-facing docs) examined ~440 deferral/admission candidates and, after verifying each against the code, found **~125 genuinely-open items** — the rest were stale claims about work that had since shipped.

Two structural problems produced them:

1. **Orphaned deferrals.** 31 items exist *only* inside a plan file's "Deferred" section — referenced by no backlog, no tracker, nothing. A plan defers something, the plan is archived by the next plan, and the item is lost. Nothing in the process forces a deferral into a durable list.
2. **Silent no-ops at scale.** The `default_provider` defect (ISSUES.md I-1 — parsed, documented, exposed in the web UI, zero runtime consumers) is not an isolated bug; it is a *pattern*. The sweep found ~12 more instances of the same shape, several with editable fields **and policy-lock toggles** in the CP Settings UI.

This charter fixes the process (§2) and sequences the backlog (§3).

## 2. Tracking system

### 2.1 Two files, one rule each

| File | Holds | Test |
|---|---|---|
| `ISSUES.md` | Things that are **wrong** in shipped code — defects, drift, dishonest docs | "A user could hit this and be surprised" |
| `TODO.md` | Things deliberately **not built yet** — features, roadmap | "We knew, we chose not to, nothing is broken" |

An item belongs to exactly one. When a deferred feature turns out to be half-wired (shipped but inert), it moves from TODO.md to ISSUES.md — inert is *wrong*, not *deferred*.

### 2.2 `ISSUES.md` becomes two-tier

The existing format (symptom / root cause / impact / fix) is right but does not scale to ~70 open items at ~40 lines each.

- **Tier 1 — triage table.** Every open defect, one row: `ID | Sev | Area | Title | Arc | Status`. Scannable, diffable, complete.
- **Tier 2 — full write-ups.** The existing four-part treatment, but only for items in the **active arc** (and everything already written: I-1…I-5). An item gets promoted to Tier 2 when its arc starts.

IDs continue the existing sequence (`I-6` onward) and are never reused. `## Open` / `## Fixed` sections persist; an item moves to Fixed with its PR number.

### 2.3 Severity

| | Meaning |
|---|---|
| **P0** | Breaks, misleads, or destroys data for a user *today* — including "user follows our own docs and gets a wrong result" |
| **P1** | Wrong or missing, but workaroundable once you know |
| **P2** | Cosmetic, internal, or latent |

### 2.4 The anti-orphan rule

**Every "Deferred" / "Out of scope" item in a plan or spec must be mirrored into `TODO.md` (feature) or `ISSUES.md` (defect) in the same PR that writes the plan.** A deferral that lives only in a plan file is considered lost.

Enforcement is by review discipline in the plan template's final task, not by tooling. (A CI grep over plan "Deferred" sections was considered and rejected as brittle — item titles are prose and would need exact-match keys, which is more ceremony than the problem warrants at this repo's size.)

## 3. Arc decomposition

Seven arcs, sequenced. Each gets its own spec + implementation plan and ships as its own PR arc. The sequencing rule is: **data-loss and governance first, then security, then remove dual-path tax, then correctness, then truth in docs.** Docs come last deliberately — documenting behavior that is about to change means writing it twice.

| Arc | Theme | Items | Rationale |
|---|---|---|---|
| **1** | **Config integrity** — every declared config key either reaches the runtime or is deleted; `rupu config set` data-loss fixed; `[policy].lock` enforced on the CLI | 16 | Contains the only data-destroying bug found and the only governance control that silently isn't one. One coherent test pattern: *assert the configured value reaches the runtime*. |
| **2** | **Safety** — workspace containment on read tools, autoflow author allowlist, unattended-cleanup permission mode, action-step tool breadth | 6 | Security-shaped, small, independent of everything else. |
| **3** | **Single UI path** — delete the `classic` CP web renderers; `next` becomes the only path; drop both config keys, both hooks, both localStorage overrides | 5 tracked (52 branch sites, 33 files) | Purely subtractive. Removes the "keep classic byte-stable" tax that every CP web PR currently pays, making arcs 4 and 6 cheaper. Gated on operator visual validation — **cleared 2026-07-24**. |
| **4** | **Gate/action correctness** — templated numeric args, indexable action output, reject-path routing, timeout provenance | 12 | Makes a shipped feature usable for its headline purpose. Cheaper after Arc 3 collapses the dual render paths. |
| **5** | **Provider wire correctness** — Gemini thinking-config 400, effort-level portability, `Retry-After` | 5 | Hard breakage, isolated to `rupu-providers`. |
| **6** | **Docs truth pass** — config reference, workflow-schema completion, the `actions:` contradiction, MSRV, broken examples | ~25 | Last, so it documents settled behavior including the single UI. |
| **7+** | **Feature backlog** — multi-host transport gaps, CP web deferrals, roadmap items | ~60 | Genuinely deferred. Lives in `TODO.md`; not part of this program. |

### 3.1 Per-arc contract

Each arc:
- gets a spec (`docs/superpowers/specs/`) and a plan (`docs/superpowers/plans/`);
- lists its `I-NNN` items explicitly, and those items are promoted to ISSUES.md Tier 2 when it starts;
- closes each item in the same PR that fixes it — moving the row to `## Fixed` with the PR number, and **stating the validation that proves it** (the test name, or the command and its output);
- may not silently drop an item: an item found to be wrong, obsolete, or not-a-defect is closed with that reasoning recorded, not deleted.

### 3.2 Validation bar

An item is closable only with evidence. For a dead-config item that means a test asserting the configured value is *observable at the consumer* — not merely that it parses. The `default_provider` fix (I-1) is the reference: the defect was invisible to a parse test and to a config-layering test, because both passed the whole time it was broken.

## 4. Non-goals

- Fixing everything in one arc. The backlog is a program; arcs ship independently.
- Rewriting `TODO.md` into an issue tracker. It stays a curated feature backlog; it only gets a truth pass to remove claims the sweep proved shipped.
- Migrating to GitHub Issues. The two markdown files are the tracker.
