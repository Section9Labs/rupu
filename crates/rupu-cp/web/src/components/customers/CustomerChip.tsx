import { X } from 'lucide-react';
import type { CustomerRef } from '../../lib/api';
import { cn } from '../../lib/cn';
import { CustomerDot } from './CustomerDot';

export const UNKNOWN_CUSTOMER_TITLE = "This host or record can't say whose this is";
export const DERIVED_CUSTOMER_TITLE = "Attributed from the project's current customer";

/**
 * A row's customer, in one of three states (docs/cp-customers-api.md):
 *  - `customer` set     → tinted pill with the customer's dot + name;
 *  - `customer` null    → dashed outlined "No customer" (the record has none);
 *  - `unknown`          → muted "Unknown customer": the `customer` key was
 *                         ABSENT, i.e. the host/record can't say. Never render
 *                         an absent key as "No customer".
 * `derived` marks an attribution taken from the project's current customer.
 * `onRemove` adds a "Clear customer scope" × (the top-bar scope chip).
 */
export function CustomerChip({
  customer,
  unknown,
  derived,
  onRemove,
  size = 'md',
}: {
  customer: CustomerRef | null;
  unknown?: boolean;
  derived?: boolean;
  onRemove?: () => void;
  size?: 'sm' | 'md';
}) {
  const sizing = size === 'sm' ? 'px-1.5 py-px text-meta gap-1' : 'px-2 py-0.5 text-xs gap-1.5';
  const base = cn('inline-flex items-center rounded-full border whitespace-nowrap', sizing);

  if (unknown) {
    return (
      <span
        title={UNKNOWN_CUSTOMER_TITLE}
        className={cn(base, 'border-line bg-surface text-ink-mute')}
      >
        Unknown customer
      </span>
    );
  }

  if (!customer) {
    return (
      <span className={cn(base, 'border-dashed border-line text-ink-dim')}>No customer</span>
    );
  }

  return (
    <span
      title={derived ? DERIVED_CUSTOMER_TITLE : undefined}
      className={cn(
        base,
        'border-brand-100 bg-brand-50 text-brand-700',
        (customer.archived || derived) && 'opacity-60',
      )}
    >
      <CustomerDot tint={customer.tint} size={size === 'sm' ? 6 : 8} />
      <span>{customer.name}</span>
      {onRemove && (
        <button
          type="button"
          aria-label="Clear customer scope"
          onClick={onRemove}
          className="-mr-0.5 inline-flex items-center rounded-full p-0.5 hover:bg-brand-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500"
        >
          <X size={size === 'sm' ? 10 : 12} aria-hidden />
        </button>
      )}
    </span>
  );
}
