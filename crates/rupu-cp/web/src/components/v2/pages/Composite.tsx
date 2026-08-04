// Interim v2 destination page: a Segmented tab bar over existing page
// bodies, so the 7-leaf IA is complete and capability-equal from Plan 1.
// The active tab lives in the `?tab=` search param so redirects from old
// routes can deep-link a specific tab and the URL stays shareable.
import { Suspense, type ReactNode } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Segmented } from '../../ui/Segmented';
import { Spinner } from '../../ui/Spinner';

export interface CompositeTab {
  value: string;
  label: string;
  element: ReactNode;
}

export function Composite({ title, tabs, defaultTab }: {
  title: string;
  tabs: CompositeTab[];
  defaultTab: string;
}) {
  const [params, setParams] = useSearchParams();
  const raw = params.get('tab');
  const tab = tabs.some((t) => t.value === raw) ? (raw as string) : defaultTab;
  const active = tabs.find((t) => t.value === tab) ?? tabs[0];
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-3 border-b border-border bg-panel px-4 py-2">
        <h1 className="text-[16px] font-semibold tracking-[-0.01em] text-ink">{title}</h1>
        <Segmented
          size="sm"
          ariaLabel={`${title} sections`}
          options={tabs.map((t) => ({ value: t.value, label: t.label }))}
          value={tab}
          onChange={(next) =>
            setParams((p) => {
              const q = new URLSearchParams(p);
              q.set('tab', next);
              return q;
            }, { replace: true })
          }
        />
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense fallback={<div className="flex h-48 items-center justify-center"><Spinner size="md" label="Loading…" /></div>}>
          {active.element}
        </Suspense>
      </div>
    </div>
  );
}
