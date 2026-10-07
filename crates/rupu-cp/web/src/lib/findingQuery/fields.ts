// The findings query field registry: the grammar spec (key/aliases/kind/
// values, in lockstep with rupu-coverage `FIELDS` via fields.json) plus the
// presentation the QueryBar shows (label, description, lucide icon).
import {
  Bot, Bug, CircleCheck, Crosshair, FileCode, FileText, FolderGit2, Hash, ListChecks, Package,
  Play, ShieldAlert, ShieldCheck, Tag, User, Workflow, type LucideIcon,
} from 'lucide-react';
import type { FieldSpec } from './grammar';

export interface QueryField extends FieldSpec {
  label: string;
  description: string;
  icon: LucideIcon;
}

export const FINDING_FIELDS: QueryField[] = [
  { key: 'severity', aliases: ['sev'], kind: 'severity', values: ['info', 'low', 'medium', 'high', 'critical'], label: 'Severity', description: 'severity:high · severity>=high', icon: ShieldAlert },
  { key: 'tag', aliases: [], kind: 'tag', values: [], label: 'Tag', description: 'tag:needs-poc · -tag:false-positive', icon: Tag },
  { key: 'has', aliases: [], kind: 'enum', values: ['tags', 'report', 'poc', 'cwe'], label: 'Has', description: 'has:poc · -has:tags (untagged)', icon: CircleCheck },
  { key: 'project', aliases: [], kind: 'text', values: [], label: 'Project', description: 'project:shop-web', icon: FolderGit2 },
  { key: 'cwe', aliases: [], kind: 'cwe', values: [], label: 'CWE', description: 'cwe:79 · cwe:CWE-89', icon: Bug },
  { key: 'owner', aliases: [], kind: 'text', values: [], label: 'Owner', description: 'owner:"Payments Team"', icon: User },
  { key: 'product', aliases: [], kind: 'text', values: [], label: 'Product', description: 'product:checkout', icon: Package },
  { key: 'verified', aliases: [], kind: 'enum', values: ['unverified', 'confirmed', 'disputed', 'inconclusive'], label: 'Verified', description: 'verified:confirmed', icon: ShieldCheck },
  { key: 'profile', aliases: [], kind: 'enum', values: ['full', 'summary'], label: 'Profile', description: 'profile:full', icon: FileText },
  { key: 'scope', aliases: [], kind: 'enum', values: ['line', 'file', 'repo', 'host', 'endpoint', 'resource'], label: 'Scope', description: 'scope:host', icon: Crosshair },
  { key: 'concern', aliases: [], kind: 'text', values: [], label: 'Concern', description: 'concern:authz-idor', icon: ListChecks },
  { key: 'agent', aliases: [], kind: 'text', values: [], label: 'Agent', description: 'agent name or codename', icon: Bot },
  { key: 'workflow', aliases: [], kind: 'text', values: [], label: 'Workflow', description: 'workflow:review-flow', icon: Workflow },
  { key: 'file', aliases: [], kind: 'text', values: [], label: 'File', description: 'file:src/api/ (path prefix)', icon: FileCode },
  { key: 'run', aliases: [], kind: 'text', values: [], label: 'Run', description: 'run:<id> (includes its sub-runs)', icon: Play },
  { key: 'id', aliases: [], kind: 'text', values: [], label: 'Finding id', description: 'id:fnd_…', icon: Hash },
];
