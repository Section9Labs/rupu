// Typed model of the structured finding report (mirrors
// rupu-coverage/src/report/types.rs and summary.rs) plus small helpers the
// report views share.

export type RiskLevel = 'Low' | 'Medium' | 'High' | 'Critical';
export type Likelihood = 'Low' | 'Medium' | 'High';
/** A field that is either real content or a sentinel string. */
export type OrSentinel<T> = T | string;

export interface Ownership { owner: string; product: string; affected_component: string; source_repository: string }
export interface Ticket { type: string; identifier: string; url?: string; notes?: string }
export interface Rating { impact: RiskLevel; likelihood: Likelihood; risk_rating: RiskLevel; risk_factor: RiskLevel; cvss_v3: string }
export interface ChainHop {
  label: string; file?: string; lines?: [number, number]; binary_va?: string;
  gate?: string; passes_because?: string; role: 'source' | 'hop' | 'sink';
}
export interface EvidenceClaim {
  claim: string; file?: string; lines?: [number, number]; binary_va?: string;
  excerpt?: string; lang?: string; sha256?: string; artifact?: string;
}
export interface Patch { diff: string; notes?: string }
export interface CiDetection { stage: string; body: string; command?: string; expect: string }
export interface RegressionTest { body: string; command: string; expect_vulnerable: string; expect_patched: string }
export interface CrossRef { finding_id: string; relation: 'duplicate' | 'sibling' | 'prerequisite' | 'supersedes'; note?: string }
export interface ArtifactRef {
  path: string; sha256: string; size: number; kind?: 'text' | 'binary';
  stored?: 'copied' | 'external'; host?: string;
}
export type VerificationStatus = 'unverified' | 'confirmed' | 'disputed' | 'inconclusive';
export interface Verification { status: VerificationStatus; by_run?: string; notes?: string }

export interface FindingReport {
  title: string;
  ownership: Ownership;
  tickets: OrSentinel<Ticket[]>;
  rating: Rating;
  category: string;
  attack_vector: string;
  cwe?: string[];
  description: string;
  impact: string;
  location: { input: string; output: string };
  root_cause: string;
  call_chain: OrSentinel<ChainHop[]>;
  evidence: EvidenceClaim[];
  remediation: string;
  recommended_patch: OrSentinel<Patch>;
  ci_cd_detection: OrSentinel<CiDetection>;
  regression_test: OrSentinel<RegressionTest>;
  replication_steps: string[];
  cross_references: OrSentinel<CrossRef[]>;
  references: string;
  artifacts?: ArtifactRef[];
  verification?: Verification;
}

export interface Completeness { filled: number; total: number; gaps: string[] }
export interface ReportSummary {
  owner: string; product: string; cwe: string[]; root_cause: string; chain: string[];
  completeness: Completeness; has_poc: boolean; verification_status?: VerificationStatus | null;
}
export type ClaimState = 'current' | 'changed' | 'missing' | 'unknown';

/** Shown where a `profile: 'full'` finding has no `report`: the ledger line
 *  carries a report this build could not parse (for example one written by a
 *  newer rupu), which loads as `None`. */
export const UNREADABLE_REPORT_NOTE =
  "This finding has a full report that this version of rupu can't display (it may have been written by a newer version).";

export function isSentinel<T>(v: OrSentinel<T>): v is string {
  return typeof v === 'string';
}

const NOT_PROVIDED = 'Not Provided — ';

/** `Unknown` and `Not Provided — …` are gaps; `None`, `None Provided`,
 *  `Not Applicable` are real answers. */
export function isGapSentinel(s: string): boolean {
  return s.trim() === 'Unknown' || s.startsWith(NOT_PROVIDED);
}

/** Which of the report's 11 tracked fields are still gaps. Ported exactly from
 *  `rupu-coverage/src/report/summary.rs::completeness` — same fields, same
 *  order, same rules — so the CP and exports agree. */
export function completeness(r: FindingReport): Completeness {
  const unknown = (s: string) => s.trim() === 'Unknown';
  const notProvided = (v: OrSentinel<unknown>) => typeof v === 'string' && v.startsWith(NOT_PROVIDED);
  const checks: [string, boolean][] = [
    ['owner', unknown(r.ownership.owner)],
    ['product', unknown(r.ownership.product)],
    ['affected_component', unknown(r.ownership.affected_component)],
    ['source_repository', unknown(r.ownership.source_repository)],
    ['tickets', typeof r.tickets === 'string' && unknown(r.tickets)],
    ['cvss_v3', unknown(r.rating.cvss_v3)],
    ['attack_vector', unknown(r.attack_vector)],
    ['call_chain', notProvided(r.call_chain)],
    ['recommended_patch', notProvided(r.recommended_patch)],
    ['ci_cd_detection', notProvided(r.ci_cd_detection)],
    ['regression_test', notProvided(r.regression_test)],
  ];
  const gaps = checks.filter(([, g]) => g).map(([n]) => n);
  return { filled: checks.length - gaps.length, total: checks.length, gaps };
}

export function sentinelLabel(s: string): string {
  return s.startsWith(NOT_PROVIDED) ? `Not provided: ${s.slice(NOT_PROVIDED.length)}` : s;
}

/** Deep link into a project's Code tab. */
export function codeHref(wsId: string, path: string, line?: number): string {
  const base = `/projects/${encodeURIComponent(wsId)}/code?path=${encodeURIComponent(path)}`;
  return line !== undefined ? `${base}&line=${line}` : base;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
