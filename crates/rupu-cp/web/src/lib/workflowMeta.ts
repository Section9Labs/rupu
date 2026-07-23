// workflowMeta — typed read/write of the `trigger`, `inputs`, and `autoflow`
// top-level workflow blocks over a `WorkflowMeta.rest` passthrough bag (see
// `workflowGraph.ts`). Pure, framework-free: no React, no DOM.
//
// `rest` carries every top-level workflow key that isn't `name` /
// `description` / `steps`, verbatim, in the order it arrived from YAML. The
// functions here read one block out of `rest` into a small typed model the
// authoring UI can bind form controls to, and write a model back into a NEW
// `rest` object — never mutating the input — preserving every sibling key
// (including unmodeled ones like `contracts`/`defaults`/`concerns`) and,
// where the touched key already existed, its position in key order.
//
// Shapes here mirror the Rust structs in
// `crates/rupu-orchestrator/src/workflow.rs` (`Trigger`, `InputDef`,
// `Autoflow` + friends), all `#[serde(deny_unknown_fields)]` with snake_case
// enums. We never emit a key or enum value outside those schemas. Per the
// "omit empty" convention used across the editor (see `graphToWorkflowObject`
// in `workflowGraph.ts`), a block that's back at its default/empty shape is
// removed from `rest` entirely rather than serialized as `{}` — e.g. a
// manual trigger has no `trigger:` key at all, a disabled-and-unconfigured
// autoflow has no `autoflow:` key.

// ── Narrowing helpers ───────────────────────────────────────────────────────
// Small typed guards over `unknown`, mirroring the style in workflowGraph.ts
// (duplicated locally since that module doesn't export them).

function asString(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined;
}

function asNumber(v: unknown): number | undefined {
  return typeof v === 'number' && !Number.isNaN(v) ? v : undefined;
}

function asBool(v: unknown): boolean | undefined {
  return typeof v === 'boolean' ? v : undefined;
}

function asArray(v: unknown): unknown[] | undefined {
  return Array.isArray(v) ? v : undefined;
}

function asStringArray(v: unknown): string[] | undefined {
  const a = asArray(v);
  return a ? a.filter((x): x is string => typeof x === 'string') : undefined;
}

function asRecord(v: unknown): Record<string, unknown> | undefined {
  return typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined;
}

/** Set (or delete, when `value` is `undefined`) a single top-level key on
 *  `rest`, returning a NEW object. When setting a key that already exists,
 *  its position in key order is preserved; a brand-new key is appended at
 *  the end. Never mutates `rest`. */
function setOrDeleteKey(
  rest: Record<string, unknown>,
  key: string,
  value: unknown | undefined,
): Record<string, unknown> {
  const hasKey = Object.prototype.hasOwnProperty.call(rest, key);
  if (value === undefined) {
    if (!hasKey) return { ...rest };
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(rest)) {
      if (k !== key) out[k] = rest[k];
    }
    return out;
  }
  const out: Record<string, unknown> = {};
  let inserted = false;
  for (const k of Object.keys(rest)) {
    if (k === key) {
      out[k] = value;
      inserted = true;
    } else {
      out[k] = rest[k];
    }
  }
  if (!inserted) out[key] = value;
  return out;
}

// ── Trigger ──────────────────────────────────────────────────────────────────
// Mirrors `Trigger` / `TriggerKind` in workflow.rs. `on: manual` is the
// implicit default and carries no other fields; `on: cron` requires `cron`
// and forbids `event`/`filter`; `on: event` requires `event`, allows
// `filter`, and forbids `cron`. Validation of those rules happens in Rust at
// parse time — this layer only shapes what gets emitted so a well-formed
// model round-trips cleanly; it doesn't reject a malformed model.

export type TriggerOn = 'manual' | 'cron' | 'event';

export interface TriggerModel {
  on: TriggerOn;
  cron?: string;
  event?: string;
  filter?: string;
}

const TRIGGER_ONS: ReadonlySet<string> = new Set(['manual', 'cron', 'event']);

/** Read the `trigger:` block, defaulting to `{ on: 'manual' }` when absent
 *  or malformed. */
export function readTrigger(rest: Record<string, unknown>): TriggerModel {
  const raw = asRecord(rest.trigger);
  if (!raw) return { on: 'manual' };

  const onRaw = asString(raw.on);
  const on: TriggerOn = onRaw && TRIGGER_ONS.has(onRaw) ? (onRaw as TriggerOn) : 'manual';
  const model: TriggerModel = { on };

  const cron = asString(raw.cron);
  if (cron !== undefined) model.cron = cron;
  const event = asString(raw.event);
  if (event !== undefined) model.event = event;
  const filter = asString(raw.filter);
  if (filter !== undefined) model.filter = filter;
  return model;
}

