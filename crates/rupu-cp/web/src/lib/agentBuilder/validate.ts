// Pure, I/O-free validation of an `AgentDraft`. No React, no fetch — safe to
// call on every keystroke from the builder UI. Vocab is imported from
// `./agentSpec`, never re-declared here, so the allowed value sets can never
// drift between the two files.
import {
  AgentDraft,
  AUTH_MODES,
  PERMISSION_MODES,
  EFFORT_LEVELS,
  CONTEXT_WINDOWS,
  OUTPUT_FORMATS,
  ANTHROPIC_SPEED,
  ANTHROPIC_CTX_MGMT,
  BUILTIN_TOOLS,
  ConcernEntry,
  Severity,
} from './agentSpec';

export interface FieldError {
  field: string;
  message: string;
}

// `agentSpec.ts` exports the `Severity` *type* but no runtime vocab const for
// it (unlike the other enum fields) — this is the one list this file must
// own, since there's nothing to import.
const SEVERITIES: readonly Severity[] = ['info', 'low', 'medium', 'high', 'critical'];

/** CP backend slug rule (`crates/rupu-cp/src/api/fs_safety.rs::validate_name`)
 *  is `^[A-Za-z][A-Za-z0-9_-]*$`. The builder deliberately enforces a
 *  stricter subset here — lowercase letters/digits/hyphens only — so every
 *  name the UI accepts is also guaranteed to pass the backend's check. */
const NAME_SLUG_RE = /^[a-z0-9][a-z0-9-]*$/;

const NAME_SLUG_MESSAGE =
  'Name must be lowercase letters, digits, and hyphens only (e.g. "security-reviewer")';

function checkEnum<T extends string>(
  field: string,
  value: T | undefined,
  vocab: readonly T[],
  errors: FieldError[]
): void {
  if (value === undefined) return;
  if (!(vocab as readonly string[]).includes(value)) {
    errors.push({ field, message: `"${value}" is not a valid ${field} (expected one of: ${vocab.join(', ')})` });
  }
}

function checkConcernSeverities(concerns: ConcernEntry[], errors: FieldError[]): void {
  concerns.forEach((entry, i) => {
    if (entry.kind === 'inline') {
      checkEnum(`concerns[${i}].severity`, entry.severity, SEVERITIES, errors);
    } else {
      entry.overrides?.forEach((o, j) => {
        checkEnum(`concerns[${i}].overrides[${j}].severity`, o.severity, SEVERITIES, errors);
      });
    }
  });
}

function isDottedMcpToolName(name: string): boolean {
  return name.includes('.');
}

export function validateAgentDraft(d: AgentDraft): { ok: boolean; errors: FieldError[]; warnings: FieldError[] } {
  const errors: FieldError[] = [];
  const warnings: FieldError[] = [];

  if (!d.name || d.name.trim().length === 0) {
    errors.push({ field: 'name', message: 'Name is required' });
  } else if (!NAME_SLUG_RE.test(d.name)) {
    errors.push({ field: 'name', message: NAME_SLUG_MESSAGE });
  }

  if (d.compactAtPercent !== undefined && (d.compactAtPercent < 10 || d.compactAtPercent > 95)) {
    errors.push({
      field: 'compactAtPercent',
      message: `compactAtPercent must be between 10 and 95 (got ${d.compactAtPercent})`,
    });
  }

  checkEnum('auth', d.auth, AUTH_MODES, errors);
  checkEnum('permissionMode', d.permissionMode, PERMISSION_MODES, errors);
  checkEnum('effort', d.effort, EFFORT_LEVELS, errors);
  checkEnum('contextWindow', d.contextWindow, CONTEXT_WINDOWS, errors);
  checkEnum('outputFormat', d.outputFormat, OUTPUT_FORMATS, errors);
  checkEnum('anthropicSpeed', d.anthropicSpeed, ANTHROPIC_SPEED, errors);
  checkEnum('anthropicContextManagement', d.anthropicContextManagement, ANTHROPIC_CTX_MGMT, errors);

  if (d.concerns) checkConcernSeverities(d.concerns, errors);

  if (!d.body || d.body.trim().length === 0) {
    warnings.push({ field: 'body', message: 'system prompt (body) is empty — the server accepts this, but the agent has no instructions' });
  }

  if (d.tools !== undefined) {
    if (d.tools.length === 0) {
      warnings.push({ field: 'tools', message: 'empty tools list grants the full default registry' });
    } else {
      for (const tool of d.tools) {
        if (!(BUILTIN_TOOLS as readonly string[]).includes(tool) && !isDottedMcpToolName(tool)) {
          warnings.push({
            field: 'tools',
            message: `"${tool}" is not a builtin tool or a dotted MCP tool name (e.g. "scm.prs.get")`,
          });
        }
      }
    }
  }

  return { ok: errors.length === 0, errors, warnings };
}
