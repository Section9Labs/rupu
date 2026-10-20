// New agentiflow — a definition's YAML, checked by the server as you type
// (`POST /api/agentiflow-defs/validate`), then created in the global layer
// (`POST /api/agentiflow-defs`) and opened on its definition page.

import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { apiErrorMessage } from '../../lib/api';
import { agentiflowsApi } from '../../lib/api3b/agentiflows';
import { defHref, useDefValidity } from '../../lib/agentiflowDefs';
import CodeEditor from '../CodeEditor';
import { DialogFrame } from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { DefValidityBadge } from './DefValidityBadge';

export const NEW_AGENTIFLOW_TEMPLATE = `name: my-agentiflow
description: What this fleet is for.
lead: my-lead-agent
engagement_profiles: [code]

goals:
  - id: first-goal
    objective: Describe what done looks like.
    target: { findings: {}, count_gte: 1 }
    required: true

budget:
  usd: 5
  rounds: 6
  wall_clock: "30m"

pool:
  agents: [my-lead-agent]
`;

export default function NewAgentiflowDefModal({ onClose }: { onClose: () => void }) {
  const navigate = useNavigate();
  const [raw, setRaw] = useState(NEW_AGENTIFLOW_TEMPLATE);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const validity = useDefValidity(raw);

  async function create() {
    if (creating) return;
    setCreating(true);
    setError(null);
    try {
      const created = await agentiflowsApi.createDef(raw);
      navigate(defHref(created));
      onClose();
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
      setCreating(false);
    }
  }

  return (
    <DialogFrame title="New agentiflow" onRequestClose={onClose} className="max-w-2xl">
      <div className="mt-1 flex items-center gap-3">
        <p className="flex-1 text-ui text-ink-dim">
          Written to the global layer, named by its <code className="font-mono">name:</code>.
        </p>
        <DefValidityBadge validity={validity} />
      </div>
      <div className="mt-3">
        <CodeEditor value={raw} onChange={setRaw} language="yaml" ariaLabel="New agentiflow definition" />
      </div>
      {validity && !validity.ok && (
        <p className="mt-2 break-words font-mono text-note text-err">{validity.error}</p>
      )}
      {error && (
        <p role="alert" className="mt-2 text-ui font-medium text-err">
          {error}
        </p>
      )}
      <div className="mt-4 flex items-center justify-end gap-2">
        <Button variant="secondary" onClick={onClose} disabled={creating}>
          Cancel
        </Button>
        <Button variant="solid" onClick={() => void create()} disabled={creating || validity?.ok === false}>
          {creating ? 'Creating…' : 'Create'}
        </Button>
      </div>
    </DialogFrame>
  );
}
