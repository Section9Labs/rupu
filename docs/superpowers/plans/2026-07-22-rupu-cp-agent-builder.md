# CP Agent Builder (card-based agent authoring) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

## Context

Today, authoring a rupu agent in the control-plane is **raw-text only**: both the "New agent" modal (`Agents.tsx`) and the `AgentDetail` edit view drop you into a CodeMirror box on the full `.md` (YAML frontmatter + markdown body). Every one of the ~21 frontmatter keys is hand-typed, with no pickers, no field discovery, no inline validation, and a camelCase/snake_case trap (`maxTurns`/`permissionMode`/`outputSchema` vs `provider`/`model`); parse errors only surface server-side after Create/Save (`deny_unknown_fields` rejects the whole file). This is pass 1 of a three-part CP redesign (Agent Builder → Flow Designer → Run Room), chosen first because it is **fully self-contained — zero orchestrator dependency** — and it establishes the shared card/design-system components the later screens reuse. Approved interactive mockup: https://claude.ai/code/artifact/dc78f8a7-f1d9-47ca-830c-3b267a674fcc

**Goal:** A card-based, structured Agent Builder (a card per frontmatter field-group, with pickers, a nested output-schema/concerns builder, live `.md` preview, AI-assist, and a raw fallback) that renders in place of the raw editor for create + edit, **behind a config feature flag**, defaulting to the current UI.

**Architecture:** A pure, unit-tested model layer (`lib/agentBuilder/` — `AgentDraft` type, `serializeAgent`/`parseAgent` round-trip via `js-yaml`, `validateAgentDraft`) with a thin React card UI (`components/agentBuilder/`) that reuses the existing `situationRoom` `.sr-*` design tokens, `ui/Button`, and `CodeHighlight`. Gating is a new `[cp].agent_authoring_ui` config field (surfaced by the existing `/api/config` with **no API change**) plus a per-browser `localStorage` dev override, read through one `useAgentAuthoringUi()` hook. The builder emits **only** the exact frontmatter keys, so it can never trip `deny_unknown_fields`.

**Tech Stack:** Rust (rupu-config), React 19 + TypeScript + Tailwind + Vitest (crates/rupu-cp/web), js-yaml.

## Global Constraints

- **Emit only these exact frontmatter keys** (source of truth `crates/rupu-agent/src/spec.rs:29-137`, `#[serde(deny_unknown_fields)]`): `name` (required), `description`, `provider`, `auth`, `model`, `tools` (list), `maxTurns`, `permissionMode`, `anthropicOauthPrefix`, `effort`, `contextWindow`, `outputFormat`, `outputSchema`, `anthropicTaskBudget`, `anthropicContextManagement`, `anthropicSpeed`, `dispatchableAgents` (list), `concerns`, `maxTokens`, `contextWindowTokens`, `compactAtPercent`. Never emit any other key.
- **Enum vocabularies (exact strings):** `auth` = `api-key` | `sso`; `permissionMode` = `ask` | `bypass` | `readonly`; `effort` = `auto` | `minimal` | `low` | `medium` | `high` | `max`; `contextWindow` = `default` | `1m`; `outputFormat` = `text` | `json`; `anthropicContextManagement` = `tool_clearing` | `none`; `anthropicSpeed` = `fast` | `standard`. `provider`/`model` are free-form strings (provider suggestions: anthropic, openai, gemini, copilot). Known built-in tool names: `bash, read_file, write_file, edit_file, ast_grep, grep, glob, dispatch_agent, dispatch_agents_parallel` (+ dotted MCP names e.g. `scm.prs.get`, allowed as free text).
- **Body is the system prompt verbatim** — no templating. The `.md` = `---\n<frontmatter>\n---\n\n<body>`.
- **Workspace deps only** — no versions in crate `Cargo.toml` (root `Cargo.toml` pins them). Rust: `#![deny(clippy::all)]` workspace-wide; `thiserror` for libs.
- **Reuse, don't reinvent:** `js-yaml` (`yaml.dump`/`yaml.load`), `splitFrontmatter` + `CodeHighlight` (`src/components/CodeHighlight.tsx`), `ui/Button` (`variant`/`size`), the `.sr-*` tokens in `src/styles.css:278-393`. Match the published mockup for card layout/interactions.
- **Feature flag default = classic.** New UI only shows when `cp.agent_authoring_ui === "next"` OR `localStorage['rupu.cp.agentUi'] === "next"`. Fall back to classic on any error/unset.
- **Test command:** `npm test` (vitest) run from `crates/rupu-cp/web`. Web tests use `// @vitest-environment jsdom`, `vi.spyOn(api, ...)` (not `vi.mock`), and mock heavy children (CodeEditor → `<textarea>`). Rust: `cargo test -p rupu-config`.
- **Do not run `cargo fmt` package-wide** (main is fmt-dirty under the pinned toolchain per repo convention); only `cargo fmt -- <file>` per touched file if needed.

