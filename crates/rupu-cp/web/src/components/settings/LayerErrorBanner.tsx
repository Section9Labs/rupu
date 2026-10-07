// A malformed config layer (customer or project). The server still serves the
// view, resolved from the layers that do parse, and says which it kept
// (`layer_error_kept`, `crates/rupu-cp/src/api/config.rs` `get_config`):
//   - `global_customer`: only the PROJECT layer is broken and its customer's
//     layer resolves — the values are global + that customer's layer;
//   - `global`: a broken customer layer (which drops the project layer too),
//     or a broken project layer with no customer — the global config alone.
// The page says exactly that instead of quietly showing those values as if
// they were the broken layer's.

import { ErrorBanner } from '../ui/ErrorBanner';

export type KeptLayers = 'global' | 'global_customer';

export function layerErrorCopy(kept: KeptLayers | undefined, customerName?: string | null): string {
  if (kept === 'global_customer') {
    const theirs = customerName ? `${customerName}'s layer` : "its customer's layer";
    return `This project's config doesn't parse — the values below are the global config plus ${theirs}, without this project's own settings, until it's fixed.`;
  }
  if (kept === 'global') {
    return "A config layer doesn't parse — the values below are the global config alone until it's fixed.";
  }
  return "A config layer doesn't parse — the values below leave out the layer that doesn't parse until it's fixed.";
}

export function LayerErrorBanner({
  message,
  hint,
  kept,
  customerName,
}: {
  message: string;
  hint?: string;
  kept?: KeptLayers;
  customerName?: string | null;
}) {
  return (
    <ErrorBanner>
      <p>
        {layerErrorCopy(kept, customerName)}
        {hint ? ` ${hint}` : ''}
      </p>
      <p className="mt-1 whitespace-pre-wrap font-mono text-note">{message}</p>
    </ErrorBanner>
  );
}
