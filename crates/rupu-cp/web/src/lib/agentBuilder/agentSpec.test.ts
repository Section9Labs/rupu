import { describe, it, expect } from 'vitest';
import yaml from 'js-yaml';
import {
  serializeAgent,
  parseAgent,
  emptyDraft,
  PROVIDERS,
  AUTH_MODES,
  PERMISSION_MODES,
  EFFORT_LEVELS,
  CONTEXT_WINDOWS,
  OUTPUT_FORMATS,
  ANTHROPIC_SPEED,
  ANTHROPIC_CTX_MGMT,
  BUILTIN_TOOLS,
} from './agentSpec';

describe('agentSpec', () => {
  it('serializes only present keys, name first, body after fences', () => {
    const d = emptyDraft();
    d.name = 'security-reviewer';
    d.provider = 'anthropic';
    d.model = 'claude-sonnet-4-6';
    d.tools = ['read_file', 'grep', 'scm.prs.get'];
    d.permissionMode = 'readonly';
    d.maxTurns = 10;
    d.outputFormat = 'json';
    d.outputSchema = [
      { name: 'severity', type: 'enum', enumValues: ['low', 'high'] },
      { name: 'title', type: 'string' },
    ];
    d.body = 'You are a reviewer.';
    const md = serializeAgent(d);
    expect(md.startsWith('---\nname: security-reviewer')).toBe(true);
    expect(md).toContain('permissionMode: readonly');
    expect(md).toContain('tools:'); // list emitted
    expect(md).not.toContain('description:'); // empty key omitted
    expect(md.trim().endsWith('You are a reviewer.')).toBe(true);
  });

  it('round-trips parse(serialize(d)) preserving modeled fields', () => {
    const d = emptyDraft();
    d.name = 'x';
    d.effort = 'high';
    d.dispatchableAgents = ['code-reviewer'];
    d.concerns = [{ kind: 'include', ref: 'owasp', overrides: ['mode=full'] }];
    d.body = 'body text';
    const back = parseAgent(serializeAgent(d));
    expect(back.name).toBe('x');
    expect(back.effort).toBe('high');
    expect(back.dispatchableAgents).toEqual(['code-reviewer']);
    expect(back.concerns?.[0]).toMatchObject({ kind: 'include', ref: 'owasp' });
    expect(back.body).toBe('body text');
  });

  it('parse preserves unknown keys via passthrough and re-emits them', () => {
    const raw = '---\nname: y\nsomeFutureKey: 42\n---\n\nb';
    const back = parseAgent(raw);
    expect(back.name).toBe('y');
    expect(serializeAgent(back)).toContain('someFutureKey: 42');
  });

  it('emptyDraft has no present fields and serializes to just name + body', () => {
    const d = emptyDraft();
    expect(d.body).toBe('');
    d.name = 'bare';
    const md = serializeAgent(d);
    expect(md).toBe('---\nname: bare\n---\n\n\n');
  });

  it('serializes outputSchema as JSON-schema mapping with enum props', () => {
    const d = emptyDraft();
    d.name = 'a';
    d.outputSchema = [
      { name: 'severity', type: 'enum', enumValues: ['low', 'high'] },
      { name: 'title', type: 'string' },
      { name: 'count', type: 'number' },
    ];
    const md = serializeAgent(d);
    const { frontmatter } = splitFm(md);
    const parsed = yaml.load(frontmatter) as Record<string, unknown>;
    expect(parsed.outputSchema).toEqual({
      type: 'object',
      additionalProperties: false,
      required: ['severity', 'title', 'count'],
      properties: {
        severity: { type: 'string', enum: ['low', 'high'] },
        title: { type: 'string' },
        count: { type: 'number' },
      },
    });
  });

  it('round-trips outputSchema through parse', () => {
    const d = emptyDraft();
    d.name = 'a';
    d.outputSchema = [
      { name: 'severity', type: 'enum', enumValues: ['low', 'high'] },
      { name: 'title', type: 'string' },
    ];
    const back = parseAgent(serializeAgent(d));
    expect(back.outputSchema).toEqual([
      { name: 'severity', type: 'enum', enumValues: ['low', 'high'] },
      { name: 'title', type: 'string' },
    ]);
  });

  it('serializes concerns: include and inline entries to the include/inline shape', () => {
    const d = emptyDraft();
    d.name = 'a';
    d.concerns = [
      { kind: 'include', ref: 'owasp', overrides: ['mode=full'] },
      { kind: 'inline', ref: 'no-secrets', globs: '**/*.ts' },
    ];
    const md = serializeAgent(d);
    const { frontmatter } = splitFm(md);
    const parsed = yaml.load(frontmatter) as Record<string, unknown>;
    expect(parsed.concerns).toEqual([
      { include: 'owasp', overrides: ['mode=full'] },
      { id: 'no-secrets', applicable_globs: '**/*.ts' },
    ]);
  });

  it('round-trips concerns include/inline through parse', () => {
    const d = emptyDraft();
    d.name = 'a';
    d.concerns = [
      { kind: 'include', ref: 'owasp' },
      { kind: 'inline', ref: 'no-secrets', globs: '**/*.ts' },
    ];
    const back = parseAgent(serializeAgent(d));
    expect(back.concerns).toEqual([
      { kind: 'include', ref: 'owasp' },
      { kind: 'inline', ref: 'no-secrets', globs: '**/*.ts' },
    ]);
  });

  it('preserves body verbatim (no templating)', () => {
    const d = emptyDraft();
    d.name = 'a';
    d.body = 'line one\n\nline two with {{ not a template }}\n';
    const back = parseAgent(serializeAgent(d));
    expect(back.body).toBe(d.body.replace(/\n+$/, ''));
  });

  it('never emits an unmodeled key from the frontmatter allowlist', () => {
    const d = emptyDraft();
    d.name = 'a';
    d.description = 'desc';
    d.provider = 'anthropic';
    d.auth = 'api-key';
    d.model = 'claude-sonnet-4-6';
    d.tools = ['read_file'];
    d.maxTurns = 5;
    d.permissionMode = 'ask';
    d.anthropicOauthPrefix = false;
    d.effort = 'medium';
    d.contextWindow = 'default';
    d.outputFormat = 'text';
    d.anthropicTaskBudget = 100;
    d.anthropicContextManagement = 'tool_clearing';
    d.anthropicSpeed = 'fast';
    d.dispatchableAgents = ['x'];
    d.maxTokens = 4096;
    d.contextWindowTokens = 100000;
    d.compactAtPercent = 80;
    const md = serializeAgent(d);
    const { frontmatter } = splitFm(md);
    const parsed = yaml.load(frontmatter) as Record<string, unknown>;
    const allowed = new Set([
      'name',
      'description',
      'provider',
      'auth',
      'model',
      'tools',
      'maxTurns',
      'permissionMode',
      'anthropicOauthPrefix',
      'effort',
      'contextWindow',
      'outputFormat',
      'outputSchema',
      'anthropicTaskBudget',
      'anthropicContextManagement',
      'anthropicSpeed',
      'dispatchableAgents',
      'concerns',
      'maxTokens',
      'contextWindowTokens',
      'compactAtPercent',
    ]);
    for (const key of Object.keys(parsed)) {
      expect(allowed.has(key)).toBe(true);
    }
  });

  it('exports the expected vocab consts', () => {
    expect(PROVIDERS).toContain('anthropic');
    expect(AUTH_MODES).toContain('api-key');
    expect(PERMISSION_MODES).toEqual(expect.arrayContaining(['ask', 'bypass', 'readonly']));
    expect(EFFORT_LEVELS).toEqual(
      expect.arrayContaining(['auto', 'minimal', 'low', 'medium', 'high', 'max'])
    );
    expect(CONTEXT_WINDOWS.length).toBeGreaterThan(0);
    expect(OUTPUT_FORMATS).toEqual(expect.arrayContaining(['text', 'json']));
    expect(ANTHROPIC_SPEED).toContain('fast');
    expect(ANTHROPIC_CTX_MGMT).toContain('tool_clearing');
    expect(BUILTIN_TOOLS).toEqual(
      expect.arrayContaining(['bash', 'read_file', 'write_file', 'edit_file', 'grep', 'glob'])
    );
  });
});

// Local helper mirroring splitFrontmatter's contract for assertions.
function splitFm(md: string): { frontmatter: string; body: string } {
  const m = md.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/);
  if (!m) throw new Error('no frontmatter found');
  return { frontmatter: m[1], body: m[2] };
}
