// `/skill` in a prompt (the Claude Code spelling): the leading `/entry`
// tokens of a message invoke those skills. The server resolves them
// (`rupu_skills::resolve_slash`); this module only drives the composer's
// menu and its hints.
import type { SkillRow } from './api';
import { fuzzyScore } from './fuzzy';

/** A skill the `/` menu offers. */
export interface SlashSkill {
  /** What follows the `/`: the name, layer-qualified when both layers install it. */
  entry: string;
  description: string | null;
}

/** The valid skills of a listing, one entry per name the server accepts. */
export function slashSkillsOf(rows: SkillRow[]): SlashSkill[] {
  const byEntry = new Map<string, SlashSkill>();
  for (const s of rows) {
    if (s.status !== 'ok') continue;
    const entry = s.both_layers ? `${s.layer}:${s.name}` : s.name;
    if (!byEntry.has(entry)) byEntry.set(entry, { entry, description: s.description });
  }
  return [...byEntry.values()].sort((a, b) => a.entry.localeCompare(b.entry));
}

/** The `/` token the caret ends, when it is one of the message's leading
 *  tokens: where it starts and what is typed after its `/`. */
export function slashTokenAt(text: string, caret: number): { start: number; query: string } | null {
  if (caret < text.length && !/\s/.test(text[caret])) return null;
  const m = /^\s*(?:\/\S+\s+)*\/(\S*)$/.exec(text.slice(0, caret));
  if (!m) return null;
  return { start: caret - m[1].length - 1, query: m[1] };
}

/** The menu's skills for `query`, best match first. */
export function matchSlashSkills(skills: SlashSkill[], query: string, limit = 8): SlashSkill[] {
  return skills
    .map((s) => ({ s, hit: fuzzyScore(query, s.entry) }))
    .filter((x) => x.hit !== null)
    .sort((a, b) => b.hit!.score - a.hit!.score)
    .slice(0, limit)
    .map((x) => x.s);
}

/** `text` with the token from `start` to `caret` replaced by `/entry` and a
 *  space, and the caret after that space. */
export function insertSlashSkill(
  text: string,
  start: number,
  caret: number,
  entry: string,
): { text: string; caret: number } {
  const rest = text.slice(caret);
  const head = `${text.slice(0, start)}/${entry} `;
  return { text: head + rest.replace(/^ /, ''), caret: head.length };
}

const ENTRY = /^(?:(?:global|project):)?[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** What the server will make of the message's leading tokens, given the
 *  skills it knows: the entries it invokes, and the first token that reads
 *  as a skill name but names none (the server sends it, and what follows,
 *  as text). */
export function readSlashTokens(
  text: string,
  skills: SlashSkill[],
): { invoked: string[]; unknown: string | null } {
  const bare = (entry: string) => entry.replace(/^(global|project):/, '');
  const names = new Set(skills.map((s) => bare(s.entry)));
  const invoked: string[] = [];
  for (const m of text.trimStart().matchAll(/\/(\S+)(?:\s+|$)/gy)) {
    const entry = m[1];
    if (!ENTRY.test(entry)) break;
    if (!names.has(bare(entry))) return { invoked, unknown: entry };
    if (!invoked.includes(entry)) invoked.push(entry);
  }
  return { invoked, unknown: null };
}
