// TargetPicker — Spotlight-style grouped fuzzy picker for run targets.
//
// Pure logic (`rankAndGroup`) is exported so tests can import it directly
// without any React/DOM setup.

import { useEffect, useRef, useState } from 'react';
import { fuzzyScore } from '../lib/fuzzy';
import {
  WORKSPACE_ITEM,
  projectItems,
  repoItems,
  dirItems,
  inferFreeTextItem,
  looksLikePath,
  type TargetKind,
  type TargetItem,
} from '../lib/targetItems';
import { api } from '../lib/api';
import type { ProjectRow, RepoEntry } from '../lib/api';

// ---------------------------------------------------------------------------
// Pure ranking — exported for unit tests.
// ---------------------------------------------------------------------------

const KIND_ORDER: TargetKind[] = ['workspace', 'project', 'repo', 'directory'];
const GROUP_CAP = 6;

export interface RankedItem {
  item: TargetItem;
  matched: number[];
}

export interface RankedGroup {
  kind: TargetKind;
  items: RankedItem[];
}

/**
 * Score, filter, sort and group `items` by `query`.
 *
 * - Empty query: all items pass with score 0.
 * - Non-empty query: `fuzzyScore` on `label`; if null, fallback to `sublabel`
 *   (matched indices stay [] since they reference sublabel positions, not label).
 * - Groups are returned in fixed order: workspace → project → repo → directory.
 * - Each group is sorted descending by score and capped at 6 items.
 */
export function rankAndGroup(items: TargetItem[], query: string): RankedGroup[] {
  const scored: { item: TargetItem; score: number; matched: number[] }[] = [];

  for (const item of items) {
    if (query === '') {
      scored.push({ item, score: 0, matched: [] });
      continue;
    }
    // Try label.
    const byLabel = fuzzyScore(query, item.label);
    if (byLabel !== null) {
      scored.push({ item, score: byLabel.score, matched: byLabel.matched });
      continue;
    }
    // Fallback: sublabel (matched indices belong to the sublabel string, so
    // we return [] for label highlighting purposes).
    if (item.sublabel) {
      const bySub = fuzzyScore(query, item.sublabel);
      if (bySub !== null) {
        scored.push({ item, score: bySub.score, matched: [] });
      }
    }
  }

  // Bucket by kind.
  const byKind = new Map<TargetKind, typeof scored>();
  for (const k of KIND_ORDER) byKind.set(k, []);
  for (const s of scored) {
    byKind.get(s.item.kind)?.push(s);
  }

  const groups: RankedGroup[] = [];
  for (const kind of KIND_ORDER) {
    const bucket = byKind.get(kind)!;
    if (bucket.length === 0) continue;
    bucket.sort((a, b) => b.score - a.score);
    groups.push({
      kind,
      items: bucket.slice(0, GROUP_CAP).map((s) => ({ item: s.item, matched: s.matched })),
    });
  }

  return groups;
}

// ---------------------------------------------------------------------------
// Highlight helper.
// ---------------------------------------------------------------------------

