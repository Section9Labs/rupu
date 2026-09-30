// The declaring agent's codename for a finding row. `FindingOut` rows carry a
// top-level `codename` (+ `codename_derived`); plain `FindingRecord`s (the
// coverage-detail list) only have the stored `declared_by.codename`. Both are
// server-minted — this never derives a name.

import type { FindingOut, FindingRecord } from '../../lib/api';

export function findingCodename(
  f: FindingRecord,
): { codename: string; derived: boolean } | undefined {
  const out = f as Partial<FindingOut>;
  if (typeof out.codename === 'string' && out.codename) {
    return { codename: out.codename, derived: out.codename_derived === true };
  }
  const by = f.declared_by as { codename?: unknown } | null | undefined;
  if (by && typeof by === 'object' && typeof by.codename === 'string' && by.codename) {
    return { codename: by.codename, derived: false };
  }
  return undefined;
}
