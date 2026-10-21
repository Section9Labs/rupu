// The invented `acme-shop-auth` agentiflow (spec asset
// `docs/superpowers/specs/assets/agentiflow-flow/fixture.js`), reshaped into
// the CP API's types: the detail, its children, the flow's message space and
// board, and the generated workflow child `gannet#1` with its own space,
// board and run graph. Invented from scratch; shared by the flow tests.

import type {
  AgentiflowDetail,
  AgentiflowEvent,
  BoardView,
  RunChild,
  SpaceMessage,
  SpaceParticipant,
} from '../api';
import type { FanoutState, GraphNode, RunGraphModel, StepState, UnitView } from '../runGraphModel';
import type { FlowInput, FlowSpace } from './flowModel';

export const FLOW_ID = 'af_01M5MOCKFLOW';
export const CREW = 'cobalt-harbor';
export const LEAD_CODENAME = `${CREW}/heron`;

/** `2026-10-20T14:00Z` plus `min` minutes, as RFC 3339. */
export const at = (min: number): string => new Date(Date.UTC(2026, 9, 20, 14, min)).toISOString();
/** The same instant in epoch ms. */
export const ms = (min: number): number => Date.UTC(2026, 9, 20, 14, min);

export const RUN = {
  lynx1: 'run_01M5LYNX1',
  lynx2: 'run_01M5LYNX2',
  lynx3: 'run_01M5LYNX3',
  wren1: 'sub_01M5WREN1',
  wren2: 'sub_01M5WREN2',
  gannet1: 'run_01M5GANNET1',
  kite1: 'run_01M5KITE1',
  kite2: 'run_01M5KITE2',
  kite3: 'run_01M5KITE3',
  wren3: 'sub_01M5WREN3',
  owl1: 'run_01M5OWL1',
} as const;

const goalsAt = (verified: number, report: number) => [
  { id: 'verified', met: verified >= 2, current: verified },
  { id: 'report', met: report >= 1, current: report },
];

const roundEvent = (
  round: number,
  start: number,
  end: number,
  decision: string,
  goals: { id: string; met: boolean; current: number }[],
  spent_usd: number,
  spent_tokens: number,
): AgentiflowEvent => ({
  kind: 'round',
  ts: at(end),
  round,
  started_at: at(start),
  decision,
  goals,
  goals_met: goals.filter((g) => g.met).length,
  goals_total: goals.length,
  budget: 'ok',
  converge: false,
  steering: 0,
  outcome: 'yielded',
  spent_usd,
  spent_tokens,
});

export const EVENTS: AgentiflowEvent[] = [
  { kind: 'run_started', ts: at(0), id: FLOW_ID, name: 'acme-shop-auth', goals: 2, engagement_profiles: ['web'] },
  roundEvent(0, 1, 23, 'Map the surface before testing anything.', goalsAt(0, 0), 8.4, 1_310_000),
  roundEvent(
    1,
    24,
    84,
    'Generated auth-sweep to probe every auth endpoint, with a review panel.',
    goalsAt(0, 0),
    29.75,
    4_350_000,
  ),
  roundEvent(
    2,
    85,
    111,
    'Verify the two highest findings; retry the verifier that hit a 529.',
    goalsAt(0, 0),
    39.05,
    5_720_000,
  ),
  // Round 3 ("Write the engagement report.") is running: its event is
  // written only when it finishes.
];

const transcript = (round: number) => `/home/acme/.rupu/agentiflows/${FLOW_ID}/lead/round-${round}.jsonl`;

export const DETAIL: AgentiflowDetail = {
  record: {
    id: FLOW_ID,
    name: 'acme-shop-auth',
    codename: CREW,
    codename_derived: false,
    status: 'running',
    stop_reason: null,
    // Stamped when the flow stops; 0 while it runs.
    rounds: 0,
    // Goals are stamped when the flow completes; empty while it runs.
    goals_met: 0,
    goals_total: 0,
    spent_usd: 41.37,
    spent_tokens: 6_060_000,
    started_at: at(0),
    ended_at: null,
    engagement_profiles: ['web'],
    runner_alive: true,
    trigger: 'manual',
    runner_pid: 48211,
    goals: [],
  },
  def: {
    name: 'acme-shop-auth',
    description:
      'Authorized test of the acme-shop staging auth surface: find and verify real authentication and authorization flaws.',
    lead: 'audit-lead',
    engagement_profiles: ['web'],
    trigger: null,
    goals: [
      {
        id: 'verified',
        objective: 'At least 2 verified auth findings',
        required: true,
        verify_with: 'exploit-verifier',
        predicate: 'findings, count >= 2, verified by exploit-verifier',
        target: { findings: { count: 2, verified: true } },
      },
      {
        id: 'report',
        objective: 'An engagement report',
        required: true,
        verify_with: null,
        predicate: 'artifacts, count >= 1',
        target: { artifacts: { count: 1 } },
      },
    ],
    coverage: null,
    budget: { usd: 150, rounds: 10 },
    scope: { hosts: ['staging.acme-shop.test'] },
    pool: {
      agents: [
        'surface-mapper',
        'dependency-auditor',
        'config-reviewer',
        'exploit-verifier',
        'engagement-reporter',
      ],
      workflows: 'all',
    },
    round: null,
  },
  budget_state: 'ok',
  events: EVENTS,
  units: [],
  lead_transcripts: [0, 1, 2, 3].map((round) => ({ round, path: transcript(round) })),
};

