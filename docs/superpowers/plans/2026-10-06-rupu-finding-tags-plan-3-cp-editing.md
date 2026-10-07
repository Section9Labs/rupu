# Finding Tags — Plan 3 (tag editing in the CP) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Operators can add and remove tags in the CP web UI:
- on a single finding, from its page, which also shows the finding's tag history
- on many findings at once, from the Findings table, using row checkboxes and a bulk bar

**Architecture:** Two new endpoints, `POST /api/findings/tags` and `GET /api/findings/tags`, are thin layers over Plan 1's `tag_findings_across` and `tags_in_use`. `GET /api/findings/:id` gains `tag_history` and `tags_editable`. The web gains:
- a reusable `TagInput`/`TagEditor`/`TagHistory` set in `components/findings/tags/`
- optional row selection in the generic `SortableTable`
- a `BulkTagBar`

The server stays the authority. The client pre-validates tags with the TS `parseTag` twin only for instant feedback.

**Tech Stack:** Rust (axum) · React 18 + TypeScript + Tailwind · lucide-react · vitest + Testing Library.

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md`. Read the "Surfaces → CP" and "Query language (Plans 2–3)" sections. Plans 1 (#766) and 2 (#770) are merged. The UI layout follows the mock matt approved on 2026-10-06:
- chips in the table
- a bulk bar that appears only while rows are selected, with "Tag…" / "Untag…" opening an autocomplete
- on the finding page, removable chips plus "+ tag" in the header, with a collapsible tag history naming the agent codename or the operator, and when

## Global Constraints

- **Tag syntax** (shared Rust `Tag::parse` / TS `parseTag`):
  - Trim, then lowercase. The result is 1–64 characters from `[a-z0-9._:/-]`, and the first character is `[a-z0-9]`.
  - An invalid tag is rejected, never rewritten.
  - A finding can hold at most 32 tags. A change that would grow a finding past 32 is refused.
- **One writer.** Every write goes through `rupu_coverage::apply`, via `rupu_cp::api::findings::tag_findings_across`. CP writes are attributed `TagActor::operator(OperatorSurface::Cp)`.
- **Atomicity.** Each workspace's batch is atomic; the request as a whole is not. Results are reported per workspace, plus a list of `unknown` ids.
- **Unreadable tag logs (decision A):**
  - The findings stay visible with their declared tags.
  - Tag editing is **disabled** for them: their row checkboxes are disabled with a reason, and the finding page shows tags read-only.
  - The workspaces in question are those listed in the list response's `tags_unavailable: [{ws_id, project}]`, or those where the finding detail reports `tags_editable: false`.
- **Rust repo rules:**
  - Workspace deps only. No new npm deps either.
  - One integration-test binary per crate.
  - `#![deny(clippy::all)]`.
  - Run rustfmt per file, never `cargo fmt`. Revert any reflow of lines you didn't write.
  - Run targeted tests only (`cargo test -p rupu-cp --lib api::findings`, `npx vitest run <paths>`).
- **Web rules:**
  - Use rupu Tailwind tokens: `panel`, `surface`, `surface-hover`, `surface-active`, `border`, `ink*`, `brand-*`, `err`/`err-bg`, `warn`/`warn-bg`, `ok`/`ok-bg`.
  - Avoid `Array.prototype.at` (the lib target lacks it).
  - vitest runs with `globals: false`, so add `afterEach(cleanup)` in Testing Library files.
- **Git:** no stash, no checkout, no push. Every commit ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Fixtures** are invented, never assessment data.

## File map

| File | Responsibility |
|---|---|
| `crates/rupu-cp/src/api/findings.rs` | `POST /api/findings/tags`, `GET /api/findings/tags`, detail `tag_history` + `tags_editable` |
| `crates/rupu-cp/web/src/lib/api.ts` | tag types, `tagFindings`, `getTagsInUse`, `FindingDetail` additions |
| `crates/rupu-cp/web/src/components/findings/tags/{TagInput,TagEditor,TagHistory,tagResult}.ts(x)` (new) | reusable tag UI + result summary |
| `crates/rupu-cp/web/src/components/lists/SortableTable.tsx` | optional row selection |
| `crates/rupu-cp/web/src/components/findings/{FindingsTable,BulkTagBar}.tsx` | selection passthrough, bulk bar |
| `crates/rupu-cp/web/src/pages/{Findings,FindingDetail}.tsx` | wiring |
| `crates/rupu-cp/web/src/components/query/{QueryBar.tsx,suggest.ts}` | Plan 2 polish |
| `docs/coverage.md`, `CLAUDE.md`, spec | docs |

---

