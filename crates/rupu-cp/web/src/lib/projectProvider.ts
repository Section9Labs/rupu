// projectProvider — classify a project's SCM host from its git remote, so the
// Projects table can show an identifying icon per row (GitHub / GitLab /
// other-remote / local-only). Pure + host-scoped so a repo merely NAMED
// "github-tools" on GitLab isn't misread as GitHub.

export type ProjectProvider = 'github' | 'gitlab' | 'remote' | 'local';

/** Extract the host from a git remote in either scp-like (`git@host:path`) or
 *  URL-like (`scheme://[user@]host[:port]/path`) form. Returns null when no host
 *  is discernible (e.g. a bare local path, or empty). */
export function remoteHost(remote?: string | null): string | null {
  if (!remote) return null;
  const s = remote.trim();
  if (s === '') return null;

  // URL-like: scheme://[user@]host[:port]/...  (https, ssh, git, http)
  const url = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/]*@)?([^:/?#]+)/i.exec(s);
  if (url) return url[1].toLowerCase() || null;

  // scp-like: [user@]host:path  (no scheme, a colon before the path, and the
  // part before the colon is not itself a drive/path). Require an '@' or a
  // dotted host to avoid matching a bare `C:\...` or `relative:thing`.
  const scp = /^(?:[^@/]*@)?([^@/:]+):/.exec(s);
  if (scp && (s.includes('@') || scp[1].includes('.'))) return scp[1].toLowerCase();

  return null;
}

/** Classify the SCM provider for a project's remote. No remote → 'local'. A
 *  recognizable GitHub/GitLab host → that provider (host-scoped, so GH
 *  Enterprise `github.acme.com` and self-hosted `gitlab.acme.com` still match);
 *  any other host → 'remote'. */
export function projectProvider(remote?: string | null): ProjectProvider {
  const host = remoteHost(remote);
  if (host === null) return 'local';
  if (host === 'github.com' || host.endsWith('.github.com') || host.includes('github')) {
    return 'github';
  }
  if (host === 'gitlab.com' || host.endsWith('.gitlab.com') || host.includes('gitlab')) {
    return 'gitlab';
  }
  return 'remote';
}

/** Human-readable label for the provider (tooltip / aria). Includes the host
 *  for a generic remote so a self-hosted server is still identifiable. */
export function providerLabel(remote?: string | null): string {
  const p = projectProvider(remote);
  switch (p) {
    case 'github':
      return 'GitHub';
    case 'gitlab':
      return 'GitLab';
    case 'local':
      return 'Local workspace';
    case 'remote': {
      const host = remoteHost(remote);
      return host ? `Remote (${host})` : 'Remote';
    }
  }
}
