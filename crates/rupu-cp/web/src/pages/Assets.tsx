// Security → Assets — the engagement asset inventory across every registered
// workspace. The read side of the asset model (`rupu_coverage::asset`): hosts
// and services for `network` engagements, sites and routes for `web`, files for
// `code`. Thin page shell over the shared `AssetInventory` view.

import AssetInventory from '../components/agentiflow/AssetInventory';

export default function Assets() {
  return (
    <div className="p-8">
      <header className="mb-6">
        <h1 className="text-2xl font-semibold text-ink">Assets</h1>
        <p className="mt-1 text-sm text-ink-dim">
          Everything the fleets have discovered — hosts and services, sites and routes, files — with how deeply each has
          been examined.
        </p>
      </header>
      <AssetInventory />
    </div>
  );
}
