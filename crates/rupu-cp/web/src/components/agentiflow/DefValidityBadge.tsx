// The server's verdict on an agentiflow definition draft, as the workflow
// editor's ValidityBadge shows it: "✓ valid", or the first problem.

import type { DefValidity } from '../../lib/api3b/agentiflows';

export function DefValidityBadge({ validity }: { validity: DefValidity | null }) {
  if (!validity) return null;
  if (validity.ok) {
    return (
      <span className="inline-flex items-center rounded-full px-2 py-0.5 text-note font-medium ring-1 bg-ok-bg text-ok ring-ok/30">
        ✓ valid
      </span>
    );
  }
  return (
    <span
      className="inline-flex max-w-[20rem] items-center truncate rounded-full px-2 py-0.5 text-note font-medium ring-1 bg-err-bg text-err ring-err/30"
      title={validity.error}
    >
      ✕ {validity.error}
    </span>
  );
}

export default DefValidityBadge;
