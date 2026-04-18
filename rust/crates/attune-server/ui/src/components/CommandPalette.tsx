/** CommandPalette · Cmd+K 全局搜索（会话 + 条目 + 跳转） */

import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { useSignal, useComputed } from '@preact/signals';
import { api } from '../store/api';
import {
  chatSessions,
  items,
  currentView,
  activeSessionId,
  drawerContent,
} from '../store/signals';

type SearchResult = {
  kind: 'session' | 'item' | 'view';
  id: string;
  title: string;
  subtitle?: string;
  action: () => void;
};

const VIEW_SHORTCUTS: SearchResult[] = [
  { kind: 'view', id: 'v-chat', title: '💬 对话', action: () => (currentView.value = 'chat') },
  { kind: 'view', id: 'v-items', title: '📄 条目', action: () => (currentView.value = 'items') },
  { kind: 'view', id: 'v-remote', title: '🔗 远程目录', action: () => (currentView.value = 'remote') },
  { kind: 'view', id: 'v-knowledge', title: '📊 知识全景', action: () => (currentView.value = 'knowledge') },
  { kind: 'view', id: 'v-settings', title: '⚙ 设置', action: () => (currentView.value = 'settings') },
];

export type CommandPaletteProps = {
  open: boolean;
  onClose: () => void;
};

export function CommandPalette({ open, onClose }: CommandPaletteProps): JSX.Element | null {
  const query = useSignal('');
  const activeIdx = useSignal(0);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // 全文搜索（条目内容）
  const searchResults = useSignal<Array<{ id: string; title: string }>>([]);

  useEffect(() => {
    if (!open) {
      query.value = '';
      activeIdx.value = 0;
      searchResults.value = [];
      return;
    }
    // 打开时 autoFocus
    setTimeout(() => inputRef.current?.focus(), 10);
  }, [open]);

  // 搜索条目（带 debounce 感觉的简单方案：每次 input 立即搜，关键词很短不做）
  useEffect(() => {
    if (!open || !query.value.trim()) {
      searchResults.value = [];
      return;
    }
    const q = query.value.trim();
    if (q.length < 2) return;
    let cancelled = false;
    const id = setTimeout(async () => {
      try {
        const res = await api.get<{ results: Array<{ item_id: string; title: string }> }>(
          `/search?q=${encodeURIComponent(q)}&top_k=10`,
        );
        if (cancelled) return;
        searchResults.value = (res.results ?? []).map((r) => ({
          id: r.item_id,
          title: r.title,
        }));
      } catch {
        searchResults.value = [];
      }
    }, 220);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
  }, [query.value, open]);

  const results = useComputed<SearchResult[]>(() => {
    const q = query.value.trim().toLowerCase();
    const out: SearchResult[] = [];

    // 视图（前缀匹配）
    if (!q || 'views'.includes(q) || q.length < 2) {
      out.push(...VIEW_SHORTCUTS.filter((v) => !q || v.title.toLowerCase().includes(q)));
    }

    // 会话
    const sessions = chatSessions.value
      .filter((s) => !q || s.title.toLowerCase().includes(q))
      .slice(0, 5);
    for (const s of sessions) {
      out.push({
        kind: 'session',
        id: s.id,
        title: s.title || '未命名对话',
        subtitle: '会话',
        action: () => {
          activeSessionId.value = s.id;
          currentView.value = 'chat';
        },
      });
    }

    // 已加载条目（前缀）
    const localItems = items.value
      .filter((it) => q && it.title.toLowerCase().includes(q))
      .slice(0, 5);
    for (const it of localItems) {
      out.push({
        kind: 'item',
        id: it.id,
        title: it.title || '(无标题)',
        subtitle: `条目 · ${it.source_type}`,
        action: () => {
          drawerContent.value = { type: 'reader', itemId: it.id };
        },
      });
    }

    // 服务端搜索结果
    for (const r of searchResults.value) {
      if (out.some((o) => o.kind === 'item' && o.id === r.id)) continue;
      out.push({
        kind: 'item',
        id: r.id,
        title: r.title || '(无标题)',
        subtitle: '条目 · 全文匹配',
        action: () => {
          drawerContent.value = { type: 'reader', itemId: r.id };
        },
      });
    }

    return out.slice(0, 20);
  });

  function select(idx: number) {
    const r = results.value[idx];
    if (!r) return;
    r.action();
    onClose();
  }

  function handleKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      activeIdx.value = Math.min(activeIdx.value + 1, results.value.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      activeIdx.value = Math.max(0, activeIdx.value - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      select(activeIdx.value);
    }
  }

  if (!open) return null;

  return (
    <div
      onClick={onClose}
      className="fade-in"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(36, 43, 55, 0.4)',
        display: 'flex',
        alignItems: 'flex-start',
        justifyContent: 'center',
        paddingTop: '12vh',
        zIndex: 1500,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKey}
        className="modal-in"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        style={{
          width: '90%',
          maxWidth: 560,
          background: 'var(--color-surface)',
          borderRadius: 'var(--radius-xl)',
          boxShadow: 'var(--shadow-xl)',
          overflow: 'hidden',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <input
          ref={inputRef}
          type="text"
          value={query.value}
          onInput={(e) => {
            query.value = e.currentTarget.value;
            activeIdx.value = 0;
          }}
          placeholder="搜索对话、条目、视图…"
          aria-label="Search"
          style={{
            padding: 'var(--space-4) var(--space-5)',
            border: 'none',
            borderBottom: '1px solid var(--color-border)',
            outline: 'none',
            fontSize: 'var(--text-base)',
            color: 'var(--color-text)',
            background: 'transparent',
          }}
        />
        <div style={{ maxHeight: 400, overflow: 'auto' }}>
          {results.value.length === 0 ? (
            <div
              style={{
                padding: 'var(--space-5)',
                fontSize: 'var(--text-sm)',
                color: 'var(--color-text-secondary)',
                textAlign: 'center',
              }}
            >
              没有匹配结果
            </div>
          ) : (
            results.value.map((r, i) => (
              <button
                key={`${r.kind}-${r.id}`}
                type="button"
                onClick={() => select(i)}
                onMouseEnter={() => (activeIdx.value = i)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  width: '100%',
                  padding: 'var(--space-3) var(--space-5)',
                  background:
                    activeIdx.value === i ? 'var(--color-surface-hover)' : 'transparent',
                  border: 'none',
                  color: 'var(--color-text)',
                  fontSize: 'var(--text-sm)',
                  textAlign: 'left',
                  cursor: 'pointer',
                }}
              >
                <span
                  style={{
                    flex: 1,
                    whiteSpace: 'nowrap',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                  }}
                >
                  {r.title}
                </span>
                {r.subtitle && (
                  <span
                    style={{
                      fontSize: 'var(--text-xs)',
                      color: 'var(--color-text-secondary)',
                      marginLeft: 'var(--space-2)',
                    }}
                  >
                    {r.subtitle}
                  </span>
                )}
              </button>
            ))
          )}
        </div>
        <footer
          style={{
            padding: 'var(--space-2) var(--space-5)',
            borderTop: '1px solid var(--color-border)',
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            display: 'flex',
            gap: 'var(--space-4)',
          }}
        >
          <span>↑↓ 导航</span>
          <span>↵ 选择</span>
          <span>Esc 关闭</span>
        </footer>
      </div>
    </div>
  );
}
