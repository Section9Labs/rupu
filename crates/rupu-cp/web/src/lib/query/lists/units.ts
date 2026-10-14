// A fan-out step's units list (the run's transcript browser): its query
// fields.
import { AlertTriangle, Bot, CircleDot, Cpu, Fingerprint, KeyRound, Server } from 'lucide-react';
import type { UnitView } from '../../runGraphModel';
import { rowField, type RowField } from '../rows';

export const UNIT_STATES = ['pending', 'running', 'awaiting_approval', 'paused', 'done', 'failed', 'skipped'];

export const UNIT_FIELDS: RowField<UnitView>[] = [
  rowField({ key: 'state', aliases: ['status'], values: UNIT_STATES, match: 'exact', get: (u) => u.state, facet: true, label: 'State', description: 'state:failed · state:running,awaiting_approval', icon: CircleDot }),
  rowField({ key: 'key', match: 'contains', get: (u) => u.key, text: true, label: 'Unit key', description: 'key:src/api (the fan-out item, contains)', icon: KeyRound }),
  rowField({ key: 'codename', match: 'contains', get: (u) => u.codename, text: true, label: 'Codename', description: 'codename:heron#4', icon: Fingerprint }),
  rowField({ key: 'agent', match: 'contains', get: (u) => u.agent, text: true, facet: true, label: 'Agent', description: 'agent:scanner', icon: Bot }),
  rowField({ key: 'provider', match: 'exact', get: (u) => u.provider, facet: true, label: 'Provider', description: 'provider:anthropic', icon: Server }),
  rowField({ key: 'model', match: 'exact', get: (u) => u.model, facet: true, label: 'Model', description: 'model:claude-opus-4-1', icon: Cpu }),
  rowField({ key: 'has', values: ['warning', 'transcript'], match: 'exact', get: (u) => [(u.warnings?.length ?? 0) > 0 ? 'warning' : null, u.transcriptPath ? 'transcript' : null], facet: true, label: 'Has', description: 'has:warning · -has:transcript', icon: AlertTriangle }),
];
