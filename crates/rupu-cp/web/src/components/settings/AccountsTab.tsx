// Settings → Accounts (Plan 3b mock §11): will my runs authenticate? One
// table of every provider and SCM account (`GET /api/auth/status`, with
// `GET /api/scm/accounts` saying which are repo accounts and where they
// point), each with its state. No answer carries a credential, and this tab
// shows nothing beyond what they return. An SSO token that renews itself
// reads as ok; a needs-login row shows the CLI command to run, because
// logging in stays human-only. Below, the global `[[scm.rules]]` with Add
// rule (`POST /api/scm/rules`); a new rule shows once the next read lists it.

import { useCallback, useEffect, useId, useState, type FormEvent } from 'react';
import { apiErrorMessage } from '../../lib/api';
import {
  accountModes,
  admin,
  loginCommand,
  ruleLines,
  type AccountAuth,
  type ScmAccountRow,
  type ScmRule,
} from '../../lib/api3b/admin';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Select } from '../ui/Select';
import { Skeleton } from '../ui/Skeleton';
import { StatusChip } from '../ui/StatusChip';
import { FieldGroup } from './ConfigField';

const ISSUE_TRACKERS = new Set(['linear', 'jira']);

function host(url: string | null, kind: string): string {
  if (url) {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  }
  return kind === 'gitlab' ? 'gitlab.com' : kind === 'github' ? 'github.com' : kind;
}

function kindLabel(a: AccountAuth, scm: ScmAccountRow | undefined): string {
  if (scm) return `scm · ${host(scm.base_url, scm.kind)}`;
  if (a.kind && ISSUE_TRACKERS.has(a.kind)) return `issues · ${a.kind}`;
  return a.kind ? `provider · ${a.kind}` : 'provider';
}

function StateCell({ a }: { a: AccountAuth }) {
  if (a.state === 'ok') {
    const renews = !a.api_key && a.sso?.refreshable;
    return (
      <span className="flex flex-wrap items-center gap-2">
        <StatusChip tone="completed" dot>
          {renews ? 'ok · refreshes itself' : 'ok'}
        </StatusChip>
        {a.sso && !renews && <span className="text-ink-mute">{a.sso.summary}</span>}
      </span>
    );
  }
  return (
    <span className="flex flex-wrap items-center gap-2">
      <StatusChip tone={a.state === 'needs_login' ? 'awaiting_approval' : 'neutral'} dot={a.state === 'needs_login'}>
        {a.state === 'needs_login' ? 'needs login' : 'no credential'}
      </StatusChip>
      <code className="font-mono text-ink-dim">{loginCommand(a.account, a.sso !== null)}</code>
    </span>
  );
}

const thCls = 'px-3 py-2 text-left text-meta font-semibold uppercase tracking-wide text-ink-mute';

function AccountRows({ rows, scm }: { rows: AccountAuth[]; scm: Map<string, ScmAccountRow> }) {
  return (
    <>
      {rows.map((a) => (
        <tr key={a.account} aria-label={a.account}>
          <td className="px-3 py-2 font-mono text-ink">{a.account}</td>
          <td className="px-3 py-2 text-ink-mute">{kindLabel(a, scm.get(a.account))}</td>
          <td className="px-3 py-2 text-ink-mute">{accountModes(a)}</td>
          <td className="px-3 py-2">
            <StateCell a={a} />
          </td>
        </tr>
      ))}
    </>
  );
}

