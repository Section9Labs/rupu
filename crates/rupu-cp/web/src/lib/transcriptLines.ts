/**
 * TS ports of `rupu_transcript::grant::tool_grant_line` and
 * `rupu_transcript::skills::{skill_index_line, skill_load_line}`, so the web
 * transcript says exactly what `rupu transcript` says. `transcriptLines.test.ts`
 * pins the same inputs and strings as the Rust tests.
 */

const NAMES_SHOWN = 12;

export function toolGrantLine(
  entries: { tool: string }[],
  narrowed: string[],
  unavailable: { tool: string; missing: string[] }[],
): string {
  const names = entries.map((e) => e.tool);
  const more = Math.max(0, names.length - NAMES_SHOWN);
  let line = `${entries.length} tool${entries.length === 1 ? '' : 's'}`;
  if (names.length > 0) {
    line += `: ${names.slice(0, NAMES_SHOWN).join(', ')}`;
    if (more > 0) line += ` (+${more} more)`;
  }
  if (narrowed.length > 0) line += `  ·  narrowed by actions: ${narrowed.join(', ')}`;
  if (unavailable.length > 0) {
    const u = unavailable.map((m) => `${m.tool} (needs ${m.missing.join(', ')})`);
    line += `  ·  unavailable: ${u.join(', ')}`;
  }
  return line;
}

export function skillIndexLine(
  entries: { name: string; preload: boolean; indexed: boolean }[],
): string {
  const noun = entries.length === 1 ? 'skill' : 'skills';
  if (entries.length === 0) return `0 ${noun}`;
  const names = entries.map((e) =>
    e.preload ? `${e.name} (preloaded)` : e.indexed ? e.name : `${e.name} (not indexed)`,
  );
  return `${entries.length} ${noun}: ${names.join(', ')}`;
}

export function skillLoadLine(name: string, via: string, contentHash: string): string {
  if (contentHash === '') return `loaded skill ${name} via ${via}`;
  const short = Array.from(contentHash).slice(0, 12).join('');
  return `loaded skill ${name} (${short}) via ${via}`;
}
