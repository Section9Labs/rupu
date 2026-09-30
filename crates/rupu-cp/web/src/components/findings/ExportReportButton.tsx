// ExportReportButton — the "Export report" trigger the findings lists share.
// It owns the dialog's open state and is disabled (with the reason on hover
// and for assistive tech) when the current filtered set is empty, so the
// dialog is never opened over nothing.

import { useEffect, useId, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { ExportDialog, NO_FINDINGS_TEXT, type ExportableFinding } from './ExportDialog';

export interface ExportReportButtonProps {
  /** The rows currently shown, after the page's filters. */
  findings: readonly ExportableFinding[];
  defaultTitle: string;
  /** Scopes the export to one project. */
  wsId?: string;
}

export function ExportReportButton({ findings, defaultTitle, wsId }: ExportReportButtonProps) {
  const [open, setOpen] = useState(false);
  const reasonId = useId();
  const triggerRef = useRef<HTMLSpanElement>(null);
  const wasOpen = useRef(false);
  const empty = findings.length === 0;

  // Give focus back to the trigger once the dialog is gone. Remembering
  // `document.activeElement` at open time is not enough: a mouse click does
  // not focus a button in Safari / Firefox on macOS. Done in an effect so it
  // runs after the dialog (and its focus containment) has unmounted.
  useEffect(() => {
    if (open) {
      wasOpen.current = true;
    } else if (wasOpen.current) {
      wasOpen.current = false;
      triggerRef.current?.querySelector('button')?.focus();
    }
  }, [open]);

  return (
    <>
      {/* The tooltip sits on the wrapper: a disabled button gets no hover events. */}
      <span ref={triggerRef} className="inline-flex" title={empty ? NO_FINDINGS_TEXT : undefined}>
        <Button
          variant="secondary"
          disabled={empty}
          aria-describedby={empty ? reasonId : undefined}
          onClick={() => setOpen(true)}
        >
          Export report
        </Button>
      </span>
      {empty && (
        <span id={reasonId} className="sr-only">
          {NO_FINDINGS_TEXT}
        </span>
      )}
      <ExportDialog
        open={open}
        onClose={() => setOpen(false)}
        findings={findings}
        defaultTitle={defaultTitle}
        wsId={wsId}
      />
    </>
  );
}

export default ExportReportButton;
