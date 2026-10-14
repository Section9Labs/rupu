import { Hash } from 'lucide-react';
import { describe, expect, it } from 'vitest';
import { filterRows, parseTime, rowFacets, rowField, type RowField } from './rows';

interface Row {
  id: string;
  name: string;
  status: string;
  tools: string[];
  path: string;
  level: string;
  from: string;
  at?: string;
  runs: number;
  broken: boolean;
}

const R = (over: Partial<Row> & { id: string }): Row => ({
  name: over.id,
  status: 'ok',
  tools: [],
  path: '',
  level: 'low',
  from: '',
  runs: 0,
  broken: false,
  ...over,
});

const ROWS: Row[] = [
  R({ id: 'a', name: 'Recon', status: 'Active', tools: ['bash', 'grep'], path: 'src/api/x.ts', level: 'high', from: 'pipit#1', at: '2026-10-14T11:00:00Z', runs: 3 }),
  R({ id: 'b', name: 'service-analyst', status: 'archived', tools: ['bash'], path: 'src/web/y.ts', level: 'medium', from: 'pipit#20', at: new Date(2026, 9, 10, 12).toISOString(), runs: 12 }),
  R({ id: 'c', name: 'exploit-verifier', status: 'active', tools: [], path: 'lib/api/z.ts', level: 'low', from: 'heron', at: '2026-09-01T12:00:00Z', broken: true }),
  R({ id: 'd', name: 'reviewer', status: 'failed', tools: ['grep'], path: 'src/api/w.ts', level: 'critical', from: 'pipit#2' }),
];

const f = (init: Partial<RowField<Row>> & Pick<RowField<Row>, 'key' | 'get' | 'match'>): RowField<Row> =>
  rowField<Row>({ label: init.key, description: init.key, icon: Hash, ...init });

const FIELDS: RowField<Row>[] = [
  f({ key: 'name', get: (r) => r.name, match: 'contains', text: true }),
  f({ key: 'status', aliases: ['is'], get: (r) => r.status, match: 'exact', facet: true }),
  f({ key: 'tool', get: (r) => r.tools, match: 'exact', facet: true }),
  f({ key: 'path', get: (r) => r.path, match: 'prefix', text: true }),
  f({ key: 'level', get: (r) => r.level, match: 'ordered', values: ['low', 'medium', 'high', 'critical'] }),
  f({ key: 'from', get: (r) => r.from, match: 'address' }),
  f({ key: 'runs', get: (r) => r.runs, match: 'exact' }),
  f({ key: 'broken', get: (r) => r.broken, match: 'exact' }),
  f({ key: 'since', get: (r) => r.at, match: 'since' }),
  f({ key: 'until', get: (r) => r.at, match: 'until' }),
];

const NOW = Date.parse('2026-10-14T12:00:00Z');
const ids = (q: string) => {
  const r = filterRows(ROWS, q, FIELDS, { now: NOW });
  expect(r.error).toBeNull();
  return r.rows.map((x) => x.id);
};

