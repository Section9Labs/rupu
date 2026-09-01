// Shared test fixtures for the explorer surface — imported ONLY by
// `*.test.tsx` files (page tests + explorer component tests), never by
// production code. Kept beside the components so fixture shape and wire
// shape evolve together.

import type {
  BucketAgg,
  ExplorerResponse,
  FlowView,
  HistogramView,
  KpiView,
  LaneAgg,
  NetflowResponse,
  NodeAgg,
  SankeyView,
  TimelineView,
} from '../../../lib/netflow';

export const node = (id: string, calls = 1, errors = 0): NodeAgg => ({
  id,
  label: id,
  calls,
  errors,
});

export const bucket = (calls = 0, errors = 0): BucketAgg => ({ calls, errors });

export function emptySankey(): SankeyView {
  return { workflows: [], origins: [], orgs: [], wf_origin: [], origin_org: [] };
}

export function emptyTimeline(): TimelineView {
  return {
    bucket_ms: 0,
    from: '2026-08-01T00:00:00Z',
    to: '2026-08-01T01:00:00Z',
    lanes: [],
    runs: [],
    runs_in_window: 0,
  };
}

export function emptyHistogram(): HistogramView {
  return { from: null, to: null, bucket_ms: 0, buckets: [] };
}

export function emptyKpis(): KpiView {
  return {
    flows: 0,
    endpoints: 0,
    orgs: 0,
    errors: 0,
    bytes_in: null,
    bytes_out: null,
    bytes_partial: false,
  };
}

export function emptyExplorerResponse(): ExplorerResponse {
  return {
    sankey: emptySankey(),
    timeline: emptyTimeline(),
    histogram: emptyHistogram(),
    kpis: emptyKpis(),
    dropped_total: 0,
    asn_loaded: true,
    window: { from: null, to: null },
  };
}

export function emptyFlowsResponse(): NetflowResponse {
  return {
    flows: [],
    hosts: [],
    dropped_total: 0,
    asn_loaded: true,
    window: { from: null, to: null },
  };
}

export function lane(overrides: Partial<LaneAgg> = {}): LaneAgg {
  return {
    host: 'api.anthropic.com',
    port: 443,
    org: 'Cloudflare',
    org_id: 'as13335',
    asn: 13335,
    fidelity: 'http',
    calls: 5,
    errors: 0,
    p95_ms: 120,
    buckets: [bucket(2), bucket(3, 1), bucket(), bucket()],
    ...overrides,
  };
}

/** A populated explorer response: one workflow → one origin → one org,
 *  two endpoints under the org, one bucket'd histogram. */
export function populatedExplorerResponse(): ExplorerResponse {
  return {
    sankey: {
      workflows: [node('review-wf', 3), node('unknown', 0)],
      origins: [node('provider:anthropic', 3)],
      orgs: [{ id: 'as13335', label: 'Cloudflare', calls: 3, errors: 1 }],
      wf_origin: [{ from: 'review-wf', to: 'provider:anthropic', calls: 3, errors: 1 }],
      origin_org: [{ from: 'provider:anthropic', to: 'as13335', calls: 3, errors: 1 }],
    },
    timeline: {
      bucket_ms: 60_000,
      from: '2026-08-01T00:00:00Z',
      to: '2026-08-01T01:00:00Z',
      lanes: [
        lane(),
        lane({ host: 'cdn.anthropic.com', calls: 2, errors: 1, fidelity: 'coarse' }),
      ],
      runs: [1, 2, 0, 0],
      runs_in_window: 2,
    },
    histogram: {
      from: '2026-08-01T00:00:00Z',
      to: '2026-08-01T01:00:00Z',
      bucket_ms: 42_857,
      buckets: [bucket(2), bucket(1, 1), bucket(), bucket(), bucket(), bucket(), bucket()],
    },
    kpis: {
      flows: 3,
      endpoints: 2,
      orgs: 1,
      errors: 1,
      bytes_in: 4096,
      bytes_out: 512,
      bytes_partial: false,
      p95_ms: 120,
    },
    dropped_total: 0,
    asn_loaded: true,
    window: { from: null, to: null },
  };
}

export function flowView(overrides: Partial<FlowView> = {}): FlowView {
  return {
    id: 'f1',
    ts: '2026-08-01T00:10:00Z',
    ctx: { origin: { kind: 'provider', name: 'anthropic' } },
    fidelity: 'http',
    method: 'POST',
    scheme: 'https',
    host: 'api.anthropic.com',
    port: 443,
    path: '/v1/messages',
    outcome: 'ok',
    status: 200,
    body_complete: true,
    bytes_in: 2048,
    bytes_out: 256,
    duration_ms: 30,
    ...overrides,
  };
}
