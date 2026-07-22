// AgentBuilder — the card-composer shell (Tasks 6-7 of the Agent Builder
// plan). Palette (left) | canvas of active field cards (center) | live `.md`
// preview (right), a header with the name field / mode toggle / validity
// badge / submit, and a Cards / Raw / AI mode switch. Wires together the
// pure model (`agentSpec.ts`, `validate.ts`), the static `CARD_REGISTRY`
// (`fields.ts`), and the control primitives (`controls.tsx`).
//
// Every `CARD_REGISTRY` entry now renders a real card body via `renderCardBody`
// below.
//
// Card add is real HTML5 drag-and-drop (palette card -> `dragstart` sets
// `text/card`; canvas -> `dragover`/`drop` calls `addCard`), ported from the
// approved mockup, PLUS click-to-add as a fallback (same handler either way
// — see the palette `onClick`). Canvas cards are also reorderable by
// dragging their header (`text/reorder`, tracked via `dragCardId`/
// `dragOverId` state rather than the mockup's manual DOM class toggling, so
// drag-over highlighting stays declarative). Card remove is still a click
// (✕ on the card header).
import { useEffect, useState } from 'react';
import {
  parseAgent,
  serializeAgent,
  type AgentDraft,
} from '../../lib/agentBuilder/agentSpec';
import { validateAgentDraft } from '../../lib/agentBuilder/validate';
import { CARD_REGISTRY, type CardMeta } from '../../lib/agentBuilder/fields';
import { Segmented } from './controls';
import Identity from './cards/Identity';
import Model from './cards/Model';
import Prompt from './cards/Prompt';
import Tools from './cards/Tools';
import Permission from './cards/Permission';
import Reasoning from './cards/Reasoning';
import Context from './cards/Context';
import Output from './cards/Output';
import Dispatch from './cards/Dispatch';
import Anthropic from './cards/Anthropic';
import Concerns from './cards/Concerns';
import CodeHighlight from '../CodeHighlight';
import CodeEditor from '../CodeEditor';
import { Button } from '../ui/Button';
import { cn } from '../../lib/cn';
import type { GenerateBody, GeneratedDef, ProviderModels } from '../../lib/api';

export interface AgentBuilderProps {
  initialRaw: string;
  submitLabel: string;
  submitting: boolean;
  error: string | null;
  onSubmit: (raw: string) => void;
  onCancel?: () => void;
  aiModels?: ProviderModels[];
  onGenerate?: (body: GenerateBody) => Promise<GeneratedDef>;
  agentNames?: string[];
}

type Mode = 'cards' | 'raw' | 'ai';

// Which `AgentDraft` keys each non-Core card owns. Doubles as (a) the
// "does the parsed draft already have data for this card" check that seeds
// the initial canvas, and (b) what gets cleared when a card is removed —
// one map, so the two can never drift apart.
const CARD_FIELDS: Partial<Record<string, (keyof AgentDraft)[]>> = {
  model: ['provider', 'auth', 'model'],
  tools: ['tools'],
  permission: ['permissionMode'],
  reasoning: ['effort', 'maxTurns', 'maxTokens'],
  context: ['contextWindow', 'contextWindowTokens', 'compactAtPercent'],
  output: ['outputFormat', 'outputSchema'],
  dispatch: ['dispatchableAgents'],
  anthropic: ['anthropicSpeed', 'anthropicContextManagement', 'anthropicTaskBudget', 'anthropicOauthPrefix'],
  concerns: ['concerns'],
};

// Canvas render order preference: Identity/Model up front, Prompt last (the
// body is usually the biggest thing and reads naturally at the end), the
// rest of the registry in between.
const CANVAS_ORDER = ['identity', 'model', 'tools', 'permission', 'reasoning', 'context', 'output', 'dispatch', 'anthropic', 'concerns', 'prompt'];

function hasValue(v: unknown): boolean {
  if (v === undefined || v === null) return false;
  if (typeof v === 'string') return v.length > 0;
  if (Array.isArray(v)) return v.length > 0;
  return true;
}