function child(c: Partial<RunChild> & Pick<RunChild, 'handle' | 'kind' | 'name' | 'run_id' | 'status'>): RunChild {
  const handle = c.handle ?? '';
  return {
    parent_run_id: FLOW_ID,
    depth: 1,
    codename: `${LEAD_CODENAME}>${handle}`,
    address: handle,
    success: c.status === 'done' ? true : c.status === 'failed' ? false : null,
    error: null,
    tokens: null,
    duration_ms: null,
    started_at: null,
    finished_at: null,
    transcript_path: `/home/acme/.rupu/runs/${c.run_id}/transcript.jsonl`,
    legacy: false,
    ...c,
  };
}

const span = (start: number, end: number | null) => ({
  started_at: at(start),
  finished_at: end === null ? null : at(end),
  duration_ms: end === null ? null : (end - start) * 60_000,
});

/** As `GET /api/agentiflows/:id/children` lists them: the lead's units in
 *  ledger order, then each unit's own launch tree. */
export const CHILDREN: RunChild[] = [
  child({
    handle: 'lynx#1', kind: 'agent', name: 'surface-mapper', run_id: RUN.lynx1, status: 'done', round: 0,
    ...span(2, 19), tokens: { input: 410_000, output: 9_800 },
    output_excerpt: 'Mapped 14 routes, 3 auth flows, 2 admin panels.',
  }),
  child({
    handle: 'lynx#2', kind: 'agent', name: 'dependency-auditor', run_id: RUN.lynx2, status: 'done', round: 0,
    ...span(2, 15), tokens: { input: 260_000, output: 6_100 },
    output_excerpt: '9 outdated packages, 1 with a known advisory (session lib).',
  }),
  child({
    handle: 'lynx#3', kind: 'agent', name: 'config-reviewer', run_id: RUN.lynx3, status: 'done', round: 0,
    ...span(3, 22), tokens: { input: 330_000, output: 7_400 },
    output_excerpt: 'Debug toolbar enabled in staging config; CORS allows *.',
  }),
  child({
    handle: 'gannet#1', kind: 'workflow', name: 'auth-sweep', run_id: RUN.gannet1, status: 'done', round: 1,
    generated: true, ...span(26, 83), tokens: { input: 2_900_000, output: 61_000 },
    transcript_path: null,
    output_excerpt:
      '4 findings: reset-token reuse (high), missing invite rate limit (medium), role escalation via /admin/roles (critical), verbose session errors (low).',
  }),
  child({
    handle: 'kite#1', kind: 'agent', name: 'exploit-verifier', run_id: RUN.kite1, status: 'done', round: 2,
    ...span(86, 104), tokens: { input: 780_000, output: 14_000 },
    output_excerpt: 'Role escalation confirmed with a PoC request; screenshot attached.',
  }),
  child({
    handle: 'kite#2', kind: 'agent', name: 'exploit-verifier', run_id: RUN.kite2, status: 'failed', round: 2,
    ...span(86, 92), tokens: { input: 150_000, output: 2_200 },
    error: 'provider overloaded (529) after 3 retries',
  }),
  child({
    handle: 'kite#3', kind: 'agent', name: 'exploit-verifier', run_id: RUN.kite3, status: 'done', round: 2,
    ...span(93, 110), tokens: { input: 640_000, output: 11_500 },
    output_excerpt: 'Reset-token reuse confirmed; tokens valid 24h after use.',
  }),
  child({
    handle: 'owl#1', kind: 'agent', name: 'engagement-reporter', run_id: RUN.owl1, status: 'running', round: 3,
    ...span(114, null), tokens: { input: 310_000, output: 5_200 },
  }),
  child({
    handle: 'wren#1', kind: 'subagent', name: 'route-crawler', run_id: RUN.wren1, parent_run_id: RUN.lynx1,
    depth: 2, codename: `${LEAD_CODENAME}>lynx#1>wren#1`, address: 'lynx#1>wren#1', status: 'done', round: 0,
    ...span(5, 11), tokens: { input: 90_000, output: 2_100 }, output_excerpt: 'Crawled /api, /admin, /auth.',
  }),
  child({
    handle: 'wren#2', kind: 'subagent', name: 'js-bundle-reader', run_id: RUN.wren2, parent_run_id: RUN.lynx1,
    depth: 2, codename: `${LEAD_CODENAME}>lynx#1>wren#2`, address: 'lynx#1>wren#2', status: 'done', round: 0,
    ...span(6, 13), tokens: { input: 120_000, output: 3_300 },
    output_excerpt: 'Found 4 undocumented endpoints in the bundle.',
  }),
  child({
    handle: 'wren#3', kind: 'subagent', name: 'token-replayer', run_id: RUN.wren3, parent_run_id: RUN.kite3,
    depth: 2, codename: `${LEAD_CODENAME}>kite#3>wren#3`, address: 'kite#3>wren#3', status: 'done', round: 2,
    ...span(97, 104), tokens: { input: 70_000, output: 1_900 }, output_excerpt: 'Replayed 5 tokens, 5 accepted.',
  }),
];

