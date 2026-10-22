// @vitest-environment jsdom
// RunEventFeed: one run's events on the shared event rail. Run-level cards (no
// codename of their own) take the run's crew from `crewByRun`, so their rows
// draw the run's thread in its tint; a warning is a quiet amber row.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import RunEventFeed, { type SeqEvent } from './RunEventFeed';
import { crewTint } from '../lib/codename';

afterEach(cleanup);

const events: SeqEvent[] = [
  { seq: 1, event: { type: 'run_started', run_id: 'run-1', workflow_path: '/w/nightly.yaml' } as SeqEvent['event'] },
  { seq: 2, event: { type: 'run_completed', run_id: 'run-1', status: 'completed' } as SeqEvent['event'] },
];

describe('RunEventFeed — crewByRun', () => {
  it('threads run-level rows in the run crew tint', () => {
    const { container } = render(
      <MemoryRouter>
        <RunEventFeed events={events} connection="live" crewByRun={new Map([['run-1', 'cobalt-harbor']])} />
      </MemoryRouter>,
    );
    const rows = [...container.querySelectorAll<HTMLElement>('[data-evkey]')];
    expect(rows).toHaveLength(2);
    expect(rows[0].style.getPropertyValue('--crew')).toBe(crewTint('cobalt-harbor', 'light'));
    // newest first: the completion is joined downward to the start
    expect(rows[0]).toHaveAttribute('data-th-dn');
    expect(rows[1]).toHaveAttribute('data-th-up');
  });

  it('no tint without the map', () => {
    const { container } = render(
      <MemoryRouter>
        <RunEventFeed events={events} connection="live" />
      </MemoryRouter>,
    );
    for (const row of container.querySelectorAll<HTMLElement>('[data-evkey]')) {
      expect(row.style.getPropertyValue('--crew')).toBe('');
    }
  });
});

describe('RunEventFeed — step_warning', () => {
  const warning: SeqEvent[] = [
    { seq: 1, event: { type: 'step_started', run_id: 'run-1', step_id: 'sweep', kind: 'for_each' } as SeqEvent['event'] },
    {
      seq: 2,
      event: { type: 'step_warning', run_id: 'run-1', step_id: 'sweep', index: 1, message: 'host gpu-9 sent no coverage stream' } as SeqEvent['event'],
    },
  ];

  it('renders a warning row with its message, beside the normal rows', () => {
    const { container } = render(
      <MemoryRouter>
        <RunEventFeed events={warning} connection="live" />
      </MemoryRouter>,
    );
    const row = screen.getByText('host gpu-9 sent no coverage stream').closest('[data-evkey]')!;
    expect(row).toHaveAttribute('data-kind', 'warning');
    expect(row).toHaveAttribute('data-accent', 'warn');
    expect(row).toHaveAttribute('data-mark', 'warning');
    // the step's ordinary row is untouched
    expect(screen.getByText('sweep started')).toBeInTheDocument();
    // a warning is not a failure: nothing is pinned
    expect(container.querySelector('[data-testid="needs-you-deck"]')).toBeNull();
  });

  it('a run whose events carry no time collapses the clock column', () => {
    const { container } = render(
      <MemoryRouter>
        <RunEventFeed events={warning} connection="live" />
      </MemoryRouter>,
    );
    expect(container.querySelector('.evx-scroll')).toHaveAttribute('data-untimed');
  });
});
