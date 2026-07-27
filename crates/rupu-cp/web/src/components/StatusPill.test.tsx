// @vitest-environment jsdom
// Task 2 — shared animated status glyph. Verifies the motion marker shows up
// exactly where the motion language says it should (running/awaiting, never
// terminal/resting states) and that a SESSION status routes through the same
// rendering path as a RUN status, so all four run-like list pages (plus
// Sessions, once Task 3 wires it in) speak one visual language.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/react';
import { SessionStatusPill, StatusPill } from './StatusPill';

afterEach(() => {
  cleanup();
});

describe('StatusPill — shared motion marker', () => {
  it('renders the motion marker for a running run status', () => {
    const { container } = render(<StatusPill status="running" />);
    const pill = container.querySelector('[data-motion]');
    expect(pill).not.toBeNull();
    expect(pill).toHaveAttribute('data-motion', 'rg-pulse-run');
    expect(pill?.className).toContain('rg-pulse-run');
  });

  it('renders the (amber) motion marker for an awaiting_approval run status', () => {
    const { container } = render(<StatusPill status="awaiting_approval" />);
    const pill = container.querySelector('[data-motion]');
    expect(pill).not.toBeNull();
    expect(pill).toHaveAttribute('data-motion', 'rg-pulse-await');
  });

  it.each(['pending', 'completed', 'failed', 'paused', 'rejected', 'cancelled'] as const)(
    'renders NO motion marker for the terminal/resting status "%s"',
    (status) => {
      const { container } = render(<StatusPill status={status} />);
      expect(container.querySelector('[data-motion]')).toBeNull();
    },
  );
});

describe('StatusPill vs SessionStatusPill — one shared visual language', () => {
  it('a SESSION "running" renders the identical motion marker as a RUN "running"', () => {
    const run = render(<StatusPill status="running" />);
    const session = render(<SessionStatusPill status="running" />);

    const runMarker = run.container.querySelector('[data-motion]');
    const sessionMarker = session.container.querySelector('[data-motion]');

    expect(runMarker).not.toBeNull();
    expect(sessionMarker).not.toBeNull();
    expect(sessionMarker).toHaveAttribute('data-motion', runMarker!.getAttribute('data-motion'));
  });

  it.each(['idle', 'failed', 'stopped'] as const)(
    'session "%s" is a static (non-animated) pill',
    (status) => {
      const { container } = render(<SessionStatusPill status={status} />);
      expect(container.querySelector('[data-motion]')).toBeNull();
    },
  );

  it('a genuinely unrecognized session status renders the RAW label, never a fabricated real state', () => {
    const { getByText, queryByText, container } = render(
      <SessionStatusPill status="zzz-not-a-real-status" />,
    );
    expect(getByText('zzz-not-a-real-status')).toBeInTheDocument();
    expect(queryByText('Stopped')).toBeNull();
    expect(container.querySelector('[data-motion]')).toBeNull();
  });

  it('preserves the session-native words — idle/failed/stopped never render as run words', () => {
    expect(render(<SessionStatusPill status="idle" />).getByText('Idle')).toBeInTheDocument();
    expect(render(<SessionStatusPill status="failed" />).getByText('Failed')).toBeInTheDocument();
    expect(render(<SessionStatusPill status="stopped" />).getByText('Stopped')).toBeInTheDocument();
    // Never renamed into run-status vocabulary.
    expect(render(<SessionStatusPill status="idle" />).queryByText('Completed')).toBeNull();
    expect(render(<SessionStatusPill status="stopped" />).queryByText('Cancelled')).toBeNull();
  });
});
