// PageHeader — the ONE way a page (or a section header) opens, per the common
// visual language (spec §"Page header"): an optional Back affordance, a title
// at one size, an optional one-line subtitle, and a right-aligned action zone.
// One primary (`solid`) action per view; secondary/ghost sit beside it; brand
// purple never becomes a chrome button here.
//
// Rendering only — callers pass their own `actions` (a range Segmented, a
// "Usage →" secondary, …). Token classes only.

import { Link } from 'react-router-dom';
import { ChevronLeft } from 'lucide-react';
import { cn } from '../../lib/cn';

export interface PageHeaderProps {
  title: React.ReactNode;
  /** One short line under the title; omitted entirely when absent (no empty node). */
  subtitle?: React.ReactNode;
  /** Optional back affordance above the title. */
  back?: { label: string; to: string };
  /** Right-aligned actions (buttons, a Segmented, a chip). */
  actions?: React.ReactNode;
  /** Extra classes on the header row (layout only). */
  className?: string;
}

export function PageHeader({ title, subtitle, back, actions, className }: PageHeaderProps) {
  return (
    <header className={cn('flex flex-wrap items-start gap-x-4 gap-y-2', className)}>
      <div className="min-w-0 flex-1">
        {back && (
          <Link
            to={back.to}
            className="mb-1 inline-flex items-center gap-1 text-note text-ink-mute hover:text-ink"
          >
            <ChevronLeft size={13} aria-hidden />
            {back.label}
          </Link>
        )}
        <h1 className="truncate text-lg font-semibold tracking-tight text-ink">{title}</h1>
        {subtitle != null && subtitle !== '' && (
          <p className="mt-0.5 text-ui text-ink-mute">{subtitle}</p>
        )}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </header>
  );
}

export default PageHeader;
