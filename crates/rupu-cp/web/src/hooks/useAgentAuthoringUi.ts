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