/** Write the `trigger:` block. Only fields valid for the chosen `on` mode
 *  are emitted (e.g. `event: 'event'` never carries `cron`); a manual
 *  trigger — the default — omits the `trigger` key entirely rather than
 *  emitting `{ on: 'manual' }`. */
export function writeTrigger(rest: Record<string, unknown>, model: TriggerModel): Record<string, unknown> {
  if (model.on === 'manual') {
    return setOrDeleteKey(rest, 'trigger', undefined);
  }

  const raw: Record<string, unknown> = { on: model.on };
  if (model.on === 'cron') {
    if (model.cron !== undefined) raw.cron = model.cron;
  } else if (model.on === 'event') {
    if (model.event !== undefined) raw.event = model.event;
    if (model.filter !== undefined) raw.filter = model.filter;
  }
  return setOrDeleteKey(rest, 'trigger', raw);
}

// ── Inputs ───────────────────────────────────────────────────────────────────
// Mirrors `InputDef` in workflow.rs. The YAML/Rust shape is a
// `BTreeMap<String, InputDef>` (a name-keyed object), not a list — we model
// it as an array in TS for form-friendly ordering/editing and convert on
// read/write. Rust's field names are `type` (not `ty`) and `enum` (not
// `allowed`) on the wire (via `#[serde(rename = ...)]`); we must match those,
// never the Rust-internal identifiers.

export type InputType = 'string' | 'int' | 'bool';

export interface InputModel {
  name: string;
  type: InputType;
  required: boolean;
  default?: unknown;
  enumValues: string[];
  description?: string;
}

const INPUT_TYPES: ReadonlySet<string> = new Set(['string', 'int', 'bool']);

function asInputType(v: unknown): InputType | undefined {
  const s = asString(v);
  return s && INPUT_TYPES.has(s) ? (s as InputType) : undefined;
}

/** Read the `inputs:` map into an ordered array (object key order). Absent
 *  or malformed `inputs` reads as `[]`. */
export function readInputs(rest: Record<string, unknown>): InputModel[] {
  const raw = asRecord(rest.inputs);
  if (!raw) return [];

  const out: InputModel[] = [];
  for (const [name, v] of Object.entries(raw)) {
    const o = asRecord(v) ?? {};
    const model: InputModel = {
      name,
      type: asInputType(o.type) ?? 'string',
      required: asBool(o.required) ?? false,
      enumValues: asStringArray(o.enum) ?? [],
    };
    if (o.default !== undefined) model.default = o.default;
    const description = asString(o.description);
    if (description !== undefined) model.description = description;
    out.push(model);
  }
  return out;
}

/** Write the `inputs:` block as a name-keyed object (matching the Rust
 *  `BTreeMap<String, InputDef>` shape), in `inputs` array order. Emits
 *  `type:`/`enum:` (never `ty:`/`allowed:`); an empty `enum` and an
 *  undefined `default` are omitted rather than emitted as `[]` / `null`. An
 *  empty `inputs` array omits the `inputs` key entirely. */
export function writeInputs(rest: Record<string, unknown>, inputs: InputModel[]): Record<string, unknown> {
  if (inputs.length === 0) {
    return setOrDeleteKey(rest, 'inputs', undefined);
  }

  const map: Record<string, unknown> = {};
  for (const input of inputs) {
    const o: Record<string, unknown> = {
      type: input.type,
      required: input.required,
    };
    if (input.default !== undefined) o.default = input.default;
    if (input.enumValues.length > 0) o.enum = input.enumValues;
    if (input.description !== undefined) o.description = input.description;
    map[input.name] = o;
  }
  return setOrDeleteKey(rest, 'inputs', map);
}

// ── Autoflow ─────────────────────────────────────────────────────────────────
// Mirrors `Autoflow` / `AutoflowSelector` / `AutoflowClaim` /
// `AutoflowWorkspace` / `AutoflowOutcomeRef` in workflow.rs.

export type AutoflowEntity = 'issue' | 'pull_request';
export type AutoflowIssueState = 'open' | 'closed';
export type DraftFilter = 'include' | 'exclude' | 'only';
export type AuthorScope = 'collaborators' | 'org_members';
export type SkipAction = 'skip' | 'label_needs_human';
export type AutoflowClaimKey = 'issue' | 'pr_head_sha';
export type AutoflowWorkspaceStrategy = 'worktree' | 'in_place';

export interface AutoflowSelectorModel {
  states?: AutoflowIssueState[];
  labels_all?: string[];
  labels_any?: string[];
  labels_none?: string[];
  limit?: number;
  draft?: DraftFilter;
  base?: string;
  authors?: string[];
  authors_from?: AuthorScope;
  on_skip?: SkipAction;
}