const participant = (
  address: string,
  agent: string,
  run_id: string,
  kind: string,
  live: boolean,
  extra: Partial<SpaceParticipant> = {},
): SpaceParticipant => ({ address, codename: `${LEAD_CODENAME}>${address}`, agent, run_id, kind, live, ...extra });

/** The flow's space. A workflow child is its own message root, so
 *  `gannet#1` is not a participant here. */
export const FLOW_PARTICIPANTS: SpaceParticipant[] = [
  { address: 'lead', codename: LEAD_CODENAME, agent: 'audit-lead', run_id: FLOW_ID, kind: 'flow_lead', live: true },
  participant('lynx#1', 'surface-mapper', RUN.lynx1, 'flow_unit', false),
  participant('lynx#2', 'dependency-auditor', RUN.lynx2, 'flow_unit', false),
  participant('lynx#3', 'config-reviewer', RUN.lynx3, 'flow_unit', false),
  participant('lynx#1>wren#1', 'route-crawler', RUN.wren1, 'subagent', false),
  participant('lynx#1>wren#2', 'js-bundle-reader', RUN.wren2, 'subagent', false),
  participant('kite#1', 'exploit-verifier', RUN.kite1, 'flow_unit', false),
  participant('kite#2', 'exploit-verifier', RUN.kite2, 'flow_unit', false),
  participant('kite#3', 'exploit-verifier', RUN.kite3, 'flow_unit', false),
  participant('kite#3>wren#3', 'token-replayer', RUN.wren3, 'subagent', false),
  participant('owl#1', 'engagement-reporter', RUN.owl1, 'flow_unit', true),
];

function msg(m: Partial<SpaceMessage> & Pick<SpaceMessage, 'id' | 'kind' | 'from' | 'body'>): SpaceMessage {
  return { ts: at(0), to: null, delivery: { resolved: [] }, seen: [], ...m };
}

