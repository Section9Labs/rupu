// @vitest-environment jsdom
// AutoflowCard — Task 1 adds editable `source`/`priority` controls. Both
// fields already round-trip through readAutoflow/writeAutoflow (see
// lib/workflowMeta.ts); this only covers the new form controls binding to them.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import AutoflowCard from './AutoflowCard';

afterEach(cleanup);

describe('AutoflowCard — source/priority controls (Task 1)', () => {
  it('renders source and priority controls bound to the model', () => {
    const spy = vi.fn();
    render(
      <AutoflowCard
        rest={{ autoflow: { enabled: true, entity: 'issue', source: 'github', priority: 5 } }}
        onRest={spy}
      />,
    );

    expect(screen.getByLabelText('Autoflow source')).toHaveValue('github');
    expect(screen.getByLabelText('Autoflow priority')).toHaveValue(5);
  });

  it('editing source flows through commit() to model.source', () => {
    const spy = vi.fn();
    render(
      <AutoflowCard rest={{ autoflow: { enabled: true, entity: 'issue' } }} onRest={spy} />,
    );
    fireEvent.change(screen.getByLabelText('Autoflow source'), { target: { value: 'gitlab' } });
    const rest = spy.mock.calls[spy.mock.calls.length - 1][0] as Record<string, unknown>;
    expect((rest.autoflow as Record<string, unknown>).source).toBe('gitlab');
  });

  it('editing priority flows through commit() to model.priority', () => {
    const spy = vi.fn();
    render(
      <AutoflowCard rest={{ autoflow: { enabled: true, entity: 'issue' } }} onRest={spy} />,
    );
    fireEvent.change(screen.getByLabelText('Autoflow priority'), { target: { value: '9' } });
    const rest = spy.mock.calls[spy.mock.calls.length - 1][0] as Record<string, unknown>;
    expect((rest.autoflow as Record<string, unknown>).priority).toBe(9);
  });

  it('does not render the controls (or any body) when Autoflow is disabled', () => {
    render(<AutoflowCard rest={{}} onRest={() => {}} />);
    expect(screen.queryByLabelText('Autoflow source')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Autoflow priority')).not.toBeInTheDocument();
  });
});
