// Display helpers for agent codenames. The server mints every name; this
// module only parses it for presentation and looks up the generated palette.

import { CREW_TINTS, ROLE_BADGES, type BadgeShape } from './codenamePalette.gen';

export interface ParsedCodename {
  crew: string;
  leaf: string;
  role?: string;
}

/** crew = before '/', leaf = after '/' (or crew when none), role = last
 *  segment's word (strip #n, .a, trailing digits). */
export function parseCodename(c: string): ParsedCodename {
  const slash = c.indexOf('/');
  if (slash < 0) return { crew: c, leaf: c, role: undefined };
  const crew = c.slice(0, slash);
  const leaf = c.slice(slash + 1);
  const last = leaf.split('>').pop() ?? leaf;
  const word = last.split(/[#.]/)[0].replace(/\d+$/, '');
  return { crew, leaf, role: word || undefined };
}

export function crewTint(crew: string, mode: 'light' | 'dark'): string | undefined {
  return CREW_TINTS[crew.split('-')[0]]?.[mode];
}

export function roleBadge(
  role: string,
  mode: 'light' | 'dark',
): { shape: BadgeShape; color: string } | undefined {
  const b = ROLE_BADGES[role];
  return b ? { shape: b.shape, color: b[mode] } : undefined;
}

/** `leaf · agent · provider/model`, omitting absent parts (mirrors the CLI's member_label). */
export function memberLabel(
  codename: string | undefined,
  agent: string | undefined,
  provider?: string,
  model?: string,
): string {
  const parts: string[] = [];
  if (codename) parts.push(parseCodename(codename).leaf);
  if (agent) parts.push(agent);
  const pm = [provider, model].filter(Boolean).join('/');
  if (pm) parts.push(pm);
  return parts.join(' · ');
}
