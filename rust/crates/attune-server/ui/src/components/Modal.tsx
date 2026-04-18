/** Attune Modal · Focus trap + Esc close + backdrop click close */

import type { ComponentChildren, JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useFocusTrap } from '../hooks/useFocusTrap';

export type ModalProps = {
  open: boolean;
  onClose: () => void;
  title?: string;
  children: ComponentChildren;
  maxWidth?: number;
  /** 禁用 backdrop click 关闭（关键操作确认用） */
  disableBackdropClose?: boolean;
  /** 禁用 ESC 关闭 */
  disableEscClose?: boolean;
};

export function Modal({
  open,
  onClose,
  title,
  children,
  maxWidth = 560,
  disableBackdropClose = false,
  disableEscClose = false,
}: ModalProps): JSX.Element | null {
  const ref = useFocusTrap<HTMLDivElement>(open);

  useEffect(() => {
    if (!open) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !disableEscClose) {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener('keydown', handleKey);
    // 锁滚动
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', handleKey);
      document.body.style.overflow = prevOverflow;
    };
  }, [open, disableEscClose, onClose]);

  if (!open) return null;

  return (
    <div
      className="fade-in"
      onClick={disableBackdropClose ? undefined : onClose}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(36, 43, 55, 0.4)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 'var(--space-4)',
        zIndex: 1000,
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={title ? 'modal-title' : undefined}
        className="modal-in"
        onClick={(e) => e.stopPropagation()}
        style={{
          background: 'var(--color-surface)',
          borderRadius: 'var(--radius-xl)',
          boxShadow: 'var(--shadow-xl)',
          maxWidth,
          width: '100%',
          maxHeight: '90vh',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}
      >
        {title && (
          <header
            style={{
              padding: 'var(--space-4) var(--space-5)',
              borderBottom: '1px solid var(--color-border)',
            }}
          >
            <h2
              id="modal-title"
              style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}
            >
              {title}
            </h2>
          </header>
        )}
        <div style={{ padding: 'var(--space-5)', overflow: 'auto' }}>{children}</div>
      </div>
    </div>
  );
}
