/** Tooltip — ⓘ 小图标 hover/click 显示一句话解释.
 *
 * 用法:
 *   <Tooltip text="主密码用于本地加密..." />
 *   <Tooltip text="..." size="sm" />
 *
 * 设计:
 * - hover 显示 (PC) + click 显示 (触屏)
 * - aria-describedby 让屏幕阅读器能读 (a11y)
 * - 内容是 1 句话解释, 太长用换行符 `\n` 隔
 * - 不替代主文案, 是辅助补充 (用户不点也能完成主流程)
 */

import type { JSX } from 'preact';
import { useState, useRef, useEffect } from 'preact/hooks';

export type TooltipProps = {
  /** 解释文案 (短句, 可用 \n 换行) */
  text: string;
  /** 图标尺寸: sm (12px) / md (14px, 默认) */
  size?: 'sm' | 'md';
  /** 可选 "了解更多" 链接 (后续 Help drawer 联动用) */
  learnMoreHref?: string;
};

let idCounter = 0;

export function Tooltip({ text, size = 'md', learnMoreHref }: TooltipProps): JSX.Element {
  const [open, setOpen] = useState(false);
  const tipId = useRef(`tooltip-${++idCounter}`);
  const wrapRef = useRef<HTMLSpanElement | null>(null);

  // 点击外部关闭 (触屏需要)
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onEsc = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('click', onDocClick);
    document.addEventListener('keydown', onEsc);
    return () => {
      document.removeEventListener('click', onDocClick);
      document.removeEventListener('keydown', onEsc);
    };
  }, [open]);

  const iconSize = size === 'sm' ? 12 : 14;

  return (
    <span
      ref={wrapRef}
      style={{ position: 'relative', display: 'inline-block', marginLeft: 4 }}
    >
      <button
        type="button"
        onClick={(e) => { e.stopPropagation(); setOpen((v) => !v); }}
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        aria-label="帮助"
        aria-describedby={open ? tipId.current : undefined}
        aria-expanded={open}
        style={{
          width: iconSize,
          height: iconSize,
          padding: 0,
          border: 'none',
          background: 'var(--color-surface-muted, #E9ECEF)',
          color: 'var(--color-text-secondary)',
          borderRadius: '50%',
          fontSize: Math.max(iconSize - 4, 9),
          lineHeight: `${iconSize}px`,
          fontWeight: 600,
          cursor: 'pointer',
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          verticalAlign: 'middle',
        }}
      >
        ?
      </button>
      {open && (
        <span
          id={tipId.current}
          role="tooltip"
          style={{
            position: 'absolute',
            zIndex: 1000,
            top: '100%',
            left: '50%',
            transform: 'translateX(-50%)',
            marginTop: 6,
            padding: '8px 12px',
            background: 'var(--color-text, #1f2937)',
            color: 'var(--color-surface, #fff)',
            borderRadius: 'var(--radius-sm, 4px)',
            fontSize: 'var(--text-xs, 12px)',
            lineHeight: 1.5,
            whiteSpace: 'pre-line',
            maxWidth: 280,
            width: 'max-content',
            boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
            textAlign: 'left',
            fontWeight: 400,
          }}
          onMouseEnter={() => setOpen(true)}
          onMouseLeave={() => setOpen(false)}
        >
          {text}
          {learnMoreHref && (
            <>
              {'\n'}
              <a
                href={learnMoreHref}
                target="_blank"
                rel="noopener noreferrer"
                style={{ color: 'var(--color-accent, #60a5fa)', textDecoration: 'underline' }}
              >
                了解更多
              </a>
            </>
          )}
        </span>
      )}
    </span>
  );
}