### Task 1: CP endpoints — write tags, tags in use, detail history

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs` (routes, the two new handlers, the `FindingDetail` struct, `get_finding`)
- Test: the in-file test module of `findings.rs`. Use the same helpers Plan 2's `list_findings_*` tests use: workspace registration, finding seeding, `app_for` + `oneshot`.

**Interfaces:**
- Consumes (all exist):
  - `tag_findings_across(&Path, &TagChange, &TagActor) -> Result<TagAcrossResult, TagError>`
  - `collect_all_findings`
  - `rupu_coverage::{TagChangeInput, TagActor, OperatorSurface, TagLog, tag_history, tags_in_use, TagCount, TagEvent}`
  - `crate::api::code::load_workspace(&AppState, &ws_id)`
- Produces:
  - **`POST /api/findings/tags`**, body `{finding_ids, add?, remove?}` (`TagChangeInput`, `deny_unknown_fields`):
    - **200** `TagAcrossResult`: `{workspaces: [{ws_id, outcomes?, error?}], unknown: [id]}`
    - **400** `{error}` for an invalid tag, an empty change, add+remove overlap, no ids, or more than 1000 ids
    - **404** `{error}` when no id matches any registered workspace
    - **422** from axum's JSON extractor for an unknown field or a malformed body
  - **`GET /api/findings/tags?ws_id=`**: `[{tag, count}]`, most used first (`tags_in_use` over the scope)
  - **`GET /api/findings/:id`** gains:
    - `tag_history: TagEvent[]`, in file order
    - `tags_editable: bool`, false when the owning workspace or its tag log can't be read

- [ ] **Step 1: Write the failing tests.** Add them to the in-file test module. Seed two workspaces: `ws1` with `fnd_a` and `fnd_b`, and `ws2` with `fnd_c`. Then:
  1. `POST {"finding_ids":["fnd_a","fnd_c"],"add":["Needs-POC"]}` returns 200.
     - `workspaces` has two entries, each with `outcomes[0].after == ["needs-poc"]`, and `unknown == []`.
     - Afterwards, `GET /api/findings?q=tag%3Aneeds-poc` lists both findings.
     - The events in `ws1`'s `finding_tags.jsonl` carry `"by":{"kind":"operator",…,"via":"cp"}`.
  2. `POST {"finding_ids":["fnd_a","fnd_nope"],"remove":["x"]}` returns 200 with `unknown == ["fnd_nope"]`.
  3. `POST {"finding_ids":["fnd_nope"],"add":["x"]}` returns 404.
  4. Bad requests return 400, each with an `error` string:
     - `{"finding_ids":["fnd_a"],"add":["bad tag"]}`
     - `{"finding_ids":["fnd_a"]}`
     - `{"finding_ids":["fnd_a"],"add":["x"],"remove":["x"]}`
     - 1001 ids
  5. `{"finding_ids":["fnd_a"],"tags":["x"]}` returns a 4xx.
  6. Within one test: tag as in test 1, then `GET /api/findings/tags` returns `[{"tag":"needs-poc","count":2}]`. `GET /api/findings/tags?ws_id=ws2` returns count 1.
  7. Within one test: tag `fnd_a` as in test 1, then `GET /api/findings/fnd_a` has:
     - `tags == ["needs-poc"]`
     - `tags_editable == true`
     - `tag_history.len() == 1`, with `tag_history[0].op == "add"`

     Make `ws1`'s `finding_tags.jsonl` a directory and the same request returns `tags_editable == false`, `tag_history == []`, and still 200.
  8. A `POST` touching `fnd_a` (in the unreadable `ws1`) and `fnd_c` returns 200. The `ws1` entry has an `error` and the `ws2` entry has `outcomes`.

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-cp --lib api::findings`
  - Expected: FAIL. The routes don't exist yet.

- [ ] **Step 3: Implement.** Add the routes before `/api/findings/:id`. The static `tags` segment wins over the parameter either way, as `export` does.

```rust
        .route("/api/findings/tags", get(tags_in_use).post(tag_findings))
```

  Then the handlers, after `list_findings`:

```rust
/// At most this many findings per `POST /api/findings/tags`.
const MAX_TAG_BATCH: usize = 1000;

/// `POST /api/findings/tags` — add/remove tags on findings wherever they
/// live (`tag_findings_across`: atomic per workspace, results per workspace),
/// attributed to the CP operator.
async fn tag_findings(
    State(s): State<AppState>,
    Json(body): Json<rupu_coverage::TagChangeInput>,
) -> ApiResult<Json<TagAcrossResult>> {
    if body.finding_ids.len() > MAX_TAG_BATCH {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_TAG_BATCH} findings per change"
        )));
    }
    let change = body
        .into_change()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    change
        .check()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let global = s.global_dir.clone();
    let by = rupu_coverage::TagActor::operator(rupu_coverage::OperatorSurface::Cp);
    let result = tokio::task::spawn_blocking(move || tag_findings_across(&global, &change, &by))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if result.workspaces.is_empty() {
        return Err(ApiError::not_found(format!(
            "unknown finding id(s): {}",
            result.unknown.join(", ")
        )));
    }
    Ok(Json(result))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TagsQuery {
    pub ws_id: Option<String>,
}

/// `GET /api/findings/tags` — the tags in use (with how many findings carry
/// each), most used first: the tag editor's autocomplete.
async fn tags_in_use(
    State(s): State<AppState>,
    Query(q): Query<TagsQuery>,
) -> ApiResult<Json<Vec<rupu_coverage::TagCount>>> {
    let global = s.global_dir.clone();
    let counts = tokio::task::spawn_blocking(move || {
        let all = collect_all_findings(&global);
        rupu_coverage::tags_in_use(
            all.iter()
                .filter(|f| q.ws_id.as_deref().is_none_or(|w| f.ws_id == w))
                .map(|f| &f.record),
        )
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(counts))
}
```

  Add the fields to `FindingDetail`:

```rust
    /// This finding's tag changes, in file order (`ledger::tags`).
    pub tag_history: Vec<rupu_coverage::TagEvent>,
    /// Whether its workspace's tag log can be read (and so edited). False
    /// keeps the declared tags visible, read-only (decision A).
    pub tags_editable: bool,
```

  In `get_finding`, after `workflow_name` is set:

```rust
    let (tag_history, tags_editable) = match crate::api::code::load_workspace(&s, &finding.ws_id) {
        Ok(ws) => {
            let log = rupu_coverage::TagLog::for_workspace(std::path::Path::new(&ws.path));
            let id_for_history = finding.record.id.clone();
            match tokio::task::spawn_blocking(move || rupu_coverage::tag_history(&log, &id_for_history))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
            {
                Ok(h) => (h, true),
                Err(_) => (Vec::new(), false),
            }
        }
        Err(_) => (Vec::new(), false),
    };
```

  - Construct `FindingDetail` with both fields.
  - If `load_workspace`'s signature or the ws path field differs, adapt to the real one. It's used a few lines below, in `get_finding`.
  - If `Option::is_none_or` isn't available under the pinned toolchain, use `map_or(true, |w| f.ws_id == w)`.

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run `cargo test -p rupu-cp --lib api::findings`, `cargo test -p rupu-cp --test it finding_artifacts::`, `cargo check --workspace --all-targets`, and clippy `-D warnings` on rupu-cp.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/src/api/findings.rs
git commit -m "feat(cp): POST/GET /api/findings/tags and finding tag history

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Web API types + reusable tag components

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`
- Create: `crates/rupu-cp/web/src/components/findings/tags/TagInput.tsx`, `TagEditor.tsx`, `TagHistory.tsx`, `tagResult.ts`
- Create tests: `crates/rupu-cp/web/src/components/findings/tags/TagEditor.test.tsx`, `TagHistory.test.tsx`, `tagResult.test.ts`

**Interfaces:**
- Consumes:
  - `parseTag` (`lib/findingQuery/grammar.ts`)
  - `fuzzyScore` (`lib/fuzzy.ts`)
  - `apiErrorMessage` and `request` (`lib/api.ts`)
  - `cn`
- Produces in `api.ts`:

```ts
export type TagActor =
  | { kind: 'agent'; run_id: string; model: string; surface: string; codename?: string; agent?: string; provider?: string }
  | { kind: 'operator'; user: string; via: 'cli' | 'cp' };
