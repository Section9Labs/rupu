// RunYamlBar — the New workflow modal's "Run without saving" (Plan 3b mock
// placement table, "Launch"): launch the definition as typed, without saving
// it under a name, through `POST /api/workflows/run`. It shows a field for
// each input the YAML declares and a Mode picker; the server's refusal (a
// parse error, a remote host) shows under the button. Local only.

import { useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import yaml from 'js-yaml';
import { apiErrorMessage, type LaunchMode } from '../../lib/api';
import { launchApi } from '../../lib/api3b/launch';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Select } from '../ui/Select';

/** The input names a workflow's YAML declares, or none while it doesn't parse. */
export function declaredInputsOf(text: string): string[] {
  try {
    const doc: unknown = yaml.load(text);
    if (typeof doc !== 'object' || doc === null) return [];
    const inputs = (doc as Record<string, unknown>).inputs;
    if (typeof inputs !== 'object' || inputs === null || Array.isArray(inputs)) return [];
    return Object.keys(inputs);
  } catch {
    return [];
  }
}

export default function RunYamlBar({ yamlText, onLaunched }: { yamlText: string; onLaunched: () => void }) {
  const navigate = useNavigate();
  const names = useMemo(() => declaredInputsOf(yamlText), [yamlText]);
  const [values, setValues] = useState<Record<string, string>>({});
  const [mode, setMode] = useState<LaunchMode>('ask');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    if (running) return;
    setRunning(true);
    setError(null);
    const inputs: Record<string, string> = {};
    for (const n of names) {
      const v = values[n] ?? '';
      if (v.trim() !== '') inputs[n] = v;
    }
    try {
      const res = await launchApi.runWorkflowYaml({
        yaml: yamlText,
        mode,
        ...(Object.keys(inputs).length > 0 ? { inputs } : {}),
      });
      navigate(`/runs/${res.run_id}`);
      onLaunched();
    } catch (e: unknown) {
      setError(apiErrorMessage(e) || 'Failed to launch run');
      setRunning(false);
    }
  }

  return (
    <div className="space-y-2 rounded-lg border border-border bg-surface/40 px-3 py-2.5" data-testid="run-yaml">
      {names.length > 0 && (
        <div className="grid grid-cols-2 gap-2">
          {names.map((n) => (
            <label key={n} className="block">
              <span className="mb-0.5 block font-mono text-note text-ink-dim">{n}</span>
              <Input
                value={values[n] ?? ''}
                onChange={(e) => setValues((prev) => ({ ...prev, [n]: e.target.value }))}
                disabled={running}
                aria-label={`Input ${n}`}
                className="w-full max-w-none"
              />
            </label>
          ))}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-note text-ink-dim">Run it once, as typed, without saving it.</span>
        <span className="flex-1" />
        <Select
          value={mode}
          onChange={(e) => setMode(e.target.value as LaunchMode)}
          disabled={running}
          aria-label="Run mode"
        >
          <option value="ask">Ask</option>
          <option value="bypass">Bypass</option>
          <option value="readonly">Read-only</option>
        </Select>
        <Button variant="secondary" onClick={() => void run()} disabled={running || !yamlText.trim()}>
          {running ? 'Launching…' : 'Run without saving'}
        </Button>
      </div>
      {error && (
        <p role="alert" className="text-ui font-medium text-err">
          {error}
        </p>
      )}
    </div>
  );
}
