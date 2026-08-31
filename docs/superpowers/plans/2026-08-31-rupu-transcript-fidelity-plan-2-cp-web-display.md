# Transcript Fidelity Plan 2 — CP Web Display Parity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The CP web transcript renders every recorded event in true order — first-class thinking blocks (v2) and legacy thinking, gate rows, per-turn tokens, prompt/seed/notice/compaction blocks, standalone orphans — and surfaces unparseable lines instead of silently dropping them.

**Architecture:** `/api/transcript` gains an `unparsed` count; `transcriptView.ts`'s `TurnView` moves from `{assistant, tools[]}` to an ordered `blocks[]` model grouped by `turn_start` (the runner has always written it); `Turn.tsx` renders blocks in sequence. Pairing logic (call_id, audit FIFO, diff/terminal adjacency) is preserved verbatim.

**Tech Stack:** Rust (axum handler), TypeScript + React + vitest (`crates/rupu-cp/web`, `npm test` = vitest).

**Spec:** `docs/superpowers/specs/2026-08-31-rupu-transcript-fidelity-design.md`

## Global Constraints

- Branch `feat/transcript-fidelity-plan-2` off main AFTER Plan 1 merges; PR-only, never direct to main.
- No `any` in web code; static Tailwind class strings only (existing repo rules in the component headers).
- Legacy transcripts (v1: `assistant_message.thinking`, no `turn_start`-reliant assumptions violated) must render at least as well as today — pin with tests.
- `make cp-web` must build clean (release binaries embed `web/dist`); run web tests with `cd crates/rupu-cp/web && npm test -- --run`.
- `cargo test -p rupu-cp && cargo clippy -p rupu-cp --all-targets` before every commit claim. Never package-wide `cargo fmt`.

---

### Task 1: API honesty — `unparsed` count + TS wire types