## File Structure

**Rust (flag only):**
- Modify `crates/rupu-config/src/policy_config.rs` — add `agent_authoring_ui` field to `CpConfig`.

**Web — pure logic (`crates/rupu-cp/web/src/lib/agentBuilder/`):**
- `agentSpec.ts` — `AgentDraft` type, vocab consts, `emptyDraft()`, `serializeAgent(draft) → string`, `parseAgent(raw) → AgentDraft`.
- `validate.ts` — `validateAgentDraft(draft) → { ok: boolean; errors: FieldError[]; warnings: FieldError[] }`.
- `fields.ts` — the card/field-group registry metadata (id, label, yamlKeys, group, required).
- `*.test.ts` siblings.

**Web — hook:**
- `src/hooks/useAgentAuthoringUi.ts` — resolves `'classic' | 'next'`.

**Web — components (`crates/rupu-cp/web/src/components/agentBuilder/`):**
- `AgentBuilder.tsx` — shell (palette + canvas + live preview + mode toggle). Reusable by create + edit.
- `cards/` — one file per field-group card (Identity, Model, Tools, Permission, Reasoning, Context, Output, Dispatch, Anthropic, Concerns, Prompt).
- `controls.tsx` — shared control primitives (Segmented, ChipsInput, Scale, LabeledRow).
- `AgentBuilder.test.tsx`.

**Web — CSS:**
- Append an `.ab-*` block to `src/styles.css` (namespaced, mirroring the `.sr-*` block).

**Web — wiring:**
- Modify `src/pages/Agents.tsx` (`NewAgentModal`) — gate create UI on the flag.
- Modify `src/pages/AgentDetail.tsx` — gate edit UI on the flag.

---

## Task 1: Feature-flag config field (`CpConfig.agent_authoring_ui`)

**Files:**
- Modify: `crates/rupu-config/src/policy_config.rs` (struct `CpConfig`, ~line 19)
- Test: same file's `#[cfg(test)]` module (or `crates/rupu-config/tests/` if that's the crate's pattern — check first).

**Interfaces:**
- Produces: `CpConfig.agent_authoring_ui: String` (default `"classic"`), serialized into the `/api/config` response `cp` object automatically (`crates/rupu-cp/src/api/config.rs:90` already does `serde_json::to_value(&resolved.config.cp)`).

- [ ] **Step 1: Read the current struct + its default helpers.** Read `crates/rupu-config/src/policy_config.rs` around `CpConfig` (line 19) to copy the exact `#[serde(...)]` / `default_true`-style pattern used by neighbors (`autoflow_reconcile_enabled` etc.).

