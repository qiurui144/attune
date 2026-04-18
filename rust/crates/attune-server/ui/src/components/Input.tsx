/** Attune Input · Label + Error + a11y */

import type { JSX } from 'preact';
import { useId } from 'preact/hooks';

export type InputProps = {
  label?: string;
  error?: string;
  hint?: string;
  type?: 'text' | 'password' | 'email' | 'url' | 'number';
  value?: string;
  placeholder?: string;
  disabled?: boolean;
  required?: boolean;
  autoFocus?: boolean;
  onInput?: (e: JSX.TargetedInputEvent<HTMLInputElement>) => void;
  onKeyDown?: (e: JSX.TargetedKeyboardEvent<HTMLInputElement>) => void;
  'aria-label'?: string;
  'data-testid'?: string;
};

export function Input({
  label,
  error,
  hint,
  type = 'text',
  value,
  placeholder,
  disabled,
  required,
  autoFocus,
  onInput,
  onKeyDown,
  'aria-label': ariaLabel,
  'data-testid': testId,
}: InputProps): JSX.Element {
  const id = useId();
  const errorId = `${id}-error`;
  const hintId = `${id}-hint`;
  const describedBy = [error ? errorId : null, hint ? hintId : null]
    .filter(Boolean)
    .join(' ') || undefined;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
      {label && (
        <label
          htmlFor={id}
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
            fontWeight: 500,
          }}
        >
          {label}
          {required && (
            <span aria-hidden="true" style={{ color: 'var(--color-error)', marginLeft: 4 }}>
              *
            </span>
          )}
        </label>
      )}
      <input
        id={id}
        type={type}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        required={required}
        autoFocus={autoFocus}
        onInput={onInput}
        onKeyDown={onKeyDown}
        aria-label={ariaLabel}
        aria-invalid={!!error}
        aria-describedby={describedBy}
        data-testid={testId}
        className="interactive"
        style={{
          height: 'var(--btn-h-md)',
          padding: '0 var(--space-3)',
          fontSize: 'var(--text-base)',
          color: 'var(--color-text)',
          background: 'var(--color-surface)',
          border: `1px solid ${error ? 'var(--color-error)' : 'var(--color-border)'}`,
          borderRadius: 'var(--radius-md)',
          outline: 'none',
        }}
      />
      {hint && !error && (
        <span
          id={hintId}
          style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}
        >
          {hint}
        </span>
      )}
      {error && (
        <span
          id={errorId}
          role="alert"
          style={{ fontSize: 'var(--text-xs)', color: 'var(--color-error)' }}
        >
          {error}
        </span>
      )}
    </div>
  );
}
