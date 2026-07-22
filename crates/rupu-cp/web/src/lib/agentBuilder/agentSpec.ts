// Pure model layer mapping a rupu agent `.md` (YAML frontmatter + markdown
// body) to/from an editable `AgentDraft`. Mirrors the Rust `Frontmatter`
// struct in `crates/rupu-agent/src/spec.rs` (`#[serde(deny_unknown_fields)]`)
// — emitting any key outside that struct's field list breaks agent creation,
// so `serializeAgent` only ever writes the allowlisted keys below plus
// whatever unknown keys were preserved via `_passthrough` when parsing an
// existing file.
import yaml from 'js-yaml';
import { splitFrontmatter } from '../../components/CodeHighlight';

// ── Vocab consts ────────────────────────────────────────────────────────

export const PROVIDERS = ['anthropic', 'openai', 'gemini', 'copilot', 'openai-compatible'] as const;
export const AUTH_MODES = ['api-key', 'sso'] as const;
export const PERMISSION_MODES = ['ask', 'bypass', 'readonly'] as const;
export const EFFORT_LEVELS = ['auto', 'minimal', 'low', 'medium', 'high', 'max'] as const;
export const CONTEXT_WINDOWS = ['default', '1m'] as const;
export const OUTPUT_FORMATS = ['text', 'json'] as const;
export const ANTHROPIC_SPEED = ['fast'] as const;
export const ANTHROPIC_CTX_MGMT = ['tool_clearing'] as const;
export const BUILTIN_TOOLS = [
  'bash',
  'read_file',
  'write_file',
  'edit_file',
  'ast_grep',
  'grep',
  'glob',
  'dispatch_agent',
  'dispatch_agents_parallel',
] as const;

// ── Types ───────────────────────────────────────────────────────────────

export interface SchemaProp {
  name: string;
  type: 'string' | 'number' | 'boolean' | 'enum' | 'array' | 'object';
  enumValues?: string[];
}

export type Severity = 'info' | 'low' | 'medium' | 'high' | 'critical';

export interface InlineConcern {
  kind: 'inline';
  id: string;
  name: string;
  description: string;
  severity?: Severity;
  applicableGlobs?: string[];
}

export interface ConcernOverride {
  id: string;
  severity?: Severity;
  applicableGlobs?: string[];
}

export interface IncludeConcern {
  kind: 'include';
  template: string;
  mode?: 'full' | 'index' | 'auto';
  overrides?: ConcernOverride[];
}

export type ConcernEntry = InlineConcern | IncludeConcern;

export interface AgentDraft {
  name: string;
  description?: string;
  provider?: string;
  auth?: string;
  model?: string;
  tools?: string[];
  maxTurns?: number;
  permissionMode?: string;
  anthropicOauthPrefix?: boolean;
  effort?: string;
  contextWindow?: string;
  outputFormat?: string;
  outputSchema?: SchemaProp[];
  anthropicTaskBudget?: number;
  anthropicContextManagement?: string;
  anthropicSpeed?: string;
  dispatchableAgents?: string[];
  concerns?: ConcernEntry[];
  maxTokens?: number;
  contextWindowTokens?: number;
  compactAtPercent?: number;
  body: string;
  /** Unknown frontmatter keys parsed from an existing file, preserved and
   *  re-emitted verbatim so editing never silently drops fields the UI
   *  doesn't model yet. */
  _passthrough?: Record<string, unknown>;
}

export function emptyDraft(): AgentDraft {
  return { name: '', body: '' };
}

// ── Serialize ───────────────────────────────────────────────────────────

/** Stable frontmatter key order (name first). Must match the Rust
 *  `Frontmatter` struct's field order in `crates/rupu-agent/src/spec.rs`. */
const KEY_ORDER: (keyof AgentDraft)[] = [
  'name',
  'description',
  'provider',
  'auth',
  'model',
  'tools',
  'maxTurns',
  'permissionMode',
  'anthropicOauthPrefix',
  'effort',
  'contextWindow',
  'outputFormat',
  'outputSchema',
  'anthropicTaskBudget',
  'anthropicContextManagement',
  'anthropicSpeed',
  'dispatchableAgents',
  'concerns',
  'maxTokens',
  'contextWindowTokens',
  'compactAtPercent',
];