// The set of card ids that "want" to be on the canvas for a given draft:
// required cards, and any card (including Model) whose owned fields
// (`CARD_FIELDS`) have a value — regardless of whether that value arrived
// via a card edit, Raw-mode YAML, or AI generation. Shared by the initial
// `useState` seed AND the reactive union effect below, so "what belongs on
// the canvas for this draft" is computed exactly one way. Model is NOT
// special-cased as always-wanted: doing so meant `removeCard('model')`
// (which clears provider/auth/model) got immediately re-added by the next
// reactive union pass, silently wiping the user's selection and snapping
// the blank card back (see AgentBuilder.test.tsx "model card removal
// sticks").
function wantedCardIds(draft: AgentDraft): string[] {
  const byId = new Map(CARD_REGISTRY.map((c) => [c.id, c] as const));
  return CANVAS_ORDER.filter((id) => {
    const meta = byId.get(id);
    if (!meta) return false;
    if (meta.required) return true;
    const fields = CARD_FIELDS[id];
    return fields ? fields.some((f) => hasValue(draft[f])) : false;
  });
}

function computeInitialOrder(draft: AgentDraft): string[] {
  return wantedCardIds(draft);
}

// Insert any card ids that now "want" to be present (per `wantedCardIds`)
// but aren't yet in `prev`, without disturbing the existing order — new
// cards land just before `prompt` (if present) so the body still reads last.
// Returns `prev` unchanged (same reference) when nothing is missing, so a
// caller wiring this into a `setOrder` functional update never causes an
// extra render/loop.
function unionWantedCards(prev: string[], draft: AgentDraft): string[] {
  const present = new Set(prev);
  const toAdd = wantedCardIds(draft).filter((id) => !present.has(id));
  if (toAdd.length === 0) return prev;
  const promptIdx = prev.indexOf('prompt');
  if (promptIdx === -1) return [...prev, ...toAdd];
  return [...prev.slice(0, promptIdx), ...toAdd, ...prev.slice(promptIdx)];
}

const GROUPS: CardMeta['group'][] = ['Core', 'Runtime', 'Advanced'];

function cardIcon(id: string): string {
  return id.slice(0, 2).toUpperCase();
}

function renderCardBody(
  id: string,
  draft: AgentDraft,
  patch: (p: Partial<AgentDraft>) => void,
  agentNames: string[] | undefined,
) {
  switch (id) {
    case 'identity':
      return <Identity draft={draft} patch={patch} />;
    case 'model':
      return <Model draft={draft} patch={patch} />;
    case 'prompt':
      return <Prompt draft={draft} patch={patch} />;
    case 'tools':
      return <Tools draft={draft} patch={patch} />;
    case 'permission':
      return <Permission draft={draft} patch={patch} />;
    case 'reasoning':
      return <Reasoning draft={draft} patch={patch} />;
    case 'context':
      return <Context draft={draft} patch={patch} />;
    case 'output':
      return <Output draft={draft} patch={patch} />;
    case 'dispatch':
      return <Dispatch draft={draft} patch={patch} agentNames={agentNames} />;
    case 'anthropic':
      return <Anthropic draft={draft} patch={patch} />;
    case 'concerns':
      return <Concerns draft={draft} patch={patch} />;
    default:
      return <div className="ab-hint">Coming in the next task.</div>;
  }
}

function ValidityBadge({ ok }: { ok: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 py-1',
        'text-meta font-semibold uppercase tracking-wide',
        ok ? 'border-ok/30 bg-ok-bg text-ok' : 'border-err/30 bg-err-bg text-err',
      )}
    >
      <span className="h-1.5 w-1.5 rounded-full bg-current" />
      {ok ? 'valid' : 'invalid'}
    </span>
  );
}

