// Security → Assets — the engagement asset inventory across every registered
// workspace. The read side of the asset model (`rupu_coverage::asset`): hosts
// and services for `network` engagements, sites and routes for `web`, files for
// `code`. Thin page shell over the shared `AssetInventory` view.

import AssetInventory from '../components/agentiflow/AssetInventory';
import { PageHeader } from '../components/ui/PageHeader';

export default function Assets() {
  return (
    <div className="p-8">
      <PageHeader
        className="mb-6"
        title="Assets"
        subtitle="Everything the fleets have discovered — hosts and services, sites and routes, files — with how deeply each has been examined."
      />
      <AssetInventory />
    </div>
  );
}
