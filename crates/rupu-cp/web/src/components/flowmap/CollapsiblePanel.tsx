// CollapsiblePanel — a panel under a graph's canvas (the run page's Messages
// and Board, the agentiflow map's Messages and Goals) whose title row folds
// it to that row: the title and its count stay, the body hides. The state is
// remembered per panel kind in `localStorage` (`rupu.cp.panel.<name>`); where
// storage can't be read or written the panel opens expanded and still folds
// for the page's life. Folding is instant (no height animation); only the
// chevron turns (styles/graph-motion.css, `afx-panel-*`).

import { useCallback, useId, useState, type ReactNode } from 'react';
import { ChevronDown } from 'lucide-react';

export const PANEL_KEY = (name: string) => `rupu.cp.panel.${name}`;

function readCollapsed(name: string): boolean {
  try {
    return localStorage.getItem(PANEL_KEY(name)) === 'collapsed';
  } catch {
    return false;
  }
}

function writeCollapsed(name: string, collapsed: boolean): void {
  try {
    localStorage.setItem(PANEL_KEY(name), collapsed ? 'collapsed' : 'expanded');
  } catch {
    // No storage: the state lasts as long as the page.
  }
}

export default function CollapsiblePanel({
  name,
  title,
  count,
  sub,
  label,
  testId,
  children,
}: {
  /** The panel kind: its storage key. */
  name: string;
  title: string;
  /** What the folded header still says (a count, `1/1`, `1 claim · 7 posts`). */
  count?: ReactNode;
  /** A muted line after the count (a description), shown either way. */
  sub?: ReactNode;
  label?: string;
  testId?: string;
  children: ReactNode;
}) {
  const [collapsed, setCollapsed] = useState(() => readCollapsed(name));
  const bodyId = useId();
  const toggle = useCallback(() => {
    setCollapsed((c) => {
      writeCollapsed(name, !c);
      return !c;
    });
  }, [name]);
  return (
    <section className="afx-panel" aria-label={label ?? title} data-testid={testId} data-collapsed={collapsed}>
      <h3 className="afx-panel-h">
        <button type="button" className="afx-panel-toggle" aria-expanded={!collapsed} aria-controls={bodyId} onClick={toggle}>
          <ChevronDown size={14} aria-hidden className="afx-panel-chev" />
          {title}
          {count != null && <span className="afx-panel-count">{count}</span>}
        </button>
        {sub}
      </h3>
      <div id={bodyId} hidden={collapsed}>
        {children}
      </div>
    </section>
  );
}
