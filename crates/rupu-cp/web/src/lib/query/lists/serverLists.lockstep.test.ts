// The web half of the list-query lockstep: each server-filtered registry
// must equal the server's, as `crates/rupu-cp/tests/fixtures/list_query/`
// records it. Fix the registry, never the fixture.
import { describe, expect, it } from 'vitest';
import type { ServerField } from '../rows';
import { WORKFLOW_RUN_FIELDS } from './workflowRuns';
import { AUTOFLOW_RUN_FIELDS } from './autoflowRuns';
import { AGENT_RUN_FIELDS } from './agentRuns';
import { SESSION_FIELDS } from './sessions';
import workflowRuns from '../../../../../tests/fixtures/list_query/workflow_runs.fields.json';
import autoflowRuns from '../../../../../tests/fixtures/list_query/autoflow_runs.fields.json';
import agentRuns from '../../../../../tests/fixtures/list_query/agent_runs.fields.json';
import sessions from '../../../../../tests/fixtures/list_query/sessions.fields.json';

const spec = (fs: readonly ServerField[]) =>
  fs.map(({ key, aliases, kind, values, match, text, facet }) => ({ aliases, facet, key, kind, match, text, values }));

describe('server list registries match the server fixtures', () => {
  it.each([
    ['workflow_runs', WORKFLOW_RUN_FIELDS, workflowRuns],
    ['autoflow_runs', AUTOFLOW_RUN_FIELDS, autoflowRuns],
    ['agent_runs', AGENT_RUN_FIELDS, agentRuns],
    ['sessions', SESSION_FIELDS, sessions],
  ] as const)('%s', (_name, fields, fixture) => {
    expect(spec(fields)).toEqual(fixture);
  });
});
