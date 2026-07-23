// @vitest-environment jsdom
// CodeEditorImpl — tooltip-escape config coverage (mirrors
// `ExpressionField.test.tsx`'s "tooltip config" block). CodeEditorImpl's
// markdown mode registers `autocompletion()` (see `baseExtensions`) and
// `@codemirror/lang-markdown` supplies a real HTML-tag completion source, so
// the same popup-clipping bug ExpressionFieldImpl had applies here too (agent
// `.md` editors in AgentDetail.tsx / Agents.tsx sit in `overflow` containers).
// We spy on `tooltips()` itself (as the ExpressionField test does) and assert
// the shared `buildTooltipExtensions()` builder — now in `./cmTooltips` — is
// wired into CodeEditorImpl's extension set by mounting the real editor
// (no Suspense/lazy boundary here, so no module-cache ordering hazard) and
// checking `tooltips` was invoked with the body-parented, viewport-fixed
// config.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, cleanup } from '@testing-library/react';

vi.mock('@codemirror/view', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@codemirror/view')>();
  return { ...actual, tooltips: vi.fn(actual.tooltips) };
});

afterEach(cleanup);

describe('CodeEditorImpl tooltip config (shared with ExpressionFieldImpl)', () => {
  it('mounts with the shared body-parented, viewport-fixed tooltip extension', async () => {
    const { tooltips } = await import('@codemirror/view');
    vi.mocked(tooltips).mockClear();
    const CodeEditorImpl = (await import('./CodeEditorImpl')).default;

    render(<CodeEditorImpl value="# hi" onChange={() => {}} language="markdown" ariaLabel="Body" />);

    expect(tooltips).toHaveBeenCalledWith({ position: 'fixed', parent: document.body });
  });

  it('the shared builder is the one from ./cmTooltips (same module ExpressionFieldImpl re-exports)', async () => {
    const { buildTooltipExtensions: fromShared } = await import('./cmTooltips');
    const { buildTooltipExtensions: fromExpression } = await import('./workflow-editor/ExpressionFieldImpl');
    expect(fromExpression).toBe(fromShared);
  });
});
