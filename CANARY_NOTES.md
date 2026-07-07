# Phase 1 — Environment Readiness Canary Notes
## Issue: github:section9labs/rupu/issues/188
## Branch: issue-188-phase-1
## Timestamp: 2026-07-07T19:40:03Z

---

## Exit-Criteria Checklist

| # | Check | Result |
|---|-------|--------|
| 1 | `rupu auth status` — **github ✓** | ✅ SSO active, no expiry |
| 2 | `rupu auth status` — **anthropic ✓** | ✅ API key present |
| 3 | `rupu repos list` — **Section9Labs/rupu** present | ✅ github / main / public |
| 4 | Issue #188 state — **OPEN + `autoflow` label** | ✅ confirmed via `gh issue view` |

All four exit criteria are **green**. Phase 1 is complete.

---

## Credential Details (`rupu auth status` snapshot)

```
┌───────────┬─────────┬─────────────────┐
│ PROVIDER  │ API-KEY │ SSO             │
├───────────┼─────────┼─────────────────┤
│ anthropic │ ✓       │ —               │
│ openai    │ ✓       │ ✓ expires in 1d │
│ gemini    │ —       │ —               │
│ copilot   │ —       │ ✓ no expiry     │
│ github    │ —       │ ✓ no expiry     │
│ gitlab    │ —       │ —               │
│ linear    │ ✓       │ —               │
│ jira      │ ✓       │ —               │
│ oracle    │ ✓       │ —               │
└───────────┴─────────┴─────────────────┘
```

**No blocking errors.** The `openai` SSO token expires in ~1 day — not required for this autoflow
(which uses `anthropic`), but worth refreshing before extended runs.

---

## Repo Registration (`rupu repos list` snapshot)

`Section9Labs/rupu` is registered:

```
│ github │ Section9Labs/rupu │ main │ public │
```

Local worktree checkout path (this workspace):

```
/Users/matt/.rupu/autoflows/worktrees/github--section9labs--rupu/issue-188
```

Note: `rupu repos list` does not display the local path column for GitHub-backed repos;
the local worktree path is the autoflow-managed checkout above.

---

## Issue State (`gh issue view 188 --repo section9labs/rupu`)

```json
{
  "state": "OPEN",
  "labels": [{"name": "autoflow", "description": "Managed by rupu autoflow demo", "color": "0e8a16"}],
  "title": "[autoflow demo] controller pickup smoke test"
}
```

---

## Environment Deviations / Notes

- No custom `RUPU_HOME` override in use; default path applies.
- No proxy configured.
- `gemini` and `gitlab` credentials absent — neither is required for `issue-supervisor-dispatch`
  (which targets `anthropic` + `github`).
- `openai` SSO expires in ~1 day — refresh before Phase 2 if multi-provider fallback is needed.
- No alternate LLM configured; default model will be resolved by the serve loop.

---

## Next Step

Phase 2 — **Claim Acquisition Verification** may now begin.
Run `rupu autoflow serve` and confirm that `issue-supervisor-dispatch` picks up issue #188.
