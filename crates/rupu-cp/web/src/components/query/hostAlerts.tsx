// The one quiet line under a per-host list's query bar, when some host's
// rows are missing: who, and why. A host whose older rows failed to load
// gets a Retry.
import { Fragment, type ReactNode } from 'react';
import { noteText, type HostNote } from '../../lib/query/serverList';

export function hostAlerts(notes: readonly HostNote[], noun: string, onRetry: (hostId: string) => void): ReactNode[] {
  if (notes.length === 0) return [];
  return [
    <span key="hosts" data-testid="host-alert">
      {notes.map((n, i) => (
        <Fragment key={`${n.kind}:${n.hostId}:${n.kind === 'field' ? n.field : ''}`}>
          {i > 0 && ' · '}
          {noteText(n, noun)}
          {n.kind === 'paging' && (
            <>
              {' '}
              <button type="button" className="underline" aria-label={`Retry ${n.name}`} onClick={() => onRetry(n.hostId)}>
                Retry
              </button>
            </>
          )}
        </Fragment>
      ))}
    </span>,
  ];
}
