// A malformed config layer (customer or project): the server then serves the
// global config alone as `effective` (and keeps the raw text so it can be
// fixed), so the page says so instead of quietly showing global values as if
// they were the layer's.

import { ErrorBanner } from '../ui/ErrorBanner';

export function LayerErrorBanner({ message, hint }: { message: string; hint?: string }) {
  return (
    <ErrorBanner>
      <p>
        A config layer doesn&apos;t parse — the values below are the global config alone until it&apos;s fixed.
        {hint ? ` ${hint}` : ''}
      </p>
      <p className="mt-1 whitespace-pre-wrap font-mono text-note">{message}</p>
    </ErrorBanner>
  );
}
