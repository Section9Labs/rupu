/**
 * Netflow API client and pure display helpers.
 *
 * Types mirror `crates/rupu-cp/src/api/netflow.rs` and the underlying
 * `rupu-netflow` wire types exactly — that Rust source is the contract, not
 * this file. In particular:
 *
 *   - A `null` byte count means "not observable" (Coarse fidelity — e.g.
 *     octocrab-backed SCM calls genuinely cannot report bytes), which is
 *     NOT the same as zero. Rendering it as `0 B` would state something
 *     false. `formatBytes` preserves the distinction.
 *   - `HostRollup` is computed server-side (`rupu_netflow::ledger::host_rollup`)
 *     so the percentile and unknown-bytes rules have exactly one
 *     implementation. This module does no client-side rollup — it only
 *     fetches and formats what the server already aggregated.
 *   - `asn_loaded: false` means enrichment was unavailable, not that flows
 *     had no ASN.
 */

import { ApiError } from './api';

// ---------------------------------------------------------------------------
// Types — mirror crates/rupu-netflow's wire shapes field-for-field.
//
// Optionality rule (verify every field against its Rust attribute, don't
// guess): a Rust `Option<T>` with `skip_serializing_if` becomes `field?: T`
// here — the key is OMITTED when `None`, so the runtime value is
// `undefined`, never `null`. A plain `Option<T>` with no
// `skip_serializing_if` becomes `field: T | null` — the key is always
// present, `null` when `None`. Typing an omitted-key field as `T | null`
// (or vice versa) compiles fine but is a runtime trap: `x.p !== null`
// reads true for an absent key, so a `.toFixed()` on it throws.
// ---------------------------------------------------------------------------

export type Fidelity = 'coarse' | 'http' | 'full';

export type Outcome = 'ok' | 'http_error' | 'transport_error' | 'timeout';

/**
 * `rupu_netflow::ctx::Origin` — adjacently tagged (`tag = "kind", content =
 * "name"`). `name` is present only for the variants that carry data
 * (`provider` / `scm`); the unit variants (`update` / `cp` / `system`)
 * serialize with no `name` key at all, not `name: null`.
 *
 * Careful: "can occur" is not "is captured". `update` / `cp` / `system`
 * can be constructed, but every production call site wires them to a
 * `NullSink`, so no flow with those origins ever reaches a ledger at any
 * scope — see `rupu-netflow`'s crate doc. Only `provider` and `scm` are
 * actually recorded.
 *
 * This lists only egress that can actually occur, mirroring the Rust enum:
 * `mcp` and `webhook` don't exist because neither subsystem makes outbound
 * HTTP (MCP dispatches into SCM connectors, which tag their own calls
 * `scm`; the webhook server is inbound-only).
 */
export interface Origin {
  kind: 'provider' | 'scm' | 'update' | 'cp' | 'system';
  name?: string;
}

/**
 * `rupu_netflow::ctx::FlowCtx`. All four id fields are
 * `#[serde(default, skip_serializing_if = "Option::is_none")]` — omitted,
 * never `null`, when absent.
 */
export interface FlowCtx {
  run_id?: string;
  step_id?: string;
  agent?: string;
  workspace_id?: string;
  origin: Origin;
}

/** `rupu_netflow::asn::table::AsnInfo` — both fields always present. */
export interface AsnInfo {
  asn: number;
  org: string;
}

/**
 * `rupu_cp::api::netflow::FlowView` — a `FlowRecord` flattened plus
 * read-time ASN enrichment. `asn` is `None`/absent when either the peer IP
 * is unknown (Coarse fidelity) or the ASN table has no entry for it — the
 * UI must not conflate that with `asn_loaded: false` at the response level.
 */
