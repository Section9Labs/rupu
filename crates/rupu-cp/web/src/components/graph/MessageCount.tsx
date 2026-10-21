// The message-count badge a card carries in its head (its arcs are drawn on
// hover, focus or selection, and past a space's ARC_REST_LIMIT only so): the
// run graph's step cards, an agentiflow round, child and workflow frame.

import { Mail } from 'lucide-react';

export function MessageCount({ n }: { n: number | undefined }) {
  if (!n) return null;
  return (
    <span className="gx-count afx-mcount" title={`${n} message${n === 1 ? '' : 's'}`} data-testid="flow-mcount">
      <Mail size={11} aria-hidden />
      {n}
    </span>
  );
}
