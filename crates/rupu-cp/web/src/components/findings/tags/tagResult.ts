// One line for what a bulk tag change did (`POST /api/findings/tags`), for
// the bulk bar and the finding page.
import type { TagAcrossResult } from '../../../lib/api';

const plural = (n: number) => `${n} finding${n === 1 ? '' : 's'}`;

export function summarizeTagResult(
  r: TagAcrossResult,
  mode: 'add' | 'remove',
  projectOf: (wsId: string) => string,
): { message: string; ok: boolean } {
  const outcomes = r.workspaces.flatMap((w) => w.outcomes ?? []);
  const changed = outcomes.filter((o) => o.before.join('\u0000') !== o.after.join('\u0000')).length;
  const same = outcomes.length - changed;
  const parts = [
    `${mode === 'add' ? 'Tagged' : 'Untagged'} ${plural(changed)}${same > 0 ? ` (${same} already ${mode === 'add' ? 'had it' : "didn't have it"})` : ''}.`,
  ];
  const failed = r.workspaces.filter((w) => w.error);
  for (const w of failed) parts.push(`${projectOf(w.ws_id)}'s tags couldn't be changed: ${w.error}.`);
  if (r.unknown.length > 0) parts.push(`${plural(r.unknown.length)} no longer exist${r.unknown.length === 1 ? 's' : ''}.`);
  return { message: parts.join(' '), ok: failed.length === 0 && r.unknown.length === 0 };
}