function isPresent(v: unknown): boolean {
  if (v === undefined || v === null) return false;
  if (typeof v === 'string') return v.length > 0;
  if (Array.isArray(v)) return v.length > 0;
  return true;
}

function schemaPropsToJsonSchema(props: SchemaProp[]): Record<string, unknown> {
  const properties: Record<string, unknown> = {};
  const required: string[] = [];
  for (const p of props) {
    required.push(p.name);
    if (p.type === 'enum') {
      properties[p.name] = { type: 'string', enum: p.enumValues ?? [] };
    } else {
      properties[p.name] = { type: p.type };
    }
  }
  return {
    type: 'object',
    additionalProperties: false,
    required,
    properties,
  };
}

function overridesToYamlShape(overrides: ConcernOverride[]): unknown[] {
  return overrides.map((o) => {
    const out: Record<string, unknown> = { id: o.id };
    if (o.severity) out.severity = o.severity;
    if (o.applicableGlobs && o.applicableGlobs.length > 0) out.applicable_globs = o.applicableGlobs;
    return out;
  });
}

function concernsToYamlShape(entries: ConcernEntry[]): unknown[] {
  return entries.map((e) => {
    if (e.kind === 'include') {
      const out: Record<string, unknown> = { include: e.template };
      if (e.mode && e.mode !== 'auto') out.mode = e.mode;
      if (e.overrides && e.overrides.length > 0) out.overrides = overridesToYamlShape(e.overrides);
      return out;
    }
    const out: Record<string, unknown> = { id: e.id, name: e.name, description: e.description };
    if (e.severity) out.severity = e.severity;
    if (e.applicableGlobs && e.applicableGlobs.length > 0) out.applicable_globs = e.applicableGlobs;
    return out;
  });
}

export function serializeAgent(d: AgentDraft): string {
  const obj: Record<string, unknown> = {};
  for (const key of KEY_ORDER) {
    const value = d[key];
    if (!isPresent(value)) continue;
    if (key === 'outputSchema') {
      obj.outputSchema = schemaPropsToJsonSchema(value as SchemaProp[]);
    } else if (key === 'concerns') {
      obj.concerns = concernsToYamlShape(value as ConcernEntry[]);
    } else {
      obj[key] = value;
    }
  }
  // Re-emit unknown keys preserved from the original file. Merged after the
  // modeled keys so a passthrough key never shadows a modeled one — modeled
  // fields always win once the UI starts editing them.
  if (d._passthrough) {
    for (const [k, v] of Object.entries(d._passthrough)) {
      if (!(k in obj)) obj[k] = v;
    }
  }
  const frontmatter = yaml.dump(obj).trimEnd();
  // Normalize trailing newlines before adding exactly one, so repeated
  // parse/serialize round-trips are idempotent (`parseAgent` only strips
  // leading newlines, never trailing — without this, each edit→save cycle
  // would accumulate an extra trailing blank line: `x\n` -> `x\n\n` -> ...).
  return `---\n${frontmatter}\n---\n\n${d.body.replace(/\n+$/, '')}\n`;
}

// ── Parse ───────────────────────────────────────────────────────────────

const MODELED_KEYS = new Set<string>(KEY_ORDER as string[]);

function jsonSchemaToSchemaProps(schema: unknown): SchemaProp[] | undefined {
  if (!schema || typeof schema !== 'object') return undefined;
  const s = schema as Record<string, unknown>;
  const properties = (s.properties ?? {}) as Record<string, Record<string, unknown>>;
  const required = Array.isArray(s.required) ? (s.required as string[]) : Object.keys(properties);
  const props: SchemaProp[] = [];
  for (const name of required) {
    const p = properties[name];
    if (!p) continue;
    if (Array.isArray(p.enum)) {
      props.push({ name, type: 'enum', enumValues: p.enum as string[] });
    } else {
      props.push({ name, type: (p.type as SchemaProp['type']) ?? 'string' });
    }
  }
  return props;
}

function yamlShapeToOverrides(raw: unknown): ConcernOverride[] | undefined {
  if (!Array.isArray(raw) || raw.length === 0) return undefined;
  return raw.map((o): ConcernOverride => {
    const ov = o as Record<string, unknown>;
    const out: ConcernOverride = { id: String(ov.id ?? '') };
    if (typeof ov.severity === 'string') out.severity = ov.severity as Severity;
    if (Array.isArray(ov.applicable_globs)) out.applicableGlobs = ov.applicable_globs as string[];
    return out;
  });
}

