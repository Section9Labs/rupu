// CustomerFormDialog — create / edit a customer. Rendered by its owner only
// while open (so every opening starts clean); the owner returns focus to its
// trigger on close. Modal: Escape / Cancel / overlay click close, Tab is
// trapped, the first field is focused on open.
//
// Closing (Escape, overlay click, Cancel) is ignored while a save is in flight;
// with unsaved edits it asks "Discard changes?" first (real buttons, inline).
//
// Create: Name (required), Slug (auto-suggested from the name until edited;
// `^[a-z0-9][a-z0-9-]{0,62}$`, and `none` is reserved — it is the filter's
// "no customer"), Contact, Notes, Color (`#rrggbb`, empty = derived from the
// slug). Edit: no slug (it is the customer's identity). A 409 (slug taken) and
// the API's other 400s are shown inline; success reloads the scope's customer
// list and calls `onSaved`.

import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { api, apiErrorMessage, ApiError, type CustomerDto, type NewCustomerBody } from '../../lib/api';
import { useCustomerScope } from '../../lib/customerScope';
import { Button } from '../ui/Button';
import { ErrorBanner } from '../ui/ErrorBanner';
import { CustomerDot } from './CustomerDot';

export interface CustomerFormDialogProps {
  mode: 'create' | 'edit';
  /** The customer being edited (edit mode). */
  initial?: CustomerDto;
  onSaved: (customer: CustomerDto) => void;
  onClose: () => void;
}

const SLUG_RE = /^[a-z0-9][a-z0-9-]{0,62}$/;
const COLOR_RE = /^#[0-9a-fA-F]{6}$/;
const RESERVED_SLUG = 'none';

/** Lowercase, runs of non-alphanumerics → `-`, trimmed, ≤ 63 chars. */
export function suggestSlug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 63)
    .replace(/-+$/g, '');
}

function slugProblem(slug: string): string | null {
  if (slug === '') return null; // empty is "not filled in yet", not an error to show
  if (slug === RESERVED_SLUG) return '“none” is reserved — it’s the filter’s “no customer”.';
  if (!SLUG_RE.test(slug)) {
    return 'Use lowercase letters, digits and dashes, starting with a letter or digit (up to 63 characters).';
  }
  return null;
}

const FOCUSABLE = 'button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])';
const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60 aria-[invalid=true]:border-err';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';
const hintCls = 'mt-1 text-note text-ink-mute';
const errCls = 'mt-1 text-note text-err';

