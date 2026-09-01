// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup } from '@testing-library/react';
import NetflowTable from './NetflowTable';
import type { FlowView } from '../../lib/netflow';

afterEach(() => {
  cleanup();
});

function flow(over: Partial<FlowView> = {}): FlowView {
  return {
    id: '01J',
    ts: '2026-08-03T00:00:00Z',
    ctx: { origin: { kind: 'provider', name: 'anthropic' }, run_id: 'r1' },
    fidelity: 'http',
    method: 'POST',
    scheme: 'https',
    host: 'api.anthropic.com',
    port: 443,
    path: '/v1/messages',
    outcome: 'ok',
    body_complete: true,
    bytes_in: 2048,
    bytes_out: 512,
    duration_ms: 30,
    asn: { asn: 13335, org: 'CLOUDFLARENET' },
    ...over,
  };
}

describe('NetflowTable', () => {
  it('renders host, path and ASN org', () => {
    render(<NetflowTable flows={[flow()]} droppedTotal={0} asnLoaded />);
    expect(screen.getByText('api.anthropic.com')).toBeInTheDocument();
    expect(screen.getByText('/v1/messages')).toBeInTheDocument();
    expect(screen.getByText(/CLOUDFLARENET/)).toBeInTheDocument();
    expect(screen.getByText(/AS13335/)).toBeInTheDocument();
  });

  it('no longer renders a fidelity column — fidelity lives in the detail panel and coverage popover', () => {
    // v3 redesign ("honesty on demand"): the per-row badge moved into
    // `FlowDetailPanel` (opened by row click) and the explorer's
    // CoveragePopover legend, both of which have their own tests.
    render(<NetflowTable flows={[flow({ fidelity: 'coarse' })]} droppedTotal={0} asnLoaded />);
    expect(screen.queryByText('coarse')).not.toBeInTheDocument();
    expect(screen.queryByText(/fidelity/i)).not.toBeInTheDocument();
  });

  it('shows Run and Workflow attribution columns only when showAttribution is set', () => {
    const attributed = flow({ run_id: 'run-9', workflow: 'review-wf' });
    const { unmount } = render(
      <NetflowTable flows={[attributed]} droppedTotal={0} asnLoaded showAttribution />,
    );
    expect(screen.getByText('run-9')).toBeInTheDocument();
    expect(screen.getByText('review-wf')).toBeInTheDocument();
    unmount();

    // Run scope (default): the columns would be noise — every row belongs
    // to the run already on screen.
    render(<NetflowTable flows={[attributed]} droppedTotal={0} asnLoaded />);
    expect(screen.queryByText('run-9')).not.toBeInTheDocument();
    expect(screen.queryByText('review-wf')).not.toBeInTheDocument();
  });

  it('renders an unknown byte count as a dash, not zero', () => {
    // `FlowView.bytes_in`/`bytes_out` are `number | undefined` — Rust's
    // `skip_serializing_if` OMITS the key when unobservable, it never
    // serializes `null`. `undefined` is therefore the correct fixture
    // value here, matching real wire payloads (see `NetflowSummary`'s
    // `p50_ms`/`p95_ms` doc comment for the identical Rust attribute
    // combination).
    render(
      <NetflowTable
        flows={[flow({ fidelity: 'coarse', bytes_in: undefined, bytes_out: undefined })]}
        droppedTotal={0}
        asnLoaded
      />,
    );
    expect(screen.queryByText('0 B')).not.toBeInTheDocument();
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  });

  it('moves the dropped sentence off populated tables (it lives in the coverage popover)', () => {
    // v3: on a NON-empty table the loss accounting surfaces through the
    // explorer's always-reachable CoveragePopover (same single-sourced
    // `droppedTotalSentence`), not a permanent banner. The all-dropped
    // empty case below still gets the loud in-place banner.
    render(<NetflowTable flows={[flow()]} droppedTotal={17} asnLoaded />);
    expect(screen.queryByText(/17 flows dropped/i)).not.toBeInTheDocument();
  });

  it('explains a missing ASN table instead of leaving a blank column', () => {
    // `FlowView.asn` is `AsnInfo | undefined` — the Rust side
    // (`#[serde(skip_serializing_if = "Option::is_none")]`) OMITS the key
    // rather than serializing `null`, so `undefined` is the correct "no
    // ASN" value here, not `null`.
    render(<NetflowTable flows={[flow({ asn: undefined })]} droppedTotal={0} asnLoaded={false} />);
    expect(screen.getByText(/ASN data not loaded/i)).toBeInTheDocument();
  });

  it('states the phase-1 scope limit on the empty state', () => {
    render(<NetflowTable flows={[]} droppedTotal={0} asnLoaded />);
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
    expect(screen.getByText(/does not cover.*subprocess/i)).toBeInTheDocument();
  });

  it('surfaces a non-zero dropped count even when every flow was dropped', () => {
    // Fix round 1: the empty-flows branch used to return EmptyState alone,
    // so an all-dropped scope silently under-reported — exactly the defect
    // the dropped banner exists to prevent, in its worst form (total loss
    // reads as "nothing happened" rather than "everything was lost").
    render(<NetflowTable flows={[]} droppedTotal={9} asnLoaded />);
    expect(screen.getByText(/9 flows dropped/i)).toBeInTheDocument();
    expect(screen.getByText(/No network flows recorded/i)).toBeInTheDocument();
  });

  // Task 4 (time-range picker): an empty result now has two distinct
  // honest readings — "nothing matched the selected window" vs "nothing
  // was ever recorded" — and the empty state must not conflate them. The
  // `appliedWindow` prop is the server's OWN echo of the applied
  // `?from=`/`?to=` (`NetflowResponse.window`), not the picker's local UI
  // state, so this reflects what the server actually did, not what was
  // merely requested.
  it('states "no flows in range" — not "nothing recorded" — when a window was applied and matched nothing', () => {
    render(
      <NetflowTable
        flows={[]}
        droppedTotal={0}
        asnLoaded
        appliedWindow={{ from: '2026-08-17T14:00:00Z', to: null }}
      />,
    );
    expect(screen.getByText(/no network flows.*this range/i)).toBeInTheDocument();
    // Must not claim the ledger itself is empty — only that this window is.
    expect(screen.queryByText(/^No network flows recorded for this scope$/i)).not.toBeInTheDocument();
  });

  it('keeps the unbounded "nothing recorded at all" wording when no window was applied', () => {
    render(
      <NetflowTable
        flows={[]}
        droppedTotal={0}
        asnLoaded
        appliedWindow={{ from: null, to: null }}
      />,
    );
    expect(screen.getByText(/No network flows recorded for this scope/i)).toBeInTheDocument();
    expect(screen.queryByText(/this range/i)).not.toBeInTheDocument();
  });

  it('defaults to the unbounded wording when no `appliedWindow` prop is passed at all (back-compat)', () => {
    render(<NetflowTable flows={[]} droppedTotal={0} asnLoaded />);
    expect(screen.getByText(/No network flows recorded for this scope/i)).toBeInTheDocument();
  });

  it('words a filtered-to-nothing empty list by the filters, not the scope or the range', () => {
    // The third distinct cause of emptiness (v3 cross-filters): claiming
    // "nothing recorded for this scope" — or even "nothing in this
    // range" — over a set the user just filtered to zero would be false.
    render(
      <NetflowTable
        flows={[]}
        droppedTotal={0}
        asnLoaded
        filtersActive
        appliedWindow={{ from: '2026-08-17T14:00:00Z', to: null }}
      />,
    );
    expect(screen.getByText(/no flows match the current filters/i)).toBeInTheDocument();
    expect(screen.queryByText(/no network flows/i)).not.toBeInTheDocument();
  });

  it('does not apply the range-empty wording when flows are present, even with a window applied', () => {
    render(
      <NetflowTable
        flows={[flow()]}
        droppedTotal={0}
        asnLoaded
        appliedWindow={{ from: '2026-08-17T14:00:00Z', to: null }}
      />,
    );
    expect(screen.queryByText(/no network flows/i)).not.toBeInTheDocument();
  });
});
