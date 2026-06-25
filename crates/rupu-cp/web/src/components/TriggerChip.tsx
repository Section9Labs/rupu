// Trigger badge — manual / cron / event, shared across project tab bodies.
// Lifted verbatim from ProjectDetail.tsx so the new Runs tab can import it.
// Static Tailwind classes keyed off a small map.

const TRIGGER_CHIP_CLS: Record<string, string> = {
  manual: 'bg-slate-100 text-slate-600',
  cron: 'bg-violet-50 text-violet-700',
  event: 'bg-sky-50 text-sky-700',
};

export function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CHIP_CLS[trigger] ?? 'bg-slate-100 text-slate-600';
  return (
    <span
      className={`inline-flex items-center rounded text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5 ${cls}`}
    >
      {trigger}
    </span>
  );
}
