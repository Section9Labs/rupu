// Raw TOML editor tab — used by both the global Settings Raw tab and the
// project Config tab's Raw sub-tab. Mirrors the AgentDetail / WorkflowDetail
// "Definition" pattern: a single read-only `CodeHighlight` view by default,
// with an Edit button that swaps in a `CodeEditor` + Cancel/Save. There is no
// side-by-side reference-next-to-textarea layout — view and edit are the same
// surface, toggled by `editing` state (self-contained here, like AgentDetail).

import { useState } from 'react';
import type { ReactNode } from 'react';
import { Pencil } from 'lucide-react';
import CodeHighlight from '../CodeHighlight';
import CodeEditor from '../CodeEditor';
import { Button } from '../ui/Button';

export interface RawTabProps {
  heading: ReactNode;
  savedRaw: string;
  /** Persist `draft`. On failure, reject/throw (after surfacing the failure
   *  via `saveError`) so the tab stays in edit mode instead of silently
   *  returning to the read-only view. */
  onSave: (draft: string) => void | Promise<void>;
  saving: boolean;
  saveError?: string | null;
  emptyPlaceholder?: string;
  ariaLabel?: string;
}

export function RawTab({
  heading,
  savedRaw,
  onSave,
  saving,
  saveError,
  emptyPlaceholder,
  ariaLabel = 'Edit raw TOML',
}: RawTabProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');

  function startEdit() {
    setDraft(savedRaw);
    setEditing(true);
  }

  function cancelEdit() {
    setEditing(false);
  }

  async function handleSave() {
    if (saving) return;
    try {
      await onSave(draft);
      setEditing(false);
    } catch {
      // Stay in edit mode so the operator can fix it — the caller is
      // expected to also surface the failure via `saveError`.
    }
  }

  return (
    <div className="space-y-4">
      <div className="mb-2 flex items-center justify-between gap-2">
        <h3 className="text-sm font-semibold text-ink">{heading}</h3>
        {!editing && (
          <Button variant="secondary" size="sm" onClick={startEdit} aria-label="Edit" className="gap-1.5">
            <Pencil size={13} />
            Edit
          </Button>
        )}
      </div>

      {editing ? (
        <div className="space-y-3">
          <CodeEditor value={draft} onChange={setDraft} language="toml" ariaLabel={ariaLabel} />
          {saveError && (
            <p role="alert" className="text-ui font-medium text-err">
              {saveError}
            </p>
          )}
          <div className="flex items-center justify-end gap-2">
            <Button variant="secondary" onClick={cancelEdit} disabled={saving}>
              Cancel
            </Button>
            <Button onClick={() => void handleSave()} disabled={saving || draft === savedRaw}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </div>
      ) : (
        <CodeHighlight code={savedRaw || (emptyPlaceholder ?? '# empty\n')} language="toml" />
      )}
    </div>
  );
}
