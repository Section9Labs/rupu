// @vitest-environment jsdom
// ExpressionField — a light render test. The CodeMirror body is lazy-loaded, so
// in a synchronous test render the Suspense FALLBACK (a styled input/textarea
// with the same value/onChange API) is what mounts — which is exactly what we
// assert here: the field shows its value and edits flow through onChange.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import ExpressionField from './ExpressionField';
import type { ExprContext } from '../../lib/workflowExpressions';

// CodeMirror's completion tooltip in jsdom is too limited to reliably open
// and inspect where it mounted (Suspense fallback replaces the real editor in
// synchronous test renders anyway — see below). Instead we spy on
// `tooltips()` itself and assert `buildTooltipExtensions()` — the small
// exported builder ExpressionFieldImpl wires into its extensions array —
// calls it with the body-parented, viewport-fixed config that escapes the
// inspector rail's `overflow-y-auto` clipping.
// NOTE: `./ExpressionFieldImpl` is intentionally NOT statically imported here
// (even though the tooltip test below needs it) — the fallback/size-prop
// tests above rely on it being unresolved at render time so the Suspense
// FALLBACK mounts synchronously (per the file-header comment). Importing it
// eagerly at module scope pre-warms the module cache and makes React.lazy's
// dynamic import resolve before the assertions run, mounting the REAL
// CodeMirror editor instead and breaking those tests. It's dynamically
// imported instead, inside the one test that needs it, after every other
// test in this file has already run.
vi.mock('@codemirror/view', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@codemirror/view')>();
  return { ...actual, tooltips: vi.fn(actual.tooltips) };
});

const CTX: ExprContext = {
  nodeKind: 'step',
  isForEachPrompt: false,
  isPanelField: false,
  inputNames: [],
  priorSteps: [],
};

afterEach(cleanup);

describe('ExpressionField (fallback)', () => {
  it('renders the value and emits edits via onChange (multiline → textarea)', () => {
    const spy = vi.fn();
    render(
      <ExpressionField value="hello {{ inputs.x }}" onChange={spy} context={CTX} multiline ariaLabel="Prompt" />,
    );
    const field = screen.getByLabelText('Prompt') as HTMLTextAreaElement;
    expect(field.value).toBe('hello {{ inputs.x }}');
    fireEvent.change(field, { target: { value: 'changed' } });
    expect(spy).toHaveBeenCalledWith('changed');
  });

  it('renders a single-line input when not multiline', () => {
    render(<ExpressionField value="x" onChange={() => {}} context={CTX} ariaLabel="When" />);
    const field = screen.getByLabelText('When');
    expect(field.tagName).toBe('INPUT');
  });
});

describe('ExpressionField size prop (Task 5, roomier long-text)', () => {
  it('defaults to the compact shell — no .wfx-ta-lg marker', () => {
    const { container } = render(
      <ExpressionField value="" onChange={() => {}} context={CTX} multiline ariaLabel="Prompt" />,
    );
    expect(container.querySelector('.wfx-ta-lg')).not.toBeInTheDocument();
  });

  it('size="large" adds the .wfx-ta-lg marker to the field shell', () => {
    const { container } = render(
      <ExpressionField value="" onChange={() => {}} context={CTX} multiline ariaLabel="Prompt" size="large" />,
    );
    expect(container.querySelector('.wfx-ta-lg')).toBeInTheDocument();
  });

  it('size="large" also gives the Suspense-fallback textarea more rows', () => {
    render(
      <ExpressionField value="" onChange={() => {}} context={CTX} multiline ariaLabel="Prompt" size="large" />,
    );
    expect(screen.getByLabelText('Prompt')).toHaveAttribute('rows', '8');
  });
});

describe('ExpressionFieldImpl tooltip config (Task 3, popup clipping fix)', () => {
  it('configures a body-parented, viewport-fixed tooltip so the completion popup escapes the rail\'s overflow-y-auto clipping', async () => {
    const { buildTooltipExtensions } = await import('./ExpressionFieldImpl');
    const { tooltips } = await import('@codemirror/view');
    vi.mocked(tooltips).mockClear();

    buildTooltipExtensions();
    expect(tooltips).toHaveBeenCalledWith({ position: 'fixed', parent: document.body });
  });

  it('is SSR/jsdom-safe: no-ops when document is unavailable', async () => {
    const { buildTooltipExtensions } = await import('./ExpressionFieldImpl');
    const { tooltips } = await import('@codemirror/view');
    vi.mocked(tooltips).mockClear();

    vi.stubGlobal('document', undefined);
    try {
      expect(buildTooltipExtensions()).toEqual([]);
      expect(tooltips).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
