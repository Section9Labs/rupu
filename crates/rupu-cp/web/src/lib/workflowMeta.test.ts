// @vitest-environment node
import { describe, it, expect } from 'vitest';
import {
  readTrigger,
  writeTrigger,
  readInputs,
  writeInputs,
  readAutoflow,
  writeAutoflow,
  contractOutputKeys,
  type TriggerModel,
  type InputModel,
  type AutoflowModel,
} from './workflowMeta';

// ── trigger ──────────────────────────────────────────────────────────────────

describe('readTrigger', () => {
  it('defaults to manual when absent', () => {
    expect(readTrigger({})).toEqual({ on: 'manual' });
  });

  it('defaults to manual when trigger is malformed', () => {
    expect(readTrigger({ trigger: 'nope' })).toEqual({ on: 'manual' });
    expect(readTrigger({ trigger: { on: 'weekly' } })).toEqual({ on: 'manual' });
  });

  it('reads a cron trigger', () => {
    expect(readTrigger({ trigger: { on: 'cron', cron: '0 * * * *' } })).toEqual({
      on: 'cron',
      cron: '0 * * * *',
    });
  });

  it('reads an event trigger with filter', () => {
    expect(
      readTrigger({ trigger: { on: 'event', event: 'github.pr.opened', filter: "{{event.repo == 'rupu'}}" } }),
    ).toEqual({ on: 'event', event: 'github.pr.opened', filter: "{{event.repo == 'rupu'}}" });
  });
});

describe('writeTrigger', () => {
  it('round-trips a stable model (write(read(rest)) === rest for canonical rest)', () => {
    const rest = { trigger: { on: 'cron', cron: '*/5 * * * *' } };
    const model = readTrigger(rest);
    expect(writeTrigger(rest, model)).toEqual(rest);
  });

  it('round-trips an event trigger with filter', () => {
    const rest = { trigger: { on: 'event', event: 'github.issue.created', filter: 'x' } };
    const model = readTrigger(rest);
    expect(writeTrigger(rest, model)).toEqual(rest);
  });

  it('event mode emits on/event/filter and never cron', () => {
    const model: TriggerModel = { on: 'event', event: 'github.pr.opened', filter: 'f' };
    const out = writeTrigger({}, model);
    expect(out.trigger).toEqual({ on: 'event', event: 'github.pr.opened', filter: 'f' });
    expect((out.trigger as Record<string, unknown>).cron).toBeUndefined();
  });

  it('manual trigger deletes the trigger key', () => {
    const rest = { trigger: { on: 'cron', cron: '* * * * *' }, defaults: { foo: 1 } };
    const out = writeTrigger(rest, { on: 'manual' });
    expect('trigger' in out).toBe(false);
    expect(out.defaults).toEqual({ foo: 1 });
  });

  it('preserves key position when updating an existing trigger key', () => {
    const rest = { a: 1, trigger: { on: 'manual' }, b: 2 };
    const out = writeTrigger(rest, { on: 'cron', cron: '* * * * *' });
    expect(Object.keys(out)).toEqual(['a', 'trigger', 'b']);
  });

  it('appends trigger at the end when adding a new key', () => {
    const rest = { a: 1, b: 2 };
    const out = writeTrigger(rest, { on: 'cron', cron: '* * * * *' });
    expect(Object.keys(out)).toEqual(['a', 'b', 'trigger']);
  });

  it('does not mutate the input rest', () => {
    const rest = { trigger: { on: 'manual' } };
    const frozen = JSON.parse(JSON.stringify(rest));
    writeTrigger(rest, { on: 'cron', cron: '* * * * *' });
    expect(rest).toEqual(frozen);
  });
});

// ── inputs ───────────────────────────────────────────────────────────────────

