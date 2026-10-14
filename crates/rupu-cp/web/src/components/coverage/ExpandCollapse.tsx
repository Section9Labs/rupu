// Expand-all / collapse-all for a coverage tab's accordion rows.
import { Button } from '../ui/Button';

export default function ExpandCollapse({ onExpandAll, onCollapseAll }: { onExpandAll: () => void; onCollapseAll: () => void }) {
  return (
    <>
      <Button variant="secondary" size="sm" onClick={onExpandAll} className="px-2 text-xs">
        expand all
      </Button>
      <Button variant="secondary" size="sm" onClick={onCollapseAll} className="px-2 text-xs">
        collapse all
      </Button>
    </>
  );
}
