# F1d — Messaging on by default

- **Overview + decisions:** [F1-messaging-everywhere.md](F1-messaging-everywhere.md). This card reverses FD2.
- **Builds on:** F1a (space, log, `Bus`, `messaging:` key), F1b (every run joins its space), W2 (grants), W7 (launch caps).
- **Operator decision (2026-10-13):** agents can message by default everywhere. A workflow (and so an autoflow), an agentiflow, or an agent's frontmatter can opt out. Full tool set. Cross-root messaging stays out of scope (FD6, a later card).

## 1. Problem

Since F1b every run is in its space and the operator can reach it. The agents themselves can send only when a workflow sets `messaging:`, an agentiflow launched them, or their `tools:` names a messaging tool. A sub-agent cannot tell its parent anything unless its agent file was written for it. The opt-in is a step most authors never take, so the Messages tab of most runs only ever shows the operator.

## 2. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | Who gets the messaging tools by default? | Every run with a message space: standalone `rupu run`, session turns, workflow steps and units, sub-agents, process `agent` children, agentiflow leads and units. Granted as the ambient grant `ambient:messaging_default`, which is `WORKFLOW_MESSAGING` (`board.claim/release/post/read`, `msg.send`). An agentiflow lead and unit keep their origin grants (a superset), and the default adds nothing to them. |
| D2 | Workflow key | `messaging:` absent or `true` → the full set. `{ tools: [...] }` → only those (as today). `false` → **off** for every step and unit and everything they launch. Autoflows are workflows with an `autoflow:` block, so this key is their opt-out too. |
| D3 | Agentiflow key | New `messaging: false` on the agentiflow definition (`AgentiflowDef`). Absent or `true` = today's behaviour. `false` = the lead and every unit get no tool that needs `Service::MessageBus` (every `board.*`, including the lead's `board.directive` / `board.retract`, and `msg.send`). The operator still reaches them. |
| D4 | Agent frontmatter | New `messaging: false` in agent frontmatter. That agent gets no messaging tool wherever it runs. `messaging: false` together with a `tools:` entry naming a messaging tool (exact, alias, `board.*` / `msg.*`) is a load error: the file contradicts itself. Absent or `true` = the default. |
| D5 | Precedence | Off wins. The run's messaging is off when any of these says off: its agent (D4), its workflow (D2), its agentiflow (D3), or its parent (D6). Off means the run's bus is not offered to its tools: `Service::MessageBus` is not provided, so the grant drops every messaging tool. A tool the agent names exactly is reported unavailable through W2's `tool_unavailable` notice, never silently. An origin grant is not exempt. |
| D6 | Inheritance | Off is a launch cap, like the connector cap. A run whose messaging is off launches children with messaging off: a sub-agent through its parent's `RunIdentity.launch`, a process child through a new `LaunchLink` field, rendered `--launch-no-messaging`. A child never has more than its parent. A workflow child dispatched from an opted-out run inherits off too, although it is its own root. |
| D7 | Unchanged | The space, the join at start, the operator's path, the Messages tab, the `*` rule (`*` still never names a messaging tool, D22, which no longer matters for a default-on run), cross-root messaging (FD6) and placed units (FD7). A placed unit runs as a standalone `rupu run` on its host, so it has its own space there and gets the default like any standalone run. The workflow's switch reaches it: the coordinator sets `UnitDispatch.messaging` (a `MessagingSwitch`) to off when the run was launched under the cap (`StepFactory::messaging_capped`, its link's `no_messaging`), else to the workflow's `messaging:` as is; the fleet dispatcher passes it on as `AgentLaunchRequest.messaging` (`LaunchMessaging`, on the wire `"off"` or `{"tools": [...]}`, absent for the default), and the host runs `rupu run --no-messaging` (`OffReason::Launch`, a cap its children inherit) or `rupu run --messaging-tool <name>` per tool (`ambient:workflow_messaging`, no cap), checked against the catalog before the run starts. Every connector delivers it or refuses the unit: capability `agent.no_messaging` for off and `agent.messaging_tools` for tools (HTTP `features`, tunnel `Hello`, bucket worker marker), SSH gated in the launch command. |

## 3. Data shape

One resolution, decided once by `RunAssembler` for each run:

```rust
/// What a run may write into its message space (F1d).
enum MessagingGrant {
    /// No messaging tools; the bus stays receive-only. Records why (agent / workflow / agentiflow / parent).
    Off(OffReason),
    /// These tools, canonical.
    Tools(Vec<&'static str>, GrantReason),
}
```

Inputs: the origin (its default or origin grant), `Origin::WorkflowStep.messaging` (a tri-state now: default, tools, off), the agentiflow's switch (it travels on the lead's and units' launch), `AgentSpec.messaging`, and the parent's cap (`LaunchScope` for in-process, `LaunchLink` for a process child). `takes_part` and the `ambient:workflow_messaging` / `ambient:messaging_default` grant both come from this value, so the "is the bus offered" decision and the grant can't disagree.

`LaunchScope` gains `messaging_off: bool` (the cap passed down). `LaunchLink` gains `no_messaging: bool`.

## 4. Where it lands

| Crate | Change |
|---|---|
| `rupu-tools/src/grant.rs` | `AmbientGrant::messaging_default()` (`ambient:messaging_default`) |
| `rupu-orchestrator/src/workflow.rs` | `messaging_tools()` returns the full set when the key is absent. `false` is a real "off" that reaches the step origin |
| `rupu-agent/src/spec.rs` | `messaging:` frontmatter key and the D4 contradiction check in `validate_tools` |
| `rupu-agentiflow/src/def.rs` + lead/unit launch | `messaging:` key, carried to the lead's and units' launches |
| `rupu-runtime/src/assembly/` | `MessagingGrant` resolution, `takes_part` from it, the default grant, the cap in `launch_scope` |
| `rupu-runtime/src/argv.rs` + `rupu-cli/src/cmd/launch_link.rs` | `--launch-no-messaging` |
| `rupu-launch` | pass the cap to every child kind, including continue and relaunch paths |
| `rupu-orchestrator/src/runner.rs` + `rupu-cli/src/fleet_unit_dispatcher.rs` + `rupu-cp` connectors | `UnitDispatch.messaging` → `AgentLaunchRequest.messaging` → `rupu run --no-messaging` / `--messaging-tool` on the host, capabilities `agent.no_messaging` / `agent.messaging_tools` (D7) |
| docs | `docs/messaging.md` (the "takes part" paragraph, "Turning it on in a workflow" becomes "Turning it off"), `docs/transcript-schema.md` if the grant reason is listed, F1 index FD2 row, CLAUDE.md |

## 5. Tests

- `origin_matrix` and `star_parity_per_origin` snapshots: every origin shows the default grant, and off shows none.
- A standalone run with no `tools:` mention of messaging is offered `msg.send` (`tool_grant` names `ambient:messaging_default`).
- A sub-agent sends `msg.send to: parent` with no messaging in its agent file. The parent's next model call sees it.
- Workflow `messaging: false`: steps get no messaging tools, an agent naming `msg.send` gets a `tool_unavailable` notice, and a sub-agent a step launches gets none either.
- Agent `messaging: false`: no tools in any origin. With a messaging tool in `tools:` the agent fails to load.
- Agentiflow `messaging: false`: neither the lead nor a unit is offered any `board.*` / `msg.send`. `flow_toolset_parity` keeps today's lists when the key is absent.
- `argv_roundtrip` covers `--launch-no-messaging`. A process child of an opted-out run has messaging off.
- A placed unit of a `messaging: false` workflow, or of a run launched under the cap, is dispatched with messaging off, and one of a `messaging: { tools: [...] }` workflow with exactly that list (off when capped). `rupu run --no-messaging` has messaging off (`OffReason::Launch`) and caps its children; `rupu run --messaging-tool msg.send` is offered only that tool as `ambient:workflow_messaging` and caps nothing; an invalid name is a usage error. `argv_roundtrip` covers the default, off and tools. Every connector carries the switch, and HTTP, tunnel, bucket and SSH refuse a peer without the capability it needs (a tools launch on a peer advertising only `agent.no_messaging` included).
- The workflow editor and other web code that treat `messaging:` as opt-in follow the new meaning.

## 6. Non-goals

Cross-root messaging (a workflow child reaching its parent run, FD6). Agents replying to the operator (`to: operator` stays refused). A space transport for placed units (FD7). A Messages-tab hint for opted-out runs.
