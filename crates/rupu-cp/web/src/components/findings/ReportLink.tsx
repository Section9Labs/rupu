// The one link to a finding's `/findings/:id` report page. It carries the
// page it was clicked on, so the report's back link returns there.

import { Link } from 'react-router-dom';
import { findingPath } from '../../lib/findingWorkspace';
import { useReturnHere } from '../../lib/backLink';

export function ReportLink({
  id,
  wsId,
  className,
  children,
}: {
  id: string;
  wsId?: string | null;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <Link to={findingPath(id, wsId)} state={useReturnHere()} className={className}>
      {children}
    </Link>
  );
}
