// The page-header chip naming the current customer scope (mockup artboard 4):
// the customer's dot + name and a × that clears the scope (`setScope(null)`,
// so the page refetches unfiltered). "Unassigned" for the `none` scope; a slug
// whose row hasn't resolved yet shows the slug. Renders nothing when unscoped.

import { X } from 'lucide-react';
import { useCustomerScope } from '../../lib/customerScope';
import { CustomerChip } from './CustomerChip';

export function ScopeChip() {
  const { scope, customer, setScope } = useCustomerScope();
  if (scope === null) return null;
  const clear = () => setScope(null);
  if (customer) return <CustomerChip customer={customer} onRemove={clear} />;
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap rounded-full border border-dashed border-line px-2 py-0.5 text-xs text-ink-dim">
      {scope === 'none' ? 'Unassigned' : scope}
      <button
        type="button"
        aria-label="Clear customer scope"
        onClick={clear}
        className="-mr-0.5 inline-flex items-center rounded-full p-0.5 hover:bg-surface-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500"
      >
        <X size={12} aria-hidden />
      </button>
    </span>
  );
}
