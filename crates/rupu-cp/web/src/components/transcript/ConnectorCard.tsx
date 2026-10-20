/**
 * ConnectorCard — one bespoke view family for the SCM / issue-tracker
 * connectors (`scm.*`, `issues.*`, `github.*`, `gitlab.*`), across every
 * platform.
 *
 * Rather than a layout per tool (there are ~20), this derives a legible target
 * (owner/repo, project#number, a path), a platform/tracker chip and an outcome
 * (created / closed / commented / rejected) for the summary, then renders the
 * output intent-first in the body:
 *   • issues.get           → title + Markdown body
 *   • scm.repos.get        → description, default branch, visibility, clone URL
 *   • scm.files.read       → the file content
 *   • *.list / *.comments  → a count + compact rows
 *   • create / comment     → the returned id / number / url
 *   • anything else        → a StructuredView (never a raw blob)
 * and turns the common connector errors ("project must be owner/repo", "account
 * resolution failed") into a plain-language note.
 *
 * Parsing (`parseConnector`, `interpretConnectorError`) is pure and unit-tested.
 * No `any`. Static Tailwind class strings only.
 */

import type { ReactNode } from 'react';
import type { ToolView } from './transcriptView';
import { Chip, KeyVals } from './toolChrome';
import StructuredView from './StructuredView';
import Markdown from './Markdown';

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

function tryJson(text: string | undefined): unknown {
  if (!text) return null;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.length > 0 ? v : null;
}

export interface ConnectorTarget {
  repo: string | null;
  project: string | null;
  number: number | null;
  path: string | null;
  platform: string | null;
}

export interface ConnectorOutcome {
  label: string;
  tone: 'ok' | 'neutral' | 'err';
}

export interface ConnectorErrorView {
  raw: string;
  friendly: string;
}

export interface ConnectorView {
  ns: string;
  verb: string;
  target: ConnectorTarget;
  outcome: ConnectorOutcome | null;
  /** Parsed output JSON, when the output was JSON. */
  output: unknown;
  /** Raw output when it wasn't JSON (e.g. a plain string). */
  rawOutput: string | null;
  error: ConnectorErrorView | null;
}

function targetOf(input: Record<string, unknown>): ConnectorTarget {
  const owner = str(input.owner);
  const repo = str(input.repo);
  return {
    repo: owner && repo ? `${owner}/${repo}` : null,
    project: str(input.project),
    number: typeof input.number === 'number' ? input.number : null,
    path: str(input.path),
    platform: str(input.platform) ?? str(input.tracker) ?? str(input.account),
  };
}

function outcomeOf(tool: ToolView, verb: string): ConnectorOutcome | null {
  if (tool.error) return { label: 'rejected', tone: 'err' };
  // Not finished yet (live transcript): no outcome chip, so the row keeps its
  // "running" StatusBadge instead of showing a premature green "created".
  if (tool.output === undefined) return null;
  if (verb.endsWith('create')) return { label: 'created', tone: 'ok' };
  if (verb === 'workflows_dispatch' || verb === 'pipeline_trigger') return { label: 'dispatched', tone: 'ok' };
  if (verb === 'comment') return { label: 'commented', tone: 'ok' };
  if (verb === 'update_state') {
    const input = isRecord(tool.input) ? tool.input : {};
    const state = str(input.state);
    return state ? { label: state, tone: 'neutral' } : null;
  }
  return null;
}

/** Turn a connector error into a plain-language note (prefixes stripped). */
export function interpretConnectorError(error: string): ConnectorErrorView {
  const raw = error
    .replace(/^execution:\s*tool dispatch failed:\s*/i, '')
    .replace(/^execution:\s*/i, '')
    .trim();

  const projectShape = raw.match(/project must be `owner\/repo`:\s*(.+)$/);
  if (projectShape) {
    return { raw, friendly: `The project must be \`owner/repo\` — the agent sent \`${projectShape[1].trim()}\`.` };
  }
  if (/^account resolution failed/i.test(raw)) {
    return { raw, friendly: 'No account is configured for this host — bind one with `rupu scm bind` or pass `--account`.' };
  }
  if (/^not found/i.test(raw)) {
    return { raw, friendly: 'The connector could not find that resource.' };
  }
  return { raw, friendly: raw };
}