function yamlShapeToConcerns(entries: unknown): ConcernEntry[] | undefined {
  if (!Array.isArray(entries)) return undefined;
  return entries.map((raw): ConcernEntry => {
    const e = raw as Record<string, unknown>;
    if (typeof e.include === 'string') {
      const entry: IncludeConcern = { kind: 'include', template: e.include };
      if (typeof e.mode === 'string') entry.mode = e.mode as IncludeConcern['mode'];
      const overrides = yamlShapeToOverrides(e.overrides);
      if (overrides) entry.overrides = overrides;
      return entry;
    }
    const entry: InlineConcern = {
      kind: 'inline',
      id: String(e.id ?? ''),
      name: String(e.name ?? ''),
      description: String(e.description ?? ''),
    };
    if (typeof e.severity === 'string') entry.severity = e.severity as Severity;
    if (Array.isArray(e.applicable_globs)) entry.applicableGlobs = e.applicable_globs as string[];
    return entry;
  });
}

export function parseAgent(raw: string): AgentDraft {
  const { frontmatter, body } = splitFrontmatter(raw);
  const parsed = (frontmatter ? yaml.load(frontmatter) : {}) as Record<string, unknown> | null;
  const fm = parsed ?? {};

  const draft: AgentDraft = {
    name: typeof fm.name === 'string' ? fm.name : '',
    // `serializeAgent` always separates the closing `---` fence from the body
    // with a blank line; `splitFrontmatter`'s regex only consumes one of
    // those newlines, leaving a leading blank line in the captured body.
    // Mirrors `crates/rupu-agent/src/spec.rs:196-199`: trim ONLY leading
    // newlines/`---`, never trailing — trailing content is preserved
    // verbatim.
    body: body.replace(/^\n+/, ''),
  };

  if (typeof fm.description === 'string') draft.description = fm.description;
  if (typeof fm.provider === 'string') draft.provider = fm.provider;
  if (typeof fm.auth === 'string') draft.auth = fm.auth;
  if (typeof fm.model === 'string') draft.model = fm.model;
  if (Array.isArray(fm.tools)) draft.tools = fm.tools as string[];
  if (typeof fm.maxTurns === 'number') draft.maxTurns = fm.maxTurns;
  if (typeof fm.permissionMode === 'string') draft.permissionMode = fm.permissionMode;
  if (typeof fm.anthropicOauthPrefix === 'boolean') draft.anthropicOauthPrefix = fm.anthropicOauthPrefix;
  if (typeof fm.effort === 'string') draft.effort = fm.effort;
  if (typeof fm.contextWindow === 'string') draft.contextWindow = fm.contextWindow;
  if (typeof fm.outputFormat === 'string') draft.outputFormat = fm.outputFormat;
  if (fm.outputSchema !== undefined) {
    const props = jsonSchemaToSchemaProps(fm.outputSchema);
    if (props) draft.outputSchema = props;
  }
  if (typeof fm.anthropicTaskBudget === 'number') draft.anthropicTaskBudget = fm.anthropicTaskBudget;
  if (typeof fm.anthropicContextManagement === 'string')
    draft.anthropicContextManagement = fm.anthropicContextManagement;
  if (typeof fm.anthropicSpeed === 'string') draft.anthropicSpeed = fm.anthropicSpeed;
  if (Array.isArray(fm.dispatchableAgents)) draft.dispatchableAgents = fm.dispatchableAgents as string[];
  if (fm.concerns !== undefined) {
    const concerns = yamlShapeToConcerns(fm.concerns);
    if (concerns) draft.concerns = concerns;
  }
  if (typeof fm.maxTokens === 'number') draft.maxTokens = fm.maxTokens;
  if (typeof fm.contextWindowTokens === 'number') draft.contextWindowTokens = fm.contextWindowTokens;
  if (typeof fm.compactAtPercent === 'number') draft.compactAtPercent = fm.compactAtPercent;

  const passthrough: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(fm)) {
    if (!MODELED_KEYS.has(k)) passthrough[k] = v;
  }
  if (Object.keys(passthrough).length > 0) draft._passthrough = passthrough;

  return draft;
}