function HighlightedLabel({ label, matched }: { label: string; matched: number[] }) {
  const set = new Set(matched);
  return (
    <>
      {Array.from(label).map((ch, i) =>
        set.has(i) ? (
          <mark key={i} className="bg-transparent text-brand-700 font-semibold">
            {ch}
          </mark>
        ) : (
          <span key={i}>{ch}</span>
        ),
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Chip colors per kind.
// ---------------------------------------------------------------------------

const KIND_CHIP: Record<TargetKind, string> = {
  project:   'bg-ok-bg text-ok ring-ok/30',
  repo:      'bg-violet-50 text-violet-700 ring-violet-200',
  directory: 'bg-surface text-ink-mute ring-border',
  workspace: 'bg-surface text-ink-mute ring-border',
};

const KIND_LABEL: Record<TargetKind, string> = {
  project:   'project',
  repo:      'repo',
  directory: 'directory',
  workspace: 'workspace',
};

// ---------------------------------------------------------------------------
// Component.
// ---------------------------------------------------------------------------

export interface TargetPickerProps {
  value: TargetItem;
  onChange: (item: TargetItem) => void;
  disabled?: boolean;
}

const INPUT_CLS =
  'w-full rounded border border-border bg-panel px-2.5 py-1.5 text-lead text-ink ' +
  'placeholder:text-ink-mute focus:outline-none focus:ring-1 focus:ring-brand-500 ' +
  'disabled:cursor-not-allowed disabled:opacity-50';

export default function TargetPicker({ value, onChange, disabled }: TargetPickerProps) {
  const [query, setQuery]       = useState(value.label);
  const [open, setOpen]         = useState(false);
  const [active, setActive]     = useState(0);
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [repos, setRepos]       = useState<RepoEntry[]>([]);
  const [dirs, setDirs]         = useState<TargetItem[]>([]);

  const containerRef  = useRef<HTMLDivElement>(null);
  const blurTimer     = useRef<ReturnType<typeof setTimeout> | null>(null);
  const browseTimer   = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Sync input text when the value prop changes externally.
  useEffect(() => { setQuery(value.label); }, [value]);

  // Fetch projects + repos once on mount; tolerate errors.
  useEffect(() => {
    api.getProjects().then(setProjects).catch(() => setProjects([]));
    api.getRepos().then(setRepos).catch(() => setRepos([]));
  }, []);

  // Debounced directory browse.
  useEffect(() => {
    if (browseTimer.current) clearTimeout(browseTimer.current);
    if (!looksLikePath(query)) {
      setDirs([]);
      return;
    }
    let cancelled = false;
    browseTimer.current = setTimeout(() => {
      api
        .browseDir(query)
        .then((res) => { if (!cancelled) setDirs(dirItems(res.dirs)); })
        .catch(() => { if (!cancelled) setDirs([]); });
    }, 150);
    return () => {
      cancelled = true;
      if (browseTimer.current) clearTimeout(browseTimer.current);
    };
  }, [query]);

  // Close on outside click.
  useEffect(() => {
    function onOutside(e: MouseEvent) {
      if (!containerRef.current?.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener('mousedown', onOutside);
    return () => document.removeEventListener('mousedown', onOutside);
  }, []);

  // Clean up timers on unmount.
  useEffect(() => () => {
    if (blurTimer.current)  clearTimeout(blurTimer.current);
    if (browseTimer.current) clearTimeout(browseTimer.current);
  }, []);

  // Build candidate list each render.
  const baseItems: TargetItem[] = [
    WORKSPACE_ITEM,
    ...projectItems(projects),
    ...repoItems(repos),
    ...dirs,
  ];
  const freeText = inferFreeTextItem(query);
  const candidates = [...baseItems];
  if (freeText && !baseItems.some((it) => it.label === freeText.label)) {
    candidates.push(freeText);
  }

  const groups = rankAndGroup(candidates, query);
  const flat: RankedItem[] = groups.flatMap((g) => g.items);
  const clampedActive = Math.min(active, Math.max(0, flat.length - 1));

  function select(item: TargetItem) {
    onChange(item);
    setQuery(item.label);
    setOpen(false);
    setActive(0);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (!open) {
      if (e.key === 'ArrowDown') { setOpen(true); e.preventDefault(); }
      return;
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setActive((a) => Math.min(a + 1, flat.length - 1));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setActive((a) => Math.max(a - 1, 0));
        break;
      case 'Enter':
        e.preventDefault();
        if (flat[clampedActive]) select(flat[clampedActive].item);
        break;
      case 'Escape':
        e.preventDefault();
        setOpen(false);
        break;
    }
  }

  // Render — we track a mutable index across groups for keyboard-active highlight.
  let flatIdx = 0;

  return (
    <div ref={containerRef} className="relative">
      <input
        type="text"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open}
        value={query}
        placeholder="search projects, repos, or a path…"
        disabled={disabled}
        className={INPUT_CLS}
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
          setActive(0);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => {
          // Resolve the current query text and commit to parent synchronously
          // (before the blur timer, so the parent has the correct value when
          // e.g. the Launch button's onClick fires immediately after blur).
          const q = query.trim();
          if (q === '') {
            onChange(WORKSPACE_ITEM);
            setQuery(WORKSPACE_ITEM.label);
          } else {
            // Exact case-insensitive label match against base candidates.
            const matched = baseItems.find(
              (it) => it.label.toLowerCase() === q.toLowerCase(),
            );
            if (matched) {
              onChange(matched);
            } else {
              const free = inferFreeTextItem(q);
              if (free) {
                onChange(free);
              } else {
                // Unresolvable — revert the visible text to the last committed value.
                setQuery(value.label);
              }
            }
          }
          // Schedule dropdown close (delay lets row mousedown fire first).
          blurTimer.current = setTimeout(() => setOpen(false), 150);
        }}
        onKeyDown={onKeyDown}
      />

      {open && groups.length > 0 && (
        <div className="absolute z-20 mt-1 w-full overflow-hidden rounded-md border border-border bg-panel shadow-lg">
          {groups.map((group) => (
            <div key={group.kind}>
              {/* Group header chip */}
              <div className="flex items-center gap-1.5 px-2.5 pt-2 pb-1">
                <span
                  className={
                    'inline-flex items-center rounded px-1.5 py-0.5 text-note font-medium ring-1 ' +
                    KIND_CHIP[group.kind]
                  }
                >
                  {KIND_LABEL[group.kind]}
                </span>
              </div>

              {/* Group rows */}
              {group.items.map((ranked) => {
                const myIdx = flatIdx++;
                const isActive = myIdx === clampedActive;
                return (
                  <div
                    key={`${ranked.item.kind}-${ranked.item.label}`}
                    role="option"
                    aria-selected={isActive}
                    onMouseDown={(e) => {
                      // Prevent the input blur from closing the dropdown before
                      // we can register the click.
                      e.preventDefault();
                      select(ranked.item);
                    }}
                    className={
                      'cursor-pointer px-2.5 py-1.5 ' +
                      (isActive ? 'bg-brand-50' : 'hover:bg-surface-hover')
                    }
                  >
                    <div className="text-lead leading-snug text-ink">
                      <HighlightedLabel label={ranked.item.label} matched={ranked.matched} />
                    </div>
                    {ranked.item.sublabel && (
                      <div className="truncate text-note leading-snug text-ink-mute">
                        {ranked.item.sublabel}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