export interface TagEvent { id: string; finding_id: string; op: 'add' | 'remove'; tag: string; by: TagActor; at: string }
export interface TagOutcome { finding_id: string; before: string[]; after: string[] }
export interface WorkspaceTagResult { ws_id: string; outcomes?: TagOutcome[]; error?: string }
export interface TagAcrossResult { workspaces: WorkspaceTagResult[]; unknown: string[] }
export interface TagCount { tag: string; count: number }
// FindingDetail gains: tag_history: TagEvent[]; tags_editable: boolean;
// api.tagFindings(findingIds: string[], change: { add?: string[]; remove?: string[] }): Promise<TagAcrossResult>   // POST /api/findings/tags
// api.getTagsInUse(opts?: { wsId?: string }): Promise<TagCount[]>                                                   // GET /api/findings/tags
```

- Produces the following components:
  - `TagInput({suggestions, exclude?, onSubmit(tag), onCancel?, label})` — a validated, autocompleting single-tag input
  - `TagEditor({tags, suggestions, disabledReason?, onAdd(tag): Promise<void>, onRemove(tag): Promise<void>})`
  - `TagHistory({events})`
  - `summarizeTagResult(r: TagAcrossResult, mode: 'add' | 'remove', projectOf: (wsId: string) => string): { message: string; ok: boolean }`

- [ ] **Step 1: Write the failing tests.**

`tagResult.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { summarizeTagResult } from './tagResult';

const proj = (ws: string) => ({ ws1: 'shop-web', ws2: 'billing-api' })[ws] ?? ws;

describe('summarizeTagResult', () => {
  it('counts changed and unchanged findings', () => {
    const r = {
      workspaces: [{ ws_id: 'ws1', outcomes: [
        { finding_id: 'a', before: [], after: ['x'] },
        { finding_id: 'b', before: ['x'], after: ['x'] },
      ] }],
      unknown: [],
    };
    expect(summarizeTagResult(r, 'add', proj)).toEqual({ message: 'Tagged 1 finding (1 already had it).', ok: true });
  });
  it('names failed projects and missing findings', () => {
    const r = {
      workspaces: [
        { ws_id: 'ws1', outcomes: [{ finding_id: 'a', before: ['x'], after: [] }] },
        { ws_id: 'ws2', error: 'finding-tag log I/O: Is a directory' },
      ],
      unknown: ['gone1', 'gone2'],
    };
    expect(summarizeTagResult(r, 'remove', proj)).toEqual({
      message: "Untagged 1 finding. billing-api's tags couldn't be changed: finding-tag log I/O: Is a directory. 2 findings no longer exist.",
      ok: false,
    });
  });
});
```

`TagEditor.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TagEditor } from './TagEditor';

afterEach(cleanup);
const sugg = [{ tag: 'needs-poc', count: 3 }, { tag: 'class:sqli', count: 2 }];

