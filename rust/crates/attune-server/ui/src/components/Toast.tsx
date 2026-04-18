/** Attune Toast · 右下角非阻塞提示 · 自动消失 */

import type { JSX } from 'preact';
import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';

export type ToastKind = 'info' | 'success' | 'warning' | 'error';
export type ToastItem = {
  id: string;
  kind: ToastKind;
  message: string;
  /** ms；0 = 不自动消失 */
  duration?: number;
};

const toasts = signal<ToastItem[]>([]);

export function toast(kind: ToastKind, message: string, duration = 3000): string {
  const id = crypto.randomUUID();
  toasts.value = [...toasts.value, { id, kind, message, duration }];
  if (duration > 0) {
    setTimeout(() => dismissToast(id), duration);
  }
  return id;
}

export function dismissToast(id: string): void {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

const KIND_COLORS: Record<ToastKind, { bg: string; border: string; icon: string }> = {
  info: { bg: 'var(--color-info)', border: 'var(--color-info)', icon: 'ℹ' },
  success: { bg: 'var(--color-success)', border: 'var(--color-success)', icon: '✓' },
  warning: { bg: 'var(--color-warning)', border: 'var(--color-warning)', icon: '⚠' },
  error: { bg: 'var(--color-error)', border: 'var(--color-error)', icon: '✕' },
};

export function ToastContainer(): JSX.Element {
  return (
    <div
      aria-live="polite"
      aria-atomic="false"
      style={{
        position: 'fixed',
        bottom: 'var(--space-5)',
        right: 'var(--space-5)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-2)',
        zIndex: 2000,
        pointerEvents: 'none',
      }}
    >
      {toasts.value.map((t) => (
        <ToastItemView key={t.id} toast={t} />
      ))}
    </div>
  );
}

function ToastItemView({ toast: t }: { toast: ToastItem }): JSX.Element {
  const colors = KIND_COLORS[t.kind];
  useEffect(() => {
    // keyboard dismiss via Esc would be redundant; they auto-dismiss
  }, []);
  return (
    <div
      role={t.kind === 'error' || t.kind === 'warning' ? 'alert' : 'status'}
      className="fade-slide-in"
      style={{
        padding: 'var(--space-3) var(--space-4)',
        minWidth: 280,
        maxWidth: 480,
        background: 'var(--color-surface)',
        border: `1px solid ${colors.border}`,
        borderLeft: `4px solid ${colors.bg}`,
        borderRadius: 'var(--radius-md)',
        boxShadow: 'var(--shadow-lg)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
        fontSize: 'var(--text-sm)',
        color: 'var(--color-text)',
        pointerEvents: 'auto',
      }}
    >
      <span aria-hidden="true" style={{ color: colors.bg, fontSize: 'var(--text-base)' }}>
        {colors.icon}
      </span>
      <span style={{ flex: 1 }}>{t.message}</span>
      <button
        type="button"
        onClick={() => dismissToast(t.id)}
        aria-label="Dismiss"
        style={{
          background: 'transparent',
          border: 'none',
          color: 'var(--color-text-secondary)',
          cursor: 'pointer',
          fontSize: 'var(--text-lg)',
          padding: 0,
          lineHeight: 1,
        }}
      >
        ×
      </button>
    </div>
  );
}
