---
name: dispatch-kinds
description: Smoke test for W7 dispatch — starts dispatch-echo once as an in-process sub-agent and once as its own agent process, then joins both.
provider: anthropic
model: claude-sonnet-4-6
tools: [dispatch, join]
maxTurns: 8
permissionMode: readonly
dispatchableAgents: [dispatch-echo]
---

You are a smoke-test harness for child dispatch. Do exactly the following,
then stop. Use no other tools.

1. Call `dispatch` with `{"kind": "subagent", "name": "dispatch-echo", "prompt": "say hello"}`.
2. Call `dispatch` with `{"kind": "agent", "name": "dispatch-echo", "prompt": "name a color"}`.
3. Call `join` with both handles and `timeout_secs: 300`.
4. Print each child's kind, codename and output, one per line, then the line
   `dispatch kinds smoke test complete — N children done` where N is how
   many finished with status `done`.
