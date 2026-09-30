import Markdown from '../../transcript/Markdown';

export default function ReplicationSteps({ steps }: { steps: string[] }) {
  return (
    <ol className="space-y-2">
      {steps.map((s, i) => (
        <li key={i} className="grid grid-cols-[1.5rem_minmax(0,1fr)] gap-2">
          <span className="grid h-5 w-5 place-items-center rounded-full bg-surface text-meta font-semibold text-ink ring-1 ring-border">{i + 1}</span>
          <div className="text-ink-dim [&_p]:text-ui"><Markdown text={s} /></div>
        </li>
      ))}
    </ol>
  );
}
