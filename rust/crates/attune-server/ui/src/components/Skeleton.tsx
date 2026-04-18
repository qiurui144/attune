/** Attune Skeleton · 加载占位 */

import type { JSX } from 'preact';

export type SkeletonProps = {
  width?: string | number;
  height?: string | number;
  /** 适合文本块（多行） */
  lines?: number;
  circle?: boolean;
};

export function Skeleton({
  width = '100%',
  height = '1em',
  lines = 1,
  circle = false,
}: SkeletonProps): JSX.Element {
  if (lines > 1) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
        {Array.from({ length: lines }).map((_, i) => (
          <div
            key={i}
            className="skeleton"
            style={{
              width: i === lines - 1 ? '60%' : width,
              height,
            }}
            aria-hidden="true"
          />
        ))}
      </div>
    );
  }
  return (
    <div
      className="skeleton"
      style={{
        width,
        height,
        borderRadius: circle ? '50%' : undefined,
      }}
      aria-hidden="true"
    />
  );
}
