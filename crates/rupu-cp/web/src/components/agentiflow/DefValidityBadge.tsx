// The server's verdict on an agentiflow definition draft: "✓ valid" or
// "✕ invalid" (the problem in its title; the page shows it in full).

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
  // The full message sits under the editor; the badge only says which way.
  return (
    <span
      className="inline-flex items-center rounded-full px-2 py-0.5 text-note font-medium ring-1 bg-err-bg text-err ring-err/30"
      title={validity.error}
    >
      ✕ invalid
    </span>
  );
}

export default DefValidityBadge;
