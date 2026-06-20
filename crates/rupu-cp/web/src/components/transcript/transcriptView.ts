/**
 * Pure event → view mapping for the agent transcript panel.
 *
 * Turns the flat `TranscriptEvent[]` stream (adjacently-tagged) into an ordered
 * render model the `TranscriptPanel` component can paint without re-deriving
 * structure. All pairing / attaching logic lives here so it stays testable:
 *
 *   • `tool_result` events are paired to their `tool_call` by `call_id`.
 *   • `assistant_message.thinking` rides on the assistant item.
 *   • a header is surfaced from `run_start` (agent / model / provider).
 *   • a footer is surfaced from `run_complete` (status, total tokens, duration),
 *     falling back to the last `usage` event when the run hasn't completed.
 *
 * No React, no DOM — a deterministic function over the event list.
 */

import type { TranscriptEvent } from '../../lib/transcript';

// ---------------------------------------------------------------------------
// View model
// ---------------------------------------------------------------------------

export interface TranscriptHeader {
  agent: string;
  model: string;
  provider: string;
  mode: string;
  startedAt: string;
}

export interface TranscriptFooter {
  /** Run status when known (`run_complete`), else null (still running). */
  status: string | null;
  totalTokens: number | null;
  durationMs: number | null;
  error: string | null;
}

export interface ToolResultView {
  output: string;
  error: string | null;
  durationMs: number | null;
}

export interface UserItem {
  kind: 'user';
  key: string;
  content: string;
}

export interface AssistantItem {
  kind: 'assistant';
  key: string;
  content: string;
  /** Attached `assistant_message.thinking`, if any. */
  thinking: string | null;
}

export interface ToolItem {
  kind: 'tool';
  key: string;
  /** call_id linking the call to its result; present even for orphan results. */
  callId: string;
  /** Tool name from the `tool_call`; null for an unpaired `tool_result`. */
  tool: string | null;
  /** Raw `tool_call.input` (unknown JSON shape). */
  input: unknown;
  /** Paired result, or null while the call is still in-flight. */
  result: ToolResultView | null;
}

/** A finding surfaced from an `action_emitted`/finding-shaped event. */
export interface FindingItem {
  kind: 'finding';
  key: string;
  severity: string | null;
  title: string;
}

/** A file edit / command-run chip. */
export interface ChipItem {
  kind: 'chip';
  key: string;
  /** 'file_edit' | 'command_run' | other event type. */
  variant: string;
  label: string;
}

/** Any other event we don't specialise — rendered as a dim meta line. */
export interface EventItem {
  kind: 'event';
  key: string;
  type: string;
  label: string;
}

export type TranscriptItem =
  | UserItem
  | AssistantItem
  | ToolItem
  | FindingItem
  | ChipItem
  | EventItem;

