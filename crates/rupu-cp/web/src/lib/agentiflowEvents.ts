// agentiflowEvents — adapt an agentiflow run's lifecycle events
// (`run_started` / `round` / `run_stopped`, from `GET /api/agentiflows/:id`)
// into the shared Situation Room `StreamCard`s, so the agentiflow Events tab
// renders through the SAME `RunEventFeed` / `EventCard` the run detail and the
// global /events wall use — rather than a bespoke list.
//
// Agentiflow events are a different schema than the orchestrator `RunEvent`s
// `cardFromEvent` understands, hence this dedicated mapper. Cards are tinted by
// the lead's crew and returned newest-first.

import type { StreamCard, CardAccent } from './situationRoom/cards';
import type { AgentiflowEvent } from './api';
import { parseCodename } from './codename';
import { stopReasonTone, formatSpendUsd, formatSpendTokens } from './agentiflow';

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}

export function agentiflowEventCards(events: AgentiflowEvent[], codename?: string): StreamCard[] {
  const crew = (codename && parseCodename(codename).crew) || undefined;
  const out: StreamCard[] = [];

  events.forEach((ev, i) => {
    const ts = Date.parse(str(ev.ts) ?? '') || 0;
    const base = { key: `af-ev-${i}-${ts}`, ts, crew };
    const kind = str(ev.kind);

    if (kind === 'run_started') {
      const raw = ev as Record<string, unknown>;
      const profiles = Array.isArray(raw.engagement_profiles) ? (raw.engagement_profiles as unknown[]).map(String).join(', ') : undefined;
      const goals = num(raw.goals);
      const detail = [profiles && `profiles ${profiles}`, goals != null ? `${goals} goal${goals === 1 ? '' : 's'}` : null].filter(Boolean).join(' · ') || undefined;
      out.push({ ...base, form: 'lifecycle', group: 'activity', accent: 'brand', badge: 'Started', title: 'Engagement started', detail });
    } else if (kind === 'round') {
      const raw = ev as Record<string, unknown>;
      const n = num(raw.round) ?? 0;
      const err = str(raw.error);
      const isErr = str(raw.outcome) === 'error' || !!err;
      const met = num(raw.goals_met);
      const total = num(raw.goals_total);
      const budget = str(raw.budget);
      const usd = num(raw.spent_usd);
      const tokens = num(raw.spent_tokens);
      const parts: string[] = [];
      if (met != null && total != null) parts.push(`goals ${met}/${total}`);
      if (budget) parts.push(budget);
      if (raw.converge === true) parts.push('converging');
      if (usd != null || tokens != null) parts.push(`${formatSpendUsd(usd)}${tokens != null ? ` · ${formatSpendTokens(tokens)}` : ''}`);
      out.push({
        ...base,
        form: isErr ? 'error' : 'activity',
        group: isErr ? 'error' : 'activity',
        accent: isErr ? 'error' : 'brand',
        badge: `Round ${n}`,
        title: isErr ? `Round ${n} — failed` : `Round ${n}`,
        detail: isErr ? err ?? 'error' : parts.join(' · ') || undefined,
      });
    } else if (kind === 'run_stopped') {
      const reason = str((ev as Record<string, unknown>).stop_reason);
      const tone = reason ? stopReasonTone(reason) : 'neutral';
      const accent: CardAccent = tone === 'ok' ? 'brand' : tone === 'err' ? 'error' : tone === 'warn' ? 'warn' : 'brand';
      out.push({ ...base, form: 'lifecycle', group: tone === 'err' ? 'error' : 'activity', accent, badge: 'Stopped', title: 'Engagement stopped', detail: reason });
    } else if (kind === 'af_unit_started') {
      const raw = ev as Record<string, unknown>;
      const cn = str(raw.codename);
      out.push({
        ...base,
        crew: (cn && parseCodename(cn).crew) || crew,
        form: 'activity',
        group: 'activity',
        accent: 'brand',
        badge: str(raw.unit_kind) === 'workflow' ? 'Workflow' : 'Dispatched',
        title: str(raw.agent) ?? str(raw.participant) ?? 'unit',
        detail: str(raw.participant),
        codename: cn,
        codenameDerived: raw.codename_derived === true,
        agent: str(raw.agent),
        transcriptPath: str(raw.transcript_path),
      });
    } else if (kind === 'af_unit_completed') {
      const raw = ev as Record<string, unknown>;
      const cn = str(raw.codename);
      const failed = raw.success === false || !!str(raw.error);
      out.push({
        ...base,
        crew: (cn && parseCodename(cn).crew) || crew,
        form: failed ? 'error' : 'complete',
        group: failed ? 'error' : 'activity',
        accent: failed ? 'error' : 'brand',
        badge: failed ? 'Failed' : 'Done',
        title: str(raw.agent) ?? str(raw.participant) ?? 'unit',
        detail: str(raw.error) ?? str(raw.output),
        codename: cn,
        codenameDerived: raw.codename_derived === true,
        agent: str(raw.agent),
        provider: str(raw.provider),
        model: str(raw.model),
        tokensIn: num(raw.input_tokens),
        tokensOut: num(raw.output_tokens),
        transcriptPath: str(raw.transcript_path),
      });
    } else if (kind) {
      // An unrecognized kind still gets a plain activity card so nothing is dropped.
      const label = kind.replace(/_/g, ' ');
      out.push({ ...base, form: 'activity', group: 'activity', accent: 'brand', badge: label, title: label });
    }
  });

  return out.reverse(); // newest-first, like RunEventFeed
}