export interface FlowView {
  id: string;
  ts: string;
  ctx: FlowCtx;
  fidelity: Fidelity;
  method: string;
  scheme: string;
  host: string;
  port: number;
  path: string;
  peer_ip?: string;
  resolved_ips?: string[];
  http_version?: string;
  status?: number;
  outcome: Outcome;
  error?: string;
  /** `Option<u64>` with `skip_serializing_if` — omitted, never `null`. */
  bytes_out?: number;
  bytes_in?: number;
  body_complete: boolean;
  ttfb_ms?: number;
  duration_ms?: number;
  asn?: AsnInfo;
  /** The TOP-LEVEL run this flow folds into (ledger-file id mapped back
   *  through step/fan-out/sub-agent records — `rupu_cp::api::netflow::
   *  RunMetaIndex`). Distinct from `ctx.run_id`, which is unset on every
   *  production flow. Omitted when no run record accounts for the ledger
   *  id (e.g. a standalone agent run). */
  run_id?: string;
  /** `RunRecord::workflow_name` of that root run; omitted with `run_id`. */
  workflow?: string;
}

/**
 * `rupu_netflow::ledger::views::HostRollup` — computed server-side. Do not
 * re-derive any of this in TypeScript.
 *
 * `bytes_in`/`bytes_out` have NO `skip_serializing_if` — the key always
 * serializes, `null` when unobservable, hence `T | null` (required key).
 * `p50_ms`/`p95_ms` DO have `skip_serializing_if` — the key is omitted
 * entirely when `None`, hence `field?: T` (optional, never actually
 * `null`), matching `FlowView.ttfb_ms`/`duration_ms`'s treatment of the
 * identical Rust attribute combination.
 */
export interface HostRollup {
  host: string;
  port: number;
  calls: number;
  bytes_in: number | null;
  bytes_out: number | null;
  errors: number;
  p50_ms?: number;
  p95_ms?: number;
}

/** `rupu_cp::api::netflow::NetflowResponse`. */
export interface NetflowResponse {
  flows: FlowView[];
  hosts: HostRollup[];
  /** Records lost to writer overflow, for the WHOLE ledger file(s) this
   *  response reads — NOT scoped to `?from=`/`?to=` when a time filter is
   *  applied. At run scope this is already a sum across more than one
   *  file whenever the run dispatched a step or a sub-agent (see
   *  `rupu_cp::api::netflow::run_and_unit_ids`); project/global scope sum
   *  across every contributing run's ledger on top of that. A drop batch
   *  carries no per-record timestamp, so it can never be attributed to a
   *  window; this count is the same value regardless of which (if any)
   *  filter produced `flows`. Named `dropped_total` (not `dropped`) so a
   *  filtered view can never be misread as "nothing was lost in this
   *  window" — see `rupu_netflow::ledger::views::read_flows_in_range`'s doc. */
  dropped_total: number;
  /** `false` means enrichment was unavailable, not that flows lack an ASN. */
  asn_loaded: boolean;
  /** `rupu_cp::api::netflow::WindowEcho` — the `?from=`/`?to=` window the
   *  server actually applied to produce `flows` (both fields present, each
   *  independently `null` when that side is unbounded). This is the
   *  positive confirmation a caller's filter was honoured — Task 4 (the
   *  time-range picker) uses it, rather than its own request state, to
   *  decide whether an empty `flows` means "nothing in this range" or
   *  "nothing recorded at all", since the echo reflects what the server
   *  actually did, not what the UI asked for. */
  window: { from: string | null; to: string | null };
}

// ---------------------------------------------------------------------------
// Explorer aggregate types — mirror `rupu_netflow::ledger::explorer` and
// `rupu_cp::api::netflow::ExplorerResponse` field-for-field. Same
// optionality rule as above: `skip_serializing_if` → `field?: T`, plain
// `Option<T>` → `field: T | null`.
// ---------------------------------------------------------------------------

/** `rupu_netflow::ledger::explorer::UNKNOWN_KEY` — the explicit bucket
 *  both the workflow and org dimensions use for unresolved flows (never
 *  silently dropped). */
export const NETFLOW_UNKNOWN_KEY = 'unknown';

/** One topology-column node. `calls: 0` means "in scope but filtered /
 *  windowed out" — render dimmed, never remove (out-of-scope nodes are
 *  absent from the response entirely). */
export interface NodeAgg {
  id: string;
  label: string;
  calls: number;
  errors: number;
}

/** One ribbon between adjacent topology columns. `calls` is the ONLY
 *  sanctioned visual weight (never bytes — Fix 4 rationale). */
export interface LinkAgg {
  from: string;
  to: string;
  calls: number;
  errors: number;
}

