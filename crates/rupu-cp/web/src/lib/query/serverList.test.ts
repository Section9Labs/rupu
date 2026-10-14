import { describe, expect, it } from 'vitest';
import { hostFacet, hostNotes, hostPickOf, mergeFacets, noteText } from './serverList';
import { WORKFLOW_RUN_FIELDS } from './lists/workflowRuns';
import { emptySlice, type HostSlice } from '../perHost/types';

const LOCAL = { id: 'local', name: 'Local', transport_kind: 'local' };
const PROD = { id: 'host_prod', name: 'prod', transport_kind: 'http_cp' };
const MINI = { id: 'mini', name: 'mini', transport_kind: 'ssh' };
const slice = (h: typeof LOCAL, over: Partial<HostSlice<unknown>>): HostSlice<unknown> => ({ ...emptySlice(h), ...over });

describe('hostPickOf', () => {
  it('reads positive host terms as the hosts to ask and negated ones as hosts to leave out', () => {
    expect(hostPickOf('status:failed', WORKFLOW_RUN_FIELDS)).toBeNull();
    expect(hostPickOf('host:local,Mini', WORKFLOW_RUN_FIELDS)).toEqual({ include: ['local', 'mini'], exclude: [] });
    expect(hostPickOf('-host:mini', WORKFLOW_RUN_FIELDS)).toEqual({ include: null, exclude: ['mini'] });
    expect(hostPickOf('host:local,mini host:mini', WORKFLOW_RUN_FIELDS)).toEqual({ include: ['mini'], exclude: [] });
    expect(hostPickOf('host:local -host:local', WORKFLOW_RUN_FIELDS)).toEqual({ include: ['local'], exclude: ['local'] });
  });

  it('asks every host while the query doesn’t parse', () => {
    expect(hostPickOf('host:mini colour:red', WORKFLOW_RUN_FIELDS)).toBeNull();
  });
});

describe('mergeFacets', () => {
  it('sums every host’s counts, most used first', () => {
    expect(
      mergeFacets([
        { status: { failed: 2, running: 1 }, host: { local: 3 } },
        { status: { running: 4 }, host: { mini: 4 } },
      ]),
    ).toEqual({
      status: [
        { value: 'running', count: 5 },
        { value: 'failed', count: 2 },
      ],
      host: [
        { value: 'mini', count: 4 },
        { value: 'local', count: 3 },
      ],
    });
  });
});

describe('hostFacet', () => {
  it('offers every registered host with its name, state and count', () => {
    const slices = [slice(LOCAL, { state: 'ok' }), slice(PROD, { state: 'offline' })];
    expect(hostFacet([LOCAL, PROD, MINI], slices, [{ value: 'local', count: 9 }])).toEqual([
      { value: 'local', count: 9, detail: 'Local · live' },
      { value: 'host_prod', count: undefined, detail: 'prod · offline' },
      { value: 'mini', count: undefined, detail: 'not queried' },
    ]);
  });
});

describe('hostNotes', () => {
  it('names each host whose rows are missing, once, with why', () => {
    const slices = [
      slice(LOCAL, { state: 'ok', pagingFailed: true }),
      slice(PROD, { state: 'offline' }),
      slice(MINI, { state: 'unavailable', reason: "host mini can't report a customer for every run" }),
    ];
    const notes = hostNotes(slices, [{ withoutCustomer: ['mini'], unfilterable: [{ host: 'old', field: 'status' }, { host: 'ssh', field: 'is' }] }], [LOCAL, PROD, MINI]);
    expect(notes.map((n) => noteText(n, 'runs'))).toEqual([
      "Local: older runs couldn't load",
      'prod offline: its runs are missing',
      "mini can't filter by customer: its runs are left out",
      "old can't filter by status: its runs are left out",
      "ssh can't list archived runs: they are left out",
    ]);
  });

  it('reads a 501 reason for the field it names', () => {
    const slices = [
      slice(PROD, { state: 'unavailable', reason: "host host_prod can't report `model` for every row" }),
      slice(MINI, { state: 'unavailable', reason: "host mini can't list archived runs" }),
      slice(LOCAL, { state: 'unavailable', reason: 'not implemented' }),
    ];
    expect(hostNotes(slices, [], []).map((n) => noteText(n, 'sessions'))).toEqual([
      "prod can't filter by model: its sessions are left out",
      "mini can't list archived sessions: they are left out",
      "Local can't serve this list: its sessions are missing",
    ]);
  });
});
