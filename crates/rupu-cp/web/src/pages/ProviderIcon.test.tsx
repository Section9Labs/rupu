// @vitest-environment jsdom
import { describe, expect, it, afterEach } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { ProviderIcon } from './Projects';

afterEach(cleanup);

describe('ProviderIcon', () => {
  it('renders a GitHub-labeled icon for a github remote', () => {
    render(<ProviderIcon remote="git@github.com:org/repo.git" />);
    expect(screen.getByRole('img', { name: 'GitHub' })).toBeTruthy();
  });

  it('renders a GitLab-labeled icon for a gitlab remote', () => {
    render(<ProviderIcon remote="https://gitlab.com/team/repo" />);
    expect(screen.getByRole('img', { name: 'GitLab' })).toBeTruthy();
  });

  it('renders a Local-workspace icon when there is no remote', () => {
    render(<ProviderIcon remote={null} />);
    expect(screen.getByRole('img', { name: 'Local workspace' })).toBeTruthy();
  });

  it('renders a host-labeled Remote icon for an unrecognized host', () => {
    render(<ProviderIcon remote="git@bitbucket.org:team/repo.git" />);
    expect(screen.getByRole('img', { name: 'Remote (bitbucket.org)' })).toBeTruthy();
  });
});