describe('readInputs', () => {
  it('returns [] when absent', () => {
    expect(readInputs({})).toEqual([]);
  });

  it('parses the name-keyed map shape (type/enum, not ty/allowed)', () => {
    const rest = {
      inputs: {
        target_branch: { type: 'string', required: true, default: 'main', description: 'branch to target' },
        max_retries: { type: 'int', required: false, default: 3 },
        confirm: { type: 'bool', required: false, enum: ['true', 'false'] },
      },
    };
    const inputs = readInputs(rest);
    expect(inputs).toEqual([
      {
        name: 'target_branch',
        type: 'string',
        required: true,
        default: 'main',
        enumValues: [],
        description: 'branch to target',
      },
      { name: 'max_retries', type: 'int', required: false, default: 3, enumValues: [] },
      { name: 'confirm', type: 'bool', required: false, enumValues: ['true', 'false'] },
    ]);
  });

  it('defaults type to string and required to false when absent', () => {
    const inputs = readInputs({ inputs: { x: {} } });
    expect(inputs).toEqual([{ name: 'x', type: 'string', required: false, enumValues: [] }]);
  });
});

describe('writeInputs', () => {
  it('round-trips the map shape via readInputs', () => {
    const rest = {
      inputs: {
        a: { type: 'string', required: true, enum: ['x', 'y'] },
        b: { type: 'int', required: false, default: 5 },
      },
    };
    const model = readInputs(rest);
    expect(writeInputs({}, model)).toEqual(rest);
  });

  it('emits type:/enum: keys, never ty:/allowed:', () => {
    const model: InputModel[] = [{ name: 'x', type: 'string', required: true, enumValues: ['a', 'b'] }];
    const out = writeInputs({}, model);
    const x = (out.inputs as Record<string, unknown>).x as Record<string, unknown>;
    expect(x.type).toBe('string');
    expect(x.enum).toEqual(['a', 'b']);
    expect('ty' in x).toBe(false);
    expect('allowed' in x).toBe(false);
  });

  it('omits empty enum and undefined default', () => {
    const model: InputModel[] = [{ name: 'x', type: 'string', required: false, enumValues: [] }];
    const out = writeInputs({}, model);
    const x = (out.inputs as Record<string, unknown>).x as Record<string, unknown>;
    expect('enum' in x).toBe(false);
    expect('default' in x).toBe(false);
  });

  it('empty inputs array omits the inputs key', () => {
    const out = writeInputs({ inputs: { a: { type: 'string', required: false } } }, []);
    expect('inputs' in out).toBe(false);
  });

  it('preserves an unrelated rest.contracts key and its position', () => {
    const rest = { contracts: { outputs: { verdict: {} } }, inputs: {} };
    const model = readInputs({ inputs: { a: { type: 'string', required: false } } });
    const out = writeInputs(rest, model);
    expect(Object.keys(out)).toEqual(['contracts', 'inputs']);
    expect(out.contracts).toEqual({ outputs: { verdict: {} } });
  });

  it('does not mutate the input rest', () => {
    const rest = { inputs: { a: { type: 'string', required: false } } };
    const frozen = JSON.parse(JSON.stringify(rest));
    writeInputs(rest, []);
    expect(rest).toEqual(frozen);
  });
});

// ── autoflow ─────────────────────────────────────────────────────────────────

const FULL_AUTOFLOW_RAW = {
  enabled: true,
  entity: 'pull_request',
  source: 'github',
  priority: 10,
  selector: {
    states: ['open'],
    labels_all: ['ready'],
    labels_any: ['bug', 'feature'],
    labels_none: ['blocked'],
    limit: 5,
    draft: 'exclude',
    base: 'main',
    authors: ['octocat'],
    authors_from: 'collaborators',
    on_skip: 'label_needs_human',
  },
  wake_on: ['pr.opened', 'pr.synchronize'],
  reconcile_every: '15m',
  claim: { key: 'pr_head_sha', ttl: '1h' },
  workspace: { strategy: 'worktree', branch: 'autoflow/{{issue.number}}' },
  outcome: { output: 'verdict' },
};

