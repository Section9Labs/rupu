// Shared severity pill, used across the coverage concern tabs.
export default function SeverityChip({ severity }: { severity: string }) {
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
      {severity}
    </span>
  );
}
