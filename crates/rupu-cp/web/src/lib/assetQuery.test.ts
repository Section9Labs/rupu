// assetQuery — field derivation + client-side evaluation of the asset query,
// built on the shared findings grammar. `depth` is ordered (so `depth>=tested`
// works); every other field is exact (enum) or substring (text).

import { describe, it, expect } from 'vitest';
import { assetFields, assetFacets, filterAssets } from './assetQuery';
import type { AssetRow } from './api';

const A = (over: Partial<AssetRow>): AssetRow => ({
  id: '', ws_id: 'w', project: 'p', target_id: 't', kind: '', profile: '', sub_kind: '', label: '', coords: {}, ...over,
});

const ASSETS: AssetRow[] = [
  A({ id: 'h1', kind: 'network:host', profile: 'network', sub_kind: 'host', label: 'router-a', depth: 'tested', coords: { host: '38.104.174.125' } }),
  A({ id: 'h2', kind: 'network:host', profile: 'network', sub_kind: 'host', label: 'router-b', depth: 'discovered', coords: { host: '38.104.174.126' } }),
  A({ id: 's1', kind: 'network:service', profile: 'network', sub_kind: 'service', label: 'ssh', depth: 'tested', coords: { host: '38.104.174.125', port: 22, proto: 'tcp' } }),
  A({ id: 's2', kind: 'network:service', profile: 'network', sub_kind: 'service', label: 'rdp', depth: 'exploited', coords: { host: '50.0.0.1', port: 3389, proto: 'tcp' } }),
  A({ id: 'm1', kind: 'code:module', profile: 'code', sub_kind: 'module', label: 'auth.go', depth: 'reviewed', coords: { path: 'gateway/auth/auth.go' } }),
];

const FIELDS = assetFields(ASSETS);
const ids = (rows: AssetRow[]) => rows.map((r) => r.id).sort();

describe('assetFields', () => {
  it('derives the field set from the assets, with depth as an ordered rung', () => {
    const depth = FIELDS.find((f) => f.key === 'depth')!;
    expect(depth.kind).toBe('severity'); // ordered → unlocks depth>=…
    expect(depth.values).toEqual(['discovered', 'reviewed', 'tested', 'exploited']); // ladder order, only those present
    expect(FIELDS.find((f) => f.key === 'profile')!.values).toEqual(['code', 'network']);
    expect(FIELDS.find((f) => f.key === 'kind')!.values).toEqual(['host', 'module', 'service']);
    expect(FIELDS.find((f) => f.key === 'kind')!.aliases).toContain('type');
  });
});

describe('assetFacets', () => {
  it('counts values in use per field', () => {
    const f = assetFacets(ASSETS);
    expect(f.profile).toEqual([
      { value: 'network', count: 4 },
      { value: 'code', count: 1 },
    ]);
    expect(Object.fromEntries(f.kind.map((v) => [v.value, v.count]))).toEqual({ host: 2, service: 2, module: 1 });
  });
});

describe('filterAssets', () => {
  const run = (q: string) => filterAssets(ASSETS, q, FIELDS);

  it('returns everything for an empty query', () => {
    expect(run('   ').rows).toHaveLength(5);
  });
  it('filters by exact enum (kind) and substring (host)', () => {
    expect(ids(run('kind:service').rows)).toEqual(['s1', 's2']);
    expect(ids(run('host:38.104').rows)).toEqual(['h1', 'h2', 's1']);
  });
  it('compares depth as an ordered rung', () => {
    // tested(6) and exploited(8) pass; reviewed(5) and discovered(0) do not.
    expect(ids(run('depth>=tested').rows)).toEqual(['h1', 's1', 's2']);
    expect(ids(run('depth:exploited').rows)).toEqual(['s2']);
  });
  it('supports negation and AND of terms', () => {
    expect(ids(run('-kind:host').rows)).toEqual(['m1', 's1', 's2']);
    expect(ids(run('profile:network depth>=tested').rows)).toEqual(['h1', 's1', 's2']);
  });
  it('matches bare text across label and coordinates', () => {
    expect(ids(run('ssh').rows)).toEqual(['s1']);
    expect(ids(run('port:3389').rows)).toEqual(['s2']);
  });
  it('does not filter on a parse error, and reports it', () => {
    const r = run('depth:bogus');
    expect(r.error).toBeTruthy();
    expect(r.rows).toHaveLength(5);
  });
});