describe('readAutoflow', () => {
  it('returns null when absent', () => {
    expect(readAutoflow({})).toBeNull();
  });

  it('parses the full shape including selector/claim/workspace/outcome', () => {
    const model = readAutoflow({ autoflow: FULL_AUTOFLOW_RAW });
    expect(model).toEqual({
      enabled: true,
      entity: 'pull_request',
      source: 'github',
      priority: 10,
      selector: {
        states: ['open'],
        labels_all: ['ready'],
        labels_any: ['bug', 'feature'],
        labels_none: ['blocked'],
        limit: 5,
        draft: 'exclude',
        base: 'main',
        authors: ['octocat'],
        authors_from: 'collaborators',
        on_skip: 'label_needs_human',
      },
      wake_on: ['pr.opened', 'pr.synchronize'],
      reconcile_every: '15m',
      claim: { key: 'pr_head_sha', ttl: '1h' },
      workspace: { strategy: 'worktree', branch: 'autoflow/{{issue.number}}' },
      outcome: { output: 'verdict' },
    });
  });

  it('defaults enabled/entity/selector/wake_on when block is minimal', () => {
    expect(readAutoflow({ autoflow: {} })).toEqual({
      enabled: false,
      entity: 'issue',
      selector: {},
      wake_on: [],
    });
  });
});

describe('writeAutoflow', () => {
  it('round-trips the full shape via readAutoflow', () => {
    const rest = { autoflow: FULL_AUTOFLOW_RAW };
    const model = readAutoflow(rest);
    expect(writeAutoflow({}, model)).toEqual(rest);
  });

  it('null omits the autoflow key', () => {
    const out = writeAutoflow({ autoflow: FULL_AUTOFLOW_RAW }, null);
    expect('autoflow' in out).toBe(false);
  });

  it('disabling autoflow (empty/default model) omits the key', () => {
    const emptyDisabled: AutoflowModel = {
      enabled: false,
      entity: 'issue',
      selector: {},
      wake_on: [],
    };
    const out = writeAutoflow({ autoflow: FULL_AUTOFLOW_RAW }, emptyDisabled);
    expect('autoflow' in out).toBe(false);
  });

  it('a disabled model that still carries configuration is preserved with enabled:false', () => {
    const disabledButConfigured: AutoflowModel = {
      enabled: false,
      entity: 'pull_request',
      selector: { labels_all: ['ready'] },
      wake_on: ['pr.opened'],
    };
    const out = writeAutoflow({}, disabledButConfigured);
    expect(out.autoflow).toEqual({
      enabled: false,
      entity: 'pull_request',
      selector: { labels_all: ['ready'] },
      wake_on: ['pr.opened'],
    });
  });

  it('omits an empty selector object entirely', () => {
    const model: AutoflowModel = { enabled: true, entity: 'issue', selector: {}, wake_on: [] };
    const out = writeAutoflow({}, model);
    expect('selector' in (out.autoflow as Record<string, unknown>)).toBe(false);
  });

  it('preserves an unrelated rest.contracts key and its position', () => {
    const rest = { contracts: { outputs: { verdict: {} } }, autoflow: FULL_AUTOFLOW_RAW };
    const model = readAutoflow(rest);
    const out = writeAutoflow(rest, model);
    expect(Object.keys(out)).toEqual(['contracts', 'autoflow']);
    expect(out.contracts).toEqual({ outputs: { verdict: {} } });
  });

  it('does not mutate the input rest', () => {
    const rest = { autoflow: FULL_AUTOFLOW_RAW };
    const frozen = JSON.parse(JSON.stringify(rest));
    writeAutoflow(rest, null);
    expect(rest).toEqual(frozen);
  });
});

// ── contractOutputKeys ────────────────────────────────────────────────────────

describe('contractOutputKeys', () => {
  it('returns [] when contracts is absent', () => {
    expect(contractOutputKeys({})).toEqual([]);
  });

  it('returns [] when contracts.outputs is absent or malformed', () => {
    expect(contractOutputKeys({ contracts: {} })).toEqual([]);
    expect(contractOutputKeys({ contracts: { outputs: 'nope' } })).toEqual([]);
  });

  it('returns the contract output names', () => {
    expect(
      contractOutputKeys({ contracts: { outputs: { verdict: { type: 'string' }, score: { type: 'int' } } } }),
    ).toEqual(['verdict', 'score']);
  });

  it('does not modify contracts', () => {
    const rest = { contracts: { outputs: { verdict: {} } } };
    const frozen = JSON.parse(JSON.stringify(rest));
    contractOutputKeys(rest);
    expect(rest).toEqual(frozen);
  });
});
