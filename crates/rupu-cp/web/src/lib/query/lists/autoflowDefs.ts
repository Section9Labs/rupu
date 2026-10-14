// The Build › Autoflows definition list's query fields.
import { FolderGit2, Timer, Workflow, Zap } from 'lucide-react';
import type { AutoflowDefRow } from '../../api';
import { rowField, type RowField } from '../rows';

export const AUTOFLOW_DEF_FIELDS: RowField<AutoflowDefRow>[] = [
  rowField({ key: 'name', match: 'contains', get: (d) => [d.name, d.slug], text: true, label: 'Name', description: 'name:nightly (name or file, contains)', icon: Workflow }),
  rowField({ key: 'scope', aliases: ['project'], match: 'exact', get: (d) => d.scope, facet: true, label: 'Scope', description: 'scope:global · scope:<project>', icon: FolderGit2 }),
  rowField({ key: 'trigger', values: ['manual', 'cron', 'event'], match: 'exact', get: (d) => d.trigger, facet: true, label: 'Trigger', description: 'trigger:cron · trigger:event', icon: Zap }),
  rowField({ key: 'is', values: ['enabled', 'disabled'], match: 'exact', get: (d) => (d.enabled ? 'enabled' : 'disabled'), facet: true, label: 'Is', description: 'is:enabled · is:disabled', icon: Timer }),
];
