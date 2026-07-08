// Shared config-form building blocks — extracted from pages/Settings.tsx (T4/T5)
// so the per-project Config tab (T6) can render the SAME typed fields,
// provenance badges, and Raw TOML editor as the global Settings page.
//
// Two consumers:
//  - pages/Settings.tsx — the GLOBAL config (`GET /api/config`, no `project`).
//    Every field is editable; a lock glyph toggles the GLOBAL `[policy].lock`
//    list (independent of the per-field Save).
//  - components/project/ProjectConfigTab.tsx — the per-PROJECT config
//    (`GET /api/config?project=<id>`, `PUT /api/config/project/:id`). A field
//    whose resolved value is enforced by the global policy lock cannot be
//    edited or unlocked from here — locking is a global concept — so it
//    renders read-only with a 🔒 + "enforced by global policy" note instead
//    of an editable control. Pass `lockedReadOnly` down to opt into that mode
//    (and omit `onToggleLock`, since there is nothing to toggle).
//
// The label + provenance-badge + input + lock-toggle row, the boolean toggle
// switch, the field-group card, and the empty-tab state live in
// `components/settings/ConfigField.tsx`; the Raw TOML tab lives in
// `components/settings/RawEditor.tsx` (both T4 extractions — this file keeps
// re-exporting their public names unchanged so existing imports don't move).

import type { KeyProvenance } from '../lib/api';
import { ConfigField, FieldGroup, EmptyTabState, type ConfigFieldProps } from './settings/ConfigField';

export { EmptyTabState };
export { RawTab } from './settings/RawEditor';

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/** Read a dotted-path value out of a loosely-typed JSON object. */
export function getPath(obj: unknown, dotted: string): unknown {
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

/** Order-independent set equality for two string arrays (lock lists). */
export function sameSet(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  const sa = new Set(a);
  return b.every((k) => sa.has(k));
}

// ---------------------------------------------------------------------------
// Per-tab bodies
// ---------------------------------------------------------------------------

export interface TabProps {
  eff: Record<string, unknown>;
  prov: Record<string, KeyProvenance>;
  lockList: string[];
  fieldValue: (key: string) => unknown;
  onChange: (key: string, value: unknown) => void;
  onToggleLock?: (key: string) => void;
  lockedReadOnly?: boolean;
}

export function GeneralTab({ prov, lockList, fieldValue, onChange, onToggleLock, lockedReadOnly }: TabProps) {
  return (
    <div className="space-y-4">
      <FieldGroup title="Defaults" description="Fallbacks used when a run doesn't specify its own provider/model.">
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
        />
      </FieldGroup>

      <FieldGroup title="Runtime behavior">
        <ConfigField
          label="Permission mode"
          dottedKey="permission_mode"
          kind="select"
          options={['ask', 'bypass', 'readonly']}
          help="Controls whether tool calls need approval before they run."
          value={fieldValue('permission_mode')}
          provenance={prov.permission_mode}
          locked={lockList.includes('permission_mode')}
          onChange={onChange}
          onToggleLock={onToggleLock}
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
        />
      </FieldGroup>
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

export function ProvidersTab({ eff, prov, lockList, fieldValue, onChange, onToggleLock, lockedReadOnly }: TabProps) {
  const providers = asRecord(getPath(eff, 'providers'));
  const names = Object.keys(providers).sort();
  if (names.length === 0) {
    return (
      <EmptyTabState text="No custom provider overrides configured. Entries appear here once added under [providers.<name>] in config.toml." />
    );
  }
  return (
    <div className="space-y-4">
      {names.map((name) => (
        <FieldGroup key={name} title={name}>
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
                lockedReadOnly={lockedReadOnly}
              />
            );
          })}
        </FieldGroup>
      ))}
    </div>
  );
}

export function AutoflowTab({ prov, lockList, fieldValue, onChange, onToggleLock, lockedReadOnly }: TabProps) {
  return (
    <div className="space-y-4">
      <FieldGroup title="Repository & checkout">
        <ConfigField
          label="Enabled"
          dottedKey="autoflow.enabled"
          kind="boolean"
          value={fieldValue('autoflow.enabled')}
          provenance={prov['autoflow.enabled']}
          locked={lockList.includes('autoflow.enabled')}
          onChange={onChange}
          onToggleLock={onToggleLock}
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
        />
      </FieldGroup>

      <FieldGroup title="Execution">
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
        />
      </FieldGroup>
    </div>
  );
}

