// An invented `acme-shop-auth` workflow run with a message space, for the
// run page's layer tests: three steps (collect → probe, a two-unit
// for_each → report), its events, its space and its board. Times are
// `fixtures.ts`' `at(min)`.

import type { BoardView, RunEvent, RunGraphResponse, SpaceMessage, SpaceParticipant } from '../api';
import type { GraphNode, RunGraphModel } from '../runGraphModel';
import { at } from './fixtures';

export const RUN_ID = 'run_01M5ACMERUN';

const nodes: GraphNode[] = [
  { id: 'collect', kind: 'step', agent: 'endpoint-collector', state: 'done' },
  {
    id: 'probe',
    kind: 'for_each',
    agent: 'auth-prober',
    state: 'done',
    fanout: {
      total: 2,
      byState: { pending: 0, running: 0, awaiting_approval: 0, paused: 0, done: 2, failed: 0, skipped: 0 },
      units: [
        { index: 0, key: '/api/login', agent: 'auth-prober', state: 'done' },
        { index: 1, key: '/api/tokens', agent: 'auth-prober', state: 'done' },
      ],
    },
  },
  { id: 'report', kind: 'step', agent: 'finding-writer', state: 'done' },
];

export const RUN_MODEL: RunGraphModel = {
  nodes,
  edges: [
    { from: 'collect', to: 'probe' },
    { from: 'probe', to: 'report' },
  ],
  nodeById: (id) => nodes.find((n) => n.id === id),
};

export const RUN_POSITIONS = new Map([
  ['collect', { x: 0, y: 0, width: 220, height: 90 }],
  ['probe', { x: 300, y: 0, width: 260, height: 110 }],
  ['report', { x: 640, y: 0, width: 220, height: 90 }],
]);

const base = { run_id: RUN_ID };
export const RUN_EVENTS: RunEvent[] = [
  { ...base, type: 'run_started', event_version: 1, workflow_path: 'acme-shop-auth.yaml', started_at: at(0) },
  { ...base, type: 'step_started', step_id: 'collect', kind: 'step', agent: 'endpoint-collector' },
  {
    ...base, type: 'agent_started', step_id: 'collect', agent: 'endpoint-collector',
    agent_run_id: 'run_01M5ACMECOLLECT', transcript_path: '/t/collect.jsonl',
  },
  { ...base, type: 'step_completed', step_id: 'collect', success: true, duration_ms: 300_000 },
  { ...base, type: 'step_started', step_id: 'probe', kind: 'for_each', agent: 'auth-prober' },
  { ...base, type: 'unit_started', step_id: 'probe', index: 0, unit_key: '/api/login', transcript_path: '/t/p0.jsonl' },
  { ...base, type: 'unit_completed', step_id: 'probe', index: 0, unit_key: '/api/login', success: true, tokens_in: 1, tokens_out: 1 },
  { ...base, type: 'unit_started', step_id: 'probe', index: 1, unit_key: '/api/tokens', transcript_path: '/t/p1.jsonl' },
  { ...base, type: 'unit_completed', step_id: 'probe', index: 1, unit_key: '/api/tokens', success: true, tokens_in: 1, tokens_out: 1 },
  { ...base, type: 'step_completed', step_id: 'probe', success: true, duration_ms: 600_000 },
  { ...base, type: 'step_started', step_id: 'report', kind: 'step', agent: 'finding-writer' },
  { ...base, type: 'step_completed', step_id: 'report', success: true, duration_ms: 300_000 },
  { ...base, type: 'run_completed', status: 'completed', finished_at: at(20) },
];

