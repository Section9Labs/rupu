import { describe, expect, it } from 'vitest';
import { summarizeTagResult } from './tagResult';

const proj = (ws: string) => ({ ws1: 'shop-web', ws2: 'billing-api' })[ws] ?? ws;

describe('summarizeTagResult', () => {
  it('counts changed and unchanged findings', () => {
    const r = {
      workspaces: [{ ws_id: 'ws1', outcomes: [
        { finding_id: 'a', before: [], after: ['x'] },
        { finding_id: 'b', before: ['x'], after: ['x'] },
      ] }],
      unknown: [],
    };
    expect(summarizeTagResult(r, 'add', proj)).toEqual({ message: 'Tagged 1 finding (1 already had it).', ok: true });
  });
  it('names failed projects and missing findings', () => {
    const r = {
      workspaces: [
        { ws_id: 'ws1', outcomes: [{ finding_id: 'a', before: ['x'], after: [] }] },
        { ws_id: 'ws2', error: 'finding-tag log I/O: Is a directory' },
      ],
      unknown: ['gone1', 'gone2'],
    };
    expect(summarizeTagResult(r, 'remove', proj)).toEqual({
      message: "Untagged 1 finding. billing-api's tags couldn't be changed: finding-tag log I/O: Is a directory. 2 findings no longer exist.",
      ok: false,
    });
  });
});
