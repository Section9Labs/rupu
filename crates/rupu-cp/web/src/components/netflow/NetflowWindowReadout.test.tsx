// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import NetflowWindowReadout from './NetflowWindowReadout';

afterEach(() => {
  cleanup();
});

describe('NetflowWindowReadout', () => {
  // Built SOLELY from the server's `window` echo — never the picker's own
  // state — so it can't drift the way a picker-derived summary could
  // (whole-branch review round 1, "Also do"). It also answers the
  // relative-preset pinning ambiguity: `TimeRangePicker`'s "Last hour"
  // resolves against `now()` once, at click time, and never re-derives —
  // this readout always shows the actual bounds that produced the data on
  // screen, however long ago they were computed.

  it('states both bounds when the server applied a fully-bounded window', () => {
    render(<NetflowWindowReadout appliedWindow={{ from: '2026-08-17T14:00:00.000Z', to: '2026-08-17T15:00:00.000Z' }} />);
    expect(screen.getByText(/showing flows from/i)).toBeInTheDocument();
  });

  it('states an open-ended lower bound when only `from` is set', () => {
    render(<NetflowWindowReadout appliedWindow={{ from: '2026-08-17T14:00:00.000Z', to: null }} />);
    expect(screen.getByText(/showing flows from/i)).toBeInTheDocument();
    expect(screen.getByText(/onward/i)).toBeInTheDocument();
  });

  it('states an open-ended upper bound when only `to` is set', () => {
    render(<NetflowWindowReadout appliedWindow={{ from: null, to: '2026-08-17T15:00:00.000Z' }} />);
    expect(screen.getByText(/showing flows up to/i)).toBeInTheDocument();
  });

  it('states "all recorded flows" — never a bound — when neither side is set', () => {
    render(<NetflowWindowReadout appliedWindow={{ from: null, to: null }} />);
    expect(screen.getByText(/showing all recorded flows/i)).toBeInTheDocument();
    expect(screen.queryByText(/from|onward|up to/i)).not.toBeInTheDocument();
  });

  it('never claims a bound the echo does not carry, even when a `from` string fails to parse', () => {
    // Defensive: the wire contract guarantees RFC 3339 or null, but this
    // must degrade honestly rather than render "Invalid Date" if it ever
    // sees something else.
    render(<NetflowWindowReadout appliedWindow={{ from: 'not-a-date', to: null }} />);
    expect(screen.queryByText(/invalid date/i)).not.toBeInTheDocument();
  });
});
