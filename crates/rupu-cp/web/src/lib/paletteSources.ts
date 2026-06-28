// Command-palette entity model + pure ranking.
//
// Turns the heterogeneous API list rows (runs, agents, workflows, …) into a
// uniform `PaletteItem` shape, then fuzzy-ranks + groups them by kind for the
// ⌘K palette. All logic here is pure (no React / DOM / fetch) so it can be
// unit-tested directly — the component layer only wires fetching + rendering.

import { fuzzyScore } from './fuzzy';
import type {
  RunListRow,
  AgentSummary,
  WorkflowSummary,
  AutoflowDefRow,
  SessionSummary,
  ProjectRow,
  CoverageSummary,
  FindingOut,
  AutoflowClaim,
  WorkerRecord,
} from './api';

// ---------------------------------------------------------------------------
// Item model
// ---------------------------------------------------------------------------

export type EntityKind =
  | 'page'
  | 'run'
  | 'agent'
  | 'workflow'
  | 'autoflow'
  | 'session'
  | 'project'
  | 'coverage'
  | 'finding'
  | 'issue'
  | 'worker';

export interface PaletteItem {
  kind: EntityKind;
  id: string;
  title: string;
  subtitle?: string;
  /** react-router path to navigate to, or null when not navigable. */
  to: string | null;
  keywords?: string;
}

export interface PaletteGroup {
  kind: EntityKind;
  items: PaletteItem[];
}

// Fixed display order — pages first, then the entity sections.
const KIND_ORDER: EntityKind[] = [
  'page',
  'run',
  'agent',
  'workflow',
  'autoflow',
  'session',
  'project',
  'coverage',
  'finding',
  'issue',
  'worker',
];

const GROUP_CAP = 6;

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/**
 * Fuzzy-score, filter, group and cap palette items by `query`.
 *
 * - `pages` (always the page-kind items) are merged with `items` so pages are
 *   ranked alongside every other entity. Pages always form their own group.
 * - Each item is scored via `fuzzyScore` against its `title`, falling back to
 *   `subtitle` then `keywords`. A null on all three drops the item.
 * - Empty query: every item passes with score 0.
 * - Groups are returned in `KIND_ORDER`, each sorted descending by score and
 *   capped at 6 items.
 */
export function rankPalette(
  query: string,
  items: PaletteItem[],
  pages: PaletteItem[],
): PaletteGroup[] {
  const q = query.trim();
  const scored: { item: PaletteItem; score: number }[] = [];

  for (const item of [...pages, ...items]) {
    if (q === '') {
      scored.push({ item, score: 0 });
      continue;
    }
    const byTitle = fuzzyScore(q, item.title);
    if (byTitle !== null) {
      scored.push({ item, score: byTitle.score });
      continue;
    }
    if (item.subtitle) {
      const bySub = fuzzyScore(q, item.subtitle);
      if (bySub !== null) {
        scored.push({ item, score: bySub.score });
        continue;
      }
    }
    if (item.keywords) {
      const byKw = fuzzyScore(q, item.keywords);
      if (byKw !== null) {
        scored.push({ item, score: byKw.score });
      }
    }
  }

  const byKind = new Map<EntityKind, typeof scored>();
  for (const k of KIND_ORDER) byKind.set(k, []);
  for (const s of scored) byKind.get(s.item.kind)?.push(s);

  const groups: PaletteGroup[] = [];
  for (const kind of KIND_ORDER) {
    const bucket = byKind.get(kind)!;
    if (bucket.length === 0) continue;
    bucket.sort((a, b) => b.score - a.score);
    // Pages are a finite, curated nav list — never cap them. Only the
    // open-ended entity groups get the per-group cap.
    const capped = kind === 'page' ? bucket : bucket.slice(0, GROUP_CAP);
    groups.push({
      kind,
      items: capped.map((s) => s.item),
    });
  }
  return groups;
}

// ---------------------------------------------------------------------------
// Mappers — API row types → PaletteItem[]
// ---------------------------------------------------------------------------

/** First 8 chars of an id — enough to disambiguate in a subtitle. */
function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : '';
}

export function runItems(rows: RunListRow[]): PaletteItem[] {
  return rows.map((r) => ({
    kind: 'run' as const,
    id: r.id,
    title: r.workflow_name,
    subtitle: `${shortId(r.id)} · ${r.status}`,
    to: `/runs/${r.id}`,
    keywords: r.id,
  }));
}

export function agentItems(rows: AgentSummary[]): PaletteItem[] {
  return rows.map((a) => ({
    kind: 'agent' as const,
    id: a.name,
    title: a.name,
    subtitle: [a.provider, a.model].filter(Boolean).join('/') || undefined,
    to: `/agents/${encodeURIComponent(a.name)}`,
  }));
}

export function workflowItems(rows: WorkflowSummary[]): PaletteItem[] {
  return rows.map((w) => ({
    kind: 'workflow' as const,
    id: w.name,
    title: w.name,
    subtitle: w.scope,
    to: `/workflows/${encodeURIComponent(w.name)}`,
  }));
}

export function autoflowItems(rows: AutoflowDefRow[]): PaletteItem[] {
  return rows.map((a) => ({
    kind: 'autoflow' as const,
    id: a.slug,
    title: a.name,
    subtitle: `autoflow · ${a.trigger}`,
    // Autoflows reuse the workflow detail route, keyed by slug.
    to: `/workflows/${a.slug}`,
  }));
}

export function sessionItems(rows: SessionSummary[]): PaletteItem[] {
  return rows.map((s) => {
    const status = str(s.status);
    return {
      kind: 'session' as const,
      id: s.session_id,
      title: s.agent_name,
      subtitle: [shortId(s.session_id), status].filter(Boolean).join(' · '),
      to: `/sessions/${s.session_id}`,
      keywords: s.session_id,
    };
  });
}

export function projectItems(rows: ProjectRow[]): PaletteItem[] {
  return rows.map((p) => ({
    kind: 'project' as const,
    id: p.ws_id,
    title: p.name,
    subtitle: p.path,
    to: `/projects/${p.ws_id}`,
  }));
}

export function coverageItems(rows: CoverageSummary[]): PaletteItem[] {
  return rows.map((c) => ({
    kind: 'coverage' as const,
    id: c.target_id,
    title: c.target_id,
    subtitle: c.project,
    to: `/coverage/${encodeURIComponent(c.target_id)}`,
  }));
}

export function findingItems(rows: FindingOut[]): PaletteItem[] {
  // No per-finding detail route — all findings link to the findings list.
  return rows.map((f) => ({
    kind: 'finding' as const,
    id: f.id,
    title: f.summary,
    subtitle: [f.severity, f.file_path].filter(Boolean).join(' · ') || undefined,
    to: '/findings',
  }));
}

export function issueItems(rows: AutoflowClaim[]): PaletteItem[] {
  return rows.map((c) => ({
    kind: 'issue' as const,
    id: c.issue_ref,
    title: c.issue_title || c.issue_display_ref || c.issue_ref,
    subtitle: [c.issue_display_ref, c.status].filter(Boolean).join(' · ') || undefined,
    to: c.last_run_id ? `/runs/${c.last_run_id}` : '/runs/autoflows',
    keywords: c.issue_ref,
  }));
}

export function workerItems(rows: WorkerRecord[]): PaletteItem[] {
  // No per-worker detail route — all workers link to the workers list.
  return rows.map((w) => ({
    kind: 'worker' as const,
    id: w.worker_id,
    title: w.name,
    subtitle: [w.kind, w.host].filter(Boolean).join(' · ') || undefined,
    to: '/workers',
  }));
}
