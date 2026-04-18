/**
 * Attune i18n 引擎（轻量自写 · 无依赖）
 * 见 docs/superpowers/specs/2026-04-19-ux-quality-design.md §2
 *
 * 特性：
 *   - 字符串 key 查表
 *   - {param} 参数插值
 *   - plural form（one/other）
 *   - 缺 key 时 fallback 到 zh，再 fallback 到 key 本身
 *   - 切换 locale 自动触发全局 re-render（signal 驱动）
 */

import { signal } from '@preact/signals';
import { zh } from './zh';
import { en } from './en';

export type Locale = 'zh' | 'en';
export type PluralForm = { one: string; other: string };
export type Message = string | PluralForm;
export type Messages = Record<string, Message>;

const MESSAGE_MAP: Record<Locale, Messages> = {
  zh: zh as Messages,
  en: en as Messages,
};

/** 当前 locale · 组件通过订阅此 signal 实现国际化重渲染 */
export const currentLocale = signal<Locale>(detectInitialLocale());

function detectInitialLocale(): Locale {
  if (typeof navigator === 'undefined') return 'zh';
  const lang = (navigator.language || 'zh').toLowerCase();
  if (lang.startsWith('en')) return 'en';
  return 'zh';
}

export function setLocale(locale: Locale): void {
  if (!(locale in MESSAGE_MAP)) {
    console.warn(`Unsupported locale: ${locale}`);
    return;
  }
  currentLocale.value = locale;
  document.documentElement.setAttribute('lang', locale === 'zh' ? 'zh-CN' : 'en');
}

/**
 * 查询消息。key 不存在时按 zh → key 本身依次 fallback。
 *
 * ```ts
 * t('common.save')                     // → "保存"
 * t('error.network', { message: 'EHOSTUNREACH' })  // → "网络错误：EHOSTUNREACH"
 * ```
 */
export function t(
  key: string,
  params?: Record<string, string | number>,
): string {
  const msg = lookup(key, currentLocale.value);
  const text = typeof msg === 'string' ? msg : msg.other;
  return params ? interpolate(text, params) : text;
}

/**
 * 查询 plural 消息。
 *
 * ```ts
 * plural('items.count', 1)   // "1 item"  (en) or "1 条"  (zh)
 * plural('items.count', 5)   // "5 items" (en) or "5 条"  (zh)
 * ```
 */
export function plural(
  key: string,
  count: number,
  params?: Record<string, string | number>,
): string {
  const msg = lookup(key, currentLocale.value);
  if (typeof msg === 'string') {
    // 不是 plural 结构，按普通消息插值
    return interpolate(msg, { ...params, count });
  }
  const text = count === 1 ? msg.one : msg.other;
  return interpolate(text, { ...params, count });
}

function lookup(key: string, locale: Locale): Message {
  const primary = MESSAGE_MAP[locale]?.[key];
  if (primary !== undefined) return primary;
  const fallback = MESSAGE_MAP.zh?.[key];
  if (fallback !== undefined) return fallback;
  return key; // 最终 fallback：key 本身（开发期能看到缺哪个）
}

function interpolate(
  text: string,
  params: Record<string, string | number>,
): string {
  return text.replace(/\{(\w+)\}/g, (_, k: string) => {
    const v = params[k];
    return v !== undefined ? String(v) : `{${k}}`;
  });
}