export function parseConnector(tool: ToolView): ConnectorView {
  const dot = tool.tool.indexOf('.');
  const ns = dot >= 0 ? tool.tool.slice(0, dot) : tool.tool;
  const verb = dot >= 0 ? tool.tool.slice(dot + 1) : '';
  const input = isRecord(tool.input) ? tool.input : {};
  const parsed = tryJson(tool.output);
  return {
    ns,
    verb,
    target: targetOf(input),
    outcome: outcomeOf(tool, verb),
    output: parsed,
    rawOutput: parsed === null && tool.output ? tool.output : null,
    error: tool.error ? interpretConnectorError(tool.error) : null,
  };
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

export function ConnectorSummary({ view }: { view: ConnectorView }) {
  const t = view.target;
  return (
    <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
      {t.repo && (
        <Chip tone="neutral" mono>
          {t.repo}
        </Chip>
      )}
      {t.project && !t.repo && (
        <Chip tone="neutral" mono>
          {t.project}
        </Chip>
      )}
      {t.number !== null && <Chip tone="info" mono>#{t.number}</Chip>}
      {t.path && (
        <Chip tone="neutral" mono>
          {t.path}
        </Chip>
      )}
      {t.platform && <Chip tone="neutral">{t.platform}</Chip>}
      {view.outcome && <Chip tone={view.outcome.tone}>{view.outcome.label}</Chip>}
    </span>
  );
}

/** A compact list of homogeneous rows (repos / branches / comments). */
function RowList({ rows }: { rows: Record<string, unknown>[] }) {
  const shown = rows.slice(0, 12);
  return (
    <div className="space-y-1">
      <p className="text-meta text-ink-mute">{rows.length} result{rows.length === 1 ? '' : 's'}</p>
      {shown.map((r, i) => {
        // issue-comment rows carry author + body; repo rows carry r.owner/repo.
        // issue-comment rows: author + body.
        const author = str(r.author);
        const body = str(r.body);
        if (author && body) {
          return (
            <div key={i} className="rounded bg-surface px-2 py-1">
              <p className="font-mono text-meta text-ink-mute">{author}</p>
              <p className="text-note text-ink-dim line-clamp-2">{body}</p>
            </div>
          );
        }
        // repo rows (nested `r`, or a flat `full_name`), branch rows (`name`),
        // issue/PR rows (`#number title`); a JSON snippet only as last resort.
        const repo = isRecord(r.r) ? `${str(r.r.owner) ?? ''}/${str(r.r.repo) ?? ''}` : null;
        const num = typeof r.number === 'number' ? r.number : null;
        const title = str(r.title);
        const label =
          (repo && repo !== '/' ? repo : null) ??
          str(r.full_name) ??
          (num !== null ? `#${num}${title ? ` ${title}` : ''}` : null) ??
          title ??
          str(r.name) ??
          JSON.stringify(r).slice(0, 80);
        return (
          <div key={i} className="font-mono text-note text-ink-dim truncate">
            {label}
          </div>
        );
      })}
      {rows.length > shown.length && <p className="text-meta text-ink-mute">+{rows.length - shown.length} more</p>}
    </div>
  );
}

/** Build the body content for a successful connector call (a plain helper, not
 *  a component — `ConnectorBody` needs the value to decide whether to render a
 *  wrapper at all). */
function renderOutput(view: ConnectorView): ReactNode {
  const out = view.output;

  // issues.get → title + markdown body
  if (view.verb === 'get' && view.ns === 'issues' && isRecord(out)) {
    const title = str(out.title);
    const body = str(out.body);
    return (
      <div className="space-y-1.5">
        {title && <p className="text-ui font-semibold text-ink leading-snug">{title}</p>}
        {body && (
          <div className="text-note text-ink-dim">
            <Markdown text={body} />
          </div>
        )}
      </div>
    );
  }

  // scm.repos.get → key facts
  if (view.verb === 'repos.get' && isRecord(out)) {
    return (
      <KeyVals
        rows={[
          ['description', str(out.description)],
          ['default', str(out.default_branch) ? <span className="font-mono">{str(out.default_branch)}</span> : null],
          ['visibility', out.private === true ? 'private' : out.private === false ? 'public' : null],
          ['clone', str(out.clone_url_https) ? <span className="font-mono break-all">{str(out.clone_url_https)}</span> : null],
        ]}
      />
    );
  }

  // scm.files.read → the file content
  if (view.verb === 'files.read' && isRecord(out)) {
    const content = str(out.content);
    return (
      <div className="space-y-1">
        {str(out.path) && <p className="font-mono text-meta text-ink-mute">{str(out.path)}</p>}
        {content && (
          <pre className="whitespace-pre-wrap rounded bg-surface p-2 font-mono text-[10.5px] leading-snug text-ink">
            {content}
          </pre>
        )}
      </div>
    );
  }

  // lists / comment threads → compact rows
  if (Array.isArray(out)) {
    if (out.length === 0) return <p className="text-note text-ink-mute">No results.</p>;
    if (out.every(isRecord)) return <RowList rows={out} />;
  }

  // created / commented → the returned id / number / url
  if (isRecord(out) && (view.outcome?.tone === 'ok')) {
    const number = typeof out.number === 'number' ? out.number : null;
    const id = str(out.id);
    const url = str(out.url) ?? str(out.html_url);
    if (number !== null || id || url) {
      return (
        <div className="flex flex-wrap items-center gap-1.5">
          {number !== null && <Chip tone="info" mono>#{number}</Chip>}
          {id && number === null && <Chip tone="neutral" mono>{id}</Chip>}
          {url && (
            <a href={url} target="_blank" rel="noopener noreferrer" className="font-mono text-note text-brand-700 underline underline-offset-2 break-all">
              {url}
            </a>
          )}
        </div>
      );
    }
  }

  // empty object (e.g. update_state → {}) — nothing to show
  if (isRecord(out) && Object.keys(out).length === 0) return null;

  // fallback — a structured view (never a raw blob)
  if (out !== null) return <StructuredView value={out} />;
  if (view.rawOutput) {
    return (
      <pre className="whitespace-pre-wrap rounded bg-surface p-2 font-mono text-[10.5px] leading-snug text-ink">
        {view.rawOutput}
      </pre>
    );
  }
  return null;
}

export function ConnectorBody({ view }: { view: ConnectorView }) {
  if (view.error) {
    // A crafted explanation (differs from the raw message) carries intentional
    // `code` spans, so it renders as Markdown with the raw error quoted below.
    // The generic fallback (friendly === raw) is the server's own text — render
    // it as plain mono so stray `_`/`*`/backticks aren't reflowed.
    const crafted = view.error.friendly !== view.error.raw;
    return (
      <div className="px-3 py-2 space-y-1.5">
        {crafted ? (
          <>
            <div className="rounded bg-surface px-2.5 py-2 text-note text-ink-dim leading-relaxed">
              <Markdown text={view.error.friendly} />
            </div>
            <p className="font-mono text-meta text-ink-mute break-all">{view.error.raw}</p>
          </>
        ) : (
          <p className="font-mono text-note text-ink-dim break-all">{view.error.raw}</p>
        )}
      </div>
    );
  }
  const body = renderOutput(view);
  if (!body) return null;
  return <div className="px-3 py-2">{body}</div>;
}
