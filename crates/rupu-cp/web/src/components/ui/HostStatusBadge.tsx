// HostStatusBadge — a small chip that encodes host health status using the
// project's semantic color tokens so it works in both light and dark themes.
// Replaces the duplicated HOST_STATUS_CLASS maps in Hosts.tsx + HostDetail.tsx.

import { Chip } from './Chip';
import type { HostStatus } from '../../lib/api';

const STATUS_CLS: Record<HostStatus, string> = {
  online:  'bg-ok-bg text-ok ring-ok/30',
  stale:   'bg-warn-bg text-warn ring-warn/30',
  offline: 'bg-err-bg text-err ring-err/30',
};

export function HostStatusBadge({ status }: { status: HostStatus }) {
  return <Chip className={STATUS_CLS[status]}>{status}</Chip>;
}
