import { describe, it, expect } from 'vitest';
import { validateAgentDraft } from './validate';
import { emptyDraft } from './agentSpec';

function validDraft() {
  const d = emptyDraft();
  d.name = 'security-reviewer';
  d.body = 'You are a reviewer.';
  return d;
}

describe('validateAgentDraft', () => {
  it('errors when name is missing', () => {
    const d = emptyDraft();
    d.body = 'body';
    const { ok, errors } = validateAgentDraft(d);
    expect(ok).toBe(false);
    expect(errors).toContainEqual(expect.objectContaining({ field: 'name' }));
  });

  it('errors when name is not a valid slug', () => {
    const d = validDraft();
    d.name = 'Not A Slug!';
    const { ok, errors } = validateAgentDraft(d);
    expect(ok).toBe(false);
    expect(errors.some((e) => e.field === 'name')).toBe(true);
  });

  it('errors when compactAtPercent is out of [10,95]', () => {
    const low = validDraft();
    low.compactAtPercent = 5;
    expect(validateAgentDraft(low).ok).toBe(false);
    expect(validateAgentDraft(low).errors.some((e) => e.field === 'compactAtPercent')).toBe(true);

    const high = validDraft();
    high.compactAtPercent = 96;
    expect(validateAgentDraft(high).ok).toBe(false);

    const inRange = validDraft();
    inRange.compactAtPercent = 50;
    expect(validateAgentDraft(inRange).errors.some((e) => e.field === 'compactAtPercent')).toBe(false);
  });

  it('errors when an enum-typed field has a value outside its vocab', () => {
    const cases: [keyof ReturnType<typeof validDraft>, string][] = [
      ['auth', 'not-a-real-auth-mode'],
      ['permissionMode', 'nonsense'],
      ['effort', 'ultra'],
      ['contextWindow', '10m'],
      ['outputFormat', 'xml'],
      ['anthropicSpeed', 'slow'],
      ['anthropicContextManagement', 'bogus'],
    ];
    for (const [field, badValue] of cases) {
      const d = validDraft();
      (d as unknown as Record<string, unknown>)[field as string] = badValue;
      const { ok, errors } = validateAgentDraft(d);
      expect(ok).toBe(false);
      expect(errors.some((e) => e.field === field)).toBe(true);
    }
  });

  it('errors when an inline concern severity is outside the vocab', () => {
    const d = validDraft();
    d.concerns = [
      {
        kind: 'inline',
        id: 'c1',
        name: 'Concern',
        description: 'desc',
        severity: 'super-critical' as never,
      },
    ];
    const { ok, errors } = validateAgentDraft(d);
    expect(ok).toBe(false);
    expect(errors.some((e) => e.field.includes('severity'))).toBe(true);
  });

  it('warns on a tool name that is neither builtin nor dotted-MCP', () => {
    const d = validDraft();
    d.tools = ['bash', 'totally-unknown-tool'];
    const { ok, warnings } = validateAgentDraft(d);
    expect(ok).toBe(true);
    expect(warnings.some((w) => w.field === 'tools')).toBe(true);
  });

  it('does not warn on a dotted MCP tool name', () => {
    const d = validDraft();
    d.tools = ['scm.prs.get'];
    const { warnings } = validateAgentDraft(d);
    expect(warnings.some((w) => w.field === 'tools')).toBe(false);
  });

  it('warns when tools is an explicit empty list', () => {
    const d = validDraft();
    d.tools = [];
    const { ok, warnings } = validateAgentDraft(d);
    expect(ok).toBe(true);
    expect(warnings).toContainEqual(
      expect.objectContaining({
        field: 'tools',
        message: expect.stringContaining('full default registry'),
      })
    );
  });

  it('warns when body is empty or whitespace-only, without blocking submit', () => {
    const empty = validDraft();
    empty.body = '';
    const { ok: okEmpty, warnings: warningsEmpty } = validateAgentDraft(empty);
    expect(okEmpty).toBe(true);
    expect(warningsEmpty).toContainEqual(
      expect.objectContaining({ field: 'body', message: expect.stringContaining('system prompt (body) is empty') })
    );

    const whitespace = validDraft();
    whitespace.body = '   \n  ';
    const { ok: okWhitespace, warnings: warningsWhitespace } = validateAgentDraft(whitespace);
    expect(okWhitespace).toBe(true);
    expect(warningsWhitespace.some((w) => w.field === 'body')).toBe(true);
  });

  it('does not warn on body when non-empty', () => {
    const d = validDraft();
    const { warnings } = validateAgentDraft(d);
    expect(warnings.some((w) => w.field === 'body')).toBe(false);
  });

  it('is ok:true with no errors for a fully valid draft', () => {
    const d = validDraft();
    d.provider = 'anthropic';
    d.auth = 'api-key';
    d.model = 'claude-sonnet-4-6';
    d.tools = ['bash', 'read_file', 'scm.prs.get'];
    d.permissionMode = 'ask';
    d.effort = 'medium';
    d.contextWindow = 'default';
    d.outputFormat = 'json';
    d.anthropicSpeed = 'fast';
    d.anthropicContextManagement = 'tool_clearing';
    d.compactAtPercent = 80;
    const { ok, errors } = validateAgentDraft(d);
    expect(errors).toEqual([]);
    expect(ok).toBe(true);
  });
});
