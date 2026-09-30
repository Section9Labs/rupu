// @vitest-environment jsdom
// RunEventFeed: run-level cards (no codename of their own) take the run's crew
// from `crewByRun`, so they get the crew tint stripe like named cards do.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import RunEventFeed, { type SeqEvent } from './RunEventFeed';

afterEach(cleanup);

const events: SeqEvent[] = [
  { seq: 1, event: { type: 'run_completed', run_id: 'run-1', status: 'completed' } as SeqEvent['event'] },
];

describe('RunEventFeed — crewByRun', () => {
  it('tints run-level cards with the run crew', () => {
    render(
      <MemoryRouter>
        <RunEventFeed events={events} connection="live" crewByRun={new Map([['run-1', 'cobalt-harbor']])} />
      </MemoryRouter>,
    );
    expect(screen.getByTestId('sr-crew-stripe')).toBeInTheDocument();
  });

  it('no stripe without the map', () => {
    render(
      <MemoryRouter>
        <RunEventFeed events={events} connection="live" />
      </MemoryRouter>,
    );
    expect(screen.queryByTestId('sr-crew-stripe')).toBeNull();
  });
});
