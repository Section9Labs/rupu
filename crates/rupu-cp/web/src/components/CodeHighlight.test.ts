import { describe, it, expect } from 'vitest';
import { splitFrontmatter } from './CodeHighlight';

describe('splitFrontmatter', () => {
  it('splits a leading YAML frontmatter block from the body', () => {
    const raw = '---\nname: x\nmodel: y\n---\nHello body\n';
    expect(splitFrontmatter(raw)).toEqual({
      frontmatter: 'name: x\nmodel: y',
      body: 'Hello body\n',
    });
  });

  it('returns null frontmatter when there is none', () => {
    const raw = 'Just a body, no frontmatter';
    expect(splitFrontmatter(raw)).toEqual({
      frontmatter: null,
      body: 'Just a body, no frontmatter',
    });
  });

  it('handles frontmatter with no body', () => {
    const raw = '---\nname: x\n---\n';
    expect(splitFrontmatter(raw)).toEqual({ frontmatter: 'name: x', body: '' });
  });

  it('handles CRLF line endings', () => {
    const raw = '---\r\nname: x\r\n---\r\nBody\r\n';
    expect(splitFrontmatter(raw)).toEqual({ frontmatter: 'name: x', body: 'Body\r\n' });
  });

  it('treats an unterminated frontmatter fence as plain body', () => {
    const raw = '---\nname: x\nno closing fence';
    expect(splitFrontmatter(raw)).toEqual({
      frontmatter: null,
      body: '---\nname: x\nno closing fence',
    });
  });
});