export interface AutoflowClaimModel {
  key: AutoflowClaimKey;
  ttl?: string;
}

export interface AutoflowWorkspaceModel {
  strategy: AutoflowWorkspaceStrategy;
  branch?: string;
}

export interface AutoflowOutcomeModel {
  output: string;
}

export interface AutoflowModel {
  enabled: boolean;
  entity: AutoflowEntity;
  source?: string;
  priority?: number;
  selector: AutoflowSelectorModel;
  wake_on: string[];
  reconcile_every?: string;
  claim?: AutoflowClaimModel;
  workspace?: AutoflowWorkspaceModel;
  outcome?: AutoflowOutcomeModel;
}

const AUTOFLOW_ENTITIES: ReadonlySet<string> = new Set(['issue', 'pull_request']);
const ISSUE_STATES: ReadonlySet<string> = new Set(['open', 'closed']);
const DRAFT_FILTERS: ReadonlySet<string> = new Set(['include', 'exclude', 'only']);
const AUTHOR_SCOPES: ReadonlySet<string> = new Set(['collaborators', 'org_members']);
const SKIP_ACTIONS: ReadonlySet<string> = new Set(['skip', 'label_needs_human']);
const CLAIM_KEYS: ReadonlySet<string> = new Set(['issue', 'pr_head_sha']);
const WORKSPACE_STRATEGIES: ReadonlySet<string> = new Set(['worktree', 'in_place']);

function asEnumValue<T extends string>(v: unknown, allowed: ReadonlySet<string>): T | undefined {
  const s = asString(v);
  return s && allowed.has(s) ? (s as T) : undefined;
}

function readSelector(raw: Record<string, unknown> | undefined): AutoflowSelectorModel {
  const o = raw ?? {};
  const model: AutoflowSelectorModel = {};

  const statesRaw = asStringArray(o.states);
  if (statesRaw) {
    const states = statesRaw.filter((s): s is AutoflowIssueState => ISSUE_STATES.has(s));
    if (states.length > 0) model.states = states;
  }
  const labelsAll = asStringArray(o.labels_all);
  if (labelsAll && labelsAll.length > 0) model.labels_all = labelsAll;
  const labelsAny = asStringArray(o.labels_any);
  if (labelsAny && labelsAny.length > 0) model.labels_any = labelsAny;
  const labelsNone = asStringArray(o.labels_none);
  if (labelsNone && labelsNone.length > 0) model.labels_none = labelsNone;
  const limit = asNumber(o.limit);
  if (limit !== undefined) model.limit = limit;
  const draft = asEnumValue<DraftFilter>(o.draft, DRAFT_FILTERS);
  if (draft !== undefined) model.draft = draft;
  const base = asString(o.base);
  if (base !== undefined) model.base = base;
  const authors = asStringArray(o.authors);
  if (authors && authors.length > 0) model.authors = authors;
  const authorsFrom = asEnumValue<AuthorScope>(o.authors_from, AUTHOR_SCOPES);
  if (authorsFrom !== undefined) model.authors_from = authorsFrom;
  const onSkip = asEnumValue<SkipAction>(o.on_skip, SKIP_ACTIONS);
  if (onSkip !== undefined) model.on_skip = onSkip;

  return model;
}

function selectorToRaw(s: AutoflowSelectorModel): Record<string, unknown> {
  const o: Record<string, unknown> = {};
  if (s.states && s.states.length > 0) o.states = s.states;
  if (s.labels_all && s.labels_all.length > 0) o.labels_all = s.labels_all;
  if (s.labels_any && s.labels_any.length > 0) o.labels_any = s.labels_any;
  if (s.labels_none && s.labels_none.length > 0) o.labels_none = s.labels_none;
  if (s.limit !== undefined) o.limit = s.limit;
  if (s.draft !== undefined) o.draft = s.draft;
  if (s.base !== undefined) o.base = s.base;
  if (s.authors && s.authors.length > 0) o.authors = s.authors;
  if (s.authors_from !== undefined) o.authors_from = s.authors_from;
  if (s.on_skip !== undefined) o.on_skip = s.on_skip;
  return o;
}

function isSelectorEmpty(s: AutoflowSelectorModel): boolean {
  return Object.keys(selectorToRaw(s)).length === 0;
}

/** Read the `autoflow:` block. Returns `null` when absent (there is no
 *  meaningful "default autoflow" — the block's very presence is what opts a
 *  workflow into autonomous execution). */
