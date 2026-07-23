// @vitest-environment jsdom
// InputsCard — renders only under `workflowEditorUi === 'next'` (no classic
// gating needed inside the component itself). Task 5 covers only the
// Description field's move from a single-line input to a roomier textarea.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import InputsCard from './InputsCard';

afterEach(cleanup);

describe('InputsCard — roomier description field (Task 5)', () => {
  it('the Description field is a multi-row, vertically-resizable textarea', () => {
    render(<InputsCard rest={{ inputs: { repo: { type: 'string' } } }} onRest={() => {}} />);
    const field = screen.getByLabelText('Input 1 description');
    expect(field.tagName).toBe('TEXTAREA');
    expect(field).toHaveAttribute('rows', '4');
    expect(field.className).toContain('resize-y');
  });

  it('editing the description still round-trips through onRest', () => {
    const spy = vi.fn();
    render(<InputsCard rest={{ inputs: { repo: { type: 'string' } } }} onRest={spy} />);
    fireEvent.change(screen.getByLabelText('Input 1 description'), { target: { value: 'the repo to clone' } });
    const [rest] = spy.mock.calls[spy.mock.calls.length - 1];
    expect((rest.inputs as Record<string, { description?: string }>).repo.description).toBe('the repo to clone');
  });
});
