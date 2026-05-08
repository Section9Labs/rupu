# Writing Good Agents

> See also: [agent-format.md](agent-format.md) · [workflow-authoring.md](workflow-authoring.md) · [development-flows.md](development-flows.md)

---

## What a good rupu agent looks like

A good agent is narrow, explicit, and testable.

It should answer six questions clearly:

1. What job does this agent own?
2. What tools may it use?
3. What sequence should it follow?
4. What counts as done?
5. What must it not do?
6. What shape should the final answer have?

If those answers are vague, the agent will behave vaguely.

---

## Design rules

### 1. Give the agent one job

Prefer:

- `repo-investigator`
- `repo-implementer`
- `security-reviewer`
- `pr-author`

Avoid one giant agent that plans, edits, reviews, deploys, and comments on issues all in a single prompt.

### 2. Declare the smallest useful tool set

Examples:

- reviewers: `read_file`, `grep`, `glob`, selected SCM read tools
- implementers: built-in file tools plus `bash`
- issue / PR actors: explicit `issues.*` or `scm.*` tools only

Do not hand write-capable tools to agents that are supposed to be advisory.

### 3. Describe the work sequence

Tell the model how to proceed. For example:

1. reproduce or inspect
2. read relevant code
3. propose the smallest viable change
4. validate with a focused command
5. stop

Good prompts reduce wandering.

### 4. Set a stop condition

Examples:

- "Stop after writing the tests. Do not modify production code unless required."
- "Stop after posting one review comment."
- "Do not refactor surrounding code."

Without a stop condition, many agents keep expanding scope.

### 5. Make the output contract explicit

Examples:

- concise bullet list
- Markdown spec with named sections
- JSON object with required fields
- first line formatted as `PR: github:owner/repo#123`

If another workflow step depends on the output, make that contract painfully clear.

### 6. Separate implementation from review

A reliable pattern is:

- implementer agent with write tools
- reviewer agents with read-only tools
- optional fixer agent used only to address panel findings

That separation makes workflows more predictable and easier to audit.

---

## Recommended template

```markdown
---
name: my-agent
description: One-sentence purpose.
provider: anthropic
model: claude-sonnet-4-6
tools: [read_file, grep, glob]
maxTurns: 12
permissionMode: readonly
---

You are a <role>.

Your job:
- <single clear responsibility>

Process:
1. <first step>
2. <second step>
3. <third step>

Constraints:
- <things this agent must not do>
- <scope limits>

Output:
- <exact response shape>

Stop when:
- <clear stop condition>
```

Use this as a baseline and specialize from there.

---

## Common agent archetypes

### Investigator

Use when you need diagnosis, not edits.

Recommended traits:

- tools: read-only plus optional `bash`
- output: root cause, affected files, suggested validation
- stop condition: no edits

### Implementer

Use when the task is already understood and bounded.

Recommended traits:

- tools: built-in write tools plus `bash`
- output: changed files, commands run, residual risk
- stop condition: minimal scoped change only

### Test writer

Use when coverage is the goal.

Recommended traits:

- read code, inspect existing tests, add focused tests
- avoid product-code changes unless a real bug blocks testability

### Reviewer

Use when you want textual findings only.

Recommended traits:

- read-only tools
- no edits
- findings grouped by severity or theme

### Panel reviewer

Use in workflow `panel:` steps.

Panel reviewers should:

- stay read-only
- return parseable findings JSON
- keep output short and precise

Recommended final shape:

```json
{
  "findings": [
    {
      "severity": "low",
      "title": "Short title",
      "body": "One sentence detail"
    }
  ]
}
```

### SCM actor

Use when the job is to open a PR, post an issue comment, or trigger a pipeline.

Recommended traits:

- only the explicit SCM / issue tools required
- strict output contract, usually including the created resource ref
- no code edits unless that is also part of the job

---

## Anti-patterns

| Anti-pattern | Why it fails | Better pattern |
| --- | --- | --- |
| One agent does everything | Scope drifts and failures are hard to diagnose | Split planner, implementer, reviewer, and PR actor |
| Reviewer has write tools | The review may mutate the thing being reviewed | Keep reviewers read-only |
| Prompt says "be smart" but not what to do | The model improvises too much | Give an ordered process |
| No final output contract | Downstream workflow steps become brittle | Specify exact headings or JSON |
| No stop condition | Agent keeps expanding the task | Explicitly say when to stop |
| Hidden SCM assumptions | PR / issue actions fail at runtime | Name the required tools and repo context |

---

## Practical conventions for repo teams

- Keep shared, project-relevant agents under `<repo>/.rupu/agents/`.
- Keep highly personal utility agents under `~/.rupu/agents/`.
- Use lowercase, hyphen-separated names.
- Keep `description:` short and useful; it becomes your agent catalog.
- Prefer explicit `provider`, `model`, `tools`, and `permissionMode` in checked-in files.
- Version the prompt like code. Small prompt changes can materially change behavior.

---

## When to make a workflow instead of a better agent

Create a workflow when the process has:

- multiple specialists
- review or approval boundaries
- fan-out across files or targets
- retry / fix loops
- issue or event driven entry points

Do not force orchestration into one giant agent prompt when the runtime already has first-class workflow support.
