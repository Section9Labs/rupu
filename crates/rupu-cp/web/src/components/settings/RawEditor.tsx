// Raw TOML editor tab — extracted from components/ConfigEditor.tsx (T4
// redesign). A read-only "as written" reference (syntax highlighted) next to
// an editable draft textarea, used by both the global Settings Raw tab and
// the project Config tab's Raw sub-tab. Visual layout only — the dirty /
// reset / save wiring (and what gets posted to the config API) is unchanged.

import type { ReactNode } from 'react';
import CodeHighlight from '../CodeHighlight';
import { Button } from '../ui/Button';

const RAW_TEXTAREA_CLS =
  'h-80 w-full resize-y rounded-lg border border-border bg-panel px-3 py-2 font-mono text-ui ' +
  'leading-relaxed text-ink shadow-sm placeholder:text-ink-mute transition-colors ' +
  'focus:border-brand-500 focus:outline-none focus:ring-2 focus:ring-brand-500/20';

export interface RawTabProps {
  heading: ReactNode;
  savedRaw: string;
  draft: string;
  onChangeDraft: (v: string) => void;
  onSave: () => void;
  saving: boolean;
  error: string | null;
  emptyPlaceholder?: string;
}

export function RawTab({
  heading,
  savedRaw,
  draft,
  onChangeDraft,
  onSave,
  saving,
  error,
  emptyPlaceholder,
}: RawTabProps) {
  const dirty = draft !== savedRaw;
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div className="min-w-0">
          <div className="mb-2 flex items-baseline justify-between gap-2">
            <h3 className="text-sm font-semibold text-ink">{heading}</h3>
            <span className="shrink-0 text-note text-ink-mute">reference · read-only</span>
          </div>
          <CodeHighlight code={savedRaw || (emptyPlaceholder ?? '# empty\n')} language="toml" />
        </div>

        <div className="min-w-0">
          <div className="mb-2 flex items-baseline justify-between gap-2">
            <label htmlFor="raw-toml-editor" className="text-sm font-medium text-ink">
              Edit raw TOML
            </label>
            {dirty && (
              <span className="inline-flex shrink-0 items-center gap-1 text-note font-medium text-warn">
                <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-warn" />
                unsaved edits
              </span>
            )}
          </div>
          <textarea
            id="raw-toml-editor"
            value={draft}
            onChange={(e) => onChangeDraft(e.target.value)}
            spellCheck={false}
            className={RAW_TEXTAREA_CLS}
          />
        </div>
      </div>

      {error && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      <div className="flex items-center justify-end gap-2 border-t border-border pt-3">
        <Button variant="secondary" onClick={() => onChangeDraft(savedRaw)} disabled={saving || !dirty}>
          Reset
        </Button>
        <Button onClick={onSave} disabled={saving || !dirty}>
          {saving ? 'Saving…' : 'Save'}
        </Button>
      </div>
    </div>
  );
}