**Files:**
- Modify: `crates/rupu-cp/src/api/transcript.rs:157-172` (`get_transcript` local branch)
- Modify: `crates/rupu-cp/web/src/lib/transcript.ts`
- Test: inline `#[cfg(test)]` in `transcript.rs` (or the crate's existing API-test home if handlers are tested elsewhere — follow the neighboring pattern)

**Interfaces:**
- Produces: `GET /api/transcript` response `{ events, summary, unparsed: number }`. A parse failure on the FINAL line only is NOT counted (mid-write tail during live runs, expected). TS: `TranscriptResponse.unparsed?: number` plus event-union entries for the v2 variants (exact shapes below) consumed by Tasks 2–3.

- [ ] **Step 1: Write the failing Rust test** (pure helper extraction makes it testable — implement `read_events_counting_unparsed(path) -> (Vec<Event>, usize)` and test that):

```rust
#[cfg(test)]
mod read_tests {
    use super::read_events_counting_unparsed;
    use std::io::Write as _;

    #[test]
    fn unparsed_counts_bad_lines_but_forgives_a_truncated_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, r#"{{"type":"turn_start","data":{{"turn_idx":0}}}}"#).unwrap();
        writeln!(f, "THIS IS NOT JSON").unwrap();
        writeln!(f, r#"{{"type":"turn_end","data":{{"turn_idx":0}}}}"#).unwrap();
        write!(f, r#"{{"type":"assistant_delta","data":{{"conte"#).unwrap(); // torn tail
        let (events, unparsed) = read_events_counting_unparsed(&p).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(unparsed, 1, "mid-file garbage counts; the torn final line does not");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-cp unparsed_counts` → FAIL.

- [ ] **Step 3: Implement** in `transcript.rs`:

```rust
/// Read all parseable events, counting unparseable lines — EXCEPT a parse
/// failure on the final line, which is the signature of a mid-write tail on
/// a live transcript, not corruption.
fn read_events_counting_unparsed(
    path: &Path,
) -> Result<(Vec<rupu_transcript::Event>, usize), rupu_transcript::ReadError> {
    let mut events = Vec::new();
    let mut unparsed = 0usize;
    let mut last_line_failed = false;
    for ev in rupu_transcript::JsonlReader::iter(path)? {
        match ev {
            Ok(e) => {
                events.push(e);
                last_line_failed = false;
            }
            Err(_) => {
                unparsed += 1;
                last_line_failed = true;
            }
        }
    }
    if last_line_failed {
        unparsed -= 1;
    }
    Ok((events, unparsed))
}
```

and in `get_transcript`'s local branch replace the `.filter_map(Result::ok)` collect with:

```rust
    let (events, unparsed) =
        read_events_counting_unparsed(&path).map_err(|e| ApiError::internal(e.to_string()))?;
    let summary = rupu_transcript::JsonlReader::summary(&path).ok();
    Ok(Json(
        serde_json::json!({ "events": events, "summary": summary, "unparsed": unparsed }),
    ))
```

- [ ] **Step 4: TS wire types** — in `web/src/lib/transcript.ts`, extend the union (before the catch-all) and the response:

```ts
  | { type: 'thinking'; data: { text?: string | null; provider: string; model: string; raw?: unknown } }
  | { type: 'thinking_delta'; data: { content: string } }
  | { type: 'user_message'; data: { content: string } }
  | { type: 'seed'; data: { message_count: number; sha256?: string; source_transcript?: string | null; messages?: unknown } }
  | { type: 'notice'; data: { kind: string; message: string } }
  | { type: 'compaction'; data: { seq: number; summarized_messages: number; backup_path?: string; messages?: unknown } }
```

```ts
export interface TranscriptResponse {
  events: TranscriptEvent[];
  summary: TranscriptSummary | null;
  /** Lines the server could not parse (absent on older servers). */
  unparsed?: number;
}
```

- [ ] **Step 5: Run** — `cargo test -p rupu-cp && cd crates/rupu-cp/web && npx tsc -b` → PASS.

- [ ] **Step 6: Commit** — `git commit -m "feat(cp): /api/transcript reports unparsed lines; TS types for transcript schema v2"`

---

### Task 2: `transcriptView` — ordered blocks model, nothing dropped

**Files:**
- Modify: `crates/rupu-cp/web/src/components/transcript/transcriptView.ts`
- Test: `crates/rupu-cp/web/src/components/transcript/transcriptView.test.ts`

**Interfaces:**
- Consumes: Task 1 TS types.
- Produces (Task 3 renders exactly this):

```ts
export type TurnBlock =
  | { kind: 'assistant'; content: string }
  | { kind: 'thinking'; text: string | null; provider: string } // text null = redacted
  | { kind: 'user'; content: string }
  | { kind: 'tool'; view: ToolView }
  | { kind: 'gate'; gateId: string; prompt: string; decision: string | null; decidedBy: string | null }
  | { kind: 'seed'; messageCount: number; sourceTranscript: string | null }
  | { kind: 'notice'; noticeKind: string; message: string }
  | { kind: 'compaction'; seq: number; summarized: number }
  | { kind: 'unknown'; type: string };

export interface TurnView {
  blocks: TurnBlock[];
  tokensIn: number | null;
  tokensOut: number | null;
  summary: { toolCount: number; findingCount: number; result: 'ok' | 'error' | 'running' };
}
```

(`TranscriptHeader`/`TranscriptFooter`/`ToolView`/`ToolAuditView`/`classify`/`asFinding` unchanged.)

- [ ] **Step 1: Write the failing tests** (append to `transcriptView.test.ts`, matching its existing event-literal style):

```ts
describe('v2 blocks model', () => {
  it('groups by turn_start and preserves in-turn block order incl. thinking', () => {
    const view = buildTranscriptView([
      { type: 'turn_start', data: { turn_idx: 0 } },
      { type: 'thinking', data: { text: 'pick a tool', provider: 'anthropic', model: 'm' } },
      { type: 'tool_call', data: { call_id: 'c1', tool: 'grep', input: { q: 'x' } } },
      { type: 'tool_result', data: { call_id: 'c1', output: 'hit', duration_ms: 3 } },
      { type: 'turn_end', data: { turn_idx: 0, tokens_in: 11, tokens_out: 7 } },
      { type: 'turn_start', data: { turn_idx: 1 } },
      { type: 'thinking', data: { text: null, provider: 'anthropic', model: 'm' } },
      { type: 'assistant_message', data: { content: 'done' } },
      { type: 'turn_end', data: { turn_idx: 1, tokens_in: 5, tokens_out: 2 } },
    ]);
    expect(view.turns).toHaveLength(2);
    expect(view.turns[0].blocks.map((b) => b.kind)).toEqual(['thinking', 'tool']);
    expect(view.turns[0].tokensIn).toBe(11);
    const t1 = view.turns[1].blocks;
    expect(t1[0]).toEqual({ kind: 'thinking', text: null, provider: 'anthropic' });
    expect(t1[1]).toEqual({ kind: 'assistant', content: 'done' });
  });

  it('renders gate_requested, seed, user_message, notice, compaction and unknown — nothing dropped', () => {
    const view = buildTranscriptView([
      { type: 'seed', data: { message_count: 4 } },
      { type: 'user_message', data: { content: 'do the task' } },
      { type: 'turn_start', data: { turn_idx: 0 } },
      { type: 'gate_requested', data: { gate_id: 'g1', prompt: 'ship it?', decision: 'approve', decided_by: 'matt' } },
      { type: 'notice', data: { kind: 'context_trim', message: 'trimmed' } },
      { type: 'compaction', data: { seq: 1, summarized_messages: 9 } },
      { type: 'hologram_projection', data: { x: 1 } },
    ]);
    const kinds = view.turns.flatMap((t) => t.blocks.map((b) => b.kind));
    expect(kinds).toEqual(['seed', 'user', 'gate', 'notice', 'compaction', 'unknown']);
    const gate = view.turns.flatMap((t) => t.blocks).find((b) => b.kind === 'gate');
    expect(gate).toMatchObject({ gateId: 'g1', decision: 'approve', decidedBy: 'matt' });
  });

  it('legacy v1: assistant_message.thinking becomes a thinking block before the assistant block, and assistant_message still opens a turn when no turn_start groups it', () => {
    const view = buildTranscriptView([
      { type: 'assistant_message', data: { content: 'first', thinking: 'old-style' } },
      { type: 'tool_call', data: { call_id: 'c1', tool: 'bash', input: {} } },
      { type: 'assistant_message', data: { content: 'second' } },
    ]);
    expect(view.turns).toHaveLength(2);
    expect(view.turns[0].blocks.map((b) => b.kind)).toEqual(['thinking', 'assistant', 'tool']);
  });

  it('an orphan tool_result renders standalone instead of vanishing', () => {
    const view = buildTranscriptView([
      { type: 'turn_start', data: { turn_idx: 0 } },
      { type: 'tool_result', data: { call_id: 'ghost', output: 'late output', duration_ms: 1 } },
    ]);
    const tools = view.turns.flatMap((t) => t.blocks).filter((b) => b.kind === 'tool');
    expect(tools).toHaveLength(1);
    expect((tools[0] as { view: { output?: string } }).view.output).toBe('late output');
  });

  it('orphan file_edit / command_run render standalone', () => {
    const view = buildTranscriptView([
      { type: 'turn_start', data: { turn_idx: 0 } },
      { type: 'file_edit', data: { path: 'a.rs', kind: 'modify', diff: '--- x' } },
      { type: 'command_run', data: { argv: ['bash', '-lc', 'ls'], cwd: '/w', exit_code: 0, stdout_bytes: 1, stderr_bytes: 0 } },
    ]);
    const tools = view.turns.flatMap((t) => t.blocks).filter((b) => b.kind === 'tool');
    expect(tools).toHaveLength(2);
  });
});
```

Also update the EXISTING tests in the file mechanically: everywhere they read `turn.assistant?.content` / `turn.assistant?.thinking` / `turn.tools`, read the equivalent from `blocks` (`blocks.find(b => b.kind==='assistant')`, thinking blocks, `blocks.filter(b => b.kind==='tool').map(b => b.view)`). Behavior pinned by those tests (pairing, audits, findings, header/footer) must not change.

- [ ] **Step 2: Run to verify failure** — `cd crates/rupu-cp/web && npm test -- --run transcriptView` → FAIL.

- [ ] **Step 3: Rewrite the builder.** Keep every helper and the pairing structures; change turn management and add arms. Core changes to `buildTranscriptView`:

```ts
  const turns: TurnView[] = [];
  let current: TurnView | null = null;
  let sawTurnStart = false;

  function newTurn(): TurnView {
    const t: TurnView = {
      blocks: [],
      tokensIn: null,
      tokensOut: null,
      summary: { toolCount: 0, findingCount: 0, result: 'running' },
    };
    turns.push(t);
    current = t;
    return t;
  }
  function ensureTurn(): TurnView {
    return current ?? newTurn();
  }
```

Event arms (pairing maps `toolByCall` / `pendingDiff` / `pendingTerminal` / `pendingAuditsByTool` / `pendingActionArgsByTool` keep their existing logic — only where a view lands changes):

```ts
      case 'turn_start': {
        sawTurnStart = true;
        newTurn();
        break;
      }
      case 'turn_end': {
        const t = ensureTurn();
        t.tokensIn = asNumber(data.tokens_in);
        t.tokensOut = asNumber(data.tokens_out);
        break;
      }
      case 'thinking': {
        ensureTurn().blocks.push({
          kind: 'thinking',
          text: asString(data.text),
          provider: asString(data.provider) ?? '',
        });
        break;
      }
      case 'assistant_message': {
        // v1 files have no reliable turn framing convention here: keep the
        // legacy "assistant opens a turn" grouping unless turn_start is
        // doing the grouping.
        const turn = sawTurnStart ? ensureTurn() : newTurn();
        const legacyThinking = asString(data.thinking);
        if (legacyThinking !== null) {
          turn.blocks.push({ kind: 'thinking', text: legacyThinking, provider: '' });
        }
        turn.blocks.push({ kind: 'assistant', content: asString(data.content) ?? '' });
        break;
      }
      case 'user_message': {
        ensureTurn().blocks.push({ kind: 'user', content: asString(data.content) ?? '' });
        break;
      }
      case 'seed': {
        ensureTurn().blocks.push({
          kind: 'seed',
          messageCount: asNumber(data.message_count) ?? 0,
          sourceTranscript: asString(data.source_transcript),
        });
        break;
      }
      case 'gate_requested': {
        ensureTurn().blocks.push({
          kind: 'gate',
          gateId: asString(data.gate_id) ?? '',
          prompt: asString(data.prompt) ?? '',
          decision: asString(data.decision),
          decidedBy: asString(data.decided_by),
        });
        break;
      }
      case 'notice': {
        ensureTurn().blocks.push({
          kind: 'notice',
          noticeKind: asString(data.kind) ?? '',
          message: asString(data.message) ?? '',
        });
        break;
      }
      case 'compaction': {
        ensureTurn().blocks.push({
          kind: 'compaction',
          seq: asNumber(data.seq) ?? 0,
          summarized: asNumber(data.summarized_messages) ?? 0,
        });
        break;
      }
```

`tool_call` pushes `{ kind: 'tool', view }` via `ensureTurn().blocks.push(...)` (everything else in that arm unchanged). `tool_result`'s orphan path becomes:

```ts
        // Orphan result (no tool_call in this snapshot): standalone card.
        if (!view) {
          const orphan: ToolView = { tool: '(result)', input: undefined, kind: 'generic' };
          if (callId !== '') orphan.callId = callId;
          const output = asString(data.output);
          if (output !== null) orphan.output = output;
          const error = asString(data.error);
          if (error !== null) orphan.error = error;
          const durationMs = asNumber(data.duration_ms);
          if (durationMs !== null) orphan.durationMs = durationMs;
          ensureTurn().blocks.push({ kind: 'tool', view: orphan });
        }
```

`file_edit` / `command_run` orphan paths (when `pendingDiff` / `pendingTerminal` are null) push a standalone `{kind:'tool'}` block with `tool: 'file_edit'` (kind `'diff'`, the `diff` payload attached) / `tool: 'command_run'` (kind `'terminal'`, the terminal payload attached) instead of dropping. `tool_audit`'s standalone fallback pushes its block via `ensureTurn().blocks.push({ kind: 'tool', view: ... })`. The `default:` arm becomes:

```ts
      case 'assistant_delta':
      case 'thinking_delta':
        break; // consolidated events carry the same content
      case 'run_start':
      case 'usage':
      case 'run_complete':
        /* header/footer arms above, unchanged */
        break;
      default: {
        ensureTurn().blocks.push({ kind: 'unknown', type: ev.type });
        break;
      }
```

Summary finalization derives from blocks:

```ts
  for (const turn of turns) {
    const tools = turn.blocks.filter((b): b is Extract<TurnBlock, { kind: 'tool' }> => b.kind === 'tool');
    const hasError = tools.some((t) => t.view.error !== undefined);
    turn.summary = {
      toolCount: tools.length,
      findingCount: tools.filter((t) => t.view.kind === 'finding').length,
      result: hasError ? 'error' : sawRunComplete ? 'ok' : 'running',
    };
  }
  // Drop turns that ended up with no blocks (e.g. a turn_start whose turn
  // only produced deltas) so empty shells don't render.
  const nonEmpty = turns.filter((t) => t.blocks.length > 0);
  return { header, turns: nonEmpty, footer };
```

- [ ] **Step 4: Run** — `npm test -- --run transcriptView && npx tsc -b` → PASS (new + updated legacy tests). `tsc` will flag every other consumer of the old `TurnView` shape (`turnSeries.ts`, `TranscriptPanel.tsx`, `Turn.tsx`): update `turnSeries.ts` and any non-`Turn.tsx` consumer now with the mechanical accessors (`blocks.find(b => b.kind === 'assistant')`, `blocks.filter(b => b.kind === 'tool').map(b => b.view)`); `Turn.tsx` itself is Task 3.

- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): transcriptView v2 — ordered blocks, turn_start grouping, gate/seed/notice/compaction/unknown, standalone orphans"`

