// Generic portal'd tooltip. Two modes:
//
//   Uncontrolled (no `open` prop) — opens on mouseenter/focus,
//   closes on mouseleave/blur. `delay` defaults to 0 for instant
//   appearance (vs the ~1s native browser-tooltip delay).
//
//   Controlled (`open` + `onOpenChange` set) — caller drives
//   open/close. Closes on outside click, Escape, or any descendant
//   calling onOpenChange(false).
//
// Portal target is document.body so the tooltip is never clipped by
// SVG viewport or parent overflow:hidden.

import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

interface TooltipProps {
  content: ReactNode;
  placement?: 'top' | 'bottom';
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  delay?: number;
  children: ReactNode;
}

export function Tooltip({
  content,
  placement = 'top',
  open: controlledOpen,
  onOpenChange,
  delay = 0,
  children,
}: TooltipProps) {
  const isControlled = controlledOpen !== undefined;
  const [hoverOpen, setHoverOpen] = useState(false);
  const open = isControlled ? !!controlledOpen : hoverOpen;

  const anchorRef = useRef<HTMLSpanElement | null>(null);
  const tooltipRef = useRef<HTMLDivElement | null>(null);
  const timerRef = useRef<number | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const id = useId();

  function clearDelay() {
    if (timerRef.current != null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }

  function showHover() {
    if (isControlled) return;
    clearDelay();
    if (delay <= 0) { setHoverOpen(true); return; }
    timerRef.current = window.setTimeout(() => setHoverOpen(true), delay);
  }

  function hideHover() {
    if (isControlled) return;
    clearDelay();
    setHoverOpen(false);
  }

  useEffect(() => {
    if (!open) { setPos(null); return; }
    function update() {
      const a = anchorRef.current;
      if (!a) return;
      const r = a.getBoundingClientRect();
      const left = r.left + r.width / 2;
      const top = placement === 'top' ? r.top : r.bottom;
      setPos({ left, top });
    }
    update();
    window.addEventListener('scroll', update, true);
    window.addEventListener('resize', update);
    let ro: ResizeObserver | null = null;
    if (anchorRef.current && typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(update);
      ro.observe(anchorRef.current);
    }
    return () => {
      window.removeEventListener('scroll', update, true);
      window.removeEventListener('resize', update);
      if (ro) ro.disconnect();
    };
  }, [open, placement]);

  useEffect(() => {
    if (!isControlled || !open) return;
    function onDown(ev: MouseEvent) {
      const t = ev.target as Node | null;
      if (!t) return;
      if (anchorRef.current?.contains(t)) return;
      if (tooltipRef.current?.contains(t)) return;
      onOpenChange?.(false);
    }
    function onKey(ev: KeyboardEvent) {
      if (ev.key === 'Escape') onOpenChange?.(false);
    }
    document.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
    };
  }, [isControlled, open, onOpenChange]);

  useEffect(() => clearDelay, []);

  const tooltipNode = open && pos ? (
    <div
      ref={tooltipRef}
      role={isControlled ? 'dialog' : 'tooltip'}
      id={id}
      style={{
        position: 'fixed',
        left: pos.left,
        top: pos.top,
        transform: placement === 'top'
          ? 'translate(-50%, calc(-100% - 6px))'
          : 'translate(-50%, 6px)',
        pointerEvents: isControlled ? 'auto' : 'none',
        zIndex: 50,
      }}
      className="bg-panel border border-border rounded-md shadow-lg px-2 py-1.5 text-xs text-ink max-w-sm"
    >
      {content}
    </div>
  ) : null;

  return (
    <>
      <span
        ref={anchorRef}
        aria-describedby={!isControlled && open ? id : undefined}
        onMouseEnter={showHover}
        onMouseLeave={hideHover}
        onFocus={showHover}
        onBlur={hideHover}
        style={{ display: 'contents' }}
      >
        {children}
      </span>
      {tooltipNode && createPortal(tooltipNode, document.body)}
    </>
  );
}