export const RUN_RECORD: Pick<RunGraphResponse, 'run' | 'step_results' | 'units'> = {
  run: { id: RUN_ID, workflow_name: 'acme-shop-auth', status: 'completed', started_at: at(0), finished_at: at(20) } as RunGraphResponse['run'],
  step_results: [
    { run_id: RUN_ID, step_id: 'collect', success: true, finished_at: at(5) },
    { run_id: RUN_ID, step_id: 'probe', success: true, finished_at: at(15) },
    { run_id: RUN_ID, step_id: 'report', success: true, finished_at: at(20) },
  ],
  units: [
    { step_id: 'probe', index: 0, item: '/api/login', run_id: RUN_ID, transcript_path: '/t/p0.jsonl', output: '', success: true, finished_at: at(12) },
    { step_id: 'probe', index: 1, item: '/api/tokens', run_id: RUN_ID, transcript_path: '/t/p1.jsonl', output: '', success: true, finished_at: at(14) },
  ],
};

export const RUN_PARTICIPANTS: SpaceParticipant[] = [
  // Found by its agent run: this participant names no step.
  { address: 'collect', agent: 'endpoint-collector', run_id: 'run_01M5ACMECOLLECT', kind: 'workflow_step', live: false },
  { address: 'probe#0', agent: 'auth-prober', run_id: 'run_01M5ACMEP0', step_id: 'probe', unit: '0', kind: 'workflow_step', live: false },
  { address: 'probe#1', agent: 'auth-prober', run_id: 'run_01M5ACMEP1', step_id: 'probe', unit: '1', kind: 'workflow_step', live: false },
  { address: 'report', agent: 'finding-writer', run_id: 'run_01M5ACMEREPORT', step_id: 'report', kind: 'workflow_step', live: false },
];

function msg(m: Partial<SpaceMessage> & Pick<SpaceMessage, 'id' | 'kind' | 'from' | 'body'>): SpaceMessage {
  return { ts: at(0), to: null, delivery: { resolved: [] }, seen: [], ...m };
}

export const RUN_MESSAGES: SpaceMessage[] = [
  msg({ id: 'msg_r3', ts: at(3), kind: 'post', post_kind: 'claim', from: 'collect', body: 'claim: package-lock.json' }),
  msg({
    id: 'msg_r4', ts: at(10), kind: 'operator', from: 'operator', to: 'broadcast', body: 'Skip the staging admin routes tonight.',
    delivery: { resolved: ['collect', 'probe#0', 'probe#1', 'report'] },
  }),
  msg({
    id: 'msg_r1', ts: at(11), kind: 'direct', from: 'probe#0', to: 'probe#1', body: '/api/login rate-limits after 5 tries.',
    delivery: { resolved: ['probe#1'] },
  }),
  msg({
    id: 'msg_r2', ts: at(13), kind: 'direct', from: 'probe#1', to: 'report', body: 'Token reuse confirmed on /api/tokens.',
    delivery: { resolved: ['report'] },
  }),
  msg({
    id: 'msg_r5', ts: at(17), kind: 'direct', from: 'report', to: 'ghost#9', body: 'Can you re-check /api/reset?',
    delivery: { resolved: [], undelivered: [{ to: 'ghost#9', reason: 'unknown_participant' }] },
  }),
  msg({
    id: 'msg_r6', ts: at(18), kind: 'direct', from: 'lead', to: 'report', body: 'Wrap up.',
    delivery: { resolved: ['report'] },
  }),
];

export const RUN_BOARD: BoardView = {
  claims: [
    {
      key: 'package-lock.json', key_known: true, owner: 'collect', acquired_at: at(2),
      lease_expires_at: at(62), ended_at: at(5), state: 'released',
    },
  ],
  directives: [],
  journal: true,
};

/** The run as `GET /api/runs/:id/graph` serves it. */
export const RUN_GRAPH_RESPONSE: RunGraphResponse = {
  ...RUN_RECORD,
  workflow: {
    steps: [
      { id: 'collect', kind: 'step', agent: 'endpoint-collector' },
      { id: 'probe', kind: 'for_each', agent: 'auth-prober', for_each: '{{ endpoints }}' },
      { id: 'report', kind: 'step', agent: 'finding-writer' },
    ],
    edges: [
      { from: 'collect', to: 'probe' },
      { from: 'probe', to: 'report' },
    ],
  },
} as RunGraphResponse;
