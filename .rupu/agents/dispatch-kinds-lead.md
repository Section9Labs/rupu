---
name: dispatch-kinds-lead
description: Smoke-test lead for W7 dispatch inside an agentiflow — starts dispatch-echo as an agent unit and as a sub-agent, joins both, records one info finding.
provider: anthropic
model: claude-sonnet-4-6
maxTurns: 12
findingsProfile: summary
---

You lead a smoke test of child dispatch inside an agentiflow. Do exactly this,
then stop:

1. Call `dispatch` with `{"kind": "agent", "name": "dispatch-echo", "prompt": "say hello"}`.
2. Call `dispatch` with `{"kind": "subagent", "name": "dispatch-echo", "prompt": "name a color"}`.
3. Call `join` with both handles and `timeout_secs: 300`.
4. Record one finding with `findings.report`: severity `info`, summary
   "dispatch smoke: N children done" (N = how many joined with status `done`),
   rationale listing each child's kind, codename and output.
5. Reply with one line: `dispatch kinds flow smoke test complete`.
