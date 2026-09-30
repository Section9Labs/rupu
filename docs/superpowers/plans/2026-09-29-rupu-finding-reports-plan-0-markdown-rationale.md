# Finding reports — Plan 0: render finding rationale as markdown

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status:** shipped as Section9Labs/rupu#674. The code below is what landed (review added the compact descendant-variant styling, its test, and a `FindingRow.evidence.test.tsx` expand test).

**Goal:** The web CP's findings tables render `evidence.rationale` as markdown instead of a plain-text wall, using the renderer the transcript and Code-tab cards already use.

**Architecture:** `FindingEvidence.tsx` is the shared evidence panel, and `FindingRow.tsx` has a hand-copied duplicate of its body. Switch `FindingEvidence`'s rationale to `components/transcript/Markdown.tsx`, then delete `FindingRow`'s duplicate and render `<FindingEvidence>` in its place. `InlineFindingCard.tsx` and the transcript `FindingCard.tsx` already use `Markdown`, so they are untouched. macOS's findings table doesn't display the rationale at all, so there's nothing to change there.

**Tech Stack:** React 18 + TypeScript, react-markdown + rehype-highlight (already dependencies), vitest + @testing-library/react.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` (§ Rendering → Web → "Quick fix")

## Global Constraints

- No new npm dependencies.
- Web tests run with `cd crates/rupu-cp/web && npx vitest run <path>`.
- GUI validation rule: matt checks the Findings page in the browser before merge.
- Every change goes through a feature branch + PR, never straight to `main`.

---

### Task 1: FindingEvidence renders rationale as markdown; FindingRow reuses it

**Files:**
- Modify: `crates/rupu-cp/web/src/components/findings/FindingEvidence.tsx`
- Modify: `crates/rupu-cp/web/src/components/findings/FindingRow.tsx:118-139` (the `{hasEvidence && open && (...)}` block)
- Create: `crates/rupu-cp/web/src/components/findings/FindingEvidence.test.tsx`

**Interfaces:**
- Consumes: `Markdown` default export from `components/transcript/Markdown.tsx`, prop `text: string`.
- Produces: `FindingEvidence({ finding }: { finding: FindingRecord })`, same signature as today.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/findings/FindingEvidence.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { FindingEvidence } from './FindingEvidence';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

function finding(rationale: string): FindingRecord {
  return {
    id: 'f1',
    severity: 'high',
    summary: 's',
    evidence: { rationale, references: [] },
  } as unknown as FindingRecord;
}

describe('FindingEvidence', () => {
  it('renders inline code in the rationale as <code>, not literal backticks', () => {
    render(<FindingEvidence finding={finding('the `parseToken` call trusts input')} />);
    const code = screen.getByText('parseToken');
    expect(code.tagName).toBe('CODE');
    expect(screen.queryByText(/`parseToken`/)).toBeNull();
  });

  it('renders blank-line-separated paragraphs as separate <p> elements', () => {
    const { container } = render(
      <FindingEvidence finding={finding('First paragraph.\n\nSecond paragraph.')} />,
    );
    const paras = Array.from(container.querySelectorAll('p')).map((p) => p.textContent);
    expect(paras).toContain('First paragraph.');
    expect(paras).toContain('Second paragraph.');
  });

  it('renders a fenced block as a <pre>', () => {
    const { container } = render(
      <FindingEvidence finding={finding('Build:\n\n```sh\ncargo build --release\n```')} />,
    );
    expect(container.querySelector('pre')).not.toBeNull();
  });

  it('keeps the rationale compact and dim via wrapper descendant variants', () => {
    const { container } = render(
      <FindingEvidence finding={finding('First paragraph.\n\n- item one\n- item two')} />,
    );
    const p = container.querySelector('p');
    expect(p).not.toBeNull();
    // Markdown renders inside its own `.prose-rupu` div; the styling wrapper is
    // that div's parent.
    const wrapper = p!.closest('.prose-rupu')?.parentElement;
    expect(wrapper).toBeTruthy();
    expect(wrapper!.className).toContain('[&_p]:text-ink-dim');
    expect(wrapper!.className).toContain('[&_p]:text-ui');
    expect(wrapper!.className).toContain('[&_p]:leading-snug');
    expect(wrapper!.className).toContain('[&_li]:text-ink-dim');
    expect(wrapper!.className).toContain('[&_li]:text-ui');
    expect(container.querySelector('li')).not.toBeNull();
  });

  it('keeps the empty-state message when there is no evidence', () => {
    render(<FindingEvidence finding={finding('')} />);
    expect(screen.getByText('No evidence recorded.')).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the test and check that it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/FindingEvidence.test.tsx`