/** `rupu_netflow::ledger::explorer::SankeyView`. */
export interface SankeyView {
  workflows: NodeAgg[];
  origins: NodeAgg[];
  orgs: NodeAgg[];
  wf_origin: LinkAgg[];
  origin_org: LinkAgg[];
}

/** One dense time bucket. */
export interface BucketAgg {
  calls: number;
  errors: number;
}

/** One endpoint swimlane. `buckets` is dense — always the view's full
 *  bucket count, zeros included. `fidelity` is the LEAST-observable
 *  contributor's level. `p50_ms`/`p95_ms` mirror `HostRollup`'s
 *  omitted-when-unobservable convention. */
export interface LaneAgg {
  host: string;
  port: number;
  /** Org display label ("Unknown network" for the unknown group). */
  org: string;
  /** Org filter key (`as<number>` / `unknown`). */
  org_id: string;
  asn?: number;
  fidelity: Fidelity;
  calls: number;
  errors: number;
  p50_ms?: number;
  p95_ms?: number;
  buckets: BucketAgg[];
}

/** `rupu_netflow::ledger::explorer::TimelineView`. */
export interface TimelineView {
  bucket_ms: number;
  from: string;
  to: string;
  lanes: LaneAgg[];
  /** Dense: active in-scope run count per bucket. */
  runs: number[];
  runs_in_window: number;
}

/** The activity strip's full-history histogram. Bounds are the SERVER's
 *  (min/max recorded ts across the scope, ignoring window and filters);
 *  both `null` (all buckets zero) when the scope has no flows at all. */
export interface HistogramView {
  from: string | null;
  to: string | null;
  bucket_ms: number;
  buckets: BucketAgg[];
}

/** `rupu_netflow::ledger::explorer::KpiView`. `bytes_in`/`bytes_out` are
 *  observed-only sums — `null` when NO flow contributed an observed value
 *  (rendering that as `0 B` would claim we saw zero bytes); when
 *  `bytes_partial` is true the sums exclude at least one unobservable
 *  flow and must be marked as partial. */
export interface KpiView {
  flows: number;
  endpoints: number;
  orgs: number;
  errors: number;
  bytes_in: number | null;
  bytes_out: number | null;
  bytes_partial: boolean;
  p95_ms?: number;
}

/** `rupu_cp::api::netflow::ExplorerResponse`. (No `hosts` rollup — every
 *  per-endpoint number the surface renders comes from `timeline.lanes`,
 *  whose facet semantics are the ones the org cards/lane stats need; see
 *  the Rust type's doc comment for the deliberate deviation note.) */
export interface ExplorerResponse {
  sankey: SankeyView;
  timeline: TimelineView;
  histogram: HistogramView;
  kpis: KpiView;
  /** Whole-history, never window- or filter-scoped — same contract as
   *  `NetflowResponse.dropped_total`. */
  dropped_total: number;
  asn_loaded: boolean;
  window: { from: string | null; to: string | null };
}

/** The four cross-filter sets, keyed by the server's dimension keys
 *  (workflow names / `provider:x`,`scm:x` / `as<number>`,`unknown` /
 *  `host:port`). Empty array = dimension unfiltered. */
export interface NetflowFilters {
  workflows: string[];
  origins: string[];
  orgs: string[];
  hosts: string[];
}

export const EMPTY_NETFLOW_FILTERS: NetflowFilters = {
  workflows: [],
  origins: [],
  orgs: [],
  hosts: [],
};

export function filtersAreEmpty(f: NetflowFilters): boolean {
  return (
    f.workflows.length === 0 &&
    f.origins.length === 0 &&
    f.orgs.length === 0 &&
    f.hosts.length === 0
  );
}

// (The bipartite `GraphView`/`GraphNode`/`GraphEdge` types and their
// `fetchNetflowGraph` helper were retired with `NetflowGraph.tsx` when the
// explorer replaced the last caller; `/api/netflow/graph` itself still
// exists server-side for non-web consumers.)

// ---------------------------------------------------------------------------
// Fetch helpers
// ---------------------------------------------------------------------------

