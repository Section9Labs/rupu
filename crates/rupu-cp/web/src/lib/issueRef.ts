/** A short label for a stored issue ref (`<tracker>:<project>/issues/<n>`):
 *  `owner/repo#42` for GitHub/GitLab, `<last project segment>#42` for other
 *  trackers (a Jira ref's project carries its host), the ref unchanged when it
 *  isn't in that form. */
export function shortIssueRef(ref: string): string {
  const m = /^([a-z_]+):(.+)\/issues\/(\d+)$/.exec(ref);
  if (!m) return ref;
  const [, tracker, project, number] = m;
  const label = tracker === 'github' || tracker === 'gitlab' ? project : project.split('/').pop() ?? project;
  return `${label}#${number}`;
}
