// @vitest-environment jsdom
/** Tests for ConnectorCard — scm/issues/github/gitlab, using real payloads. */

import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { parseConnector, interpretConnectorError, ConnectorSummary, ConnectorBody } from './ConnectorCard';
import type { ToolView } from './transcriptView';

afterEach(cleanup);

function tv(tool: string, overrides: Partial<ToolView>): ToolView {
  return { tool, kind: 'connector', input: {}, ...overrides };
}

describe('parseConnector — target + outcome', () => {
  it('scm.repos.get target is owner/repo', () => {
    const v = parseConnector(tv('scm.repos.get', { input: { owner: 'Section9Labs', repo: 'rupu', platform: 'github' }, output: '{"default_branch":"main","private":false,"description":"d","clone_url_https":"https://x"}' }));
    expect(v.ns).toBe('scm');
    expect(v.verb).toBe('repos.get');
    expect(v.target.repo).toBe('Section9Labs/rupu');
    expect(v.target.platform).toBe('github');
    expect(v.outcome).toBeNull();
  });

  it('issues.get target is project + number', () => {
    const v = parseConnector(tv('issues.get', { input: { number: 3, project: 'Oracle-Innovation/project-chimera', tracker: 'github' }, output: '{"title":"Campaign","body":"## Objective\\n\\ndo it"}' }));
    expect(v.target.project).toBe('Oracle-Innovation/project-chimera');
    expect(v.target.number).toBe(3);
  });

  it('create → created, update_state → the state', () => {
    expect(parseConnector(tv('scm.prs.create', { input: { base: 'main' }, output: '{"number":5}' })).outcome).toEqual({ label: 'created', tone: 'ok' });
    expect(parseConnector(tv('issues.update_state', { input: { number: 20, state: 'closed' }, output: '{}' })).outcome).toEqual({ label: 'closed', tone: 'neutral' });
    expect(parseConnector(tv('issues.comment', { input: { number: 27 }, output: '{"id":"5","author":"x"}' })).outcome).toEqual({ label: 'commented', tone: 'ok' });
  });

  it('a still-running create has no outcome chip (keeps the running badge)', () => {
    // No output and no error = in flight; must NOT show a premature "created".
    expect(parseConnector(tv('scm.prs.create', { input: { base: 'main' } })).outcome).toBeNull();
    expect(parseConnector(tv('issues.comment', { input: { number: 1 } })).outcome).toBeNull();
  });
});

describe('interpretConnectorError', () => {
  it('owner/repo shape', () => {
    const e = interpretConnectorError('execution: tool dispatch failed: bad request: project must be `owner/repo`: mint-kingdom');
    expect(e.friendly).toContain('owner/repo');
    expect(e.friendly).toContain('mint-kingdom');
  });

  it('account resolution', () => {
    const e = interpretConnectorError('execution: account resolution failed: no account rule matches (github lookup)');
    expect(e.friendly).toContain('No account is configured');
  });
});

describe('render', () => {
  it('summary shows repo, number, platform, outcome', () => {
    const v = parseConnector(tv('issues.update_state', { input: { owner: 'Section9Labs', repo: 'rupu-sandbox-gh', number: 20, state: 'closed' }, output: '{}' }));
    render(<ConnectorSummary view={v} />);
    expect(screen.getByText('Section9Labs/rupu-sandbox-gh')).toBeTruthy();
    expect(screen.getByText('#20')).toBeTruthy();
    expect(screen.getByText('closed')).toBeTruthy();
  });

  it('issues.get body renders title + markdown', () => {
    const v = parseConnector(tv('issues.get', { input: { number: 3, project: 'o/p' }, output: '{"title":"Campaign: console surface","body":"Establish the console surface."}' }));
    render(<ConnectorBody view={v} />);
    expect(screen.getByText('Campaign: console surface')).toBeTruthy();
    expect(screen.getByText(/Establish the/)).toBeTruthy();
  });

  it('error body renders the friendly note, not a raw blob', () => {
    const v = parseConnector(tv('issues.list', { input: { project: 'mint-kingdom' }, error: 'execution: tool dispatch failed: bad request: project must be `owner/repo`: mint-kingdom' }));
    render(<ConnectorBody view={v} />);
    expect(screen.getAllByText(/owner\/repo/).length).toBeGreaterThan(0);
    expect(screen.getByText(/the agent sent/)).toBeTruthy();
  });
});
