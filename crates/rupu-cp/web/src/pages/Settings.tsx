// Settings — global CP configuration. Fetches the resolved `GET /api/config`
// view (effective values + per-key provenance) and renders it as a tabbed
// form: General / Providers / Autoflow / SCM-Issues / Pricing / CP-Runtime.
// Each typed field shows its resolved value, a provenance badge (which layer
// won: global/project/env/default), and a lock toggle that adds/removes the
// key from the GLOBAL `[policy].lock` enforced-key list (`PUT
// /api/config/policy`, applied immediately — independent of the Save button).
// Edited fields accumulate in a `patch` and are submitted together via `Save
// changes` (`PUT /api/config/global`). No secret/token VALUE is ever fetched
// or rendered here — the CP's own bearer token only ever appears as
// `status.token_set: bool` (Bearer token row on the CP-Runtime tab), and it is
// never editable from this form.
//
// The Raw / Policy / Runtime-status tabs are scaffolded as placeholders —
// Task 5 fills them in (raw TOML editor, lock-list editor, richer runtime
// detail). This page owns General/Providers/Autoflow/SCM-Issues/Pricing/
// CP-Runtime only.

import { useEffect, useState } from 'react';
import {
  Activity,
  Cpu,
  DollarSign,
  FileCode,
  GitBranch,
  Lock as LockIcon,
  Server,
  SlidersHorizontal,
  Workflow,
} from 'lucide-react';
import { api, ApiError, type ConfigRuntimeStatus, type ConfigView, type KeyProvenance } from '../lib/api';
import { TabBar, TabButton } from '../components/TabBar';
import { Chip } from '../components/ui/Chip';
import { Button } from '../components/ui/Button';

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/** Read a dotted-path value out of a loosely-typed JSON object. */
function getPath(obj: unknown, dotted: string): unknown {
  return dotted.split('.').reduce<unknown>((acc, key) => {
    if (acc && typeof acc === 'object' && key in (acc as Record<string, unknown>)) {
      return (acc as Record<string, unknown>)[key];
    }
    return undefined;
  }, obj);
}

function asRecord(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
}

const SOURCE_CLASS: Record<KeyProvenance['source'], string> = {
  global: 'bg-info-bg text-info ring-info/30',
  project: 'bg-ok-bg text-ok ring-ok/30',
  env: 'bg-warn-bg text-warn ring-warn/30',
  default: 'bg-surface text-ink-mute ring-border',
};

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-sm text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none disabled:opacity-60';
const labelCls = 'text-note font-medium text-ink-dim';

type SettingsTab =
  | 'general'
  | 'providers'
  | 'autoflow'
  | 'scm'
  | 'pricing'
  | 'cp-runtime'
  | 'raw'
  | 'policy'
  | 'runtime-status';

// ---------------------------------------------------------------------------
// ConfigField — one form row: label + provenance badge + typed input + lock
// ---------------------------------------------------------------------------

interface ConfigFieldProps {
  label: string;
  dottedKey: string;
  kind: 'text' | 'number' | 'boolean' | 'select';
  value: unknown;
  options?: string[];
  placeholder?: string;
  provenance?: KeyProvenance;
  locked: boolean;
  onChange: (key: string, value: unknown) => void;
  onToggleLock: (key: string) => void;
}