/**
 * A single-argument `fetch(url)` call, mirroring `./api`'s `request<T>`
 * shape closely enough to stay on the same `ApiError` convention, without
 * pulling in `request`'s JSON body/`Content-Type` request defaults this
 * module's plain GETs don't need.
 *
 * On a non-ok response this reads the body and, when it parses as the CP's
 * standard `{ "error": "..." }` shape (`rupu_cp::error::ApiError`'s
 * `IntoResponse`), surfaces that message verbatim — this is how a
 * malformed `?from=`/`?to=` 400 (Task 3's `parse_time_range`, which names
 * the offending parameter and the expected format) actually reaches an
 * operator instead of collapsing to a bare "HTTP 400". Falls back to the
 * generic `url → HTTP status` message when the body is absent, unreadable,
 * or not JSON — e.g. the plain mocks in this module's own tests, which
 * don't implement `.text()` at all.
 */
async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
    let message = `${url} → HTTP ${res.status}`;
    let body = '';
    try {
      body = await res.text();
      const parsed = body ? (JSON.parse(body) as { error?: string }) : null;
      if (parsed?.error) message = parsed.error;
    } catch {
      // No body, unreadable, or not JSON — keep the generic message.
    }
    throw new ApiError(res.status, message, body);
  }
  return (await res.json()) as T;
}

/** `?from=`/`?to=` for the four netflow-read routes (Task 3's
 *  `TimeRangeQuery`/`GraphQuery`) — both independently optional, RFC 3339.
 *  Passing `undefined` (rather than an empty object) from a call site
 *  means "no filter", appending neither parameter — see `appendRange`. */
export interface NetflowRange {
  from?: string;
  to?: string;
}

/** Appends `from`/`to` (from `range`) and the four cross-filter params
 *  (from `filters`, comma-joined — the encoding `rupu_cp`'s
 *  `split_filter` reads back) only when present — omitted `range` AND
 *  empty/omitted `filters` produce byte-identical URLs to before either
 *  parameter existed, so every unfiltered call site is unaffected. */
function appendRange(url: string, range?: NetflowRange, filters?: NetflowFilters): string {
  const params = new URLSearchParams();
  if (range?.from) params.set('from', range.from);
  if (range?.to) params.set('to', range.to);
  if (filters) {
    if (filters.workflows.length) params.set('workflow', filters.workflows.join(','));
    if (filters.origins.length) params.set('origin', filters.origins.join(','));
    if (filters.orgs.length) params.set('org', filters.orgs.join(','));
    if (filters.hosts.length) params.set('host', filters.hosts.join(','));
  }
  const qs = params.toString();
  if (!qs) return url;
  return `${url}${url.includes('?') ? '&' : '?'}${qs}`;
}

export const fetchRunNetflow = (
  runId: string,
  range?: NetflowRange,
  filters?: NetflowFilters,
): Promise<NetflowResponse> =>
  getJson<NetflowResponse>(
    appendRange(`/api/runs/${encodeURIComponent(runId)}/netflow`, range, filters),
  );

export const fetchProjectNetflow = (
  projectId: string,
  range?: NetflowRange,
  filters?: NetflowFilters,
): Promise<NetflowResponse> =>
  getJson<NetflowResponse>(
    appendRange(`/api/projects/${encodeURIComponent(projectId)}/netflow`, range, filters),
  );

export const fetchGlobalNetflow = (
  range?: NetflowRange,
  filters?: NetflowFilters,
): Promise<NetflowResponse> =>
  getJson<NetflowResponse>(appendRange('/api/netflow', range, filters));

/** `scope` is `run:<id>` or `project:<id>`; omitted entirely for global —
 *  the same contract as `GET /api/netflow/graph`'s scope param. */
export const fetchNetflowExplorer = (
  scope?: string,
  range?: NetflowRange,
  filters?: NetflowFilters,
): Promise<ExplorerResponse> =>
  getJson<ExplorerResponse>(
    appendRange(
      scope
        ? `/api/netflow/explorer?scope=${encodeURIComponent(scope)}`
        : '/api/netflow/explorer',
      range,
      filters,
    ),
  );

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/**
 * Format a byte count for display. `null`/`undefined` render as an em dash
 * — "not observable" — never as `0 B`, which would claim we saw zero bytes
 * when we in fact saw nothing at all (Coarse fidelity, e.g. octocrab).
 */
export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return '—';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}
