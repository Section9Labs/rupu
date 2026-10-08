# F2 (feature): ephemeral agentiflows as a dispatch kind

- **Card:** F2 · **Depends on:** W7 (`dispatch`/`Launcher`/children ledger) and F1a (message spaces, for the parent ↔ child-lead bridge)
- **Origin:** matt, 2026-10-07: "give an agent the capability to create another ephemeral agent flow for some reason … we should not limit our own agents with the code we should provide the capabilities and be flexible."

## 1. Goal

An agent granted `dispatch` and `agentiflows.draft` can **compose an agentiflow on the fly**, run it as a child, talk to its lead while it runs, and `join` its result. The agentiflow can also be an existing definition by name. This is the fourth `ChildKind`, added exactly the way 00-index §7 says a launch kind is added.

## 2. Design

### 2.1 Tools

```jsonc
// agentiflows.draft — Effect::Record, needs Catalog
{ "definition": { "name": "subnet-sweep", "lead": "recon-lead", "engagement_profiles": ["network"],
                  "goals": [ … ], "scope": { … }, "pool": { "agents": [ … ], "workflows": [ … ] },
                  "budget": { … }, "round": { … } } }
// → { "file": "<caller run dir>/generated/agentiflows/subnet-sweep-01J….yaml", "name": "subnet-sweep" }

// dispatch (W7) gains a kind
{ "kind": "agentiflow", "name": "network-assessment" }      // an existing definition
{ "kind": "agentiflow", "file": "<path from agentiflows.draft>" }
// → { "handle": "ch_…", "kind": "agentiflow", "run_id": "af_…", "codename": "…" }
// join → { "status": "done", "success": true,
//          "output": { "stop_reason": "goals_met", "goals": [{ "id", "met" }], "findings": 14,
//                      "budget_used": { … }, "summary": "<lead's final text>" } }
```

**`agentiflows.draft` validation.** It runs before anything is written, so a bad definition never reaches the launcher:
- the definition parses as `AgentiflowDef` (`deny_unknown_fields`);
- the lead and every pool agent/workflow exist (`CatalogPort`);
- the lead and pool sit within the caller's inherited allowlist cap (W7 §3.4), so a drafted flow cannot widen what the caller may launch;
- the engagement profiles exist;
- the budget is no larger than the caller's remaining budget when the caller is itself inside a budgeted flow (`budget.status`).

### 2.2 Launcher

- **`ChildKind::Agentiflow`** in `rupu-tools/src/ports.rs`, implemented in `rupu-launch`.
- **`RunArgv::Agentiflow(AgentiflowRun { def: Name | File, run_id, launch: LaunchLink, parent_bridge })`** in `rupu-runtime/src/argv.rs`. This needs `rupu agentiflow run --file <path>`; today the command takes a definition name only (`rupu-cli/src/cmd/agentiflow.rs:68`).
- **Runs** as its own process through `spawn_detached`, like `workflow` children.
- **Recorded** in the children ledger. `join` reads the flow's `agentiflow.json` terminal status plus its report.
- **Policy:**
  - a new frontmatter key `dispatchableAgentiflows`, intersected with the inherited cap;
  - depth + 1;
  - the permission ceiling carries into the child flow: its lead and units are capped;
  - usage rolls into the root ledger (`--usage-root`), so a parent flow's budget counts the child flow's spend.
- **Wind-down.** `Launcher::finish` on an unjoined child flow sends the flow a graceful `stop`, then terminates it after the grace period if it is still running. The reaper covers a dead parent.

### 2.3 Parent ↔ child-lead bridge (FD6 extension)

A child agentiflow is its own message root (F1). F2 adds one bridge, and only one:

- **Parent → child flow.** The parent sends `msg.send { to: "child:<handle>" }`. The message lands in the child flow's space as `kind: direct`, `to: lead`, `from: "parent:<parent address>"`, and is logged in **both** roots' logs with a shared `id`.
- **Child flow → parent.** The child's lead sends `msg.send { to: "parent" }`. The message lands in the parent's root, addressed to the dispatching participant.
- **Framing.** Bridged messages use the untrusted agent framing on both sides; they are never operator framing.
- **Not supported:** other cross-root addressing (unit-to-unit across roots) stays unsupported. It is refused with `undelivered: cross_root`.

### 2.4 UI

- **Children section (from W7).** An agentiflow child shows its goals met/unmet and links to its `AgentiflowDetail`.
- **Bridged messages** appear in both roots' Messages tabs with a "↔ child flow" or "↔ parent" badge (F1c row variant).

## 3. Files

`rupu-tools/src/{agentiflows/draft.rs, launch/dispatch.rs}`, `rupu-tools/src/ports.rs`, `rupu-launch/src/kinds/agentiflow.rs`, `rupu-runtime/src/argv.rs`, `rupu-cli/src/cmd/agentiflow.rs` (`--file`, LaunchLink flags), `rupu-agentiflow` (accept LaunchLink: depth/ceiling/usage root/parent bridge), `rupu-fleet/src/bus.rs` (bridge routing), `rupu-agent/src/spec.rs` (`dispatchableAgentiflows`), web children section + bridged-message badge, `docs/messaging.md`, and the agentiflow docs.

## 4. Tests

1. **`draft_validates`**: unknown key, unknown agent, pool outside the cap, and an over-budget definition are each rejected with a specific error. A valid draft writes the file.
2. **`dispatch_agentiflow_join`** (`rupu-launch` it, mock provider): a drafted flow runs and `join` returns its report; usage lands in the root ledger.
3. **`child_flow_capped`**: a readonly parent's child flow's units can't `write_file`; the pool cap holds.
4. **`bridge_roundtrip`**: parent → `child:<h>` reaches the child lead's next model call; child lead → `parent` reaches the parent; both logs have the shared id.
5. **`unjoined_child_flow_stopped`**: the parent ends, and the child flow receives a graceful stop and is recorded `abandoned`.

## 5. Acceptance

- An agent can draft, launch, steer (via the bridge) and join an agentiflow without any file outside its run directory being written, and without widening its own allowlist or permission.
- matt runs one real nested flow on the Mac and checks the CP shows parent ↔ child correctly (GUI check).
