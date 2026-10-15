import { describe, it, expect } from 'vitest';
import { shortIssueRef } from './issueRef';

describe('shortIssueRef', () => {
  it('shortens GitHub and GitLab refs to owner/repo#n', () => {
    expect(shortIssueRef('github:acme/api/issues/27')).toBe('acme/api#27');
    expect(shortIssueRef('gitlab:group/sub/proj/issues/3')).toBe('group/sub/proj#3');
  });
  it('keeps only the last project segment for other trackers', () => {
    expect(shortIssueRef('jira:127.0.0.1/ENG/issues/42')).toBe('ENG#42');
  });
  it('leaves anything else unchanged', () => {
    expect(shortIssueRef('acme/api#27')).toBe('acme/api#27');
    expect(shortIssueRef('weird')).toBe('weird');
  });
});