function AddRuleForm({ accounts, onAdded, onCancel }: { accounts: string[]; onAdded: (warning?: string) => void; onCancel: () => void }) {
  const ids = { on: useId(), pattern: useId(), account: useId() };
  const [on, setOn] = useState<'owner' | 'path'>('owner');
  const [pattern, setPattern] = useState('');
  const [account, setAccount] = useState(accounts[0] ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const reply = await admin.addScmRule({ [on]: pattern.trim(), account });
      onAdded(reply.warning);
    } catch (err) {
      setError(apiErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} className="mt-3 rounded-lg border border-border bg-panel p-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-[8rem_1fr_12rem]">
        <div>
          <label htmlFor={ids.on} className="mb-1 block text-note font-semibold text-ink">
            Match on
          </label>
          <Select id={ids.on} value={on} onChange={(e) => setOn(e.target.value as 'owner' | 'path')}>
            <option value="owner">owner</option>
            <option value="path">path</option>
          </Select>
        </div>
        <div>
          <label htmlFor={ids.pattern} className="mb-1 block text-note font-semibold text-ink">
            Pattern
          </label>
          <Input
            id={ids.pattern}
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
            placeholder={on === 'owner' ? 'acme/*' : '~/Code/work/*'}
            className="max-w-none font-mono"
            spellCheck={false}
          />
        </div>
        <div>
          <label htmlFor={ids.account} className="mb-1 block text-note font-semibold text-ink">
            Account
          </label>
          <Select id={ids.account} value={account} onChange={(e) => setAccount(e.target.value)}>
            {accounts.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </Select>
        </div>
      </div>
      <p className="mt-2 text-note text-ink-mute">
        {on === 'owner'
          ? 'An owner glob, matched against the repo owner (works for daemons).'
          : 'A path glob, matched against the directory a run starts in.'}{' '}
        Written to the global <span className="font-mono">config.toml</span>, after the existing rules.
      </p>
      {error && (
        <p role="alert" className="mt-2 text-note text-err">
          {error}
        </p>
      )}
      <div className="mt-3 flex justify-end gap-2">
        <Button variant="secondary" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" variant="solid" disabled={busy || !pattern.trim() || !account}>
          {busy ? 'Saving…' : 'Save rule'}
        </Button>
      </div>
    </form>
  );
}

export function AccountsTab() {
  const [auth, setAuth] = useState<AccountAuth[] | null>(null);
  const [scm, setScm] = useState<ScmAccountRow[]>([]);
  const [rules, setRules] = useState<ScmRule[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAbsent, setShowAbsent] = useState(false);
  const [adding, setAdding] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const loadRules = useCallback(async () => {
    setRules(await admin.getScmRules());
  }, []);

  useEffect(() => {
    let live = true;
    Promise.all([admin.getAuthStatus(), admin.getScmAccounts(), admin.getScmRules()])
      .then(([a, s, r]) => {
        if (!live) return;
        setAuth(a);
        setScm(s);
        setRules(r);
      })
      .catch((e: unknown) => live && setError(apiErrorMessage(e)));
    return () => {
      live = false;
    };
  }, []);

  async function added(warning?: string) {
    setAdding(false);
    setNotice(warning ?? null);
    try {
      await loadRules();
    } catch (e) {
      setNotice(`The rule was written, but the rules could not be read back: ${apiErrorMessage(e)}`);
    }
  }

  if (error) {
    return (
      <p role="alert" className="text-sm text-err">
        {error}
      </p>
    );
  }
  if (!auth || !rules) {
    return (
      <div role="status" aria-label="Loading accounts…" className="space-y-3 py-2">
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-2/3" />
        <Skeleton className="h-4 w-2/5" />
      </div>
    );
  }

  const scmByName = new Map(scm.map((s) => [s.account, s]));
  const present = auth.filter((a) => a.state !== 'absent');
  const absent = auth.filter((a) => a.state === 'absent');
  const accountNames = [...new Set([...scm.map((s) => s.account), 'github', 'gitlab'])];

  return (
    <div className="space-y-5">
      <FieldGroup
        title="Accounts"
        description="Whether each account can authenticate on this host. No credential is ever shown; log in with the CLI."
      >
        <table className="mt-2 w-full text-note">
          <thead className="bg-surface">
            <tr>
              <th className={thCls}>Account</th>
              <th className={thCls}>Kind</th>
              <th className={thCls}>Mode</th>
              <th className={thCls}>State</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            <AccountRows rows={present} scm={scmByName} />
            {showAbsent && <AccountRows rows={absent} scm={scmByName} />}
          </tbody>
        </table>
        {absent.length > 0 && (
          <div className="pt-2">
            <button
              type="button"
              onClick={() => setShowAbsent((v) => !v)}
              aria-expanded={showAbsent}
              className="text-note text-brand-600 hover:underline"
            >
              {showAbsent
                ? 'Hide accounts with no credential'
                : `Show ${absent.length} account${absent.length === 1 ? '' : 's'} with no credential`}
            </button>
          </div>
        )}
      </FieldGroup>

      <FieldGroup
        title={
          <span className="flex items-center">
            Binding rules
            <span className="flex-1" />
            {!adding && (
              <Button variant="secondary" size="sm" onClick={() => setAdding(true)}>
                Add rule
              </Button>
            )}
          </span>
        }
        description="Which SCM account a repo uses: the first matching rule wins."
      >
        {rules.length === 0 ? (
          <p className="py-3 text-sm text-ink-dim">No rules: every repo uses its platform's default account.</p>
        ) : (
          <ol className="mt-2 rounded-lg border border-border bg-surface px-3 py-2 font-mono text-note">
            {ruleLines(rules).map((r, i) => (
              <li key={i} className="flex gap-3 py-0.5">
                <span className="w-72 min-w-0 truncate text-ink" title={r.target}>
                  {r.target}
                </span>
                <span className="text-ink-mute">→</span>
                <span className="min-w-0 flex-1 text-ink-dim">{r.account}</span>
              </li>
            ))}
          </ol>
        )}
        {notice && <p className="pt-2 text-note text-warn">{notice}</p>}
        {adding && <AddRuleForm accounts={accountNames} onAdded={(w) => void added(w)} onCancel={() => setAdding(false)} />}
      </FieldGroup>
    </div>
  );
}

export default AccountsTab;
