/**
 * SkillCard — bespoke view for `skills.load` (loading an Agent Skill into a
 * run). The tool's output is the rendered skill: a header block
 *
 *   Skill: <name> (version <hash>)
 *   Base directory: <dir>
 *   Files: a, b, c
 *   [Compatibility: …]
 *
 *   # <title>
 *   <body…>
 *
 * This view pulls the name/version/files out as chips and renders the body as
 * Markdown, instead of dumping the whole blob into a <pre>.
 *
 * Parsing (`parseSkill`) is pure and unit-tested. No `any`. Static Tailwind
 * class strings only.
 */

import type { ToolView } from './transcriptView';
import { Chip } from './toolChrome';
import Markdown from './Markdown';

export interface SkillView {
  name: string | null;
  version: string | null;
  baseDir: string | null;
  files: string[];
  compatibility: string | null;
  /** The first Markdown heading in the body, as a one-line subtitle. */
  title: string | null;
  body: string;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

export function parseSkill(tool: ToolView): SkillView {
  const input = isRecord(tool.input) ? tool.input : {};
  const output = tool.output ?? '';

  // Header is everything before the first blank line; the body follows.
  const split = output.indexOf('\n\n');
  const head = split >= 0 ? output.slice(0, split) : output;
  const body = split >= 0 ? output.slice(split + 2).trim() : '';

  const name =
    head.match(/^Skill:\s*(\S+)/m)?.[1] ?? (typeof input.name === 'string' ? input.name : null);
  const version = head.match(/\(version\s+([^)]+)\)/)?.[1]?.trim() ?? null;
  const baseDir = head.match(/^Base directory:\s*(.+)$/m)?.[1]?.trim() ?? null;
  const filesLine = head.match(/^Files:\s*(.+)$/m)?.[1] ?? '';
  const files = filesLine
    .split(',')
    .map((f) => f.trim())
    .filter((f) => f.length > 0);
  const compatibility = head.match(/^Compatibility:\s*(.+)$/m)?.[1]?.trim() ?? null;
  const title = body.match(/^#+\s+(.+)$/m)?.[1]?.trim() ?? null;

  return { name, version, baseDir, files, compatibility, title, body };
}

const MAX_FILE_CHIPS = 4;

export function SkillSummary({ view }: { view: SkillView }) {
  return (
    <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
      {view.name && <span className="font-mono text-note font-semibold text-ink">{view.name}</span>}
      {view.version && (
        <Chip tone="neutral" mono title={`version ${view.version}`}>
          v {view.version.slice(0, 8)}
        </Chip>
      )}
      {view.title && <span className="truncate text-note text-ink-dim">{view.title}</span>}
      {view.files.length > 0 && (
        <Chip tone="neutral">
          {view.files.length} file{view.files.length === 1 ? '' : 's'}
        </Chip>
      )}
    </span>
  );
}

export function SkillBody({ view }: { view: SkillView }) {
  const shown = view.files.slice(0, MAX_FILE_CHIPS);
  const extra = view.files.length - shown.length;
  return (
    <div className="px-3 py-2 space-y-2">
      {view.files.length > 0 && (
        <div className="flex flex-wrap items-center gap-1">
          {shown.map((f) => (
            <Chip key={f} tone="neutral" mono>
              {f}
            </Chip>
          ))}
          {extra > 0 && <span className="text-meta text-ink-mute">+{extra} more</span>}
        </div>
      )}
      {view.compatibility && (
        <p className="text-meta text-ink-mute">
          <span className="uppercase tracking-wide">compat</span> · {view.compatibility}
        </p>
      )}
      {view.body && (
        <div className="text-ui text-ink-dim">
          <Markdown text={view.body} />
        </div>
      )}
    </div>
  );
}
