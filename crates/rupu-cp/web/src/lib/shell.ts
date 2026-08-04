// Shell v2 feature flag. Source of truth is `[ui.cp] shell` in
// ~/.rupu/config.toml; rupu-cp's embed.rs injects it as
// `<meta name="rupu-shell" content="v1"|"v2">` when serving index.html.
// The Vite dev server serves the raw index.html (no meta), so dev builds
// may opt in via `localStorage['rupu.cp.shell'] = 'v2'` — DEV only.
export type ShellVersion = 'v1' | 'v2';

export function getShell(): ShellVersion {
  const fromMeta = document
    .querySelector('meta[name="rupu-shell"]')
    ?.getAttribute('content');
  if (fromMeta === 'v1' || fromMeta === 'v2') return fromMeta;
  if (import.meta.env.DEV) {
    try {
      if (localStorage.getItem('rupu.cp.shell') === 'v2') return 'v2';
    } catch {
      /* storage unavailable (private mode) — fall through */
    }
  }
  return 'v1';
}
