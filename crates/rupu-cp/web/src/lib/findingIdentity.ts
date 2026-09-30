// The declaring agent's codename for a finding row. `FindingOut` rows carry a
// top-level `codename` (+ `codename_derived`); plain `FindingRecord`s (the
// coverage-detail list) only have the stored `declared_by.codename`. Both are
// server-minted — this never derives a name.
//
// Also surfaces the declaring agent / provider / model from `declared_by`
// (`rupu_coverage::Attribution`). `model` has always been recorded; `agent`
// and `provider` only on findings written after they were added, so an old
// finding shows its model alone.

import type { FindingOut, FindingRecord } from './api';

export interface FindingIdentity {
  codename: string;
  derived: boolean;
  agent?: string;
  provider?: string;
  model?: string;
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v ? v : undefined;
}

export function findingCodename(f: FindingRecord): FindingIdentity | undefined {
  const by = (f.declared_by && typeof f.declared_by === 'object'
    ? (f.declared_by as Record<string, unknown>)
    : {}) as Record<string, unknown>;
  const ident = { agent: str(by.agent), provider: str(by.provider), model: str(by.model) };
  const out = f as Partial<FindingOut>;
  if (typeof out.codename === 'string' && out.codename) {
    return { codename: out.codename, derived: out.codename_derived === true, ...ident };
  }
  const codename = str(by.codename);
  if (codename) return { codename, derived: false, ...ident };
  return undefined;
}