---

### Task 3: Render the blocks — Turn.tsx, TranscriptPanel badge, per-turn tokens

**Files:**
- Modify: `crates/rupu-cp/web/src/components/transcript/Turn.tsx`
- Modify: `crates/rupu-cp/web/src/components/transcript/TranscriptPanel.tsx` (header block at ~:162-189; wherever it holds the `getTranscript` response)
- Modify: `crates/rupu-cp/web/src/lib/api.ts:2559-2583` (`getTranscript` return type already covered by Task 1's `TranscriptResponse`)
- Test: `crates/rupu-cp/web/src/components/transcript/Turn.test.tsx`

**Interfaces:**
- Consumes: Task 2's `TurnView.blocks` / `tokensIn/tokensOut`; Task 1's `unparsed`.

- [ ] **Step 1: Write the failing tests** (follow `Turn.test.tsx`'s existing render/query style):

```tsx
it('renders blocks in order: thinking (collapsible, full text), assistant, gate', () => {
  render(
    <Turn
      defaultOpen
      turn={{
        blocks: [
          { kind: 'thinking', text: 'long reasoning body', provider: 'anthropic' },
          { kind: 'assistant', content: 'the answer' },
          { kind: 'gate', gateId: 'g1', prompt: 'ship it?', decision: null, decidedBy: null },
        ],
        tokensIn: 11,
        tokensOut: 7,
        summary: { toolCount: 0, findingCount: 0, result: 'ok' },
      }}
    />,
  );
  fireEvent.click(screen.getByText('thinking'));
  expect(screen.getByText('long reasoning body')).toBeInTheDocument();
  expect(screen.getByText('the answer')).toBeInTheDocument();
  expect(screen.getByText('ship it?')).toBeInTheDocument();
  expect(screen.getByText(/11.*→.*7|11 in.*7 out/)).toBeInTheDocument();
});

it('renders a redacted thinking block as a marker, and notice/seed/compaction/unknown rows', () => {
  render(
    <Turn
      defaultOpen
      turn={{
        blocks: [
          { kind: 'seed', messageCount: 4, sourceTranscript: null },
          { kind: 'user', content: 'do the task' },
          { kind: 'thinking', text: null, provider: 'anthropic' },
          { kind: 'notice', noticeKind: 'context_trim', message: 'trimmed to fit' },
          { kind: 'compaction', seq: 1, summarized: 9 },
          { kind: 'unknown', type: 'hologram_projection' },
        ],
        tokensIn: null,
        tokensOut: null,
        summary: { toolCount: 0, findingCount: 0, result: 'running' },
      }}
    />,
  );
  expect(screen.getByText(/redacted reasoning/)).toBeInTheDocument();
  expect(screen.getByText(/4 prior messages/)).toBeInTheDocument();
  expect(screen.getByText('do the task')).toBeInTheDocument();
  expect(screen.getByText(/trimmed to fit/)).toBeInTheDocument();
  expect(screen.getByText(/summarized 9/)).toBeInTheDocument();
  expect(screen.getByText(/hologram_projection/)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run to verify failure** — `npm test -- --run Turn` → FAIL.

- [ ] **Step 3: Implement `Turn.tsx`.** Header stays: derive the snippet from the first assistant block; add a tokens pill; body iterates blocks:

```tsx
const firstAssistant = turn.blocks.find(
  (b): b is Extract<TurnBlock, { kind: 'assistant' }> => b.kind === 'assistant',
);
```

Pills row addition (next to the existing tool/finding pills):

```tsx
{turn.tokensIn !== null && turn.tokensOut !== null && (
  <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[9px] font-medium bg-surface text-ink-mute">
    {turn.tokensIn} in · {turn.tokensOut} out
  </span>
)}
```

Body:

```tsx
{open && (
  <div className="flex flex-col gap-1.5 border-t border-border px-3 pb-3 pt-2">
    {turn.blocks.map((block, i) => (
      <TurnBlockView key={i} block={block} onOpenTranscript={onOpenTranscript} runId={runId} host={host} />
    ))}
  </div>
)}
```

New components in `Turn.tsx` (the file stays the one place turn-block chrome lives):

```tsx
function TurnBlockView({ block, onOpenTranscript, runId, host }: {
  block: TurnBlock;
  onOpenTranscript?: (path: string) => void;
  runId?: string;
  host?: string;
}) {
  switch (block.kind) {
    case 'assistant':
      return (
        <div>
          <div className="mb-1 text-[9px] font-bold uppercase tracking-wide text-brand-500">assistant</div>
          <Markdown text={block.content} />
        </div>
      );
    case 'thinking':
      return <ThinkingBlock text={block.text} provider={block.provider} />;
    case 'user':
      return (
        <div>
          <div className="mb-1 text-[9px] font-bold uppercase tracking-wide text-ink-mute">prompt</div>
          <Markdown text={block.content} />
        </div>
      );
    case 'tool':
      return <ToolCard tool={block.view} onOpenTranscript={onOpenTranscript} runId={runId} host={host} />;
    case 'gate':
      return (
        <div className="rounded-lg border-l-2 border-warn bg-warn-bg/40 px-2.5 py-2">
          <div className="flex items-center gap-1.5 text-[9px] font-bold uppercase tracking-wide text-warn">
            gate <span className="font-normal normal-case text-ink-mute">#{block.gateId}</span>
          </div>
          <div className="text-note text-ink">{block.prompt}</div>
          {block.decision && (
            <div className="text-meta text-ink-dim">
              {block.decidedBy ? `${block.decision} by ${block.decidedBy}` : block.decision}
            </div>
          )}
        </div>
      );
    case 'seed':
      return (
        <div className="text-meta text-ink-mute">
          {block.messageCount} prior messages seed this run (stored once at the source)
          {block.sourceTranscript && onOpenTranscript && (
            <button
              type="button"
              onClick={() => onOpenTranscript(block.sourceTranscript as string)}
              className="ml-1.5 text-brand-500 underline-offset-2 hover:underline"
            >
              view source transcript
            </button>
          )}
        </div>
      );
    case 'notice':
      return (
        <div className="text-meta text-ink-mute">
          <span className="font-medium">{block.noticeKind}</span> · {block.message}
        </div>
      );
    case 'compaction':
      return (
        <div className="text-meta text-ink-mute">
          context compacted · seq {block.seq} · summarized {block.summarized} messages
        </div>
      );
    case 'unknown':
      return (
        <div className="text-meta italic text-ink-mute">
          unrecognized event: {block.type} (newer rupu wrote this transcript)
        </div>
      );
  }
}

function ThinkingBlock({ text, provider }: { text: string | null; provider: string }) {
  const [show, setShow] = useState(false);
  if (text === null || text === '') {
    return (
      <div className="text-meta italic text-ink-mute">
        [redacted reasoning{provider ? ` · ${provider}` : ''}]
      </div>
    );
  }
  return (
    <div>
      <button
        type="button"
        onClick={() => setShow((v) => !v)}
        className="flex items-center gap-1 text-meta font-medium text-ink-mute"
      >
        {show ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        thinking
      </button>
      {show && (
        <div className="mt-1 border-l-2 border-border pl-2 text-ink-mute opacity-80">
          <Markdown text={text} />
        </div>
      )}
    </div>
  );
}
```

Delete the now-dead `turn.assistant`/`turn.tools`-based body and the old `showThinking` state.

- [ ] **Step 4: `TranscriptPanel` unparsed badge.** Where the panel holds the `TranscriptResponse`, surface a warning chip in the existing header row (~:162-189), rendered only when `unparsed` is a positive number:

```tsx
{typeof unparsed === 'number' && unparsed > 0 && (
  <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wide ring-1 ring-inset bg-warn-bg text-warn ring-warn/30">
    {unparsed} unparsed {unparsed === 1 ? 'line' : 'lines'}
  </span>
)}
```

(Live-tail SSE events append to the same event list; the badge covers the REST snapshot — fine, torn tails are excluded server-side.)

- [ ] **Step 5: Run everything** — `npm test -- --run && npx tsc -b && cd ../../.. && make cp-web && cargo test -p rupu-cp` → PASS.

- [ ] **Step 6: Commit + PR**

```bash
git add crates/rupu-cp
git commit -m "feat(cp-web): render every transcript block — thinking in position, gates, per-turn tokens, orphans, unparsed badge"
git push origin feat/transcript-fidelity-plan-2
gh pr create --title "Transcript fidelity 2/3: CP web display parity" --body "Implements docs/superpowers/plans/2026-08-31-rupu-transcript-fidelity-plan-2-cp-web-display.md (spec: docs/superpowers/specs/2026-08-31-rupu-transcript-fidelity-design.md). Depends on Plan 1.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

Note for the reviewer/merger: visual pass required — matt loads a real run (one with gates, one legacy v1 transcript) in the CP web UI before merge.
