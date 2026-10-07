import { AlertTriangle } from 'lucide-react';

/** Shown next to any cost taken from a `UsageSummary` whose `pricing_error` is
 *  set: the number is unreliable, and the error text is the tooltip. Renders
 *  nothing when there is no error. */
export function PricingErrorMark({ error }: { error?: string | null }) {
  if (!error) return null;
  return (
    <span
      role="img"
      aria-label={`Pricing unavailable: ${error}`}
      title={error}
      className="inline-flex items-center align-middle text-warn"
    >
      <AlertTriangle size={12} aria-hidden />
    </span>
  );
}
