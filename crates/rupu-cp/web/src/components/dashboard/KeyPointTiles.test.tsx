// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
// fireEvent, not user-event: @testing-library/user-event is NOT a dependency.
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { KeyPointTiles } from './KeyPointTiles';
import type { ActiveCounts, ActiveLongest, TerminalBucket } from '../../lib/api';

afterEach(() => {
  cleanup();
});

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

const ACTIVE_ZERO: ActiveCounts = { running: 0, awaiting_approval: 0, paused: 0, pending: 0 };

describe('KeyPointTiles', () => {
  it('renders Awaiting-you and Paused with visual weight when nonzero', () => {
    wrap(
      <KeyPointTiles
        active={{ running: 0, awaiting_approval: 2, paused: 1, pending: 0 }}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    const awaiting = screen.getByTestId('tile-awaiting');
    expect(awaiting).toHaveTextContent('2');
    expect(awaiting.className).toMatch(/status-awaiting/);

    const paused = screen.getByTestId('tile-paused');
    expect(paused).toHaveTextContent('1');
    expect(paused.className).toMatch(/status-paused/);
  });

  it('renders Awaiting-you and Paused WITHOUT weight when zero', () => {
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    const awaiting = screen.getByTestId('tile-awaiting');
    expect(awaiting.className).not.toMatch(/status-awaiting/);
    const paused = screen.getByTestId('tile-paused');
    expect(paused.className).not.toMatch(/status-paused/);
  });

  it('Failed tile shows the terminal failed total and a sparkline, without crashing', () => {
    const buckets: TerminalBucket[] = [
      { ts: '2026-07-14T00:00:00Z', completed: 3, failed: 1, rejected: 0, cancelled: 0 },
      { ts: '2026-07-15T00:00:00Z', completed: 4, failed: 2, rejected: 0, cancelled: 0 },
    ];
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={buckets}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    const failed = screen.getByTestId('tile-failed');
    expect(failed).toHaveTextContent('3'); // 1 + 2
  });

  it('Success rate computes completed / total across terminal buckets', () => {
    const buckets: TerminalBucket[] = [
      { ts: '2026-07-14T00:00:00Z', completed: 8, failed: 2, rejected: 0, cancelled: 0 },
    ];
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={buckets}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    expect(screen.getByTestId('tile-success-rate')).toHaveTextContent('80%');
  });

  it('Success rate renders an em-dash when there is no terminal data at all', () => {
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    expect(screen.getByTestId('tile-success-rate')).toHaveTextContent('—');
  });

  it('Active now shows the running count plus the longest run, and links to /runs', () => {
    const longest: ActiveLongest = {
      run_id: 'run_1',
      workflow_name: 'nightly-review',
      age_ms: 2 * 3_600_000 + 14 * 60_000, // 2h14m
    };
    wrap(
      <KeyPointTiles
        active={{ running: 3, awaiting_approval: 0, paused: 0, pending: 0 }}
        activeLongest={longest}
        terminalBuckets={[]}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    const tile = screen.getByTestId('tile-active-now');
    expect(tile).toHaveTextContent('3');
    expect(tile).toHaveTextContent(/longest/i);
    expect(tile).toHaveTextContent(/2h/);
    expect(tile).toHaveTextContent('nightly-review');
    expect(tile.closest('a')).toHaveAttribute('href', '/runs');
  });

  it('Active now shows just the running count when active_longest is null', () => {
    wrap(
      <KeyPointTiles
        active={{ running: 5, awaiting_approval: 0, paused: 0, pending: 0 }}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    const tile = screen.getByTestId('tile-active-now');
    expect(tile).toHaveTextContent('5');
    expect(tile).not.toHaveTextContent(/longest/i);
  });

  it('Open findings renders an em-dash for null, NEVER a fabricated zero', () => {
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={null}
        findingsPartial={false}
      />,
    );
    const tile = screen.getByTestId('tile-findings');
    expect(tile).toHaveTextContent('—');
    expect(tile).not.toHaveTextContent(/^0$/);
  });

  it('Open findings renders a genuine zero as 0, not an em-dash', () => {
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={0}
        findingsPartial={false}
      />,
    );
    expect(screen.getByTestId('tile-findings')).toHaveTextContent('0');
  });

  it('Open findings marks a partial total as "(partial)"', () => {
    wrap(
      <KeyPointTiles
        active={ACTIVE_ZERO}
        activeLongest={null}
        terminalBuckets={[]}
        findingsOpen={7}
        findingsPartial={true}
      />,
    );
    expect(screen.getByTestId('tile-findings')).toHaveTextContent(/\(partial\)/);
  });
});