export default function AgentBuilder({
  initialRaw,
  submitLabel,
  submitting,
  error,
  onSubmit,
  onCancel,
  aiModels,
  onGenerate,
  agentNames,
}: AgentBuilderProps) {
  const [draft, setDraft] = useState<AgentDraft>(() => parseAgent(initialRaw));
  const [order, setOrder] = useState<string[]>(() => computeInitialOrder(draft));
  const [mode, setMode] = useState<Mode>('cards');
  const [rawText, setRawText] = useState(() => serializeAgent(draft));
  const [aiDescription, setAiDescription] = useState('');
  const [aiProvider, setAiProvider] = useState<string>(
    () => aiModels?.find((m) => m.is_default)?.provider ?? aiModels?.[0]?.provider ?? '',
  );
  const [aiBusy, setAiBusy] = useState(false);
  const [aiError, setAiError] = useState<string | null>(null);
  const [dragCardId, setDragCardId] = useState<string | null>(null);
  const [dragOverId, setDragOverId] = useState<string | null>(null);

  // Keep the Raw-mode buffer in sync with card-mode edits (but not the other
  // way while the user is actively typing invalid intermediate YAML — see
  // handleRawChange).
  useEffect(() => {
    if (mode !== 'raw') setRawText(serializeAgent(draft));
  }, [draft, mode]);

  // `aiModels` may arrive asynchronously (the parent fetches it after
  // mount), so re-derive the default provider whenever the list changes and
  // the current selection isn't one of its entries.
  useEffect(() => {
    if (!aiModels || aiModels.length === 0) return;
    setAiProvider((prev) => (aiModels.some((m) => m.provider === prev) ? prev : aiModels.find((m) => m.is_default)?.provider ?? aiModels[0].provider));
  }, [aiModels]);

  // Card order reacts to the draft, not just the initial mount: any field
  // group populated in the draft — via a card edit, Raw-mode YAML, or AI
  // generation — surfaces its card on the canvas once we're (back) in Cards
  // mode. Cards the user explicitly removed stay removed because
  // `removeCard` clears their owned fields, so they no longer "want" to be
  // present. `unionWantedCards` returns the same array reference when
  // nothing is missing, so this never loops.
  useEffect(() => {
    if (mode !== 'cards') return;
    setOrder((prev) => unionWantedCards(prev, draft));
  }, [draft, mode]);

  function patch(p: Partial<AgentDraft>) {
    setDraft((d) => ({ ...d, ...p }));
  }

  function addCard(id: string) {
    setOrder((prev) => (prev.includes(id) ? prev : [...prev, id]));
  }

  function removeCard(id: string, meta: CardMeta) {
    if (meta.required) return;
    setOrder((prev) => prev.filter((x) => x !== id));
    const fields = CARD_FIELDS[id];
    if (fields) {
      const clear: Partial<AgentDraft> = {};
      for (const f of fields) clear[f] = undefined;
      patch(clear);
    }
  }

  function moveCardBefore(dragId: string, targetId: string) {
    if (dragId === targetId) return;
    setOrder((prev) => {
      const from = prev.indexOf(dragId);
      const to = prev.indexOf(targetId);
      if (from === -1 || to === -1) return prev;
      const next = prev.slice();
      next.splice(from, 1);
      next.splice(next.indexOf(targetId), 0, dragId);
      return next;
    });
  }

  function handleRawChange(v: string) {
    setRawText(v);
    try {
      setDraft(parseAgent(v));
    } catch {
      // Invalid intermediate YAML mid-edit — keep the typed text, don't
      // propagate to the draft (and therefore the preview) until it parses.
    }
  }

  async function handleGenerate() {
    if (!onGenerate || !aiDescription.trim() || aiBusy) return;
    setAiBusy(true);
    setAiError(null);
    try {
      const sel = aiModels?.find((m) => m.provider === aiProvider);
      const result = await onGenerate({
        description: aiDescription,
        provider: aiProvider || undefined,
        model: sel?.models[0],
      });
      const parsed = parseAgent(result.raw);
      setDraft(parsed);
      setOrder(computeInitialOrder(parsed));
      setMode('cards');
    } catch (e) {
      setAiError(e instanceof Error ? e.message : 'Generation failed');
    } finally {
      setAiBusy(false);
    }
  }

  const validation = validateAgentDraft(draft);
  const activeIds = new Set(order);

  const modeOptions = [
    { label: 'Cards', value: 'cards' as Mode },
    ...(onGenerate ? [{ label: 'AI', value: 'ai' as Mode }] : []),
    { label: 'Raw', value: 'raw' as Mode },
  ];

  const groups: Record<CardMeta['group'], CardMeta[]> = { Core: [], Runtime: [], Advanced: [] };
  for (const c of CARD_REGISTRY) groups[c.group].push(c);
  const unusedCount = CARD_REGISTRY.filter((c) => !activeIds.has(c.id)).length;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center gap-3 border-b border-border bg-panel/80 px-4 py-2.5 backdrop-blur">
        <div className="min-w-0">
          <div className="text-lead font-semibold">Agent Builder</div>
          <div className="text-meta uppercase tracking-wide text-ink-mute">.rupu/agents · card composer</div>
        </div>
        <input
          className="ab-txt mono w-56 font-semibold"
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          aria-label="agent name"
        />
        <ValidityBadge ok={validation.ok} />
        <div className="ml-auto flex items-center gap-3">
          <Segmented<Mode> options={modeOptions} value={mode} onChange={setMode} />
          {onCancel && (
            <Button variant="secondary" size="sm" onClick={onCancel}>
              Cancel
            </Button>
          )}
          <Button
            variant="primary"
            size="sm"
            disabled={!validation.ok || submitting}
            onClick={() => onSubmit(serializeAgent(draft))}
          >
            {submitLabel}
          </Button>
        </div>
      </header>

      {error && (
        <div className="border-b border-err/30 bg-err-bg px-4 py-2 text-note text-err" role="alert">
          {error}
        </div>
      )}

      {validation.warnings.length > 0 && (
        <div className="border-b border-warn/30 bg-warn-bg px-4 py-2 text-note text-warn">
          {validation.warnings.map((w, i) => (
            <div key={`${w.field}-${i}`}>{w.message}</div>
          ))}
        </div>
      )}

      {mode === 'ai' && onGenerate && (
        <div className="flex items-center gap-2.5 border-b border-border bg-brand-50 px-4 py-2.5">
          <input
            className="ab-txt flex-1"
            placeholder="Describe the agent — e.g. “a read-only security reviewer for panel workflows”"
            value={aiDescription}
            onChange={(e) => setAiDescription(e.target.value)}
            aria-label="describe the agent"
          />
          {aiModels && aiModels.length > 0 && (
            <select
              className="ab-sel"
              value={aiProvider}
              onChange={(e) => setAiProvider(e.target.value)}
              aria-label="generation provider"
            >
              {aiModels.map((m) => (
                <option key={m.provider} value={m.provider}>
                  {m.provider} · {m.models[0]}
                </option>
              ))}
            </select>
          )}
          <Button variant="primary" size="sm" disabled={aiBusy || !aiDescription.trim()} onClick={handleGenerate}>
            {aiBusy ? 'Generating…' : 'Generate cards'}
          </Button>
          {aiError && (
            <span className="text-note text-err" role="alert">
              {aiError}
            </span>
          )}
        </div>
      )}

      {mode === 'raw' ? (
        <div className="min-h-0 flex-1 overflow-auto p-4">
          <CodeEditor value={rawText} onChange={handleRawChange} language="markdown" ariaLabel="raw agent definition" />
        </div>
      ) : (
        <div className="grid min-h-0 flex-1" style={{ gridTemplateColumns: '244px 1fr 400px' }}>
          {/* palette */}
          <div className="ab-palette">
            <div className="ab-colhead">
              <h2>Field cards</h2>
              <span className="ab-cnt">{unusedCount} unused</span>
            </div>
            <div className="ab-palette-scroll">
              {GROUPS.map((group) => (
                <div key={group}>
                  <div className="ab-pgroup">{group}</div>
                  {groups[group].map((c) => {
                    const used = activeIds.has(c.id);
                    return (
                      <div
                        key={c.id}
                        className={cn('ab-pcard', used && 'ab-used')}
                        data-req={c.required ? 1 : 0}
                        role="button"
                        tabIndex={used ? -1 : 0}
                        draggable={!used}
                        onDragStart={(e) => {
                          if (used) return;
                          e.dataTransfer.setData('text/card', c.id);
                          e.dataTransfer.effectAllowed = 'copy';
                        }}
                        onClick={() => !used && addCard(c.id)}
                        onKeyDown={(e) => {
                          if (!used && (e.key === 'Enter' || e.key === ' ')) {
                            e.preventDefault();
                            addCard(c.id);
                          }
                        }}
                        aria-label={`add ${c.label} card`}
                      >
                        <span className="ab-ic">{cardIcon(c.id)}</span>
                        <div>
                          <div className="ab-pl">{c.label}</div>
                          <div className="ab-pd mono">{c.yamlKeys}</div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              ))}
            </div>
          </div>

          {/* canvas */}
          <div className="ab-canvas-wrap">
            <div className="ab-colhead">
              <h2>Agent composition</h2>
              <span className="ab-cnt">{order.length} cards</span>
            </div>
            <div
              className="ab-canvas"
              data-testid="ab-canvas-drop"
              onDragOver={(e) => {
                if (e.dataTransfer.types.includes('text/card')) e.preventDefault();
              }}
              onDrop={(e) => {
                const id = e.dataTransfer.getData('text/card');
                if (id && !activeIds.has(id)) {
                  e.preventDefault();
                  addCard(id);
                }
              }}
            >
              <div className="ab-canvas-inner">
                {order.length === 0 && (
                  <div className="ab-dropzone ab-empty">Drag field cards here to compose the agent.</div>
                )}
                {order.map((id) => {
                  const meta = CARD_REGISTRY.find((c) => c.id === id);
                  if (!meta) return null;
                  return (
                    <div
                      className={cn('ab-card', dragOverId === id && 'ab-drag-over')}
                      key={id}
                      onDragOver={(e) => {
                        if (dragCardId && dragCardId !== id) {
                          e.preventDefault();
                          setDragOverId(id);
                        }
                      }}
                      onDragLeave={() => setDragOverId((cur) => (cur === id ? null : cur))}
                      onDrop={(e) => {
                        if (dragCardId && dragCardId !== id) {
                          e.preventDefault();
                          e.stopPropagation();
                          moveCardBefore(dragCardId, id);
                        }
                        setDragOverId(null);
                      }}
                    >
                      <div
                        className="ab-card-head"
                        draggable
                        onDragStart={(e) => {
                          setDragCardId(id);
                          e.dataTransfer.effectAllowed = 'move';
                          e.dataTransfer.setData('text/reorder', id);
                        }}
                        onDragEnd={() => {
                          setDragCardId(null);
                          setDragOverId(null);
                        }}
                      >
                        <span className="ab-grip" aria-hidden="true">
                          ⠿
                        </span>
                        <span className="ab-ic">{cardIcon(id)}</span>
                        <span className="ab-ct">{meta.label}</span>
                        <span className="ab-cyaml mono">{meta.yamlKeys}</span>
                        {!meta.required && (
                          <button
                            type="button"
                            className="ab-rm"
                            aria-label={`remove ${meta.label} card`}
                            onClick={() => removeCard(id, meta)}
                          >
                            ✕
                          </button>
                        )}
                      </div>
                      <div className="ab-card-body">{renderCardBody(id, draft, patch, agentNames)}</div>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>

          {/* live preview */}
          <div className="ab-yaml">
            <div className="ab-colhead">
              <h2>Live definition</h2>
              <span className="ab-cnt">{(draft.name || 'agent') + '.md'}</span>
            </div>
            <div className="ab-yaml-scroll" data-testid="ab-yaml">
              <CodeHighlight code={serializeAgent(draft)} frontmatter />
            </div>
            <div className="ab-yaml-foot">
              <span>{order.length} cards</span>
              <span className="ml-auto">{validation.ok ? 'parses clean · deny_unknown_fields' : validation.errors[0]?.message}</span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
