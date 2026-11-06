// SkillActions — a skill page's Update and Validate (Plan 3b mock §6).
// Update re-fetches the skill from its recorded source; an edited copy is
// left alone unless the operator confirms "Replace local edits…" (the
// update's `force`). Validate checks the installed copy against the format.
// The last answer shows as one line under the buttons; after an update the
// owner reads the skill again, so the page shows what is installed now.

import { useState } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { updateSkills, validateSkill, type SkillUpdateResult } from '../../lib/api3b/library';
import { Button } from '../ui/Button';

type Note =
  | { tone: 'ok'; text: string; warnings?: string[] }
  | { tone: 'warn'; text: string; canForce?: boolean }
  | { tone: 'err'; text: string };

export interface SkillActionsProps {
  name: string;
  layer: 'global' | 'project';
  /** The project of a project-layer skill. */
  wsId?: string;
  /** It has a source record, so Update can follow it. */
  hasSource: boolean;
  /** After an update replaced the installed copy. */
  onChanged: () => void;
}

function noteOf(r: SkillUpdateResult | undefined): Note {
  switch (r?.outcome) {
    case 'updated':
      return { tone: 'ok', text: `Updated from ${r.from ?? '?'} to ${r.to ?? '?'}.` };
    case 'up_to_date':
      return { tone: 'ok', text: 'Already up to date.' };
    case 'locally_modified':
      return { tone: 'warn', text: "This skill was edited where it is installed, so Update left it alone.", canForce: true };
    case 'no_record':
      return { tone: 'warn', text: "This skill has no source record (it was placed by hand), so it can't be updated." };
    case 'not_installed':
      return { tone: 'err', text: 'This skill is no longer installed.' };
    case 'failed':
      return { tone: 'err', text: r.error ?? 'The update failed.' };
    default:
      return { tone: 'err', text: 'The server gave no result for this skill.' };
  }
}

export function SkillActions({ name, layer, wsId, hasSource, onChanged }: SkillActionsProps) {
  const [busy, setBusy] = useState<'update' | 'validate' | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [confirming, setConfirming] = useState(false);
  const project = layer === 'project' ? wsId : undefined;

  async function update(force: boolean) {
    setBusy('update');
    setConfirming(false);
    setNote(null);
    try {
      const [result] = await updateSkills({ names: [name], project, force });
      setNote(noteOf(result));
      if (result?.outcome === 'updated') onChanged();
    } catch (e: unknown) {
      setNote({ tone: 'err', text: apiErrorMessage(e) });
    } finally {
      setBusy(null);
    }
  }

  async function validate() {
    setBusy('validate');
    setConfirming(false);
    setNote(null);
    try {
      const res = await validateSkill({ name, layer, wsId: project });
      const mine = res.skills.find((s) => (s.layer ?? layer) === layer) ?? res.skills[0];
      setNote(
        mine?.status === 'ok'
          ? { tone: 'ok', text: 'Valid.', warnings: mine.warnings }
          : { tone: 'err', text: `Invalid: ${mine?.error ?? 'the skill does not load'}` },
      );
    } catch (e: unknown) {
      setNote({ tone: 'err', text: apiErrorMessage(e) });
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap justify-end gap-2">
        <span className="inline-flex" title={hasSource ? undefined : 'No source record: this skill was placed by hand'}>
          <Button variant="secondary" disabled={!hasSource || busy !== null} onClick={() => update(false)}>
            {busy === 'update' ? 'Updating…' : 'Update'}
          </Button>
        </span>
        <Button variant="secondary" disabled={busy !== null} onClick={validate}>
          {busy === 'validate' ? 'Validating…' : 'Validate'}
        </Button>
      </div>

      {note && (
        <div
          role={note.tone === 'err' ? 'alert' : 'status'}
          className={
            note.tone === 'ok'
              ? 'max-w-md text-right text-note text-ok'
              : note.tone === 'warn'
                ? 'max-w-md text-right text-note text-warn'
                : 'max-w-md text-right text-note text-err'
          }
        >
          <p>{note.text}</p>
          {note.tone === 'ok' && note.warnings && note.warnings.length > 0 && (
            <ul className="mt-0.5 text-warn">
              {note.warnings.map((w) => (
                <li key={w}>{w}</li>
              ))}
            </ul>
          )}
          {note.tone === 'warn' && note.canForce && !confirming && (
            <Button variant="danger-outline" size="sm" className="mt-1.5" onClick={() => setConfirming(true)}>
              Replace local edits…
            </Button>
          )}
        </div>
      )}

      {confirming && (
        <div className="max-w-md rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-note text-err">
          <p>Replace the installed copy with the source's? The edits are lost.</p>
          <div className="mt-2 flex justify-end gap-2">
            <Button variant="secondary" size="sm" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button variant="danger" size="sm" onClick={() => update(true)}>
              Replace
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

export default SkillActions;