export const FLOW_MESSAGES: SpaceMessage[] = [
  msg({
    id: 'msg_001', ts: at(2), kind: 'directive', from: 'lead', to: null,
    body: 'Stay inside acme-shop staging. No destructive requests.',
    delivery: { resolved: ['lynx#1', 'lynx#2', 'lynx#3'] },
  }),
  msg({
    id: 'msg_002', ts: at(9), kind: 'post', post_kind: 'claim', from: 'lynx#1',
    body: 'claim: route map (/api, /admin, /auth)',
  }),
  msg({
    id: 'msg_003', ts: at(12), kind: 'post', post_kind: 'observation', from: 'lynx#2',
    body: 'session lib 2.1.0 has a published token-fixation advisory',
  }),
  msg({
    id: 'msg_004', ts: at(14), kind: 'direct', from: 'lynx#2', to: 'lynx#1',
    body: 'Does /auth/reset use the session lib? Worth a look.',
    delivery: { resolved: ['lynx#1'] }, seen: [{ by: 'lynx#1', turn: 6, at: at(15) }],
  }),
  msg({
    id: 'msg_005', ts: at(16), kind: 'direct', from: 'lynx#1', to: 'lynx#2',
    body: 'Yes, reset tokens go through it. Flagging for round 1.',
    delivery: { resolved: ['lynx#2'] },
  }),
  msg({
    id: 'msg_006', ts: at(20), kind: 'direct', from: 'lynx#3', to: 'lead',
    body: 'CORS allows *.acme-shop.test with credentials. Probably not exploitable alone.',
    delivery: { resolved: ['lead'] },
  }),
  msg({
    id: 'msg_007', ts: at(25), kind: 'operator', from: 'operator', to: 'lead', urgent: true, via: 'cp',
    body: 'Prioritise the admin roles endpoint, the client asked about it.',
    delivery: { resolved: ['lead'] },
  }),
  msg({
    id: 'msg_013', ts: at(88), kind: 'directive', from: 'lead', to: 'role:exploit-verifier',
    body: 'Verify escalation and reset-token reuse. Capture evidence, no data changes.',
    delivery: { resolved: ['kite#1', 'kite#2', 'kite#3'] },
  }),
  msg({
    id: 'msg_014', ts: at(92), kind: 'direct', from: 'kite#2', to: 'lead',
    body: 'Provider returned 529 three times, giving up.',
    delivery: { resolved: ['lead'] },
  }),
  msg({
    id: 'msg_015', ts: at(95), kind: 'direct', from: 'lead', to: 'kite#3',
    body: 'Take reset-token reuse, kite#2 dropped it.',
    delivery: { resolved: ['kite#3'] },
  }),
  msg({
    id: 'msg_016', ts: at(105), kind: 'post', post_kind: 'finding', from: 'kite#1',
    body: 'VERIFIED: role escalation, PoC in evidence',
  }),
  msg({
    id: 'msg_017', ts: at(108), kind: 'direct', from: 'kite#3', to: 'owl#1',
    body: 'Evidence for reset reuse is in kite#3/evidence/',
    delivery: { resolved: [], undelivered: [{ to: 'owl#1', reason: 'unknown_participant' }] },
  }),
  msg({
    id: 'msg_018', ts: at(115), kind: 'direct', from: 'owl#1', to: 'kite#3',
    body: 'Picked up your evidence from the board, thanks.',
    delivery: { resolved: ['kite#3'] },
  }),
];

export const FLOW_BOARD: BoardView = {
  claims: [
    {
      key: 'route map', key_known: true, owner: 'lynx#1', acquired_at: at(9),
      lease_expires_at: at(69), ended_at: at(19), state: 'released',
    },
    {
      key: 'reset-token reuse', key_known: true, owner: 'kite#3', acquired_at: at(95),
      lease_expires_at: at(155), ended_at: at(110), state: 'released',
    },
    {
      key: 'engagement report', key_known: true, owner: 'owl#1', acquired_at: at(114),
      lease_expires_at: at(174), ended_at: null, state: 'held',
    },
  ],
  directives: [
    {
      id: 'msg_001', author: 'lead', ts: at(2), body: 'Stay inside acme-shop staging. No destructive requests.',
      addressed_to: null, retracted_at: null, retracted: false,
    },
    {
      id: 'msg_013', author: 'lead', ts: at(88),
      body: 'Verify escalation and reset-token reuse. Capture evidence, no data changes.',
      addressed_to: 'role:exploit-verifier', retracted_at: null, retracted: false,
    },
  ],
  journal: true,
};

// --- gannet#1: the workflow the lead generated in round 1 ------------------

const unitRun = (n: number) => `run_01M5GPROBE${n}`;

/** `gannet#1`'s own space: its step agents talk here, not in the flow's. */
export const GANNET_PARTICIPANTS: SpaceParticipant[] = [
  { address: 'probe#5', codename: `${LEAD_CODENAME}>gannet#1>probe#5`, agent: 'auth-prober', run_id: unitRun(5), step_id: 'probe', unit: '5', kind: 'workflow_step', live: false },
  { address: 'probe#6', codename: `${LEAD_CODENAME}>gannet#1>probe#6`, agent: 'auth-prober', run_id: unitRun(6), step_id: 'probe', unit: '6', kind: 'workflow_step', live: false },
  { address: 'probe#7', codename: `${LEAD_CODENAME}>gannet#1>probe#7`, agent: 'auth-prober', run_id: unitRun(7), step_id: 'probe', unit: '7', kind: 'workflow_step', live: false },
  { address: 'review#3', codename: `${LEAD_CODENAME}>gannet#1>review#3`, agent: 'skeptic', run_id: 'run_01M5GREVIEW3', step_id: 'review', unit: '3', kind: 'workflow_step', live: false },
];

