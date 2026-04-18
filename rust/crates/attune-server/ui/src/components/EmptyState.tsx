/** Attune EmptyState · 统一空状态容器
 * 见 UX Quality spec §5 "空状态教育内容"
 */

import type { ComponentChildren, JSX } from 'preact';
import { Button } from './Button';
import type { ButtonVariant } from './Button';

export type EmptyStateAction = {
  label: string;
  onClick: () => void;
  variant?: ButtonVariant;
};

export type EmptyStateProps = {
  icon?: ComponentChildren;
  title: string;
  description?: string;
  actions?: EmptyStateAction[];
  /** 示例内容（chat 的 sample prompts / items 的拖拽提示） */
  examples?: string[];
  onExampleClick?: (example: string) => void;
};

export function EmptyState({
  icon,
  title,
  description,
  actions,
  examples,
  onExampleClick,
}: EmptyStateProps): JSX.Element {
  return (
    <div
      className="fade-slide-in"
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 'var(--space-4)',
        padding: 'var(--space-7) var(--space-5)',
        textAlign: 'center',
        minHeight: 320,
      }}
    >
      {icon && (
        <div
          aria-hidden="true"
          style={{
            fontSize: 48,
            color: 'var(--color-text-secondary)',
            opacity: 0.6,
          }}
        >
          {icon}
        </div>
      )}
      <h3
        style={{
          fontSize: 'var(--text-lg)',
          fontWeight: 600,
          color: 'var(--color-text)',
          margin: 0,
        }}
      >
        {title}
      </h3>
      {description && (
        <p
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            maxWidth: 480,
            margin: 0,
          }}
        >
          {description}
        </p>
      )}
      {actions && actions.length > 0 && (
        <div style={{ display: 'flex', gap: 'var(--space-2)', marginTop: 'var(--space-2)' }}>
          {actions.map((a, i) => (
            <Button key={i} variant={a.variant ?? 'primary'} onClick={a.onClick}>
              {a.label}
            </Button>
          ))}
        </div>
      )}
      {examples && examples.length > 0 && (
        <div
          style={{
            display: 'flex',
            flexWrap: 'wrap',
            gap: 'var(--space-2)',
            justifyContent: 'center',
            marginTop: 'var(--space-4)',
            maxWidth: 640,
          }}
        >
          {examples.map((ex, i) => (
            <button
              key={i}
              type="button"
              onClick={() => onExampleClick?.(ex)}
              className="interactive"
              style={{
                padding: 'var(--space-2) var(--space-3)',
                background: 'var(--color-surface)',
                border: '1px solid var(--color-border)',
                borderRadius: 'var(--radius-lg)',
                fontSize: 'var(--text-sm)',
                color: 'var(--color-text-secondary)',
                cursor: 'pointer',
              }}
              onMouseEnter={(e) =>
                (e.currentTarget.style.background = 'var(--color-surface-hover)')
              }
              onMouseLeave={(e) =>
                (e.currentTarget.style.background = 'var(--color-surface)')
              }
            >
              {ex}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