export function readAutoflow(rest: Record<string, unknown>): AutoflowModel | null {
  const raw = asRecord(rest.autoflow);
  if (!raw) return null;

  const model: AutoflowModel = {
    enabled: asBool(raw.enabled) ?? false,
    entity: asEnumValue<AutoflowEntity>(raw.entity, AUTOFLOW_ENTITIES) ?? 'issue',
    selector: readSelector(asRecord(raw.selector)),
    wake_on: asStringArray(raw.wake_on) ?? [],
  };

  const source = asString(raw.source);
  if (source !== undefined) model.source = source;
  const priority = asNumber(raw.priority);
  if (priority !== undefined) model.priority = priority;
  const reconcileEvery = asString(raw.reconcile_every);
  if (reconcileEvery !== undefined) model.reconcile_every = reconcileEvery;

  const claimRaw = asRecord(raw.claim);
  if (claimRaw) {
    const claim: AutoflowClaimModel = { key: asEnumValue<AutoflowClaimKey>(claimRaw.key, CLAIM_KEYS) ?? 'issue' };
    const ttl = asString(claimRaw.ttl);
    if (ttl !== undefined) claim.ttl = ttl;
    model.claim = claim;
  }

  const workspaceRaw = asRecord(raw.workspace);
  if (workspaceRaw) {
    const workspace: AutoflowWorkspaceModel = {
      strategy: asEnumValue<AutoflowWorkspaceStrategy>(workspaceRaw.strategy, WORKSPACE_STRATEGIES) ?? 'worktree',
    };
    const branch = asString(workspaceRaw.branch);
    if (branch !== undefined) workspace.branch = branch;
    model.workspace = workspace;
  }

  const outcomeRaw = asRecord(raw.outcome);
  if (outcomeRaw) {
    const output = asString(outcomeRaw.output);
    if (output !== undefined) model.outcome = { output };
  }

  return model;
}

/** A model that's fully at its default/empty shape: disabled, `entity:
 *  issue`, no source/priority/selector filters/wake_on/reconcile/claim/
 *  workspace/outcome. Equivalent to the `autoflow` key being absent. */
function isAutoflowEmpty(model: AutoflowModel): boolean {
  return (
    !model.enabled &&
    model.entity === 'issue' &&
    model.source === undefined &&
    (model.priority === undefined || model.priority === 0) &&
    isSelectorEmpty(model.selector) &&
    model.wake_on.length === 0 &&
    model.reconcile_every === undefined &&
    model.claim === undefined &&
    model.workspace === undefined &&
    model.outcome === undefined
  );
}

/** Write the `autoflow:` block. `null`, or a model that's disabled AND at
 *  its default/empty shape (see `isAutoflowEmpty`), omits the `autoflow` key
 *  entirely rather than emitting `{}` / `{ enabled: false }`. A disabled
 *  model that still carries configuration (selector filters, claim,
 *  workspace, etc.) is preserved with `enabled: false` so re-enabling later
 *  doesn't lose it. */
export function writeAutoflow(rest: Record<string, unknown>, model: AutoflowModel | null): Record<string, unknown> {
  if (model === null || isAutoflowEmpty(model)) {
    return setOrDeleteKey(rest, 'autoflow', undefined);
  }

  const raw: Record<string, unknown> = { enabled: model.enabled, entity: model.entity };
  if (model.source !== undefined) raw.source = model.source;
  if (model.priority !== undefined) raw.priority = model.priority;
  const selectorRaw = selectorToRaw(model.selector);
  if (Object.keys(selectorRaw).length > 0) raw.selector = selectorRaw;
  if (model.wake_on.length > 0) raw.wake_on = model.wake_on;
  if (model.reconcile_every !== undefined) raw.reconcile_every = model.reconcile_every;
  if (model.claim !== undefined) {
    const claim: Record<string, unknown> = { key: model.claim.key };
    if (model.claim.ttl !== undefined) claim.ttl = model.claim.ttl;
    raw.claim = claim;
  }
  if (model.workspace !== undefined) {
    const workspace: Record<string, unknown> = { strategy: model.workspace.strategy };
    if (model.workspace.branch !== undefined) workspace.branch = model.workspace.branch;
    raw.workspace = workspace;
  }
  if (model.outcome !== undefined) raw.outcome = { output: model.outcome.output };

  return setOrDeleteKey(rest, 'autoflow', raw);
}

// ── Contracts (read-only lookup) ──────────────────────────────────────────────

/** The declared output names from `contracts.outputs` (for populating the
 *  autoflow outcome `<select>`). Never modifies `contracts`. Returns `[]`
 *  when `contracts.outputs` is absent or malformed. */
export function contractOutputKeys(rest: Record<string, unknown>): string[] {
  const contracts = asRecord(rest.contracts);
  if (!contracts) return [];
  const outputs = asRecord(contracts.outputs);
  if (!outputs) return [];
  return Object.keys(outputs);
}
