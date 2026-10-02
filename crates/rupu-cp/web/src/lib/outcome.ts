/**
 * TS port of `rupu_transcript::outcome::recovery_line` — the one-line phrase
 * for a recovery action, kept identical to the CLI's. Plan 3 replaces both
 * with the full presentation module.
 */

export interface RecoveryLineInput {
  rung: number;
  action: string;
  attempt?: number | null;
  budget?: number | null;
  provider?: string | null;
  model?: string | null;
  reason?: string | null;
}

function target(provider?: string | null, model?: string | null): string {
  if (provider && model) return `${provider}/${model}`;
  return model || provider || '?';
}

export function recoveryLine(r: RecoveryLineInput): string {
  let phrase: string;
  switch (r.action) {
    case 'continued':
      phrase =
        r.attempt != null && r.budget != null ? `continued ${r.attempt}/${r.budget}` : 'continued';
      break;
    case 'fell_back':
      phrase = `fell back to ${target(r.provider, r.model)}`;
      break;
    case 'served_by_fallback':
      phrase = `served by ${r.model || target(r.provider, r.model)}`;
      break;
    case 'skipped':
      phrase = r.reason
        ? `skipped ${target(r.provider, r.model)}: ${r.reason}`
        : `skipped ${target(r.provider, r.model)}`;
      break;
    case 'failed':
      phrase = 'no recovery left';
      break;
    default:
      // retried, compacted, asked, parked — and any action a newer writer adds.
      phrase = r.action;
  }
  return `↺ rung ${r.rung} · ${phrase}`;
}
