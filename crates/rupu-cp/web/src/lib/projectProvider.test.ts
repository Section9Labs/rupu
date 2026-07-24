// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { projectProvider, providerLabel, remoteHost } from './projectProvider';

describe('remoteHost', () => {
  it('parses scp-like remotes', () => {
    expect(remoteHost('git@github.com:org/repo.git')).toBe('github.com');
    expect(remoteHost('git@gitlab.acme.com:team/repo.git')).toBe('gitlab.acme.com');
  });
  it('parses URL-like remotes (https/ssh/git)', () => {
    expect(remoteHost('https://github.com/org/repo')).toBe('github.com');
    expect(remoteHost('ssh://git@gitlab.com:22/team/repo.git')).toBe('gitlab.com');
    expect(remoteHost('git://example.org/repo.git')).toBe('example.org');
  });
  it('lowercases the host', () => {
    expect(remoteHost('git@GitHub.com:org/repo.git')).toBe('github.com');
  });
  it('returns null for empty / local paths', () => {
    expect(remoteHost(null)).toBeNull();
    expect(remoteHost(undefined)).toBeNull();
    expect(remoteHost('')).toBeNull();
    expect(remoteHost('   ')).toBeNull();
    expect(remoteHost('/Users/matt/Code/local-thing')).toBeNull();
    expect(remoteHost('./relative')).toBeNull();
  });
});

describe('projectProvider', () => {
  it('classifies github (incl. enterprise host)', () => {
    expect(projectProvider('git@github.com:org/repo.git')).toBe('github');
    expect(projectProvider('https://github.com/org/repo')).toBe('github');
    expect(projectProvider('https://github.acme.com/org/repo')).toBe('github');
  });
  it('classifies gitlab (incl. self-hosted host)', () => {
    expect(projectProvider('git@gitlab.com:team/repo.git')).toBe('gitlab');
    expect(projectProvider('https://gitlab.acme.com/team/repo')).toBe('gitlab');
  });
  it('does not misread a repo merely named github on another host', () => {
    // host is gitlab.com; path contains "github" — must classify by host.
    expect(projectProvider('git@gitlab.com:org/github-tools.git')).toBe('gitlab');
  });
  it('classifies an unrecognized remote host as remote', () => {
    expect(projectProvider('git@bitbucket.org:team/repo.git')).toBe('remote');
    expect(projectProvider('https://git.sr.ht/~user/repo')).toBe('remote');
  });
  it('classifies no-remote as local', () => {
    expect(projectProvider(null)).toBe('local');
    expect(projectProvider('')).toBe('local');
    expect(projectProvider('/Users/matt/Code/thing')).toBe('local');
  });
});

describe('providerLabel', () => {
  it('labels each provider', () => {
    expect(providerLabel('git@github.com:o/r.git')).toBe('GitHub');
    expect(providerLabel('git@gitlab.com:o/r.git')).toBe('GitLab');
    expect(providerLabel(null)).toBe('Local workspace');
    expect(providerLabel('git@bitbucket.org:o/r.git')).toBe('Remote (bitbucket.org)');
  });
});
