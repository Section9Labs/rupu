// Derive a CWE id + canonical MITRE URL from a finding.
//
// Two sources, in priority order:
//   1. `concern_id` — e.g. `cwe-top25-2023:cwe-787-out-of-bounds-write`
//   2. any `evidence.references` URL under cwe.mitre.org/data/definitions/<n>
// Returns `null` when neither yields a CWE number.

export function cweFromFinding(finding: {
  concern_id?: string | null;
  evidence?: { references?: string[] } | null;
}): { id: string; url: string } | null {
  const fromConcern = finding.concern_id?.match(/cwe[-_]?(\d+)/i);
  if (fromConcern) return mk(fromConcern[1]);

  for (const ref of finding.evidence?.references ?? []) {
    const m = ref.match(/cwe\.mitre\.org\/data\/definitions\/(\d+)/i);
    if (m) return mk(m[1]);
  }

  return null;
}

/** Build a CWE `{id, url}` from a bare number or an id string (`"639"`,
 *  `"CWE-639"`, `"cwe_639"`). `null` when no number can be found. */
export function cweRef(raw: string): { id: string; url: string } | null {
  const m = raw.match(/(\d+)/);
  return m ? mk(m[1]) : null;
}

/** Every distinct CWE id (`CWE-639`) a list row carries: the report summary's
 *  declared CWEs first, then the concern/evidence-derived one. */
export function findingCweIds(finding: {
  report_summary?: { cwe: string[] } | null;
  concern_id?: string | null;
  evidence?: { references?: string[] } | null;
}): string[] {
  const ids = new Set<string>();
  for (const raw of finding.report_summary?.cwe ?? []) {
    const ref = cweRef(raw);
    if (ref) ids.add(ref.id);
  }
  const derived = cweFromFinding(finding);
  if (derived) ids.add(derived.id);
  return [...ids];
}

function mk(n: string): { id: string; url: string } {
  return {
    id: `CWE-${n}`,
    url: `https://cwe.mitre.org/data/definitions/${n}.html`,
  };
}
