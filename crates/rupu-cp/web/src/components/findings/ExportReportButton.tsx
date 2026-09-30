// ExportReportButton — the "Export report" trigger the findings lists share.
// It owns the dialog's open state and is disabled (with the reason on hover
// and for assistive tech) when the current filtered set is empty, so the
// dialog is never opened over nothing.

import { useId, useState } from 'react';
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
  const empty = findings.length === 0;
  return (
    <>
      {/* The tooltip sits on the wrapper: a disabled button gets no hover events. */}
      <span className="inline-flex" title={empty ? NO_FINDINGS_TEXT : undefined}>
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
