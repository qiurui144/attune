/**
 * useFocusTrap · Modal / Drawer 内焦点陷阱
 * 见 UX Quality spec §1 "Tab order · Modal 打开时焦点陷阱"
 *
 * 用法：
 *   const ref = useFocusTrap(isOpen);
 *   <div ref={ref}>...</div>
 *
 * 行为：
 *   - 激活时记录当前 focused 元素，把焦点移到容器内首个 focusable
 *   - Tab 循环在容器内部（最后一个之后回到第一个，反向同理）
 *   - 释放时焦点回到激活前的元素
 */

import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(', ');

export function useFocusTrap<T extends HTMLElement>(active: boolean): RefObject<T> {
  const ref = useRef<T | null>(null);
  const lastFocusedRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!active || !ref.current) return;
    const container = ref.current;

    // 记录激活前的焦点元素（用于还原）
    lastFocusedRef.current = document.activeElement as HTMLElement | null;

    // 首个 focusable 拿焦点
    const first = container.querySelector<HTMLElement>(FOCUSABLE_SELECTOR);
    first?.focus();

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Tab') return;
      const focusable = Array.from(
        container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
      ).filter((el) => !el.hasAttribute('data-focus-trap-ignore'));
      if (focusable.length === 0) return;
      const firstEl = focusable[0]!;
      const lastEl = focusable[focusable.length - 1]!;
      const activeEl = document.activeElement;

      if (e.shiftKey && activeEl === firstEl) {
        e.preventDefault();
        lastEl.focus();
      } else if (!e.shiftKey && activeEl === lastEl) {
        e.preventDefault();
        firstEl.focus();
      }
    };

    container.addEventListener('keydown', handleKeyDown);
    return () => {
      container.removeEventListener('keydown', handleKeyDown);
      // 还原焦点
      lastFocusedRef.current?.focus();
    };
  }, [active]);

  return ref as RefObject<T>;
}