- [ ] **Step 2: Write the failing test.** Add to the crate's test module:
```rust
#[test]
fn cp_config_defaults_agent_authoring_ui_to_classic() {
    let cfg: CpConfig = toml::from_str("").expect("empty [cp] parses");
    assert_eq!(cfg.agent_authoring_ui, "classic");
}

#[test]
fn cp_config_accepts_next_agent_authoring_ui() {
    let cfg: CpConfig = toml::from_str("agent_authoring_ui = \"next\"").unwrap();
    assert_eq!(cfg.agent_authoring_ui, "next");
}
```

- [ ] **Step 3: Run it and confirm it fails to compile** (`agent_authoring_ui` missing): `cargo test -p rupu-config cp_config_defaults_agent_authoring_ui_to_classic`.

- [ ] **Step 4: Add the field.** In `CpConfig`, add (matching the file's default-fn convention):
```rust
/// Which agent-authoring UI the CP web app renders: "classic" (raw editor)
/// or "next" (the card-based Agent Builder). Defaults to classic so the new
/// UI is opt-in behind this flag.
#[serde(default = "default_agent_authoring_ui")]
pub agent_authoring_ui: String,
```
and the helper near the other defaults:
```rust
fn default_agent_authoring_ui() -> String { "classic".to_string() }
```
Also add `agent_authoring_ui: default_agent_authoring_ui()` to any manual `Default`/constructor for `CpConfig` if one exists.

- [ ] **Step 5: Run tests to verify pass.** `cargo test -p rupu-config` → PASS. (No `/api/config` change needed — verify by reading `config.rs:90` that `cp` is serialized whole.)

- [ ] **Step 6: Commit.**
```bash
git add crates/rupu-config/src/policy_config.rs
git commit -m "feat(config): add [cp].agent_authoring_ui flag (default classic)"
```

---

## Task 2: `useAgentAuthoringUi()` hook

**Files:**
- Create: `crates/rupu-cp/web/src/hooks/useAgentAuthoringUi.ts`
- Test: `crates/rupu-cp/web/src/hooks/useAgentAuthoringUi.test.ts`

**Interfaces:**
- Consumes: `api.getConfig()` → `ConfigView` (`api.ts:2127`, `.cp: Record<string, unknown>`).
- Produces: `useAgentAuthoringUi(): 'classic' | 'next'` — a React hook. Also `export function resolveAgentUi(cp: Record<string, unknown> | null, override: string | null): 'classic' | 'next'` (pure, unit-tested).

- [ ] **Step 1: Write the failing test** (pure resolver):
```ts
import { describe, it, expect } from 'vitest';
import { resolveAgentUi } from './useAgentAuthoringUi';
describe('resolveAgentUi', () => {
  it('localStorage override wins', () => {
    expect(resolveAgentUi({ agent_authoring_ui: 'classic' }, 'next')).toBe('next');
    expect(resolveAgentUi({ agent_authoring_ui: 'next' }, 'classic')).toBe('classic');
  });
  it('falls back to server config when no override', () => {
    expect(resolveAgentUi({ agent_authoring_ui: 'next' }, null)).toBe('next');
  });
  it('defaults to classic when unset or unknown', () => {
    expect(resolveAgentUi(null, null)).toBe('classic');
    expect(resolveAgentUi({ agent_authoring_ui: 'bogus' }, null)).toBe('classic');
  });
});
```

- [ ] **Step 2: Run → fail** (`npx vitest run src/hooks/useAgentAuthoringUi.test.ts`): module not found.

- [ ] **Step 3: Implement.**
```ts
import { useEffect, useState } from 'react';
import { api } from '../lib/api';

export type AgentUi = 'classic' | 'next';
const STORAGE_KEY = 'rupu.cp.agentUi';

export function resolveAgentUi(cp: Record<string, unknown> | null, override: string | null): AgentUi {
  const pick = (v: unknown): AgentUi | null => (v === 'next' || v === 'classic' ? v : null);
  return pick(override) ?? pick(cp?.agent_authoring_ui) ?? 'classic';
}

// Module-level cached fetch so the config is loaded at most once per session.
let cpPromise: Promise<Record<string, unknown> | null> | null = null;
function loadCp(): Promise<Record<string, unknown> | null> {
  if (!cpPromise) cpPromise = api.getConfig().then((v) => v.cp ?? null).catch(() => null);
  return cpPromise;
}
function readOverride(): string | null {
  try { return window.localStorage.getItem(STORAGE_KEY); } catch { return null; }
}

export function useAgentAuthoringUi(): AgentUi {
  // Seed synchronously from the localStorage override so a dogfooder sees
  // 'next' on first paint with no flash; otherwise start 'classic' and
  // upgrade once server config resolves.
  const [ui, setUi] = useState<AgentUi>(() => resolveAgentUi(null, readOverride()));
  useEffect(() => {
    let live = true;
    loadCp().then((cp) => { if (live) setUi(resolveAgentUi(cp, readOverride())); });
    return () => { live = false; };
  }, []);
  return ui;
}
```

- [ ] **Step 4: Run → pass.** `npx vitest run src/hooks/useAgentAuthoringUi.test.ts`.

- [ ] **Step 5: Commit.** `git add src/hooks/useAgentAuthoringUi.ts src/hooks/useAgentAuthoringUi.test.ts && git commit -m "feat(cp-web): useAgentAuthoringUi flag hook"`

---

## Task 3: `AgentDraft` model + `serializeAgent`/`parseAgent` (round-trip)

**Files:**
- Create: `crates/rupu-cp/web/src/lib/agentBuilder/agentSpec.ts`
- Test: `crates/rupu-cp/web/src/lib/agentBuilder/agentSpec.test.ts`

**Interfaces:**
- Produces:
  - `interface AgentDraft` — one optional field per frontmatter key, plus `body: string`. `outputSchema` modeled as `SchemaProp[]` (`{ name; type: 'string'|'number'|'boolean'|'enum'|'array'|'object'; enumValues?: string[] }`); `concerns` modeled as `ConcernEntry[]` (`{ kind: 'include'|'inline'; ref: string; overrides?: string[]; globs?: string }`).
  - `emptyDraft(): AgentDraft`
  - `serializeAgent(d: AgentDraft): string` — builds the `.md`; emits only present/non-empty keys, in a stable order; uses `yaml.dump` for the frontmatter mapping then wraps with `---` fences + body.
  - `parseAgent(raw: string): AgentDraft` — `splitFrontmatter(raw)` → `yaml.load` the frontmatter → map into `AgentDraft` (inverse of serialize), body = the markdown body. Unknown keys are preserved into a `_passthrough` mapping so editing never drops fields the UI doesn't model yet, and `serializeAgent` re-emits them.
- Consumes: `import yaml from 'js-yaml'` and `import { splitFrontmatter } from '../../components/CodeHighlight'`.

- [ ] **Step 1: Write failing round-trip + shape tests.**
```ts
import { describe, it, expect } from 'vitest';
import { serializeAgent, parseAgent, emptyDraft } from './agentSpec';

it('serializes only present keys, name first, body after fences', () => {
  const d = emptyDraft();
  d.name = 'security-reviewer';
  d.provider = 'anthropic'; d.model = 'claude-sonnet-4-6';
  d.tools = ['read_file', 'grep', 'scm.prs.get'];
  d.permissionMode = 'readonly'; d.maxTurns = 10;
  d.outputFormat = 'json';
  d.outputSchema = [{ name: 'severity', type: 'enum', enumValues: ['low','high'] }, { name: 'title', type: 'string' }];
  d.body = 'You are a reviewer.';
  const md = serializeAgent(d);
  expect(md.startsWith('---\nname: security-reviewer')).toBe(true);
  expect(md).toContain('permissionMode: readonly');
  expect(md).toContain('tools:');            // list emitted
  expect(md).not.toContain('description:');   // empty key omitted
  expect(md.trim().endsWith('You are a reviewer.')).toBe(true);
});

it('round-trips parse(serialize(d)) preserving modeled fields', () => {
  const d = emptyDraft();
  d.name = 'x'; d.effort = 'high'; d.dispatchableAgents = ['code-reviewer'];
  d.concerns = [{ kind: 'include', ref: 'owasp', overrides: ['mode=full'] }];
  d.body = 'body text';
  const back = parseAgent(serializeAgent(d));
  expect(back.name).toBe('x');
  expect(back.effort).toBe('high');
  expect(back.dispatchableAgents).toEqual(['code-reviewer']);
  expect(back.concerns?.[0]).toMatchObject({ kind: 'include', ref: 'owasp' });
  expect(back.body).toBe('body text');
});

it('parse preserves unknown keys via passthrough and re-emits them', () => {
  const raw = '---\nname: y\nsomeFutureKey: 42\n---\n\nb';
  const back = parseAgent(raw);
  expect(back.name).toBe('y');
  expect(serializeAgent(back)).toContain('someFutureKey: 42');
});
```

- [ ] **Step 2: Run → fail.**

- [ ] **Step 3: Implement `agentSpec.ts`.** Define `AgentDraft`, `SchemaProp`, `ConcernEntry`, vocab consts (`PROVIDERS`, `AUTH_MODES`, `PERMISSION_MODES`, `EFFORT_LEVELS`, `CONTEXT_WINDOWS`, `OUTPUT_FORMATS`, `ANTHROPIC_SPEED`, `ANTHROPIC_CTX_MGMT`, `BUILTIN_TOOLS`), `emptyDraft()`, and the two functions. `serializeAgent` builds an ordered plain object of only-present keys (converting `outputSchema` props → the JSON-schema mapping `{ type: object, additionalProperties: false, required: [...], properties: {...} }`; converting `concerns` entries → the include/inline YAML shape), merges `_passthrough`, then `` `---\n${yaml.dump(obj).trimEnd()}\n---\n\n${d.body}\n` ``. `parseAgent` inverts it. Keep key order stable (name, description, provider, auth, model, tools, maxTurns, permissionMode, effort, contextWindow, contextWindowTokens, compactAtPercent, outputFormat, outputSchema, dispatchableAgents, anthropic*, concerns).

- [ ] **Step 4: Run → pass.**

- [ ] **Step 5: Commit.** `git add src/lib/agentBuilder/agentSpec.ts src/lib/agentBuilder/agentSpec.test.ts && git commit -m "feat(cp-web): agent draft model + serialize/parse round-trip"`

---

## Task 4: `validateAgentDraft` + field registry

**Files:**
- Create: `crates/rupu-cp/web/src/lib/agentBuilder/validate.ts`, `.../fields.ts`
- Test: `crates/rupu-cp/web/src/lib/agentBuilder/validate.test.ts`

**Interfaces:**
- Produces: `interface FieldError { field: string; message: string }`; `validateAgentDraft(d: AgentDraft): { ok: boolean; errors: FieldError[]; warnings: FieldError[] }` (error: missing `name`, non-slug `name`, `compactAtPercent` out of [10,95], enum value not in vocab; warning: tool name not in `BUILTIN_TOOLS` and not dotted-MCP, empty `tools` list ⇒ "full registry granted"). `fields.ts`: `CARD_REGISTRY: CardMeta[]` (`{ id, label, yamlKeys, group: 'Core'|'Runtime'|'Advanced', required?: boolean }`) — drives the palette.

- [ ] **Step 1: Write failing tests** (name required; bad compact %; unknown tool warns; good draft ok=true). 
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** `validate.ts` (pure, no I/O) and `fields.ts` (the 11-card registry — Identity, Prompt [Core]; Model, Tools, Permission, Reasoning [Runtime]; Context, Output, Dispatch, Anthropic, Concerns [Advanced]).
- [ ] **Step 4: Run → pass.**
- [ ] **Step 5: Commit.** `-m "feat(cp-web): agent draft validation + card registry"`

---

## Task 5: Shared control primitives + `.ab-*` styles

**Files:**
- Create: `crates/rupu-cp/web/src/components/agentBuilder/controls.tsx`
- Modify: `crates/rupu-cp/web/src/styles.css` (append `.ab-*` block after the `.sr-*` block at :393)
- Test: `crates/rupu-cp/web/src/components/agentBuilder/controls.test.tsx`

**Interfaces:**
- Produces: `Segmented<T>({ options, value, onChange })`, `ChipsInput({ list, suggestions, placeholder, onChange })`, `Scale({ options, value, onChange })`, `LabeledRow({ label, yamlKey, hint, children })`. Reuse `.sr-*` tokens; new `.ab-card`, `.ab-card-head`, `.ab-palette`, `.ab-pcard` classes port the mockup's look.

- [ ] **Step 1: Write failing test** (ChipsInput renders chips, Enter adds, × removes; Segmented marks the pressed option `aria-pressed`).
- [ ] **Step 2: Run → fail. Step 3: Implement** controls + CSS (port class rules verbatim from the mockup `agent-builder.html`, renamed `.ab-*`; use `rgb(var(--c-*))` tokens). **Step 4: Run → pass. Step 5: Commit** `-m "feat(cp-web): agent builder control primitives + styles"`.

---

## Task 6: `AgentBuilder` shell + Identity/Model/Prompt cards + live YAML preview

**Files:**
- Create: `crates/rupu-cp/web/src/components/agentBuilder/AgentBuilder.tsx`, `.../cards/{Identity,Model,Prompt}.tsx`
- Test: `crates/rupu-cp/web/src/components/agentBuilder/AgentBuilder.test.tsx`

**Interfaces:**
- Produces:
```ts
interface AgentBuilderProps {
  initialRaw: string;            // parseAgent → draft state
  submitLabel: string;           // "Create agent" | "Save"
  submitting: boolean;
  error: string | null;
  onSubmit: (raw: string) => void;    // serializeAgent(draft)
  onCancel?: () => void;
  // AI-assist (create only); omit to hide the AI tab:
  aiModels?: ProviderModels[];
  onGenerate?: (body: GenerateBody) => Promise<GeneratedDef>;
}
```
- Consumes: `parseAgent`/`serializeAgent`/`validateAgentDraft`, `CARD_REGISTRY`, `CodeHighlight` (`code={serializeAgent(draft)} language="yaml"` for the frontmatter preview or `frontmatter` for the whole `.md`), `ui/Button`.

- [ ] **Step 1: Write failing test.** Render `<AgentBuilder initialRaw={NEW_AGENT_TEMPLATE-equivalent} submitLabel="Create agent" .../>`; assert the name input shows the parsed name; type a new name; assert the live preview (`data-testid="ab-yaml"`) contains it; click "Create agent"; assert `onSubmit` called with a raw string containing `name: <typed>`.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** the shell: `draft` state seeded from `parseAgent(initialRaw)`; three-column layout (palette | canvas of active cards | live preview); mode toggle Cards/Raw/AI (AI only when `onGenerate`); Raw mode = `CodeEditor` on `serializeAgent(draft)`↔`parseAgent`; a validity badge from `validateAgentDraft`; footer submit button (disabled when `!ok || submitting`). Implement Identity, Model, Prompt cards using the control primitives (Identity: name+description; Model: provider Segmented + model text + auth Segmented; Prompt: textarea → `draft.body`). Preview element carries `data-testid="ab-yaml"`.
- [ ] **Step 4: Run → pass.**
- [ ] **Step 5: Commit** `-m "feat(cp-web): AgentBuilder shell + identity/model/prompt cards + live preview"`.

---

## Task 7: Remaining cards (Tools, Permission, Reasoning, Context, Output+schema, Dispatch, Anthropic, Concerns)

**Files:**
- Create: `crates/rupu-cp/web/src/components/agentBuilder/cards/{Tools,Permission,Reasoning,Context,Output,Dispatch,Anthropic,Concerns}.tsx`
- Modify: `AgentBuilder.tsx` (register the cards; drag/click add-from-palette + remove)
- Test: extend `AgentBuilder.test.tsx`

Each card is a small controlled component `({ draft, patch }: { draft: AgentDraft; patch: (p: Partial<AgentDraft>) => void })`. Concrete control per card (all values flow through `patch` → re-serialize preview):

| Card | Controls (yaml key) |
|---|---|
| Tools | `ChipsInput` over `draft.tools` with `BUILTIN_TOOLS` suggestions; empty-list hint "full registry granted" (`tools`) |
| Permission | `Segmented` ask/bypass/readonly (`permissionMode`) with per-value hint |
| Reasoning | `Scale` off/auto/minimal/low/medium/high/max (`effort`); number inputs `maxTurns`, `maxTokens` (placeholder 8192) |
| Context | `Segmented` default/1m (`contextWindow`); numbers `contextWindowTokens`, `compactAtPercent` |
| Output | `Segmented` text/json (`outputFormat`); when json, a nested schema builder over `draft.outputSchema` (add/remove `SchemaProp`, type `<select>`, enum CSV input) |
| Dispatch | `ChipsInput` over `draft.dispatchableAgents` with agent-name suggestions from `api.getAgents()` (passed in as a prop `agentNames?: string[]`) |
| Anthropic | `Segmented` speed fast/standard; `Segmented` ctx-mgmt tool_clearing/none; number `anthropicTaskBudget`; toggle `anthropicOauthPrefix` |
| Concerns | list of `ConcernEntry`; "+ include template" / "+ inline concern" buttons; per-entry ref input + overrides `ChipsInput` (include) or globs input (inline) |

- [ ] **Step 1: Write failing tests** — one focused assertion per non-trivial card (e.g. add a tool chip → preview contains `tools:` incl the tool; switch Output to json + add a prop → preview contains `outputSchema:` and the prop; toggle permission → preview `permissionMode: bypass`).
- [ ] **Step 2: Run → fail. Step 3: Implement all eight cards + palette add/remove wiring** (port layout/interactions from the mockup; drag from palette with `dragstart`/`drop` + click-to-add fallback, exactly as `agent-builder.html`). **Step 4: Run → pass. Step 5: Commit** `-m "feat(cp-web): remaining agent builder cards (tools/permission/reasoning/context/output/dispatch/anthropic/concerns)"`.

---

## Task 8: AI-assist + Raw mode

**Files:**
- Modify: `AgentBuilder.tsx`
- Test: extend `AgentBuilder.test.tsx`

- [ ] **Step 1: Failing test** — render with `onGenerate` mock resolving `{ raw: '---\nname: gen\n...' }`; switch to AI tab; type a description; click Generate; assert cards repopulate (name input shows `gen`) and mode returns to Cards. Also: switch to Raw, edit text, switch back to Cards → change reflected (parse round-trip).
- [ ] **Step 2: Run → fail. Step 3: Implement** the AI tab (textarea + provider `<select>` from `aiModels` + Generate button calling `onGenerate` then `setDraft(parseAgent(res.raw))`); Raw tab (`CodeEditor` bound to `serializeAgent(draft)` ↔ `parseAgent`). **Step 4: pass. Step 5: Commit** `-m "feat(cp-web): agent builder AI-assist + raw mode"`.

---

## Task 9: Wire into create (`NewAgentModal`) behind the flag

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Agents.tsx` (`NewAgentModal`, lines 123-300)
- Test: `crates/rupu-cp/web/src/pages/NewAgentModal.test.tsx` (extend)

- [ ] **Step 1: Failing test** — with `localStorage['rupu.cp.agentUi']='next'` (and `getConfig` mocked), open the modal, assert the Agent Builder renders (name field present) and NOT the raw `data-testid="code-editor"`; fill name; submit; assert `api.createAgent` called with a raw string; with the flag unset assert the classic raw editor still renders.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — call `useAgentAuthoringUi()`; when `'next'`, render `<AgentBuilder initialRaw={NEW_AGENT_TEMPLATE} submitLabel="Create agent" submitting={creating} error={error} onSubmit={(raw)=>createFrom(raw)} aiModels={models} onGenerate={api.generateAgent} agentNames={...} />` where `createFrom` factors the existing `create()` body to accept `raw`; else keep the current describe/edit UI unchanged. Keep Escape-to-close.
- [ ] **Step 4: Run → pass.**
- [ ] **Step 5: Commit** `-m "feat(cp-web): render Agent Builder in New Agent modal behind flag"`.

---

## Task 10: Wire into edit (`AgentDetail`) behind the flag

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/AgentDetail.tsx` (edit section, lines 24-196)
- Test: `crates/rupu-cp/web/src/pages/AgentDetail.test.tsx` (extend)

- [ ] **Step 1: Failing test** — flag `'next'`, click Edit, assert Agent Builder renders seeded from `agent.raw` (name field shows the agent name), change a field, Save → `api.saveAgent(name, raw)` called; flag unset → classic `CodeEditor` still used.
- [ ] **Step 2: Run → fail.**
- [ ] **Step 3: Implement** — `useAgentAuthoringUi()`; when `'next'` and `editing`, render `<AgentBuilder initialRaw={agent.raw} submitLabel="Save" submitting={saving} error={saveError} onSubmit={(raw)=>saveFrom(raw)} />` (no AI tab on edit); `saveFrom` wraps existing `save()` to accept raw. Non-editing view keeps `<CodeHighlight code={agent.raw} frontmatter />`.
- [ ] **Step 4: Run → pass.**
- [ ] **Step 5: Commit** `-m "feat(cp-web): render Agent Builder in AgentDetail edit behind flag"`.

---

## Task 11: Full-suite green + build + PR

- [ ] **Step 1:** `npm run build` (tsc + vite) from `crates/rupu-cp/web` → clean.
- [ ] **Step 2:** `npx vitest run` → all green (existing 900+ tests + the new ones).
- [ ] **Step 3:** `cargo test -p rupu-config` → green.
- [ ] **Step 4:** Commit any lint/build fixups; open a draft PR summarizing the flag + the two entry points, and noting the flag defaults to classic (zero visual change until `[cp] agent_authoring_ui = "next"` or the localStorage override).

## Verification (end-to-end)

1. **Flag off (default):** build + run the CP; `/agents` "New agent" and `AgentDetail` "Edit" show the **current raw editor** unchanged. (`cargo test -p rupu-config` proves the default; a web test proves classic renders when unset.)
2. **Flag on:** set `localStorage['rupu.cp.agentUi'] = 'next'` (or `[cp] agent_authoring_ui = "next"` in config.toml). Reload → "New agent" shows the card builder; add cards, watch the live `.md` preview update; Create → `POST /api/agents` with the serialized raw; open the agent → Edit → cards seeded from `raw`; Save → `PUT /api/agents/:name`.
3. **No-regression proof:** the builder emits only known keys, so `deny_unknown_fields` can't trip; the raw/AI fallbacks and the classic path are all still reachable. Round-trip test (Task 3) guarantees editing an existing agent never drops unmodeled keys (`_passthrough`).

## Out of scope (later passes)
- Server `POST /api/agents/validate` endpoint for a live badge (client-side `validateAgentDraft` covers pass 1; the final Create/Save is the authoritative validator). Add later if we want workflow-parity.
- Flow Designer, Run Room (passes 2 & 3).
- A user-facing Settings toggle for the flag (dogfood via localStorage / config.toml for now).
