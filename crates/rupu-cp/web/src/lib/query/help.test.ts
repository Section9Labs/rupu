import { describe, expect, it } from 'vitest';
import { FINDING_FIELDS } from '../findingQuery/fields';
import { SKILL_FIELDS } from './lists/skills';
import { SESSION_FIELDS } from './lists/sessions';
import { WORKFLOW_RUN_FIELDS } from './lists/workflowRuns';
import { helpRows, serverListOf } from './help';

describe('serverListOf', () => {
  it('names the server list of a registry the server filters with', () => {
    expect(serverListOf(FINDING_FIELDS)).toBe('findings');
    expect(serverListOf(WORKFLOW_RUN_FIELDS)).toBe('workflow_runs');
    expect(serverListOf(SESSION_FIELDS)).toBe('sessions');
  });

  it('has none for a list the browser filters', () => {
    expect(serverListOf(SKILL_FIELDS)).toBeNull();
  });
});

describe('helpRows', () => {
  it('takes keys, kinds and values from the server, and the example from the local field', () => {
    const rows = helpRows(FINDING_FIELDS, [
      { key: 'severity', aliases: ['sev'], kind: 'severity', values: ['info', 'low', 'medium', 'high', 'critical'] },
      { key: 'new_key', aliases: [], kind: 'enum', values: ['a', 'b'] },
    ]);
    expect(rows).toEqual([
      {
        key: 'severity',
        aliases: ['sev'],
        kind: 'ordered · supports >= <=',
        values: ['info', 'low', 'medium', 'high', 'critical'],
        example: 'severity:high · severity>=high',
      },
      { key: 'new_key', aliases: [], kind: 'one of', values: ['a', 'b'], example: 'new_key:a' },
    ]);
  });

  it('describes time and text fields with a made-up example when the local registry has none', () => {
    const rows = helpRows([], [
      { key: 'since', aliases: [], kind: 'time', values: [] },
      { key: 'owner', aliases: [], kind: 'text', values: [] },
      { key: 'tag', aliases: [], kind: 'tag', values: [] },
    ]);
    expect(rows.map((r) => [r.key, r.kind, r.example])).toEqual([
      ['since', 'time · 30m, 12h, 7d, 2w, a date or RFC 3339', 'since:7d'],
      ['owner', 'text', 'owner:…'],
      ['tag', 'tag', 'tag:…'],
    ]);
  });

  it('reads the local registry when there is no server one', () => {
    const rows = helpRows(SKILL_FIELDS, null);
    expect(rows.map((r) => r.key)).toEqual(SKILL_FIELDS.map((f) => f.key));
    expect(rows[0].example).toBe(SKILL_FIELDS[0].description);
  });
});