export const GANNET_MESSAGES: SpaceMessage[] = [
  msg({ id: 'msg_008', ts: at(34), kind: 'post', post_kind: 'claim', from: 'probe#6', body: 'claim: /api/tokens' }),
  msg({
    id: 'msg_009', ts: at(41), kind: 'direct', from: 'probe#6', to: 'probe#5',
    body: 'Seeing 500s on /api/tokens, are you hitting it too?', delivery: { resolved: ['probe#5'] },
  }),
  msg({
    id: 'msg_010', ts: at(43), kind: 'direct', from: 'probe#5', to: 'probe#6',
    body: 'No, /session is clean. Might be rate limiting.', delivery: { resolved: ['probe#6'] },
  }),
  msg({
    id: 'msg_011', ts: at(60), kind: 'direct', from: 'review#3', to: 'probe#7',
    body: 'Need a request/response pair for the role escalation before I confirm.',
    delivery: { resolved: ['probe#7'] },
  }),
  msg({
    id: 'msg_012', ts: at(66), kind: 'direct', from: 'probe#7', to: 'review#3',
    body: 'Attached: PATCH /admin/roles as a viewer returns 200 and the role sticks.',
    delivery: { resolved: ['review#3'] },
  }),
];

export const GANNET_BOARD: BoardView = {
  claims: [
    {
      key: '/api/tokens', key_known: true, owner: 'probe#6', acquired_at: at(34),
      lease_expires_at: at(94), ended_at: at(56), state: 'released',
    },
  ],
  directives: [],
  journal: true,
};

const PROBED = [
  '/api/login', '/api/logout', '/api/reset', '/api/invite', '/api/session', '/api/tokens',
  '/api/admin/users', '/api/admin/roles', '/api/export', '/api/orders', '/api/cart', '/api/webhooks',
];

function fanout(units: UnitView[]): FanoutState {
  const byState: Record<StepState, number> = {
    pending: 0, running: 0, awaiting_approval: 0, paused: 0, done: 0, failed: 0, skipped: 0,
  };
  for (const u of units) byState[u.state] += 1;
  return { total: units.length, byState, units };
}

const GANNET_NODES: GraphNode[] = [
  { id: 'collect', kind: 'step', agent: 'endpoint-collector', state: 'done' },
  {
    id: 'probe', kind: 'for_each', agent: 'auth-prober', state: 'done',
    fanout: fanout(PROBED.map((key, index) => ({
      index, key, agent: 'auth-prober', state: key === '/api/tokens' ? 'failed' : 'done',
    }))),
  },
  {
    id: 'review', kind: 'panel', state: 'done', round: { current: 2, max: 2 },
    gate: { max_iterations: 2, until_severity: 'high', fix_with: 'auth-prober' },
    fanout: fanout(['auth-reviewer', 'session-reviewer', 'skeptic'].map((agent, index) => ({
      index, key: agent, agent, state: 'done',
    }))),
  },
  {
    id: 'approve', kind: 'gate', state: 'done',
    approval_gate: { auto_approve: false, has_on_reject: false, timeout_seconds: null },
  },
  { id: 'report', kind: 'step', agent: 'finding-writer', state: 'done' },
];

/** `gannet#1`'s run graph as `buildRunGraphModel` would give it. */
export const GANNET_GRAPH: RunGraphModel = {
  nodes: GANNET_NODES,
  edges: [
    { from: 'collect', to: 'probe' },
    { from: 'probe', to: 'review' },
    { from: 'review', to: 'approve' },
    { from: 'approve', to: 'report' },
  ],
  nodeById: (id) => GANNET_NODES.find((n) => n.id === id),
};

export const SPACES: FlowSpace[] = [
  { id: 'flow', participants: FLOW_PARTICIPANTS, messages: FLOW_MESSAGES },
  { id: RUN.gannet1, participants: GANNET_PARTICIPANTS, messages: GANNET_MESSAGES },
];

/** Everything the map is drawn from, fully loaded (gannet#1's frame open). */
export function fixtureInput(): FlowInput {
  return {
    detail: DETAIL,
    children: CHILDREN,
    spaces: SPACES,
    boards: new Map([
      ['flow', FLOW_BOARD],
      [RUN.gannet1, GANNET_BOARD],
    ]),
    childGraphs: new Map([[RUN.gannet1, GANNET_GRAPH]]),
  };
}
