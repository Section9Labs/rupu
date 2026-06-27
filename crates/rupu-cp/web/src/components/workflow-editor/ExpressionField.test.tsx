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