function ConfigField({
  label,
  dottedKey,
  kind,
  value,
  options,
  placeholder,
  provenance,
  locked,
  onChange,
  onToggleLock,
}: ConfigFieldProps) {
  const id = dottedKey;
  const source = provenance?.source ?? 'default';

  return (
    <div className="flex items-start gap-3 py-2.5 border-b border-border/60 last:border-b-0">
      <div className="min-w-0 flex-1">
        <div className="mb-1 flex items-center gap-2">
          <label htmlFor={id} className={labelCls}>
            {label}
          </label>
          <Chip className={SOURCE_CLASS[source]}>{source}</Chip>
        </div>

        {kind === 'boolean' ? (
          <input
            id={id}
            type="checkbox"
            checked={Boolean(value)}
            onChange={(e) => onChange(dottedKey, e.target.checked)}
            className="accent-brand-600"
          />
        ) : kind === 'select' ? (
          <select
            id={id}
            value={typeof value === 'string' ? value : ''}
            onChange={(e) => onChange(dottedKey, e.target.value === '' ? undefined : e.target.value)}
            className={fieldCls}
          >
            <option value="">—</option>
            {(options ?? []).map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        ) : (
          <input
            id={id}
            type={kind === 'number' ? 'number' : 'text'}
            value={value == null ? '' : String(value)}
            placeholder={placeholder}
            onChange={(e) => {
              if (kind === 'number') {
                const raw = e.target.value;
                onChange(dottedKey, raw.trim() === '' ? undefined : Number(raw));
              } else {
                onChange(dottedKey, e.target.value === '' ? undefined : e.target.value);
              }
            }}
            className={fieldCls}
          />
        )}
      </div>
      <button
        type="button"
        aria-label={locked ? `Unlock ${dottedKey}` : `Lock ${dottedKey}`}
        aria-pressed={locked}
        title={
          locked
            ? 'Enforced by global policy — click to unlock'
            : 'Click to lock (enforce this key globally)'
        }
        onClick={() => onToggleLock(dottedKey)}
        className="mt-5 shrink-0 text-sm leading-none hover:opacity-80"
      >
        {locked ? '\u{1F512}' /* 🔒 */ : '\u{1F513}' /* 🔓 */}
      </button>
    </div>
  );
}

function EmptyTabState({ text }: { text: string }) {
  return <p className="py-8 text-center text-sm text-ink-dim">{text}</p>;
}

// ---------------------------------------------------------------------------
// Per-tab bodies
// ---------------------------------------------------------------------------

interface TabProps {
  eff: Record<string, unknown>;
  prov: Record<string, KeyProvenance>;
  lockList: string[];
  fieldValue: (key: string) => unknown;
  onChange: (key: string, value: unknown) => void;
  onToggleLock: (key: string) => void;
}

function GeneralTab({ prov, lockList, fieldValue, onChange, onToggleLock }: TabProps) {
  return (
    <div>
      <ConfigField
        label="Default provider"
        dottedKey="default_provider"
        kind="text"
        placeholder="anthropic"
        value={fieldValue('default_provider')}
        provenance={prov.default_provider}
        locked={lockList.includes('default_provider')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Default model"
        dottedKey="default_model"
        kind="text"
        placeholder="claude-sonnet-4-6"
        value={fieldValue('default_model')}
        provenance={prov.default_model}
        locked={lockList.includes('default_model')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Permission mode"
        dottedKey="permission_mode"
        kind="select"
        options={['ask', 'bypass', 'readonly']}
        value={fieldValue('permission_mode')}
        provenance={prov.permission_mode}
        locked={lockList.includes('permission_mode')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Log level"
        dottedKey="log_level"
        kind="select"
        options={['trace', 'debug', 'info', 'warn', 'error']}
        value={fieldValue('log_level')}
        provenance={prov.log_level}
        locked={lockList.includes('log_level')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
    </div>
  );
}

const PROVIDER_FIELDS: Array<{ key: string; label: string; kind: ConfigFieldProps['kind'] }> = [
  { key: 'kind', label: 'Kind', kind: 'text' },
  { key: 'base_url', label: 'Base URL', kind: 'text' },
  { key: 'default_model', label: 'Default model', kind: 'text' },
  { key: 'org_id', label: 'Org id', kind: 'text' },
  { key: 'region', label: 'Region', kind: 'text' },
  { key: 'timeout_ms', label: 'Timeout (ms)', kind: 'number' },
  { key: 'max_retries', label: 'Max retries', kind: 'number' },
  { key: 'max_concurrency', label: 'Max concurrency', kind: 'number' },
  { key: 'stream', label: 'Stream', kind: 'boolean' },
];

function ProvidersTab({ eff, prov, lockList, fieldValue, onChange, onToggleLock }: TabProps) {
  const providers = asRecord(getPath(eff, 'providers'));
  const names = Object.keys(providers).sort();
  if (names.length === 0) {
    return (
      <EmptyTabState text="No custom provider overrides configured. Entries appear here once added under [providers.<name>] in config.toml." />
    );
  }
  return (
    <div className="space-y-6">
      {names.map((name) => (
        <div key={name}>
          <h3 className="mb-2 text-sm font-semibold text-ink">{name}</h3>
          {PROVIDER_FIELDS.map((f) => {
            const dottedKey = `providers.${name}.${f.key}`;
            return (
              <ConfigField
                key={dottedKey}
                label={f.label}
                dottedKey={dottedKey}
                kind={f.kind}
                value={fieldValue(dottedKey)}
                provenance={prov[dottedKey]}
                locked={lockList.includes(dottedKey)}
                onChange={onChange}
                onToggleLock={onToggleLock}
              />
            );
          })}
        </div>
      ))}
    </div>
  );
}

function AutoflowTab({ prov, lockList, fieldValue, onChange, onToggleLock }: TabProps) {
  return (
    <div>
      <ConfigField
        label="Enabled"
        dottedKey="autoflow.enabled"
        kind="boolean"
        value={fieldValue('autoflow.enabled')}
        provenance={prov['autoflow.enabled']}
        locked={lockList.includes('autoflow.enabled')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Repo"
        dottedKey="autoflow.repo"
        kind="text"
        placeholder="github:owner/repo"
        value={fieldValue('autoflow.repo')}
        provenance={prov['autoflow.repo']}
        locked={lockList.includes('autoflow.repo')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Checkout"
        dottedKey="autoflow.checkout"
        kind="select"
        options={['worktree', 'in_place']}
        value={fieldValue('autoflow.checkout')}
        provenance={prov['autoflow.checkout']}
        locked={lockList.includes('autoflow.checkout')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Worktree root"
        dottedKey="autoflow.worktree_root"
        kind="text"
        placeholder="~/.rupu/autoflows/worktrees"
        value={fieldValue('autoflow.worktree_root')}
        provenance={prov['autoflow.worktree_root']}
        locked={lockList.includes('autoflow.worktree_root')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Permission mode"
        dottedKey="autoflow.permission_mode"
        kind="select"
        options={['ask', 'bypass', 'readonly']}
        value={fieldValue('autoflow.permission_mode')}
        provenance={prov['autoflow.permission_mode']}
        locked={lockList.includes('autoflow.permission_mode')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Strict templates"
        dottedKey="autoflow.strict_templates"
        kind="boolean"
        value={fieldValue('autoflow.strict_templates')}
        provenance={prov['autoflow.strict_templates']}
        locked={lockList.includes('autoflow.strict_templates')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Max active"
        dottedKey="autoflow.max_active"
        kind="number"
        value={fieldValue('autoflow.max_active')}
        provenance={prov['autoflow.max_active']}
        locked={lockList.includes('autoflow.max_active')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Cleanup after"
        dottedKey="autoflow.cleanup_after"
        kind="text"
        placeholder="7d"
        value={fieldValue('autoflow.cleanup_after')}
        provenance={prov['autoflow.cleanup_after']}
        locked={lockList.includes('autoflow.cleanup_after')}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
    </div>
  );
}

const SCM_PLATFORM_FIELDS: Array<{ key: string; label: string; kind: ConfigFieldProps['kind'] }> = [
  { key: 'base_url', label: 'Base URL', kind: 'text' },
  { key: 'timeout_ms', label: 'Timeout (ms)', kind: 'number' },
  { key: 'max_concurrency', label: 'Max concurrency', kind: 'number' },
  { key: 'clone_protocol', label: 'Clone protocol', kind: 'select' },
];

function ScmTab({ eff, prov, lockList, fieldValue, onChange, onToggleLock }: TabProps) {
  const scm = asRecord(getPath(eff, 'scm'));
  const platforms = Object.keys(scm).filter((k) => k !== 'default').sort();

  return (
    <div className="space-y-6">
      <div>
        <h3 className="mb-2 text-sm font-semibold text-ink">Default SCM</h3>
        <ConfigField
          label="Platform"
          dottedKey="scm.default.platform"
          kind="select"
          options={['github', 'gitlab']}
          value={fieldValue('scm.default.platform')}
          provenance={prov['scm.default.platform']}
          locked={lockList.includes('scm.default.platform')}
          onChange={onChange}
          onToggleLock={onToggleLock}
        />
        <ConfigField
          label="Owner"
          dottedKey="scm.default.owner"
          kind="text"
          value={fieldValue('scm.default.owner')}
          provenance={prov['scm.default.owner']}
          locked={lockList.includes('scm.default.owner')}
          onChange={onChange}
          onToggleLock={onToggleLock}
        />
        <ConfigField
          label="Repo"
          dottedKey="scm.default.repo"
          kind="text"
          value={fieldValue('scm.default.repo')}
          provenance={prov['scm.default.repo']}
          locked={lockList.includes('scm.default.repo')}
          onChange={onChange}
          onToggleLock={onToggleLock}
        />
      </div>

      <div>
        <h3 className="mb-2 text-sm font-semibold text-ink">Default issue tracker</h3>
        <ConfigField
          label="Tracker"
          dottedKey="issues.default.tracker"
          kind="text"
          placeholder="github | gitlab | linear | jira"
          value={fieldValue('issues.default.tracker')}
          provenance={prov['issues.default.tracker']}
          locked={lockList.includes('issues.default.tracker')}
          onChange={onChange}
          onToggleLock={onToggleLock}
        />
        <ConfigField
          label="Project"
          dottedKey="issues.default.project"
          kind="text"
          value={fieldValue('issues.default.project')}
          provenance={prov['issues.default.project']}
          locked={lockList.includes('issues.default.project')}
          onChange={onChange}
          onToggleLock={onToggleLock}
        />
      </div>

      {platforms.length > 0 && (
        <div>
          <h3 className="mb-2 text-sm font-semibold text-ink">Per-platform overrides</h3>
          {platforms.map((platform) => (
            <div key={platform} className="mb-4">
              <h4 className="mb-1 text-note font-semibold uppercase tracking-wide text-ink-mute">
                {platform}
              </h4>
              {SCM_PLATFORM_FIELDS.map((f) => {
                const dottedKey = `scm.${platform}.${f.key}`;
                return (
                  <ConfigField
                    key={dottedKey}
                    label={f.label}
                    dottedKey={dottedKey}
                    kind={f.kind}
                    options={f.key === 'clone_protocol' ? ['https', 'ssh'] : undefined}
                    value={fieldValue(dottedKey)}
                    provenance={prov[dottedKey]}
                    locked={lockList.includes(dottedKey)}
                    onChange={onChange}
                    onToggleLock={onToggleLock}
                  />
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function PricingRow({
  dottedPrefix,
  prov,
  lockList,
  fieldValue,
  onChange,
  onToggleLock,
}: {
  dottedPrefix: string;
} & Omit<TabProps, 'eff'>) {
  return (
    <>
      <ConfigField
        label="Input $/Mtok"
        dottedKey={`${dottedPrefix}.input_per_mtok`}
        kind="number"
        value={fieldValue(`${dottedPrefix}.input_per_mtok`)}
        provenance={prov[`${dottedPrefix}.input_per_mtok`]}
        locked={lockList.includes(`${dottedPrefix}.input_per_mtok`)}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Output $/Mtok"
        dottedKey={`${dottedPrefix}.output_per_mtok`}
        kind="number"
        value={fieldValue(`${dottedPrefix}.output_per_mtok`)}
        provenance={prov[`${dottedPrefix}.output_per_mtok`]}
        locked={lockList.includes(`${dottedPrefix}.output_per_mtok`)}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
      <ConfigField
        label="Cached input $/Mtok"
        dottedKey={`${dottedPrefix}.cached_input_per_mtok`}
        kind="number"
        value={fieldValue(`${dottedPrefix}.cached_input_per_mtok`)}
        provenance={prov[`${dottedPrefix}.cached_input_per_mtok`]}
        locked={lockList.includes(`${dottedPrefix}.cached_input_per_mtok`)}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />
    </>
  );
}

function PricingTab({ eff, prov, lockList, fieldValue, onChange, onToggleLock }: TabProps) {
  const pricing = asRecord(getPath(eff, 'pricing'));
  const agents = asRecord(pricing.agents);
  const providerNames = Object.keys(pricing)
    .filter((k) => k !== 'agents')
    .sort();

  const hasAny = providerNames.length > 0 || Object.keys(agents).length > 0;
  if (!hasAny) {
    return (
      <EmptyTabState text="No custom pricing overrides configured. Built-in defaults apply for known models." />
    );
  }

  return (
    <div className="space-y-6">
      {providerNames.map((provider) => {
        const models = asRecord(pricing[provider]);
        const modelNames = Object.keys(models).sort();
        if (modelNames.length === 0) return null;
        return (
          <div key={provider}>
            <h3 className="mb-2 text-sm font-semibold text-ink">{provider}</h3>
            {modelNames.map((model) => (
              <div key={model} className="mb-3">
                <h4 className="mb-1 text-note font-semibold text-ink-mute">{model}</h4>
                <PricingRow
                  dottedPrefix={`pricing.${provider}.${model}`}
                  prov={prov}
                  lockList={lockList}
                  fieldValue={fieldValue}
                  onChange={onChange}
                  onToggleLock={onToggleLock}
                />
              </div>
            ))}
          </div>
        );
      })}

      {Object.keys(agents).length > 0 && (
        <div>
          <h3 className="mb-2 text-sm font-semibold text-ink">Agent fallback pricing</h3>
          {Object.keys(agents)
            .sort()
            .map((agent) => (
              <div key={agent} className="mb-3">
                <h4 className="mb-1 text-note font-semibold text-ink-mute">{agent}</h4>
                <PricingRow
                  dottedPrefix={`pricing.agents.${agent}`}
                  prov={prov}
                  lockList={lockList}
                  fieldValue={fieldValue}
                  onChange={onChange}
                  onToggleLock={onToggleLock}
                />
              </div>
            ))}
        </div>
      )}
    </div>
  );
}

function CpRuntimeTab({
  prov,
  lockList,
  fieldValue,
  onChange,
  onToggleLock,
  status,
}: Omit<TabProps, 'eff'> & { status: ConfigRuntimeStatus }) {
  return (
    <div className="space-y-6">
      <div>
        <ConfigField
          label="Max workspace bytes"
          dottedKey="cp.max_workspace_bytes"
          kind="number"
          placeholder="default (10485760)"
          value={fieldValue('cp.max_workspace_bytes')}
          provenance={prov['cp.max_workspace_bytes']}
          locked={lockList.includes('cp.max_workspace_bytes')}
          onChange={onChange}
          onToggleLock={onToggleLock}
        />
      </div>

      <div className="border-t border-border pt-4">
        <h3 className="mb-3 text-sm font-semibold text-ink">Runtime status</h3>
        <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div>
            <dt className="text-note font-medium uppercase tracking-wide text-ink-dim">Bind address</dt>
            <dd className="mt-0.5 font-mono text-sm text-ink">{status.bind}</dd>
          </div>
          <div>
            <dt className="text-note font-medium uppercase tracking-wide text-ink-dim">Bearer token</dt>
            <dd className="mt-0.5 text-sm text-ink">
              {status.token_set ? '••• set' : 'not configured'}
            </dd>
          </div>
          {status.restart_required_keys.length > 0 && (
            <div className="sm:col-span-2">
              <dt className="text-note font-medium uppercase tracking-wide text-ink-dim">
                Requires restart to apply
              </dt>
              <dd className="mt-0.5 font-mono text-sm text-ink-dim">
                {status.restart_required_keys.join(', ')}
              </dd>
            </div>
          )}
        </dl>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function Settings() {
  const [configView, setConfigView] = useState<ConfigView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pendingPatch, setPendingPatch] = useState<Record<string, unknown>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveInfo, setSaveInfo] = useState<string | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [lockList, setLockList] = useState<string[]>([]);
  const [lockError, setLockError] = useState<string | null>(null);
  const [tab, setTab] = useState<SettingsTab>('general');

  function reload(): Promise<void> {
    return api
      .getConfig()
      .then((data) => {
        setConfigView(data);
        const locks = getPath(data.effective, 'policy.lock');
        setLockList(Array.isArray(locks) ? (locks.filter((l): l is string => typeof l === 'string')) : []);
        setLoadError(null);
      })
      .catch((e: unknown) => {
        setLoadError(e instanceof Error ? e.message : 'Failed to load config');
      });
  }

  useEffect(() => {
    void reload();
  }, []);

  function fieldValue(key: string): unknown {
    if (Object.prototype.hasOwnProperty.call(pendingPatch, key)) return pendingPatch[key];
    return configView ? getPath(configView.effective, key) : undefined;
  }

  function handleFieldChange(key: string, value: unknown) {
    // Clearing a field (select "—" / emptying a text or number input) yields
    // `undefined`. The write path (`PUT /api/config/global`) has no way to
    // UNSET a key in v1 — sending `null` 400s server-side, and
    // `JSON.stringify` silently drops `undefined`-valued keys, which would
    // otherwise produce an empty (or partial) patch that "succeeds" as a
    // silent no-op. So an `undefined` transition is never staged as an edit;
    // drop the key from the pending patch instead, which reverts the field's
    // displayed value back to its current resolved value (see `fieldValue`).
    // Unsetting a key is only possible via the Raw TOML tab.
    setSaveInfo(null);
    setPendingPatch((prev) => {
      if (value === undefined) {
        if (!(key in prev)) return prev;
        const next = { ...prev };
        delete next[key];
        return next;
      }
      return { ...prev, [key]: value };
    });
  }

  async function handleToggleLock(key: string) {
    const prev = lockList;
    const next = prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key];
    setLockList(next);
    setLockError(null);
    try {
      await api.putPolicy(next);
      await reload();
    } catch (e: unknown) {
      setLockList(prev);
      setLockError(e instanceof Error ? e.message : 'Failed to update lock policy');
    }
  }

  async function handleSave() {
    if (Object.keys(pendingPatch).length === 0) return;
    // Defensive: `handleFieldChange` already keeps `undefined`-valued entries
    // out of `pendingPatch`, but never send one to the wire even if that
    // invariant is somehow violated — `JSON.stringify` drops `undefined`
    // keys, which would otherwise turn into a silently-successful no-op patch.
    const effectivePatch = Object.fromEntries(
      Object.entries(pendingPatch).filter(([, v]) => v !== undefined),
    );
    if (Object.keys(effectivePatch).length === 0) {
      setSaveError(null);
      setSaveInfo('No changes to save.');
      setPendingPatch({});
      return;
    }
    setSaving(true);
    setSaveError(null);
    setSaveInfo(null);
    setReadOnly(false);
    try {
      await api.putGlobalConfig({ patch: effectivePatch });
      setPendingPatch({});
      await reload();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) {
        setReadOnly(true);
      } else {
        setSaveError(e instanceof Error ? e.message : 'Failed to save config');
      }
    } finally {
      setSaving(false);
    }
  }

  if (loadError) {
    return (
      <div className="p-8">
        <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">{loadError}</div>
      </div>
    );
  }

  if (configView === null) {
    return (
      <div className="p-8">
        <div className="text-sm text-ink-dim">Loading settings…</div>
      </div>
    );
  }

  const eff = configView.effective;
  const prov = configView.provenance;
  const dirtyCount = Object.keys(pendingPatch).length;

  return (
    <div className="p-8 space-y-6">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Settings</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Global rupu configuration, resolved from{' '}
            <span className="font-mono">~/.rupu/config.toml</span>.
          </p>
        </div>
        <div className="flex items-center gap-3">
          {dirtyCount > 0 && (
            <span className="text-note text-ink-dim">
              {dirtyCount} unsaved change{dirtyCount === 1 ? '' : 's'}
            </span>
          )}
          <Button onClick={() => void handleSave()} disabled={dirtyCount === 0 || saving}>
            {saving ? 'Saving…' : 'Save changes'}
          </Button>
        </div>
      </header>

      {readOnly && (
        <div className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-sm text-warn">
          This is a read-only deploy — editing config requires <code className="font-mono">rupu cp serve</code>.
        </div>
      )}
      {saveError && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {saveError}
        </div>
      )}
      {saveInfo && (
        <div className="rounded-lg border border-border bg-surface px-4 py-3 text-sm text-ink-dim">
          {saveInfo}
        </div>
      )}
      {lockError && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {lockError}
        </div>
      )}

      <div className="-mx-8">
        <TabBar>
          <TabButton active={tab === 'general'} onClick={() => setTab('general')} icon={SlidersHorizontal} label="General" />
          <TabButton active={tab === 'providers'} onClick={() => setTab('providers')} icon={Cpu} label="Providers" />
          <TabButton active={tab === 'autoflow'} onClick={() => setTab('autoflow')} icon={Workflow} label="Autoflow" />
          <TabButton active={tab === 'scm'} onClick={() => setTab('scm')} icon={GitBranch} label="SCM / Issues" />
          <TabButton active={tab === 'pricing'} onClick={() => setTab('pricing')} icon={DollarSign} label="Pricing" />
          <TabButton active={tab === 'cp-runtime'} onClick={() => setTab('cp-runtime')} icon={Server} label="CP-Runtime" />
          <TabButton active={tab === 'raw'} onClick={() => setTab('raw')} icon={FileCode} label="Raw" />
          <TabButton active={tab === 'policy'} onClick={() => setTab('policy')} icon={LockIcon} label="Policy" />
          <TabButton
            active={tab === 'runtime-status'}
            onClick={() => setTab('runtime-status')}
            icon={Activity}
            label="Runtime status"
          />
        </TabBar>
      </div>

      <section className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
        {tab === 'general' && (
          <GeneralTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'providers' && (
          <ProvidersTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'autoflow' && (
          <AutoflowTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'scm' && (
          <ScmTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'pricing' && (
          <PricingTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'cp-runtime' && (
          <CpRuntimeTab
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
            status={configView.status}
          />
        )}
        {(tab === 'raw' || tab === 'policy' || tab === 'runtime-status') && (
          <div className="py-10 text-center text-sm text-ink-dim">
            {tab === 'raw' && 'Raw TOML editor — coming in Task 5.'}
            {tab === 'policy' && 'Policy lock-list editor — coming in Task 5.'}
            {tab === 'runtime-status' && 'Runtime status detail — coming in Task 5.'}
          </div>
        )}
      </section>
    </div>
  );
}