Expected: FAIL. The first test finds the literal backticked `` `parseToken` `` text (the rationale renders in a `<p>`), the paragraph test gets a single `<p>` containing both paragraphs, and the styling test finds no descendant-variant classes on the wrapper.

- [ ] **Step 3: Switch FindingEvidence to Markdown**

In `crates/rupu-cp/web/src/components/findings/FindingEvidence.tsx`, add the import after the existing `api` import:

```tsx
import Markdown from '../transcript/Markdown';
```

Replace:

```tsx
      {rationale && (
        <p className="text-ui text-ink-dim leading-snug whitespace-pre-wrap">{rationale}</p>
      )}
```

with:

```tsx
      {rationale && (
        <div className="[&_p]:text-ui [&_p]:text-ink-dim [&_p]:leading-snug [&_li]:text-ui [&_li]:text-ink-dim [&_li]:leading-snug">
          <Markdown text={rationale} />
        </div>
      )}
```

Update the file's header comment. Its first line, `// Evidence panel for a finding — rationale / code excerpt / references. Lifted`, stays. Add this line after the existing comment block:

```tsx
// The rationale is agent-written markdown (backticks, paragraphs, fenced
// blocks), rendered through the transcript's Markdown component — the same
// convention InlineFindingCard and the transcript FindingCard already use.
```

- [ ] **Step 4: Run the test and check that it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/FindingEvidence.test.tsx`
Expected: PASS (4 tests).

- [ ] **Step 5: Replace FindingRow's duplicate body with FindingEvidence**

In `crates/rupu-cp/web/src/components/findings/FindingRow.tsx`, add the import:

```tsx
import { FindingEvidence } from './FindingEvidence';
```

Replace the whole block:

```tsx
          {hasEvidence && open && (
            <div className="mt-2 space-y-2">
              {rationale && (
                <p className="text-ui text-ink-dim leading-snug whitespace-pre-wrap">
                  {rationale}
                </p>
              )}
              {excerpt && (
                <pre className="overflow-x-auto rounded bg-surface ring-1 ring-border px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">
                  {excerpt}
                </pre>
              )}
              {references.length > 0 && (
                <ul className="list-disc pl-4 text-note text-ink-mute space-y-0.5">
                  {references.map((ref, i) => (
                    <li key={i} className="break-all font-mono">{ref}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
```

with:

```tsx
          {hasEvidence && open && (
            <div className="mt-2">
              <FindingEvidence finding={finding} />
            </div>
          )}
```

`rationale`, `excerpt`, and `references` are still read at the top of the component (lines ~37-40) to compute `hasEvidence`. Keep those lines. If TypeScript or ESLint then flags `excerpt` or `references` as unused, inline them into the `hasEvidence` expression:

```tsx
  const hasEvidence = Boolean(
    finding.evidence?.rationale ||
      finding.evidence?.code_excerpt ||
      (finding.evidence?.references ?? []).length > 0,
  );
```

and delete the three now-unused `const`s.

- [ ] **Step 6: Run the findings component tests and the type check**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings && npx tsc --noEmit -p .`
Expected: all findings tests PASS, including the existing `FindingRow.deeplink.test.tsx` and `FindingsTable.deeplink.test.tsx`; `tsc` exits 0.

- [ ] **Step 7: Build the web bundle**

Run: `make cp-web`
Expected: the vite build succeeds. The markdown chunk (`manualChunks.markdown` in `vite.config.ts`) now also loads from the findings pages, which is expected.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-cp/web/src/components/findings/FindingEvidence.tsx \
        crates/rupu-cp/web/src/components/findings/FindingEvidence.test.tsx \
        crates/rupu-cp/web/src/components/findings/FindingRow.tsx
git commit -m "fix(cp-web): render finding rationale as markdown in findings tables

FindingEvidence printed evidence.rationale in a whitespace-pre-wrap <p>, so
agent-written markdown (inline code, paragraphs, fenced blocks) showed as a
wall of literal text. Render it through the transcript Markdown component,
matching InlineFindingCard and the transcript FindingCard, and drop
FindingRow's hand-copied duplicate of the evidence panel.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 9: Hand off for GUI check**

Ask matt to open a run's Findings tab and the global Findings page in the browser, expand a finding with a long rationale, and confirm that the code spans, paragraphs, and fenced blocks render. Do not merge before that.
