import { describe, it, expect } from 'vitest';
import { parseErrorDetail } from './errorDetail';
import { languageForPath } from './lang';

describe('parseErrorDetail', () => {
  it('parses a whole-string JSON object', () => {
    const p = parseErrorDetail('{"code":429,"message":"rate limited"}');
    expect(p.json).toEqual({ code: 429, message: 'rate limited' });
    expect(p.pretty).toContain('"code": 429');
    expect(p.prefix).toBeUndefined();
  });

  it('recovers JSON embedded after a prefix message', () => {
    const p = parseErrorDetail('provider error: {"type":"overloaded","retry":true}');
    expect(p.json).toEqual({ type: 'overloaded', retry: true });
    expect(p.prefix).toBe('provider error:');
  });

  it('parses a JSON array', () => {
    const p = parseErrorDetail('[{"a":1},{"b":2}]');
    expect(Array.isArray(p.json)).toBe(true);
  });

  it('leaves plain text as raw with no json', () => {
    const p = parseErrorDetail('clone timed out on mini-host');
    expect(p.json).toBeUndefined();
    expect(p.pretty).toBeUndefined();
    expect(p.raw).toBe('clone timed out on mini-host');
  });

  it('does not treat a bare scalar as pretty-printable JSON', () => {
    expect(parseErrorDetail('42').json).toBeUndefined();
    expect(parseErrorDetail('true').json).toBeUndefined();
  });
});

describe('languageForPath', () => {
  it('maps common extensions to registered hljs languages', () => {
    expect(languageForPath('src/routes/billing.ts')).toBe('typescript');
    expect(languageForPath('main.rs')).toBe('rust');
    expect(languageForPath('app.py')).toBe('python');
    expect(languageForPath('go.mod')).toBe('plaintext'); // .mod not mapped
    expect(languageForPath('server.go')).toBe('go');
  });

  it('recognizes extension-less Dockerfile/Makefile by name', () => {
    expect(languageForPath('deploy/Dockerfile')).toBe('dockerfile');
    expect(languageForPath('Makefile')).toBe('makefile');
  });

  it('falls back to plaintext for unknown or missing paths', () => {
    expect(languageForPath('data.bin')).toBe('plaintext');
    expect(languageForPath(undefined)).toBe('plaintext');
    expect(languageForPath(null)).toBe('plaintext');
  });
});