export interface TranscriptView {
  header: TranscriptHeader | null;
  items: TranscriptItem[];
  footer: TranscriptFooter | null;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function asString(v: unknown): string | null {
  return typeof v === 'string' ? v : null;
}

function asNumber(v: unknown): number | null {
  return typeof v === 'number' ? v : null;
}

/** Short, single-line preview of an arbitrary JSON value for chips/labels. */
function previewValue(v: unknown, max = 120): string {
  let s: string;
  if (typeof v === 'string') s = v;
  else if (v === null || v === undefined) s = '';
  else {
    try {
      s = JSON.stringify(v);
    } catch {
      s = String(v);
    }
  }
  s = s.replace(/\s+/g, ' ').trim();
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

export function buildTranscriptView(events: TranscriptEvent[]): TranscriptView {
  let header: TranscriptHeader | null = null;
  let footer: TranscriptFooter | null = null;

  const items: TranscriptItem[] = [];
  // Index of the ToolItem in `items`, keyed by call_id, so a later
  // `tool_result` can be attached to its earlier `tool_call`.
  const toolByCall = new Map<string, number>();

  events.forEach((ev, idx) => {
    const data = (ev.data ?? {}) as Record<string, unknown>;
    const key = `${ev.type}-${idx}`;

    switch (ev.type) {
      case 'run_start': {
        header = {
          agent: asString(data.agent) ?? '',
          model: asString(data.model) ?? '',
          provider: asString(data.provider) ?? '',
          mode: asString(data.mode) ?? '',
          startedAt: asString(data.started_at) ?? '',
        };
        break;
      }

      case 'user_message': {
        items.push({ kind: 'user', key, content: asString(data.content) ?? '' });
        break;
      }

      case 'assistant_message': {
        items.push({
          kind: 'assistant',
          key,
          content: asString(data.content) ?? '',
          thinking: asString(data.thinking),
        });
        break;
      }

      case 'tool_call': {
        const callId = asString(data.call_id) ?? key;
        const pos = items.length;
        items.push({
          kind: 'tool',
          key,
          callId,
          tool: asString(data.tool),
          input: data.input,
          result: null,
        });
        toolByCall.set(callId, pos);
        break;
      }

      case 'tool_result': {
        const callId = asString(data.call_id) ?? '';
        const result: ToolResultView = {
          output: asString(data.output) ?? '',
          error: asString(data.error),
          durationMs: asNumber(data.duration_ms),
        };
        const pos = toolByCall.get(callId);
        if (pos !== undefined) {
          const existing = items[pos];
          if (existing.kind === 'tool') existing.result = result;
        } else {
          // Unpaired result — surface it as its own tool item so nothing is lost.
          items.push({
            kind: 'tool',
            key,
            callId,
            tool: null,
            input: undefined,
            result,
          });
        }
        break;
      }

      case 'file_edit': {
        const path = asString(data.path) ?? asString(data.file) ?? '';
        items.push({
          kind: 'chip',
          key,
          variant: 'file_edit',
          label: path || previewValue(data),
        });
        break;
      }

      case 'command_run': {
        const cmd = asString(data.command) ?? asString(data.cmd) ?? '';
        items.push({
          kind: 'chip',
          key,
          variant: 'command_run',
          label: cmd || previewValue(data),
        });
        break;
      }

      case 'action_emitted': {
        // Findings ride in on action_emitted (report_finding) — surface those
        // specially; other actions fall through to a dim event line.
        const action = asString(data.action) ?? asString(data.name);
        const isFinding =
          action === 'report_finding' || 'severity' in data || 'finding' in data;
        if (isFinding) {
          items.push({
            kind: 'finding',
            key,
            severity: asString(data.severity),
            title:
              asString(data.title) ?? asString(data.summary) ?? previewValue(data),
          });
        } else {
          items.push({
            kind: 'event',
            key,
            type: ev.type,
            label: action ? `${action} · ${previewValue(data)}` : previewValue(data),
          });
        }
        break;
      }

      case 'usage': {
        const input = asNumber(data.input_tokens) ?? 0;
        const output = asNumber(data.output_tokens) ?? 0;
        // Only seed a footer from usage if run_complete hasn't already set one.
        if (!footer) {
          footer = {
            status: null,
            totalTokens: input + output,
            durationMs: null,
            error: null,
          };
        } else if (footer.totalTokens === null) {
          footer.totalTokens = input + output;
        }
        break;
      }

      case 'run_complete': {
        footer = {
          status: asString(data.status),
          totalTokens: asNumber(data.total_tokens),
          durationMs: asNumber(data.duration_ms),
          error: asString(data.error),
        };
        break;
      }

      // turn_start / turn_end / assistant_delta / gate_requested and any
      // forward-compat variants: skipped (deltas are coalesced into the final
      // assistant_message; turn markers carry no render payload of their own).
      case 'turn_start':
      case 'turn_end':
      case 'assistant_delta':
      case 'gate_requested':
        break;

      default: {
        items.push({ kind: 'event', key, type: ev.type, label: previewValue(data) });
        break;
      }
    }
  });

  return { header, items, footer };
}
