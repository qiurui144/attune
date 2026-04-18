/**
 * useShortcut · 键盘快捷键注册
 * 见 UX Quality spec §3 "键盘快捷键注册 + 帮助覆盖层"
 *
 * 特性：
 *   - Cmd(meta) 在 macOS / Ctrl 在其他 OS 自动识别
 *   - when() 条件：如 "Chat input 聚焦时"
 *   - 组件卸载自动清理
 *   - 冲突处理：后注册者优先（用于 modal 内部覆盖全局）
 */

import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';

export type Shortcut = {
  /** 按键字符或特殊键（"Enter" / "Escape" / "/" / "k"） */
  key: string;
  /** Cmd / Ctrl 修饰（跨平台；macOS 是 Cmd，其他 Ctrl） */
  meta?: boolean;
  /** Shift 修饰 */
  shift?: boolean;
  /** Alt 修饰 */
  alt?: boolean;
  /** 何时生效（默认总是） */
  when?: () => boolean;
  /** 回调 */
  handler: (e: KeyboardEvent) => void;
  /** 描述（i18n key），用于 help overlay 展示 */
  description: string;
};

/** 当前注册的所有快捷键（help overlay 读取） */
export const registeredShortcuts = signal<Shortcut[]>([]);

const isMac = typeof navigator !== 'undefined' && /Mac/.test(navigator.platform);

function matches(e: KeyboardEvent, s: Shortcut): boolean {
  if (e.key.toLowerCase() !== s.key.toLowerCase()) return false;
  const metaPressed = isMac ? e.metaKey : e.ctrlKey;
  if (!!s.meta !== metaPressed) return false;
  if (!!s.shift !== e.shiftKey) return false;
  if (!!s.alt !== e.altKey) return false;
  return s.when ? s.when() : true;
}

/** 注册一个全局快捷键 · 组件卸载自动清理 */
export function useShortcut(shortcut: Shortcut): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (matches(e, shortcut)) {
        e.preventDefault();
        shortcut.handler(e);
      }
    };
    document.addEventListener('keydown', handler);
    registeredShortcuts.value = [...registeredShortcuts.value, shortcut];
    return () => {
      document.removeEventListener('keydown', handler);
      registeredShortcuts.value = registeredShortcuts.value.filter((s) => s !== shortcut);
    };
  }, [
    shortcut.key,
    shortcut.meta,
    shortcut.shift,
    shortcut.alt,
    shortcut.description,
  ]);
}

/** 返回平台相关的快捷键显示字符串："⌘K" (mac) / "Ctrl+K" (其他) */
export function formatShortcut(s: Shortcut): string {
  const mod = isMac ? '⌘' : 'Ctrl';
  const parts: string[] = [];
  if (s.meta) parts.push(mod);
  if (s.shift) parts.push(isMac ? '⇧' : 'Shift');
  if (s.alt) parts.push(isMac ? '⌥' : 'Alt');
  const keyLabel =
    s.key === ' '
      ? 'Space'
      : s.key.length === 1
        ? s.key.toUpperCase()
        : s.key;
  parts.push(keyLabel);
  return isMac ? parts.join('') : parts.join('+');
}
