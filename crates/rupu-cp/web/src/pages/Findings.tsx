// Global Findings — every finding across every project, in the shared
// `FindingsExplorer` (query bar, severity tiles, export, bulk tagging). The
// table's Project / Target columns show each finding's owning project · target.
//
// Customer scope (customers Plan 2B): the list follows the global scope
// (`useScopedList`), or — embedded in a customer's Findings tab — a fixed
// `customer` prop, which also drops the page header and padding. Either limits
// it to the findings of that customer's projects (`GET /api/findings?customer=`,
// applied before `q`; the facets and tiles count only the kept findings).
// Findings are the coordinator's own records, keyed on each project's current
// assignment, so no host is ever left out here. A scope the backend rejects
// (400) is cleared with a notice.

import { FindingsExplorer } from '../components/findings/FindingsExplorer';
import { useScopedList } from '../lib/useScopedList';

export default function Findings({ customer: fixedCustomer }: { customer?: string } = {}) {
  const { customer, embedded, guard } = useScopedList(fixedCustomer, []);

  return (
    <div className={embedded ? undefined : 'p-8'}>
      {!embedded && (
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Findings</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Every finding raised across all registered projects. Filter with the query bar, e.g.{' '}
          <code className="font-mono text-note">severity&gt;=high tag:needs-poc</code> — press / to focus.
        </p>
      </header>
      )}

      <FindingsExplorer
        scope={{ customer }}
        guard={guard}
        exportTitle="Findings report"
        empty={{
          title: 'No findings',
          hint:
            customer === 'none'
              ? 'No project without a customer has recorded a finding yet.'
              : customer
                ? 'None of this customer’s projects has recorded a finding yet.'
                : 'Run an assessment workflow to start recording findings across your projects.',
        }}
      />
    </div>
  );
}
