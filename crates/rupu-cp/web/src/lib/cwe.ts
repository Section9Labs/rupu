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

function mk(n: string): { id: string; url: string } {
  return {
    id: `CWE-${n}`,
    url: `https://cwe.mitre.org/data/definitions/${n}.html`,
  };
}