describe('filterRows', () => {
  it('returns every row for an empty query', () => {
    expect(ids('  ')).toEqual(['a', 'b', 'c', 'd']);
  });

  it('exact ignores case and matches any of a list', () => {
    expect(ids('status:active')).toEqual(['a', 'c']);
    expect(ids('is:ARCHIVED')).toEqual(['b']);
    expect(ids('tool:grep')).toEqual(['a', 'd']);
    expect(ids('tool:gre')).toEqual([]);
  });

  it('contains and prefix', () => {
    expect(ids('name:ANALYST')).toEqual(['b']);
    expect(ids('path:src/api')).toEqual(['a', 'd']);
    expect(ids('path:api')).toEqual([]);
  });

  it('ordered compares ranks', () => {
    expect(ids('level>=high')).toEqual(['a', 'd']);
    expect(ids('level<medium')).toEqual(['c']);
    expect(ids('level:medium')).toEqual(['b']);
  });

  it('address matches exactly with #, by role word without', () => {
    expect(ids('from:pipit')).toEqual(['a', 'b', 'd']);
    expect(ids('from:pipit#2')).toEqual(['d']);
    expect(ids('from:HERON')).toEqual(['c']);
  });

  it('numbers and booleans match as strings', () => {
    expect(ids('runs:12')).toEqual(['b']);
    expect(ids('broken:true')).toEqual(['c']);
  });

  it('negates, ORs a term’s values and ANDs terms', () => {
    expect(ids('-status:active')).toEqual(['b', 'd']);
    expect(ids('status:archived,failed')).toEqual(['b', 'd']);
    expect(ids('status:active tool:bash')).toEqual(['a']);
    expect(ids('-tool:bash,grep')).toEqual(['c']);
  });

  it('bare words search the text fields', () => {
    expect(ids('recon')).toEqual(['a']);
    expect(ids('lib/')).toEqual(['c']);
    expect(ids('-api')).toEqual(['b']);
    expect(ids('"exploit-ver"')).toEqual(['c']);
  });

  it('bare words search what `text` names instead, when given', () => {
    const r = filterRows(ROWS, 'heron', FIELDS, { text: (x) => [x.from] });
    expect(r.rows.map((x) => x.id)).toEqual(['c']);
  });

  it('since and until take relative times and dates; a row without one never matches', () => {
    expect(ids('since:2h')).toEqual(['a']);
    expect(ids('since:7d')).toEqual(['a', 'b']);
    expect(ids('since:1w')).toEqual(['a', 'b']);
    expect(ids('since:90m')).toEqual(['a']);
    expect(ids('since:2026-10-10')).toEqual(['a', 'b']);
    expect(ids('until:2026-10-10')).toEqual(['b', 'c']);
    expect(ids('until:7d')).toEqual(['c']);
    expect(ids('since:2026-10-09T12:00:00Z until:2026-10-14T10:00:00Z')).toEqual(['b']);
    expect(ids('-since:7d')).toEqual(['c', 'd']);
  });

  it('a time that names none matches nothing and says why', () => {
    const r = filterRows(ROWS, 'since:soon', FIELDS, { now: NOW });
    expect(r.rows).toEqual([]);
    expect(r.error).toBe('`soon` is not a time (e.g. 7d, 12h, 2026-10-01)');
    expect(filterRows(ROWS, 'until:2026-02-30', FIELDS, { now: NOW }).error).toBe('`2026-02-30` is not a time (e.g. 7d, 12h, 2026-10-01)');
  });

  it('a parse error filters nothing and says why', () => {
    const r = filterRows(ROWS, 'colour:red', FIELDS);
    expect(r.rows.map((x) => x.id)).toEqual(['a', 'b', 'c', 'd']);
    expect(r.error).toBe('unknown key `colour` (quote the text to search for it)');
    expect(filterRows(ROWS, 'level:huge', FIELDS).error).toBe('`huge` is not a level (one of low, medium, high, critical)');
  });
});

describe('rowFacets', () => {
  it('counts the facet fields’ values, most used first', () => {
    expect(rowFacets(ROWS, FIELDS)).toEqual({
      status: [
        { value: 'Active', count: 1 },
        { value: 'archived', count: 1 },
        { value: 'active', count: 1 },
        { value: 'failed', count: 1 },
      ],
      tool: [
        { value: 'bash', count: 2 },
        { value: 'grep', count: 2 },
      ],
    });
  });

  it('counts what a facet function names', () => {
    const fields = [f({ key: 'name', get: (r) => r.name, match: 'exact', facet: (r) => r.level })];
    expect(rowFacets(ROWS, fields).name.map((x) => x.value)).toEqual(['high', 'medium', 'low', 'critical']);
  });
});

describe('parseTime', () => {
  it('reads relative units and local dates', () => {
    expect(parseTime('30m', 'since', NOW)).toBe(NOW - 30 * 60_000);
    expect(parseTime('2026-10-01', 'since', NOW)).toBe(new Date(2026, 9, 1).getTime());
    expect(parseTime('2026-10-01', 'until', NOW)).toBe(new Date(2026, 9, 2).getTime() - 1);
    expect(parseTime('yesterday', 'since', NOW)).toBeNull();
  });
});
