import { describe, expect, it } from 'vitest';
import { suggest } from './suggest';
import { parseQuery } from '../../lib/findingQuery/grammar';
import { FINDING_FIELDS } from '../../lib/findingQuery/fields';

const facets = {
  tag: [
    { value: 'class:sqli', count: 5 },
    { value: 'needs-poc', count: 3 },
    { value: 'triaged', count: 1 },
  ],
  severity: [
    { value: 'critical', count: 1 },
    { value: 'high', count: 4 },
    { value: 'medium', count: 0 },
    { value: 'low', count: 2 },
    { value: 'info', count: 0 },
  ],
  owner: [
    { value: 'Payments Team', count: 2 },
    { value: 'Acme, Inc.', count: 1 },
  ],
};

describe('suggest', () => {
  it('lists keys for an empty draft, completing to key:', () => {
    const s = suggest('', FINDING_FIELDS, facets);
    expect(s[0]).toMatchObject({ kind: 'key', label: 'severity', insert: 'severity:', commit: false });
    expect(s.length).toBe(8);
  });
  it('fuzzy-matches keys and offers a text search', () => {
    const s = suggest('ta', FINDING_FIELDS, facets);
    expect(s[0]).toMatchObject({ kind: 'key', label: 'tag', insert: 'tag:' });
    expect(s[s.length - 1]).toMatchObject({ kind: 'text', insert: 'ta', commit: true });
  });
  it('suggests values in use for a key, most used first, with counts', () => {
    const s = suggest('tag:', FINDING_FIELDS, facets);
    expect(s.map((x) => x.label)).toEqual(['class:sqli', 'needs-poc', 'triaged']);
    expect(s[0]).toMatchObject({ kind: 'value', detail: '5', insert: 'tag:class:sqli', commit: true });
  });
  it('filters values by the partial after the last comma and keeps negation and op', () => {
    const s = suggest('-tag:class:sqli,ne', FINDING_FIELDS, facets);
    expect(s[0]).toMatchObject({ label: 'needs-poc', insert: '-tag:class:sqli,needs-poc' });
    expect(suggest('severity>=hi', FINDING_FIELDS, facets)[0]).toMatchObject({ insert: 'severity>=high' });
  });
  it('enum fields offer their fixed values even when unused', () => {
    expect(suggest('has:', FINDING_FIELDS, facets).map((x) => x.label)).toEqual(['tags', 'report', 'poc', 'cwe']);
  });
  it('quotes values that need it', () => {
    expect(suggest('owner:pay', FINDING_FIELDS, facets)[0].insert).toBe('owner:"Payments Team"');
  });
  it('an unknown key suggests nothing', () => {
    expect(suggest('nope:', FINDING_FIELDS, facets)).toEqual([]);
  });
  it('offers nothing for a comparison with a comma, or on a non-severity key', () => {
    expect(suggest('severity>=high,', FINDING_FIELDS, facets)).toEqual([]);
    expect(suggest('tag>', FINDING_FIELDS, facets)).toEqual([]);
  });
  it('matches the item being typed when it opens a quote', () => {
    expect(suggest('owner:"Pay', FINDING_FIELDS, facets)[0].insert).toBe('owner:"Payments Team"');
  });
  it('does not split on a comma inside an open quote', () => {
    expect(suggest('owner:"Acme, I', FINDING_FIELDS, facets)[0].insert).toBe('owner:"Acme, Inc."');
  });
  it('every committed value insert parses', () => {
    const drafts = ['', 'ta', 'tag:', '-tag:class:sqli,ne', 'severity>=hi', 'has:', 'owner:pay', 'owner:"Pay', 'owner:"Acme, I', 'tag:x,'];
    for (const draft of drafts) {
      for (const s of suggest(draft, FINDING_FIELDS, facets)) {
        if (s.commit && s.kind === 'value') {
          expect(parseQuery(s.insert, FINDING_FIELDS).ok, `${draft} -> ${s.insert}`).toBe(true);
        }
      }
    }
  });
});
