import { useEffect, useState } from 'react';
import { api } from '../lib/api';

export type WorkflowEditorUi = 'classic' | 'next';
const STORAGE_KEY = 'rupu.cp.workflowEditorUi';

export function resolveWorkflowEditorUi(cp: Record<string, unknown> | null, override: string | null): WorkflowEditorUi {
  const pick = (v: unknown): WorkflowEditorUi | null => (v === 'next' || v === 'classic' ? v : null);
  return pick(override) ?? pick(cp?.workflow_editor_ui) ?? 'classic';
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

export function useWorkflowEditorUi(): WorkflowEditorUi {
  // Seed synchronously from the localStorage override so a dogfooder sees
  // 'next' on first paint with no flash; otherwise start 'classic' and
  // upgrade once server config resolves.
  const [ui, setUi] = useState<WorkflowEditorUi>(() => resolveWorkflowEditorUi(null, readOverride()));
  useEffect(() => {
    let live = true;
    loadCp().then((cp) => { if (live) setUi(resolveWorkflowEditorUi(cp, readOverride())); });
    return () => { live = false; };
  }, []);
  return ui;
}
