/** Attune Drawer · Right slide-in · Focus trap · ESC close · Resizable */

import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { useFocusTrap } from '../hooks/useFocusTrap';

export type DrawerProps = {
  open: boolean;
  onClose: () => void;
  title?: string;
  children: ComponentChildren;
  /** 初始宽度 px（可拖拽调节） */
  defaultWidth?: number;
  /** 最小宽度 px */
  minWidth?: number;
  /** 最大宽度 px */
  maxWidth?: number;
};

export function Drawer({
  open,
  onClose,
  title,
  children,
  defaultWidth = 480,
  minWidth = 320,
  maxWidth = 800,
}: DrawerProps): JSX.Element | null {
  const ref = useFocusTrap<HTMLDivElement>(open);
  const [width, setWidth] = useState(defaultWidth);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    if (!open) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [open, onClose]);

  useEffect(() => {
    if (!dragging) return;
    const handleMove = (e: MouseEvent) => {
      const newWidth = Math.min(Math.max(window.innerWidth - e.clientX, minWidth), maxWidth);
      setWidth(newWidth);
    };
    const handleUp = () => setDragging(false);
    document.addEventListener('mousemove', handleMove);
    document.addEventListener('mouseup', handleUp);
    return () => {
      document.removeEventListener('mousemove', handleMove);
      document.removeEventListener('mouseup', handleUp);
    };
  }, [dragging, minWidth, maxWidth]);

  if (!open) return null;

  return (
    <div
      className="fade-in"
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(36, 43, 55, 0.2)',
        zIndex: 900,
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={title ? 'drawer-title' : undefined}
        className="drawer-in-right"
        onClick={(e) => e.stopPropagation()}
        style={{
          position: 'absolute',
          top: 0,
          right: 0,
          bottom: 0,
          width,
          background: 'var(--color-surface)',
          boxShadow: 'var(--shadow-xl)',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}
      >
        {/* 拖拽手柄 */}
        <div
          onMouseDown={() => setDragging(true)}
          aria-hidden="true"
          style={{
            position: 'absolute',
            left: 0,
            top: 0,
            bottom: 0,
            width: 4,
            cursor: 'ew-resize',
            background: dragging ? 'var(--color-accent)' : 'transparent',
            transition: 'background var(--duration-fast) var(--ease-out)',
          }}
        />
        {title && (
          <header
            style={{
              padding: 'var(--space-4) var(--space-5)',
              borderBottom: '1px solid var(--color-border)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
            }}
          >
            <h2
              id="drawer-title"
              style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}
            >
              {title}
            </h2>
            <button
              type="button"
              onClick={onClose}
              aria-label="Close"
              style={{
                background: 'transparent',
                border: 'none',
                fontSize: 'var(--text-xl)',
                color: 'var(--color-text-secondary)',
                cursor: 'pointer',
                padding: 'var(--space-1) var(--space-2)',
              }}
            >
              ×
            </button>
          </header>
        )}
        <div style={{ flex: 1, overflow: 'auto', padding: 'var(--space-5)' }}>{children}</div>
      </div>
    </div>
  );
}
