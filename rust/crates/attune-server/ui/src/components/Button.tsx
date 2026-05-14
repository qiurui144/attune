/** Attune Button · A11y baseline contract
 * 见 UX Quality spec §1 "组件 baseline contract"
 */

import type { ComponentChildren, JSX } from 'preact';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type ButtonSize = 'sm' | 'md' | 'lg';

export type ButtonProps = {
  variant?: ButtonVariant;
  size?: ButtonSize;
  disabled?: boolean;
  loading?: boolean;
  fullWidth?: boolean;
  onClick?: (e: JSX.TargetedMouseEvent<HTMLButtonElement>) => void;
  type?: 'button' | 'submit' | 'reset';
  /** 无文字内容时必填（icon-only 按钮） */
  'aria-label'?: string;
  /** toggle 按钮标明是否按下 */
  'aria-pressed'?: boolean;
  children?: ComponentChildren;
};

const VARIANT_STYLES: Record<ButtonVariant, JSX.CSSProperties> = {
  primary: {
    background: 'var(--color-accent)',
    color: 'var(--color-accent-on, white)',
    border: '1px solid var(--color-accent)',
    boxShadow: '0 1px 2px rgba(36, 43, 55, 0.06)',
  },
  secondary: {
    background: 'var(--color-surface)',
    color: 'var(--color-text)',
    border: '1px solid var(--color-border)',
  },
  ghost: {
    background: 'transparent',
    color: 'var(--color-text)',
    border: '1px solid transparent',
  },
  danger: {
    background: 'var(--color-error)',
    color: 'white',
    border: '1px solid var(--color-error)',
    boxShadow: '0 1px 2px rgba(36, 43, 55, 0.06)',
  },
};

const SIZE_STYLES: Record<ButtonSize, JSX.CSSProperties> = {
  sm: { height: 'var(--btn-h-sm)', padding: '0 var(--space-3)', fontSize: 'var(--text-sm)' },
  md: { height: 'var(--btn-h-md)', padding: '0 var(--space-4)', fontSize: 'var(--text-base)' },
  lg: { height: 'var(--btn-h-lg)', padding: '0 var(--space-5)', fontSize: 'var(--text-base)' },
};

export function Button({
  variant = 'secondary',
  size = 'md',
  disabled = false,
  loading = false,
  fullWidth = false,
  onClick,
  type = 'button',
  children,
  ...ariaProps
}: ButtonProps): JSX.Element {
  const isDisabled = disabled || loading;
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={isDisabled}
      aria-disabled={isDisabled}
      aria-busy={loading}
      {...ariaProps}
      className="interactive"
      style={{
        ...VARIANT_STYLES[variant],
        ...SIZE_STYLES[size],
        width: fullWidth ? '100%' : undefined,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 'var(--space-2)',
        borderRadius: 'var(--radius-md)',
        fontWeight: 500,
        cursor: isDisabled ? 'not-allowed' : 'pointer',
        opacity: isDisabled ? 0.5 : 1,
        whiteSpace: 'nowrap',
      }}
      onMouseEnter={(e) => {
        if (isDisabled) return;
        if (variant === 'primary') {
          e.currentTarget.style.background = 'var(--color-accent-hover)';
          e.currentTarget.style.boxShadow = '0 2px 6px rgba(36, 43, 55, 0.14)';
        } else if (variant === 'secondary') {
          e.currentTarget.style.background = 'var(--color-surface-hover)';
          e.currentTarget.style.boxShadow = '0 1px 3px rgba(36, 43, 55, 0.08)';
        } else if (variant === 'ghost') {
          e.currentTarget.style.background = 'var(--color-surface-hover)';
        } else if (variant === 'danger') {
          e.currentTarget.style.background = 'var(--color-error)';
          e.currentTarget.style.boxShadow = '0 2px 6px rgba(201, 112, 112, 0.25)';
        }
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = VARIANT_STYLES[variant].background as string;
        e.currentTarget.style.boxShadow = (VARIANT_STYLES[variant].boxShadow as string) ?? 'none';
      }}
    >
      {loading && <span className="spinner" aria-hidden="true" />}
      {children}
    </button>
  );
}
