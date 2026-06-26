// Renders a list of strings (file paths), showing the first `cap` then a
// "show all N" toggle so one huge concern can't flood the view.
import { useState } from 'react';

export default function CappedList({ items, cap = 10 }: { items: string[]; cap?: number }) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? items : items.slice(0, cap);
  return (
    <div>
      <ul className="space-y-0.5">
        {shown.map((f) => (
          <li key={f} className="text-[11px] font-mono text-ink-mute break-all">
            {f}
          </li>
        ))}
      </ul>
      {items.length > cap && (
        <button
          onClick={() => setExpanded((v) => !v)}
          className="mt-1 text-[11px] font-medium text-brand-700 hover:text-brand-500"
        >
          {expanded ? 'show less' : `show all ${items.length}`}
        </button>
      )}
    </div>
  );
}
