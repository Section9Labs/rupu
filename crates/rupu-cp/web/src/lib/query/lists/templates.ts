// The concern-template catalog's query fields.
import { FileText, ListChecks, ShieldAlert, Tag } from 'lucide-react';
import type { TemplateSummary } from '../../api';
import { rowField, type RowField } from '../rows';

export const TEMPLATE_FIELDS: RowField<TemplateSummary>[] = [
  rowField({ key: 'name', match: 'contains', get: (t) => t.name, text: true, label: 'Template', description: 'name:owasp (contains)', icon: ListChecks }),
  rowField({ key: 'description', aliases: ['desc'], match: 'contains', get: (t) => t.description, text: true, label: 'Description', description: 'description:web', icon: FileText }),
  rowField({ key: 'version', match: 'exact', get: (t) => t.version, facet: true, label: 'Version', description: 'version:3', icon: Tag }),
  rowField({
    key: 'severity',
    aliases: ['sev'],
    values: ['info', 'low', 'medium', 'high', 'critical'],
    match: 'ordered',
    get: (t) => Object.entries(t.severity_breakdown).filter(([, n]) => n > 0).map(([s]) => s),
    facet: true,
    label: 'Severity',
    description: 'severity:critical · severity>=high (has a concern of it)',
    icon: ShieldAlert,
  }),
];
