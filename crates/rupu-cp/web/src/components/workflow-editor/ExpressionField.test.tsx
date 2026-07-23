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