describe('TagEditor', () => {
  it('removes a tag with its ✕', async () => {
    const onRemove = vi.fn().mockResolvedValue(undefined);
    render(<TagEditor tags={['needs-poc']} suggestions={sugg} onAdd={vi.fn()} onRemove={onRemove} />);
    fireEvent.click(screen.getByRole('button', { name: 'Remove tag needs-poc' }));
    await waitFor(() => expect(onRemove).toHaveBeenCalledWith('needs-poc'));
  });
  it('adds a typed tag, normalized, and offers in-use tags not already present', async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    render(<TagEditor tags={['needs-poc']} suggestions={sugg} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    expect(screen.queryByRole('option', { name: /needs-poc/ })).toBeNull();
    expect(screen.getByRole('option', { name: /class:sqli/ })).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: ' Triaged ' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Add tag' }), { key: 'Enter' });
    await waitFor(() => expect(onAdd).toHaveBeenCalledWith('triaged'));
  });
  it('rejects an invalid tag without calling onAdd', () => {
    const onAdd = vi.fn();
    render(<TagEditor tags={[]} suggestions={[]} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: 'not ok' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Add tag' }), { key: 'Enter' });
    expect(onAdd).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('invalid tag');
  });
  it('shows a server error from onAdd', async () => {
    render(<TagEditor tags={[]} suggestions={[]} onAdd={vi.fn().mockRejectedValue(new Error('too many tags'))} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: 'x' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Add tag' }), { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent('too many tags');
  });
  it('is read-only with a reason when disabled', () => {
    render(<TagEditor tags={['x']} suggestions={[]} disabledReason="this project's tags couldn't be read" onAdd={vi.fn()} onRemove={vi.fn()} />);
    expect(screen.queryByRole('button', { name: 'Remove tag x' })).toBeNull();
    expect(screen.getByText('tags read-only')).toHaveAttribute('title', "this project's tags couldn't be read");
  });
});
```

`TagHistory.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { TagHistory } from './TagHistory';
import type { TagEvent } from '../../../lib/api';

afterEach(cleanup);

const ev = (op: 'add' | 'remove', tag: string, by: TagEvent['by'], at: string): TagEvent => ({ id: `tge_${tag}${op}`, finding_id: 'fnd_a', op, tag, by, at });

describe('TagHistory', () => {
  it('lists newest first with the actor', () => {
    render(
      <TagHistory
        events={[
          ev('add', 'class:sqli', { kind: 'agent', run_id: 'run_1', model: 'm', surface: 'workflow', agent: 'sqli-hunter', codename: 'jade-reef/heron#3' }, '2026-10-06T10:00:00Z'),
          ev('remove', 'false-positive', { kind: 'operator', user: 'alice', via: 'cp' }, '2026-10-06T11:00:00Z'),
        ]}
      />,
    );
    const items = screen.getAllByRole('listitem');
    expect(items[0]).toHaveTextContent('− false-positive');
    expect(items[0]).toHaveTextContent('alice via cp');
    expect(items[1]).toHaveTextContent('+ class:sqli');
    expect(items[1]).toHaveTextContent('jade-reef/heron#3 (sqli-hunter)');
  });
  it('says so when there is no history', () => {
    render(<TagHistory events={[]} />);
    expect(screen.getByText('No tag changes yet.')).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/tags`
  - Expected: FAIL, because the modules don't exist.

- [ ] **Step 3: Implement.** Start with `api.ts`: add the types above. Extend the `FindingDetail` interface with `tag_history: TagEvent[]` and `tags_editable: boolean`. Add the two methods next to `getFinding`:

```ts
  tagFindings(findingIds: string[], change: { add?: string[]; remove?: string[] }): Promise<TagAcrossResult> {
    return request<TagAcrossResult>('/api/findings/tags', {
      method: 'POST',
      body: JSON.stringify({ finding_ids: findingIds, add: change.add ?? [], remove: change.remove ?? [] }),
    });
  },
  getTagsInUse(opts?: { wsId?: string }): Promise<TagCount[]> {
    const qs = opts?.wsId ? `?ws_id=${encodeURIComponent(opts.wsId)}` : '';
    return request<TagCount[]>(`/api/findings/tags${qs}`);
  },
```

  Existing test fixtures that build a `FindingDetail` must add `tag_history: []` and `tags_editable: true`. Fix every one that `tsc` reports.

`tagResult.ts`:

```ts
// One line for what a bulk tag change did (`POST /api/findings/tags`), for
// the bulk bar and the finding page.
import type { TagAcrossResult } from '../../../lib/api';

const plural = (n: number) => `${n} finding${n === 1 ? '' : 's'}`;

export function summarizeTagResult(
  r: TagAcrossResult,
  mode: 'add' | 'remove',
  projectOf: (wsId: string) => string,
): { message: string; ok: boolean } {
  const outcomes = r.workspaces.flatMap((w) => w.outcomes ?? []);
  const changed = outcomes.filter((o) => o.before.join('\u0000') !== o.after.join('\u0000')).length;
  const same = outcomes.length - changed;
  const parts = [
    `${mode === 'add' ? 'Tagged' : 'Untagged'} ${plural(changed)}${same > 0 ? ` (${same} already ${mode === 'add' ? 'had it' : "didn't have it"})` : ''}.`,
  ];
  const failed = r.workspaces.filter((w) => w.error);
  for (const w of failed) parts.push(`${projectOf(w.ws_id)}'s tags couldn't be changed: ${w.error}.`);
  if (r.unknown.length > 0) parts.push(`${plural(r.unknown.length)} no longer exist${r.unknown.length === 1 ? 's' : ''}.`);
  return { message: parts.join(' '), ok: failed.length === 0 && r.unknown.length === 0 };
}
```

`TagInput.tsx`:

```tsx
// One free-form tag, validated with the same rules as the server
// (`parseTag`), with the tags already in use offered most-used first.
import { useId, useMemo, useState } from 'react';
import { cn } from '../../../lib/cn';
import { parseTag } from '../../../lib/findingQuery/grammar';
import { fuzzyScore } from '../../../lib/fuzzy';

export interface TagSuggestion {
  tag: string;
  count: number;
}

export function TagInput({
  suggestions,
  exclude = [],
  onSubmit,
  onCancel,
  label,
}: {
  suggestions: TagSuggestion[];
  exclude?: string[];
  onSubmit: (tag: string) => void;
  onCancel?: () => void;
  label: string;
}) {
  const listId = useId();
  const [text, setText] = useState('');
  const [active, setActive] = useState(-1);
  const [error, setError] = useState<string | null>(null);
  const options = useMemo(() => {
    const skip = new Set(exclude);
    const needle = text.trim();
    return suggestions
      .filter((s) => s.tag !== '' && !skip.has(s.tag))
      .map((s) => ({ ...s, hit: fuzzyScore(needle, s.tag) }))
      .filter((s) => s.hit !== null)
      .sort((a, b) => (needle === '' ? 0 : b.hit!.score - a.hit!.score) || b.count - a.count)
      .slice(0, 8);
  }, [suggestions, exclude, text]);

  const submit = (raw: string) => {
    const p = parseTag(raw);
    if (!p.ok) {
      setError(p.message);
      return;
    }
    onSubmit(p.tag);
    setText('');
    setActive(-1);
    setError(null);
  };

  return (
    <div className="relative">
      <input
        role="combobox"
        aria-label={label}
        aria-autocomplete="list"
        aria-haspopup="listbox"
        aria-expanded={options.length > 0}
        aria-controls={listId}
        aria-activedescendant={active >= 0 ? `${listId}-${active}` : undefined}
        // eslint-disable-next-line jsx-a11y/no-autofocus -- opened on purpose by the "+ tag" button
        autoFocus
        value={text}
        placeholder="tag…"
        onChange={(e) => {
          setText(e.target.value);
          setActive(-1);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            if (options.length === 0) return;
            const step = e.key === 'ArrowDown' ? 1 : -1;
            setActive((i) => (i + step + options.length + (i < 0 && step < 0 ? 1 : 0)) % options.length);
          } else if (e.key === 'Enter') {
            e.preventDefault();
            submit(active >= 0 && options[active] ? options[active].tag : text);
          } else if (e.key === 'Escape') {
            e.preventDefault();
            onCancel?.();
          }
        }}
        className="w-44 rounded-md border border-border bg-panel px-2 py-0.5 font-mono text-note text-ink outline-none focus:ring-2 focus:ring-brand-500/50"
      />
      {error && (
        <p role="alert" className="mt-1 text-note text-err">
          {error}
        </p>
      )}
      {options.length > 0 && (
        <ul
          id={listId}
          role="listbox"
          aria-label="Tags in use"
          className="absolute z-30 mt-1 w-56 rounded-lg border border-border bg-panel py-1 shadow-card"
        >
          {options.map((o, i) => (
            <li
              key={o.tag}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => submit(o.tag)}
              className={cn(
                'flex cursor-pointer items-center justify-between px-3 py-1 font-mono text-note',
                i === active ? 'bg-surface-active text-ink' : 'text-ink-dim hover:bg-surface-hover',
              )}
            >
              <span>{o.tag}</span>
              <span className="text-ink-mute">{o.count}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
```

  If the repo has no eslint config enforcing jsx-a11y, drop the eslint comment.

`TagEditor.tsx`:

```tsx
// A finding's tags, editable: ✕ removes, "+ tag" adds (TagInput). Read-only
// with a reason when the finding's tag log can't be read (decision A).
import { Plus, X } from 'lucide-react';
import { useState } from 'react';
import { apiErrorMessage } from '../../../lib/api';
import { TagInput, type TagSuggestion } from './TagInput';

export function TagEditor({
  tags,
  suggestions,
  disabledReason,
  onAdd,
  onRemove,
}: {
  tags: string[];
  suggestions: TagSuggestion[];
  disabledReason?: string | null;
  onAdd: (tag: string) => Promise<void>;
  onRemove: (tag: string) => Promise<void>;
}) {
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label="Tags">
      {tags.map((t) => (
        <span
          key={t}
          className="inline-flex items-center gap-1 rounded bg-surface px-1.5 py-0.5 font-mono text-note text-ink ring-1 ring-border"
        >
          {t}
          {!disabledReason && (
            <button
              type="button"
              aria-label={`Remove tag ${t}`}
              disabled={busy}
              onClick={() => void run(() => onRemove(t))}
              className="rounded text-ink-mute hover:text-ink disabled:opacity-50"
            >
              <X size={11} />
            </button>
          )}
        </span>
      ))}
      {disabledReason ? (
        <span className="text-note text-ink-mute" title={disabledReason}>
          tags read-only
        </span>
      ) : adding ? (
        <TagInput
          label="Add tag"
          suggestions={suggestions}
          exclude={tags}
          onCancel={() => setAdding(false)}
          onSubmit={(t) =>
            void run(async () => {
              await onAdd(t);
              setAdding(false);
            })
          }
        />
      ) : (
        <button
          type="button"
          disabled={busy}
          onClick={() => setAdding(true)}
          className="inline-flex items-center gap-1 rounded border border-dashed border-border px-1.5 py-0.5 text-note text-ink-mute hover:text-ink"
        >
          <Plus size={11} aria-hidden />
          tag
        </button>
      )}
      {error && (
        <p role="alert" className="w-full text-note text-err">
          {error}
        </p>
      )}
    </div>
  );
}
```

  `apiErrorMessage` of a plain `Error` returns its message, so the test's `new Error('too many tags')` reads as "too many tags".

`TagHistory.tsx`:

```tsx
// A finding's tag changes, newest first: + / − tag, who (agent codename and
// name, or operator and where), when.
import type { TagActor, TagEvent } from '../../../lib/api';

function who(by: TagActor): string {
  if (by.kind === 'operator') return `${by.user} via ${by.via}`;
  const name = by.codename ?? by.agent ?? 'agent';
  return by.codename && by.agent ? `${name} (${by.agent})` : name;
}

export function TagHistory({ events }: { events: TagEvent[] }) {
  if (events.length === 0) return <p className="text-note text-ink-mute">No tag changes yet.</p>;
  const newest = [...events].reverse();
  return (
    <ul className="space-y-1 text-note">
      {newest.map((e) => (
        <li key={e.id} className="flex flex-wrap items-baseline gap-x-2">
          <span className={e.op === 'add' ? 'font-mono text-ok' : 'font-mono text-err'}>
            {e.op === 'add' ? '+' : '−'} {e.tag}
          </span>
          <span className="text-ink-dim">{who(e.by)}</span>
          <time className="text-ink-mute" dateTime={e.at}>
            {new Date(e.at).toLocaleString()}
          </time>
        </li>
      ))}
    </ul>
  );
}
```

  If `text-ok` / `text-err` aren't tokens, use the tokens the repo has (`grep -n "ok\b\|err\b" tailwind.config.ts`).

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run `npx vitest run src/components/findings/tags src/pages/FindingDetail.test.tsx` and `npm run build`.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): tag editor, tag input, tag history components

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Finding page — edit tags and see their history

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/FindingDetail.tsx`
- Test: `crates/rupu-cp/web/src/pages/FindingDetail.test.tsx` (extend; it already spies `api.getFinding`)

**Interfaces:**
- Consumes:
  - `TagEditor`, `TagHistory` and `summarizeTagResult` (Task 2)
  - `api.tagFindings`, `api.getTagsInUse` and `api.getFinding`
  - `FindingDetail.tags`, `.tag_history` and `.tags_editable`
- Produces: tags on the finding page, in both layouts:
  - **Full report:** the editor sits directly under `ReportHeader`. A "Tags" `Section` (id `s-tags`, added to `RAIL` before `s-prov`) holds a `<details>` "Tag history" with `TagHistory`.
  - **No report:** the editor sits under the h1, and the same Section sits before `Provenance`.

- [ ] **Step 1: Write the failing tests** in `FindingDetail.test.tsx`. Use the file's existing render helper and `getFinding` spy, with fixture details that now include `tags`, `tag_history` and `tags_editable`.
  1. A detail with `tags: ['needs-poc']` and `tags_editable: true`:
     - The chip and "Remove tag needs-poc" render.
     - Clicking remove calls `api.tagFindings(['<id>'], { remove: ['needs-poc'] })`, which is spied to resolve `{workspaces:[{ws_id:'ws1', outcomes:[{finding_id:'<id>', before:['needs-poc'], after:[]}]}], unknown:[]}`.
     - `getFinding` is then called again (a refetch).
  2. With `tags_editable: false`, there's no remove button and the text "tags read-only" is shown.
  3. When the `tagFindings` response has a workspace `error`, the editor shows an alert carrying the summary message, and no refetch happens.
  4. The "Tag history" disclosure lists `tag_history` events (one agent add); the actor text renders.
  5. `api.getTagsInUse` is called with `{ wsId: detail.ws_id }`, and its tags feed the editor's suggestions.

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `npx vitest run src/pages/FindingDetail.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.** Add a `FindingTags` component inside `FindingDetail.tsx`:

```tsx
/** The editable tag row plus the suggestions it needs. Changes go through
 *  `POST /api/findings/tags`; any per-workspace error or unknown id is a
 *  thrown error the editor shows, otherwise the page refetches the finding. */
function FindingTags({ detail, onChanged }: { detail: Detail; onChanged: () => Promise<void> }) {
  const [suggestions, setSuggestions] = useState<TagSuggestion[]>([]);
  useEffect(() => {
    let live = true;
    api.getTagsInUse({ wsId: detail.ws_id }).then(
      (t) => { if (live) setSuggestions(t); },
      () => { /* suggestions are optional */ },
    );
    return () => { live = false; };
  }, [detail.ws_id]);
  const change = async (mode: 'add' | 'remove', tag: string) => {
    const r = await api.tagFindings([detail.id], mode === 'add' ? { add: [tag] } : { remove: [tag] });
    const s = summarizeTagResult(r, mode, () => detail.project || detail.ws_id);
    if (!s.ok) throw new Error(s.message);
    await onChanged();
  };
  return (
    <TagEditor
      tags={detail.tags ?? []}
      suggestions={suggestions}
      disabledReason={detail.tags_editable ? null : "This project's tag log couldn't be read, so its tags can't be changed here."}
      onAdd={(t) => change('add', t)}
      onRemove={(t) => change('remove', t)}
    />
  );
}

function TagsSection({ detail }: { detail: Detail }) {
  return (
    <Section id="s-tags" title="Tags">
      <details>
        <summary className="cursor-pointer text-ui text-ink-dim">Tag history ({detail.tag_history.length})</summary>
        <div className="mt-2">
          <TagHistory events={detail.tag_history} />
        </div>
      </details>
    </Section>
  );
}
```

  Then wire it into the page:
  - In `FindingDetail()`, add `const refetch = async () => setDetail(await api.getFinding(id));`.
  - Render `<FindingTags detail={detail} onChanged={refetch} />` under the h1 in the no-report branch, and right after `<ReportHeader … />` in the report branch.
  - Render `<TagsSection detail={detail} />` before `<Provenance …/>` in both branches.
  - Add `['s-tags', 'Tags']` to `RAIL` before `['s-prov', 'Provenance']`.
  - Import `TagEditor`, `TagHistory`, `summarizeTagResult` and `type TagSuggestion`.
  - If `Section` requires props this snippet lacks, follow its real signature.

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run `npx vitest run src/pages/FindingDetail.test.tsx src/components/findings` and `npm run build`.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): edit a finding's tags and see their history on its page

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Bulk tagging — row selection in SortableTable + BulkTagBar on Findings

**Files:**
- Modify: `crates/rupu-cp/web/src/components/lists/SortableTable.tsx` (optional `selection` prop)
- Modify: `crates/rupu-cp/web/src/components/findings/FindingsTable.tsx` (pass `selection` through)
- Create: `crates/rupu-cp/web/src/components/findings/BulkTagBar.tsx`
- Modify: `crates/rupu-cp/web/src/pages/Findings.tsx`
- Tests:
  - `crates/rupu-cp/web/src/components/lists/SortableTable.selection.test.tsx` (new)
  - `crates/rupu-cp/web/src/components/findings/BulkTagBar.test.tsx` (new)
  - `crates/rupu-cp/web/src/pages/Findings.test.tsx` (extend)

**Interfaces:**
- Produces:

```ts
export interface RowSelection<T> {
  isSelected: (row: T) => boolean;
  /** null = selectable; a string = why not (disabled checkbox title). */
  blockedReason?: (row: T) => string | null;
  label: (row: T) => string;
  onToggle: (row: T) => void;
  /** Header checkbox: select or clear every selectable visible row. */
  onToggleAll: (rows: T[], selected: boolean) => void;
}
// SortableTable prop: selection?: RowSelection<T>
// FindingsTable prop: selection?: RowSelection<FindingRecord>
// BulkTagBar({ count, suggestions, onApply(mode, tag): Promise<{message: string; ok: boolean}>, onClear })
```

- [ ] **Step 1: Write the failing tests.**

`SortableTable.selection.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SortableTable, { type Column } from './SortableTable';

afterEach(cleanup);
type Row = { id: string; locked?: boolean };
const cols: Column<Row>[] = [{ key: 'id', header: 'Id', subject: true, render: (r) => r.id }];
const rows: Row[] = [{ id: 'a' }, { id: 'b' }, { id: 'c', locked: true }];

function setup(selected: Set<string>) {
  const onToggle = vi.fn();
  const onToggleAll = vi.fn();
  render(
    <MemoryRouter>
      <SortableTable
        columns={cols}
        rows={rows}
        rowKey={(r) => r.id}
        selection={{
          isSelected: (r) => selected.has(r.id),
          blockedReason: (r) => (r.locked ? 'locked' : null),
          label: (r) => `Select ${r.id}`,
          onToggle,
          onToggleAll,
        }}
      />
    </MemoryRouter>,
  );
  return { onToggle, onToggleAll };
}

describe('SortableTable selection', () => {
  it('renders a checkbox per row, disabled with a reason when blocked', () => {
    const { onToggle } = setup(new Set(['a']));
    expect(screen.getByRole('checkbox', { name: 'Select a' })).toBeChecked();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select b' }));
    expect(onToggle).toHaveBeenCalledWith(rows[1]);
    const locked = screen.getByRole('checkbox', { name: 'Select c' });
    expect(locked).toBeDisabled();
    expect(locked).toHaveAttribute('title', 'locked');
  });
  it('header checkbox selects every selectable row and is indeterminate when some are selected', () => {
    const { onToggleAll } = setup(new Set(['a']));
    const all = screen.getByRole('checkbox', { name: 'Select all' }) as HTMLInputElement;
    expect(all.indeterminate).toBe(true);
    fireEvent.click(all);
    expect(onToggleAll).toHaveBeenCalledWith([rows[0], rows[1]], true);
  });
  it('no selection prop renders no checkboxes', () => {
    render(
      <MemoryRouter>
        <SortableTable columns={cols} rows={rows} rowKey={(r) => r.id} />
      </MemoryRouter>,
    );
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
});
```

`BulkTagBar.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { BulkTagBar } from './BulkTagBar';

afterEach(cleanup);

describe('BulkTagBar', () => {
  it('tags the selection with a chosen tag and shows the result', async () => {
    const onApply = vi.fn().mockResolvedValue({ message: 'Tagged 2 findings.', ok: true });
    render(<BulkTagBar count={2} suggestions={[{ tag: 'needs-poc', count: 3 }]} onApply={onApply} onClear={vi.fn()} />);
    expect(screen.getByText('2 selected')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Tag…' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Tag selected findings' }), { target: { value: 'needs-poc' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Tag selected findings' }), { key: 'Enter' });
    expect(await screen.findByRole('status')).toHaveTextContent('Tagged 2 findings.');
    expect(onApply).toHaveBeenCalledWith('add', 'needs-poc');
  });
  it('untag mode and clear', async () => {
    const onApply = vi.fn().mockResolvedValue({ message: "Untagged 1 finding. billing-api's tags couldn't be changed: x.", ok: false });
    const onClear = vi.fn();
    render(<BulkTagBar count={3} suggestions={[]} onApply={onApply} onClear={onClear} />);
    fireEvent.click(screen.getByRole('button', { name: 'Untag…' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Untag selected findings' }), { target: { value: 'x' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Untag selected findings' }), { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent("billing-api's tags couldn't be changed");
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onClear).toHaveBeenCalled();
  });
});
```

  Extend `Findings.test.tsx`, keeping its existing spies:
  1. With a response of two findings, the table shows checkboxes. Selecting one shows "1 selected". Choosing "Tag…", typing `triaged`, then Enter calls `api.tagFindings(['<id>'], { add: ['triaged'] })`, which is spied to resolve an outcome. Then:
     - `getFindings` is called again, which is the reload.
     - the bar disappears, since the selection clears.
     - the result message remains visible as a `role="status"` line above the table.
  2. A finding whose `ws_id` is in `tags_unavailable` has a disabled checkbox.
  3. Changing `q` (a tile click) clears the selection.

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `npx vitest run src/components/lists/SortableTable.selection.test.tsx src/components/findings/BulkTagBar.test.tsx src/pages/Findings.test.tsx`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**
  - **`SortableTable`.** Add `selection?: RowSelection<T>` to the props and export the `RowSelection` interface. When it is present:
    - Render a leading `<th className="w-8 pl-3">`. It holds a header checkbox (`aria-label="Select all"`), whose state is:
      - `checked` when every selectable visible row is selected;
      - `indeterminate` (set through a ref in an effect) when some but not all are selected.
    - Clicking the header checkbox calls `onToggleAll(selectableVisibleRows, !allSelected)`.
    - Each row gets a leading `<td className="w-8 pl-3">` with `<input type="checkbox" aria-label={label(row)} checked={isSelected(row)} disabled={reason !== null} title={reason ?? undefined} onChange={() => onToggle(row)} onClick={(e) => e.stopPropagation()} />`.
    - That checkbox cell goes BEFORE the expand-chevron cell.
    - Increase `totalCols` by one so the detail row's `colSpan` stays right.
    - "Visible rows" means the `sorted` rows.
  - **`FindingsTable`.** Add `selection?: RowSelection<FindingRecord>` and pass it through.
  - **`BulkTagBar.tsx`:**

```tsx
// Shown while findings are selected: "N selected · Tag… · Untag… · Clear".
// Tag…/Untag… open a TagInput; the result line stays until the next action.
import { useState } from 'react';
import { TagInput, type TagSuggestion } from './tags/TagInput';

export function BulkTagBar({
  count,
  suggestions,
  onApply,
  onClear,
}: {
  count: number;
  suggestions: TagSuggestion[];
  onApply: (mode: 'add' | 'remove', tag: string) => Promise<{ message: string; ok: boolean }>;
  onClear: () => void;
}) {
  const [mode, setMode] = useState<'add' | 'remove' | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ message: string; ok: boolean } | null>(null);

  const apply = async (tag: string) => {
    if (!mode) return;
    setBusy(true);
    try {
      setResult(await onApply(mode, tag));
      setMode(null);
    } catch (e: unknown) {
      setResult({ message: e instanceof Error ? e.message : String(e), ok: false });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-1">
      <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface px-3 py-2 text-ui">
        <span className="font-medium text-ink">{count} selected</span>
        <span className="flex-1" />
        {mode ? (
          <TagInput
            label={mode === 'add' ? 'Tag selected findings' : 'Untag selected findings'}
            suggestions={suggestions}
            onSubmit={(t) => void apply(t)}
            onCancel={() => setMode(null)}
          />
        ) : (
          <>
            <button type="button" disabled={busy} onClick={() => setMode('add')} className="rounded border border-border px-2 py-0.5 hover:bg-surface-hover">
              Tag…
            </button>
            <button type="button" disabled={busy} onClick={() => setMode('remove')} className="rounded border border-border px-2 py-0.5 hover:bg-surface-hover">
              Untag…
            </button>
          </>
        )}
        <button type="button" onClick={onClear} className="rounded px-2 py-0.5 text-ink-mute hover:text-ink">
          Clear
        </button>
      </div>
      {result && (
        <p role={result.ok ? 'status' : 'alert'} className={result.ok ? 'text-note text-ink-dim' : 'text-note text-err'}>
          {result.message}
        </p>
      )}
    </div>
  );
}
```

  - **`Findings.tsx`.** Use selection keyed `ws_id/target_id/id`.
    - Add `const [selected, setSelected] = useState<ReadonlySet<string>>(new Set())`.
    - Add `const [reload, setReload] = useState(0)` and include `reload` in the fetch effect's deps.
    - Add `const [bulkNote, setBulkNote] = useState<{message: string; ok: boolean} | null>(null)`.
    - Clear the selection and `bulkNote` when `q` changes: `useEffect(() => { setSelected(new Set()); setBulkNote(null); }, [q])`.
    - Build `selection` from `data`:

```tsx
  const rowKey = (f: FindingRecord) => {
    const o = f as FindingOut;
    return `${o.ws_id}/${o.target_id}/${o.id}`;
  };
  const unavailable = new Set((data?.tags_unavailable ?? []).map((w) => w.ws_id));
  const selection: RowSelection<FindingRecord> = {
    isSelected: (f) => selected.has(rowKey(f)),
    blockedReason: (f) => {
      const o = f as FindingOut;
      return unavailable.has(o.ws_id) ? `${o.project || o.ws_id}'s tags couldn't be read, so they can't be changed` : null;
    },
    label: (f) => `Select ${f.id}`,
    onToggle: (f) =>
      setSelected((prev) => {
        const next = new Set(prev);
        const k = rowKey(f);
        if (next.has(k)) next.delete(k);
        else next.add(k);
        return next;
      }),
    onToggleAll: (rows, on) =>
      setSelected((prev) => {
        const next = new Set(prev);
        for (const r of rows) {
          if (on) next.add(rowKey(r));
          else next.delete(rowKey(r));
        }
        return next;
      }),
  };
  const selectedRows = (data?.findings ?? []).filter((f) => selected.has(rowKey(f)));
  const projectOf = (wsId: string) => (data?.findings ?? []).find((f) => f.ws_id === wsId)?.project ?? wsId;
  const applyBulk = async (mode: 'add' | 'remove', tag: string) => {
    const ids = [...new Set(selectedRows.map((f) => f.id))];
    const r = await api.tagFindings(ids, mode === 'add' ? { add: [tag] } : { remove: [tag] });
    const summary = summarizeTagResult(r, mode, projectOf);
    setBulkNote(summary);
    setSelected(new Set());
    setReload((n) => n + 1);
    return summary;
  };
```

    - **Bulk bar:** render it between the banner and the table when `selected.size > 0 && data`: `<BulkTagBar count={selected.size} suggestions={(data.facets?.tag ?? []).map((v) => ({ tag: v.value, count: v.count }))} onApply={applyBulk} onClear={() => setSelected(new Set())} />`.
    - **Result note:** when there is no selection but `bulkNote` is set, render it as `role={bulkNote.ok ? 'status' : 'alert'}`. The bar unmounts after the selection clears, so this keeps the result visible.
    - **Table:** pass `selection` to `<FindingsTable … selection={selection} />`.
    - `projectOf` falls back to the ws id, but `tags_unavailable` now carries `project`. Prefer it: `data?.tags_unavailable.find((w) => w.ws_id === wsId)?.project ?? …`.

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run `npx vitest run src/components/lists src/components/findings src/pages` and `npm run build`.
  - Other `SortableTable` users must be unaffected, because the prop is optional. Run `npx vitest run src` once to confirm.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): select findings and tag or untag them in bulk

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Plan 2 polish — QueryBar accessibility + suggestion de-duplication

**Files:**
- Modify: `crates/rupu-cp/web/src/components/query/QueryBar.tsx` and `QueryBar.test.tsx`
- Modify: `crates/rupu-cp/web/src/components/query/suggest.ts` and `suggest.test.ts`

**Interfaces:** no API changes.

- [ ] **Step 1: Write the failing tests.**
  - **QueryBar.** Once open, the combobox carries:
    - `aria-autocomplete="list"`
    - `aria-haspopup="listbox"`
    - an `aria-describedby` pointing at the alert while an invalid draft is shown

    The listbox has `aria-label="Suggestions"`.
  - **suggest.**
    - `suggest('has:tags,', FINDING_FIELDS, facets)` doesn't offer `tags` again. Values already in the token's prefix are excluded.
    - A facet value `''` is never offered.

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `npx vitest run src/components/query`
  - Expected: FAIL.

- [ ] **Step 3: Implement.**
  - **QueryBar:** add the attributes. Give the alert `id={`${listId}-error`}` and set `aria-describedby` on the input only while `error` is set.
  - **suggest:** in the value branch, compute the values already in the prefix (split it on unquoted commas with the same walk as `splitItem`, then unquote), and filter them and `''` out of `pool`.

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run `npx vitest run src/components/query src/pages/Findings.test.tsx` and `npm run build`.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cp/web/src/components/query
git commit -m "fix(cp-web): query bar accessibility; suggestions skip chosen and empty values

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Docs

**Files:**
- Modify: `docs/coverage.md`
  - In "Tagging findings", add the CP: the finding page editor and history, bulk select in the Findings table, and read-only behavior for unreadable tag logs.
  - Document `POST /api/findings/tags` and `GET /api/findings/tags`, plus the detail fields `tag_history` and `tags_editable`.
- Modify: `CLAUDE.md`, the `rupu-cp` entry. Add one clause: `POST /api/findings/tags` (operator-attributed `via: cp`, through `tag_findings_across`), `GET /api/findings/tags`, and detail `tag_history`/`tags_editable`.
- Modify: the spec. Mark Plan 3 complete, with this plan's path, in the Plans lists.

- [ ] **Step 1: Write it.** Verify every claim against the code. Use invented examples.
- [ ] **Step 2: Commit.**

```bash
git add docs/coverage.md CLAUDE.md docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md
git commit -m "docs: tag editing in the CP

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Final verification (after Task 6)

- [ ] `cargo check --workspace --all-targets` and `cargo clippy -p rupu-cp --all-targets -- -D warnings` are clean.
- [ ] `cargo test -p rupu-cp --lib api::findings` passes. So does `cargo test -p rupu-cp --test it finding_artifacts::`.
- [ ] `cd crates/rupu-cp/web && npx vitest run src && npm run build` passes.
- [ ] Visual check by the controller in the browser pane against a scratch control plane with invented findings, before the PR merges:
  - [ ] the finding page tag editor and history
  - [ ] selecting rows
  - [ ] bulk Tag… and Untag…
  - [ ] the result line
  - [ ] a disabled checkbox for an unreadable workspace
