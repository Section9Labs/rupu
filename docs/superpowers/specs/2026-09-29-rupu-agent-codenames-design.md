# rupu agent codenames — design

**Date:** 2026-09-29
**Status:** approved in brainstorm, pending spec review
**Scope:** `rupu-cli` + `rupu-cp` (web). The macOS app is deprecated and out of scope; all serde changes are additive so it keeps decoding.

## 1. Problem

Every run, session, fan-out unit and sub-agent is identified by `run_<ULID>` / `sub_<ULID>` / `ses_<ULID>`. Correct for the machine, useless for a human scanning Live Events, a findings table or a transcript: you cannot tell at a glance which run a finding came from, or which of 1,200 fan-out units produced it.

## 2. Goal

Every execution gets a short, stable, human codename that answers *which crew?* and *which member?* in one token, rendered with a stable visual identity (tint + badge), and shown everywhere the ULID is shown today.

Non-goals: LLM self-naming (costs a turn, non-deterministic, collides); renaming history when word lists change; replacing ULIDs as the primary key.

## 3. Naming grammar

```
<crew> / <role>[#n][.attempt] ( ">" <role>[#n][.attempt] )*
```

| Part | Vocabulary | Assigned from |
|---|---|---|
| **crew** | `color-noun` (e.g. `cobalt-harbor`) — ~48 curated colors × ~200 curated place/object nouns (no animals) ≈ 10k | hash of the top-level id (workflow run / standalone agent run / session) |
| **role** | animal (~250 curated, short, distinct) | hash of the **agent definition name**, collision-resolved per crew (§4.3) — stable across runs |
| **#n** | 1-based integer | the *n*-th instance of that role under the same parent |
| **.attempt** | integer ≥ 2 | retry of the same unit; absent on first attempt |
| **`>`** | — | sub-agent dispatched by the left-hand agent |

Examples:

| What | Codename |
|---|---|
| Workflow run | `cobalt-harbor` |
| Singleton agent step | `cobalt-harbor/heron` |
| Fan-out unit (0-based index 411) | `cobalt-harbor/heron#412` |
| Sub-agent of that unit | `cobalt-harbor/heron#412>lynx#1` |
| Third parallel sub-agent of same role | `cobalt-harbor/heron#412>lynx#3` |
| Deeper nesting | `…>lynx#3>otter#1` |
| Retry of a unit | `cobalt-harbor/heron#412.2` |
| Standalone `rupu run <agent>` | `amber-lantern/heron` |
| Session agent (every turn) | `saffron-ridge/heron` |
| Sub-agent in a session | `saffron-ridge/heron>lynx#5` |