const SCM_PLATFORM_FIELDS: Array<{ key: string; label: string; kind: ConfigFieldProps['kind'] }> = [
  { key: 'base_url', label: 'Base URL', kind: 'text' },
  { key: 'timeout_ms', label: 'Timeout (ms)', kind: 'number' },
  { key: 'max_concurrency', label: 'Max concurrency', kind: 'number' },
  { key: 'clone_protocol', label: 'Clone protocol', kind: 'select' },
];

export function ScmTab({ eff, prov, lockList, fieldValue, onChange, onToggleLock, lockedReadOnly }: TabProps) {
  const scm = asRecord(getPath(eff, 'scm'));
  const platforms = Object.keys(scm).filter((k) => k !== 'default').sort();

  return (
    <div className="space-y-4">
      <FieldGroup title="Default SCM">
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
        />
      </FieldGroup>

      <FieldGroup title="Default issue tracker">
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
          lockedReadOnly={lockedReadOnly}
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
          lockedReadOnly={lockedReadOnly}
        />
      </FieldGroup>

      {platforms.length > 0 && (
        <div className="space-y-4">
          <h3 className="text-note font-semibold uppercase tracking-wide text-ink-mute">
            Per-platform overrides
          </h3>
          {platforms.map((platform) => (
            <FieldGroup key={platform} title={platform}>
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
                    lockedReadOnly={lockedReadOnly}
                  />
                );
              })}
            </FieldGroup>
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
  lockedReadOnly,
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
        lockedReadOnly={lockedReadOnly}
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
        lockedReadOnly={lockedReadOnly}
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
        lockedReadOnly={lockedReadOnly}
      />
    </>
  );
}

export function PricingTab({ eff, prov, lockList, fieldValue, onChange, onToggleLock, lockedReadOnly }: TabProps) {
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
    <div className="space-y-4">
      {providerNames.map((provider) => {
        const models = asRecord(pricing[provider]);
        const modelNames = Object.keys(models).sort();
        if (modelNames.length === 0) return null;
        return (
          <div key={provider} className="space-y-3">
            <h3 className="text-note font-semibold uppercase tracking-wide text-ink-mute">{provider}</h3>
            {modelNames.map((model) => (
              <FieldGroup key={model} title={model}>
                <PricingRow
                  dottedPrefix={`pricing.${provider}.${model}`}
                  prov={prov}
                  lockList={lockList}
                  fieldValue={fieldValue}
                  onChange={onChange}
                  onToggleLock={onToggleLock}
                  lockedReadOnly={lockedReadOnly}
                />
              </FieldGroup>
            ))}
          </div>
        );
      })}

      {Object.keys(agents).length > 0 && (
        <div className="space-y-3">
          <h3 className="text-note font-semibold uppercase tracking-wide text-ink-mute">
            Agent fallback pricing
          </h3>
          {Object.keys(agents)
            .sort()
            .map((agent) => (
              <FieldGroup key={agent} title={agent}>
                <PricingRow
                  dottedPrefix={`pricing.agents.${agent}`}
                  prov={prov}
                  lockList={lockList}
                  fieldValue={fieldValue}
                  onChange={onChange}
                  onToggleLock={onToggleLock}
                  lockedReadOnly={lockedReadOnly}
                />
              </FieldGroup>
            ))}
        </div>
      )}
    </div>
  );
}

/** Just the `cp.max_workspace_bytes` field — the Settings page's CP-Runtime
 *  tab wraps this with a read-only runtime-status block that doesn't apply
 *  per-project; the project Config tab uses this field-only tab as-is. */
export function CpFieldTab({ prov, lockList, fieldValue, onChange, onToggleLock, lockedReadOnly }: Omit<TabProps, 'eff'>) {
  return (
    <FieldGroup title="Workspace limits">
      <ConfigField
        label="Max workspace bytes"
        dottedKey="cp.max_workspace_bytes"
        kind="number"
        placeholder="default (10485760)"
        help="Upper bound on a cloned/checked-out workspace's on-disk size."
        value={fieldValue('cp.max_workspace_bytes')}
        provenance={prov['cp.max_workspace_bytes']}
        locked={lockList.includes('cp.max_workspace_bytes')}
        onChange={onChange}
        onToggleLock={onToggleLock}
        lockedReadOnly={lockedReadOnly}
      />
    </FieldGroup>
  );
}