export function CustomerFormDialog({ mode, initial, onSaved, onClose }: CustomerFormDialogProps) {
  const { reload } = useCustomerScope();
  const create = mode === 'create';
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const firstRef = useRef<HTMLInputElement>(null);

  const [name, setName] = useState(initial?.name ?? '');
  const [slug, setSlug] = useState('');
  const [slugTouched, setSlugTouched] = useState(false);
  const [contact, setContact] = useState(initial?.contact ?? '');
  const [notes, setNotes] = useState(initial?.notes ?? '');
  const [color, setColor] = useState(initial?.color ?? '');
  const [busy, setBusy] = useState(false);
  // Server-reported problems, cleared as the field they name is edited.
  const [slugServerError, setSlugServerError] = useState<string | null>(null);
  const [colorServerError, setColorServerError] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [confirmingDiscard, setConfirmingDiscard] = useState(false);
  const confirmRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    firstRef.current?.focus();
  }, []);

  const dirty =
    name !== (initial?.name ?? '') ||
    contact !== (initial?.contact ?? '') ||
    notes !== (initial?.notes ?? '') ||
    color !== (initial?.color ?? '') ||
    (create && slugTouched && slug !== '');

  // Every way of closing goes through here.
  function requestClose() {
    if (busy) return;
    if (dirty) setConfirmingDiscard(true);
    else onClose();
  }
  const requestCloseRef = useRef(requestClose);
  requestCloseRef.current = requestClose;
  const confirmingRef = useRef(false);
  confirmingRef.current = confirmingDiscard;
  useEffect(() => {
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key !== 'Escape') return;
      // Escape on the "Discard changes?" prompt means "keep editing".
      if (confirmingRef.current) setConfirmingDiscard(false);
      else requestCloseRef.current();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);
  useEffect(() => {
    if (confirmingDiscard) confirmRef.current?.querySelector('button')?.focus();
  }, [confirmingDiscard]);

  function trapTab(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== 'Tab' || !panelRef.current) return;
    const stops = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => !(el as HTMLInputElement).disabled,
    );
    if (stops.length === 0) return;
    const at = stops.indexOf(document.activeElement as HTMLElement);
    if (e.shiftKey && at <= 0) {
      e.preventDefault();
      stops[stops.length - 1].focus();
    } else if (!e.shiftKey && (at === -1 || at === stops.length - 1)) {
      e.preventDefault();
      stops[0].focus();
    }
  }

  function onName(v: string) {
    setName(v);
    if (create && !slugTouched) {
      setSlug(suggestSlug(v));
      setSlugServerError(null);
    }
  }

  const trimmedName = name.trim();
  const slugError = create ? (slugServerError ?? slugProblem(slug)) : null;
  const colorTrim = color.trim();
  const colorError =
    colorServerError ?? (colorTrim !== '' && !COLOR_RE.test(colorTrim) ? 'Use the form #rrggbb, e.g. #336699.' : null);
  const valid =
    trimmedName !== '' && colorError === null && (!create || (slug !== '' && slugProblem(slug) === null));

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (busy || !valid) return;
    setBusy(true);
    setFormError(null);
    try {
      let saved: CustomerDto;
      if (create) {
        const body: NewCustomerBody = { slug, name: trimmedName };
        if (notes.trim()) body.notes = notes.trim();
        if (contact.trim()) body.contact = contact.trim();
        if (colorTrim) body.color = colorTrim.toLowerCase();
        saved = await api.createCustomer(body);
      } else {
        // `""` clears a field; the API leaves absent fields alone.
        saved = await api.updateCustomer(initial!.slug, {
          name: trimmedName,
          notes: notes.trim(),
          contact: contact.trim(),
          color: colorTrim.toLowerCase(),
        });
      }
      reload();
      onSaved(saved);
    } catch (err: unknown) {
      const msg = apiErrorMessage(err);
      const status = err instanceof ApiError ? err.status : 0;
      if (create && (status === 409 || (status === 400 && /slug/i.test(msg)))) setSlugServerError(msg);
      else if (status === 400 && /colou?r/i.test(msg)) setColorServerError(msg);
      else setFormError(msg);
      setBusy(false);
    }
  }

  return (
    <div
      data-testid="customer-form-overlay"
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/30 p-4 pt-[8vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) requestClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={trapTab}
        className="w-full max-w-md rounded-xl border border-border bg-panel p-5 shadow-card"
      >
        <h2 id={titleId} className="text-base font-semibold text-ink">
          {create ? 'New customer' : `Edit ${initial?.name ?? 'customer'}`}
        </h2>

        <form onSubmit={onSubmit} className="mt-4 space-y-4" noValidate>
          <div>
            <label htmlFor={`${titleId}-name`} className={labelCls}>
              Name <span aria-hidden>*</span>
            </label>
            <input
              id={`${titleId}-name`}
              ref={firstRef}
              type="text"
              required
              value={name}
              onChange={(e) => onName(e.target.value)}
              disabled={busy}
              className={fieldCls}
            />
          </div>

          {create && (
            <div>
              <label htmlFor={`${titleId}-slug`} className={labelCls}>
                Slug <span aria-hidden>*</span>
              </label>
              <input
                id={`${titleId}-slug`}
                type="text"
                value={slug}
                onChange={(e) => {
                  setSlug(e.target.value);
                  setSlugTouched(true);
                  setSlugServerError(null);
                }}
                disabled={busy}
                spellCheck={false}
                aria-invalid={slugError ? true : undefined}
                aria-describedby={`${titleId}-slug-msg`}
                className={`${fieldCls} font-mono`}
              />
              {slugError ? (
                <p id={`${titleId}-slug-msg`} className={errCls}>
                  {slugError}
                </p>
              ) : (
                <p id={`${titleId}-slug-msg`} className={hintCls}>
                  The customer’s permanent id — used in URLs and on the command line.
                </p>
              )}
            </div>
          )}

          <div>
            <label htmlFor={`${titleId}-contact`} className={labelCls}>
              Contact
            </label>
            <input
              id={`${titleId}-contact`}
              type="text"
              value={contact}
              onChange={(e) => setContact(e.target.value)}
              disabled={busy}
              className={fieldCls}
            />
          </div>

          <div>
            <label htmlFor={`${titleId}-notes`} className={labelCls}>
              Notes
            </label>
            <textarea
              id={`${titleId}-notes`}
              rows={3}
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
              disabled={busy}
              className={fieldCls}
            />
          </div>

          <div>
            <label htmlFor={`${titleId}-color`} className={labelCls}>
              Color
            </label>
            <div className="flex items-center gap-2">
              <span
                className="inline-flex h-6 w-6 items-center justify-center rounded border border-border"
                aria-hidden
              >
                {COLOR_RE.test(colorTrim) ? (
                  <CustomerDot tint={{ light: colorTrim, dark: colorTrim }} size={12} />
                ) : initial ? (
                  // Edit: the tint the server derived for this customer.
                  <CustomerDot tint={initial.tint} size={12} />
                ) : (
                  // Create: the tint is derived server-side from the slug, so
                  // there is nothing to preview yet.
                  <span data-neutral-dot className="inline-block h-3 w-3 rounded-full bg-border" />
                )}
              </span>
              <input
                id={`${titleId}-color`}
                type="text"
                value={color}
                onChange={(e) => {
                  setColor(e.target.value);
                  setColorServerError(null);
                }}
                placeholder="#rrggbb"
                disabled={busy}
                spellCheck={false}
                aria-invalid={colorError ? true : undefined}
                className={`${fieldCls} font-mono`}
              />
            </div>
            {colorError ? (
              <p className={errCls}>{colorError}</p>
            ) : (
              colorTrim === '' && <p className={hintCls}>Empty — derived from the slug.</p>
            )}
          </div>

          {formError && <ErrorBanner>{formError}</ErrorBanner>}

          {confirmingDiscard ? (
            <div ref={confirmRef} role="alertdialog" aria-label="Discard changes?" className="flex items-center justify-end gap-2">
              <span className="mr-auto text-ui font-medium text-ink">Discard changes?</span>
              <Button variant="secondary" onClick={() => setConfirmingDiscard(false)}>
                Keep editing
              </Button>
              <Button variant="danger" onClick={onClose}>
                Discard
              </Button>
            </div>
          ) : (
            <div className="flex items-center justify-end gap-2">
              <Button variant="secondary" onClick={requestClose} disabled={busy}>
                Cancel
              </Button>
              <Button type="submit" disabled={busy || !valid}>
                {create ? 'Create customer' : 'Save'}
              </Button>
            </div>
          )}
        </form>
      </div>
    </div>
  );
}

export default CustomerFormDialog;