**`#n` rule.** Omitted only when the orchestrator knows statically the agent is a singleton (a plain linear workflow step, a standalone run, a session's own agent). Fan-out units (`for_each`, `parallel`, `panel` members, fixers) and every dynamic dispatch (`dispatch_agent`, `dispatch_agents_parallel`) are always numbered from `#1`. Fan-out `n` = unit index + 1. Dynamic-dispatch `n` = a per-(parent, role) counter; in sessions that counter is **session-wide and persisted** so it keeps climbing across turns and worker restarts.

**Sessions** keep one identity across turns. Turns are labelled "turn N", never renamed.

**Display.** Long paths render as the leaf (`heron#412>lynx#3`) under a crew chip; full path in tooltip/breadcrumb. Full string is always the searchable/copyable form.

## 4. `rupu-codename` crate

New leaf crate, no rupu deps (workspace deps only: `serde`).

### 4.1 Contents
- Word lists (`colors`, `nouns`, `roles`) as `&'static [&'static str]`, curated: lowercase ASCII, ≤ 8 chars, no homophones/near-duplicates, nothing offensive, no status-color collisions (see palette).
- `Palette`: per color word, a `{ light: "#rrggbb", dark: "#rrggbb" }` pair, AA-contrast checked against both CP themes' surface tokens in a unit test. Status-adjacent hues (pure red / green) are desaturated so a crew tint never reads as failed/passed.
- `RoleBadge`: `{ shape: Shape, hue: HueIdx }` — 12 shapes (circle, triangle, square, diamond, pentagon, hexagon, star, cross, ring, chevron, drop, bolt) × 12 hues ≈ 144, derived from the role word.
- `Codename` struct (`crew`, `segments: Vec<Segment { role, n: Option<u32>, attempt: Option<u32> }>`), `Display`, `FromStr`, serde as its string form.
- `fn crew_for(id: &str) -> Crew` — FNV-1a 64 over the **whole** id string (the ULID's first 48 bits are a timestamp; hashing only a prefix would make same-millisecond runs collide). Never `DefaultHasher` (not stable across Rust versions).
- `RoleAllocator` — per-crew: `fn role_for(&mut self, agent_def: &str) -> &str`. Hash agent name → index; if the word is already taken by a *different* agent def in this crew, linear-probe to the next free word. Same def ⇒ same word within the crew. Serializable so it can be persisted (§5).
- `InstanceCounter` — per (parent codename, role) monotone counter, serializable.

### 4.2 Stability contract
- Names are **minted once and stored**; they are never recomputed for records that have one. Word-list edits therefore never rename history.
- Word lists are append-only in practice; the derive-on-read fallback (§6) is the only thing that recomputes, and it is explicitly marked `derived`.

### 4.3 Role collisions
Across *different* crews the same agent def always hashes to the same base word (cross-run memory: "heron = security-reviewer"). Only within a crew can a probe move it; the allocator state is persisted in the run so resume keeps the same assignment.

## 5. Minting + storage

Every id-mint site also mints the codename, and the codename travels with the id.

| Site | Mints |
|---|---|
| `runner.rs` / `in_process.rs` / `cp_launcher.rs` / `cmd/workflow.rs` workflow run start | crew; `RoleAllocator` persisted on `RunRecord` |
| per-step agent run (`runner.rs` step dispatch) | `crew/role` |
| fan-out unit (`unit_index`, incl. placed units — minted by the coordinator into `UnitDispatch` next to `run_id`, same pattern as PR #646) | `crew/role#n` |
| unit retry (`retry_run_id`) | `…#n.attempt` |
| `dispatch_agent` / `dispatch_agents_parallel` (`rupu-tools`) | parent codename + `>role#n`; parent codename + counters reach the tool via `ToolContext` |
| `cp_agent_launcher.rs`, `rupu run <agent>` | crew + `crew/role` |
| `cmd/session.rs` session create | crew + `crew/role`; `RoleAllocator` + `InstanceCounter`s persisted in the session record |

New fields (all `#[serde(default, skip_serializing_if = "Option::is_none")]`, so every legacy file round-trips byte-for-byte):
- `RunRecord.codename: Option<String>` (crew) + `RunRecord.codename_state: Option<AllocatorState>`
- session record: same pair
- `rupu_transcript::Event::RunStart.codename: Option<String>` (full instance codename)
- `StepResultRecord.codename`, `ItemResultRecord.codename`, `UnitCheckpoint.codename`
- `DispatchOutcome.codename`
- orchestrator `FindingRecord.codename` (panel findings: the member that raised it)
- `rupu_coverage` ledger `Attribution.codename` (findings via `findings.record` / `report_finding`: the declaring instance)
- executor `Event` variants that carry an actor gain `codename`

**Agent self-awareness.** The system prompt gets one line: `Your call sign in this run is <leaf> (crew <crew>). Sign any comments, issues or PR notes you post with it.` — the "self-naming" layer, zero extra turns. `RunStart.system_prompt` already records it.

## 6. Legacy records — derive on read

`rupu-cp` DTOs fill `codename` when the stored field is absent:
- crew ← `crew_for(run/session id)`
- role ← base hash of agent name (no collision probing — could duplicate within an old crew; accepted)
- `#n` ← unit index + 1 where known; dynamic sub-dispatch counters unknown ⇒ `>role` without `#n`

DTOs carry `codename_derived: bool` (true only for these); the web renders derived names slightly muted. The CLI uses the same helper (lives in `rupu-codename` as `derive_legacy(...)`) so both surfaces agree.

## 7. CLI surfaces

- Run/session start line: `▶ cobalt-harbor  (run_01J…)`.
- Live workflow output (`output/live_run.rs`) and `rupu transcript` pretty-print: per-agent line prefix `◆ heron#412>lynx#3` (badge glyph + tint via existing color handling; plain text under `NO_COLOR`).
- `rupu workflow runs`, `rupu session list`, `rupu transcript list`: NAME column.
- **Codename as id.** Every command that accepts a run/session/transcript id also accepts a codename (full or crew-only). Crews repeat, so a crew-only lookup resolves to the **most recent** match and, if other matches exist within 30 days, prints a note listing them with ids. A full instance codename resolves within that crew.

## 8. CP surfaces (web)

- **DTOs:** `codename`, `codename_derived` on runs, sessions, step/unit rows, transcript events, findings (`FindingOut`), Live Events/firehose payloads.
- **Palette:** `rupu-codename` exports palette + badge tables; the web consumes a generated TS module (generated in `build.rs` or a `make` target, checked in); a `cargo test` asserts the checked-in TS matches — drift fails CI.
- **Components:** `CrewChip` (tint dot + name), `RoleBadge` (SVG shape in role hue) + `AgentName` (badge + leaf, full path tooltip).
- **Where:** Activity/runs table (crew chip primary, ULID secondary); run detail header; run graph nodes (badge + role; fan-out node `heron ×1,240`; unit selection shows `#412`); transcript turns/tool cards attribute their actor; Live Events / Situation Room cards get the crew tint stripe + actor, roster groups by crew; findings (per-run/project/global) actor column + crew chip; session list + header; gate notifications ("cobalt-harbor is waiting for approval").
- **Search:** Live Events search (#670) and Activity filter match codename substrings.

## 9. Scale

At 1,000+ units the instance part is a plain integer by design; views group by role (`heron ×1,240 · 3 failed · 17 findings`) and show instance names only on drill-in. Fan-out rows show the unit's item as secondary context (`heron#412 · src/auth/session.rs`) — the item is context, the codename is identity.

## 10. Testing

- `rupu-codename`: determinism (golden table of id → crew), whole-id hashing (two ULIDs in the same ms differ), allocator probing + persistence round-trip, parse/display round-trip incl. nesting/attempts, word-list hygiene (unique, lowercase, length), palette contrast.
- Orchestrator: a fan-out run yields `#1..#N` unique codenames; resume keeps role assignment; retry yields `.2`; placed units carry coordinator-minted codenames.
- Tools: nested `dispatch_agent` produces `>role#n` with per-parent counters; session counter survives reload.
- Serde: every touched record round-trips a legacy fixture byte-for-byte.
- CP: derive-on-read on a legacy run marks `codename_derived`; TS palette drift test.
- CLI: codename resolution (most-recent, ambiguity note), NAME columns snapshot.

## 11. Plans

1. **Core + CLI** — `rupu-codename`, minting at every site, stored fields, system-prompt line, CLI surfaces, codename-as-id resolution.
2. **CP** — DTO fields, derive-on-read, palette TS generation, web components + surfaces, search.
